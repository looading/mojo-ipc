//! Actual two-process typed IPC. Run an optimized build with a trusted native SDK.
#![allow(non_snake_case)]
use anyhow::{Context, Result, bail, ensure};
use ipc::{PendingReceiver, PendingRemote, Remote, Role, Runtime};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
mod codec;
mod complex;
mod complex_cases;
mod nullable;
mod primitive;
mod raw;
mod support;
mod tail;
mod protocol {
    include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
}
use protocol::bench::Bench;
use support::{Options, checkpoint, event, payload, summarize_ns};

const SIZES: [usize; 5] = [0, 64, 1024, 65_536, 1_048_576];

#[derive(Default)]
struct Service {
    expected: Vec<u8>,
    received: u64,
    bytes: u64,
    valid: bool,
    held: Vec<Box<dyn FnOnce(u64) + Send>>,
}
impl Bench for Service {
    primitive::service_methods!();
    nullable::service_methods!();
    complex::service_methods!();
    fn Info(&mut self, callback: impl FnOnce(u32) + Send + 'static) {
        callback(std::process::id());
    }
    fn Echo(
        &mut self,
        id: u64,
        data: Vec<u8>,
        callback: impl FnOnce(u64, Vec<u8>) + Send + 'static,
    ) {
        callback(id, data);
    }
    fn Reset(&mut self, size: u32, callback: impl FnOnce() + Send + 'static) {
        self.expected = payload(size as usize);
        self.received = 0;
        self.bytes = 0;
        self.valid = true;
        callback();
    }
    fn Push(&mut self, id: u64, data: Vec<u8>) {
        self.valid &= id == self.received && data == self.expected;
        self.received += 1;
        self.bytes += data.len() as u64;
    }
    fn Inspect(&mut self, callback: impl FnOnce(u64, u64, bool, u64) + Send + 'static) {
        callback(
            self.received,
            self.bytes,
            self.valid,
            self.held.len() as u64,
        );
    }
    fn Hold(&mut self, _id: u64, callback: impl FnOnce(u64) + Send + 'static) {
        self.held.push(Box::new(callback));
    }
    fn Crash(&mut self) {
        // Intentional failure of this harness's own child, after an application barrier.
        std::process::exit(17);
    }
}
ipc::register_mojom_state_object_impls!(impl Bench for Service);

fn child_monitor(parent: u32, watchdog: Duration) {
    std::thread::spawn(move || {
        let started = Instant::now();
        #[cfg(windows)]
        let process = unsafe { OpenProcess(0x0010_0000, 0, parent) };
        loop {
            std::thread::sleep(Duration::from_secs(1));
            #[cfg(unix)]
            let alive = unsafe { getppid() as u32 == parent };
            #[cfg(windows)]
            let alive = !process.is_null() && unsafe { WaitForSingleObject(process, 0) } == 258;
            #[cfg(not(any(unix, windows)))]
            let alive = true;
            if !alive || started.elapsed() >= watchdog {
                eprintln!("benchmark child watchdog: parent lost or total deadline exceeded");
                std::process::exit(124);
            }
        }
    });
}
#[cfg(unix)]
unsafe extern "C" {
    fn getppid() -> i32;
}
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, timeout: u32) -> u32;
}

fn participant(library: &Path, token: &str) -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let value = |prefix: &str| args.iter().find_map(|s| s.strip_prefix(prefix));
    let parent: u32 = value("--bench-parent=")
        .context("missing harness parent")?
        .parse()?;
    let watchdog: u64 = value("--bench-watchdog=")
        .context("missing watchdog")?
        .parse()?;
    // Independent of Mojo/dispatch; also works if initialization or native dispatch stalls.
    child_monitor(parent, Duration::from_secs(watchdog));
    // SAFETY: this benchmark is launched with an operator-supplied, trusted SDK image.
    let runtime = unsafe { Runtime::load(library, Role::Participant)? };
    // SAFETY: only the SDK spawn helper starts this branch, transferring this token once.
    let pipe = unsafe { runtime.accept_inherited(token)? };
    if args.iter().any(|arg| arg == "--bench-raw") {
        return raw::participant(&runtime, pipe);
    }
    let (tx, rx) = mpsc::channel();
    let _receiver = PendingReceiver::<dyn Bench>::new(pipe).bind_with_options(
        Service::default(),
        Some(runtime.dispatch_runner()),
        Some(Box::new(move || {
            let _ = tx.send(());
        })),
    );
    rx.recv().context("child disconnect notification")?;
    Ok(())
}

