//! Nullable primitive-element arrays with independent, fixed short workloads.
//! Every type/budget is tested with all None, all Some, and mixed null tags.
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

macro_rules! service_methods {
    () => {
        fn EchoNullableBool(
            &mut self, id: u64, data: Vec<Option<bool>>,
            callback: impl FnOnce(u64, Vec<Option<bool>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableInt8(
            &mut self, id: u64, data: Vec<Option<i8>>,
            callback: impl FnOnce(u64, Vec<Option<i8>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableUint8(
            &mut self, id: u64, data: Vec<Option<u8>>,
            callback: impl FnOnce(u64, Vec<Option<u8>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableInt16(
            &mut self, id: u64, data: Vec<Option<i16>>,
            callback: impl FnOnce(u64, Vec<Option<i16>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableUint16(
            &mut self, id: u64, data: Vec<Option<u16>>,
            callback: impl FnOnce(u64, Vec<Option<u16>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableInt32(
            &mut self, id: u64, data: Vec<Option<i32>>,
            callback: impl FnOnce(u64, Vec<Option<i32>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableUint32(
            &mut self, id: u64, data: Vec<Option<u32>>,
            callback: impl FnOnce(u64, Vec<Option<u32>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableInt64(
            &mut self, id: u64, data: Vec<Option<i64>>,
            callback: impl FnOnce(u64, Vec<Option<i64>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableUint64(
            &mut self, id: u64, data: Vec<Option<u64>>,
            callback: impl FnOnce(u64, Vec<Option<u64>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableFloat(
            &mut self, id: u64, data: Vec<Option<ipc::__private::ordered_float::OrderedFloat<f32>>>,
            callback: impl FnOnce(u64, Vec<Option<ipc::__private::ordered_float::OrderedFloat<f32>>>) + Send + 'static,
        ) { callback(id, data); }
        fn EchoNullableDouble(
            &mut self, id: u64, data: Vec<Option<ipc::__private::ordered_float::OrderedFloat<f64>>>,
            callback: impl FnOnce(u64, Vec<Option<ipc::__private::ordered_float::OrderedFloat<f64>>>) + Send + 'static,
        ) { callback(id, data); }
    };
}
pub(super) use service_methods;

#[derive(Clone, Copy, Debug)]
enum NullPattern {
    AllNone,
    AllSome,
    Mixed,
}
const PATTERNS: [NullPattern; 3] = [
    NullPattern::AllNone,
    NullPattern::AllSome,
    NullPattern::Mixed,
];
impl NullPattern {
    fn name(self) -> &'static str {
        match self {
            Self::AllNone => "all_none",
            Self::AllSome => "all_some",
            Self::Mixed => "mixed",
        }
    }
    fn is_some(self, index: usize) -> bool {
        match self {
            Self::AllNone => false,
            Self::AllSome => true,
            Self::Mixed => index % 3 != 0,
        }
    }
    fn null_count(self, elements: usize) -> usize {
        match self {
            Self::AllNone => elements,
            Self::AllSome => 0,
            Self::Mixed => elements.div_ceil(3),
        }
    }
}

trait ArrayCase: 'static {
    type Item: Clone + Send + Sync + 'static;
    type Params: codec::MojomParse<Self::Params, ()>;
    const NAME: &'static str;
    const ELEMENT_BITS: usize;
    fn values(elements: usize, pattern: NullPattern) -> Vec<Option<Self::Item>>;
    fn equal(left: &[Option<Self::Item>], right: &[Option<Self::Item>]) -> bool;
    fn params(id: u64, data: Vec<Option<Self::Item>>) -> Self::Params;
    fn unpack(params: Self::Params) -> (u64, Vec<Option<Self::Item>>);
    fn send(
        remote: &mut Remote<dyn Bench>,
        id: u64,
        data: Vec<Option<Self::Item>>,
        callback: impl FnOnce(u64, Vec<Option<Self::Item>>) + Send + 'static,
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
            fn values(elements: usize, pattern: NullPattern) -> Vec<Option<Self::Item>> {
                (0..elements)
                    .map(|index| pattern.is_some(index).then(|| ($make)(index)))
                    .collect()
            }
            fn equal(left: &[Option<Self::Item>], right: &[Option<Self::Item>]) -> bool {
                ($equal)(left, right)
            }
            fn params(id: u64, data: Vec<Option<Self::Item>>) -> Self::Params {
                $params { id, data }
            }
            fn unpack(params: Self::Params) -> (u64, Vec<Option<Self::Item>>) {
                (params.id, params.data)
            }
            fn send(
                remote: &mut Remote<dyn Bench>,
                id: u64,
                data: Vec<Option<Self::Item>>,
                callback: impl FnOnce(u64, Vec<Option<Self::Item>>) + Send + 'static,
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
    EchoNullableBool,
    Bench_EchoNullableBool_Params,
    |index| index % 7 < 3,
    |left: &[Option<bool>], right: &[Option<bool>]| left == right
);
array_case!(
    Int8Array,
    "int8",
    i8,
    8,
    EchoNullableInt8,
    Bench_EchoNullableInt8_Params,
    |index| integer_bits(index) as i8,
    |left: &[Option<i8>], right: &[Option<i8>]| left == right
);
array_case!(
    Uint8Array,
    "uint8",
    u8,
    8,
    EchoNullableUint8,
    Bench_EchoNullableUint8_Params,
    |index| integer_bits(index) as u8,
    |left: &[Option<u8>], right: &[Option<u8>]| left == right
);
array_case!(
    Int16Array,
    "int16",
    i16,
    16,
    EchoNullableInt16,
    Bench_EchoNullableInt16_Params,
    |index| integer_bits(index) as i16,
    |left: &[Option<i16>], right: &[Option<i16>]| left == right
);
array_case!(
    Uint16Array,
    "uint16",
    u16,
    16,
    EchoNullableUint16,
    Bench_EchoNullableUint16_Params,
    |index| integer_bits(index) as u16,
    |left: &[Option<u16>], right: &[Option<u16>]| left == right
);
array_case!(
    Int32Array,
    "int32",
    i32,
    32,
    EchoNullableInt32,
    Bench_EchoNullableInt32_Params,
    |index| integer_bits(index) as i32,
    |left: &[Option<i32>], right: &[Option<i32>]| left == right
);
array_case!(
    Uint32Array,
    "uint32",
    u32,
    32,
    EchoNullableUint32,
    Bench_EchoNullableUint32_Params,
    |index| integer_bits(index) as u32,
    |left: &[Option<u32>], right: &[Option<u32>]| left == right
);
array_case!(
    Int64Array,
    "int64",
    i64,
    64,
    EchoNullableInt64,
    Bench_EchoNullableInt64_Params,
    |index| integer_bits(index) as i64,
    |left: &[Option<i64>], right: &[Option<i64>]| left == right
);
array_case!(
    Uint64Array,
    "uint64",
    u64,
    64,
    EchoNullableUint64,
    Bench_EchoNullableUint64_Params,
    integer_bits,
    |left: &[Option<u64>], right: &[Option<u64>]| left == right
);
array_case!(
    FloatArray,
    "float",
    OrderedFloat<f32>,
    32,
    EchoNullableFloat,
    Bench_EchoNullableFloat_Params,
    float_value,
    |left: &[Option<OrderedFloat<f32>>], right: &[Option<OrderedFloat<f32>>]| left.len()
        == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| match (left, right) {
                (None, None) => true,
                (Some(left), Some(right)) => left.0.to_bits() == right.0.to_bits(),
                _ => false,
            })
);
array_case!(
    DoubleArray,
    "double",
    OrderedFloat<f64>,
    64,
    EchoNullableDouble,
    Bench_EchoNullableDouble_Params,
    double_value,
    |left: &[Option<OrderedFloat<f64>>], right: &[Option<OrderedFloat<f64>>]| left.len()
        == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| match (left, right) {
                (None, None) => true,
                (Some(left), Some(right)) => left.0.to_bits() == right.0.to_bits(),
                _ => false,
            })
);

fn nanoseconds(elapsed: Duration) -> u64 {
    elapsed.as_nanos().min(u128::from(u64::MAX)) as u64
}
fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

// Array payload excludes its eight-byte header and final eight-byte alignment.
// Tag bytes precede a separate bool bitmap or aligned fixed-width value slots.
fn wire_payload_bytes<C: ArrayCase>(elements: usize) -> usize {
    let tags = elements.div_ceil(8);
    if C::ELEMENT_BITS == 1 {
        tags * 2
    } else if elements == 0 {
        0
    } else {
        align_up(tags, C::ELEMENT_BITS / 8) + elements * (C::ELEMENT_BITS / 8)
    }
}
fn elements_for_budget<C: ArrayCase>(budget: usize) -> usize {
    let (mut low, mut high) = (0, budget * 8 + 1);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if wire_payload_bytes::<C>(middle) <= budget {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}
fn expected<C: ArrayCase>(budget: usize, pattern: NullPattern) -> Vec<Option<C::Item>> {
    C::values(elements_for_budget::<C>(budget), pattern)
}
fn metadata<C: ArrayCase>(target: usize, elements: usize, pattern: NullPattern) -> Value {
    let tags = elements.div_ceil(8);
    let values = (elements * C::ELEMENT_BITS).div_ceil(8);
    let actual = wire_payload_bytes::<C>(elements);
    json!({
        "primitive_type":C::NAME,"null_pattern":pattern.name(),"null_count":pattern.null_count(elements),
        "payload_bytes":actual,"target_array_wire_payload_bytes":target,"array_wire_payload_bytes":actual,
        "elements":elements,"element_bits":C::ELEMENT_BITS,"array_header_bytes":8,
        "array_tag_bitmap_bytes":tags,"array_value_bytes":values,"array_internal_padding_bytes":actual-tags-values,
        "array_trailing_alignment_bytes":align_up(actual,8)-actual,"expected_encoded_body_bytes":32+align_up(actual,8),
        "minimum_sample_ms":SAMPLE_MS,"minimum_completed":MINIMUM_COUNT,"warmup_count":WARMUP_COUNT,"window":1,
        "float_validation":"None tags and Some IEEE-754 bits, including signed zero and NaN payloads",
    })
}

fn pair<C: ArrayCase>(expected: &[Option<C::Item>], id: u64) -> Result<(u64, u64, usize)> {
    let input = black_box(C::params(id, expected.to_vec()));
    let started = Instant::now();
    let (bytes, handles, offset) = codec::serialize::<(), C::Params>(input, &());
    let encode_ns = nanoseconds(started.elapsed());
    ensure!(
        handles.is_empty() && offset == 0,
        "nullable codec produced handles or interfaces"
    );
    let started = Instant::now();
    let decoded =
        codec::deserialize_exact::<(), C::Params>(black_box(&bytes), &mut [], offset, &())
            .with_context(|| format!("decode nullable {} array", C::NAME))?;
    let decode_ns = nanoseconds(started.elapsed());
    let (actual_id, actual) = C::unpack(decoded);
    ensure!(
        actual_id == id && C::equal(expected, &actual),
        "nullable codec changed ID, tags or {} bits",
        C::NAME
    );
    // Assert the budget formula against real generated Params bytes, outside the clocks.
    // Params: eight-byte struct header, id, array pointer; array header is eight bytes.
    let payload = wire_payload_bytes::<C>(expected.len());
    ensure!(
        bytes.len() == 32 + align_up(payload, 8),
        "nullable {} encoded body extent mismatch",
        C::NAME
    );
    ensure!(
        u64::from_le_bytes(bytes[16..24].try_into().unwrap()) == 8,
        "nullable array pointer layout changed"
    );
    ensure!(
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize == 8 + payload,
        "nullable array header byte size mismatch"
    );
    ensure!(
        u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize == expected.len(),
        "nullable array header count mismatch"
    );
    black_box(&actual);
    Ok((encode_ns, decode_ns, bytes.len()))
}

fn codec_case<C: ArrayCase>(rows: &mut Vec<Value>) -> Result<()> {
    for target in WIRE_PAYLOAD_BYTES {
        for pattern in PATTERNS {
            let expected = expected::<C>(target, pattern);
            let actual = wire_payload_bytes::<C>(expected.len());
            ensure!(
                actual <= target && wire_payload_bytes::<C>(expected.len() + 1) > target,
                "nullable payload budget is not maximal"
            );
            event(
                json!({"event":"case_started","name":"nullable_array_codec","primitive_type":C::NAME,
                "null_pattern":pattern.name(),"payload_bytes":actual,"target_array_wire_payload_bytes":target}),
            );
            for id in 0..WARMUP_COUNT {
                pair::<C>(&expected, id)?;
            }
            let mut timings = Latencies::new();
            let mut completed = 0;
            let mut encoded_body_bytes = None;
            let started = Instant::now();
            while completed < MINIMUM_COUNT || started.elapsed() < Duration::from_millis(SAMPLE_MS)
            {
                let (encode, decode, bytes) = pair::<C>(&expected, completed)?;
                ensure!(
                    encoded_body_bytes.is_none_or(|previous| previous == bytes),
                    "encoded nullable size changed"
                );
                encoded_body_bytes = Some(bytes);
                completed += 1;
                timings.record(completed, encode, decode);
            }
            let seconds = started.elapsed().as_secs_f64();
            let (encode, decode) = timings.summaries(completed);
            let mut row = metadata::<C>(target, expected.len(), pattern);
            let fields = json!({
                "name":"nullable_array_codec","scope":"in-process generated Mojom Params codec; no native runtime initialized",
                "native_runtime_required":false,"completed":completed,"elapsed_seconds":seconds,
                "encoded_body_bytes":encoded_body_bytes,"encode_latency":encode,"decode_latency":decode,
                "codec_pairs_per_second":completed as f64/seconds,
                "array_wire_payload_mib_per_second":completed as f64*actual as f64/seconds/1_048_576.0,
                "latency_sampling":{"mode":if completed>SAMPLE_CAPACITY {"paired_reservoir"} else {"all"},
                    "capacity":SAMPLE_CAPACITY,"quantiles":"nearest-rank of stored samples","min_mean_max":"all completed observations"},
                "throughput_scope":"full pair loop including input clone, tag/bitwise validation and wire extent checks; one actual array payload per pair",
                "small_sample_tail_caution":completed<100,
            });
            row.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            event(json!({"event":"case_finished","result":&row}));
            rows.push(row);
        }
    }
    Ok(())
}
fn serial<C: ArrayCase>(
    session: &mut Session,
    expected: &Arc<Vec<Option<C::Item>>>,
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
                    "nullable {} response bits or ID mismatch",
                    C::NAME
                );
                completed += 1;
                timings.record(completed, ns, ns);
            }
            Event::Disconnected => bail!("nullable {} peer disconnected", C::NAME),
            _ => bail!("unexpected event during nullable {} echo", C::NAME),
        }
    }
    let elapsed = started.elapsed();
    ensure!(session.pending() == 0, "nullable callbacks not drained");
    let (latency, _) = timings.summaries(completed);
    Ok((completed, elapsed, latency))
}

fn typed_case<C: ArrayCase>(session: &mut Session, rows: &mut Vec<Value>) -> Result<()> {
    for target in WIRE_PAYLOAD_BYTES {
        for pattern in PATTERNS {
            let expected = Arc::new(expected::<C>(target, pattern));
            let actual = wire_payload_bytes::<C>(expected.len());
            ensure!(
                actual <= target && wire_payload_bytes::<C>(expected.len() + 1) > target,
                "nullable payload budget is not maximal"
            );
            event(
                json!({"event":"case_started","name":"nullable_array_serial","primitive_type":C::NAME,
                "null_pattern":pattern.name(),"payload_bytes":actual,"target_array_wire_payload_bytes":target}),
            );
            serial::<C>(session, &expected, WARMUP_COUNT, Duration::ZERO)?;
            let (completed, elapsed, latency) = serial::<C>(
                session,
                &expected,
                MINIMUM_COUNT,
                Duration::from_millis(SAMPLE_MS),
            )?;
            let seconds = elapsed.as_secs_f64();
            let mut row = metadata::<C>(target, expected.len(), pattern);
            let fields = json!({
                "name":"nullable_array_serial","scope":"two-process typed generated nullable Mojom Echo; complete Rust SDK path",
                "sent":completed,"completed":completed,"elapsed_seconds":seconds,"rpc_per_second":completed as f64/seconds,
                "array_wire_payload_one_direction_mib_per_second":completed as f64*actual as f64/seconds/1_048_576.0,
                "array_wire_payload_both_directions_mib_per_second":2.0*completed as f64*actual as f64/seconds/1_048_576.0,
                "final_pending":0,"max_inflight":1,"latency":latency,
                "latency_sampling":{"mode":if completed>SAMPLE_CAPACITY {"reservoir"} else {"all"},"capacity":SAMPLE_CAPACITY},
                "latency_clock":"typed method call to callback entry; excludes input clone and callback tag/bitwise validation",
                "throughput_clock":"wall time including input clone, tag/bitwise validation and mpsc driving",
                "small_sample_tail_caution":completed<100,
            });
            row.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            event(json!({"event":"case_finished","result":&row}));
            rows.push(row);
        }
    }
    Ok(())
}

macro_rules! all_cases {
    ($function:ident, $($argument:expr),*) => {
        $function::<BoolArray>($($argument),*)?;
        $function::<Int8Array>($($argument),*)?;
        $function::<Uint8Array>($($argument),*)?;
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
    let mut rows = Vec::with_capacity(99);
    all_cases!(codec_case, &mut rows);
    Ok(rows)
}
pub fn typed_benchmark(runtime: &Runtime, options: &Options) -> Result<Vec<Value>> {
    let mut session = Session::open(runtime, options)?;
    let mut rows = Vec::with_capacity(99);
    all_cases!(typed_case, &mut session, &mut rows);
    session.close(0)?;
    Ok(rows)
}
pub fn results(codec_only: Vec<Value>, typed_serial: Vec<Value>) -> Value {
    json!({
        "configuration":{"types":["bool","int8","uint8","int16","uint16","int32","uint32","int64","uint64","float","double"],
            "null_patterns":["all_none","all_some","mixed"],"mixed_none_rule":"index % 3 == 0",
            "target_array_wire_payload_bytes":WIRE_PAYLOAD_BYTES,"minimum_sample_ms":SAMPLE_MS,
            "minimum_completed":MINIMUM_COUNT,"warmup_count":WARMUP_COUNT,"window":1,
            "element_count_rule":"largest count whose tag bitmap + internal alignment + fixed value slots fit target budget"},
        "wire_payload_scope":"array body data: tag bitmap + value slots + internal alignment; excludes eight-byte array header, final eight-byte alignment, struct/message headers, pointer and ID",
        "null_patterns":"all_none, all_some, and mixed with None at index % 3 == 0; None retains its fixed value slot",
        "float_validation":"generated Option<OrderedFloat<f32/f64>> compares None/Some tags and Some IEEE-754 bits, preserving NaN payloads and signed zero",
        "codec_only":codec_only,"typed_serial":typed_serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn boundaries<C: ArrayCase>() -> Result<()> {
        for elements in [0, 1, 7, 8, 9, 63, 64, 65] {
            for pattern in PATTERNS {
                let expected = C::values(elements, pattern);
                ensure!(expected.len() == elements);
                ensure!(
                    expected.iter().filter(|value| value.is_none()).count()
                        == pattern.null_count(elements)
                );
                pair::<C>(&expected, 0x1234_5678_9abc_def0)?;
            }
        }
        for budget in WIRE_PAYLOAD_BYTES {
            let elements = elements_for_budget::<C>(budget);
            ensure!(wire_payload_bytes::<C>(elements) <= budget);
            ensure!(wire_payload_bytes::<C>(elements + 1) > budget);
        }
        Ok(())
    }
    #[test]
    fn generated_nullable_array_boundaries_preserve_tags_and_bits() -> Result<()> {
        all_cases!(boundaries,);
        Ok(())
    }
    #[test]
    fn nullable_float_validation_distinguishes_tags_zero_sign_and_nan_payloads() {
        assert!(!FloatArray::equal(&[None], &[Some(OrderedFloat(0.0))]));
        assert!(!FloatArray::equal(
            &[Some(OrderedFloat(0.0))],
            &[Some(OrderedFloat(-0.0))]
        ));
        assert!(!DoubleArray::equal(
            &[Some(OrderedFloat(f64::from_bits(0x7ff8_0000_0000_1234)))],
            &[Some(OrderedFloat(f64::from_bits(0x7ff8_0000_0000_4321)))]
        ));
        assert!(DoubleArray::equal(&[None], &[None]));
        assert_eq!(wire_payload_bytes::<BoolArray>(9), 4);
        assert_eq!(wire_payload_bytes::<Int16Array>(9), 20);
        assert_eq!(wire_payload_bytes::<DoubleArray>(9), 80);
    }
}
