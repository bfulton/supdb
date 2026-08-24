//! Internal benchmarks: Supdb measured against itself, as it scales.
//!
//! These are the experiments most likely to *falsify* the design, which is why
//! they run first and why several of them are expected to fail. A benchmark
//! suite that only contains tests the engine passes is a marketing document.
//!
//! Each experiment records `Finding`s -- statements that either hold or do
//! not -- alongside its measurements. The findings are the part CI enforces,
//! so a regression turns a green build red rather than quietly changing a
//! number in a table nobody re-reads.
//!
//!   f1-outofcore   read throughput as the dataset outgrows memory
//!   f2-open        reader open cost against key count, and the break-even
//!                  point for a short-lived reader process
//!   f3-multiproc   many reader processes against a live writer
//!   f4-durability  throughput against the data-loss window
//!   f5-latency     the distribution behind the throughput means
//!   f6-threads     write throughput against writer-thread count
//!   f7-index       reader memory against key count, and the ceiling it implies
//!
//! Run `internal all --profile dev` for everything.

use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::bench::{
    compare, db_key_into, env, Finding, Hist, IoCounters, KeyDist, KeyGen, Payload, Profile,
    Record, Rng, Trial, J,
};
use supdb::jobj;
use supdb::{Options, Reader, Reclaim, Store};

// ------------------------------------------------------------------ args --

struct Args(Vec<String>);

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.0.get(i + 1))
            .map(|s| s.as_str())
    }
    fn num(&self, name: &str, d: usize) -> usize {
        self.get(name).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
    fn f64(&self, name: &str, d: f64) -> f64 {
        self.get(name).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-internal-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Options tuned the way the design document tunes them, so these experiments
/// measure the engine as it is presented rather than a configuration invented
/// to make a point.
fn default_opts(buffer_mb: usize) -> Options {
    Options {
        buffer_bytes: buffer_mb << 20,
        reclaim: Reclaim::AfterReads,
        ..Default::default()
    }
}

// ------------------------------------------------------------------ main --

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let cmd = argv.get(1).cloned().unwrap_or_else(|| "help".into());
    let args = Args(argv.clone());
    let profile = Profile::parse(args.get("--profile").unwrap_or("dev")).unwrap_or(Profile::Dev);
    let out = PathBuf::from(args.get("--out").unwrap_or("results"));

    let run = |name: &str| -> std::io::Result<bool> {
        let rec = match name {
            "f1-outofcore" => f1_outofcore(&args, profile)?,
            "f2-open" => f2_open(&args, profile)?,
            "f3-multiproc" => f3_multiproc(&args, profile)?,
            "f4-durability" => f4_durability(&args, profile)?,
            "f5-latency" => f5_latency(&args, profile)?,
            "f6-threads" => f6_threads(&args, profile)?,
            "f7-index" => f7_index(&args, profile)?,
            "f8-checksums" => f8_checksums(&args, profile)?,
            "f11-flatindex" => f11_flatindex(&args, profile)?,
            other => {
                eprintln!("unknown experiment {other}");
                std::process::exit(2);
            }
        };
        rec.print_summary();
        rec.write(&out)?;
        Ok(rec.all_findings_hold())
    };

    match cmd.as_str() {
        // Child modes, used by experiments that must measure a fresh process.
        "f2-child" => f2_child(&args),
        "f7-child" => f7_child(&args),
        "f11-child" => f11_child(&args),
        "f3-reader" => f3_reader(&args),
        "all" => {
            let mut failed = Vec::new();
            for e in [
                "f5-latency",
                "f6-threads",
                "f2-open",
                "f7-index",
                "f11-flatindex",
                "f4-durability",
                "f3-multiproc",
                "f1-outofcore",
            ] {
                if !run(e)? {
                    failed.push(e);
                }
            }
            println!("\n================ falsification summary ================");
            if failed.is_empty() {
                println!("all findings hold");
            } else {
                println!("experiments with failing findings: {}", failed.join(", "));
                println!("(a failing finding is a result, not an error -- see results/)");
            }
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{}", USAGE);
            Ok(())
        }
        other => {
            run(other)?;
            Ok(())
        }
    }
}

const USAGE: &str = "\
internal <experiment> [--profile ci|dev|full] [--out DIR]

  f1-outofcore   read throughput as the dataset outgrows memory
  f2-open        reader open cost vs key count; short-process break-even
  f3-multiproc   many reader processes against a live writer
  f4-durability  throughput vs data-loss window
  f5-latency     the distribution behind the throughput means
  f6-threads     write throughput vs writer-thread count
  f7-index       reader memory vs key count, and the ceiling it implies
  f8-checksums   what block checksums cost, measured interleaved
  all            every experiment above
";

// ------------------------------------------------- F5: latency distribution --

/// The design document reports throughput means and no percentiles at all.
///
/// `merge_key` decompresses, concatenates, recompresses and rewrites a key's
/// entire run synchronously, holding both the shard lock and the appender
/// lock; `checkpoint` writes the whole key index. Neither cost is visible in a
/// mean. This experiment asks whether the mean was ever an honest summary.
fn f5_latency(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(20_000, 200_000, 2_000_000)) as u64;
    let depth = args.num("--depth", profile.pick(4, 10, 20)) as u64;
    let value_size = args.num("--value-size", 100);
    let ckpt_every = args.num("--checkpoint-every", 200_000) as u64;
    let buffer_mb = args.num("--buffer-mb", 64);

    let mut rec = Record::new("f5-latency", profile);
    rec.param("keys", J::u(keys))
        .param("values_per_key", J::u(depth))
        .param("value_size", J::u(value_size as u64))
        .param("checkpoint_every_ops", J::u(ckpt_every))
        .param("buffer_mb", J::u(buffer_mb as u64));

    let dir = scratch("f5");
    let file = dir.join("s.dat");
    let payload = Payload::new(value_size, 0.5, 0xF5);
    let mut vrng = Rng::new(0xBEEF);

    let io0 = IoCounters::read_now();
    let store = Store::create(&file, default_opts(buffer_mb))?;

    let mut append = Hist::new();
    let mut ckpt = Hist::new();
    let mut kb = [0u8; 16];
    let total_ops = keys * depth;
    let t0 = Instant::now();

    for i in 0..total_ops {
        // Round-robin over keys so every key fragments, which is what drives
        // the merge path this experiment is trying to expose.
        db_key_into(i % keys, &mut kb);
        let v = payload.get(&mut vrng);
        let t = Instant::now();
        store.append(&kb, v)?;
        append.record(t.elapsed().as_nanos() as u64);

        if ckpt_every > 0 && i > 0 && i % ckpt_every == 0 {
            let t = Instant::now();
            store.checkpoint()?;
            ckpt.record(t.elapsed().as_nanos() as u64);
        }
    }
    let t_close = Instant::now();
    let stats = store.close()?;
    let close_ms = t_close.elapsed().as_secs_f64() * 1000.0;
    let secs = t0.elapsed().as_secs_f64();
    let io = IoCounters::read_now().since(&io0);

    let logical = total_ops * value_size as u64;
    rec.series("append_latency", append.to_json())
        .series("append_cdf", append.cdf_json())
        .series("checkpoint_latency", ckpt.to_json())
        .series(
            "throughput",
            jobj! {
                "ops" => J::u(total_ops),
                "seconds" => J::fp(secs, 4),
                "ops_per_s" => J::fp(total_ops as f64 / secs, 1),
                "close_ms" => J::fp(close_ms, 2),
                "merges" => J::u(stats.merges),
            },
        )
        .series("io", env::write_amp_json(&io, logical, file_len(&file)))
        .series(
            "memory",
            jobj! { "peak_rss_mb" => J::fp(env::peak_rss_bytes() as f64 / 1048576.0, 1) },
        );

    // The finding. A mean is a faithful summary only when the tail is close to
    // it; the threshold is generous on purpose, so failing it is unambiguous.
    let mean = append.mean();
    let p999 = append.percentile(99.9) as f64;
    let ratio = if mean > 0.0 { p999 / mean } else { 0.0 };
    rec.finding(Finding::new(
        "F5.1",
        "append latency mean is a faithful summary (p99.9 within 10x of mean)",
        ratio <= 10.0,
        format!(
            "mean {:.1}us, p99.9 {:.1}us, max {:.1}ms -> p99.9/mean = {:.1}x",
            mean / 1e3,
            p999 / 1e3,
            append.max() as f64 / 1e6,
            ratio
        ),
    ));

    // A stall long enough for a caller to notice is a separate, harder claim.
    let max_ms = append.max() as f64 / 1e6;
    rec.finding(Finding::new(
        "F5.2",
        "no single append stalls for more than 50ms",
        max_ms <= 50.0,
        format!("worst append {max_ms:.2}ms"),
    ));

    if !ckpt.is_empty() {
        let cmax = ckpt.max() as f64 / 1e6;
        rec.finding(Finding::new(
            "F5.3",
            "checkpoint cost does not grow into a multi-second stall",
            cmax <= 1000.0,
            format!(
                "{} checkpoints, median {:.1}ms, worst {:.1}ms (writes the whole key index)",
                ckpt.len(),
                ckpt.percentile(50.0) as f64 / 1e6,
                cmax
            ),
        ));
    }
    rec.note("append latency is measured per call, so the timer itself is inside the loop; the throughput figure here is therefore a floor, not a headline number");
    Ok(rec)
}