enum Event {
    Info(u32),
    Echo {
        id: u64,
        valid: bool,
        ns: u64,
    },
    Reset,
    Inspect {
        received: u64,
        bytes: u64,
        valid: bool,
        held: u64,
    },
    Disconnected,
    UnexpectedSuccess,
}
struct Session {
    remote: Option<Remote<dyn Bench>>,
    child: ipc::ChildProcess,
    pid: u32,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    timeout: Duration,
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
                .saturating_add(options.complex_watchdog_seconds())
                .saturating_add(600)
        );
        let (child, pipe) = runtime.spawn_worker(
            executable.to_str().context("non-UTF8 executable")?,
            &[&parent, &watchdog],
        )?;
        let (tx, rx) = mpsc::channel();
        let disconnected = tx.clone();
        let mut remote = PendingRemote::<dyn Bench>::new(pipe).bind_with_options(
            Some(runtime.dispatch_runner()),
            Some(Box::new(move || {
                let _ = disconnected.send(Event::Disconnected);
            })),
        );
        let reply = tx.clone();
        remote.Info(move |pid| {
            let _ = reply.send(Event::Info(pid));
        });
        let mut session = Self {
            remote: Some(remote),
            child,
            pid: 0,
            tx,
            rx,
            timeout: support::timeout(options),
        };
        match session.receive()? {
            Event::Info(pid) => session.pid = pid,
            _ => bail!("expected child Info handshake"),
        }
        event(json!({"event":"worker_started", "pid":session.pid}));
        Ok(session)
    }
    fn remote(&mut self) -> &mut Remote<dyn Bench> {
        self.remote.as_mut().unwrap()
    }
    fn receive(&self) -> Result<Event> {
        self.rx
            .recv_timeout(self.timeout)
            .context("RPC timeout/channel closed")
    }
    fn pending(&self) -> usize {
        self.remote
            .as_ref()
            .map_or(0, |r| r.pending_response_count())
    }
    fn reset(&mut self, size: usize) -> Result<()> {
        let tx = self.tx.clone();
        self.remote().Reset(size as u32, move || {
            let _ = tx.send(Event::Reset);
        });
        ensure!(
            matches!(self.receive()?, Event::Reset),
            "reset failed/disconnected"
        );
        Ok(())
    }
    fn inspect(&mut self) -> Result<(u64, u64, bool, u64)> {
        let tx = self.tx.clone();
        self.remote().Inspect(move |received, bytes, valid, held| {
            let _ = tx.send(Event::Inspect {
                received,
                bytes,
                valid,
                held,
            });
        });
        match self.receive()? {
            Event::Inspect {
                received,
                bytes,
                valid,
                held,
            } => Ok((received, bytes, valid, held)),
            _ => bail!("inspect failed/disconnected"),
        }
    }
    fn send_echo(&mut self, id: u64, expected: &Arc<Vec<u8>>) {
        let data = (**expected).clone();
        let expected = expected.clone();
        let tx = self.tx.clone();
        // Payload preparation is outside this RTT timer, but inside wall-clock throughput.
        let start = Instant::now();
        self.remote().Echo(id, data, move |actual, returned| {
            // Capture callback arrival before validation or the control-thread mpsc wakeup.
            let ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let valid = actual == id && returned == *expected;
            let _ = tx.send(Event::Echo { id, valid, ns });
        });
    }
    fn close(mut self, expected_code: i32) -> Result<()> {
        self.remote.take(); // own-end drop does not call our disconnect handler
        let code = self.child.wait(self.timeout)?;
        event(json!({"event":"worker_exited", "pid":self.pid,"exit_code":code}));
        ensure!(
            code == expected_code,
            "child {} exit {code}, expected {expected_code}",
            self.pid
        );
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // Close even on errors. The independent child parent-monitor handles a stalled pipe.
        self.remote.take();
    }
}

