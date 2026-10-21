use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, BufRead, Write},
    path::PathBuf,
    thread,
    time::Duration,
};

pub const LEGACY_SIZES: [usize; 5] = [0, 64, 1024, 65_536, 1_048_576];
pub const LEGACY_PHASES: [&str; 8] = [
    "codec",
    "raw",
    "typed",
    "oneway",
    "correctness",
    "stress",
    "churn",
    "crash",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticOperation {
    Typed,
    Raw,
    CodecEncode,
    CodecDecode,
    CodecPair,
}
impl DiagnosticOperation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Typed => "typed",
            Self::Raw => "raw",
            Self::CodecEncode => "codec-encode",
            Self::CodecDecode => "codec-decode",
            Self::CodecPair => "codec-pair",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Diagnostic {
    pub payload_bytes: usize,
    pub operation: DiagnosticOperation,
    pub start_gate: bool,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub output: PathBuf,
    pub iterations: u64,
    pub warmup: u64,
    pub sample_ms: u64,
    pub stress_seconds: u64,
    pub churn: u64,
    pub window: usize,
    pub timeout_seconds: u64,
    pub matrices: Vec<String>,
    pub complex_cases: Vec<String>,
    pub complex_codec_ms: u64,
    pub complex_typed_ms: u64,
    pub complex_iterations: u64,
    pub complex_warmup: u64,
    pub legacy_sizes: Vec<usize>,
    pub legacy_phases: Vec<String>,
    pub diagnostic: Option<Diagnostic>,
}

impl Options {
    pub fn matrix(&self, name: &str) -> bool {
        self.matrices.iter().any(|selected| selected == name)
    }
    pub fn legacy_phase(&self, name: &str) -> bool {
        self.legacy_phases.iter().any(|phase| phase == name)
    }
    pub fn complex_watchdog_seconds(&self) -> u64 {
        if self.matrix("complex") {
            self.complex_typed_ms
                .saturating_mul(self.complex_cases.len() as u64)
                .saturating_mul(3)
                .div_ceil(1000)
        } else {
            0
        }
    }
    pub fn parse() -> Result<Option<Self>> {
        let mut options = Self {
            output: PathBuf::from("benchmark-result.json"),
            iterations: 5_000,
            warmup: 500,
            sample_ms: 1_000,
            stress_seconds: 30,
            churn: 100,
            window: 64,
            timeout_seconds: 15,
            matrices: vec!["legacy".into(), "primitive".into(), "nullable".into()],
            complex_cases: crate::complex_cases::CASE_NAMES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            complex_codec_ms: 1_000,
            complex_typed_ms: 1_000,
            complex_iterations: 8,
            complex_warmup: 2,
            legacy_sizes: LEGACY_SIZES.to_vec(),
            legacy_phases: LEGACY_PHASES
                .iter()
                .map(|phase| (*phase).to_owned())
                .collect(),
            diagnostic: None,
        };
        let mut diagnostic_case = None;
        let mut diagnostic_operation = None;
        let mut start_gate = false;
        let mut window_set = false;
        let mut arguments = std::env::args_os().skip(1);
        while let Some(argument) = arguments.next() {
            let argument = argument.to_str().context(
                "option names must be valid UTF-8; pass non-UTF-8 output paths after --output",
            )?;
            let (name, inline_value) = argument
                .split_once('=')
                .map_or((argument, None), |(name, value)| (name, Some(value)));
            if name == "--help" || name == "-h" {
                if inline_value.is_some() {
                    bail!("{name} does not take a value");
                }
                println!(
                    "Usage: mojo-ipc-bench [OPTIONS]\n\
                     \n\
                     --output PATH          JSON result file (default: benchmark-result.json)\n\
                     --iterations N         Minimum RPCs per case; default matrix caps large cases, dedicated diagnostics do not (default: 5000)\n\
                     --warmup N             Warmup RPCs per case (default: 500; zero allowed)\n\
                     --sample-ms N          Minimum measurement time per case (default: 1000; zero=count only)\n\
                     --stress-seconds N     Stress duration (default: 30; zero disables)\n\
                     --churn N              Spawn/close cycles (default: 100; zero disables)\n\
                     --window N             Maximum outstanding requests (default: 64; 1..=4096)\n\
                     --timeout-seconds N    Progress/child timeout (default: 15; positive)\n\
                     --matrices LIST        Comma-separated legacy,primitive,nullable,complex (default: first three)\n\
                     --complex-cases LIST   Comma-separated complex case names (default: all nine)\n\
                     --complex-codec-ms N   Per complex codec case minimum ms (default: 1000; zero=count only)\n\
                     --complex-typed-ms N   Per complex typed case minimum ms (default: 1000; zero=count only)\n\
                     --complex-iterations N Minimum samples per complex case (default: 8; use >=200 for formal runs)\n\
                     --complex-warmup N     Complex warmup count (default: 2; zero allowed)\n\
                     --legacy-sizes LIST    Subset of 0,64,1024,65536,1048576 (default: all)\n\
                     --legacy-phases LIST   Subset of codec,raw,typed,oneway,correctness,stress,churn,crash (default: all)\n\
                     --case SIZE            Dedicated diagnostic payload: 0,64,1024,65536,1048576; requires --operation\n\
                     --operation MODE       Diagnostic typed|raw|codec-encode|codec-decode|codec-pair; serial window1\n\
                     --start-gate           Diagnostic only: after warmup emit parent/child PIDs, wait for START on stdin\n\
                     -h, --help             Print this help\n\
                     \n\
                     Options accept --name VALUE or --name=VALUE.\n\
                     MOJO_BENCH_SUPERVISED=1 enables checkpoint ACKs on stdin."
                );
                return Ok(None);
            }
            if name == "--start-gate" {
                if inline_value.is_some() {
                    bail!("--start-gate does not take a value");
                }
                start_gate = true;
                continue;
            }
            if !matches!(
                name,
                "--output"
                    | "--iterations"
                    | "--warmup"
                    | "--sample-ms"
                    | "--stress-seconds"
                    | "--churn"
                    | "--window"
                    | "--timeout-seconds"
                    | "--matrices"
                    | "--complex-cases"
                    | "--complex-codec-ms"
                    | "--complex-typed-ms"
                    | "--complex-iterations"
                    | "--complex-warmup"
                    | "--legacy-sizes"
                    | "--legacy-phases"
                    | "--case"
                    | "--operation"
            ) {
                bail!("unknown argument {name:?}; use --help");
            }
            let value = inline_value
                .map(OsString::from)
                .or_else(|| arguments.next())
                .with_context(|| format!("{name} requires a value"))?;
            if name == "--output" {
                if value.is_empty() {
                    bail!("--output requires a nonempty path");
                }
                options.output = PathBuf::from(value);
                continue;
            }
            if name == "--operation" {
                diagnostic_operation =
                    Some(match value.to_str().context("operation must be UTF-8")? {
                        "typed" => DiagnosticOperation::Typed,
                        "raw" => DiagnosticOperation::Raw,
                        "codec-encode" => DiagnosticOperation::CodecEncode,
                        "codec-decode" => DiagnosticOperation::CodecDecode,
                        "codec-pair" => DiagnosticOperation::CodecPair,
                        other => bail!("invalid diagnostic operation {other:?}"),
                    });
                continue;
            }
            if name == "--legacy-sizes" {
                let mut selected = Vec::new();
                for item in value
                    .to_str()
                    .context("size selection must be UTF-8")?
                    .split(',')
                {
                    let size = item
                        .parse::<usize>()
                        .context("legacy sizes must be unsigned integers")?;
                    if !LEGACY_SIZES.contains(&size) || selected.contains(&size) {
                        bail!("invalid or duplicate legacy size {item:?}");
                    }
                    selected.push(size);
                }
                options.legacy_sizes = selected;
                continue;
            }
            if name == "--legacy-phases" {
                let mut selected = Vec::new();
                for item in value
                    .to_str()
                    .context("phase selection must be UTF-8")?
                    .split(',')
                {
                    if !LEGACY_PHASES.contains(&item) || selected.iter().any(|phase| phase == item)
                    {
                        bail!("invalid or duplicate legacy phase {item:?}");
                    }
                    selected.push(item.to_owned());
                }
                options.legacy_phases = selected;
                continue;
            }
            if name == "--matrices" || name == "--complex-cases" {
                let text = value.to_str().context("selection must be UTF-8")?;
                let allowed: &[&str] = if name == "--matrices" {
                    &["legacy", "primitive", "nullable", "complex"]
                } else {
                    &crate::complex_cases::CASE_NAMES
                };
                let mut selected = Vec::new();
                for item in text.split(',') {
                    if !allowed.contains(&item) || selected.iter().any(|previous| previous == item)
                    {
                        bail!(
                            "invalid or duplicate {name} item {item:?}; allowed: {}",
                            allowed.join(",")
                        );
                    }
                    selected.push(item.to_owned());
                }
                if name == "--matrices" {
                    options.matrices = selected;
                } else {
                    options.complex_cases = selected;
                }
                continue;
            }
            let number: u64 = value
                .to_str()
                .with_context(|| format!("{name} requires an unsigned integer"))?
                .parse()
                .with_context(|| format!("{name} requires an unsigned integer"))?;
            match name {
                "--iterations" => {
                    if number == 0 {
                        bail!("--iterations must be positive");
                    }
                    options.iterations = number;
                }
                "--warmup" => options.warmup = number,
                "--sample-ms" => options.sample_ms = number,
                "--stress-seconds" => options.stress_seconds = number,
                "--churn" => options.churn = number,
                "--window" => {
                    window_set = true;
                    if !(1..=4_096).contains(&number) {
                        bail!("--window must be between 1 and 4096");
                    }
                    options.window = number as usize;
                }
                "--timeout-seconds" => {
                    if number == 0 {
                        bail!("--timeout-seconds must be positive");
                    }
                    options.timeout_seconds = number;
                }
                "--complex-codec-ms" => options.complex_codec_ms = number,
                "--complex-typed-ms" => options.complex_typed_ms = number,
                "--complex-warmup" => options.complex_warmup = number,
                "--case" => {
                    let size =
                        usize::try_from(number).context("diagnostic case size exceeds usize")?;
                    if !LEGACY_SIZES.contains(&size) {
                        bail!("invalid diagnostic case size {size}");
                    }
                    diagnostic_case = Some(size);
                }
                "--complex-iterations" => {
                    if number == 0 {
                        bail!("--complex-iterations must be positive");
                    }
                    options.complex_iterations = number;
                }
                _ => unreachable!("option names were validated above"),
            }
        }
        match (diagnostic_case, diagnostic_operation) {
            (Some(payload_bytes), Some(operation)) => {
                if window_set && options.window != 1 {
                    bail!("dedicated diagnostics require --window=1");
                }
                if options.output.exists() {
                    bail!(
                        "diagnostic output already exists: {}",
                        options.output.display()
                    );
                }
                options.window = 1;
                options.diagnostic = Some(Diagnostic {
                    payload_bytes,
                    operation,
                    start_gate,
                });
            }
            (None, None) if !start_gate => {}
            _ => bail!(
                "--case and --operation must be supplied together; --start-gate requires both"
            ),
        }
        Ok(Some(options))
    }
}

/// Summarize parent-clock round-trip samples, using nearest-rank percentiles.
pub fn summarize_ns(samples: &mut [u64]) -> Value {
    if samples.is_empty() {
        return json!({
            "samples": 0,
            "min_us": null,
            "mean_us": null,
            "p50_us": null,
            "p90_us": null,
            "p95_us": null,
            "p99_us": null,
            "max_us": null,
        });
    }
    samples.sort_unstable();
    let percentile_us = |percent: u128| {
        let rank = (samples.len() as u128 * percent).div_ceil(100);
        samples[(rank - 1) as usize] as f64 / 1_000.0
    };
    // u128 holds the sum even for a usize-sized slice of u64 samples.
    let sum_ns: u128 = samples.iter().map(|&sample| u128::from(sample)).sum();
    json!({
        "samples": samples.len(),
        "min_us": samples[0] as f64 / 1_000.0,
        "mean_us": sum_ns as f64 / samples.len() as f64 / 1_000.0,
        "p50_us": percentile_us(50),
        "p90_us": percentile_us(90),
        "p95_us": percentile_us(95),
        "p99_us": percentile_us(99),
        "max_us": samples[samples.len() - 1] as f64 / 1_000.0,
    })
}

/// A reproducible nonuniform byte pattern; every byte is nonzero.
pub fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251 + 1) as u8).collect()
}

