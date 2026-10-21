//! Selected byte/RTT diagnostics, separate from the default legacy matrix.
//! All returned bytes/IDs are checked. Quantiles use a bounded sample reservoir;
//! the logarithmic histogram and min/mean/max cover every observed operation.
use super::{
    Event, Session,
    protocol::bench::{Bench, Bench_Echo_Params},
    support::{self, Diagnostic, DiagnosticOperation, Options},
};
use anyhow::{Context, Result, bail, ensure};
use ipc::__private::{mojom_value_parser as codec, mojom_value_parser_core::UntypedHandle};
use ipc::{Role, Runtime};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    io::{self, BufRead},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

const CAPACITY: usize = 100_000;
const FIXTURE_ID: u64 = 0x1234_5678_9abc_def0;
type Encoded = (Vec<u8>, Vec<UntypedHandle>, u64);

pub(super) struct Samples {
    values: Vec<u64>,
    count: u64,
    sum: u128,
    minimum: u64,
    maximum: u64,
    histogram: [u64; 65],
    random: u64,
}
impl Samples {
    pub(super) fn new() -> Self {
        Self {
            values: Vec::with_capacity(CAPACITY),
            count: 0,
            sum: 0,
            minimum: u64::MAX,
            maximum: 0,
            histogram: [0; 65],
            random: 0x243f_6a88_85a3_08d3,
        }
    }
    pub(super) fn record(&mut self, value: u64) {
        self.count += 1;
        self.sum += u128::from(value);
        self.minimum = self.minimum.min(value);
        self.maximum = self.maximum.max(value);
        self.histogram[(u64::BITS - value.leading_zeros()) as usize] += 1;
        if self.values.len() < CAPACITY {
            self.values.push(value);
            return;
        }
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        let index = self.random.wrapping_mul(0x2545_f491_4f6c_dd1d) % self.count;
        if index < CAPACITY as u64 {
            self.values[index as usize] = value;
        }
    }
    pub(super) fn finish(mut self) -> Value {
        if self.count == 0 {
            return Value::Null;
        }
        self.values.sort_unstable();
        let percentile =
            |p: usize| self.values[(self.values.len() * p).div_ceil(100) - 1] as f64 / 1000.0;
        let bins = self
            .histogram
            .iter()
            .enumerate()
            .map(|(index, &count)| {
                let lower = if index == 0 { 0 } else { 1_u64 << (index - 1) };
                let upper = 1_u64.checked_shl(index as u32);
                json!({"lower_inclusive_ns":lower,"upper_exclusive_ns":upper,"count":count})
            })
            .collect::<Vec<_>>();
        json!({"observations":self.count,"samples":self.values.len(),"capacity":CAPACITY,
            "sampling":if self.count>CAPACITY as u64 {"reservoir"}else{"all"},
            "min_us":self.minimum as f64/1000.0,"mean_us":self.sum as f64/self.count as f64/1000.0,
            "p50_us":percentile(50),"p90_us":percentile(90),"p95_us":percentile(95),"p99_us":percentile(99),
            "max_us":self.maximum as f64/1000.0,
            "quantiles":"nearest-rank of stored samples; never estimated from histogram buckets",
            "histogram":{"scope":"all observations, base2 nanosecond buckets; final null upper bound means 2^64", "bins":bins}})
    }
}
pub(super) fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}
pub(super) fn checksum(bytes: &[u8]) -> String {
    let value = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    });
    format!("{value:016x}")
}

pub(super) fn ready(options: &Options, diagnostic: Diagnostic, child: Option<u32>) -> Result<()> {
    support::event(
        json!({"event":"probe_ready","parent_pid":std::process::id(),"child_pid":child,
        "operation":diagnostic.operation.name(),"payload_bytes":diagnostic.payload_bytes,
        "start_gate":diagnostic.start_gate,"minimum_sample_ms":options.sample_ms,
        "minimum_completed":options.iterations,"warmup_completed":options.warmup}),
    );
    if diagnostic.start_gate {
        let mut input = String::new();
        ensure!(
            io::stdin().lock().read_line(&mut input)? > 0
                && input.trim_end_matches(['\r', '\n']) == "START",
            "expected START after probe_ready"
        );
    }
    support::event(
        json!({"event":"probe_started","parent_pid":std::process::id(),"child_pid":child,
        "operation":diagnostic.operation.name(),"payload_bytes":diagnostic.payload_bytes}),
    );
    Ok(())
}

