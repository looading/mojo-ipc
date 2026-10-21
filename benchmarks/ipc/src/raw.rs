//! Byte-message transport comparison, without Mojom encoding or typed routers.
//!
//! This still measures Rust message allocation/copying, MessagePipeWatcher,
//! native IO/dispatch, and the harness channel. It is not a bare Core benchmark.

use anyhow::{Context, Result, anyhow, bail, ensure};
use ipc::bindings::message_pipe_watcher::{MessagePipeWatcher, ResponseSender};
use ipc::system::{
    message::{ReadableWithHandlesMessage, SendableMessage, WritableMessage},
    message_pipe::MessageEndpoint,
};
use ipc::{ChildProcess, Runtime};
use serde_json::{Value, json};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use super::support::{Diagnostic, Options, event, payload, summarize_ns};

const HANDSHAKE_ID: u64 = u64::MAX;
const HEADER_BYTES: usize = 8;
const SIZES: [usize; 5] = [0, 64, 1024, 65_536, 1_048_576];
const PAYLOAD_BUDGET: usize = 4 * 1024 * 1024;

fn writable(bytes: &[u8]) -> Result<SendableMessage> {
    WritableMessage::new_with_bytes(bytes)
        .map(Into::into)
        .map_err(|error| anyhow!("create raw byte message: {error:?}"))
}

fn message_bytes(message: ReadableWithHandlesMessage) -> Result<Vec<u8>> {
    let (handles, bytes_message) = message
        .read_data()
        .map_err(|error| anyhow!("read raw attached handles: {error:?}"))?;
    ensure!(
        handles.is_empty(),
        "raw benchmark received attached handles"
    );
    let bytes = bytes_message
        .read_bytes()
        .map_err(|error| anyhow!("read raw message bytes: {error:?}"))?;
    Ok(bytes.to_vec())
}

enum ParticipantEvent {
    Disconnected,
    Failed(String),
}

/// Serve byte-for-byte replies until the parent closes its message endpoint.
/// The caller has already initialized the runtime, accepted the invitation,
/// and started the independent child monitor.
pub fn participant(runtime: &Runtime, pipe: MessageEndpoint) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let disconnected = tx.clone();
    let _watcher = MessagePipeWatcher::new_with_runner(
        pipe,
        runtime.dispatch_runner(),
        move |message: ReadableWithHandlesMessage, sender: ResponseSender| {
            let result = (|| -> Result<()> {
                let bytes = message_bytes(message)?;
                ensure!(bytes.len() >= HEADER_BYTES, "short raw request frame");
                let id = u64::from_le_bytes(bytes[..HEADER_BYTES].try_into()?);
                let reply = if id == HANDSHAKE_ID {
                    ensure!(bytes.len() == HEADER_BYTES, "invalid raw handshake frame");
                    let mut reply = Vec::with_capacity(HEADER_BYTES + 4);
                    reply.extend_from_slice(&HANDSHAKE_ID.to_le_bytes());
                    reply.extend_from_slice(&std::process::id().to_le_bytes());
                    writable(&reply)?
                } else {
                    writable(&bytes)?
                };
                ensure!(sender.try_send_response(reply), "raw response send failed");
                Ok(())
            })();
            if let Err(error) = result {
                // Native task callbacks must never unwind. Let the child main
                // thread report the failure and close its endpoint normally.
                let _ = tx.send(ParticipantEvent::Failed(format!("{error:#}")));
            }
        },
        Some(Box::new(move || {
            let _ = disconnected.send(ParticipantEvent::Disconnected);
        })),
        true,
    )
    .context("create raw participant message watcher")?;
    match rx.recv().context("raw participant event channel closed")? {
        ParticipantEvent::Disconnected => Ok(()),
        ParticipantEvent::Failed(error) => bail!("raw participant: {error}"),
    }
}

enum ParentEvent {
    Message { arrived: Instant, bytes: Vec<u8> },
    Disconnected,
    Failed(String),
}

struct Session {
    watcher: Option<MessagePipeWatcher>,
    child: ChildProcess,
    rx: mpsc::Receiver<ParentEvent>,
    timeout: Duration,
    pid: u32,
    next_id: u64,
}

