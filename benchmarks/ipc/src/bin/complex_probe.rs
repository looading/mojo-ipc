//! Isolated complex-value codec probe, sharing the typed harness's fixtures.
#![allow(non_snake_case, dead_code)]
use anyhow::{Context, Result, bail, ensure};
use ipc::__private::{mojom_value_parser as mojom, mojom_value_parser_core::UntypedHandle};
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
#[path = "../codec.rs"]
mod codec;
#[path = "../complex_cases.rs"]
mod complex_cases;
#[path = "../support.rs"]
mod support;
use complex_cases::{ComplexCase, Fixture, all_cases};
const ID: u64 = 0x1234_5678_9abc_def0;
type Encoded = (Vec<u8>, Vec<UntypedHandle>, u64);
#[derive(Clone, Copy, PartialEq)]
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
struct Options {
    case: String,
    size: &'static str,
    budget: usize,
    operation: Operation,
    seconds: f64,
    minimum: u64,
    warmup: u64,
    start_gate: bool,
    output: Option<PathBuf>,
}
impl Options {
    fn parse() -> Result<Option<Self>> {
        let mut o = Self {
            case: "record_array".into(),
            size: "large",
            budget: 131_072,
            operation: Operation::Pair,
            seconds: 15.0,
            minimum: 8,
            warmup: 2,
            start_gate: false,
            output: None,
        };
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            let arg = arg.to_str().context("option must be UTF-8")?;
            let (name, inline) = arg
                .split_once('=')
                .map_or((arg, None), |(k, v)| (k, Some(v)));
            if matches!(name, "--help" | "-h") {
                ensure!(inline.is_none(), "help takes no value");
                println!(
                    "Usage: complex_probe [OPTIONS]\n--case NAME        utf8_string|record|record_array|nested_array|string_map|numeric_map|union_array|nullable_nested|string_array\n--size NAME        small|medium|large (body budgets 1KiB/16KiB/128KiB; default large)\n--operation MODE   encode|decode|pair (default pair)\n--seconds N        Minimum measured wall seconds (default 15; 0 allowed for count-only)\n--iterations N     Minimum completed operations (default 8)\n--warmup N         Unmeasured complete operations (default 2; 0 allowed)\n--start-gate       After warmup emit probe_ready with PID; wait for START on stdin\n--output PATH      New JSON output file (stdout always emits JSONL)\nNo native runtime is initialized. Full clone/validation/drop wall cost is reported separately."
                );
                return Ok(None);
            }
            if name == "--start-gate" {
                ensure!(inline.is_none(), "start gate takes no value");
                o.start_gate = true;
                continue;
            }
            ensure!(
                matches!(
                    name,
                    "--case"
                        | "--size"
                        | "--operation"
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
                .with_context(|| format!("{name} requires value"))?;
            if name == "--output" {
                ensure!(!value.is_empty(), "empty output path");
                o.output = Some(value.into());
                continue;
            }
            let value = value.to_str().context("value must be UTF-8")?;
            match name {
                "--case" => o.case = value.to_owned(),
                "--size" => {
                    let (size, budget) = complex_cases::SIZES
                        .iter()
                        .find(|(s, _)| *s == value)
                        .context("size must be small|medium|large")?;
                    o.size = size;
                    o.budget = *budget;
                }
                "--operation" => {
                    o.operation = match value {
                        "encode" => Operation::Encode,
                        "decode" => Operation::Decode,
                        "pair" => Operation::Pair,
                        _ => bail!("invalid operation"),
                    }
                }
                "--seconds" => o.seconds = value.parse().context("invalid seconds")?,
                "--iterations" => o.minimum = value.parse().context("invalid iterations")?,
                "--warmup" => o.warmup = value.parse().context("invalid warmup")?,
                _ => unreachable!(),
            }
        }
        ensure!(
            complex_cases::CASE_NAMES.contains(&o.case.as_str()),
            "invalid case {}",
            o.case
        );
        ensure!(
            o.seconds.is_finite() && (0.0..=3600.0).contains(&o.seconds),
            "seconds must be finite in [0,3600]"
        );
        ensure!(o.minimum > 0, "iterations must be positive");
        if let Some(path) = &o.output {
            ensure!(!path.exists(), "output already exists: {}", path.display());
        }
        Ok(Some(o))
    }
}
fn ns(d: Duration) -> u64 {
    d.as_nanos().min(u128::from(u64::MAX)) as u64
}
#[inline(never)]
fn profile_clone<C: ComplexCase>(data: &C::Data) -> C::Params {
    C::params(ID, black_box(data).clone())
}
#[inline(never)]
fn profile_encode<C: ComplexCase>(input: C::Params) -> Encoded {
    mojom::serialize::<(), C::Params>(black_box(input), &())
}
#[inline(never)]
fn profile_decode<C: ComplexCase>(bytes: &[u8]) -> Result<C::Params> {
    mojom::deserialize_exact::<(), C::Params>(black_box(bytes), &mut [], 0, &())
        .context("probe decode")
}
#[inline(never)]
fn profile_validate<C: ComplexCase>(fixture: &Fixture<C>, actual: C::Params) -> Result<C::Data> {
    let (id, data) = C::unpack(actual);
    ensure!(
        fixture.validate(ID, id, &data),
        "full content/bit/checksum mismatch"
    );
    Ok(data)
}
#[inline(never)]
fn profile_drop<T>(value: T) {
    drop(black_box(value));
}
fn iteration<C: ComplexCase>(o: &Options, f: &Fixture<C>, canonical: &[u8]) -> Result<(u64, u64)> {
    let mut e = 0;
    let mut d = 0;
    let encoded = if o.operation != Operation::Decode {
        let input = profile_clone::<C>(&f.data);
        let start = Instant::now();
        let tuple = profile_encode::<C>(input);
        e = ns(start.elapsed());
        ensure!(
            tuple.1.is_empty() && tuple.2 == 0 && tuple.0.len() == f.request_bytes,
            "encoded extent/handles mismatch"
        );
        Some(tuple)
    } else {
        None
    };
    let bytes = encoded
        .as_ref()
        .map_or(canonical, |tuple| tuple.0.as_slice());
    // Encode-only still decodes and validates each produced body, outside its
    // measured encode interval. Decode-only reuses the immutable preflight body.
    let actual = if o.operation == Operation::Encode {
        profile_decode::<C>(bytes)?
    } else {
        let start = Instant::now();
        let value = profile_decode::<C>(bytes)?;
        d = ns(start.elapsed());
        value
    };
    let data = profile_validate::<C>(f, actual)?;
    profile_drop(data);
    profile_drop(encoded);
    Ok((e, d))
}
fn run_case<C: ComplexCase>(o: &Options, output: &mut Option<Value>) -> Result<()> {
    if o.case != C::NAME {
        return Ok(());
    }
    let f = Fixture::<C>::new(o.size, o.budget)?;
    let canonical = complex_cases::encode::<C>(ID, f.data.clone())?;
    for _ in 0..o.warmup {
        iteration::<C>(o, &f, &canonical)?;
    }
    support::event(
        json!({"event":"probe_ready","pid":std::process::id(),"case":C::NAME,"operation":o.operation.name(),"size_label":o.size,"scale":f.scale,"encoded_body_bytes":f.request_bytes,"start_gate":o.start_gate}),
    );
    if o.start_gate {
        let mut line = String::new();
        ensure!(
            io::stdin().lock().read_line(&mut line)? > 0
                && line.trim_end_matches(['\r', '\n']) == "START",
            "expected START after warmup"
        );
    }
    let mut timings = codec::Latencies::new();
    let mut completed = 0;
    let started = Instant::now();
    while completed < o.minimum || started.elapsed() < Duration::from_secs_f64(o.seconds) {
        let (e, d) = iteration::<C>(o, &f, &canonical)?;
        completed += 1;
        timings.record(completed, e, d);
    }
    let seconds = started.elapsed().as_secs_f64();
    let (e, d) = timings.summaries(completed);
    let mut row = f.metadata();
    row.as_object_mut().unwrap().extend(json!({"ok":true,"scope":"isolated generated Mojom Params codec; no native library/runtime/transport initialized",
        "pid":std::process::id(),"operation":o.operation.name(),"completed":completed,"elapsed_seconds":seconds,
        "operations_per_second":completed as f64/seconds,"encoded_body_mib_per_second":completed as f64*f.request_bytes as f64/seconds/1_048_576.0,
        "encode_latency":if o.operation==Operation::Decode{Value::Null}else{e},"decode_latency":if o.operation==Operation::Encode{Value::Null}else{d},
        "configuration":{"seconds":o.seconds,"minimum_completed":o.minimum,"warmup_count":o.warmup,"start_gate":o.start_gate,"fixture_version":1},
        "target":env!("BENCH_TARGET"),"compiler":env!("BENCH_RUSTC"),"profile":env!("BENCH_PROFILE"),"opt_level":env!("BENCH_OPT_LEVEL"),
        "latency_sampling":{"capacity":100000,"mode":if completed>100000{"paired_reservoir"}else{"all"},"quantiles":"nearest-rank stored observations; min/mean/max all observations"},
        "latency_clock":"profile_encode/profile_decode only; input clone, final output full field/bit/checksum validation and drop outside; codec intermediate allocation/drop included",
        "throughput_clock":"complete wall loop includes clone (encode/pair), decode oracle even encode-only, full validation, drop and timing/statistics overhead; not isolated encode/decode throughput",
        "decode_input":"decode-only reuses one immutable generated body; pair and encode-only validate each newly encoded body; map key serialization order is not constrained",
        "small_sample_tail_caution":completed<100}).as_object().unwrap().clone());
    *output = Some(row);
    Ok(())
}
fn run() -> Result<()> {
    let Some(o) = Options::parse()? else {
        return Ok(());
    };
    ensure!(!cfg!(debug_assertions), "use release build for the probe");
    let mut output = None;
    all_cases!(run_case, &o, &mut output);
    let row = output.context("selected case did not execute")?;
    if let Some(path) = &o.output {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        serde_json::to_writer_pretty(&mut file, &row)?;
        file.write_all(b"\n")?;
    }
    support::event(json!({"event":"probe_finished","result":row}));
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        support::event(json!({"event":"probe_failed","error":format!("{error:#}")}));
        std::process::exit(1);
    }
}