pub(super) fn report(
    options: &Options,
    diagnostic: Diagnostic,
    child: Option<u32>,
    completed: u64,
    elapsed: Duration,
    mut metrics: Value,
) -> Value {
    let seconds = elapsed.as_secs_f64();
    let mut value = json!({"ok":true,"schema_version":1,"diagnostic_only":true,"probe":"legacy_tail",
        "operation":diagnostic.operation.name(),"payload_bytes":diagnostic.payload_bytes,
        "parent_pid":std::process::id(),"child_pid":child,"window":1,
        "sdk_version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "optimized":!cfg!(debug_assertions),
        "compiler":env!("BENCH_RUSTC"),"target":env!("BENCH_TARGET"),"profile":env!("BENCH_PROFILE"),
        "opt_level":env!("BENCH_OPT_LEVEL"),"chromium_revision":"bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd",
        "native_runtime_loaded":child.is_some(),"completed":completed,"sent":completed,
        "elapsed_seconds":seconds,"operations_per_second":completed as f64/seconds,
        "application_payload_one_direction_mib_per_second":completed as f64*diagnostic.payload_bytes as f64/seconds/1_048_576.0,
        "configuration":{"minimum_sample_ms":options.sample_ms,"minimum_completed":options.iterations,
            "warmup_count":options.warmup,"timeout_seconds":options.timeout_seconds,"start_gate":diagnostic.start_gate,"window":1},
        "payload_checksum":{"algorithm":"FNV-1a64, noncryptographic deterministic reference; every output also compared byte-for-byte",
            "value":checksum(&support::payload(diagnostic.payload_bytes))},
        "small_sample_tail_caution":completed<100,
        "sampling_scope":"bounded per-run reservoir for quantiles, all observations for histogram/min/mean/max; profiling is an independent diagnostic run"});
    value
        .as_object_mut()
        .unwrap()
        .append(metrics.as_object_mut().unwrap());
    value
}