/// Emit one complete, flushed JSON line without interleaving stdout writers.
pub fn event(value: Value) {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if let Err(error) = writeln!(output, "{value}").and_then(|_| output.flush()) {
        eprintln!("failed to write benchmark JSONL event: {error}");
        std::process::exit(1);
    }
}

/// Call only from the harness main thread after in-flight requests are drained.
/// A supervisor can sample resources before allowing the next stage to proceed.
pub fn checkpoint(stage: &str, iteration: u64) -> Result<()> {
    thread::sleep(Duration::from_millis(50));
    let (mach_port_name_count, mach_port_probe_error) = mach_port_probe();
    event(json!({
        "event": "checkpoint",
        "stage": stage,
        "iteration": iteration,
        "mach_port_name_count": mach_port_name_count,
        "mach_port_probe_error": mach_port_probe_error,
    }));
    if std::env::var_os("MOJO_BENCH_SUPERVISED").is_some_and(|value| value == "1") {
        let mut acknowledgement = String::new();
        let count = io::stdin()
            .lock()
            .read_line(&mut acknowledgement)
            .context("read benchmark checkpoint ACK from supervisor")?;
        if count == 0 {
            bail!("supervisor closed stdin before acknowledging checkpoint {stage:?}");
        }
        if acknowledgement.trim_end_matches(['\r', '\n']) != "ACK" {
            bail!("expected ACK for checkpoint {stage:?}, got {acknowledgement:?}");
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn mach_port_probe() -> (Option<u32>, Option<String>) {
    (
        None,
        Some("unsupported: Mach port names are only available on macOS".to_owned()),
    )
}

#[cfg(target_os = "macos")]
fn mach_port_probe() -> (Option<u32>, Option<String>) {
    // Warm up the current thread's MIG reply-port machinery once before recording
    // the first checkpoint. Queries and VM-array cleanup do not change port rights.
    static WARMUP: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    if let Err(error) = WARMUP.get_or_init(|| darwin_mach_ports::count().map(|_| ())) {
        return (
            None,
            Some(format!("Mach port probe warmup failed: {error}")),
        );
    }
    match darwin_mach_ports::count() {
        Ok(count) => (Some(count), None),
        Err(error) => (None, Some(error)),
    }
}

#[cfg(target_os = "macos")]
mod darwin_mach_ports {
    use std::{ffi::c_int, ptr};

    // Darwin LP64: Mach names, types and counts are natural_t (u32), while
    // vm_address_t and vm_size_t are uintptr_t. mach_task_self_ is a variable.
    #[link(name = "System")]
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn mach_port_names(
            task: u32,
            names: *mut *mut u32,
            names_count: *mut u32,
            types: *mut *mut u32,
            types_count: *mut u32,
        ) -> c_int;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> c_int;
    }

    pub(super) fn count() -> std::result::Result<u32, String> {
        let mut names = ptr::null_mut();
        let mut types = ptr::null_mut();
        let mut names_count = 0;
        let mut types_count = 0;
        // SAFETY: the task port belongs to this process; all out-pointers point
        // to initialized, writable locals of the SDK-declared ABI types.
        let task = unsafe { mach_task_self_ };
        let status = unsafe {
            mach_port_names(
                task,
                &mut names,
                &mut names_count,
                &mut types,
                &mut types_count,
            )
        };
        if status != 0 {
            return Err(format!("mach_port_names returned kern_return_t={status}"));
        }

        let mut errors = Vec::new();
        if names_count != types_count {
            errors.push(format!(
                "mach_port_names returned different array counts: names={names_count}, types={types_count}"
            ));
        }
        // MIG returns two independently owned out-of-line VM arrays. Release
        // both even if validation or either deallocation fails. Array elements
        // name existing rights: never mach_port_deallocate those elements.
        for (label, array, count) in [("names", names, names_count), ("types", types, types_count)]
        {
            if count == 0 {
                continue;
            }
            if array.is_null() {
                errors.push(format!(
                    "mach_port_names returned null {label} with count={count}"
                ));
                continue;
            }
            let bytes = count as usize * std::mem::size_of::<u32>();
            // SAFETY: a successful mach_port_names call supplied this VM array
            // and its element count; vm_deallocate accepts the unrounded size.
            let status = unsafe { vm_deallocate(task, array as usize, bytes) };
            if status != 0 {
                errors.push(format!(
                    "vm_deallocate({label}) returned kern_return_t={status}"
                ));
            }
        }
        if errors.is_empty() {
            Ok(names_count)
        } else {
            Err(errors.join("; "))
        }
    }
}

pub fn timeout(options: &Options) -> Duration {
    Duration::from_secs(options.timeout_seconds)
}

#[cfg(test)]
mod tests {
    use super::summarize_ns;
    use serde_json::json;

    #[test]
    fn nearest_rank_does_not_interpolate_between_observations() {
        let mut samples: Vec<u64> = (1..=20).rev().map(|value| value * 1_000).collect();
        let summary = summarize_ns(&mut samples);
        assert_eq!(summary["samples"], 20);
        assert_eq!(summary["min_us"], 1.0);
        assert_eq!(summary["mean_us"], 10.5);
        assert_eq!(summary["p50_us"], 10.0);
        assert_eq!(summary["p90_us"], 18.0);
        assert_eq!(summary["p95_us"], 19.0);
        assert_eq!(summary["p99_us"], 20.0);
        assert_eq!(summary["max_us"], 20.0);
    }

    #[test]
    fn empty_samples_are_unavailable_rather_than_zero_latency() {
        assert_eq!(
            summarize_ns(&mut []),
            json!({
                "samples": 0,
                "min_us": null,
                "mean_us": null,
                "p50_us": null,
                "p90_us": null,
                "p95_us": null,
                "p99_us": null,
                "max_us": null,
            })
        );
    }
}