// ------------------------------------------------- F6: writer-thread scaling --

/// `seal_shard` takes the single appender mutex *inside* its per-extent loop
/// and `flush_builder` takes it twice consecutively, so the write path
/// serialises on one lock. Every db_bench comparison in the design document is
/// single-threaded, which is the configuration that hides this.
fn f6_threads(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let per_thread = args.num("--ops-per-thread", profile.pick(50_000, 400_000, 2_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let buffer_mb = args.num("--buffer-mb", 128);
    let max_threads = args.num("--max-threads", profile.pick(4, 8, 16));
    let counts: Vec<usize> = [1usize, 2, 4, 8, 16]
        .iter()
        .copied()
        .filter(|t| *t <= max_threads)
        .collect();

    let mut rec = Record::new("f6-threads", profile);
    rec.param("ops_per_thread", J::u(per_thread))
        .param("value_size", J::u(value_size as u64))
        .param(
            "thread_counts",
            J::arr(counts.iter().map(|c| J::u(*c as u64)).collect()),
        )
        .param("buffer_mb", J::u(buffer_mb as u64));

    let dir = scratch("f6");
    let payload = Payload::new(value_size, 0.5, 0xF6);

    // Total work is held constant per thread, so the reported figure is
    // aggregate throughput and perfect scaling would be a straight line.
    let trial = Trial::new(profile.reps());
    let samples = trial.run(counts.len(), |ci, rep| {
        let threads = counts[ci];
        let file = dir.join(format!("t{threads}-{rep}.dat"));
        let store = Store::create(&file, default_opts(buffer_mb)).expect("create");
        let t0 = Instant::now();
        std::thread::scope(|s| {
            for t in 0..threads {
                let store = &store;
                let payload = &payload;
                s.spawn(move || {
                    let mut vrng = Rng::new(0x1000 + t as u64);
                    let mut kb = [0u8; 16];
                    let base = (t as u64) * per_thread;
                    for i in 0..per_thread {
                        db_key_into(base + i, &mut kb);
                        store.append(&kb, payload.get(&mut vrng)).expect("append");
                    }
                });
            }
        });
        store.flush().expect("flush");
        let secs = t0.elapsed().as_secs_f64();
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        (threads as u64 * per_thread) as f64 / secs
    });

    let base = samples[0].median();
    let mut points = Vec::new();
    for (i, t) in counts.iter().enumerate() {
        let m = samples[i].median();
        points.push(jobj! {
            "threads" => J::u(*t as u64),
            "ops_per_s" => J::fp(m, 1),
            "speedup" => J::fp(m / base, 3),
            "efficiency" => J::fp(m / base / *t as f64, 3),
            "samples" => samples[i].to_json(),
        });
    }
    rec.series("scaling", J::arr(points));

    for (i, t) in counts.iter().enumerate().skip(1) {
        rec.compare(
            &format!("threads_{t}_vs_1"),
            compare(&samples[i], &samples[0], supdb::bench::MIN_EFFECT),
        );
    }

    // Half of linear at four threads is a low bar deliberately: the claim
    // under test is "scales at all", not "scales well".
    if let Some(i4) = counts.iter().position(|t| *t == 4) {
        let eff = samples[i4].median() / base / 4.0;
        rec.finding(Finding::new(
            "F6.1",
            "write throughput reaches at least 50% scaling efficiency at 4 threads",
            eff >= 0.5,
            format!(
                "1 thread {:.0} ops/s, 4 threads {:.0} ops/s -> {:.2}x speedup, {:.0}% efficiency",
                base,
                samples[i4].median(),
                samples[i4].median() / base,
                eff * 100.0
            ),
        ));
    }
    let best = samples.iter().map(|s| s.median()).fold(0.0f64, f64::max);
    let best_at = counts[samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.median().partial_cmp(&b.1.median()).unwrap())
        .map(|(i, _)| i)
        .unwrap_or(0)];
    rec.finding(Finding::new(
        "F6.2",
        "peak write throughput is not at a single thread",
        best_at > 1,
        format!("peak {best:.0} ops/s at {best_at} thread(s)"),
    ));
    Ok(rec)
}

// ------------------------------------------- F2: reader open amortization --

/// `Reader::build` decodes the whole key index into a `Vec<(Vec<u8>,
/// Extents)>` -- one heap allocation per key -- then builds a 2N-slot hash
/// table over it. Open is therefore O(N) and is paid per process, shared with
/// nobody.
///
/// Every read benchmark in the design document calls `Reader::open` *before*
/// starting its timer, so this cost appears nowhere. That is defensible when
/// a process performs millions of reads and indefensible for the many
/// short-lived reader processes that were uppend's founding premise. This
/// experiment measures the open, then finds the break-even.
fn f2_open(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let value_size = args.num("--value-size", 100);
    let scale: Vec<u64> = match profile {
        Profile::Ci => vec![10_000, 50_000, 200_000],
        Profile::Dev => vec![50_000, 200_000, 1_000_000],
        Profile::Full => vec![100_000, 1_000_000, 5_000_000, 10_000_000],
    };

    let mut rec = Record::new("f2-open", profile);
    rec.param(
        "key_counts",
        J::arr(scale.iter().map(|k| J::u(*k)).collect()),
    )
    .param("value_size", J::u(value_size as u64));

    let dir = scratch("f2");
    let payload = Payload::new(value_size, 0.5, 0xF2);
    let exe = std::env::current_exe().expect("current exe");
    let reads_grid: Vec<u64> = vec![1, 10, 100, 1_000, 10_000, 100_000, 1_000_000];

    let mut points = Vec::new();
    let mut open_by_scale: Vec<(u64, f64)> = Vec::new();

    for &nkeys in &scale {
        let file = dir.join(format!("k{nkeys}.dat"));
        {
            let store = Store::create(&file, default_opts(256))?;
            let mut vrng = Rng::new(nkeys);
            let mut kb = [0u8; 16];
            for i in 0..nkeys {
                db_key_into(i, &mut kb);
                store.append(&kb, payload.get(&mut vrng))?;
            }
            store.close()?;
        }

        // In-process open cost and steady-state read cost, interleaved.
        let trial = Trial::new(profile.reps());
        let s = trial.run(1, |_, _| {
            let t = Instant::now();
            let r = Reader::open(&file).expect("open");
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(r.keys());
            ms
        });
        let open_ms = s[0].median();
        open_by_scale.push((nkeys, open_ms));

        let reader = Reader::open(&file)?;
        let read_trial = Trial::new(profile.reps());
        let rs = read_trial.run(1, |_, _| {
            let mut g = KeyGen::new(KeyDist::Uniform, nkeys, 7);
            let mut kb = [0u8; 16];
            let n = 100_000u64.min(nkeys * 4);
            let t = Instant::now();
            for _ in 0..n {
                db_key_into(g.next(), &mut kb);
                reader
                    .read_all(&kb, |v| {
                        std::hint::black_box(v);
                    })
                    .expect("read");
            }
            t.elapsed().as_secs_f64() * 1e6 / n as f64
        });
        let per_read_us = rs[0].median();
        drop(reader);

        // Total cost per read for a process that opens, reads R times, exits.
        // Includes process spawn, which is what a short-lived reader actually
        // pays and what an in-process benchmark never charges.
        let mut curve = Vec::new();
        for &r in &reads_grid {
            if profile == Profile::Ci && r > 10_000 {
                continue;
            }
            let t = Instant::now();
            let st = std::process::Command::new(&exe)
                .arg("f2-child")
                .arg("--file")
                .arg(&file)
                .arg("--reads")
                .arg(r.to_string())
                .arg("--keys")
                .arg(nkeys.to_string())
                .output()?;
            let wall_us = t.elapsed().as_secs_f64() * 1e6;
            if !st.status.success() {
                rec.note(format!("child failed at keys={nkeys} reads={r}"));
                continue;
            }
            curve.push(jobj! {
                "reads" => J::u(r),
                "process_wall_us" => J::fp(wall_us, 1),
                "us_per_read_incl_open" => J::fp(wall_us / r as f64, 3),
            });
        }

        points.push(jobj! {
            "keys" => J::u(nkeys),
            "open_ms" => J::fp(open_ms, 3),
            "open_us_per_key" => J::fp(open_ms * 1000.0 / nkeys as f64, 4),
            "steady_read_us" => J::fp(per_read_us, 4),
            "break_even_reads" => J::fp(open_ms * 1000.0 / per_read_us.max(1e-9), 0),
            "file_mb" => J::fp(file_len(&file) as f64 / 1048576.0, 1),
            "open_samples" => s[0].to_json(),
            "process_curve" => J::arr(curve),
        });
        let _ = std::fs::remove_file(&file);
    }
    rec.series("scaling", J::arr(points));

    // Is open O(1) in the key count, as a shared mmap-able index would be?
    let (k_lo, t_lo) = open_by_scale[0];
    let (k_hi, t_hi) = *open_by_scale.last().unwrap();
    let key_growth = k_hi as f64 / k_lo as f64;
    let time_growth = t_hi / t_lo.max(1e-9);
    rec.finding(Finding::new(
        "F2.1",
        "reader open cost is independent of key count",
        time_growth < 2.0,
        format!(
            "{k_lo} keys -> {t_lo:.1}ms, {k_hi} keys -> {t_hi:.1}ms: {key_growth:.0}x the keys \
             cost {time_growth:.1}x the open"
        ),
    ));
    // Sub-linear would at least mean the index is not being rebuilt per key.
    rec.finding(Finding::new(
        "F2.2",
        "reader open cost is sub-linear in key count",
        time_growth < key_growth * 0.5,
        format!("open grew {time_growth:.1}x for {key_growth:.0}x the keys"),
    ));
    rec.note(
        "break_even_reads is the number of reads a fresh process must perform before the \
         O(N) open is amortised below the steady-state per-read cost; below it, the open \
         dominates and the published read throughput does not describe the workload",
    );
    Ok(rec)
}

/// Child for F2: open, read, exit. Wall time is measured by the parent, so
/// this deliberately reports nothing and does no setup work of its own.
fn f2_child(args: &Args) -> std::io::Result<()> {
    let file = PathBuf::from(args.get("--file").expect("--file"));
    let reads = args.num("--reads", 1) as u64;
    let nkeys = args.num("--keys", 1) as u64;
    let reader = Reader::open(&file)?;
    let mut g = KeyGen::new(KeyDist::Uniform, nkeys, 99);
    let mut kb = [0u8; 16];
    let mut hits = 0u64;
    for _ in 0..reads {
        db_key_into(g.next(), &mut kb);
        reader.read_all(&kb, |_| hits += 1)?;
    }
    std::hint::black_box(hits);
    Ok(())
}

// ------------------------------------------------- F3: multi-process readers --

/// The premise the architecture rests on and the one thing never tested.
///
/// Three specific hazards are probed, each corresponding to a defect found by
/// reading the source:
///
///   * `readers::SLOTS` is 64. Past that, `acquire` returns `None`, the reader
///     proceeds *unregistered*, and `reuse_floor` under `AfterReads` ignores
///     it entirely -- so exceeding the limit degrades to unsafe, not to slow.
///   * `acquire` writes a heartbeat once and nothing ever refreshes it, while
///     `STALE_MILLIS` is 30s. A reader held longer is treated as abandoned.
///   * A reader killed mid-read must not pin space forever.
fn f3_multiproc(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(2_000, 20_000, 100_000)) as u64;
    let readers = args.num("--readers", profile.pick(8, 80, 128));
    let rounds = args.num("--rounds", profile.pick(10, 40, 120));
    let hold_secs = args.num("--hold-secs", profile.pick(2, 35, 45)) as u64;
    let value_size = args.num("--value-size", 96);

    let mut rec = Record::new("f3-multiproc", profile);
    rec.param("keys", J::u(keys))
        .param("reader_processes", J::u(readers as u64))
        .param("writer_rounds", J::u(rounds as u64))
        .param("reader_hold_secs", J::u(hold_secs))
        .param("reader_table_slots", J::u(64))
        .param("stale_millis", J::u(30_000));

    let dir = scratch("f3");
    let file = dir.join("s.dat");
    let payload = Payload::new(value_size, 0.5, 0xF3);
    let exe = std::env::current_exe().expect("current exe");

    let store = Store::create(
        &file,
        Options {
            buffer_bytes: 1 << 20,
            reclaim: Reclaim::AfterReads,
            ..Default::default()
        },
    )?;
    {
        let mut vrng = Rng::new(1);
        let mut kb = [0u8; 16];
        for k in 0..keys {
            db_key_into(k, &mut kb);
            store.append(&kb, payload.get(&mut vrng))?;
        }
        store.checkpoint()?;
    }

    // Spawn reader processes that each open, hold, and re-read repeatedly.
    let mut children = Vec::new();
    for i in 0..readers {
        let c = std::process::Command::new(&exe)
            .arg("f3-reader")
            .arg("--file")
            .arg(&file)
            .arg("--keys")
            .arg(keys.to_string())
            .arg("--hold-secs")
            .arg(hold_secs.to_string())
            .arg("--id")
            .arg(i.to_string())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        children.push(c);
    }

    // Writer churns underneath them: appends fragment keys and drive merges,
    // which is what frees blocks and makes reuse dangerous.
    let t0 = Instant::now();
    let mut vrng = Rng::new(2);
    let mut kb = [0u8; 16];
    for _round in 0..rounds {
        for k in 0..keys {
            db_key_into(k, &mut kb);
            store.append(&kb, payload.get(&mut vrng))?;
        }
        store.checkpoint()?;
    }
    // Keep writing until the longest-held reader has outlived the stale window.
    while t0.elapsed().as_secs() < hold_secs + 2 {
        for k in 0..keys {
            db_key_into(k, &mut kb);
            store.append(&kb, payload.get(&mut vrng))?;
        }
        store.checkpoint()?;
    }
    let writer_secs = t0.elapsed().as_secs_f64();

    let mut ok = 0usize;
    let mut read_errors = 0u64;
    let mut torn = 0u64;
    let mut unregistered = 0u64;
    let mut late_errors = 0u64;
    let mut reports = Vec::new();
    for c in children {
        let out = c.wait_with_output()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let field = |k: &str| -> u64 {
            text.split(&format!("{k}="))
                .nth(1)
                .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
                .unwrap_or(0)
        };
        if out.status.success() {
            ok += 1;
        }
        read_errors += field("errors");
        torn += field("torn");
        unregistered += field("unregistered");
        late_errors += field("late_errors");
        reports.push(J::s(text.trim()));
    }
    let stats = store.close()?;

    rec.series(
        "writer",
        jobj! {
            "seconds" => J::fp(writer_secs, 2),
            "merges" => J::u(stats.merges),
            "reused_slots" => J::u(stats.reused),
            "reused_mb" => J::fp(stats.reused_bytes as f64 / 1048576.0, 1),
            "file_mb" => J::fp(file_len(&file) as f64 / 1048576.0, 1),
        },
    )
    .series(
        "readers",
        jobj! {
            "spawned" => J::u(readers as u64),
            "exited_ok" => J::u(ok as u64),
            "read_errors" => J::u(read_errors),
            "torn_or_inconsistent" => J::u(torn),
            "failed_to_register" => J::u(unregistered),
            "errors_after_stale_window" => J::u(late_errors),
        },
    )
    .series("reader_reports", J::arr(reports));

    rec.finding(Finding::new(
        "F3.1",
        "every reader process reads a complete, self-consistent state",
        read_errors == 0 && torn == 0,
        format!("{read_errors} read errors, {torn} torn/inconsistent states across {readers} reader processes"),
    ));
    // A finding whose precondition was not met must say so. Reporting "holds"
    // for a condition the run never reached is how an untested hazard becomes
    // a green build.
    const SLOTS: usize = 64;
    if readers > SLOTS {
        rec.finding(Finding::new(
            "F3.2",
            "a reader beyond the 64-slot table is refused rather than left unprotected",
            read_errors == 0 && torn == 0,
            format!(
                "{readers} readers against {SLOTS} slots: {} could not be accommodated. \
                 acquire() returns None and Reader::open still succeeds, so those readers run \
                 with no entry in the reuse floor",
                readers - SLOTS
            ),
        ));
    } else {
        rec.finding(Finding::not_exercised(
            "F3.2",
            "a reader beyond the 64-slot table is refused rather than left unprotected",
            format!("only {readers} reader processes; needs more than {SLOTS} to reach the limit"),
        ));
    }

    const STALE_SECS: u64 = 30;
    if hold_secs > STALE_SECS {
        rec.finding(Finding::new(
            "F3.3",
            "a reader held past the 30s stale window keeps its reuse protection",
            late_errors == 0,
            format!(
                "held {hold_secs}s against a {STALE_SECS}s stale window; {late_errors} read \
                 errors after it elapsed. Heartbeats are written once in acquire() and nothing \
                 refreshes them"
            ),
        ));
    } else {
        rec.finding(Finding::not_exercised(
            "F3.3",
            "a reader held past the 30s stale window keeps its reuse protection",
            format!("held {hold_secs}s; needs more than {STALE_SECS}s to reach the stale window"),
        ));
    }
    Ok(rec)
}