#[derive(Default)]
struct RunStats {
    sent: u64,
    completed: u64,
    max_inflight: u64,
    max_pending: usize,
    max_ns: u64,
    samples: Vec<u64>,
    elapsed: Duration,
}
fn echo_run(
    session: &mut Session,
    size: usize,
    window: usize,
    minimum: u64,
    duration: Duration,
    collect: bool,
) -> Result<RunStats> {
    let expected = Arc::new(payload(size));
    let mut stats = RunStats::default();
    let start = Instant::now();
    let mut admission = true;
    let mut drain_deadline = None;
    // A ring verifies every callback ID exactly once; at most window requests exist.
    let mut slots = vec![None; window];
    loop {
        if admission && stats.sent >= minimum && start.elapsed() >= duration {
            admission = false;
            drain_deadline = Some(Instant::now() + session.timeout);
        }
        while admission && stats.sent - stats.completed < window as u64 {
            let id = stats.sent;
            let slot = id as usize % window;
            ensure!(slots[slot].is_none(), "inflight ring collision");
            slots[slot] = Some(id);
            session.send_echo(id, &expected);
            stats.sent += 1;
            stats.max_inflight = stats.max_inflight.max(stats.sent - stats.completed);
            stats.max_pending = stats.max_pending.max(session.pending());
            if stats.sent >= minimum && start.elapsed() >= duration {
                admission = false;
                drain_deadline = Some(Instant::now() + session.timeout);
                break;
            }
        }
        if stats.completed == stats.sent && !admission {
            break;
        }
        let wait = if let Some(deadline) = drain_deadline {
            deadline
                .checked_duration_since(Instant::now())
                .context("absolute drain deadline exceeded")?
        } else {
            session.timeout
        };
        match session
            .rx
            .recv_timeout(wait)
            .context("echo stalled or drain timed out")?
        {
            Event::Echo { id, valid, ns } => {
                ensure!(valid, "corrupt echo payload or mismatched ID at {id}");
                let slot = id as usize % window;
                ensure!(
                    slots[slot] == Some(id),
                    "duplicate/unexpected response {id}"
                );
                slots[slot] = None;
                stats.completed += 1;
                stats.max_ns = stats.max_ns.max(ns);
                if collect {
                    stats.samples.push(ns);
                }
            }
            Event::Disconnected => bail!("unexpected peer disconnect"),
            _ => bail!("unexpected event during echo"),
        }
    }
    stats.elapsed = start.elapsed();
    ensure!(
        stats.sent == stats.completed && session.pending() == 0,
        "callbacks not drained"
    );
    Ok(stats)
}

fn describe(stats: &mut RunStats, name: &str, size: usize, window: usize, warmup: u64) -> Value {
    let seconds = stats.elapsed.as_secs_f64();
    let rate = stats.completed as f64 / seconds;
    json!({"name":name,"payload_bytes":size,"window":window,"warmup_requests":warmup,
        "sent":stats.sent,"completed":stats.completed,"elapsed_seconds":seconds,
        "rpc_per_second":rate,"payload_one_direction_mib_per_second":rate * size as f64 / 1048576.0,
        "payload_both_directions_mib_per_second":2.0 * rate * size as f64 / 1048576.0,
        "max_inflight":stats.max_inflight,"max_pending":stats.max_pending,"final_pending":0,
        "latency":summarize_ns(&mut stats.samples),"max_rtt_us":stats.max_ns as f64 / 1000.0})
}

