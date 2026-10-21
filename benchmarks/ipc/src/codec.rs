//! In-process codec costs only: no native runtime, transport or dispatch calls.
use super::{
    protocol::bench::Bench_Echo_Params,
    support::{Options, event, payload, summarize_ns},
};
use anyhow::{Context, Result, ensure};
use ipc::__private::mojom_value_parser as codec;
use serde_json::{Value, json};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

const SIZES: [usize; 5] = [0, 64, 1_024, 65_536, 1_048_576];
const BYTE_BUDGET: usize = 1_048_576;
const SAMPLE_CAPACITY: usize = 100_000;

pub(super) struct Latencies {
    encode: Vec<u64>,
    decode: Vec<u64>,
    random: u64,
    encode_sum: u128,
    decode_sum: u128,
    encode_min: u64,
    decode_min: u64,
    encode_max: u64,
    decode_max: u64,
}

impl Latencies {
    pub(super) fn new() -> Self {
        Self {
            encode: Vec::with_capacity(SAMPLE_CAPACITY),
            decode: Vec::with_capacity(SAMPLE_CAPACITY),
            random: 0x243f_6a88_85a3_08d3,
            encode_sum: 0,
            decode_sum: 0,
            encode_min: u64::MAX,
            decode_min: u64::MAX,
            encode_max: 0,
            decode_max: 0,
        }
    }

    pub(super) fn record(&mut self, count: u64, encode: u64, decode: u64) {
        self.encode_sum += u128::from(encode);
        self.decode_sum += u128::from(decode);
        self.encode_min = self.encode_min.min(encode);
        self.decode_min = self.decode_min.min(decode);
        self.encode_max = self.encode_max.max(encode);
        self.decode_max = self.decode_max.max(decode);
        if self.encode.len() < SAMPLE_CAPACITY {
            self.encode.push(encode);
            self.decode.push(decode);
            return;
        }
        // Algorithm R with a reproducible xorshift64* stream. Keep both timings
        // from the same observation and bound storage for long --sample-ms runs.
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        let slot = self.random.wrapping_mul(0x2545_f491_4f6c_dd1d) % count;
        if slot < SAMPLE_CAPACITY as u64 {
            self.encode[slot as usize] = encode;
            self.decode[slot as usize] = decode;
        }
    }

    pub(super) fn summaries(&mut self, count: u64) -> (Value, Value) {
        let mut encode = summarize_ns(&mut self.encode);
        let mut decode = summarize_ns(&mut self.decode);
        for (summary, sum, minimum, maximum) in [
            (
                &mut encode,
                self.encode_sum,
                self.encode_min,
                self.encode_max,
            ),
            (
                &mut decode,
                self.decode_sum,
                self.decode_min,
                self.decode_max,
            ),
        ] {
            // Quantiles use the reservoir; these three statistics cover every
            // completed pair even if latency sample storage reached its cap.
            summary["observations"] = json!(count);
            summary["mean_us"] = json!(sum as f64 / count as f64 / 1_000.0);
            summary["min_us"] = json!(minimum as f64 / 1_000.0);
            summary["max_us"] = json!(maximum as f64 / 1_000.0);
        }
        (encode, decode)
    }
}

fn nanoseconds(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn pair(expected: &[u8], id: u64) -> Result<(u64, u64, usize)> {
    // Allocation/copy of the input is outside the encode latency interval.
    let input = black_box(Bench_Echo_Params {
        id,
        data: expected.to_vec(),
    });
    let started = Instant::now();
    let (bytes, handles, offset) = codec::serialize::<(), Bench_Echo_Params>(input, &());
    let encode_ns = nanoseconds(started.elapsed());
    ensure!(handles.is_empty(), "byte-only codec case produced handles");
    ensure!(offset == 0, "byte-only codec case produced interface IDs");

    let started = Instant::now();
    let decoded =
        codec::deserialize_exact::<(), Bench_Echo_Params>(black_box(&bytes), &mut [], offset, &())
            .context("decode in-process Bench_Echo_Params")?;
    let decode_ns = nanoseconds(started.elapsed());
    // Validation and destruction of the returned byte buffer are outside the
    // decode interval. Intermediate values freed within the codec are included.
    ensure!(decoded.id == id, "codec round-trip changed the request ID");
    ensure!(
        decoded.data.as_slice() == expected,
        "codec round-trip changed payload bytes"
    );
    black_box(&decoded);
    Ok((encode_ns, decode_ns, bytes.len()))
}

pub fn benchmark(options: &Options) -> Result<Vec<Value>> {
    let mut rows = Vec::with_capacity(SIZES.len());
    for size in SIZES
        .into_iter()
        .filter(|size| options.legacy_sizes.contains(size))
    {
        let per_size_budget = (BYTE_BUDGET / size.max(1)) as u64;
        let warmup = options.warmup.min(per_size_budget.max(2));
        let minimum = options.iterations.min(per_size_budget.max(8));
        let minimum_duration = Duration::from_millis(options.sample_ms);
        let expected = payload(size);
        event(json!({
            "event": "case_started", "name": "codec_only", "payload_bytes": size,
            "warmup_pairs": warmup, "minimum_pairs": minimum,
            "minimum_sample_ms": options.sample_ms,
        }));
        for id in 0..warmup {
            pair(&expected, id)?;
        }

        let mut latencies = Latencies::new();
        let mut completed = 0_u64;
        let mut encoded_bytes = None;
        let started = Instant::now();
        while completed < minimum || started.elapsed() < minimum_duration {
            let (encode_ns, decode_ns, size_on_wire) = pair(&expected, completed)?;
            if let Some(previous) = encoded_bytes {
                ensure!(
                    previous == size_on_wire,
                    "encoded size changed within one codec case"
                );
            } else {
                encoded_bytes = Some(size_on_wire);
            }
            completed = completed
                .checked_add(1)
                .context("codec pair count overflow")?;
            latencies.record(completed, encode_ns, decode_ns);
        }
        let elapsed = started.elapsed().as_secs_f64();
        let pairs_per_second = completed as f64 / elapsed;
        let payload_mib_per_second = pairs_per_second * size as f64 / 1_048_576.0;
        let (encode_latency, decode_latency) = latencies.summaries(completed);
        let row = json!({
            "name": "codec_only",
            "scope": "in-process Mojom encode/decode; not an IPC measurement",
            "native_runtime_required": false,
            "payload_bytes": size,
            "encoded_body_bytes": encoded_bytes,
            "warmup_pairs": warmup,
            "minimum_pairs": minimum,
            "completed": completed,
            "elapsed_seconds": elapsed,
            "encode_latency": encode_latency,
            "decode_latency": decode_latency,
            "latency_sampling": {
                "mode": if completed > SAMPLE_CAPACITY as u64 { "paired_reservoir" } else { "all" },
                "capacity": SAMPLE_CAPACITY,
                "quantiles": "nearest-rank of stored samples",
                "min_mean_max": "all completed observations",
            },
            "codec_pairs_per_second": pairs_per_second,
            "payload_mib_per_second": payload_mib_per_second,
            "encode_plus_decode_payload_mib_per_second": 2.0 * payload_mib_per_second,
            "throughput_scope": "full pair loop including input clone and output validation; one payload per pair",
        });
        event(json!({"event": "case_finished", "result": &row}));
        rows.push(row);
    }
    Ok(rows)
}
