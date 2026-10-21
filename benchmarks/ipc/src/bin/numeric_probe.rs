//! Independent codec-only profiling probe; does not initialize the native runtime.
#![allow(non_snake_case)]
use anyhow::{Context, Result, bail, ensure};
use ipc::__private::{mojom_value_parser as codec, mojom_value_parser_core::UntypedHandle};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    hint::black_box,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
mod protocol {
    include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
}
use protocol::bench::*;

const ID: u64 = 0x1234_5678_9abc_def0;
const SAMPLE_CAPACITY: usize = 100_000;
type Encoded = (Vec<u8>, Vec<UntypedHandle>, u64);

#[derive(Clone, Copy)]
enum Operation {
    Encode,
    Decode,
    Pair,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Encode => "encode",
            Self::Decode => "decode",
            Self::Pair => "pair",
        }
    }
}
#[derive(Clone, Copy)]
enum Pattern {
    AllNone,
    AllSome,
    Mixed,
}
impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Self::AllNone => "all_none",
            Self::AllSome => "all_some",
            Self::Mixed => "mixed",
        }
    }
    fn some(self, index: usize) -> bool {
        match self {
            Self::AllNone => false,
            Self::AllSome => true,
            Self::Mixed => index % 3 != 0,
        }
    }
    fn nulls(self, elements: usize) -> usize {
        match self {
            Self::AllNone => elements,
            Self::AllSome => 0,
            Self::Mixed => elements.div_ceil(3),
        }
    }
}
struct Options {
    case: String,
    operation: Operation,
    pattern: Pattern,
    seconds: f64,
    minimum: u64,
    warmup: u64,
    start_gate: bool,
    output: Option<PathBuf>,
}
impl Options {
    fn parse() -> Result<Option<Self>> {
        let mut options = Self {
            case: "u8-1m".into(),
            operation: Operation::Pair,
            pattern: Pattern::Mixed,
            seconds: 15.0,
            minimum: 8,
            warmup: 8,
            start_gate: false,
            output: None,
        };
        let mut pattern_set = false;
        let mut args = std::env::args_os().skip(1);
        while let Some(argument) = args.next() {
            let argument = argument.to_str().context("option name must be UTF-8")?;
            let (name, inline) = argument
                .split_once('=')
                .map_or((argument, None), |(name, value)| (name, Some(value)));
            if matches!(name, "--help" | "-h") {
                ensure!(inline.is_none(), "help takes no value");
                println!(
                    "Usage: codec_probe [OPTIONS]\n\
                    --case NAME        empty | u8-1m | bool-64k | nullable-bool-64k | nullable-u32-64k | nullable-u64-64k | nullable-float-64k\n\
                    --operation MODE   encode | decode | pair (default: pair)\n\
                    --pattern NAME     all_none | all_some | mixed; nullable cases only (default: mixed)\n\
                    --seconds N        Minimum measured wall duration (default: 15; positive, 2 for smoke)\n\
                    --iterations N     Minimum completed operations/pairs (default: 8)\n\
                    --warmup N         Unmeasured operations/pairs (default: 8; zero allowed)\n\
                    --start-gate       After warmup emit probe_ready, then require START on stdin\n\
                    --output PATH      Write final JSON to a new file; stdout always includes JSONL\n\
                    No native library is loaded. Profiled timings are diagnostic, not formal benchmark data."
                );
                return Ok(None);
            }
            if name == "--start-gate" {
                ensure!(inline.is_none(), "--start-gate takes no value");
                options.start_gate = true;
                continue;
            }
            ensure!(
                matches!(
                    name,
                    "--case"
                        | "--operation"
                        | "--pattern"
                        | "--seconds"
                        | "--iterations"
                        | "--warmup"
                        | "--output"
                ),
                "unknown option {name}"
            );
            let value = inline
                .map(std::ffi::OsString::from)
                .or_else(|| args.next())
                .with_context(|| format!("{name} requires a value"))?;
            if name == "--output" {
                ensure!(!value.is_empty(), "output path must not be empty");
                options.output = Some(value.into());
                continue;
            }
            let value = value
                .to_str()
                .with_context(|| format!("{name} requires UTF-8"))?;
            match name {
                "--case" => options.case = value.to_owned(),
                "--operation" => {
                    options.operation = match value {
                        "encode" => Operation::Encode,
                        "decode" => Operation::Decode,
                        "pair" => Operation::Pair,
                        _ => bail!("invalid operation {value}"),
                    }
                }
                "--pattern" => {
                    options.pattern = match value {
                        "all_none" => Pattern::AllNone,
                        "all_some" => Pattern::AllSome,
                        "mixed" => Pattern::Mixed,
                        _ => bail!("invalid pattern {value}"),
                    };
                    pattern_set = true;
                }
                "--seconds" => options.seconds = value.parse().context("invalid seconds")?,
                "--iterations" => options.minimum = value.parse().context("invalid iterations")?,
                "--warmup" => options.warmup = value.parse().context("invalid warmup")?,
                _ => unreachable!(),
            }
        }
        ensure!(
            matches!(
                options.case.as_str(),
                "empty"
                    | "u8-1m"
                    | "bool-64k"
                    | "nullable-bool-64k"
                    | "nullable-u32-64k"
                    | "nullable-u64-64k"
                    | "nullable-float-64k"
            ),
            "invalid case {}",
            options.case
        );
        ensure!(
            !pattern_set || options.case.starts_with("nullable-"),
            "--pattern only applies to nullable cases"
        );
        ensure!(
            options.seconds.is_finite() && options.seconds > 0.0 && options.seconds <= 3600.0,
            "--seconds must be finite and in (0,3600]"
        );
        ensure!(options.minimum > 0, "--iterations must be positive");
        if let Some(path) = &options.output {
            ensure!(
                !path.exists(),
                "output file already exists: {}",
                path.display()
            );
        }
        Ok(Some(options))
    }
}