fn oneway(
    session: &mut Session,
    size: usize,
    window: usize,
    minimum: u64,
    duration: Duration,
) -> Result<Value> {
    session.reset(size)?;
    let data = payload(size);
    let mut count = 0u64;
    let start = Instant::now();
    while count < minimum || start.elapsed() < duration {
        for _ in 0..window {
            session.remote().Push(count, data.clone());
            count += 1;
        }
        // A typed application barrier on the same ordered interface, not assumed OS flush.
        let (received, bytes, valid, _) = session.inspect()?;
        ensure!(
            received == count && bytes == count * size as u64 && valid,
            "oneway loss/corruption/order failure"
        );
    }
    let seconds = start.elapsed().as_secs_f64();
    ensure!(session.pending() == 0, "oneway barrier callback pending");
    Ok(
        json!({"name":"oneway_batched_barrier","payload_bytes":size,"window":window,
        "sent":count,"completed":count,"elapsed_seconds":seconds,"messages_per_second":count as f64 / seconds,
        "payload_one_direction_mib_per_second":count as f64 * size as f64 / seconds / 1048576.0,
        "barriers":count / window as u64,"valid":true,"final_pending":0}),
    )
}

struct DropProbe(Arc<AtomicU64>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn crash_check(runtime: &Runtime, options: &Options) -> Result<Value> {
    let mut session = Session::open(runtime, options)?;
    let held = 64u64;
    let released = Arc::new(AtomicU64::new(0));
    let succeeded = Arc::new(AtomicU64::new(0));
    for id in 0..held {
        let probe = DropProbe(released.clone());
        let success = succeeded.clone();
        let tx = session.tx.clone();
        session.remote().Hold(id, move |_| {
            drop(probe);
            success.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(Event::UnexpectedSuccess);
        });
    }
    ensure!(
        session.inspect()?.3 == held,
        "child did not hold all requests"
    );
    ensure!(
        session.pending() == held as usize,
        "held request count incorrect"
    );
    session.remote().Crash();
    ensure!(
        matches!(session.receive()?, Event::Disconnected),
        "missing crash disconnect"
    );
    ensure!(session.pending() == 0, "crash leaked pending requests");
    ensure!(
        released.load(Ordering::SeqCst) == held,
        "callbacks not released exactly once"
    );
    ensure!(
        succeeded.load(Ordering::SeqCst) == 0,
        "crash incorrectly reported success"
    );
    session.close(17)?;
    Ok(
        json!({"held_requests":held,"released_callbacks":held,"success_callbacks":0,"final_pending":0,"expected_exit_code":17,"passed":true}),
    )
}

fn legacy_benchmark(runtime: &Runtime, options: &Options) -> Result<Value> {
    let raw_rows = if options.legacy_phase("raw") {
        let rows = raw::benchmark(runtime, options)?;
        checkpoint("after_raw_transport", 0)?;
        rows
    } else {
        vec![]
    };
    let needs_session = ["correctness", "typed", "oneway"]
        .iter()
        .any(|phase| options.legacy_phase(phase));
    let mut session = if needs_session {
        Some(Session::open(runtime, options)?)
    } else {
        None
    };
    let mut correctness = 0;
    if options.legacy_phase("correctness") {
        for size in [0, 1, 7, 8, 9, 63, 64, 65] {
            echo_run(
                session.as_mut().unwrap(),
                size,
                1,
                16,
                Duration::ZERO,
                false,
            )?;
            correctness += 16;
        }
    }
    let mut rows = Vec::new();
    let mut windows = vec![1, options.window.min(8), options.window];
    windows.sort_unstable();
    windows.dedup();
    for size in SIZES
        .into_iter()
        .filter(|size| options.legacy_sizes.contains(size))
    {
        let warmup = options.warmup.min((512 * 1024 / size.max(1)).max(2) as u64);
        let minimum = options
            .iterations
            .min((1024 * 1024 / size.max(1)).max(8) as u64);
        let mut actual_windows: Vec<_> = windows
            .iter()
            .map(|w| (*w).min((8 * 1024 * 1024 / size.max(1)).max(1)))
            .collect();
        actual_windows.sort_unstable();
        actual_windows.dedup();
        if options.legacy_phase("typed") {
            for window in actual_windows {
                let name = if window == 1 {
                    "echo_serial"
                } else {
                    "echo_pipeline"
                };
                event(
                    json!({"event":"case_started","name":name,"payload_bytes":size,"window":window}),
                );
                if warmup > 0 {
                    echo_run(
                        session.as_mut().unwrap(),
                        size,
                        window,
                        warmup,
                        Duration::ZERO,
                        false,
                    )?;
                }
                let mut stats = echo_run(
                    session.as_mut().unwrap(),
                    size,
                    window,
                    minimum,
                    Duration::from_millis(options.sample_ms),
                    true,
                )?;
                let mut row = describe(&mut stats, name, size, window, warmup);
                row["requested_iterations"] = json!(options.iterations);
                row["effective_minimum_requests"] = json!(minimum);
                row["requested_warmup"] = json!(options.warmup);
                row["small_sample_tail_caution"] = json!(stats.samples.len() < 100);
                event(json!({"event":"case_finished","result":row}));
                rows.push(row);
            }
        }
        if options.legacy_phase("oneway") {
            let window = options.window.min((8 * 1024 * 1024 / size.max(1)).max(1));
            event(
                json!({"event":"case_started","name":"oneway_batched_barrier","payload_bytes":size,"window":window}),
            );
            let warmup_completed = if warmup > 0 {
                oneway(
                    session.as_mut().unwrap(),
                    size,
                    window,
                    warmup,
                    Duration::ZERO,
                )?["completed"]
                    .as_u64()
                    .unwrap()
            } else {
                0
            };
            let mut row = oneway(
                session.as_mut().unwrap(),
                size,
                window,
                minimum,
                Duration::from_millis(options.sample_ms),
            )?;
            row["warmup_messages"] = json!(warmup_completed);
            row["requested_iterations"] = json!(options.iterations);
            row["effective_minimum_requests"] = json!(minimum);
            row["requested_warmup"] = json!(options.warmup);
            event(json!({"event":"case_finished","result":row}));
            rows.push(row);
        }
    }
    if let Some(session) = session {
        session.close(0)?;
        checkpoint("after_performance", 0)?;
    }
    let stress_row = if options.legacy_phase("stress") && options.stress_seconds > 0 {
        let mut stress = Session::open(runtime, options)?;
        // Bring this fresh session to steady operation before the measured long run.
        if options.warmup > 0 {
            echo_run(
                &mut stress,
                1024,
                options.window,
                options.warmup,
                Duration::ZERO,
                false,
            )?;
        }
        event(
            json!({"event":"case_started","name":"stability_stress","seconds":options.stress_seconds}),
        );
        let mut stats = echo_run(
            &mut stress,
            1024,
            options.window,
            1,
            Duration::from_secs(options.stress_seconds),
            false,
        )?;
        let stress_row = describe(
            &mut stats,
            "stability_stress",
            1024,
            options.window,
            options.warmup,
        );
        event(json!({"event":"case_finished","result":stress_row}));
        stress.close(0)?;
        checkpoint("after_stress", 0)?;
        stress_row
    } else {
        json!({"enabled":false})
    };
    let mut cycles = Vec::new();
    let churn_cycles = if options.legacy_phase("churn") {
        options.churn
    } else {
        0
    };
    for iteration in 1..=churn_cycles {
        let start = Instant::now();
        let mut cycle = Session::open(runtime, options)?;
        echo_run(&mut cycle, 64, 1, 8, Duration::ZERO, false)?;
        cycle.close(0)?;
        cycles.push(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
        if iteration % 10 == 0 || iteration == churn_cycles {
            checkpoint("churn", iteration)?;
        }
    }
    let crash = if options.legacy_phase("crash") {
        let result = crash_check(runtime, options)?;
        checkpoint("after_crash", churn_cycles)?;
        result
    } else {
        json!({"enabled":false})
    };
    Ok(
        json!({"correctness_boundary_echoes":correctness,"performance":rows,"stress":stress_row,
        "raw_transport":raw_rows,"churn":{"enabled":churn_cycles>0,"cycles":churn_cycles,"passed":true,"cycle_latency":summarize_ns(&mut cycles)},
        "crash":crash}),
    )
}

fn benchmark(library: &Path, options: &Options) -> Result<Value> {
    // All pure-codec cases finish before loading the native SDK.
    let codec_rows = if options.matrix("legacy") && options.legacy_phase("codec") {
        codec::benchmark(options)?
    } else {
        vec![]
    };
    let primitive_codec = if options.matrix("primitive") {
        primitive::codec_benchmark()?
    } else {
        vec![]
    };
    let nullable_codec = if options.matrix("nullable") {
        nullable::codec_benchmark()?
    } else {
        vec![]
    };
    let complex_codec = if options.matrix("complex") {
        complex::codec_benchmark(options)?
    } else {
        vec![]
    };
    // SAFETY: the operator supplies the genuine pinned native SDK image.
    let runtime = unsafe { Runtime::load(library, Role::Broker)? };
    let legacy = if options.matrix("legacy") {
        legacy_benchmark(&runtime, options)?
    } else {
        json!({"correctness_boundary_echoes":0,"performance":[],"stress":{"enabled":false},"raw_transport":[],
            "churn":{"enabled":false,"cycles":0},"crash":{"enabled":false}})
    };
    let mut report = json!({"ok":true,"schema_version":1,"sdk_version":env!("CARGO_PKG_VERSION"),
        "chromium_revision":"bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd",
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"optimized":!cfg!(debug_assertions),
        "compiler":env!("BENCH_RUSTC"),"target":env!("BENCH_TARGET"),"profile":env!("BENCH_PROFILE"),"opt_level":env!("BENCH_OPT_LEVEL"),
        "scope":"typed Rust SDK end-to-end; includes generated protocol code, encoding, native dispatch and harness overhead",
        "configuration":{"iterations":options.iterations,"warmup":options.warmup,"sample_ms":options.sample_ms,
            "stress_seconds":options.stress_seconds,"churn":options.churn,"window":options.window,"timeout_seconds":options.timeout_seconds,
            "matrices":options.matrices,"legacy_sizes":options.legacy_sizes,"legacy_phases":options.legacy_phases},
        "latency_clock":"method call to callback entry; excludes payload preparation and callback validation",
        "throughput_clock":"wall time including payload preparation, validation, mpsc driving and drain",
        "codec_only":codec_rows,"validation_errors":0});
    report
        .as_object_mut()
        .unwrap()
        .extend(legacy.as_object().unwrap().clone());
    if options.matrix("primitive") {
        let typed = primitive::typed_benchmark(&runtime, options)?;
        checkpoint("after_primitive_arrays", 0)?;
        report["primitive_arrays"] = primitive::results(primitive_codec, typed);
    }
    if options.matrix("nullable") {
        let typed = nullable::typed_benchmark(&runtime, options)?;
        checkpoint("after_nullable_arrays", 0)?;
        report["nullable_arrays"] = nullable::results(nullable_codec, typed);
    }
    if options.matrix("complex") {
        let typed = complex::typed_benchmark(&runtime, options)?;
        checkpoint("after_complex_types", 0)?;
        report["complex_types"] = complex::results(options, complex_codec, typed);
    }
    Ok(report)
}

fn main() {
    if let Err(error) = run() {
        event(json!({"event":"failed","error":format!("{error:#}")}));
        eprintln!("benchmark failed: {error:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    if let Some(token) = Runtime::inherited_token() {
        let library =
            std::env::var_os("MOJO_RUNTIME_LIBRARY").context("missing child runtime image")?;
        return participant(Path::new(&library), &token);
    }
    let Some(options) = Options::parse()? else {
        return Ok(());
    };
    ensure!(
        !cfg!(debug_assertions),
        "use cargo run --release -p mojo-ipc-bench"
    );
    let library = std::env::var_os("MOJO_RUNTIME_LIBRARY");
    let report = if options.diagnostic.is_some() {
        tail::benchmark(library.as_deref().map(Path::new), &options)?
    } else {
        benchmark(
            Path::new(
                library
                    .as_deref()
                    .context("set MOJO_RUNTIME_LIBRARY to a trusted native SDK image")?,
            ),
            &options,
        )?
    };
    if let Some(parent) = options
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&options.output, serde_json::to_vec_pretty(&report)?)?;
    event(json!({"event":"finished","ok":true,"output":options.output}));
    Ok(())
}