#[inline(never)]
fn typed_roundtrip(session: &mut Session, expected: &Arc<Vec<u8>>, id: u64) -> Result<u64> {
    let data = (**expected).clone();
    let verify = Arc::clone(expected);
    let tx = session.tx.clone();
    let started = Instant::now();
    session.remote().Echo(id, data, move |actual, returned| {
        // Exactly the original typed callback-entry RTT boundary. Full
        // validation and destruction finish before publishing to the driver.
        let ns = nanos(started.elapsed());
        let valid = actual == id && returned == *verify;
        drop(returned);
        drop(verify);
        let _ = tx.send(Event::Echo {
            id: actual,
            valid,
            ns,
        });
    });
    match session.receive()? {
        Event::Echo {
            id: actual,
            valid,
            ns,
        } => {
            ensure!(
                actual == id && valid,
                "typed diagnostic ID/payload mismatch"
            );
            Ok(ns)
        }
        Event::Disconnected => bail!("typed diagnostic disconnected"),
        _ => bail!("unexpected typed diagnostic event"),
    }
}
fn typed(runtime: &Runtime, options: &Options, diagnostic: Diagnostic) -> Result<Value> {
    let mut session = Session::open(runtime, options)?;
    let pid = session.pid;
    let expected = Arc::new(support::payload(diagnostic.payload_bytes));
    for id in 0..options.warmup {
        typed_roundtrip(&mut session, &expected, id)?;
    }
    ensure!(session.pending() == 0, "typed warmup pending callbacks");
    ready(options, diagnostic, Some(pid))?;
    let mut samples = Samples::new();
    let mut completed = 0;
    let wall = Instant::now();
    while completed < options.iterations
        || wall.elapsed() < Duration::from_millis(options.sample_ms)
    {
        let ns = typed_roundtrip(&mut session, &expected, completed)?;
        completed += 1;
        samples.record(ns);
    }
    let elapsed = wall.elapsed();
    ensure!(session.pending() == 0, "typed diagnostic pending callbacks");
    session.close(0)?;
    let fixture = profile_encode(Bench_Echo_Params {
        id: FIXTURE_ID,
        data: (*expected).clone(),
    });
    ensure!(
        fixture.1.is_empty() && fixture.2 == 0 && fixture.0.len() == 32 + diagnostic.payload_bytes,
        "typed preflight fixture size/handles/IDs mismatch"
    );
    Ok(report(
        options,
        diagnostic,
        Some(pid),
        completed,
        elapsed,
        json!({"latency":samples.finish(),
        "final_pending":0,"max_inflight":1,"rpc_per_second":completed as f64/elapsed.as_secs_f64(),
        "preflight_encoded_body_bytes":fixture.0.len(),"preflight_encoded_body_fnv1a64":checksum(&fixture.0),
        "preflight_request_id":FIXTURE_ID,"child_exit_code":0,
        "latency_clock":"typed method entry to callback entry; input preparation, content validation and final output drop excluded",
        "throughput_clock":"dedicated diagnostic serial loop including input clone, callback full byte/ID validation, output drop before mpsc publication, driver wakeup and histogram/reservoir recording; unlike the default legacy loop, this deliberately completes returned-buffer destruction before publishing its callback event, so compare only runs of the same harness",
        "scope":"complete Rust typed SDK path, same generated Echo and native dispatch; not bare Mojo Core"}),
    ))
}

#[inline(never)]
fn profile_encode(input: Bench_Echo_Params) -> Encoded {
    codec::serialize(black_box(input), &())
}
#[inline(never)]
fn profile_decode(bytes: &[u8]) -> Result<Bench_Echo_Params> {
    codec::deserialize_exact::<(), Bench_Echo_Params>(black_box(bytes), &mut [], 0, &())
        .context("tail probe decode")
}
fn validate(actual: &Bench_Echo_Params, expected: &[u8]) -> Result<()> {
    ensure!(
        actual.id == FIXTURE_ID && actual.data.as_slice() == expected,
        "codec diagnostic ID/payload mismatch"
    );
    Ok(())
}
fn codec_iteration(
    expected: &[u8],
    canonical: &[u8],
    operation: DiagnosticOperation,
) -> Result<(Option<u64>, Option<u64>)> {
    let mut encode_ns = None;
    let mut decode_ns = None;
    let encoded = if operation != DiagnosticOperation::CodecDecode {
        let input = Bench_Echo_Params {
            id: FIXTURE_ID,
            data: black_box(expected).to_vec(),
        };
        let started = Instant::now();
        let encoded = profile_encode(input);
        encode_ns = Some(nanos(started.elapsed()));
        ensure!(
            encoded.1.is_empty() && encoded.2 == 0 && encoded.0.as_slice() == canonical,
            "codec encoded bytes/handles/IDs differ from canonical fixture"
        );
        Some(encoded)
    } else {
        None
    };
    let bytes = encoded
        .as_ref()
        .map_or(canonical, |encoded| encoded.0.as_slice());
    if operation != DiagnosticOperation::CodecEncode {
        let started = Instant::now();
        let decoded = profile_decode(bytes)?;
        decode_ns = Some(nanos(started.elapsed()));
        validate(&decoded, expected)?;
        black_box(&decoded);
        drop(decoded);
    }
    drop(encoded);
    Ok((encode_ns, decode_ns))
}
fn codec_probe(options: &Options, diagnostic: Diagnostic) -> Result<Value> {
    let expected = support::payload(diagnostic.payload_bytes);
    let canonical = profile_encode(Bench_Echo_Params {
        id: FIXTURE_ID,
        data: expected.clone(),
    });
    ensure!(
        canonical.1.is_empty()
            && canonical.2 == 0
            && canonical.0.len() == 32 + diagnostic.payload_bytes,
        "codec preflight size/handles/IDs mismatch"
    );
    validate(&profile_decode(&canonical.0)?, &expected)?;
    for _ in 0..options.warmup {
        codec_iteration(&expected, &canonical.0, diagnostic.operation)?;
    }
    ready(options, diagnostic, None)?;
    let mut encode = Samples::new();
    let mut decode = Samples::new();
    let mut completed = 0;
    let wall = Instant::now();
    while completed < options.iterations
        || wall.elapsed() < Duration::from_millis(options.sample_ms)
    {
        let (e, d) = codec_iteration(&expected, &canonical.0, diagnostic.operation)?;
        if let Some(ns) = e {
            encode.record(ns);
        }
        if let Some(ns) = d {
            decode.record(ns);
        }
        completed += 1;
    }
    let elapsed = wall.elapsed();
    Ok(report(
        options,
        diagnostic,
        None,
        completed,
        elapsed,
        json!({"encode_latency":encode.finish(),"decode_latency":decode.finish(),
        "encoded_body_bytes":canonical.0.len(),"encoded_body_fnv1a64":checksum(&canonical.0),"fixture_request_id":FIXTURE_ID,
        "scope":"same generated byte Echo Params as typed transport, pure in-process codec; no native runtime initialized",
        "latency_clock":"selected encode/decode boundary only; input clone, exact canonical byte check, full decoded ID/content validation and final output drop excluded",
        "throughput_clock":"full selected operation/pair loop including clone where applicable, full validation, final drop and statistics; encode-only checks every encoded byte/handle/ID against the preflight semantic roundtrip, with no per-iteration decode; not isolated encode/decode throughput"}),
    ))
}