trait Case {
    type Params: codec::MojomParse<Self::Params, ()>;
    fn input(&self) -> Self::Params;
    fn validate(&self, actual: &Self::Params) -> Result<()>;
}
struct Empty;
impl Case for Empty {
    type Params = Bench_Info_Params;
    fn input(&self) -> Self::Params {
        Bench_Info_Params {}
    }
    fn validate(&self, _: &Self::Params) -> Result<()> {
        Ok(())
    }
}
// Extend this adapter with a custom equality function for float fields. Such
// fields must compare Option tags and IEEE bits, not OrderedFloat equality.
macro_rules! array_case {
    ($name:ident,$item:ty,$params:ident) => {
        array_case!(
            $name,
            $item,
            $params,
            |left: &[$item], right: &[$item]| left == right
        );
    };
    ($name:ident,$item:ty,$params:ident,$equal:expr) => {
        struct $name {
            data: Vec<$item>,
        }
        impl Case for $name {
            type Params = $params;
            fn input(&self) -> Self::Params {
                $params {
                    id: ID,
                    data: self.data.clone(),
                }
            }
            fn validate(&self, actual: &Self::Params) -> Result<()> {
                ensure!(actual.id == ID, "decoded request ID changed");
                ensure!(
                    ($equal)(&actual.data, &self.data),
                    "decoded values or nullable tags changed"
                );
                Ok(())
            }
        }
    };
}
array_case!(Bytes, u8, Bench_Echo_Params);
array_case!(Bools, bool, Bench_EchoBool_Params);
array_case!(NullableBools, Option<bool>, Bench_EchoNullableBool_Params);
array_case!(NullableU32, Option<u32>, Bench_EchoNullableUint32_Params);
array_case!(NullableU64, Option<u64>, Bench_EchoNullableUint64_Params);
use ipc::__private::ordered_float::OrderedFloat;
array_case!(
    NullableFloat,
    Option<OrderedFloat<f32>>,
    Bench_EchoNullableFloat_Params,
    |left: &[Option<OrderedFloat<f32>>], right: &[Option<OrderedFloat<f32>>]| {
        left.len() == right.len()
            && left.iter().zip(right).all(|(a, b)| match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => a.0.to_bits() == b.0.to_bits(),
                _ => false,
            })
    }
);

