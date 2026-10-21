//! Additional primitive arrays, independent of the unchanged uint8 workloads.
//! Fixed short cases make the unoptimized array codec baseline practical.
use super::{
    Event, Session,
    codec::Latencies,
    protocol::bench::*,
    support::{Options, event},
};
use anyhow::{Context, Result, bail, ensure};
use ipc::__private::{mojom_value_parser as codec, ordered_float::OrderedFloat};
use ipc::{Remote, Runtime};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};

const WIRE_PAYLOAD_BYTES: [usize; 3] = [64, 1_024, 65_536];
const MINIMUM_COUNT: u64 = 8;
const WARMUP_COUNT: u64 = 2;
const SAMPLE_MS: u64 = 200;
const SAMPLE_CAPACITY: u64 = 100_000;

// Invoked inside the existing service implementation. Echo ownership is moved
// through the actual generated interface, with no extra validation in dispatch.
macro_rules! service_methods {
    () => {
        fn EchoBool(
            &mut self,
            id: u64,
            data: Vec<bool>,
            callback: impl FnOnce(u64, Vec<bool>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoInt8(
            &mut self,
            id: u64,
            data: Vec<i8>,
            callback: impl FnOnce(u64, Vec<i8>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoInt16(
            &mut self,
            id: u64,
            data: Vec<i16>,
            callback: impl FnOnce(u64, Vec<i16>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoUint16(
            &mut self,
            id: u64,
            data: Vec<u16>,
            callback: impl FnOnce(u64, Vec<u16>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoInt32(
            &mut self,
            id: u64,
            data: Vec<i32>,
            callback: impl FnOnce(u64, Vec<i32>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoUint32(
            &mut self,
            id: u64,
            data: Vec<u32>,
            callback: impl FnOnce(u64, Vec<u32>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoInt64(
            &mut self,
            id: u64,
            data: Vec<i64>,
            callback: impl FnOnce(u64, Vec<i64>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoUint64(
            &mut self,
            id: u64,
            data: Vec<u64>,
            callback: impl FnOnce(u64, Vec<u64>) + Send + 'static,
        ) {
            callback(id, data);
        }
        fn EchoFloat(
            &mut self,
            id: u64,
            data: Vec<ipc::__private::ordered_float::OrderedFloat<f32>>,
            callback: impl FnOnce(u64, Vec<ipc::__private::ordered_float::OrderedFloat<f32>>)
            + Send
            + 'static,
        ) {
            callback(id, data);
        }
        fn EchoDouble(
            &mut self,
            id: u64,
            data: Vec<ipc::__private::ordered_float::OrderedFloat<f64>>,
            callback: impl FnOnce(u64, Vec<ipc::__private::ordered_float::OrderedFloat<f64>>)
            + Send
            + 'static,
        ) {
            callback(id, data);
        }
    };
}
pub(super) use service_methods;

trait ArrayCase: 'static {
    type Item: Clone + Send + Sync + 'static;
    type Params: codec::MojomParse<Self::Params, ()>;
    const NAME: &'static str;
    const ELEMENT_BITS: usize;
    fn values(elements: usize) -> Vec<Self::Item>;
    fn equal(left: &[Self::Item], right: &[Self::Item]) -> bool;
    fn params(id: u64, data: Vec<Self::Item>) -> Self::Params;
    fn unpack(params: Self::Params) -> (u64, Vec<Self::Item>);
    fn send(
        remote: &mut Remote<dyn Bench>,
        id: u64,
        data: Vec<Self::Item>,
        callback: impl FnOnce(u64, Vec<Self::Item>) + Send + 'static,
    );
}

macro_rules! array_case {
    ($case:ident, $name:literal, $item:ty, $bits:literal, $method:ident, $params:ident, $make:expr, $equal:expr) => {
        struct $case;
        impl ArrayCase for $case {
            type Item = $item;
            type Params = $params;
            const NAME: &'static str = $name;
            const ELEMENT_BITS: usize = $bits;
            fn values(elements: usize) -> Vec<Self::Item> {
                (0..elements).map($make).collect()
            }
            fn equal(left: &[Self::Item], right: &[Self::Item]) -> bool {
                ($equal)(left, right)
            }
            fn params(id: u64, data: Vec<Self::Item>) -> Self::Params {
                $params { id, data }
            }
            fn unpack(params: Self::Params) -> (u64, Vec<Self::Item>) {
                (params.id, params.data)
            }
            fn send(
                remote: &mut Remote<dyn Bench>,
                id: u64,
                data: Vec<Self::Item>,
                callback: impl FnOnce(u64, Vec<Self::Item>) + Send + 'static,
            ) {
                remote.$method(id, data, callback);
            }
        }
    };
}

fn integer_bits(index: usize) -> u64 {
    match index % 8 {
        0 => 0,
        1 => u64::MAX,
        2 => 1,
        3 => 1_u64 << 63,
        _ => (index as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left((index % 64) as u32),
    }
}

fn float_value(index: usize) -> OrderedFloat<f32> {
    const BITS: [u32; 10] = [
        0,
        0x8000_0000,
        0x3f80_0000,
        0xbf80_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_1234,
        0xffc0_4321,
        1,
        0x7f7f_ffff,
    ];
    OrderedFloat(f32::from_bits(BITS[index % BITS.len()]))
}

fn double_value(index: usize) -> OrderedFloat<f64> {
    const BITS: [u64; 10] = [
        0,
        0x8000_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        0x7ff8_0000_0000_1234,
        0xfff8_0000_0000_4321,
        1,
        0x7fef_ffff_ffff_ffff,
    ];
    OrderedFloat(f64::from_bits(BITS[index % BITS.len()]))
}

array_case!(
    BoolArray,
    "bool",
    bool,
    1,
    EchoBool,
    Bench_EchoBool_Params,
    |index| index % 7 < 3,
    |left: &[bool], right: &[bool]| left == right
);
array_case!(
    Int8Array,
    "int8",
    i8,
    8,
    EchoInt8,
    Bench_EchoInt8_Params,
    |index| integer_bits(index) as i8,
    |left: &[i8], right: &[i8]| left == right
);
array_case!(
    Int16Array,
    "int16",
    i16,
    16,
    EchoInt16,
    Bench_EchoInt16_Params,
    |index| integer_bits(index) as i16,
    |left: &[i16], right: &[i16]| left == right
);
array_case!(
    Uint16Array,
    "uint16",
    u16,
    16,
    EchoUint16,
    Bench_EchoUint16_Params,
    |index| integer_bits(index) as u16,
    |left: &[u16], right: &[u16]| left == right
);
array_case!(
    Int32Array,
    "int32",
    i32,
    32,
    EchoInt32,
    Bench_EchoInt32_Params,
    |index| integer_bits(index) as i32,
    |left: &[i32], right: &[i32]| left == right
);
array_case!(
    Uint32Array,
    "uint32",
    u32,
    32,
    EchoUint32,
    Bench_EchoUint32_Params,
    |index| integer_bits(index) as u32,
    |left: &[u32], right: &[u32]| left == right
);
array_case!(
    Int64Array,
    "int64",
    i64,
    64,
    EchoInt64,
    Bench_EchoInt64_Params,
    |index| integer_bits(index) as i64,
    |left: &[i64], right: &[i64]| left == right
);
array_case!(
    Uint64Array,
    "uint64",
    u64,
    64,
    EchoUint64,
    Bench_EchoUint64_Params,
    integer_bits,
    |left: &[u64], right: &[u64]| left == right
);
array_case!(
    FloatArray,
    "float",
    OrderedFloat<f32>,
    32,
    EchoFloat,
    Bench_EchoFloat_Params,
    float_value,
    |left: &[OrderedFloat<f32>], right: &[OrderedFloat<f32>]| left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.0.to_bits() == right.0.to_bits())
);
array_case!(
    DoubleArray,
    "double",
    OrderedFloat<f64>,
    64,
    EchoDouble,
    Bench_EchoDouble_Params,
    double_value,
    |left: &[OrderedFloat<f64>], right: &[OrderedFloat<f64>]| left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.0.to_bits() == right.0.to_bits())
);

fn nanoseconds(elapsed: Duration) -> u64 {
    elapsed.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn wire_payload_bytes<C: ArrayCase>(elements: usize) -> usize {
    (elements * C::ELEMENT_BITS).div_ceil(8)
}

fn expected<C: ArrayCase>(target: usize) -> Vec<C::Item> {
    C::values(target * 8 / C::ELEMENT_BITS)
}

fn metadata<C: ArrayCase>(target: usize, elements: usize) -> Value {
    json!({
        "primitive_type": C::NAME, "payload_bytes": wire_payload_bytes::<C>(elements),
        "target_array_wire_payload_bytes": target, "array_wire_payload_bytes": wire_payload_bytes::<C>(elements),
        "elements": elements, "element_bits": C::ELEMENT_BITS, "array_header_bytes": 8,
        "minimum_sample_ms": SAMPLE_MS, "minimum_completed": MINIMUM_COUNT,
        "warmup_count": WARMUP_COUNT, "window": 1,
        "float_validation": "float/double compare IEEE-754 bits, including signed zero and NaN payloads",
    })
}

fn pair<C: ArrayCase>(expected: &[C::Item], id: u64) -> Result<(u64, u64, usize)> {
    let input = black_box(C::params(id, expected.to_vec()));
    let started = Instant::now();
    let (bytes, handles, offset) = codec::serialize::<(), C::Params>(input, &());
    let encode_ns = nanoseconds(started.elapsed());
    ensure!(
        handles.is_empty() && offset == 0,
        "primitive codec produced handles or interfaces"
    );
    let started = Instant::now();
    let decoded =
        codec::deserialize_exact::<(), C::Params>(black_box(&bytes), &mut [], offset, &())
            .with_context(|| format!("decode {} array", C::NAME))?;
    let decode_ns = nanoseconds(started.elapsed());
    let (actual_id, actual) = C::unpack(decoded);
    ensure!(
        actual_id == id && C::equal(expected, &actual),
        "primitive codec changed ID or {} bits",
        C::NAME
    );
    black_box(&actual);
    Ok((encode_ns, decode_ns, bytes.len()))
}

fn codec_case<C: ArrayCase>(rows: &mut Vec<Value>) -> Result<()> {
    for target in WIRE_PAYLOAD_BYTES {
        let expected = expected::<C>(target);
        ensure!(
            wire_payload_bytes::<C>(expected.len()) == target,
            "wire payload budget mismatch"
        );
        event(
            json!({"event":"case_started", "name":"primitive_array_codec", "primitive_type":C::NAME, "payload_bytes":target}),
        );
        for id in 0..WARMUP_COUNT {
            pair::<C>(&expected, id)?;
        }
        let mut timings = Latencies::new();
        let mut completed = 0;
        let mut encoded_body_bytes = None;
        let started = Instant::now();
        while completed < MINIMUM_COUNT || started.elapsed() < Duration::from_millis(SAMPLE_MS) {
            let (encode, decode, bytes) = pair::<C>(&expected, completed)?;
            ensure!(
                encoded_body_bytes.is_none_or(|previous| previous == bytes),
                "encoded primitive size changed"
            );
            encoded_body_bytes = Some(bytes);
            completed += 1;
            timings.record(completed, encode, decode);
        }
        let seconds = started.elapsed().as_secs_f64();
        let (encode, decode) = timings.summaries(completed);
        let mut row = metadata::<C>(target, expected.len());
        let fields = json!({
            "name":"primitive_array_codec", "scope":"in-process generated Mojom Params codec; no native runtime initialized",
            "native_runtime_required":false, "completed":completed, "elapsed_seconds":seconds,
            "encoded_body_bytes":encoded_body_bytes, "encode_latency":encode, "decode_latency":decode,
            "codec_pairs_per_second":completed as f64 / seconds,
            "array_wire_payload_mib_per_second":completed as f64 * target as f64 / seconds / 1_048_576.0,
            "latency_sampling":{"mode":if completed > SAMPLE_CAPACITY {"paired_reservoir"} else {"all"},
                "capacity":SAMPLE_CAPACITY,"quantiles":"nearest-rank of stored samples","min_mean_max":"all completed observations"},
            "throughput_scope":"full pair loop including input clone and bitwise validation; one array payload per pair",
            "small_sample_tail_caution":completed < 100,
        });
        row.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        event(json!({"event":"case_finished", "result":&row}));
        rows.push(row);
    }
    Ok(())
}

fn serial<C: ArrayCase>(
    session: &mut Session,
    expected: &Arc<Vec<C::Item>>,
    minimum: u64,
    duration: Duration,
) -> Result<(u64, Duration, Value)> {
    let mut completed = 0;
    let mut timings = Latencies::new();
    let started = Instant::now();
    while completed < minimum || started.elapsed() < duration {
        let id = completed;
        let data = (**expected).clone();
        let verify = Arc::clone(expected);
        let tx = session.tx.clone();
        let start = Instant::now();
        C::send(session.remote(), id, data, move |actual_id, actual| {
            let ns = nanoseconds(start.elapsed());
            let valid = actual_id == id && C::equal(&verify, &actual);
            let _ = tx.send(Event::Echo {
                id: actual_id,
                valid,
                ns,
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
                    "primitive {} response bits or ID mismatch",
                    C::NAME
                );
                completed += 1;
                timings.record(completed, ns, ns);
            }
            Event::Disconnected => bail!("primitive {} peer disconnected", C::NAME),
            _ => bail!("unexpected event during primitive {} echo", C::NAME),
        }
    }
    let elapsed = started.elapsed();
    ensure!(session.pending() == 0, "primitive callbacks not drained");
    let (latency, _) = timings.summaries(completed);
    Ok((completed, elapsed, latency))
}

fn typed_case<C: ArrayCase>(session: &mut Session, rows: &mut Vec<Value>) -> Result<()> {
    for target in WIRE_PAYLOAD_BYTES {
        let expected = Arc::new(expected::<C>(target));
        ensure!(
            wire_payload_bytes::<C>(expected.len()) == target,
            "wire payload budget mismatch"
        );
        event(
            json!({"event":"case_started", "name":"primitive_array_serial", "primitive_type":C::NAME, "payload_bytes":target}),
        );
        serial::<C>(session, &expected, WARMUP_COUNT, Duration::ZERO)?;
        let (completed, elapsed, latency) = serial::<C>(
            session,
            &expected,
            MINIMUM_COUNT,
            Duration::from_millis(SAMPLE_MS),
        )?;
        let seconds = elapsed.as_secs_f64();
        let mut row = metadata::<C>(target, expected.len());
        let fields = json!({
            "name":"primitive_array_serial", "scope":"two-process typed generated Mojom Echo; complete Rust SDK path",
            "sent":completed,"completed":completed,"elapsed_seconds":seconds,
            "rpc_per_second":completed as f64 / seconds,
            "array_wire_payload_one_direction_mib_per_second":completed as f64 * target as f64 / seconds / 1_048_576.0,
            "array_wire_payload_both_directions_mib_per_second":2.0 * completed as f64 * target as f64 / seconds / 1_048_576.0,
            "final_pending":0,"max_inflight":1,"latency":latency,
            "latency_sampling":{"mode":if completed > SAMPLE_CAPACITY {"reservoir"} else {"all"},"capacity":SAMPLE_CAPACITY},
            "latency_clock":"typed method call to callback entry; excludes input clone and callback bitwise validation",
            "throughput_clock":"wall time including input clone, bitwise validation and mpsc driving",
            "small_sample_tail_caution":completed < 100,
        });
        row.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        event(json!({"event":"case_finished", "result":&row}));
        rows.push(row);
    }
    Ok(())
}

macro_rules! all_cases {
    ($function:ident, $($argument:expr),*) => {
        $function::<BoolArray>($($argument),*)?;
        $function::<Int8Array>($($argument),*)?;
        $function::<Int16Array>($($argument),*)?;
        $function::<Uint16Array>($($argument),*)?;
        $function::<Int32Array>($($argument),*)?;
        $function::<Uint32Array>($($argument),*)?;
        $function::<Int64Array>($($argument),*)?;
        $function::<Uint64Array>($($argument),*)?;
        $function::<FloatArray>($($argument),*)?;
        $function::<DoubleArray>($($argument),*)?;
    };
}

pub fn codec_benchmark() -> Result<Vec<Value>> {
    let mut rows = Vec::with_capacity(30);
    all_cases!(codec_case, &mut rows);
    Ok(rows)
}

pub fn typed_benchmark(runtime: &Runtime, options: &Options) -> Result<Vec<Value>> {
    let mut session = Session::open(runtime, options)?;
    let mut rows = Vec::with_capacity(30);
    all_cases!(typed_case, &mut session, &mut rows);
    session.close(0)?;
    Ok(rows)
}

pub fn results(codec_only: Vec<Value>, typed_serial: Vec<Value>) -> Value {
    json!({
        "configuration":{"types":["bool","int8","int16","uint16","int32","uint32","int64","uint64","float","double"],
            "target_array_wire_payload_bytes":WIRE_PAYLOAD_BYTES,"minimum_sample_ms":SAMPLE_MS,
            "minimum_completed":MINIMUM_COUNT,"warmup_count":WARMUP_COUNT,"window":1},
        "wire_payload_scope":"array element data bytes only; bool uses ceil(elements/8); excludes array/struct/message headers, pointers and ID",
        "float_validation":"generated OrderedFloat<f32/f64> fields are compared by IEEE-754 bits, preserving NaN payloads and signed zero",
        "codec_only":codec_only,"typed_serial":typed_serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundaries<C: ArrayCase>() -> Result<()> {
        for elements in [0, 1, 7, 8, 9, 63, 64, 65] {
            let expected = C::values(elements);
            ensure!(expected.len() == elements);
            pair::<C>(&expected, 0x1234_5678_9abc_def0)?;
            ensure!(wire_payload_bytes::<C>(elements) == (elements * C::ELEMENT_BITS).div_ceil(8));
        }
        Ok(())
    }

    #[test]
    fn generated_primitive_array_codec_boundaries_preserve_bits() -> Result<()> {
        all_cases!(boundaries,);
        Ok(())
    }

    #[test]
    fn float_validation_is_bitwise_not_ordered_float_equality() {
        assert!(!FloatArray::equal(
            &[OrderedFloat(0.0)],
            &[OrderedFloat(-0.0)]
        ));
        assert!(!DoubleArray::equal(
            &[OrderedFloat(f64::from_bits(0x7ff8_0000_0000_1234))],
            &[OrderedFloat(f64::from_bits(0x7ff8_0000_0000_4321))]
        ));
        assert_eq!(wire_payload_bytes::<BoolArray>(9), 2);
    }
}
