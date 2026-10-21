//! Opt-in complex values. All fields are checked, including float IEEE bits.
use super::{
    Event, Session,
    codec::Latencies,
    complex_cases::*,
    support::{Options, event},
};
use anyhow::{Result, bail, ensure};
use ipc::Runtime;
use serde_json::{Value, json};
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
const CAPACITY: u64 = 100_000;

// Fully qualified generated names keep the expansion independent of imports in main.
macro_rules! service_methods {
    () => {
        fn EchoComplexString(
            &mut self,
            id: u64,
            data: String,
            callback: impl FnOnce(u64, String) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexRecord(
            &mut self,
            id: u64,
            data: crate::protocol::bench::ComplexRecord,
            callback: impl FnOnce(u64, crate::protocol::bench::ComplexRecord) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexRecords(
            &mut self,
            id: u64,
            data: Vec<crate::protocol::bench::ComplexRecord>,
            callback: impl FnOnce(u64, Vec<crate::protocol::bench::ComplexRecord>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexNested(
            &mut self,
            id: u64,
            data: Vec<Vec<u32>>,
            callback: impl FnOnce(u64, Vec<Vec<u32>>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexStringMap(
            &mut self,
            id: u64,
            data: std::collections::HashMap<String, crate::protocol::bench::ComplexRecord>,
            callback: impl FnOnce(
                u64,
                std::collections::HashMap<String, crate::protocol::bench::ComplexRecord>,
            ) + Send
            + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexNumericMap(
            &mut self,
            id: u64,
            data: std::collections::HashMap<i32, Vec<u8>>,
            callback: impl FnOnce(u64, std::collections::HashMap<i32, Vec<u8>>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexUnions(
            &mut self,
            id: u64,
            data: Vec<crate::protocol::bench::ComplexChoice>,
            callback: impl FnOnce(u64, Vec<crate::protocol::bench::ComplexChoice>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexNullable(
            &mut self,
            id: u64,
            data: Vec<Option<Box<crate::protocol::bench::ComplexEnvelope>>>,
            callback: impl FnOnce(u64, Vec<Option<Box<crate::protocol::bench::ComplexEnvelope>>>)
            + Send
            + 'static,
        ) {
            callback(id, data);
        }
        fn EchoComplexStrings(
            &mut self,
            id: u64,
            data: Vec<String>,
            callback: impl FnOnce(u64, Vec<String>) + Send + 'static,
        ) {
            callback(id, data);
        }
    };
}
pub(super) use service_methods;
fn ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}
fn enabled<C: ComplexCase>(options: &Options) -> bool {
    options.complex_cases.iter().any(|name| name == C::NAME)
}
fn pair<C: ComplexCase>(fixture: &Fixture<C>, id: u64) -> Result<(u64, u64)> {
    let data = black_box(fixture.data.clone());
    let start = Instant::now();
    let bytes = encode::<C>(id, data)?;
    let encode_ns = ns(start.elapsed());
    ensure!(
        bytes.len() == fixture.request_bytes,
        "complex encoded length changed"
    );
    let start = Instant::now();
    let (actual_id, actual) = decode::<C>(black_box(&bytes))?;
    let decode_ns = ns(start.elapsed());
    ensure!(
        fixture.validate(id, actual_id, &actual),
        "complex {} full content/bit/checksum mismatch",
        C::NAME
    );
    black_box(&actual);
    Ok((encode_ns, decode_ns))
}
fn codec_case<C: ComplexCase>(options: &Options, rows: &mut Vec<Value>) -> Result<()> {
    if !enabled::<C>(options) {
        return Ok(());
    }
    for (size, budget) in SIZES {
        let fixture = Fixture::<C>::new(size, budget)?;
        event(
            json!({"event":"case_started","name":"complex_codec","complex_case":C::NAME,"size_label":size,"scale":fixture.scale,"encoded_body_bytes":fixture.request_bytes}),
        );
        for id in 0..options.complex_warmup {
            pair::<C>(&fixture, id)?;
        }
        let mut timings = Latencies::new();
        let mut completed = 0;
        let start = Instant::now();
        while completed < options.complex_iterations
            || start.elapsed() < Duration::from_millis(options.complex_codec_ms)
        {
            let (e, d) = pair::<C>(&fixture, completed)?;
            completed += 1;
            timings.record(completed, e, d);
        }
        let seconds = start.elapsed().as_secs_f64();
        let (encode, decode) = timings.summaries(completed);
        let mut row = fixture.metadata();
        row.as_object_mut().unwrap().extend(json!({"name":"complex_codec","scope":"generated Mojom Params codec only; no native runtime initialized",
            "native_runtime_required":false,"completed":completed,"elapsed_seconds":seconds,
            "encode_latency":encode,"decode_latency":decode,"codec_pairs_per_second":completed as f64/seconds,
            "encoded_body_mib_per_second":completed as f64*fixture.request_bytes as f64/seconds/1_048_576.0,
            "minimum_completed":options.complex_iterations,"minimum_sample_ms":options.complex_codec_ms,"warmup_pairs":options.complex_warmup,
            "latency_sampling":{"mode":if completed>CAPACITY{"paired_reservoir"}else{"all"},"capacity":CAPACITY,"quantiles":"nearest-rank of stored samples","min_mean_max":"all observations"},
            "latency_clock":"encode excludes input clone; decode excludes final output validation and destruction; codec intermediate allocation/drop included",
            "throughput_clock":"full loop including input clone, full bitwise and canonical-content-checksum validation and drop; one encoded request body per pair",
            "small_sample_tail_caution":completed<100}).as_object().unwrap().clone());
        event(json!({"event":"case_finished","result":row}));
        rows.push(row);
    }
    Ok(())
}
fn serial<C: ComplexCase>(
    session: &mut Session,
    fixture: &Arc<Fixture<C>>,
    minimum: u64,
    duration: Duration,
) -> Result<(u64, Duration, Value)> {
    let mut completed = 0;
    let mut timings = Latencies::new();
    let start = Instant::now();
    while completed < minimum || start.elapsed() < duration {
        let id = completed;
        let data = fixture.data.clone();
        let verify = Arc::clone(fixture);
        let tx = session.tx.clone();
        let sent = Instant::now();
        C::send(session.remote(), id, data, move |actual_id, actual| {
            let elapsed = ns(sent.elapsed());
            let valid = verify.validate(id, actual_id, &actual);
            // Validation and final output drop finish before the next request starts.
            drop(actual);
            let _ = tx.send(Event::Echo {
                id: actual_id,
                valid,
                ns: elapsed,
            });
        });
        match session.receive()? {
            Event::Echo {
                id: actual_id,
                valid,
                ns,
            } => {
                ensure!(
                    actual_id == id && valid,
                    "complex {} response ID/content/bits/checksum mismatch",
                    C::NAME
                );
                completed += 1;
                timings.record(completed, ns, ns);
            }
            Event::Disconnected => bail!("complex {} peer disconnected", C::NAME),
            _ => bail!("unexpected complex event"),
        }
    }
    let elapsed = start.elapsed();
    ensure!(session.pending() == 0, "complex callbacks not drained");
    let (latency, _) = timings.summaries(completed);
    Ok((completed, elapsed, latency))
}
fn typed_case<C: ComplexCase>(
    options: &Options,
    session: &mut Session,
    rows: &mut Vec<Value>,
) -> Result<()> {
    if !enabled::<C>(options) {
        return Ok(());
    }
    for (size, budget) in SIZES {
        let fixture = Arc::new(Fixture::<C>::new(size, budget)?);
        event(
            json!({"event":"case_started","name":"complex_serial","complex_case":C::NAME,"size_label":size,"scale":fixture.scale,"encoded_body_bytes":fixture.request_bytes}),
        );
        if options.complex_warmup > 0 {
            serial::<C>(session, &fixture, options.complex_warmup, Duration::ZERO)?;
        }
        let (completed, elapsed, latency) = serial::<C>(
            session,
            &fixture,
            options.complex_iterations,
            Duration::from_millis(options.complex_typed_ms),
        )?;
        let seconds = elapsed.as_secs_f64();
        let mut row = fixture.metadata();
        row.as_object_mut().unwrap().extend(json!({"name":"complex_serial","scope":"real two-process typed generated Mojom Echo; complete Rust SDK path",
            "sent":completed,"completed":completed,"elapsed_seconds":seconds,"rpc_per_second":completed as f64/seconds,
            "encoded_body_one_direction_mib_per_second":completed as f64*fixture.request_bytes as f64/seconds/1_048_576.0,
            "encoded_body_both_directions_mib_per_second":completed as f64*(fixture.request_bytes+fixture.response_bytes)as f64/seconds/1_048_576.0,
            "final_pending":0,"max_inflight":1,"latency":latency,"minimum_completed":options.complex_iterations,
            "minimum_sample_ms":options.complex_typed_ms,"warmup_rpcs":options.complex_warmup,
            "latency_sampling":{"mode":if completed>CAPACITY{"reservoir"}else{"all"},"capacity":CAPACITY},
            "latency_clock":"typed method call to callback entry; excludes input clone, response content validation and final output drop",
            "throughput_clock":"wall time including clone, full bitwise and canonical-content-checksum validation, final output drop and mpsc driving",
            "small_sample_tail_caution":completed<100}).as_object().unwrap().clone());
        event(json!({"event":"case_finished","result":row}));
        rows.push(row);
    }
    Ok(())
}
pub fn codec_benchmark(options: &Options) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    all_cases!(codec_case, options, &mut rows);
    Ok(rows)
}
pub fn typed_benchmark(runtime: &Runtime, options: &Options) -> Result<Vec<Value>> {
    let mut session = Session::open(runtime, options)?;
    let mut rows = Vec::new();
    all_cases!(typed_case, options, &mut session, &mut rows);
    session.close(0)?;
    Ok(rows)
}
pub fn results(options: &Options, codec_only: Vec<Value>, typed_serial: Vec<Value>) -> Value {
    json!({"configuration":{"cases":options.complex_cases,"target_encoded_body_budgets_bytes":SIZES.map(|(_,bytes)|bytes),
        "codec_minimum_sample_ms":options.complex_codec_ms,"typed_minimum_sample_ms":options.complex_typed_ms,
        "minimum_completed":options.complex_iterations,"warmup_count":options.complex_warmup,"window":1,"fixture_version":1},
        "wire_byte_scope":"actual serialized generated request/response Params bodies; includes body headers, pointers, array/string/map/union headers and padding; excludes outer Mojo message/native transport framing and handles",
        "budget_scope":"largest fixture scale with request Params body <= budget; actual encoded bytes can be below target; use actual bytes for throughput",
        "content_validation":"every complete returned value is compared recursively by field and IEEE float bits, and canonical FNV-1a64 logical checksum; map keys sorted for checksum, not a deterministic-wire or cryptographic claim",
        "codec_only":codec_only,"typed_serial":typed_serial})
}
