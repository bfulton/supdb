//! db_bench's workloads, run against Supdb.
//!
//! These are single-value-per-key benchmarks: fillrandom, readrandom, readseq
//! and friends. None of Supdb's specialisation applies here -- there are no
//! runs of values to gather into an extent, no fragmentation to repair, no
//! merge threshold to tune. That is precisely the point of running them: they
//! test the write path and the index on the terms the rest of the field is
//! designed for, rather than on ours.
//!
//! Key and value shapes match db_bench's defaults: sixteen-byte keys, hundred
//! byte values, and values that are about half compressible, as db_bench
//! produces with its default compression ratio.

use std::path::PathBuf;
use std::time::Instant;
use supdb::{Options, Reader, Reclaim, Store};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Sixteen decimal digits written into a caller-owned buffer.
///
/// This used to build a String with format! and convert it, allocating twice
/// per operation. Instrumenting a put showed the benchmark loop accounting for
/// nearly half the time being attributed to the store -- db_bench does not do
/// that, so neither should this.
#[inline]
fn db_key_into(n: u64, buf: &mut [u8; 16]) {
    let mut v = n;
    for i in (0..16).rev() {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
}

fn db_key(n: u64) -> Vec<u8> {
    let mut b = [0u8; 16];
    db_key_into(n, &mut b);
    b.to_vec()
}

/// db_bench's default value: roughly half of it compressible.
fn make_value(rng: &mut Rng, size: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(size);
    let half = size / 2;
    while v.len() < half {
        v.extend_from_slice(b"aaaaaaaabbbbbbbb");
    }
    v.truncate(half);
    while v.len() < size {
        v.push((rng.next() % 94 + 33) as u8);
    }
    v
}

fn emit(bench: &str, ops: u64, secs: f64, extra: &str) {
    println!(
        "{{\"engine\":\"supdb\",\"benchmark\":\"{}\",\"ops\":{},\"seconds\":{:.4},\"ops_per_s\":{:.1},\"micros_per_op\":{:.3}{}}}",
        bench, ops, secs, ops as f64 / secs, secs * 1e6 / ops as f64, extra
    );
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |n: &str, d: usize| -> usize {
        args.iter().position(|a| a == n).map(|i| args[i + 1].parse().unwrap()).unwrap_or(d)
    };
    let num = get("--num", 1_000_000) as u64;
    let value_size = get("--value-size", 100);
    let reads = get("--reads", 1_000_000) as u64;
    let dir: PathBuf = args
        .iter()
        .position(|a| a == "--dir")
        .map(|i| PathBuf::from(&args[i + 1]))
        .expect("--dir");
    let no_compress = args.iter().any(|a| a == "--no-compress");
    let buffer_mb = get("--buffer-mb", 64);
    let merge = get("--merge", 4);
    let solo_kb = get("--solo-kb", 16);
    let shards = get("--shards", 64);
    let benches: Vec<String> = args
        .iter()
        .position(|a| a == "--benchmarks")
        .map(|i| args[i + 1].split(',').map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec!["fillrandom".into(), "readrandom".into()]);

    std::fs::create_dir_all(&dir)?;
    let file = dir.join("supdb.dat");
    let mut rng = Rng(0x2545F4914F6CDD1D);

    for b in &benches {
        match b.as_str() {
            "fillrandom" | "fillseq" => {
                // One megabyte of data, sliced at a random offset per value --
                // db_bench's RandomGenerator, and for the same reason: the loop
                // should cost an index, not a generator. A pool of whole values
                // would repeat far more than a sliding window does and would
                // flatter the compressor.
                let window: Vec<u8> = {
                    let mut r = Rng(0xA5A5_5A5A);
                    let mut v = Vec::with_capacity(1 << 20);
                    while v.len() < (1 << 20) + value_size {
                        v.extend_from_slice(&make_value(&mut r, value_size));
                    }
                    v
                };
                let _ = std::fs::remove_file(&file);
                let store = Store::create(
                    &file,
                    Options {
                        buffer_bytes: buffer_mb << 20,
                        reclaim: Reclaim::AfterReads,
                        compress: !no_compress,
                        merge_threshold: merge,
                        solo_threshold: solo_kb * 1024,
                        shards,
                        ..Default::default()
                    },
                )?;
                let mut r = Rng(rng.next());
                let mut kb = [0u8; 16];
                let t = Instant::now();
                for i in 0..num {
                    let k = if b == "fillseq" { i } else { r.next() % num };
                    db_key_into(k, &mut kb);
                    let off = (r.next() as usize) % (1 << 20);
                    store.put(&kb, &window[off..off + value_size])?;
                }
                store.flush()?;
                let stats = store.close()?;
                let secs = t.elapsed().as_secs_f64();
                let sz = std::fs::metadata(&file)?.len();
                emit(b, num, secs, &format!(",\"file_mb\":{:.1},\"keys\":{}", sz as f64 / 1048576.0, stats.keys));
            }
            // Absent keys drawn from inside the populated range, so a miss is
            // indistinguishable from a hit until the index says otherwise.
            // db_bench's readmissing uses keys past the end of the range,
            // which is the easy case for any structure that can reject on a
            // prefix or a bound.
            "readmissing_dense" => {
                let reader = Reader::open(&file)?;
                let mut r = Rng(0x51ED);
                let (mut miss, mut hit) = (0u64, 0u64);
                let t = Instant::now();
                let mut done = 0u64;
                while done < reads {
                    let k = r.next() % num;
                    let mut found = false;
                    reader.read_all(&db_key(k), |_| found = true)?;
                    if found { hit += 1 } else { miss += 1 }
                    done += 1;
                }
                let secs = t.elapsed().as_secs_f64();
                emit(b, reads, secs, &format!(",\"hit\":{},\"miss\":{}", hit, miss));
            }
            "readrandom" | "readmissing" => {
                let reader = Reader::open(&file)?;
                let mut r = Rng(0xDEADBEEF);
                let mut found = 0u64;
                let mut kb = [0u8; 16];
                let t = Instant::now();
                for _ in 0..reads {
                    let k = if b == "readmissing" { num + r.next() % num } else { r.next() % num };
                    db_key_into(k, &mut kb);
                    let mut hit = false;
                    reader.read_all(&kb, |_| hit = true)?;
                    if hit { found += 1 }
                }
                let secs = t.elapsed().as_secs_f64();
                emit(b, reads, secs, &format!(",\"found\":{}", found));
            }
            "readseq" => {
                let reader = Reader::open(&file)?;
                let t = Instant::now();
                let n = reader.scan(None, usize::MAX, |_k, _v| {})?;
                let secs = t.elapsed().as_secs_f64();
                emit(b, n, secs, "");
            }
            "deleterandom" => {
                let store = Store::create(&file, Options::default())?;
                let mut r = Rng(0xFEEDFACE);
                let t = Instant::now();
                for _ in 0..num {
                    store.delete(&db_key(r.next() % num))?;
                }
                store.close()?;
                let secs = t.elapsed().as_secs_f64();
                emit(b, num, secs, "");
            }
            other => eprintln!("# unknown benchmark {other}"),
        }
    }
    Ok(())
}