impl Session {
    fn open(runtime: &Runtime, options: &Options) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let parent = format!("--bench-parent={}", std::process::id());
        let watchdog = format!(
            "--bench-watchdog={}",
            options
                .stress_seconds
                .saturating_add(options.sample_ms.saturating_mul(30).div_ceil(1000))
                .saturating_add(600)
        );
        let (child, pipe) = runtime.spawn_worker(
            executable
                .to_str()
                .context("non-UTF8 raw benchmark executable")?,
            &["--bench-raw", &parent, &watchdog],
        )?;
        let (tx, rx) = mpsc::channel();
        let disconnected = tx.clone();
        let watcher = MessagePipeWatcher::new_with_runner(
            pipe,
            runtime.dispatch_runner(),
            move |message: ReadableWithHandlesMessage, _sender: ResponseSender| {
                // End the RTT at handler entry, before parsing/copying or
                // waking the host control thread through mpsc.
                let arrived = Instant::now();
                let event = match message_bytes(message) {
                    Ok(bytes) => ParentEvent::Message { arrived, bytes },
                    Err(error) => ParentEvent::Failed(format!("{error:#}")),
                };
                let _ = tx.send(event);
            },
            Some(Box::new(move || {
                let _ = disconnected.send(ParentEvent::Disconnected);
            })),
            true,
        )
        .context("create raw parent message watcher")?;
        let mut session = Self {
            watcher: Some(watcher),
            child,
            rx,
            timeout: Duration::from_secs(options.timeout_seconds),
            pid: 0,
            next_id: 0,
        };
        session.send(&HANDSHAKE_ID.to_le_bytes())?;
        let (_, reply) = session.receive()?;
        ensure!(
            reply.len() == HEADER_BYTES + 4,
            "short/long raw handshake reply"
        );
        ensure!(
            reply[..HEADER_BYTES] == HANDSHAKE_ID.to_le_bytes(),
            "raw handshake ID mismatch"
        );
        session.pid = u32::from_le_bytes(reply[HEADER_BYTES..].try_into()?);
        ensure!(session.pid != 0, "raw handshake returned an invalid PID");
        event(json!({"event":"worker_started", "pid":session.pid, "transport":"raw_bytes"}));
        Ok(session)
    }

    fn send(&self, bytes: &[u8]) -> Result<()> {
        self.watcher
            .as_ref()
            .context("raw watcher has been closed")?
            .send_message(writable(bytes)?)
            .map_err(|error| anyhow!("send raw message: {error:?}"))
    }

    fn receive(&self) -> Result<(Instant, Vec<u8>)> {
        match self
            .rx
            .recv_timeout(self.timeout)
            .context("raw reply stalled or channel closed")?
        {
            ParentEvent::Message { arrived, bytes } => Ok((arrived, bytes)),
            ParentEvent::Disconnected => bail!("raw peer disconnected unexpectedly"),
            ParentEvent::Failed(error) => bail!("raw response: {error}"),
        }
    }

    fn close(mut self) -> Result<()> {
        self.watcher.take();
        let code = self.child.wait(self.timeout)?;
        event(
            json!({"event":"worker_exited", "pid":self.pid, "exit_code":code, "transport":"raw_bytes"}),
        );
        ensure!(code == 0, "raw child {} exited with code {code}", self.pid);
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Also close on errors. As in the typed harness, the independent child
        // monitor terminates a stalled participant after its parent exits.
        self.watcher.take();
    }
}

#[derive(Default)]
struct RunStats {
    sent: u64,
    completed: u64,
    elapsed: Duration,
    samples: Vec<u64>,
}

fn echo_run(
    session: &mut Session,
    size: usize,
    minimum: u64,
    duration: Duration,
    collect: bool,
) -> Result<RunStats> {
    let data = payload(size);
    let mut frame = Vec::with_capacity(HEADER_BYTES + size);
    frame.extend_from_slice(&0u64.to_le_bytes());
    frame.extend_from_slice(&data);
    let mut stats = RunStats::default();
    let wall = Instant::now();
    while stats.completed < minimum || wall.elapsed() < duration {
        ensure!(session.next_id != HANDSHAKE_ID, "raw request IDs exhausted");
        let id = session.next_id;
        session.next_id += 1;
        frame[..HEADER_BYTES].copy_from_slice(&id.to_le_bytes());
        let started = Instant::now();
        session.send(&frame)?;
        stats.sent += 1;
        // Exactly one request is in flight. Receipt of its callback event is
        // also the drain barrier before admitting the next request.
        let (arrived, reply) = session.receive()?;
        let rtt = arrived
            .checked_duration_since(started)
            .context("raw callback preceded this request")?;
        ensure!(
            reply == frame,
            "raw echo ID/payload mismatch for request {id}"
        );
        stats.completed += 1;
        if collect {
            stats
                .samples
                .push(rtt.as_nanos().min(u64::MAX as u128) as u64);
        }
    }
    stats.elapsed = wall.elapsed();
    ensure!(
        stats.completed == stats.sent,
        "raw requests were not drained"
    );
    ensure!(
        session.rx.try_recv().is_err(),
        "unexpected extra raw response/event"
    );
    Ok(stats)
}

#[inline(never)]
fn diagnostic_roundtrip(session: &mut Session, frame: &mut [u8]) -> Result<u64> {
    ensure!(session.next_id != HANDSHAKE_ID, "raw request IDs exhausted");
    let id = session.next_id;
    session.next_id += 1;
    frame[..HEADER_BYTES].copy_from_slice(&id.to_le_bytes());
    let started = Instant::now();
    session.send(frame)?;
    let (arrived, reply) = session.receive()?;
    let rtt = arrived
        .checked_duration_since(started)
        .context("raw callback preceded this request")?;
    ensure!(
        reply.as_slice() == frame,
        "raw diagnostic ID/payload mismatch for request {id}"
    );
    // The RTT stops at watcher callback entry. Full validation and the final
    // reply allocation drop are still included in the wall-time loop.
    drop(reply);
    Ok(super::tail::nanos(rtt))
}