pub(super) fn benchmark(library: Option<&Path>, options: &Options) -> Result<Value> {
    let diagnostic = options
        .diagnostic
        .context("missing dedicated diagnostic selection")?;
    match diagnostic.operation {
        DiagnosticOperation::CodecEncode
        | DiagnosticOperation::CodecDecode
        | DiagnosticOperation::CodecPair => codec_probe(options, diagnostic),
        DiagnosticOperation::Typed | DiagnosticOperation::Raw => {
            // SAFETY: exactly the same operator-supplied trusted library contract
            // as the default harness; no library is loaded in codec-only probes.
            let runtime = unsafe {
                Runtime::load(
                    library.context("set MOJO_RUNTIME_LIBRARY for typed/raw diagnostics")?,
                    Role::Broker,
                )?
            };
            if diagnostic.operation == DiagnosticOperation::Typed {
                typed(&runtime, options, diagnostic)
            } else {
                super::raw::diagnostic(&runtime, options, diagnostic)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quantiles_and_all_observation_histogram_have_independent_known_oracles() {
        let mut samples = Samples::new();
        for value in 1..=100 {
            samples.record(value);
        }
        let summary = samples.finish();
        assert_eq!(summary["observations"], 100);
        assert_eq!(summary["p50_us"], 0.05);
        assert_eq!(summary["p99_us"], 0.099);
        let bins = summary["histogram"]["bins"].as_array().unwrap();
        assert_eq!(
            bins.iter()
                .map(|bin| bin["count"].as_u64().unwrap())
                .sum::<u64>(),
            100
        );
        assert_eq!(bins[1]["count"], 1);
        assert_eq!(bins[2]["count"], 2);
        assert_eq!(bins[7]["count"], 37);
    }
    #[test]
    fn zero_and_maximum_duration_use_defined_end_buckets() {
        let mut samples = Samples::new();
        samples.record(0);
        samples.record(u64::MAX);
        let summary = samples.finish();
        let bins = summary["histogram"]["bins"].as_array().unwrap();
        assert_eq!(bins[0]["count"], 1);
        assert_eq!(bins[0]["upper_exclusive_ns"], 1);
        assert_eq!(bins[64]["count"], 1);
        assert!(bins[64]["upper_exclusive_ns"].is_null());
    }
}