// Stable stack boundaries for sample; codec internals retain release optimizations.
// Keep preparation, validation and output destruction outside codec boundaries.
#[inline(never)]
fn profile_clone<C: Case>(fixture: &C) -> C::Params {
    black_box(fixture.input())
}
#[inline(never)]
fn profile_encode<P: codec::MojomParse<P, ()>>(input: P) -> Encoded {
    black_box(codec::serialize::<(), P>(black_box(input), &()))
}
#[inline(never)]
fn profile_decode<P: codec::MojomParse<P, ()>>(bytes: &[u8]) -> Result<P> {
    let result = codec::deserialize_exact::<(), P>(black_box(bytes), &mut [], 0, &())
        .context("decode probe Params");
    black_box(result)
}
#[inline(never)]
fn profile_validate_wire(encoded: &Encoded, oracle: &[u8]) -> Result<()> {
    ensure!(
        encoded.1.is_empty() && encoded.2 == 0,
        "unexpected handles or interface IDs"
    );
    ensure!(
        black_box(encoded.0.as_slice()) == black_box(oracle),
        "encoded wire bytes differ from preflight oracle"
    );
    Ok(())
}
#[inline(never)]
fn profile_validate<C: Case>(fixture: &C, actual: &C::Params) -> Result<()> {
    black_box(fixture.validate(black_box(actual)))
}
#[inline(never)]
fn profile_drop<T>(value: T) {
    drop(black_box(value));
}

#[derive(Default)]
struct Observation {
    clone_ns: Option<u64>,
    encode_ns: Option<u64>,
    decode_ns: Option<u64>,
    validation_ns: u64,
    drop_ns: u64,
}
fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}
fn iteration<C: Case>(fixture: &C, oracle: &[u8], operation: Operation) -> Result<Observation> {
    let mut row = Observation::default();
    match operation {
        Operation::Encode => {
            let start = Instant::now();
            let input = profile_clone(fixture);
            row.clone_ns = Some(nanos(start.elapsed()));
            let start = Instant::now();
            let encoded = profile_encode(input);
            row.encode_ns = Some(nanos(start.elapsed()));
            let start = Instant::now();
            profile_validate_wire(&encoded, oracle)?;
            row.validation_ns = nanos(start.elapsed());
            let start = Instant::now();
            profile_drop(encoded);
            row.drop_ns = nanos(start.elapsed());
        }
        Operation::Decode => {
            let start = Instant::now();
            let decoded = profile_decode::<C::Params>(oracle)?;
            row.decode_ns = Some(nanos(start.elapsed()));
            let start = Instant::now();
            profile_validate(fixture, &decoded)?;
            row.validation_ns = nanos(start.elapsed());
            let start = Instant::now();
            profile_drop(decoded);
            row.drop_ns = nanos(start.elapsed());
        }
        Operation::Pair => {
            let start = Instant::now();
            let input = profile_clone(fixture);
            row.clone_ns = Some(nanos(start.elapsed()));
            let start = Instant::now();
            let encoded = profile_encode(input);
            row.encode_ns = Some(nanos(start.elapsed()));
            let start = Instant::now();
            let decoded = profile_decode::<C::Params>(&encoded.0)?;
            row.decode_ns = Some(nanos(start.elapsed()));
            let start = Instant::now();
            profile_validate_wire(&encoded, oracle)?;
            profile_validate(fixture, &decoded)?;
            row.validation_ns = nanos(start.elapsed());
            let start = Instant::now();
            profile_drop((encoded, decoded));
            row.drop_ns = nanos(start.elapsed());
        }
    }
    Ok(row)
}