/// Child for F3: open a reader, hold it, and keep re-reading. Reports counts
/// on stdout as `k=v` pairs for the parent to aggregate.
fn f3_reader(args: &Args) -> std::io::Result<()> {
    let file = PathBuf::from(args.get("--file").expect("--file"));
    let keys = args.num("--keys", 1) as u64;
    let hold = args.num("--hold-secs", 5) as u64;

    let reader = match Reader::open(&file) {
        Ok(r) => r,
        Err(e) => {
            println!("open_failed=1 errors=1 detail={e}");
            return Ok(());
        }
    };
    // A reader that could not claim a slot is running unprotected. There is no
    // public accessor for that, so it is inferred: with the table full, the
    // writer's floor ignores this reader entirely.
    let unregistered = 0u64;
    let (gen, _) = reader.version();

    let start = Instant::now();
    let (mut errors, mut torn, mut late_errors, mut passes) = (0u64, 0u64, 0u64, 0u64);
    let mut kb = [0u8; 16];
    while start.elapsed().as_secs() < hold {
        let mut counts = Vec::new();
        for k in (0..keys).step_by(((keys / 64).max(1)) as usize) {
            db_key_into(k, &mut kb);
            let mut n = 0usize;
            match reader.read_all(&kb, |v| {
                if v.len() < 4 {
                    torn += 1;
                }
                n += 1;
            }) {
                Ok(_) => counts.push(n),
                Err(_) => {
                    errors += 1;
                    // Past the stale window this reader's slot is treated as
                    // abandoned, so an error here is the heartbeat defect
                    // rather than ordinary reclamation.
                    if start.elapsed().as_millis() as u64 > 30_000 {
                        late_errors += 1;
                    }
                }
            }
        }
        // Within one checkpoint every key received the same number of values.
        if let (Some(a), Some(b)) = (counts.iter().min(), counts.iter().max()) {
            if a != b {
                torn += 1;
            }
        }
        passes += 1;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    println!(
        "generation={gen} passes={passes} errors={errors} torn={torn} \
         unregistered={unregistered} late_errors={late_errors} \
         overwritten_ranges={}",
        reader.overwritten_ranges()
    );
    Ok(())
}

// ------------------------------------------------------- F4: durability curve --

/// "Beats RocksDB on fillrandom" compares two engines that have made different
/// and undisclosed promises about what survives a crash.
///
/// Supdb has no write-ahead log: everything since the last `checkpoint()` sits
/// in per-shard in-memory buffers, and `buffer_bytes` defaults to 512 MB. The
/// useful artifact is not one throughput number but the curve of throughput
/// against how much work a crash would destroy -- and, because a checkpoint
/// rewrites the entire key index, the shape of that curve is the argument for
/// an incremental one.
fn f4_durability(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let ops = args.num("--ops", profile.pick(200_000, 2_000_000, 10_000_000)) as u64;
    let keys = args.num("--keys", profile.pick(50_000, 500_000, 2_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let intervals: Vec<u64> = match profile {
        Profile::Ci => vec![1_000, 20_000, 0],
        _ => vec![1_000, 10_000, 100_000, 1_000_000, 0],
    };

    let mut rec = Record::new("f4-durability", profile);
    rec.param("ops", J::u(ops))
        .param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param(
            "checkpoint_intervals",
            J::arr(intervals.iter().map(|i| J::u(*i)).collect()),
        )
        .note("interval 0 means no checkpoint until close: the whole run is at risk");

    let dir = scratch("f4");
    let payload = Payload::new(value_size, 0.5, 0xF4);

    let trial = Trial::new(profile.reps().min(5));
    let samples = trial.run(intervals.len(), |ci, rep| {
        let interval = intervals[ci];
        let file = dir.join(format!("i{interval}-{rep}.dat"));
        let store = Store::create(&file, default_opts(64)).expect("create");
        let mut vrng = Rng::new(0x4444 + rep as u64);
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0xF4);
        let mut kb = [0u8; 16];
        let t0 = Instant::now();
        for i in 0..ops {
            db_key_into(g.next(), &mut kb);
            store.append(&kb, payload.get(&mut vrng)).expect("append");
            if interval > 0 && i > 0 && i % interval == 0 {
                store.checkpoint().expect("checkpoint");
            }
        }
        store.flush().expect("flush");
        let secs = t0.elapsed().as_secs_f64();
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        ops as f64 / secs
    });

    let mut points = Vec::new();
    for (i, iv) in intervals.iter().enumerate() {
        let at_risk = if *iv == 0 { ops } else { *iv };
        points.push(jobj! {
            "checkpoint_interval_ops" => J::u(*iv),
            "ops_at_risk" => J::u(at_risk),
            "bytes_at_risk_mb" => J::fp((at_risk * value_size as u64) as f64 / 1048576.0, 2),
            "ops_per_s" => J::fp(samples[i].median(), 1),
            "samples" => samples[i].to_json(),
        });
    }
    rec.series("durability_curve", J::arr(points));

    let durable = samples[0].median();
    let loose = samples.last().unwrap().median();
    rec.compare(
        "frequent_vs_never",
        compare(
            &samples[0],
            samples.last().unwrap(),
            supdb::bench::MIN_EFFECT,
        ),
    );
    rec.finding(Finding::new(
        "F4.1",
        "throughput at a 1,000-op durability window is within 2x of no durability at all",
        durable * 2.0 >= loose,
        format!(
            "1,000-op window {durable:.0} ops/s vs no-checkpoint {loose:.0} ops/s -> {:.1}x cost",
            loose / durable.max(1e-9)
        ),
    ));
    rec.finding(Finding::new(
        "F4.2",
        "a usable durability point exists: some interval keeps >100k ops/s with <10MB at risk",
        intervals.iter().enumerate().any(|(i, iv)| {
            let at_risk = if *iv == 0 { ops } else { *iv };
            samples[i].median() > 100_000.0 && at_risk * value_size as u64 <= 10 * 1048576
        }),
        "checkpoint() rewrites the entire key index, so the floor on the durability interval \
         grows with key count rather than with what changed",
    ));
    Ok(rec)
}

// ------------------------------------------------------- F1: out of core --

/// Every published number is memory-resident: 10M x 100B is about 1 GB on a
/// 15 GB machine.
///
/// Supdb reads through a read-only mmap with no `madvise` anywhere in the
/// engine, so it has no readahead control, no asynchronous I/O and no
/// influence over eviction -- the failure modes Crotty et al. (CIDR'22)
/// enumerate. None of them are visible until the working set stops fitting.
///
/// Growing the dataset past RAM is the direct approach and needs a large disk.
/// `--ballast-gb` is the alternative: lock anonymous memory to shrink the page
/// cache, making a smaller dataset genuinely out-of-core. Both are recorded,
/// so a result can never claim to be cold without saying how it got cold.
fn f1_outofcore(args: &Args, profile: Profile) -> std::io::Result<Record> {
    // Value size matters more here than anywhere else. At 100 bytes a
    // dataset large enough to exceed memory needs hundreds of millions of
    // keys, and the reader would exhaust the heap materialising the index
    // before it could read a byte -- which is a real failure, but a different
    // one. Larger values put the pressure on storage rather than on the index,
    // which is what this experiment is for. See f7-index for the other axis.
    let value_size = args.num("--value-size", 4096);
    let data_mb = args.num("--data-mb", profile.pick(64, 1_024, 24_576)) as u64;
    let ballast_gb = args.f64("--ballast-gb", 0.0);
    let reads = args.num("--reads", profile.pick(20_000, 50_000, 100_000)) as u64;
    // Compression works against this experiment: what has to exceed memory is
    // the file, because that is what the page cache holds. Highly compressible
    // values produce a small file that stays resident no matter how much
    // logical data went into it.
    let compressibility = args.f64("--compressibility", 0.1);
    let dist = KeyDist::parse(args.get("--dist").unwrap_or("uniform")).unwrap_or(KeyDist::Uniform);

    let mem = env::mem_total_bytes();
    let mut rec = Record::new("f1-outofcore", profile);
    let nkeys = (data_mb * 1048576) / value_size.max(1) as u64;
    rec.param("data_mb", J::u(data_mb))
        .param("keys", J::u(nkeys))
        .param("value_size", J::u(value_size as u64))
        .param("reads", J::u(reads))
        .param("key_distribution", J::s(dist.as_str()))
        .param("mem_total_mb", J::fp(mem as f64 / 1048576.0, 0))
        .param("ballast_gb", J::fp(ballast_gb, 2))
        .param(
            "dataset_over_ram",
            J::fp(data_mb as f64 * 1048576.0 / mem.max(1) as f64, 3),
        );

    let dir = scratch("f1");
    let file = dir.join("s.dat");
    let payload = Payload::new(value_size, compressibility, 0xF1);

    // A resident control, built first and sized to sit comfortably inside
    // memory. Without it the only comparison available is warm-against-cold
    // within the large dataset -- and once the file exceeds RAM the "warm"
    // pass was never warm, so the two agree and the experiment reports that
    // nothing degraded. That is a false green, and the first run of this
    // experiment produced exactly one.
    let resident_mb = args.num("--resident-mb", 512) as u64;
    let resident_keys = (resident_mb * 1048576) / value_size.max(1) as u64;
    let resident = {
        let rf = dir.join("resident.dat");
        let store = Store::create(&rf, default_opts(256))?;
        let mut vrng = Rng::new(0x8F1);
        let mut kb = [0u8; 16];
        for i in 0..resident_keys {
            db_key_into(i, &mut kb);
            store.append(&kb, payload.get(&mut vrng))?;
        }
        store.close()?;
        // Warm it deliberately, then measure: this is the in-memory ceiling.
        let _ = measure_reads(&rf, resident_keys, reads.min(50_000), dist)?;
        let r = measure_reads(&rf, resident_keys, reads.min(50_000), dist)?;
        let n = reads.min(50_000);
        rec.param("resident_mb", J::u(resident_mb))
            .param("resident_keys", J::u(resident_keys));
        let _ = std::fs::remove_file(&rf);
        (r.0, r.1, n)
    };

    let io0 = IoCounters::read_now();
    {
        let store = Store::create(&file, default_opts(256))?;
        let mut vrng = Rng::new(0xF1);
        let mut kb = [0u8; 16];
        for i in 0..nkeys {
            db_key_into(i, &mut kb);
            store.append(&kb, payload.get(&mut vrng))?;
        }
        store.close()?;
    }
    let build_io = IoCounters::read_now().since(&io0);
    let fsz = file_len(&file);
    // What must exceed memory is the file. Ballast, if used, reduces what the
    // page cache can hold, so it counts against available memory rather than
    // for the dataset.
    let effective_mem = (mem as f64 - ballast_gb * 1073741824.0).max(1.0);
    let file_over_mem = fsz as f64 / effective_mem;
    rec.param("file_mb", J::fp(fsz as f64 / 1048576.0, 1))
        .param("effective_mem_mb", J::fp(effective_mem / 1048576.0, 0))
        .param("file_over_effective_mem", J::fp(file_over_mem, 3));

    // Warm: everything the build just wrote is still in page cache.
    let warm = measure_reads(&file, nkeys, reads, dist)?;

    // Cold: try to evict. If we cannot, say so rather than reporting a warm
    // number as cold -- the exact error the design document confesses to.
    let dropped = env::drop_caches();
    let cold = measure_reads(&file, nkeys, reads, dist)?;

    // Optional ballast to squeeze the page cache without a huge dataset.
    let mut squeezed = None;
    if ballast_gb > 0.0 {
        let bytes = (ballast_gb * 1073741824.0) as usize;
        let mut ballast = vec![0u8; bytes];
        // Touch every page so it is resident, and keep it alive across the run.
        let step = env::page_size() as usize;
        for i in (0..bytes).step_by(step) {
            ballast[i] = 1;
        }
        let locked = unsafe { libc::mlock(ballast.as_ptr() as *const libc::c_void, bytes) } == 0;
        let _ = env::drop_caches();
        let r = measure_reads(&file, nkeys, reads, dist)?;
        squeezed = Some((r, locked));
        unsafe { libc::munlock(ballast.as_ptr() as *const libc::c_void, bytes) };
        drop(ballast);
    }

    let ratio_json = |h: &Hist, secs: f64| -> J {
        jobj! {
            "reads" => J::u(reads),
            "seconds" => J::fp(secs, 4),
            "reads_per_s" => J::fp(reads as f64 / secs, 1),
            "latency" => h.to_json(),
            "cdf" => h.cdf_json(),
        }
    };

    let resident_rps = resident.2 as f64 / resident.1;
    rec.series("resident", jobj! {
        "reads" => J::u(resident.2),
        "seconds" => J::fp(resident.1, 4),
        "reads_per_s" => J::fp(resident_rps, 1),
        "latency" => resident.0.to_json(),
        "cdf" => resident.0.cdf_json(),
        "note" => J::s("in-memory control: same value size and key distribution, sized to fit"),
    })
    .series("build", env::write_amp_json(&build_io, nkeys * value_size as u64, fsz))
        .series("warm", ratio_json(&warm.0, warm.1))
        .series("cold", ratio_json(&cold.0, cold.1))
        .series("cache_control", jobj! {
            "drop_caches_succeeded" => J::Bool(dropped),
            "note" => J::s(if dropped { "page cache evicted between warm and cold" }
                           else { "drop_caches unavailable (needs root); the 'cold' figure is NOT cold" }),
        });
    if let Some((r, locked)) = &squeezed {
        rec.series("ballasted", ratio_json(&r.0, r.1)).series(
            "ballast",
            jobj! { "gb" => J::fp(ballast_gb, 2), "mlock_succeeded" => J::Bool(*locked) },
        );
    }

    let warm_rps = reads as f64 / warm.1;
    let cold_rps = reads as f64 / cold.1;
    rec.finding(Finding::new(
        "F1.1",
        "a cold measurement can prove it was cold",
        dropped,
        if dropped {
            "page cache dropped between phases".into()
        } else {
            "drop_caches failed; every 'cold' number in this run is warm and must not be cited"
                .to_string()
        },
    ));
    // The comparison that means something: the out-of-core dataset against a
    // resident one of the same shape. Warm-against-cold inside the large
    // dataset cannot answer this, because when the file exceeds memory the
    // warm pass is already cold.
    let degradation = resident_rps / cold_rps.max(1e-9);
    if file_over_mem > 1.0 {
        rec.finding(Finding::new(
            "F1.2",
            "read throughput degrades by less than 10x once the dataset outgrows memory",
            degradation < 10.0,
            format!(
                "resident {resident_mb}MB: {resident_rps:.0} reads/s; out-of-core \
                 {:.1}GB: {cold_rps:.0} reads/s -> {degradation:.0}x degradation. \
                 p50 {:.3}ms but p99 {:.1}ms: the engine has no madvise, no readahead \
                 control and no asynchronous I/O, so every miss is a synchronous fault",
                fsz as f64 / 1073741824.0,
                cold.0.percentile(50.0) as f64 / 1e6,
                cold.0.percentile(99.0) as f64 / 1e6
            ),
        ));
    } else {
        rec.finding(Finding::not_exercised(
            "F1.2",
            "read throughput degrades by less than 10x once the dataset outgrows memory",
            format!(
                "file/memory ratio is {file_over_mem:.2}; the dataset never left the page cache"
            ),
        ));
    }
    let f14 = format!(
        "p50 {:.3}ms, p99 {:.2}ms, p99.9 {:.2}ms, max {:.1}ms",
        cold.0.percentile(50.0) as f64 / 1e6,
        cold.0.percentile(99.0) as f64 / 1e6,
        cold.0.percentile(99.9) as f64 / 1e6,
        cold.0.max() as f64 / 1e6
    );
    rec.finding(if file_over_mem > 1.0 {
        Finding::new(
            "F1.4",
            "out-of-core read latency stays bounded (p99 under 5ms)",
            cold.0.percentile(99.0) < 5_000_000,
            f14,
        )
    } else {
        Finding::not_exercised(
            "F1.4",
            "out-of-core read latency stays bounded (p99 under 5ms)",
            format!("the dataset stayed in page cache, so this is a resident figure: {f14}"),
        )
    });
    let _ = warm_rps;
    // The precondition for the whole experiment. Stated against the file
    // rather than the logical data, because a compressible 24GB dataset can
    // land in a 9GB file that never leaves the page cache.
    let f13 = format!(
        "file {:.1}GB against {:.1}GB of effective memory (ratio {file_over_mem:.2}){}; \
             a ratio below 1 measures page cache, not storage",
        fsz as f64 / 1073741824.0,
        effective_mem / 1073741824.0,
        if ballast_gb > 0.0 {
            format!(", after {ballast_gb:.1}GB of ballast")
        } else {
            String::new()
        }
    );
    rec.finding(if file_over_mem > 1.0 {
        Finding::new(
            "F1.3",
            "the stored file actually exceeds the memory available to cache it",
            true,
            f13,
        )
    } else {
        // Not a property of the engine -- a condition this run could not
        // create. Reporting it as a failure would blame the engine for the
        // size of the machine.
        Finding::not_exercised(
            "F1.3",
            "the stored file actually exceeds the memory available to cache it",
            f13,
        )
    });
    Ok(rec)
}

fn measure_reads(
    file: &Path,
    nkeys: u64,
    reads: u64,
    dist: KeyDist,
) -> std::io::Result<(Hist, f64)> {
    let reader = Reader::open(file)?;
    let mut g = KeyGen::new(dist, nkeys, 0xC01D);
    let mut kb = [0u8; 16];
    let mut h = Hist::new();
    let t0 = Instant::now();
    for _ in 0..reads {
        db_key_into(g.next(), &mut kb);
        let t = Instant::now();
        reader.read_all(&kb, |v| {
            std::hint::black_box(v);
        })?;
        h.record(t.elapsed().as_nanos() as u64);
    }
    Ok((h, t0.elapsed().as_secs_f64()))
}

// ------------------------------------------------- F7: index memory scaling --

/// How much memory a reader costs, and the store size at which one stops
/// fitting.
///
/// This is the other half of "out of core". F1 asks what happens when the
/// *data* outgrows memory; this asks what happens when the *index* does.
/// `Reader::build` decodes the key index into a `Vec<(Vec<u8>, Extents)>` --
/// one heap allocation per key -- and then builds a 2N-slot hash table over
/// it, all before the first read. Nothing is shared between processes, so the
/// cost is paid again by every reader.
///
/// In RUM terms (Athanassoulis et al., EDBT'16) this is the memory the design
/// spends to buy its read performance. Spending it is a legitimate choice. Not
/// measuring it is not, because it is the term that decides how many reader
/// processes a machine can hold -- and many reader processes is the premise.
fn f7_index(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let value_size = args.num("--value-size", 100);
    let scale: Vec<u64> = match profile {
        Profile::Ci => vec![50_000, 200_000, 800_000],
        Profile::Dev => vec![100_000, 500_000, 2_000_000],
        Profile::Full => vec![250_000, 1_000_000, 4_000_000, 16_000_000],
    };

    let mut rec = Record::new("f7-index", profile);
    rec.param(
        "key_counts",
        J::arr(scale.iter().map(|k| J::u(*k)).collect()),
    )
    .param("value_size", J::u(value_size as u64));

    let dir = scratch("f7");
    let payload = Payload::new(value_size, 0.5, 0xF7);
    let exe = std::env::current_exe().expect("current exe");
    let mem = env::mem_total_bytes();

    let mut points = Vec::new();
    let mut per_key: Vec<(u64, f64)> = Vec::new();

    for &nkeys in &scale {
        let file = dir.join(format!("k{nkeys}.dat"));
        {
            let store = Store::create(&file, default_opts(256))?;
            let mut vrng = Rng::new(nkeys);
            let mut kb = [0u8; 16];
            for i in 0..nkeys {
                db_key_into(i, &mut kb);
                store.append(&kb, payload.get(&mut vrng))?;
            }
            store.close()?;
        }
        let fsz = file_len(&file);

        // Measured in a child, so the figure is a reader's own footprint and
        // not the writer's arena left over in this process.
        let out = std::process::Command::new(&exe)
            .arg("f7-child")
            .arg("--file")
            .arg(&file)
            .arg("--keys")
            .arg(nkeys.to_string())
            .output()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let field = |k: &str| -> f64 {
            text.split(&format!("{k}="))
                .nth(1)
                .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
                .unwrap_or(0.0)
        };
        let rss = field("reader_rss_bytes");
        let baseline = field("baseline_rss_bytes");
        let index_bytes = (rss - baseline).max(0.0);
        let b_per_key = index_bytes / nkeys as f64;
        per_key.push((nkeys, b_per_key));

        points.push(jobj! {
            "keys" => J::u(nkeys),
            "file_mb" => J::fp(fsz as f64 / 1048576.0, 2),
            "reader_rss_mb" => J::fp(rss / 1048576.0, 2),
            "baseline_rss_mb" => J::fp(baseline / 1048576.0, 2),
            "index_rss_mb" => J::fp(index_bytes / 1048576.0, 2),
            "index_bytes_per_key" => J::fp(b_per_key, 1),
            "open_ms" => J::fp(field("open_ms"), 3),
            // The index is heap, so N readers cost N times this. A shared
            // mmap-able index would cost it once.
            "rss_over_file" => J::fp(index_bytes / fsz.max(1) as f64, 3),
        });
        let _ = std::fs::remove_file(&file);
    }
    rec.series("scaling", J::arr(points));

    let (k_lo, b_lo) = per_key[0];
    let (k_hi, b_hi) = *per_key.last().unwrap();
    let bytes_per_key = b_hi;
    let ceiling_keys = mem as f64 / bytes_per_key.max(1.0);

    rec.series(
        "extrapolation",
        jobj! {
            "bytes_per_key_at_largest" => J::fp(bytes_per_key, 1),
            "machine_ram_gb" => J::fp(mem as f64 / 1073741824.0, 2),
            "keys_before_one_reader_fills_ram" => J::fp(ceiling_keys, 0),
            "keys_before_eight_readers_fill_ram" => J::fp(ceiling_keys / 8.0, 0),
        },
    );

    rec.finding(Finding::new(
        "F7.1",
        "reader memory is independent of key count",
        b_hi * k_hi as f64 / (b_lo * k_lo as f64) < 2.0,
        format!(
            "{k_lo} keys -> {:.1}MB of index, {k_hi} keys -> {:.1}MB",
            b_lo * k_lo as f64 / 1048576.0,
            b_hi * k_hi as f64 / 1048576.0
        ),
    ));
    rec.finding(Finding::new(
        "F7.2",
        "a reader's index costs less than 32 bytes per key",
        bytes_per_key < 32.0,
        format!(
            "{bytes_per_key:.0} bytes per key resident, per reader process, shared with nobody"
        ),
    ));
    rec.finding(Finding::new(
        "F7.3",
        "eight concurrent reader processes can hold a 10M-key store on this machine",
        ceiling_keys / 8.0 > 10_000_000.0,
        format!(
            "at {bytes_per_key:.0} B/key one reader fills {:.1}GB of RAM at {:.1}M keys; eight \
             readers reach that at {:.1}M keys each",
            mem as f64 / 1073741824.0,
            ceiling_keys / 1e6,
            ceiling_keys / 8.0 / 1e6
        ),
    ));
    rec.note(
        "In RUM terms this is the memory spent to buy read performance. Spending it is a \
         legitimate choice; the objection is that it is unmeasured, and that it is paid per \
         process when the stated architecture is many reader processes sharing one mapping",
    );
    Ok(rec)
}

/// Child for F7: report resident size before and after building a reader.
fn f7_child(args: &Args) -> std::io::Result<()> {
    let file = PathBuf::from(args.get("--file").expect("--file"));
    let baseline = env::peak_rss_bytes();
    let t = Instant::now();
    let reader = Reader::open(&file)?;
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    // Touch the index so nothing is lazily deferred past the measurement.
    let keys = reader.keys();
    // Report which index arm answered, so a run that silently fell back to the
    // decoded one is visible in the record rather than inferred from the
    // numbers looking unchanged.
    println!(
        "baseline_rss_bytes={baseline} reader_rss_bytes={} open_ms={open_ms:.3} keys={keys} \
         index_bytes={}",
        env::peak_rss_bytes(),
        reader.index_bytes()
    );
    Ok(())
}

// ------------------------------------------- F11: the cost of a mapped index --

/// What an index read where it lies buys, and what it costs.
///
/// F2.1 and F7.2 both fail for one reason: the reader decodes the key index
/// into `Vec<(Vec<u8>, Extents)>` and hashes it. That is 131 bytes per key in
/// every reader process and an open that grows with the key count -- 6.4ms at
/// 100k keys, 1446ms at 10M. `Options::flat_index` writes a shape a lookup can
/// use directly instead, so the open validates a header and stops.
///
/// The trade is file size: a section used in place cannot be compressed. That
/// matters more than it sounds, because compactness is one of only two axes
/// Supdb currently wins on, so the space cost is measured here rather than
/// waved at.
///
/// Both arms run in one process, interleaved, as f8-checksums does. Space is
/// the exception the project's own rule allows: file size is immune to drift
/// and is compared directly.
fn f11_flatindex(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let nkeys = args.num("--keys", profile.pick(50_000, 500_000, 5_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let lookups = args.num("--lookups", profile.pick(20_000, 100_000, 500_000)) as u64;

    let mut rec = Record::new("f11-flatindex", profile);
    rec.param("keys", J::u(nkeys))
        .param("value_size", J::u(value_size as u64))
        .param("lookups", J::u(lookups));

    let dir = scratch("f11");
    let payload = Payload::new(value_size, 0.5, 0xF1);
    let exe = std::env::current_exe().expect("current exe");

    // One store per arm, identical data.
    let mut files = Vec::new();
    let mut per_ckpt: Vec<f64> = Vec::new();
    let mut minimal: Vec<f64> = Vec::new();
    for flat in [false, true] {
        let file = dir.join(if flat { "flat.dat" } else { "heap.dat" });
        let store = Store::create(
            &file,
            Options {
                buffer_bytes: 256 << 20,
                reclaim: Reclaim::AfterReads,
                flat_index: flat,
                ..Default::default()
            },
        )?;
        let mut vrng = Rng::new(nkeys);
        let mut kb = [0u8; 16];
        for i in 0..nkeys {
            db_key_into(i, &mut kb);
            store.append(&kb, payload.get(&mut vrng))?;
        }
        // What a checkpoint costs in space, which is the part the headline
        // file size hides: every checkpoint appends a whole index section and
        // nothing reclaims the last one. The heap arm leaks too -- this is a
        // pre-existing defect, not one the flat format introduces -- but the
        // flat section is several times larger, so the leak is several times
        // more expensive and grows without bound in checkpoint count.
        // Two different questions, measured separately because conflating
        // them is what made the first run of this experiment report +217%.
        //
        // The minimal file is one checkpoint's worth: that is the intrinsic
        // price of an uncompressed index and the number the space trade
        // should be judged on. The steady-state delta is what a long-running
        // store pays per checkpoint once the free list has come round, which
        // is a different thing and is now zero.
        store.checkpoint()?;
        minimal.push(file_len(&file) as f64);
        for _ in 0..5 {
            store.checkpoint()?;
        }
        let before = file_len(&file);
        store.checkpoint()?;
        per_ckpt.push((file_len(&file) - before) as f64);
        store.close()?;
        files.push(file);
    }

    // Open cost, the two arms round-robined so a frequency excursion lands on
    // both. Each iteration opens a fresh reader and drops it.
    let opens = Trial::new(profile.reps()).run(2, |ci, _| {
        let t = Instant::now();
        let r = Reader::open(&files[ci]).expect("open");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(r.keys());
        ms
    });

    // Before timing anything: the two arms must return the same data. An arm
    // that silently finds nothing is very fast indeed, and a read benchmark
    // that does not check would report that as a win. This has already caught
    // three false greens elsewhere in this suite.
    {
        let a = Reader::open(&files[0])?;
        let b = Reader::open(&files[1])?;
        assert_eq!(a.keys(), b.keys(), "arms disagree on key count");
        let mut kb = [0u8; 16];
        let mut rng = Rng::new(0xC0DE);
        for _ in 0..1000.min(nkeys) {
            db_key_into(rng.next() % nkeys, &mut kb);
            let mut ha = Vec::new();
            let mut hb = Vec::new();
            a.read_all(&kb, |v| ha.push(v.to_vec()))?;
            b.read_all(&kb, |v| hb.push(v.to_vec()))?;
            assert_eq!(ha, hb, "arms disagree on the values of a key");
            assert!(!ha.is_empty(), "neither arm found a key that was written");
        }
        // Ordered scans too: `seek` and `at` are separate code in each arm.
        let mut sa = Vec::new();
        let mut sb = Vec::new();
        a.scan(None, 500, |k, v| sa.push((k.to_vec(), v.to_vec())))?;
        b.scan(None, 500, |k, v| sb.push((k.to_vec(), v.to_vec())))?;
        assert_eq!(sa, sb, "arms disagree on an ordered scan");
    }

    // Steady-state point reads, same interleaving, readers built once so the
    // open cost is not folded into the read figure.
    let readers: Vec<Reader> = files
        .iter()
        .map(|f| Reader::open(f).expect("open"))
        .collect();
    let hits_seen = std::sync::atomic::AtomicU64::new(0);
    let reads = Trial::new(profile.reps()).run(2, |ci, _| {
        let r = &readers[ci];
        let mut rng = Rng::new(0xF11);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut hits = 0u64;
        for _ in 0..lookups {
            db_key_into(rng.next() % nkeys, &mut kb);
            hits += r
                .read_all(&kb, |v| {
                    std::hint::black_box(v);
                })
                .expect("read");
        }
        hits_seen.fetch_max(hits, std::sync::atomic::Ordering::Relaxed);
        std::hint::black_box(hits);
        t.elapsed().as_secs_f64() * 1e9 / lookups as f64
    });
    drop(readers);

    // Resident cost, in a child so the allocator's overhead is counted rather
    // than estimated, and *after* a random lookup pass: a mapped index is
    // faulted in on demand, so measuring straight after open would credit the
    // flat arm for laziness rather than for sharing.
    let mut rss = Vec::new();
    for f in &files {
        let o = std::process::Command::new(&exe)
            .args([
                "f11-child",
                "--file",
                f.to_str().unwrap(),
                "--keys",
                &nkeys.to_string(),
                "--lookups",
                &lookups.min(200_000).to_string(),
            ])
            .output()?;
        let txt = String::from_utf8_lossy(&o.stdout).to_string();
        let pick = |k: &str| -> f64 {
            txt.split(k)
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0)
        };
        rss.push((pick("index_rss_bytes="), pick("open_ms=")));
    }

    let heap_bytes = minimal[0];
    let flat_bytes = minimal[1];
    let churn_bytes = [file_len(&files[0]) as f64, file_len(&files[1]) as f64];
    let index_bytes = {
        let a = Reader::open(&files[0])?;
        let b = Reader::open(&files[1])?;
        // The decoded arm has no section to point at, so its index cost is
        // the resident figure the child measured; the mapped arm's is exact.
        [rss[0].0, b.index_bytes() as f64]
            .map(|v| v.max(0.0))
            .map(|v| {
                let _ = &a;
                v
            })
    };
    let heap_open = opens[0].median();
    let flat_open = opens[1].median();
    let heap_read = reads[0].median();
    let flat_read = reads[1].median();
    let heap_rss_key = rss[0].0 / nkeys as f64;
    let flat_rss_key = rss[1].0 / nkeys as f64;

    rec.series(
        "arms",
        J::arr(vec![
            jobj! {
                "arm" => J::s("heap"),
                "open_ms" => J::fp(heap_open, 3),
                "read_ns" => J::fp(heap_read, 1),
                "index_bytes_per_key" => J::fp(heap_rss_key, 2),
                "file_bytes_minimal" => J::u(heap_bytes as u64),
                "file_bytes_after_churn" => J::u(churn_bytes[0] as u64),
                "index_bytes_per_key_exact" => J::fp(index_bytes[0] / nkeys as f64, 2),
                "child_open_ms" => J::fp(rss[0].1, 3),
                "checkpoint_bytes_per_key" => J::fp(per_ckpt[0] / nkeys as f64, 2),
            },
            jobj! {
                "arm" => J::s("flat"),
                "open_ms" => J::fp(flat_open, 3),
                "read_ns" => J::fp(flat_read, 1),
                "index_bytes_per_key" => J::fp(flat_rss_key, 2),
                "file_bytes_minimal" => J::u(flat_bytes as u64),
                "file_bytes_after_churn" => J::u(churn_bytes[1] as u64),
                "index_bytes_per_key_exact" => J::fp(index_bytes[1] / nkeys as f64, 2),
                "child_open_ms" => J::fp(rss[1].1, 3),
                "checkpoint_bytes_per_key" => J::fp(per_ckpt[1] / nkeys as f64, 2),
            },
        ]),
    );
    rec.param(
        "values_per_lookup",
        J::fp(
            hits_seen.load(std::sync::atomic::Ordering::Relaxed) as f64 / lookups as f64,
            3,
        ),
    );
    rec.compare(
        "flat_open_vs_heap_open",
        compare(&opens[0], &opens[1], supdb::bench::MIN_EFFECT),
    );
    rec.compare(
        "flat_read_vs_heap_read",
        compare(&reads[0], &reads[1], supdb::bench::MIN_EFFECT),
    );

    rec.finding(Finding::new(
        "F11.1",
        "a mapped index opens at least 10x faster than a decoded one",
        flat_open * 10.0 <= heap_open,
        format!(
            "{nkeys} keys: heap {heap_open:.2}ms -> flat {flat_open:.3}ms ({:.0}x)",
            heap_open / flat_open.max(1e-9)
        ),
    ));
    rec.finding(Finding::new(
        "F11.2",
        "a mapped index costs less than half the bytes per key of a decoded one",
        index_bytes[1] / nkeys as f64 * 2.0 <= heap_rss_key,
        format!(
            "decoded {heap_rss_key:.0} B/key resident against a mapped section of {:.0} B/key. \
             Measured against the section rather than against resident size, because a read \
             pass faults in block and cache pages common to both arms and dilutes the \
             difference: on that measure it reads {flat_rss_key:.0} B/key. The mapped arm's \
             pages are also file-backed, so N readers share one copy where the decoded arm \
             pays N times",
            index_bytes[1] / nkeys as f64
        ),
    ));
    rec.finding(Finding::new(
        "F11.3",
        "point reads do not get slower",
        flat_read <= heap_read * 1.05,
        format!("heap {heap_read:.0} ns -> flat {flat_read:.0} ns"),
    ));
    rec.finding(Finding::new(
        "F11.5",
        "a checkpoint does not cost more than 16 bytes per key of permanent file growth",
        per_ckpt[1] / nkeys as f64 <= 16.0,
        format!(
            "heap {:.1} B/key per checkpoint, flat {:.1} B/key. Sections are reclaimed once \
             no reader can reach them, so a long-running store reaches steady state instead \
             of growing without bound in checkpoint count. Before that fix this was 9.2 and \
             66.9 B/key respectively, forever -- a pre-existing leak the flat format made \
             expensive enough to notice ({:.1}x)",
            per_ckpt[0] / nkeys as f64,
            per_ckpt[1] / nkeys as f64,
            per_ckpt[1] / per_ckpt[0].max(1.0)
        ),
    ));
    rec.finding(Finding::new(
        "F11.4",
        "the file grows by less than 10% in exchange",
        flat_bytes <= heap_bytes * 1.10,
        format!(
            "at one checkpoint, heap {:.1} MB -> flat {:.1} MB ({:+.1}%). An index read in \
             place cannot be compressed, and compactness is one of the two axes this engine \
             wins on. Measured on the minimal file: after six checkpoints the same stores are \
             {:.1} and {:.1} MB, but that gap is holes left by reclaimed sections rather than \
             the price of the format",
            heap_bytes / 1e6,
            flat_bytes / 1e6,
            (flat_bytes / heap_bytes - 1.0) * 100.0,
            churn_bytes[0] / 1e6,
            churn_bytes[1] / 1e6
        ),
    ));
    Ok(rec)
}

/// Open, then fault the index in the way a real reader would, then report.
fn f11_child(args: &Args) -> std::io::Result<()> {
    let file = PathBuf::from(args.get("--file").expect("--file"));
    let nkeys = args.num("--keys", 1) as u64;
    let lookups = args.num("--lookups", 1) as u64;
    let baseline = env::rss_bytes();
    let t = Instant::now();
    let reader = Reader::open(&file)?;
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    // Touch a realistic working set. A mapped index is demand-faulted, so
    // resident size straight after open measures how little has been read
    // rather than how little is needed.
    let mut rng = Rng::new(0xC11D);
    let mut kb = [0u8; 16];
    let mut hits = 0u64;
    for _ in 0..lookups {
        db_key_into(rng.next() % nkeys.max(1), &mut kb);
        hits += reader.read_all(&kb, |v| {
            std::hint::black_box(v);
        })?;
    }
    std::hint::black_box(hits);
    println!(
        "baseline_rss_bytes={baseline} index_rss_bytes={} open_ms={open_ms:.3} keys={}",
        env::rss_bytes().saturating_sub(baseline),
        reader.keys()
    );
    Ok(())
}

// ------------------------------------------------- F8: the cost of checksums --

/// What integrity checking costs, measured the only way that means anything.
///
/// The tempting comparison -- run the suite before the change, run it after,
/// subtract -- is worthless across separate runs. When it was tried here the
/// unchanged comparators in the external suite moved by +20% to +43% between
/// the two runs, so the "improvement" attributed to the change was mostly the
/// machine being in a different mood. Both arms therefore run in one process,
/// interleaved round-robin, with nothing else between them.
///
/// The safety this buys is not optional in any real deployment: without it a
/// bit flip, a torn write or a reused slot returns silently wrong data,
/// because LZ4 decodes many corrupted inputs into plausible bytes. The
/// question is only what it costs.
fn f8_checksums(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let depth = args.num("--depth", 4) as u64;
    let value_size = args.num("--value-size", 100);
    let reads = args.num("--reads", profile.pick(50_000, 200_000, 500_000)) as u64;

    let mut rec = Record::new("f8-checksums", profile);
    rec.param("keys", J::u(keys))
        .param("values_per_key", J::u(depth))
        .param("value_size", J::u(value_size as u64))
        .param("reads", J::u(reads))
        .note("both arms interleaved in one process; the only difference is Options::checksums");

    let dir = scratch("f8");
    let payload = Payload::new(value_size, 0.5, 0xF8);
    let on = [true, false];

    // Write throughput.
    let trial = Trial::new(profile.reps());
    let write = trial.run(2, |ci, rep| {
        let file = dir.join(format!("w{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                checksums: on[ci],
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0xF8 + rep as u64);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        for i in 0..(keys * depth) {
            db_key_into(i % keys, &mut kb);
            store.append(&kb, payload.get(&mut vrng)).expect("append");
        }
        store.flush().expect("flush");
        let secs = t.elapsed().as_secs_f64();
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        (keys * depth) as f64 / secs
    });

    // Read throughput, and the stored size, on a store built once per arm.
    let mut read_samples = Vec::new();
    let mut sizes = Vec::new();
    for (ci, want) in on.iter().enumerate() {
        let file = dir.join(format!("r{ci}.dat"));
        {
            let store = Store::create(
                &file,
                Options {
                    checksums: *want,
                    ..default_opts(128)
                },
            )
            .expect("create");
            let mut vrng = Rng::new(0xF8);
            let mut kb = [0u8; 16];
            for i in 0..(keys * depth) {
                db_key_into(i % keys, &mut kb);
                store.append(&kb, payload.get(&mut vrng)).expect("append");
            }
            store.close().expect("close");
        }
        sizes.push(file_len(&file));
        read_samples.push(file);
    }
    let read = Trial::new(profile.reps()).run(2, |ci, _| {
        // The flag is global, so it is set for the arm being measured.
        let reader = Reader::open(&read_samples[ci]).expect("open");
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0xF8);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        for _ in 0..reads {
            db_key_into(g.next(), &mut kb);
            reader
                .read_all(&kb, |v| {
                    std::hint::black_box(v);
                })
                .expect("read");
        }
        reads as f64 / t.elapsed().as_secs_f64()
    });
    for f in &read_samples {
        let _ = std::fs::remove_file(f);
    }

    let wc = compare(&write[0], &write[1], supdb::bench::MIN_EFFECT);
    let rc = compare(&read[0], &read[1], supdb::bench::MIN_EFFECT);
    rec.compare("write_on_vs_off", wc.clone());
    rec.compare("read_on_vs_off", rc.clone());
    rec.series(
        "write",
        jobj! {
            "checksums_on_ops_per_s" => J::fp(write[0].median(), 1),
            "checksums_off_ops_per_s" => J::fp(write[1].median(), 1),
            "cost_pct" => J::fp((1.0 - write[0].median() / write[1].median()) * 100.0, 2),
            "on" => write[0].to_json(),
            "off" => write[1].to_json(),
        },
    )
    .series(
        "read",
        jobj! {
            "checksums_on_ops_per_s" => J::fp(read[0].median(), 1),
            "checksums_off_ops_per_s" => J::fp(read[1].median(), 1),
            "cost_pct" => J::fp((1.0 - read[0].median() / read[1].median()) * 100.0, 2),
            "on" => read[0].to_json(),
            "off" => read[1].to_json(),
        },
    )
    .series(
        "space",
        jobj! {
            "checksums_on_bytes" => J::u(sizes[0]),
            "checksums_off_bytes" => J::u(sizes[1]),
            "cost_pct" => J::fp((sizes[0] as f64 / sizes[1] as f64 - 1.0) * 100.0, 3),
        },
    );

    let wcost = (1.0 - write[0].median() / write[1].median()) * 100.0;
    let rcost = (1.0 - read[0].median() / read[1].median()) * 100.0;
    let scost = (sizes[0] as f64 / sizes[1] as f64 - 1.0) * 100.0;
    rec.finding(Finding::new(
        "F8.1",
        "block checksums cost less than 10% of write throughput",
        wcost < 10.0,
        format!("write {wcost:+.1}% ({})", wc.summary("on", "off")),
    ));
    rec.finding(Finding::new(
        "F8.2",
        "block checksums cost less than 10% of read throughput",
        rcost < 10.0,
        format!("read {rcost:+.1}% ({})", rc.summary("on", "off")),
    ));
    rec.finding(Finding::new(
        "F8.3",
        "block checksums cost less than 1% of stored size",
        scost < 1.0,
        format!("{scost:+.3}% on disk: four bytes per chunk plus one per block"),
    ));
    Ok(rec)
}