/// A single uncapped, window-one diagnostic case. It shares the transport and
/// callback-entry timing boundary of the legacy comparison above.
pub(super) fn diagnostic(
    runtime: &Runtime,
    options: &Options,
    diagnostic: Diagnostic,
) -> Result<Value> {
    let mut session = Session::open(runtime, options)?;
    let pid = session.pid;
    let mut frame = Vec::with_capacity(HEADER_BYTES + diagnostic.payload_bytes);
    frame.extend_from_slice(&0u64.to_le_bytes());
    frame.extend_from_slice(&payload(diagnostic.payload_bytes));
    // This is a reproducible layout receipt; request IDs during the actual
    // run are monotonically increasing and checked by the full-frame oracle.
    let canonical_checksum = super::tail::checksum(&frame);
    for _ in 0..options.warmup {
        diagnostic_roundtrip(&mut session, &mut frame)?;
    }
    ensure!(
        session.rx.try_recv().is_err(),
        "unexpected raw event after diagnostic warmup"
    );
    super::tail::ready(options, diagnostic, Some(pid))?;
    let mut samples = super::tail::Samples::new();
    let mut completed = 0;
    let wall = Instant::now();
    while completed < options.iterations
        || wall.elapsed() < Duration::from_millis(options.sample_ms)
    {
        samples.record(diagnostic_roundtrip(&mut session, &mut frame)?);
        completed += 1;
    }
    let elapsed = wall.elapsed();
    ensure!(
        session.rx.try_recv().is_err(),
        "unexpected extra raw response/event after diagnostic drain"
    );
    session.close()?;
    Ok(super::tail::report(
        options,
        diagnostic,
        Some(pid),
        completed,
        elapsed,
        json!({
            "latency":samples.finish(),"rpc_per_second":completed as f64/elapsed.as_secs_f64(),
            "frame_header_bytes":HEADER_BYTES,"frame_bytes":frame.len(),
            "canonical_frame_request_id":0,"canonical_frame_fnv1a64":canonical_checksum,
            "frame_checksum_scope":"8-byte little-endian request ID zero followed by application payload; actual increasing request IDs and full reply contents are checked every iteration",
            "max_inflight":1,"max_pending":null,"final_pending":null,"child_exit_code":0,
            "latency_clock":"raw send API entry to watcher callback entry; excludes frame ID preparation, callback copying, driver wakeup, full reply validation and final reply drop",
            "throughput_clock":"serial loop including frame ID preparation, native message allocation/copying, callback copying, mpsc wakeup, full reply validation, final reply drop and histogram/reservoir recording",
            "scope":"raw Rust safe message APIs, MessagePipeWatcher, native IO/dispatch and harness mpsc; no Mojom codec/typed router; not bare Mojo Core"
        }),
    ))
}

/// Compare serial raw-byte RTTs with typed RPCs using the same native runtime,
/// participant launch helper, dispatch sequence, and control-thread channel.
pub fn benchmark(runtime: &Runtime, options: &Options) -> Result<Vec<Value>> {
    let mut session = Session::open(runtime, options)?;
    let mut rows = Vec::new();
    for size in SIZES
        .into_iter()
        .filter(|size| options.legacy_sizes.contains(size))
    {
        let warmup = options
            .warmup
            .min((PAYLOAD_BUDGET / size.max(1)).max(4) as u64);
        let minimum = options
            .iterations
            .min((PAYLOAD_BUDGET / size.max(1)).max(16) as u64);
        event(
            json!({"event":"case_started", "name":"raw_bytes_serial", "payload_bytes":size, "window":1}),
        );
        if warmup != 0 {
            echo_run(&mut session, size, warmup, Duration::ZERO, false)?;
        }
        let mut stats = echo_run(
            &mut session,
            size,
            minimum,
            Duration::from_millis(options.sample_ms),
            true,
        )?;
        let seconds = stats.elapsed.as_secs_f64();
        let rate = stats.completed as f64 / seconds;
        let row = json!({
            "name":"raw_bytes_serial", "payload_bytes":size, "frame_header_bytes":HEADER_BYTES,
            "window":1, "warmup_requests":warmup, "requested_warmup":options.warmup,
            "requested_iterations":options.iterations, "effective_minimum_requests":minimum,
            "sample_ms":options.sample_ms, "sent":stats.sent, "completed":stats.completed,
            "elapsed_seconds":seconds, "rpc_per_second":rate,
            "payload_one_direction_mib_per_second":rate * size as f64 / 1048576.0,
            "payload_both_directions_mib_per_second":2.0 * rate * size as f64 / 1048576.0,
            "max_inflight":1, "max_pending":null, "final_pending":null,
            "latency":summarize_ns(&mut stats.samples),
            "scope":"raw byte messages through Rust safe message APIs, MessagePipeWatcher, native IO/dispatch and mpsc; not bare Mojo Core",
            "latency_clock":"send API entry to watcher callback entry; excludes frame preparation and reply validation",
            "throughput_clock":"wall time including native message preparation, reply copying/validation, mpsc driving and drain"
        });
        event(json!({"event":"case_finished", "result":row}));
        rows.push(row);
    }
    session.close()?;
    Ok(rows)
}