struct Stats {
    samples: Vec<u64>,
    count: u64,
    sum: u128,
    minimum: u64,
    maximum: u64,
    random: u64,
}
impl Default for Stats {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            count: 0,
            sum: 0,
            minimum: u64::MAX,
            maximum: 0,
            random: 0x243f_6a88_85a3_08d3,
        }
    }
}
impl Stats {
    fn record(&mut self, value: u64) {
        self.count += 1;
        self.sum += u128::from(value);
        self.minimum = self.minimum.min(value);
        self.maximum = self.maximum.max(value);
        if self.samples.len() < SAMPLE_CAPACITY {
            self.samples.push(value);
            return;
        }
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        let index = self.random.wrapping_mul(0x2545_f491_4f6c_dd1d) % self.count;
        if index < (SAMPLE_CAPACITY as u64) {
            self.samples[index as usize] = value;
        }
    }
    fn finish(mut self) -> Value {
        if self.count == 0 {
            return Value::Null;
        }
        self.samples.sort_unstable();
        let percentile = |percent: usize| {
            self.samples[(self.samples.len() * percent).div_ceil(100) - 1] as f64 / 1000.0
        };
        json!({"observations":self.count,"samples":self.samples.len(),"min_us":self.minimum as f64/1000.0,
            "mean_us":self.sum as f64/self.count as f64/1000.0,"max_us":self.maximum as f64/1000.0,
            "p50_us":percentile(50),"p99_us":percentile(99),"sampling":if self.count>SAMPLE_CAPACITY as u64 {"reservoir"} else {"all"}})
    }
}
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}
fn emit(value: &Value) -> Result<()> {
    let stdout = io::stdout();
    let mut stream = stdout.lock();
    serde_json::to_writer(&mut stream, value)?;
    writeln!(stream)?;
    stream.flush()?;
    Ok(())
}

fn run_case<C: Case>(fixture: C, layout: Value, options: &Options) -> Result<Value> {
    let oracle = profile_encode(profile_clone(&fixture));
    ensure!(
        oracle.1.is_empty() && oracle.2 == 0,
        "preflight produced handles or interface IDs"
    );
    if let Some(payload) = layout
        .get("array_wire_payload_bytes")
        .and_then(Value::as_u64)
    {
        let payload = usize::try_from(payload).context("array payload exceeds usize")?;
        ensure!(
            oracle.0.len() == 32 + payload.div_ceil(8) * 8,
            "preflight encoded body extent mismatch"
        );
        ensure!(
            u64::from_le_bytes(oracle.0[16..24].try_into().unwrap()) == 8,
            "preflight array pointer mismatch"
        );
        ensure!(
            u32::from_le_bytes(oracle.0[24..28].try_into().unwrap()) as usize == 8 + payload,
            "preflight array header size mismatch"
        );
        ensure!(
            Some(u64::from(u32::from_le_bytes(
                oracle.0[28..32].try_into().unwrap()
            ))) == layout.get("elements").and_then(Value::as_u64),
            "preflight element count mismatch"
        );
    } else {
        ensure!(oracle.0.len() == 8, "empty Params body is not eight bytes");
    }
    let decoded = profile_decode::<C::Params>(&oracle.0)?;
    profile_validate(&fixture, &decoded)?;
    profile_drop(decoded);
    let wire_hash = fnv1a64(&oracle.0);
    for _ in 0..options.warmup {
        black_box(iteration(&fixture, &oracle.0, options.operation)?);
    }
    emit(
        &json!({"event":"probe_ready","pid":std::process::id(),"case":options.case,"operation":options.operation.name(),
        "waiting_for_start":options.start_gate,"layout":layout}),
    )?;
    if options.start_gate {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        ensure!(line.trim() == "START", "start gate requires START on stdin");
    }
    let mut clones = Stats::default();
    let mut encodes = Stats::default();
    let mut decodes = Stats::default();
    let mut validations = Stats::default();
    let mut drops = Stats::default();
    let mut completed = 0_u64;
    let mut aggregate = 0_u64;
    let duration = Duration::try_from_secs_f64(options.seconds).context("invalid duration")?;
    let wall = Instant::now();
    while completed < options.minimum || wall.elapsed() < duration {
        let observation = iteration(&fixture, &oracle.0, options.operation)?;
        completed = completed
            .checked_add(1)
            .context("operation count overflow")?;
        if let Some(value) = observation.clone_ns {
            clones.record(value);
        }
        if let Some(value) = observation.encode_ns {
            encodes.record(value);
        }
        if let Some(value) = observation.decode_ns {
            decodes.record(value);
        }
        validations.record(observation.validation_ns);
        drops.record(observation.drop_ns);
        // Every encoded output matched the oracle, or every decoded output
        // matched its fixture. Hash the oracle once; this is a validated-wire
        // checksum plus an iteration accumulator, not a per-iteration hash cost.
        aggregate = aggregate.rotate_left(7) ^ wire_hash ^ completed;
        black_box(aggregate);
    }
    let seconds = wall.elapsed().as_secs_f64();
    let array_wire = layout
        .get("array_wire_payload_bytes")
        .and_then(Value::as_u64);
    Ok(
        json!({"ok":true,"schema_version":1,"probe":"codec_only","native_runtime_loaded":false,
            "target":env!("BENCH_TARGET"),"compiler":env!("BENCH_RUSTC"),"profile":env!("BENCH_PROFILE"),"opt_level":env!("BENCH_OPT_LEVEL"),
            "optimized":!cfg!(debug_assertions),"chromium_revision":"bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd",
            "configuration":{"case":options.case,"operation":options.operation.name(),"null_pattern":if options.case.starts_with("nullable-"){Some(options.pattern.name())} else {None},
                "minimum_seconds":options.seconds,"minimum_completed":options.minimum,"warmup_count":options.warmup,"start_gate":options.start_gate},
            "layout":layout,"encoded_body_bytes":oracle.0.len(),"completed":completed,"elapsed_seconds":seconds,
            "operations_per_second":completed as f64/seconds,"array_wire_payload_mib_per_second":array_wire.map(|size|completed as f64*size as f64/seconds/1_048_576.0),
            "timing":{"input_clone":clones.finish(),"encode":encodes.finish(),"decode":decodes.finish(),"validation":validations.finish(),"output_drop":drops.finish()},
            "checksum":{"validated_wire_fnv1a64":format!("{wire_hash:016x}"),"iteration_accumulator":format!("{aggregate:016x}"),"hashing_scope":"oracle hashed once before measurement; each output compared with oracle or validated against decoded fixture"},
            "timing_scope":"encode excludes input clone, validation and output drop; decode excludes validation and returned-value drop; internal codec allocations/drops remain included; wrapper and Instant overhead included",
            "throughput_scope":"complete selected operation/pair loop, including clone where needed, full validation, output drop, timers and statistics; pair counts once, not twice",
            "sampling_scope":"sample sees the complete process loop; attribute codec work using profile_encode/profile_decode subtrees, not total process samples; waiting stacks are not CPU cost",
            "quantiles":"nearest-rank of stored samples; min/mean/max cover all observations; encode/decode percentile sums are not pair percentiles",
            "diagnostic_only":true,"small_sample_tail_caution":completed<100,
        }),
    )
}
fn nullable_layout(bits: usize, count: usize) -> usize {
    let tags = count.div_ceil(8);
    if bits == 1 {
        tags * 2
    } else if count == 0 {
        0
    } else {
        tags.div_ceil(bits / 8) * (bits / 8) + count * (bits / 8)
    }
}
fn nullable_count(bits: usize, budget: usize) -> usize {
    let (mut low, mut high) = (0, budget * 8 + 1);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if nullable_layout(bits, middle) <= budget {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}
fn layout(bits: usize, count: usize, budget: usize, nullable: bool, pattern: Pattern) -> Value {
    let actual = if nullable {
        nullable_layout(bits, count)
    } else {
        (count * bits).div_ceil(8)
    };
    let tags = if nullable { count.div_ceil(8) } else { 0 };
    let values = (count * bits).div_ceil(8);
    json!({"elements":count,"element_bits":bits,"nullable_elements":nullable,"null_count":if nullable {pattern.nulls(count)} else {0},
        "target_array_wire_payload_bytes":budget,"array_wire_payload_bytes":actual,"array_tag_bitmap_bytes":tags,"array_value_bytes":values,
        "array_internal_padding_bytes":actual-tags-values,"array_header_bytes":8,"array_trailing_alignment_bytes":actual.div_ceil(8)*8-actual,
        "wire_scope":"array data includes tags/values/internal padding; excludes array header and trailing alignment, struct/message headers, ID and pointer"})
}
fn run(options: &Options) -> Result<Value> {
    match options.case.as_str() {
        "empty" => run_case(
            Empty,
            json!({"empty_params":true,"elements":0,"array_wire_payload_bytes":null,"wire_scope":"empty Bench_Info_Params struct; no array; eight-byte struct header"}),
            options,
        ),
        "u8-1m" => {
            let count = 1_048_576;
            run_case(
                Bytes {
                    data: (0..count)
                        .map(|index| ((index * 131 + 17) % 251) as u8)
                        .collect(),
                },
                layout(8, count, count, false, options.pattern),
                options,
            )
        }
        "bool-64k" => {
            let count = 65_536 * 8;
            run_case(
                Bools {
                    data: (0..count).map(|index| index % 7 < 3).collect(),
                },
                layout(1, count, 65_536, false, options.pattern),
                options,
            )
        }
        "nullable-bool-64k" => {
            let count = nullable_count(1, 65_536);
            run_case(
                NullableBools {
                    data: (0..count)
                        .map(|index| options.pattern.some(index).then_some(index % 7 < 3))
                        .collect(),
                },
                layout(1, count, 65_536, true, options.pattern),
                options,
            )
        }
        "nullable-u32-64k" => {
            let count = nullable_count(32, 65_536);
            run_case(
                NullableU32 {
                    data: (0..count)
                        .map(|index| {
                            options
                                .pattern
                                .some(index)
                                .then_some((index as u32).wrapping_mul(0x9e37_79b9))
                        })
                        .collect(),
                },
                layout(32, count, 65_536, true, options.pattern),
                options,
            )
        }
        "nullable-u64-64k" => {
            let count = nullable_count(64, 65_536);
            run_case(
                NullableU64 {
                    data: (0..count)
                        .map(|index| {
                            options
                                .pattern
                                .some(index)
                                .then_some((index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
                        })
                        .collect(),
                },
                layout(64, count, 65_536, true, options.pattern),
                options,
            )
        }
        "nullable-float-64k" => {
            let count = nullable_count(32, 65_536);
            const BITS: [u32; 8] = [
                0, 0x80000000, 0x7fc12345, 0x7fa01234, 0x7f800000, 0xff800000, 0x3f800001,
                0xbf812345,
            ];
            run_case(
                NullableFloat {
                    data: (0..count)
                        .map(|index| {
                            options
                                .pattern
                                .some(index)
                                .then_some(OrderedFloat(f32::from_bits(BITS[index % 8])))
                        })
                        .collect(),
                },
                layout(32, count, 65_536, true, options.pattern),
                options,
            )
        }
        _ => unreachable!("validated case"),
    }
}
fn main() {
    let result = (|| -> Result<()> {
        let Some(options) = Options::parse()? else {
            return Ok(());
        };
        let result = run(&options)?;
        if let Some(path) = &options.output {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| format!("create {}", path.display()))?;
            serde_json::to_writer_pretty(&mut file, &result)?;
            writeln!(file)?;
        }
        emit(&json!({"event":"probe_finished","result":result}))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = emit(&json!({"event":"probe_failed","error":format!("{error:#}")}));
        eprintln!("codec probe failed: {error:#}");
        std::process::exit(1);
    }
}
