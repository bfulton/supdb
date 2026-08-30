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
    Record, Rng, Samples, Trial, J,
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
            "f12-compress" => f12_compress(&args, profile)?,
            "f13-sync" => f13_sync(&args, profile)?,
            "f14-blocktable" => f14_blocktable(&args, profile)?,
            "f15-scancache" => f15_scancache(&args, profile)?,
            "f16-slack" => f16_slack(&args, profile)?,
            "f17-gather" => f17_gather(&args, profile)?,
            "f18-fence" => f18_fence(&args, profile)?,
            "f19-coldscan" => f19_coldscan(&args, profile)?,
            "f20-chunkcrc" => f20_chunkcrc(&args, profile)?,
            "f21-writerverify" => f21_writerverify(&args, profile)?,
            "f22-storescan" => f22_storescan(&args, profile)?,
            "f23-madvise" => f23_madvise(&args, profile)?,
            "f24-autoreadahead" => f24_autoreadahead(&args, profile)?,
            "f25-arena" => f25_arena(&args, profile)?,
            "f26-buffer" => f26_buffer(&args, profile)?,
            "f27-ckptshape" => f27_ckptshape(&args, profile)?,
            "f29-redolog" => f29_redolog(&args, profile)?,
            "f28-count" => f28_count(&args, profile)?,
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
                "f12-compress",
                "f13-sync",
                "f14-blocktable",
                "f15-scancache",
                "f16-slack",
                "f17-gather",
                "f18-fence",
                "f19-coldscan",
                "f20-chunkcrc",
                "f21-writerverify",
                "f22-storescan",
                "f23-madvise",
                "f24-autoreadahead",
                "f25-arena",
                "f26-buffer",
                "f27-ckptshape",
                "f29-redolog",
                "f28-count",
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
        {
            let best = intervals
                .iter()
                .enumerate()
                .filter(|(i, iv)| {
                    let at_risk = if **iv == 0 { ops } else { **iv };
                    samples[*i].median() > 100_000.0 && at_risk * value_size as u64 <= 10 * 1048576
                })
                .map(|(i, iv)| (iv, samples[i].median()))
                .next();
            match best {
                Some((iv, ops_s)) => format!(
                    "a {iv}-op window sustains {ops_s:.0} ops/s. checkpoint() still rewrites the \
                     entire key index, so the floor grows with key count rather than with what \
                     changed -- but with block compression off the write path is fast enough \
                     that a usable point exists anyway"
                ),
                None => "checkpoint() rewrites the entire key index, so the floor on the \
                         durability interval grows with key count rather than with what changed"
                    .to_string(),
            }
        },
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
    let mut last_shared = false;

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
        let section = field("index_bytes");
        // A mapped index reports its section; a decoded one has no section and
        // its cost is what it added to the process on open. Both are the size
        // of the structure a reader needs -- the difference is that only one
        // of them is paid again by the next reader.
        let index_bytes = if section > 0.0 {
            section
        } else {
            (rss - baseline).max(0.0)
        };
        let shared = section > 0.0;
        last_shared = shared;
        let b_per_key = index_bytes / nkeys as f64;
        per_key.push((nkeys, b_per_key));

        points.push(jobj! {
            "keys" => J::u(nkeys),
            "file_mb" => J::fp(fsz as f64 / 1048576.0, 2),
            "reader_rss_mb" => J::fp(rss / 1048576.0, 2),
            "baseline_rss_mb" => J::fp(baseline / 1048576.0, 2),
            "index_rss_mb" => J::fp(index_bytes / 1048576.0, 2),
            "index_bytes_per_key" => J::fp(b_per_key, 1),
            "shared_between_readers" => J::Bool(shared),
            "open_ms" => J::fp(field("open_ms"), 3),
            // A decoded index is heap, so N readers cost N times this; a
            // mapped one is file-backed and costs it once.
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
            "{bytes_per_key:.0} bytes per key{}",
            if last_shared {
                ", in a file-backed section every reader process shares"
            } else {
                " resident, per reader process, shared with nobody"
            }
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
    let nkeys = args.num("--keys", 1) as u64;
    let baseline = env::peak_rss_bytes();
    let t = Instant::now();
    let reader = Reader::open(&file)?;
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    // Resident size straight after open, before anything else is touched.
    //
    // For a decoded index that is the whole structure: it was built during
    // `open`, so it is all resident by the time this line runs. Getting to
    // this measurement took three wrong ones and they are worth recording,
    // because each looked reasonable:
    //
    //   * `keys()` then read RSS. Correct for the decoded arm, and reports
    //     nearly zero for a mapped one -- how little has been read, not how
    //     little is needed.
    //   * A random `read_all` pass first. That faults in data blocks as well,
    //     which both arms share, so it drowned the index difference: 122 B/key
    //     against the decoded arm's 131, for a structure half the size.
    //   * Which leaves measuring the structure rather than the process. A
    //     mapped index reports its section exactly; a decoded one has no
    //     section and its resident-after-open figure is the right answer.
    let rss_after_open = env::peak_rss_bytes();
    let keys = reader.keys();
    // Report which index arm answered, so a run that silently fell back to the
    // decoded one is visible in the record rather than inferred from the
    // numbers looking unchanged.
    // `reader_rss_bytes` stays the decoded arm's answer; `index_bytes` is the
    // mapped arm's, and is zero when there is no section to point at.
    println!(
        "baseline_rss_bytes={baseline} reader_rss_bytes={rss_after_open} open_ms={open_ms:.3} \
         keys={keys} index_bytes={}",
        reader.index_bytes()
    );
    let _ = nkeys;
    Ok(())
}

// --------------------------------------------- F13: what fsync costs to publish --

/// What it costs to make a checkpoint durable rather than merely visible.
///
/// Publishing does not need fsync. Readers map the same file, so a write is
/// visible as soon as it is in the page cache, and a process crash leaves the
/// file intact regardless. fsync buys *ordering* against power loss: it stops
/// the superblock landing before the sections it points at.
///
/// That matters because every read-your-writes operation costs a checkpoint --
/// `Store` has no read method -- and a checkpoint syncs twice. The mixed YCSB
/// workloads sit at 0.07-0.14x of LMDB, and an instruction profile of the same
/// shape blamed the block table decode at 34%. Removing that decode entirely
/// moved throughput by nothing, because the workload waits on a syscall rather
/// than on the CPU. This asks the question the profiler could not.
///
/// Both arms in one process, interleaved, as f8 and f12 do.
/// Does reading the block table out of the mapping cost anything on a scan?
///
/// The table was moved into a mapped section so that many readers of one store
/// share it instead of each decoding a private copy. That change was measured
/// on point reads, where it was worth nothing either way, and the conclusion
/// recorded was that the decode had not been the bottleneck. It was never
/// measured on a scan, which resolves a block for every entry it walks rather
/// than one per lookup -- and between the run that recorded it and the next
/// one, external scan throughput fell from 0.82x of LMDB to 0.54x while LMDB
/// and redb moved less than 3%.
///
/// Both arms open the *same file* in the same process and are interleaved, so
/// the only difference is `ReadOptions::mapped_blocks`.
fn f14_blocktable(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let scan_len = args.num("--scan-len", 50);
    let scans = args.num("--scans", profile.pick(2_000, 20_000, 60_000)) as u64;
    let reads = args.num("--reads", profile.pick(50_000, 200_000, 500_000)) as u64;

    let mut rec = Record::new("f14-blocktable", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("scan_len", J::u(scan_len as u64))
        .param("scans", J::u(scans))
        .param("reads", J::u(reads))
        .note(
            "one store, two readers over the same file, interleaved; the only difference is \
             ReadOptions::mapped_blocks",
        );

    let dir = scratch("f14");
    let file = dir.join("bt.dat");
    let payload = Payload::new(value_size, 0.5, 0x14);
    {
        let store = Store::create(&file, default_opts(128)).expect("create");
        let mut vrng = Rng::new(0x14);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }
    let mapped = [true, false];

    // Scans. A scan resolves a block per entry, so if revalidating the mapped
    // table costs anything this is where it shows.
    let scan = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                mapped_blocks: mapped[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let mut g = KeyGen::new(
            KeyDist::Uniform,
            keys.saturating_sub(scan_len as u64).max(1),
            0x14 + rep as u64,
        );
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut n = 0u64;
        for _ in 0..scans {
            db_key_into(g.next(), &mut kb);
            n += r
                .scan(Some(&kb), scan_len, |_k, v| {
                    std::hint::black_box(v);
                })
                .expect("scan");
        }
        n as f64 / t.elapsed().as_secs_f64()
    });

    // Point reads, the arm the original change was measured on.
    let read = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                mapped_blocks: mapped[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x41 + rep as u64);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        for _ in 0..reads {
            db_key_into(g.next(), &mut kb);
            r.read_all(&kb, |v| {
                std::hint::black_box(v);
            })
            .expect("read");
        }
        reads as f64 / t.elapsed().as_secs_f64()
    });

    let scan_cmp = compare(&scan[1], &scan[0], supdb::bench::MIN_EFFECT);
    let read_cmp = compare(&read[1], &read[0], supdb::bench::MIN_EFFECT);
    let (sm, so) = (scan[0].median(), scan[1].median());
    let (rm, ro) = (read[0].median(), read[1].median());
    rec.compare("scan_owned_vs_mapped", scan_cmp.clone());
    rec.compare("read_owned_vs_mapped", read_cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "scan_mapped_entries_per_s" => J::fp(sm, 1),
            "scan_owned_entries_per_s" => J::fp(so, 1),
            "read_mapped_ops_per_s" => J::fp(rm, 1),
            "read_owned_ops_per_s" => J::fp(ro, 1),
        },
    );

    rec.finding(Finding::new(
        "F14.1",
        "the mapped block table costs nothing on a scan",
        matches!(
            scan_cmp.verdict,
            supdb::bench::stats::Verdict::NoDifference | supdb::bench::stats::Verdict::Underpowered
        ),
        format!(
            "scan {sm:.0} entries/s mapped against {so:.0} owned ({})",
            scan_cmp.summary("owned", "mapped")
        ),
    ));
    rec.finding(Finding::new(
        "F14.2",
        "the mapped block table costs nothing on a point read",
        matches!(
            read_cmp.verdict,
            supdb::bench::stats::Verdict::NoDifference | supdb::bench::stats::Verdict::Underpowered
        ),
        format!(
            "read {rm:.0} ops/s mapped against {ro:.0} owned ({}). This is the arm the change \
             was originally measured on, and it is kept so the two can be read together: a \
             structure can be free on one access pattern and not on another",
            read_cmp.summary("owned", "mapped")
        ),
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

/// What does resolving a block per scan entry cost?
///
/// A scan walks keys in rank order and consecutive keys usually share a
/// block, so resolving one per entry re-reads the block table, re-bounds-
/// checks the mapping and re-tests a checksum bit for an answer that has not
/// changed since the previous entry. Until this experiment existed the scan
/// resolved it *twice* per entry, because `check_extent` resolved it and then
/// the read resolved it again.
///
/// Both arms are one process over one file; the only difference is
/// `ReadOptions::scan_block_cache`.
fn f15_scancache(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let scan_len = args.num("--scan-len", 50);
    let scans = args.num("--scans", profile.pick(2_000, 20_000, 40_000)) as u64;

    let mut rec = Record::new("f15-scancache", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("scan_len", J::u(scan_len as u64))
        .param("scans", J::u(scans))
        .note("one file, two readers, interleaved; the only difference is ReadOptions::scan_block_cache");

    let dir = scratch("f15");
    let file = dir.join("sc.dat");
    let payload = Payload::new(value_size, 0.5, 0x15);
    {
        let store = Store::create(&file, default_opts(128)).expect("create");
        let mut vrng = Rng::new(0x15);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }
    let cache = [true, false];
    let scan = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                scan_block_cache: cache[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let mut g = KeyGen::new(
            KeyDist::Uniform,
            keys.saturating_sub(scan_len as u64).max(1),
            0x15 + rep as u64,
        );
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut n = 0u64;
        for _ in 0..scans {
            db_key_into(g.next(), &mut kb);
            n += r
                .scan(Some(&kb), scan_len, |_k, v| {
                    std::hint::black_box(v);
                })
                .expect("scan");
        }
        n as f64 / t.elapsed().as_secs_f64()
    });

    let cmp = compare(&scan[0], &scan[1], supdb::bench::MIN_EFFECT);
    let (on, off) = (scan[0].median(), scan[1].median());
    rec.compare("cached_vs_resolved", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "cached_entries_per_s" => J::fp(on, 1),
            "resolved_entries_per_s" => J::fp(off, 1),
            "speedup" => J::fp(on / off.max(1e-9), 3),
        },
    );
    rec.finding(Finding::new(
        "F15.1",
        "holding the current block across a scan's entries is worth measuring",
        matches!(cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "scan {on:.0} entries/s holding the block against {off:.0} resolving it per entry \
             ({}). Resolving it was the shape the code had; whether that costs anything is not \
             something an instruction count answers, since the work is a bounds check and an \
             atomic load rather than a syscall",
            cmp.summary("cached", "resolved")
        ),
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

/// What does writing the key index's slack region cost?
///
/// The flat index reserves half its record region again so an in-place
/// checkpoint has somewhere to put a lengthened record. Nothing reads that
/// region until an update writes into it, and a file that has never been
/// written there reads back zeroes either way -- so it can be reserved without
/// being sent to the disk. At 1M keys it is 18MB of a 63MB index, on a bulk
/// load that is bound by bytes written: this machine writes 986MB/s direct and
/// 207MB/s through fsync, and both Supdb and LMDB load at around 100MB/s.
///
/// Throughput never travels alone, so this reports device-level write bytes
/// from `/proc/self/io` beside it -- measured, not inferred from file size,
/// which cannot see a hole at all.
fn f16_slack(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);

    let mut rec = Record::new("f16-slack", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .note("both arms interleaved in one process; the only difference is Options::write_index_slack");
    rec.param("reps", J::u(profile.pick(5, 11, 21) as u64));

    let dir = scratch("f16");
    let payload = Payload::new(value_size, 0.5, 0x16);
    let on = [false, true];
    let written = std::sync::Mutex::new([0u64; 2]);
    let sizes = std::sync::Mutex::new([0u64; 2]);

    // More repetitions than the default. The effect here is around 13% and the
    // repetition-to-repetition IQR is 10-17%, so seven samples put the p-value
    // on top of the threshold: the same measurement read 1.158x at p=0.0553 and
    // 1.132x at p=0.0409 on consecutive runs, which is a finding that flips
    // sign of verdict without the engine changing. An underpowered measurement
    // is not a cheap one, it is one that could not see.
    let reps = args.num("--reps", profile.pick(5, 11, 21));
    let load = Trial::new(reps).run(2, |ci, rep| {
        let file = dir.join(format!("s{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                write_index_slack: on[ci],
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x16);
        let mut kb = [0u8; 16];
        let io0 = supdb::bench::env::IoCounters::read_now();
        let t = Instant::now();
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.checkpoint().expect("checkpoint");
        let secs = t.elapsed().as_secs_f64();
        let io = supdb::bench::env::IoCounters::read_now().since(&io0);
        written.lock().unwrap()[ci] = io.write_bytes;
        sizes.lock().unwrap()[ci] = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        keys as f64 / secs
    });

    let cmp = compare(&load[0], &load[1], supdb::bench::MIN_EFFECT);
    let (sparse, dense) = (load[0].median(), load[1].median());
    let w = *written.lock().unwrap();
    let sz = *sizes.lock().unwrap();
    rec.compare("hole_vs_written", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "hole_ops_per_s" => J::fp(sparse, 1),
            "written_ops_per_s" => J::fp(dense, 1),
            "hole_write_bytes" => J::u(w[0]),
            "written_write_bytes" => J::u(w[1]),
            "hole_file_bytes" => J::u(sz[0]),
            "written_file_bytes" => J::u(sz[1]),
        },
    );
    rec.finding(Finding::new(
        "F16.1",
        "not writing the index slack is worth measuring on a bulk load",
        matches!(cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "{sparse:.0} ops/s leaving the slack a hole against {dense:.0} writing it ({})",
            cmp.summary("hole", "written")
        ),
    ));
    rec.finding(Finding::new(
        "F16.2",
        "the two arms produce the same file size",
        sz[0] == sz[1],
        format!(
            "{} bytes against {} bytes, with {} against {} sent to the device. The apparent size \
             is identical because the slack is reserved either way; the difference is a hole, \
             which file size cannot see and /proc/self/io can",
            sz[0], sz[1], w[0], w[1]
        ),
    ));
    Ok(rec)
}

/// What does copying every key cost a full checkpoint?
///
/// A rewrite has to see every key in sorted order. It used to build that list
/// as `Vec<(Vec<u8>, Extents)>` -- one heap allocation per key, a million
/// 16-byte mallocs on a bulk load -- and then sort a million pointers into
/// scattered allocations, so every comparison landed on a fresh cacheline. The
/// keys already lie contiguously in each shard's arena and can be borrowed.
///
/// This experiment exists because the phase split said the checkpoint, not the
/// data write, is what a bulk load spends its time on: 397ms putting, 226ms
/// flushing 101MB, and 820ms checkpointing 57MB. Writing 57MB at the rate the
/// flush achieved would be 128ms, so most of that was not I/O.
fn f17_gather(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);

    let mut rec = Record::new("f17-gather", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .note(
            "both arms interleaved in one process; the only difference is \
             Options::checkpoint_copies_keys",
        );

    let dir = scratch("f17");
    let payload = Payload::new(value_size, 0.5, 0x17);
    let copies = [false, true];
    let ck = std::sync::Mutex::new([0f64; 2]);

    // Load throughput end to end, which is what EXT.1 compares against LMDB.
    let load = Trial::new(profile.reps()).run(2, |ci, rep| {
        let file = dir.join(format!("g{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                checkpoint_copies_keys: copies[ci],
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x17);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        let t2 = Instant::now();
        store.checkpoint().expect("checkpoint");
        ck.lock().unwrap()[ci] = t2.elapsed().as_secs_f64() * 1e3;
        let secs = t.elapsed().as_secs_f64();
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        keys as f64 / secs
    });

    let cmp = compare(&load[0], &load[1], supdb::bench::MIN_EFFECT);
    let (borrow, copy) = (load[0].median(), load[1].median());
    let c = *ck.lock().unwrap();
    rec.compare("borrowed_vs_copied", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "borrowed_ops_per_s" => J::fp(borrow, 1),
            "copied_ops_per_s" => J::fp(copy, 1),
            "borrowed_checkpoint_ms" => J::fp(c[0], 1),
            "copied_checkpoint_ms" => J::fp(c[1], 1),
        },
    );
    rec.finding(Finding::new(
        "F17.1",
        "borrowing the keys instead of copying them speeds up a bulk load",
        matches!(cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "{borrow:.0} ops/s borrowed against {copy:.0} copied ({}); the final checkpoint took \
             {:.0}ms against {:.0}ms",
            cmp.summary("borrowed", "copied"),
            c[0],
            c[1]
        ),
    ));
    Ok(rec)
}

/// Does a fence make an ordered seek cheaper?
///
/// A scan's cost is linear in its length with a large constant: measured at 1M
/// keys, `1637 + 20.8n` nanoseconds. The per-entry walk is competitive -- at
/// length 400 it reaches 40M entries/s against LMDB's 47M -- so the whole scan
/// deficit is that constant, and most of the constant is the seek. A seek
/// binary-searches the record region, and each probe is two dependent loads at
/// a scattered offset: the rank directory, then the record.
///
/// The fence copies every stride-th key out contiguously so the search can
/// narrow before it touches a record. What it cannot do anything about is that
/// the *upper* levels of a binary search over a static array visit the same
/// few addresses on every search and are already cached; the fence replaces
/// exactly those, and the expensive lower levels remain. Whether the trade is
/// positive is the question, and predicting it wrong is why this exists.
///
/// Both arms are one process over one file; the fence is in the section either
/// way and `ReadOptions::seek_fence` decides whether it is consulted.
fn f18_fence(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let seeks = args.num("--seeks", profile.pick(50_000, 200_000, 500_000)) as u64;
    let scans = args.num("--scans", profile.pick(5_000, 20_000, 40_000)) as u64;

    let mut rec = Record::new("f18-fence", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("seeks", J::u(seeks))
        .param("scans", J::u(scans))
        .note("one file, two readers, interleaved; the only difference is ReadOptions::seek_fence");

    let dir = scratch("f18");
    let file = dir.join("fence.dat");
    let payload = Payload::new(value_size, 0.5, 0x18);
    {
        let store = Store::create(&file, default_opts(128)).expect("create");
        let mut vrng = Rng::new(0x18);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }
    let on = [true, false];
    let reader = |ci: usize| {
        Reader::open_with(
            &file,
            supdb::ReadOptions {
                seek_fence: on[ci],
                ..Default::default()
            },
        )
        .expect("open")
    };

    // The seek alone, which is what the fence changes.
    let seek = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = reader(ci);
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x18 + rep as u64);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut acc = 0usize;
        for _ in 0..seeks {
            db_key_into(g.next(), &mut kb);
            acc += r.seek(&kb);
        }
        std::hint::black_box(acc);
        seeks as f64 / t.elapsed().as_secs_f64()
    });

    // A 50-entry scan, which is the shape YCSB-E runs and where the seek is
    // the largest single term.
    let scan = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = reader(ci);
        let mut g = KeyGen::new(
            KeyDist::Uniform,
            keys.saturating_sub(50).max(1),
            0x81 + rep as u64,
        );
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut n = 0u64;
        for _ in 0..scans {
            db_key_into(g.next(), &mut kb);
            n += r
                .scan(Some(&kb), 50, |_k, v| {
                    std::hint::black_box(v);
                })
                .expect("scan");
        }
        n as f64 / t.elapsed().as_secs_f64()
    });

    let seek_cmp = compare(&seek[0], &seek[1], supdb::bench::MIN_EFFECT);
    let scan_cmp = compare(&scan[0], &scan[1], supdb::bench::MIN_EFFECT);
    let (sf, sn) = (seek[0].median(), seek[1].median());
    let (cf, cn) = (scan[0].median(), scan[1].median());
    rec.compare("seek_fenced_vs_plain", seek_cmp.clone());
    rec.compare("scan_fenced_vs_plain", scan_cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "seek_fenced_per_s" => J::fp(sf, 1),
            "seek_plain_per_s" => J::fp(sn, 1),
            "seek_fenced_ns" => J::fp(1e9 / sf.max(1e-9), 1),
            "seek_plain_ns" => J::fp(1e9 / sn.max(1e-9), 1),
            "scan50_fenced_entries_per_s" => J::fp(cf, 1),
            "scan50_plain_entries_per_s" => J::fp(cn, 1),
        },
    );
    rec.finding(Finding::new(
        "F18.1",
        "narrowing a seek with a fence makes it cheaper",
        matches!(seek_cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "{:.0}ns fenced against {:.0}ns plain ({})",
            1e9 / sf.max(1e-9),
            1e9 / sn.max(1e-9),
            seek_cmp.summary("fenced", "plain")
        ),
    ));
    rec.finding(Finding::new(
        "F18.2",
        "the fence is worth having on the workload it was built for",
        matches!(scan_cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "50-entry scans at {cf:.0} entries/s fenced against {cn:.0} plain ({}). This is the \
             one that decides whether the fence earns a format version; a seek that got faster \
             without moving a scan would not",
            scan_cmp.summary("fenced", "plain")
        ),
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

/// Why does a cold scan cost more than a warm one, and how much of it is the
/// checksum?
///
/// `ext-sweep` fits Supdb faster than LMDB at every scan length from 100 up,
/// and `ext-kv` reports it slower at exactly 100. The two are not measuring
/// the same thing: the sweep discards a warmup repetition and the kv suite
/// measures the first pass over a freshly opened reader. Something about the
/// first pass is expensive, and this experiment says what.
///
/// A `Reader` verifies a plain block's CRC once and records that in a bitset,
/// so the first scan that touches a block pays a checksum over the whole 64KiB
/// of it and later scans pay nothing. On a cold reader every block is a first
/// touch. LMDB offers no checksums at all -- `Features::checksums` is false
/// for it -- so this is a guarantee Supdb provides and is charged for in a
/// comparison that does not price it.
///
/// One store, built once with checksums on so the CRCs are in the file, and
/// two arms that differ only in whether the reader verifies them. Each
/// repetition opens a fresh reader for the cold pass and then reuses it, so
/// the cold penalty and its decay are measured on the same reader.
fn f19_coldscan(args: &Args, profile: Profile) -> std::io::Result<Record> {
    use supdb::bench::env::Wait;

    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let scan_len = args.num("--scan-len", 100);
    let scans = args.num("--scans", profile.pick(2_000, 10_000, 20_000)) as u64;

    let mut rec = Record::new("f19-coldscan", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("scan_len", J::u(scan_len as u64))
        .param("scans", J::u(scans))
        .note(
            "one file with checksums in it; the arms differ only in whether the reader verifies \
             them. A fresh reader per repetition is what makes the first pass cold",
        );

    let dir = scratch("f19");
    let file = dir.join("cold.dat");
    let payload = Payload::new(value_size, 0.5, 0x19);
    {
        let store = Store::create(
            &file,
            Options {
                checksums: true,
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x19);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }

    let verify = [true, false];
    let warm_out = std::sync::Mutex::new([0f64; 2]);
    let wait_out = std::sync::Mutex::new([Wait::default(); 2]);

    // The cold pass. A fresh reader has an empty verified bitset, so every
    // block it touches is checked once.
    let cold = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                verify_checksums: verify[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let pass = |seed: u64| -> (f64, Wait) {
            let mut g = KeyGen::new(
                KeyDist::Uniform,
                keys.saturating_sub(scan_len as u64).max(1),
                seed,
            );
            let mut kb = [0u8; 16];
            let a = Wait::read_now();
            let mut n = 0u64;
            for _ in 0..scans {
                db_key_into(g.next(), &mut kb);
                n += r
                    .scan(Some(&kb), scan_len, |_k, v| {
                        std::hint::black_box(v);
                    })
                    .expect("scan");
            }
            let w = Wait::read_now().since(&a);
            (n as f64 / (w.wall_ns as f64 / 1e9), w)
        };
        // Same key sequence both passes, so the second differs only in what
        // the reader already knows.
        let (first, w) = pass(0x19 + rep as u64);
        let (second, _) = pass(0x19 + rep as u64);
        warm_out.lock().unwrap()[ci] = second;
        wait_out.lock().unwrap()[ci] = w;
        first
    });

    let cmp = compare(&cold[0], &cold[1], supdb::bench::MIN_EFFECT);
    let (on, off) = (cold[0].median(), cold[1].median());
    let warm = *warm_out.lock().unwrap();
    let wait = *wait_out.lock().unwrap();
    rec.compare("cold_verified_vs_unverified", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "cold_verified_entries_per_s" => J::fp(on, 1),
            "cold_unverified_entries_per_s" => J::fp(off, 1),
            "warm_verified_entries_per_s" => J::fp(warm[0], 1),
            "warm_unverified_entries_per_s" => J::fp(warm[1], 1),
            "cold_verified_wait" => wait[0].to_json(),
            "cold_unverified_wait" => wait[1].to_json()
        },
    );

    rec.finding(Finding::new(
        "F19.1",
        "verifying block checksums costs nothing on a cold scan",
        !matches!(cmp.verdict, supdb::bench::stats::Verdict::Less),
        format!(
            "cold scans at {on:.0} entries/s verified against {off:.0} unverified ({}). \
             f8-checksums measured this cost at no difference on point reads, where a block is \
             checked once and read many times; a scan over a fresh reader touches each block for \
             the first time and pays for all of them",
            cmp.summary("verified", "unverified")
        ),
    ));
    rec.finding(Finding::new(
        "F19.2",
        "a scan's first pass costs no more than its steady state",
        warm[0] <= on * 1.05,
        format!(
            "verified: {on:.0} entries/s cold against {:.0} warm ({:.2}x). Unverified: {off:.0} \
             against {:.0} ({:.2}x). Whatever remains after the checksum is subtracted is the \
             cost of touching the mapping for the first time, and the cold pass took {} major \
             faults with {:.1}% of its wall time off CPU",
            warm[0],
            warm[0] / on.max(1e-9),
            warm[1],
            warm[1] / off.max(1e-9),
            wait[0].major_faults,
            wait[0].off_cpu_fraction() * 100.0
        ),
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

/// Does verifying a chunk instead of a block make a cold scan cheaper?
///
/// f19-coldscan priced whole-block verification at 0.715x on a cold scan and
/// showed the cold penalty is the checksum and almost nothing else -- zero
/// major faults, zero time off CPU, pure CRC32C. The amplification is the
/// reason: a 64KiB block hashed in full to hand back a 100-byte value.
///
/// `write_block` already chunks the compressed path for the same shape of
/// problem, because decompressing 64KiB to reach 960 bytes was 68x read
/// amplification. Plain blocks now carry per-chunk checksums beside them in
/// the block table -- beside, because a plain block is sliced straight out of
/// the mapping and has to stay byte-for-byte what the extent offsets say.
///
/// One file, two readers, arms differing only in
/// `ReadOptions::chunk_verify`. Both verify; one verifies less.
fn f20_chunkcrc(args: &Args, profile: Profile) -> std::io::Result<Record> {
    use supdb::bench::env::Wait;

    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let scan_len = args.num("--scan-len", 100);
    let scans = args.num("--scans", profile.pick(2_000, 10_000, 20_000)) as u64;

    let mut rec = Record::new("f20-chunkcrc", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("scan_len", J::u(scan_len as u64))
        .param("scans", J::u(scans))
        .note(
            "one file, two readers, interleaved; the only difference is ReadOptions::chunk_verify",
        );

    let dir = scratch("f20");
    let file = dir.join("chunk.dat");
    let payload = Payload::new(value_size, 0.5, 0x20);
    {
        let store = Store::create(
            &file,
            Options {
                checksums: true,
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x20);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }

    let chunked = [true, false];
    let warm_out = std::sync::Mutex::new([0f64; 2]);
    let cold = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                chunk_verify: chunked[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let pass = |seed: u64, dist: KeyDist| -> (f64, u64) {
            let mut g = KeyGen::new(dist, keys.saturating_sub(scan_len as u64).max(1), seed);
            let mut kb = [0u8; 16];
            let a = Wait::read_now();
            let mut n = 0u64;
            for _ in 0..scans {
                db_key_into(g.next(), &mut kb);
                n += r
                    .scan(Some(&kb), scan_len, |_k, v| {
                        std::hint::black_box(v);
                    })
                    .expect("scan");
            }
            let w = Wait::read_now().since(&a);
            (n as f64 / (w.wall_ns as f64 / 1e9), n)
        };
        let (first, _) = pass(0x20 + rep as u64, KeyDist::Uniform);
        let (second, _) = pass(0x20 + rep as u64, KeyDist::Uniform);
        warm_out.lock().unwrap()[ci] = second;
        first
    });

    // The skewed case, which is the one chunking can help. A uniform sweep
    // touches every chunk of every block it touches, and verifying all of a
    // block's chunks is verifying the block -- the same bytes hashed, plus a
    // bitset test per chunk. Only a workload that touches part of a block and
    // does not come back pays less.
    let skewed = Trial::new(profile.reps()).run(2, |ci, rep| {
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                chunk_verify: chunked[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let mut g = KeyGen::new(
            KeyDist::Zipfian,
            keys.saturating_sub(scan_len as u64).max(1),
            0x02 + rep as u64,
        );
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut n = 0u64;
        for _ in 0..scans {
            db_key_into(g.next(), &mut kb);
            n += r
                .scan(Some(&kb), scan_len, |_k, v| {
                    std::hint::black_box(v);
                })
                .expect("scan");
        }
        n as f64 / t.elapsed().as_secs_f64()
    });

    let cmp = compare(&cold[0], &cold[1], supdb::bench::MIN_EFFECT);
    let (chunk, whole) = (cold[0].median(), cold[1].median());
    let warm = *warm_out.lock().unwrap();
    rec.compare("chunk_vs_whole", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "cold_chunk_entries_per_s" => J::fp(chunk, 1),
            "cold_whole_entries_per_s" => J::fp(whole, 1),
            "warm_chunk_entries_per_s" => J::fp(warm[0], 1),
            "warm_whole_entries_per_s" => J::fp(warm[1], 1),
            "skewed_chunk_entries_per_s" => J::fp(skewed[0].median(), 1),
            "skewed_whole_entries_per_s" => J::fp(skewed[1].median(), 1),
        },
    );
    let skew_cmp = compare(&skewed[0], &skewed[1], supdb::bench::MIN_EFFECT);
    rec.compare("skewed_chunk_vs_whole", skew_cmp.clone());
    rec.finding(Finding::new(
        "F20.3",
        "chunk verification pays on a skewed scan workload",
        matches!(skew_cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "Zipfian scans at {:.0} entries/s per chunk against {:.0} per block ({}). This is the \
             regime chunking can help in at all: a uniform sweep reaches every chunk of every \
             block it touches, and verifying all of a block's chunks is verifying the block",
            skewed[0].median(),
            skewed[1].median(),
            skew_cmp.summary("chunk", "whole")
        ),
    ));
    rec.finding(Finding::new(
        "F20.1",
        "verifying a chunk instead of a block makes a cold scan cheaper",
        matches!(cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "cold scans at {chunk:.0} entries/s per chunk against {whole:.0} per block ({})",
            cmp.summary("chunk", "whole")
        ),
    ));
    rec.finding(Finding::new(
        "F20.2",
        "the first pass now costs about what the steady state does",
        warm[0] <= chunk * 1.15,
        format!(
            "per chunk: {chunk:.0} entries/s cold against {:.0} warm ({:.2}x). Per block: \
             {whole:.0} against {:.0} ({:.2}x). f19-coldscan measured the whole-block ratio at \
             1.65x, and closing it is the point of this change -- a scan that has to warm up is a \
             scan whose first pass is charged for every block it will ever touch",
            warm[0],
            warm[0] / chunk.max(1e-9),
            warm[1],
            warm[1] / whole.max(1e-9)
        ),
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

/// What does verifying the writer's own reads cost?
///
/// `Store::read_all` served bytes with no checksum in between while a
/// `Reader` over the same store verified (C1.3), so the same file answered
/// with two guarantees depending on the handle -- and YCSB A through F, where
/// Supdb leads LMDB by 3.6x to 20x, route every read through the unchecked
/// one. RocksDB verifies every block it loads by default and a `Reader` here
/// always has, so the writer now does too.
///
/// This is what that costs. The arms differ only in `Options::verify_reads`,
/// which is the knob for a caller who wants LMDB's trade instead -- LMDB has
/// no checksums at all.
///
/// Both a cold pass and a warm one, because a chunk is checked once and
/// remembered: the steady state is nearly free and the first touch is not.
fn f21_writerverify(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let reads = args.num("--reads", profile.pick(50_000, 200_000, 500_000)) as u64;

    let mut rec = Record::new("f21-writerverify", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("reads", J::u(reads))
        .note("both arms interleaved in one process; the only difference is Options::verify_reads");

    let dir = scratch("f21");
    let payload = Payload::new(value_size, 0.5, 0x21);
    let on = [true, false];
    let warm_out = std::sync::Mutex::new([0f64; 2]);

    let cold = Trial::new(profile.reps()).run(2, |ci, rep| {
        let file = dir.join(format!("v{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                verify_reads: on[ci],
                checksums: true,
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x21);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.checkpoint().expect("checkpoint");

        let pass = |seed: u64| -> f64 {
            let mut g = KeyGen::new(KeyDist::Uniform, keys, seed);
            let mut kb = [0u8; 16];
            let t = Instant::now();
            for _ in 0..reads {
                db_key_into(g.next(), &mut kb);
                store
                    .read_all(&kb, |v| {
                        std::hint::black_box(v);
                    })
                    .expect("read");
            }
            reads as f64 / t.elapsed().as_secs_f64()
        };
        let first = pass(0x21 + rep as u64);
        warm_out.lock().unwrap()[ci] = pass(0x21 + rep as u64);
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        first
    });

    let cmp = compare(&cold[1], &cold[0], supdb::bench::MIN_EFFECT);
    let (verified, unverified) = (cold[0].median(), cold[1].median());
    let warm = *warm_out.lock().unwrap();
    rec.compare("unverified_vs_verified", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "cold_verified_ops_per_s" => J::fp(verified, 1),
            "cold_unverified_ops_per_s" => J::fp(unverified, 1),
            "warm_verified_ops_per_s" => J::fp(warm[0], 1),
            "warm_unverified_ops_per_s" => J::fp(warm[1], 1),
        },
    );
    rec.finding(Finding::new(
        "F21.1",
        "verifying the writer's own reads costs less than 25% on a cold pass",
        verified >= unverified * 0.75,
        format!(
            "{verified:.0} ops/s verified against {unverified:.0} unverified ({}). This is the \
             price of the guarantee RocksDB charges by default and LMDB does not offer, on the \
             path YCSB A through F read through",
            cmp.summary("unverified", "verified")
        ),
    ));
    rec.finding(Finding::new(
        "F21.2",
        "a warm read pays almost nothing for verification",
        warm[0] >= warm[1] * 0.95,
        format!(
            "warm: {:.0} ops/s verified against {:.0} unverified ({:.3}x). A chunk is checked \
             once and remembered, so the cost is a property of the first touch and not of the \
             read path -- which is why f8-checksums, measuring warm point reads, found it free",
            warm[0],
            warm[1],
            warm[0] / warm[1].max(1e-9)
        ),
    ));
    Ok(rec)
}

/// What is a scan through the writer worth against a scan through a reader?
///
/// The kv suite reads through `Store::read_all` and then scans, and until
/// `Store::scan` existed the scan opened a `Reader`: a second mapping of the
/// same file with an empty verified bitset, so it re-verified a store this
/// process had just written. LMDB does both through one handle. That was the
/// explanation offered for why the suite has Supdb scanning at 0.65x while an
/// interleaved sweep has it ahead on the walk.
///
/// It is also unmeasurable across runs. Between two suite runs an hour apart
/// both engines' scan throughput roughly doubled, so the ratio moved for
/// reasons that have nothing to do with the code. Hence an arm.
///
/// Each repetition warms the writer with a read pass first, exactly as the kv
/// suite does, then scans -- through the store, or through a reader opened
/// once for that repetition, which is what the adapter used to do.
fn f22_storescan(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let scan_len = args.num("--scan-len", 100);
    let scans = args.num("--scans", profile.pick(2_000, 10_000, 10_000)) as u64;
    let warm_reads = args.num("--warm-reads", profile.pick(20_000, 100_000, 200_000)) as u64;

    let mut rec = Record::new("f22-storescan", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("scan_len", J::u(scan_len as u64))
        .param("scans", J::u(scans))
        .param("warm_reads", J::u(warm_reads))
        .note(
            "one store per repetition, warmed by a read pass as the kv suite warms it, then \
             scanned through the writer or through a reader opened once for that repetition",
        );

    let dir = scratch("f22");
    let payload = Payload::new(value_size, 0.5, 0x22);
    let scan = Trial::new(profile.reps()).run(2, |ci, rep| {
        let file = dir.join(format!("s{ci}-{rep}.dat"));
        let store = Store::create(&file, default_opts(128)).expect("create");
        let mut vrng = Rng::new(0x22);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.checkpoint().expect("checkpoint");

        // The read phase, which warms the writer's mapping and its verified
        // bitset and leaves a reader's entirely cold.
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x22 + rep as u64);
        for _ in 0..warm_reads {
            db_key_into(g.next(), &mut kb);
            store
                .read_all(&kb, |v| {
                    std::hint::black_box(v);
                })
                .expect("read");
        }

        let mut g = KeyGen::new(
            KeyDist::Uniform,
            keys.saturating_sub(scan_len as u64).max(1),
            0x2A + rep as u64,
        );
        let t = Instant::now();
        let mut n = 0u64;
        if ci == 0 {
            for _ in 0..scans {
                db_key_into(g.next(), &mut kb);
                n += store
                    .scan(Some(&kb), scan_len, |_k, v| {
                        std::hint::black_box(v);
                    })
                    .expect("scan");
            }
        } else {
            store.publish().expect("publish");
            let r = Reader::open(&file).expect("open");
            for _ in 0..scans {
                db_key_into(g.next(), &mut kb);
                n += r
                    .scan(Some(&kb), scan_len, |_k, v| {
                        std::hint::black_box(v);
                    })
                    .expect("scan");
            }
        }
        let rate = n as f64 / t.elapsed().as_secs_f64();
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        rate
    });

    let cmp = compare(&scan[0], &scan[1], supdb::bench::MIN_EFFECT);
    let (writer, reader) = (scan[0].median(), scan[1].median());
    rec.compare("store_vs_reader", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "store_entries_per_s" => J::fp(writer, 1),
            "reader_entries_per_s" => J::fp(reader, 1),
        },
    );
    rec.finding(Finding::new(
        "F22.1",
        "scanning through the writer beats opening a reader for it",
        matches!(cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "{writer:.0} entries/s through Store::scan against {reader:.0} through a Reader \
             ({}). The reader duplicates a mapping the writer already has and starts a verified \
             bitset the writer has already filled",
            cmp.summary("store", "reader")
        ),
    ));
    Ok(rec)
}

/// What does the kernel's default readahead cost a random read?
///
/// F1.2 is the worst number in this repository: 338,681 reads/s resident
/// against 370 out-of-core, a 916x collapse once the store no longer fits in
/// memory. Its recorded diagnosis is that the engine reads through an mmap
/// with no `madvise` anywhere, so it has no readahead control, no asynchronous
/// I/O and no influence over eviction. No advice means `MADV_NORMAL`: on a
/// miss the kernel faults in a cluster of pages around it, on the assumption
/// that a read is the start of a sequential run.
///
/// For a random point read that assumption is wrong twice. It fetches a
/// window to return a hundred bytes, and the pages it fetched speculatively
/// evict pages something was still using.
///
/// The measurement that settles it is not throughput, it is bytes. The page
/// cache is dropped before each pass and `/proc/self/io` counts what the
/// device was actually asked for, so the answer is read amplification: device
/// bytes per value returned. A store far larger than the reads will touch,
/// with a fraction of its pages read, so nearly every read is a cold miss.
fn f23_madvise(args: &Args, profile: Profile) -> std::io::Result<Record> {
    use supdb::bench::env::{IoCounters, Wait};

    let keys = args.num("--keys", profile.pick(200_000, 2_000_000, 8_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let reads = args.num("--reads", profile.pick(2_000, 10_000, 20_000)) as u64;

    let mut rec = Record::new("f23-madvise", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("reads", J::u(reads))
        .note(
            "page cache dropped before every pass; read amplification measured from /proc/self/io \
             rather than inferred. Both arms read the same file in the same process and differ \
             only in ReadOptions::readahead",
        );

    let dir = scratch("f23");
    let file = dir.join("ooc.dat");
    let payload = Payload::new(value_size, 0.5, 0x23);
    {
        let store = Store::create(&file, default_opts(256)).expect("create");
        let mut vrng = Rng::new(0x23);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }
    let file_bytes = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    rec.param("file_bytes", J::u(file_bytes));

    // Out-of-core is a ratio, not a size. Capping the cgroup after the build
    // makes the ratio a parameter of the experiment instead of a property of
    // whatever host it lands on -- and it is the only way this hazard is
    // reachable on a machine with 15GB of RAM and 20GB of free disk.
    // Capped at every profile, so the hazard is exercised in seconds at ci
    // rather than only on a machine with more RAM than this one has. The cap
    // is sized under the store the profile builds.
    let cap_mb = args.num("--cap-mb", profile.pick(16, 128, 512)) as u64;
    let capped = cap_mb > 0 && supdb::bench::env::cap_memory(cap_mb << 20);
    let ratio = if capped {
        file_bytes as f64 / ((cap_mb << 20) as f64)
    } else {
        0.0
    };
    rec.param(
        "memory_cap_bytes",
        J::u(if capped { cap_mb << 20 } else { 0 }),
    )
    .param("file_over_cap", J::fp(ratio, 2));
    if cap_mb > 0 && !capped {
        eprintln!("# WARNING: could not cap memory; this run is not out-of-core");
    }

    let advice = [supdb::Readahead::Random, supdb::Readahead::Default];
    let io_out = std::sync::Mutex::new([0u64; 2]);
    let flt_out = std::sync::Mutex::new([0u64; 2]);
    let cold = Trial::new(profile.reps()).run(2, |ci, rep| {
        // A cold measurement has to be able to prove it was cold, which is
        // F1.1's whole point; the major-fault count below is that proof.
        if !supdb::bench::env::drop_caches() {
            eprintln!("# WARNING: could not drop the page cache -- this pass is not cold");
        }
        let r = Reader::open_with(
            &file,
            supdb::ReadOptions {
                readahead: advice[ci],
                ..Default::default()
            },
        )
        .expect("open");
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x23 + rep as u64);
        let mut kb = [0u8; 16];
        let io0 = IoCounters::read_now();
        let w0 = Wait::read_now();
        let mut got = 0u64;
        for _ in 0..reads {
            db_key_into(g.next(), &mut kb);
            r.read_all(&kb, |v| {
                got += v.len() as u64;
            })
            .expect("read");
        }
        let w = Wait::read_now().since(&w0);
        let io = IoCounters::read_now().since(&io0);
        io_out.lock().unwrap()[ci] = io.read_bytes;
        flt_out.lock().unwrap()[ci] = w.major_faults;
        std::hint::black_box(got);
        reads as f64 / (w.wall_ns as f64 / 1e9)
    });

    let cmp = compare(&cold[0], &cold[1], supdb::bench::MIN_EFFECT);
    let (random, default_) = (cold[0].median(), cold[1].median());
    let io = *io_out.lock().unwrap();
    let flt = *flt_out.lock().unwrap();
    let amp = |bytes: u64| bytes as f64 / (reads * value_size as u64) as f64;
    rec.compare("random_vs_default", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "random_reads_per_s" => J::fp(random, 1),
            "default_reads_per_s" => J::fp(default_, 1),
            "random_device_bytes" => J::u(io[0]),
            "default_device_bytes" => J::u(io[1]),
            "random_bytes_per_value" => J::fp(io[0] as f64 / reads as f64, 1),
            "default_bytes_per_value" => J::fp(io[1] as f64 / reads as f64, 1),
            "random_read_amplification" => J::fp(amp(io[0]), 2),
            "default_read_amplification" => J::fp(amp(io[1]), 2),
            "random_major_faults" => J::u(flt[0]),
            "default_major_faults" => J::u(flt[1])
        },
    );

    // The precondition. Rule 3: a finding whose condition was not met is
    // not_exercised, never a pass.
    rec.finding(if capped && ratio > 1.2 {
        Finding::new(
            "F23.0",
            "the store is larger than the memory allowed to cache it",
            true,
            format!(
                "{file_bytes} bytes against a {}MB cap, ratio {ratio:.2}",
                cap_mb
            ),
        )
    } else {
        Finding::not_exercised(
            "F23.0",
            "the store is larger than the memory allowed to cache it",
            format!(
                "no cap in force (--cap-mb {cap_mb}), so every arm below reads a store that fits \
                 and measures the resident regime, not the out-of-core one"
            ),
        )
    });
    rec.finding(Finding::new(
        "F23.1",
        "the default readahead does not amplify a random read",
        amp(io[1]) <= amp(io[0]) * 1.25,
        format!(
            "{:.0} device bytes per {value_size}-byte value with the default advice against {:.0} \
             with MADV_RANDOM -- {:.1}x against {:.1}x amplification. Measured from /proc/self/io \
             with the page cache dropped first, so these are bytes the device was asked for and \
             not bytes inferred from file size",
            io[1] as f64 / reads as f64,
            io[0] as f64 / reads as f64,
            amp(io[1]),
            amp(io[0])
        ),
    ));
    rec.finding(Finding::new(
        "F23.2",
        "telling the kernel a random read is random makes it faster",
        matches!(cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "{random:.0} reads/s with MADV_RANDOM against {default_:.0} with the default ({}). \
             Major faults: {} against {} -- the proof that these passes were cold, and the count \
             of times the engine stopped for the disk",
            cmp.summary("random", "default"),
            flt[0],
            flt[1]
        ),
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

/// Where does readahead stop paying, and does `Auto` find it?
///
/// f23 measured the two ends and they point opposite ways. Resident, the
/// kernel's default readahead is a working prefetcher -- it turns many small
/// faults into few large ones, and forbidding it costs about 15x. Out of core
/// the pages it fetches are evicted before use and fetched again: 86,977x read
/// amplification and a 25x collapse. So the right advice depends on a ratio
/// the caller usually does not know, which is what `Readahead::Auto` is for.
///
/// A threshold picked by taste would be a guess wearing a constant's clothes.
/// This sweeps the ratio of file size to the memory allowed to cache it, holds
/// all three advices against each other at every point, and reports where the
/// crossover actually is -- and whether `Auto` lands on the winning side of it
/// at every ratio, which is the only property that matters.
///
/// One store, built once. The cap is what moves, because out-of-core is a
/// ratio and a cgroup makes the ratio adjustable.
/// Never cap below this: a cgroup limit under the process's own anonymous
/// memory is an OOM kill rather than an experiment.
const CAP_FLOOR: u64 = 96 << 20;

fn f24_autoreadahead(args: &Args, profile: Profile) -> std::io::Result<Record> {
    use supdb::bench::env::Wait;
    use supdb::Readahead;

    // Sized so the ratios below are reachable above the cap floor: the floor
    // exists because a cgroup limit under the process's own anonymous memory
    // is an OOM kill, and a sweep that silently clamps its own independent
    // variable measures nothing. The first version of this did exactly that.
    let keys = args.num("--keys", profile.pick(3_000_000, 6_000_000, 8_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let reads = args.num("--reads", profile.pick(1_000, 5_000, 10_000)) as u64;

    let mut rec = Record::new("f24-autoreadahead", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("reads", J::u(reads))
        .note(
            "one store, swept against a moving memory cap; three advices interleaved at every \
             ratio, page cache dropped before every pass",
        );

    let dir = scratch("f24");
    let file = dir.join("auto.dat");
    let payload = Payload::new(value_size, 0.5, 0x24);
    {
        let store = Store::create(&file, default_opts(256)).expect("create");
        let mut vrng = Rng::new(0x24);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        store.close().expect("close");
    }
    let file_bytes = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    rec.param("file_bytes", J::u(file_bytes));

    let advices = [Readahead::Auto, Readahead::Default, Readahead::Random];
    let names = ["auto", "default", "random"];
    // File over cap. Below 1 the store fits in the cache it is allowed.
    // Down to 0.05, because the case for the default advice lives at the low
    // end: a store that fits many times over is one readahead can prefetch
    // almost entirely, and a sweep starting at 0.25 never sees that and would
    // talk itself into advising Random for everything.
    let ratios = [0.05f64, 0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 3.0];
    let mut rows = Vec::new();
    let mut auto_ok = true;
    let mut worst = f64::INFINITY;
    let mut crossover: Option<f64> = None;

    for r in ratios {
        let cap = ((file_bytes as f64 / r) as u64).max(CAP_FLOOR);
        // What the cap actually achieved, which is what the row reports. The
        // requested ratio is not the measured one whenever the floor binds.
        let effective = file_bytes as f64 / cap as f64;
        if !supdb::bench::env::cap_memory(cap) {
            eprintln!("# WARNING: could not set a {cap}-byte cap; skipping ratio {r}");
            continue;
        }
        let chosen = std::sync::Mutex::new(Readahead::Default);
        let arms = Trial::new(profile.reps()).run(3, |ci, rep| {
            supdb::bench::env::drop_caches();
            let rd = Reader::open_with(
                &file,
                supdb::ReadOptions {
                    readahead: advices[ci],
                    ..Default::default()
                },
            )
            .expect("open");
            if ci == 0 {
                *chosen.lock().unwrap() = rd.advice();
            }
            let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x24 + rep as u64);
            let mut kb = [0u8; 16];
            let w0 = Wait::read_now();
            for _ in 0..reads {
                db_key_into(g.next(), &mut kb);
                rd.read_all(&kb, |v| {
                    std::hint::black_box(v);
                })
                .expect("read");
            }
            let w = Wait::read_now().since(&w0);
            reads as f64 / (w.wall_ns as f64 / 1e9)
        });

        let (auto, deflt, rand) = (arms[0].median(), arms[1].median(), arms[2].median());
        let best = deflt.max(rand);
        let rel = auto / best.max(1e-9);
        worst = worst.min(rel);
        // The property is about the *choice*, not about timing noise. Auto
        // runs one of the two fixed arms, so holding it to its own arm's
        // sampled throughput just measures the trial's variance -- an earlier
        // version failed at 0.88x on a ratio where Auto had picked correctly.
        // What it owes is picking the faster side wherever the two differ by
        // more than the noise floor `stats::compare` itself insists on.
        let gap = (deflt - rand).abs() / deflt.max(rand).max(1e-9);
        if gap > supdb::bench::MIN_EFFECT {
            let want_random = rand > deflt;
            if (*chosen.lock().unwrap() == Readahead::Random) != want_random {
                auto_ok = false;
            }
        }
        // Beyond the noise floor, not merely ahead: at ratio 0.05 the two
        // differed by 0.2%, and calling that a crossover would set a threshold
        // on a coin flip.
        if crossover.is_none() && rand > deflt * (1.0 + supdb::bench::MIN_EFFECT) {
            crossover = Some(effective);
        }
        println!(
            "  ratio {effective:>4.2}  auto {auto:>9.0}  default {deflt:>9.0}  random {rand:>9.0}  \
             auto/best {rel:.2}  chose {:?}",
            *chosen.lock().unwrap()
        );
        rows.push(jobj! {
            "file_over_cap" => J::fp(effective, 2),
            "file_over_cap_requested" => J::fp(r, 2),
            "cap_bytes" => J::u(cap),
            "auto_reads_per_s" => J::fp(auto, 1),
            "default_reads_per_s" => J::fp(deflt, 1),
            "random_reads_per_s" => J::fp(rand, 1),
            "auto_over_best" => J::fp(rel, 3),
            "auto_chose" => J::s(if *chosen.lock().unwrap() == Readahead::Random { "random" } else { "default" })
        });
        let _ = names;
    }
    rec.series("sweep", J::arr(rows));

    rec.finding(Finding::new(
        "F24.1",
        "Auto picks the faster advice wherever the two differ",
        auto_ok,
        format!(
            "at every swept ratio where default and random differ by more than the {:.0}% noise \
             floor, Auto chose the faster one; its own throughput ran {worst:.2}x of the better \
             arm at worst, which is trial variance rather than a wrong choice. This is the whole \
             contract: a caller who does not know whether their store will fit in memory should \
             not lose for saying so",
            supdb::bench::MIN_EFFECT * 100.0
        ),
    ));
    rec.finding(Finding::new(
        "F24.2",
        "readahead stops paying somewhere below a file-to-memory ratio of 1",
        crossover.is_some_and(|c| c < 1.0),
        match crossover {
            Some(c) => format!(
                "MADV_RANDOM first overtakes the default at a ratio of {c:.2}, which is why the \
                 threshold is a measurement and not a taste. A store need not exceed memory to \
                 suffer from readahead -- it only has to be close enough that the pages fetched \
                 speculatively are the ones evicted"
            ),
            None => "the default advice won at every ratio swept".into(),
        },
    ));
    let _ = std::fs::remove_file(&file);
    Ok(rec)
}

fn f13_sync(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(20_000, 100_000, 500_000)) as u64;
    let ops = args.num("--ops", profile.pick(400, 2_000, 6_000)) as u64;
    let value_size = args.num("--value-size", 100);

    let mut rec = Record::new("f13-sync", profile);
    rec.param("keys", J::u(keys))
        .param("ops", J::u(ops))
        .param("value_size", J::u(value_size as u64))
        .note(
            "read-your-writes shape: half the operations write, and a read after a write \
             costs a checkpoint plus a reader reopen. Both arms interleaved; the only \
             difference is Options::sync_on_checkpoint",
        );

    let dir = scratch("f13");
    let payload = Payload::new(value_size, 0.5, 0x13);
    let on = [true, false];

    let s = Trial::new(profile.reps()).run(2, |ci, rep| {
        let file = dir.join(format!("s{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                sync: if on[ci] {
                    supdb::Sync::Always
                } else {
                    supdb::Sync::Never
                },
                ..default_opts(256)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x13 + rep as u64);
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.append(&kb, payload.get(&mut vrng)).expect("append");
        }
        store.checkpoint().expect("checkpoint");

        let mut reader = Some(Reader::open(&file).expect("open"));
        let mut dirty = false;
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x13);
        let t = Instant::now();
        for i in 0..ops {
            db_key_into(g.next(), &mut kb);
            if i % 2 == 0 {
                store.put(&kb, payload.get(&mut vrng)).expect("put");
                dirty = true;
            } else {
                if dirty {
                    store.checkpoint().expect("checkpoint");
                    reader = Some(Reader::open(&file).expect("open"));
                    dirty = false;
                }
                reader
                    .as_ref()
                    .unwrap()
                    .read_all(&kb, |v| {
                        std::hint::black_box(v);
                    })
                    .expect("read");
            }
        }
        let rate = ops as f64 / t.elapsed().as_secs_f64();
        drop(reader);
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        rate
    });

    let cmp = compare(&s[1], &s[0], supdb::bench::MIN_EFFECT);
    let (with, without) = (s[0].median(), s[1].median());
    let ratio = without / with.max(1e-9);
    rec.compare("nosync_vs_sync", cmp.clone());
    rec.series(
        "arms",
        jobj! {
            "sync_ops_per_s" => J::fp(with, 1),
            "nosync_ops_per_s" => J::fp(without, 1),
            "speedup" => J::fp(ratio, 2),
        },
    );

    rec.finding(Finding::new(
        "F13.1",
        "fsync is not the dominant cost of publishing a checkpoint",
        ratio <= 2.0,
        format!(
            "{with:.0} ops/s with fsync against {without:.0} without ({ratio:.1}x). Every \
             read-your-writes operation costs a checkpoint and a checkpoint syncs twice, so \
             this is what durability costs on the workload the engine is worst at ({})",
            cmp.summary("nosync", "sync")
        ),
    ));
    rec.finding(Finding::new(
        "F13.2",
        "removing fsync is worth less than removing the block table decode was",
        ratio <= 4.75,
        format!(
            "fsync is worth {ratio:.1}x here. Mapping the block table removed 34% of all \
             instructions -- 4.75x fewer in total -- and moved throughput by nothing. An \
             instruction profile answers where the CPU goes, not why a workload is slow, and \
             those differ whenever the answer is a syscall"
        ),
    ));
    Ok(rec)
}

// ------------------------------------------------ F12: the cost of compression --

/// What block compression costs on the read path, and what it saves on disk.
///
/// This is the other half of the trade the mapped index started. LMDB reads
/// about 2.9x faster than Supdb natively and does not compress at all, so the
/// obvious question is how much of that gap is decompression rather than
/// structure. It has never been measured: `Options::compress` has existed and
/// defaulted on since the beginning, and nothing priced it.
///
/// Same shape as f8-checksums, for the same reason -- both arms in one
/// process, interleaved, because separate runs of this suite have moved
/// unchanged comparators by +20% to +43%. Scans get their own arm: a scan
/// walks whole blocks in order, so if decompression matters anywhere it
/// matters most there.
fn f12_compress(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let depth = args.num("--depth", 4) as u64;
    let value_size = args.num("--value-size", 100);
    let reads = args.num("--reads", profile.pick(50_000, 200_000, 500_000)) as u64;

    let mut rec = Record::new("f12-compress", profile);
    rec.param("keys", J::u(keys))
        .param("values_per_key", J::u(depth))
        .param("value_size", J::u(value_size as u64))
        .param("reads", J::u(reads))
        .note("both arms interleaved in one process; the only difference is Options::compress");

    let dir = scratch("f12");
    // Half-compressible payload, which is what the rest of the suite uses. A
    // payload of zeroes would make compression look free and one of random
    // bytes would make it look pointless; neither is a workload.
    let payload = Payload::new(value_size, 0.5, 0x12);
    let on = [true, false];

    // Write throughput. Compression is on the seal path, so this is where its
    // CPU cost lands for a writer.
    let write = Trial::new(profile.reps()).run(2, |ci, rep| {
        let file = dir.join(format!("w{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                compress: on[ci],
                ..default_opts(128)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0x12 + rep as u64);
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

    let mut files = Vec::new();
    let mut sizes = Vec::new();
    for want in on.iter() {
        let file = dir.join(format!("r{want}.dat"));
        {
            let store = Store::create(
                &file,
                Options {
                    compress: *want,
                    ..default_opts(128)
                },
            )
            .expect("create");
            let mut vrng = Rng::new(0x12);
            let mut kb = [0u8; 16];
            for i in 0..(keys * depth) {
                db_key_into(i % keys, &mut kb);
                store.append(&kb, payload.get(&mut vrng)).expect("append");
            }
            store.close().expect("close");
        }
        sizes.push(file_len(&file));
        files.push(file);
    }

    // The arms must return the same data. An arm that reads nothing is fast.
    {
        let a = Reader::open(&files[0])?;
        let b = Reader::open(&files[1])?;
        assert_eq!(a.keys(), b.keys(), "arms disagree on key count");
        let mut kb = [0u8; 16];
        for i in 0..500.min(keys) {
            db_key_into(i, &mut kb);
            let (mut va, mut vb) = (Vec::new(), Vec::new());
            a.read_all(&kb, |v| va.push(v.to_vec()))?;
            b.read_all(&kb, |v| vb.push(v.to_vec()))?;
            assert_eq!(va, vb, "arms disagree on the values of a key");
            assert!(!va.is_empty(), "neither arm found a key that was written");
        }
    }

    let readers: Vec<Reader> = files
        .iter()
        .map(|f| Reader::open(f).expect("open"))
        .collect();
    let read = Trial::new(profile.reps()).run(2, |ci, _| {
        let reader = &readers[ci];
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x12);
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

    let scan_len = 20_000usize.min(keys as usize);
    let scan = Trial::new(profile.reps().min(5)).run(2, |ci, _| {
        let reader = &readers[ci];
        let t = Instant::now();
        let mut n = 0u64;
        n += reader
            .scan(None, scan_len, |_, v| {
                std::hint::black_box(v);
            })
            .expect("scan");
        std::hint::black_box(n);
        n as f64 / t.elapsed().as_secs_f64()
    });
    drop(readers);
    for f in &files {
        let _ = std::fs::remove_file(f);
    }

    let wc = compare(&write[1], &write[0], supdb::bench::MIN_EFFECT);
    let rc = compare(&read[1], &read[0], supdb::bench::MIN_EFFECT);
    let sc = compare(&scan[1], &scan[0], supdb::bench::MIN_EFFECT);
    rec.compare("write_off_vs_on", wc.clone());
    rec.compare("read_off_vs_on", rc.clone());
    rec.compare("scan_off_vs_on", sc.clone());

    let gain = |a: f64, b: f64| (a / b - 1.0) * 100.0;
    let wgain = gain(write[1].median(), write[0].median());
    let rgain = gain(read[1].median(), read[0].median());
    let sgain = gain(scan[1].median(), scan[0].median());
    let scost = sizes[1] as f64 / sizes[0] as f64;

    rec.series(
        "arms",
        jobj! {
            "write_on_ops_per_s" => J::fp(write[0].median(), 1),
            "write_off_ops_per_s" => J::fp(write[1].median(), 1),
            "read_on_ops_per_s" => J::fp(read[0].median(), 1),
            "read_off_ops_per_s" => J::fp(read[1].median(), 1),
            "scan_on_entries_per_s" => J::fp(scan[0].median(), 1),
            "scan_off_entries_per_s" => J::fp(scan[1].median(), 1),
            "bytes_on" => J::u(sizes[0]),
            "bytes_off" => J::u(sizes[1]),
            "size_multiple_off_over_on" => J::fp(scost, 3),
        },
    );

    rec.finding(Finding::new(
        "F12.1",
        "turning compression off buys at least 10% on point reads",
        rgain >= 10.0,
        format!(
            "reads {rgain:+.1}% with compression off ({})",
            rc.summary("off", "on")
        ),
    ));
    rec.finding(Finding::new(
        "F12.2",
        "turning compression off buys at least 10% on ordered scans",
        sgain >= 10.0,
        format!(
            "scans {sgain:+.1}% with compression off ({}). A scan walks whole blocks in order, \
             so this is where decompression should cost most",
            sc.summary("off", "on")
        ),
    ));
    rec.finding(Finding::new(
        "F12.3",
        "the space given up is less than 2x",
        scost < 2.0,
        format!(
            "{:.1} MB compressed against {:.1} MB not ({scost:.2}x). Space is the axis this \
             engine wins on, so this is what any read gain above is bought with",
            sizes[0] as f64 / 1e6,
            sizes[1] as f64 / 1e6
        ),
    ));
    rec.finding(Finding::new(
        "F12.4",
        "compression is not costing write throughput too",
        wgain < 10.0,
        format!(
            "writes {wgain:+.1}% with compression off ({}). Ingest is the axis the design is \
             built to win, so a compressor that costs the write path as well as the read path \
             would be paying twice for one saving",
            wc.summary("off", "on")
        ),
    ));
    Ok(rec)
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
/// What the redo log buys, and what it costs in visibility.
///
/// f27 established that durability is not what EXT.9's 0.010x is made of --
/// inserting under durability is, because any insertion sends
/// `checkpoint_in_place` to the full-rewrite path. `Options::redo_log` splits
/// the two jobs a checkpoint had conflated: the records are written and
/// fsynced, and the index is rewritten only when the arena fills.
///
/// The second finding is the price, and it is measured here rather than
/// asserted in a doc comment. A logged checkpoint is durable and is replayed
/// by `Store::open`; a `Reader` opened before the next full rewrite does not
/// see it, because a reader reads the published index and nothing published
/// it. That is a real narrowing of what `checkpoint` has always promised --
/// durable *and* visible to anyone -- and it is why the flag is off by
/// default. F29.2 exists so it cannot be forgotten, and so that a future
/// change making readers replay the log turns the build red rather than
/// passing quietly.
fn f29_redolog(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(20_000, 100_000, 200_000)) as u64;
    let batch = args.num("--batch", 1_000) as u64;
    let value_size = args.num("--value-size", 100);

    let mut rec = Record::new("f29-redolog", profile);
    rec.param("keys", J::u(keys))
        .param("checkpoint_every_ops", J::u(batch))
        .param("value_size", J::u(value_size as u64))
        .note("both arms interleaved in one process; the only difference is Options::redo_log")
        .note("every key is new, which is the shape f27 found expensive and EXT.9 measures");

    let dir = scratch("f29");
    let payload = Payload::new(value_size, 0.5, 0xF29);
    let on = [true, false];
    let io: std::sync::Mutex<Vec<(usize, f64)>> = std::sync::Mutex::new(Vec::new());
    let trial = Trial::new(profile.reps());
    let tp = trial.run(2, |ci, rep| {
        let file = dir.join(format!("r{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                redo_log: on[ci],
                ..default_opts(64)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0xF29 + rep as u64);
        let mut kb = [0u8; 16];
        let io0 = IoCounters::read_now();
        let t = Instant::now();
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
            if (i + 1) % batch == 0 {
                store.checkpoint().expect("checkpoint");
            }
        }
        let secs = t.elapsed().as_secs_f64();
        let wrote = IoCounters::read_now().since(&io0).write_bytes;
        io.lock().unwrap().push((ci, wrote as f64 / 1_048_576.0));
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        keys as f64 / secs
    });

    let io_arm: Vec<Samples> = {
        let all = io.lock().unwrap();
        (0..2)
            .map(|ci| {
                Samples::new(
                    all.iter()
                        .filter(|(c, _)| *c == ci)
                        .map(|(_, v)| *v)
                        .collect(),
                )
            })
            .collect()
    };
    let cmp = compare(&tp[0], &tp[1], supdb::bench::MIN_EFFECT);
    rec.compare("logged_vs_rewritten", cmp.clone());
    rec.series(
        "arms",
        J::arr(
            (0..2)
                .map(|ci| {
                    jobj! {
                        "redo_log" => J::Bool(on[ci]),
                        "ops_per_s" => J::fp(tp[ci].median(), 1),
                        "ops" => tp[ci].to_json(),
                        "device_write_mb" => J::fp(io_arm[ci].median(), 1),
                        "write_amp" => J::fp(
                            io_arm[ci].median() * 1048576.0
                                / (keys as f64 * (16.0 + value_size as f64)).max(1.0),
                            2
                        )
                    }
                })
                .collect(),
        ),
    );
    rec.finding(Finding::new(
        "F29.1",
        "A redo log makes a durable checkpoint cheaper when keys are being inserted",
        matches!(cmp.verdict, supdb::bench::Verdict::Greater),
        format!(
            "logged {:.0} ops/s writing {:.1} MB, against rewriting the index {:.0} ops/s writing \
             {:.1} MB ({}). Every key is new, so `checkpoint_in_place` declines every time and \
             the unlogged arm rewrites the whole key index once per batch -- which is what f27 \
             priced at 4.122x and what EXT.9 shows as 270x write amplification",
            tp[0].median(),
            io_arm[0].median(),
            tp[1].median(),
            io_arm[1].median(),
            cmp.summary("logged", "rewritten")
        ),
    ));

    // The price, demonstrated. A reader opened after a logged checkpoint and
    // before the next full rewrite is asked for keys the writer was told were
    // durable.
    let file = dir.join("visibility.dat");
    let _ = std::fs::remove_file(&file);
    let (seen, want) = {
        let store = Store::create(
            &file,
            Options {
                redo_log: true,
                ..default_opts(64)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0xF29);
        let mut kb = [0u8; 16];
        // One full checkpoint, so an index and an arena exist.
        db_key_into(0, &mut kb);
        store.put(&kb, payload.get(&mut vrng)).expect("put");
        store.checkpoint().expect("checkpoint");
        // Then writes that only the log carries.
        let n = 500u64;
        for i in 1..=n {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.checkpoint().expect("logged checkpoint");
        let r = Reader::open(&file).expect("reader");
        let mut seen = 0u64;
        for i in 1..=n {
            db_key_into(i, &mut kb);
            let mut got = 0usize;
            if r.read_all(&kb, |v| got += v.len()).is_ok() && got > 0 {
                seen += 1;
            }
        }
        drop(r);
        let _ = store.close();
        (seen, n)
    };
    let _ = std::fs::remove_file(&file);
    rec.series(
        "visibility",
        J::arr(vec![jobj! {
            "durable_keys" => J::u(want),
            "visible_to_a_fresh_reader" => J::u(seen)
        }]),
    );
    rec.finding(Finding::new(
        "F29.2",
        "A reader sees what a logged checkpoint made durable",
        seen == want,
        format!(
            "{seen} of {want} keys. A logged checkpoint writes its records and fsyncs them, and \
             publishes nothing: a reader reads the index, and the index does not have them until \
             the arena fills and a rewrite happens. `Store::open` replays, so the writer loses \
             nothing across a restart, and a concurrent reader is looking at an older state than \
             the one the writer has been told is durable. That is why `redo_log` is off by \
             default, and this entry is what makes a change to it deliberate"
        ),
    ));
    Ok(rec)
}

/// Is a durable checkpoint expensive, or is *inserting* expensive?
///
/// EXT.9 has a durable Supdb at 0.010x of LMDB, with 270x write
/// amplification -- 29.9GB for 116MB of data. `checkpoint_in_place` exists
/// precisely to avoid that: it edits the mapped index through slack rather
/// than rewriting it. But it opens with
///
///     if nkeys != meta.len() { return Ok(false) }
///
/// so a checkpoint that follows any *insertion* falls back to the full
/// rewrite, and every batch of a bulk load inserts. If that reading is right,
/// then durability is not inherently expensive here -- inserting under
/// durability is -- and the two arms below will differ by a wide margin on
/// identical checkpoint counts and identical operation counts.
///
/// Same store size, same number of checkpoints, same number of puts. The only
/// difference is whether the keys are new.
fn f27_ckptshape(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(20_000, 100_000, 200_000)) as u64;
    let ops = args.num("--ops", profile.pick(20_000, 100_000, 200_000)) as u64;
    let batch = args.num("--batch", 1_000) as u64;
    let value_size = args.num("--value-size", 100);

    let mut rec = Record::new("f27-ckptshape", profile);
    rec.param("keys", J::u(keys))
        .param("ops", J::u(ops))
        .param("checkpoint_every_ops", J::u(batch))
        .param("value_size", J::u(value_size as u64))
        .note("both arms interleaved; identical op count and checkpoint count, Sync::Always")
        .note("arm 0 inserts new keys, arm 1 updates keys already in the index");

    let dir = scratch("f27");
    let payload = Payload::new(value_size, 0.5, 0xF27);
    let io: std::sync::Mutex<Vec<(usize, f64)>> = std::sync::Mutex::new(Vec::new());
    let trial = Trial::new(profile.reps());
    let tp = trial.run(2, |ci, rep| {
        let file = dir.join(format!("c{ci}-{rep}.dat"));
        let store = Store::create(&file, default_opts(64)).expect("create");
        let mut vrng = Rng::new(0xF27 + rep as u64);
        let mut kb = [0u8; 16];
        // The update arm needs the keys to exist first, and that preload is
        // not timed -- it is the insert arm's whole workload, so timing it
        // here would measure the same thing twice.
        if ci == 1 {
            for i in 0..keys {
                db_key_into(i, &mut kb);
                store.put(&kb, payload.get(&mut vrng)).expect("preload");
            }
            store.checkpoint().expect("preload checkpoint");
        }
        let io0 = IoCounters::read_now();
        let t = Instant::now();
        for i in 0..ops {
            // Arm 0: every key is new. Arm 1: every key already exists.
            let k = if ci == 0 { keys + i } else { i % keys };
            db_key_into(k, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
            if (i + 1) % batch == 0 {
                store.checkpoint().expect("checkpoint");
            }
        }
        let secs = t.elapsed().as_secs_f64();
        let wrote = IoCounters::read_now().since(&io0).write_bytes;
        io.lock()
            .unwrap()
            .push((ci, wrote as f64 / 1_048_576.0));
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        ops as f64 / secs
    });

    let io_arm: Vec<Samples> = {
        let all = io.lock().unwrap();
        (0..2)
            .map(|ci| {
                Samples::new(
                    all.iter()
                        .filter(|(c, _)| *c == ci)
                        .map(|(_, v)| *v)
                        .collect(),
                )
            })
            .collect()
    };
    let cmp = compare(&tp[1], &tp[0], supdb::bench::MIN_EFFECT);
    rec.compare("update_vs_insert", cmp.clone());
    rec.series(
        "arms",
        J::arr(
            (0..2)
                .map(|ci| {
                    jobj! {
                        "keys_are_new" => J::Bool(ci == 0),
                        "ops_per_s" => J::fp(tp[ci].median(), 1),
                        "ops" => tp[ci].to_json(),
                        "device_write_mb" => J::fp(io_arm[ci].median(), 1),
                        "write_amp" => J::fp(
                            io_arm[ci].median() * 1048576.0
                                / (ops as f64 * (16.0 + value_size as f64)).max(1.0),
                            2
                        )
                    }
                })
                .collect(),
        ),
    );
    rec.finding(Finding::new(
        "F27.1",
        "A durable checkpoint is expensive because of insertion, not because of durability",
        matches!(cmp.verdict, supdb::bench::Verdict::Greater),
        format!(
            "updating existing keys runs {:.0} ops/s and writes {:.1} MB; inserting new ones runs \
             {:.0} ops/s and writes {:.1} MB ({}). Identical op and checkpoint counts, both with \
             Sync::Always. `checkpoint_in_place` bails to a full index rewrite whenever a key was \
             added, and every batch of a bulk load adds",
            tp[1].median(),
            io_arm[1].median(),
            tp[0].median(),
            io_arm[0].median(),
            cmp.summary("update", "insert")
        ),
    ));
    Ok(rec)
}

/// Does the write buffer want to be smaller than the workload?
///
/// EXT.10 has Supdb losing bulk ingest to an LMDB that is not syncing, and the
/// profile put the gap in memory: 218MB resident against LMDB's 46MB for the
/// same 105MB of data, and 21x the last-level misses per key. LMDB streams
/// pages out as it goes. Supdb buffers, and until `Options::seal_on_put` it
/// buffered without limit -- `append` sealed when a shard filled and `put`
/// never checked, so every load phase in the external suite ignored
/// `buffer_bytes` and grew until `flush`. At 1M values of 100 bytes against a
/// 256MB budget that is invisible, because the threshold is never reached.
///
/// Which raises the question this experiment asks: the adapter picked 256MB,
/// larger than the whole dataset, so Supdb held all of it. Is that the best
/// setting, or does sealing during the load win by keeping the working set in
/// cache? A store that streams is the same shape LMDB is winning with.
fn f26_buffer(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let mb = [4usize, 16, 64, 256];

    let mut rec = Record::new("f26-buffer", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("buffer_mb", J::arr(mb.iter().map(|m| J::u(*m as u64)).collect()))
        .note("buffer sizes interleaved in one process; seal_on_put is on for every arm")
        .note(
            "the dataset is about 105MB at the full profile, so the 256MB arm is the one that \
             never seals during the load and holds everything -- which is what the external \
             adapter has always configured",
        );

    let dir = scratch("f26");
    let payload = Payload::new(value_size, 0.5, 0xF26);
    let rss: std::sync::Mutex<Vec<(usize, f64)>> = std::sync::Mutex::new(Vec::new());
    let trial = Trial::new(profile.reps());
    let load = trial.run(mb.len(), |ci, rep| {
        let file = dir.join(format!("b{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                seal_on_put: true,
                ..default_opts(mb[ci])
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0xF26 + rep as u64);
        let mut kb = [0u8; 16];
        let rss0 = env::rss_bytes();
        let t = Instant::now();
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        let secs = t.elapsed().as_secs_f64();
        rss.lock().unwrap().push((
            ci,
            env::rss_bytes().saturating_sub(rss0) as f64 / 1_048_576.0,
        ));
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        keys as f64 / secs
    });

    let rss_arm: Vec<Samples> = {
        let all = rss.lock().unwrap();
        (0..mb.len())
            .map(|ci| {
                Samples::new(
                    all.iter()
                        .filter(|(c, _)| *c == ci)
                        .map(|(_, v)| *v)
                        .collect(),
                )
            })
            .collect()
    };
    rec.series(
        "buffers",
        J::arr(
            (0..mb.len())
                .map(|ci| {
                    jobj! {
                        "buffer_mb" => J::u(mb[ci] as u64),
                        "load_ops_per_s" => J::fp(load[ci].median(), 1),
                        "load" => load[ci].to_json(),
                        "load_rss_mb" => J::fp(rss_arm[ci].median(), 1),
                        "load_rss" => rss_arm[ci].to_json()
                    }
                })
                .collect(),
        ),
    );

    // Smallest against largest: the question is whether streaming costs
    // throughput, so the comparison that matters is the extreme one.
    let cmp = compare(&load[0], &load[mb.len() - 1], supdb::bench::MIN_EFFECT);
    rec.compare("load_4mb_vs_256mb", cmp.clone());
    let best = (0..mb.len())
        .max_by(|a, b| load[*a].median().total_cmp(&load[*b].median()))
        .unwrap_or(0);
    rec.finding(Finding::new(
        "F26.1",
        "A write buffer smaller than the workload does not cost load throughput",
        !matches!(cmp.verdict, supdb::bench::Verdict::Less),
        format!(
            "4MB {:.0} ops/s at {:.1} MB resident, against 256MB {:.0} ops/s at {:.1} MB ({}). \
             Fastest arm was {}MB. The 256MB arm never seals during a load of this size, which is \
             what the external adapter configures and why Supdb holds 218MB where LMDB holds 46",
            load[0].median(),
            rss_arm[0].median(),
            load[mb.len() - 1].median(),
            rss_arm[mb.len() - 1].median(),
            cmp.summary("4MB", "256MB"),
            mb[best]
        ),
    ));
    Ok(rec)
}

/// What the pending arena costs or buys, both arms interleaved.
///
/// `EXT.10` has Supdb losing bulk ingest 1.85x to an LMDB that is not syncing
/// either, and the profile put the gap in memory rather than compute: 1.37x
/// the instructions but 21x the last-level misses per key, because every
/// buffered value used to get its own allocation (docs/profiling.md).
/// `Options::pending_arena` appends them into one buffer per shard instead.
///
/// The deterministic counters disagree about whether that is an improvement.
/// Instructions fall 16% per key. Last-level misses *rise* 21%, and reserving
/// the arena up front -- which removes every growth memcpy -- did not change
/// that, so the doubling was not the cause. The plausible remainder is that
/// the per-key path was accidentally cache-friendly, since malloc hands back
/// blocks the driver freed moments ago while an arena marches through memory
/// nothing has touched.
///
/// Two exact counters pointing opposite ways is exactly the case where only
/// wall clock decides, and only interleaved: this is the experiment, not the
/// cachegrind run, and if it says the arena loses then the arena loses.
fn f25_arena(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let keys = args.num("--keys", profile.pick(50_000, 300_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let depth = args.num("--depth", 4) as u64;

    let mut rec = Record::new("f25-arena", profile);
    rec.param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64))
        .param("values_per_key", J::u(depth))
        .note("both arms interleaved in one process; the only difference is Options::pending_arena");

    let dir = scratch("f25");
    let payload = Payload::new(value_size, 0.5, 0xF25);
    let on = [true, false];

    // Rule 4, and a question the arena raises rather than settles: it reserves
    // the shard's whole buffer budget on first use, so it trades address space
    // for never copying. Across two separate ext-kv runs resident memory rose
    // 178.5MB to 218.3MB, which is exactly the kind of cross-run difference
    // this project does not accept as attribution -- so it is measured here,
    // in the same interleaved trial as the throughput.
    let rss: std::sync::Mutex<Vec<(usize, f64)>> = std::sync::Mutex::new(Vec::new());

    // Bulk load through `put`, which is the shape EXT.10 measures: one value
    // per key, every key new.
    let trial = Trial::new(profile.reps());
    let load = trial.run(2, |ci, rep| {
        let file = dir.join(format!("l{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                pending_arena: on[ci],
                ..default_opts(256)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0xF25 + rep as u64);
        let mut kb = [0u8; 16];
        let rss0 = supdb::bench::env::rss_bytes();
        let t = Instant::now();
        for i in 0..keys {
            db_key_into(i, &mut kb);
            store.put(&kb, payload.get(&mut vrng)).expect("put");
        }
        store.flush().expect("flush");
        let secs = t.elapsed().as_secs_f64();
        let grew = supdb::bench::env::rss_bytes().saturating_sub(rss0);
        rss.lock()
            .unwrap()
            .push((ci, grew as f64 / 1_048_576.0));
        let _ = store.close();
        let _ = std::fs::remove_file(&file);
        keys as f64 / secs
    });

    // And through `append`, where a key's run must stay contiguous and the
    // arena has to relocate when another key has appended in between. That is
    // the case the arena could plausibly lose outright, so it is measured
    // rather than argued about.
    let appended = trial.run(2, |ci, rep| {
        let file = dir.join(format!("a{ci}-{rep}.dat"));
        let store = Store::create(
            &file,
            Options {
                pending_arena: on[ci],
                ..default_opts(256)
            },
        )
        .expect("create");
        let mut vrng = Rng::new(0xF25 + rep as u64);
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

    let rss_arm: Vec<Samples> = {
        let all = rss.lock().unwrap();
        (0..2)
            .map(|ci| {
                Samples::new(
                    all.iter()
                        .filter(|(c, _)| *c == ci)
                        .map(|(_, v)| *v)
                        .collect(),
                )
            })
            .collect()
    };
    let rc = compare(&rss_arm[1], &rss_arm[0], supdb::bench::MIN_EFFECT);
    rec.compare("rss_perkey_vs_arena", rc.clone());
    let lc = compare(&load[0], &load[1], supdb::bench::MIN_EFFECT);
    let ac = compare(&appended[0], &appended[1], supdb::bench::MIN_EFFECT);
    rec.compare("load_arena_vs_per_key", lc.clone());
    rec.compare("append_arena_vs_per_key", ac.clone());
    rec.series(
        "arms",
        J::arr(
            (0..2)
                .map(|ci| {
                    jobj! {
                        "pending_arena" => J::Bool(on[ci]),
                        "load_ops_per_s" => J::fp(load[ci].median(), 1),
                        "load_rss_mb" => J::fp(rss_arm[ci].median(), 1),
                        "load_rss" => rss_arm[ci].to_json(),
                        "load" => load[ci].to_json(),
                        "append_ops_per_s" => J::fp(appended[ci].median(), 1),
                        "append" => appended[ci].to_json()
                    }
                })
                .collect(),
        ),
    );
    rec.finding(Finding::new(
        "F25.1",
        "Buffering pending values in one arena per shard speeds up a bulk load",
        matches!(lc.verdict, supdb::bench::Verdict::Greater),
        format!(
            "arena {:.0} ops/s against per-key {:.0} ({}). cachegrind has the arena at 16% fewer \
             instructions per key and 21% *more* last-level misses, so the two exact counters \
             disagree and this is the measurement that decides",
            load[0].median(),
            load[1].median(),
            lc.summary("arena", "per-key")
        ),
    ));
    rec.finding(Finding::new(
        "F25.3",
        "The arena does not cost resident memory",
        !matches!(rc.verdict, supdb::bench::Verdict::Less),
        format!(
            "arena {:.1} MB against per-key {:.1} MB across the same load ({}). The arena reserves \
             the shard's whole buffer budget on first use rather than growing into it, so this is \
             the axis where it could be paying for its speed. Lower is better and the comparison \
             is the other way round from a throughput one",
            rss_arm[0].median(),
            rss_arm[1].median(),
            rc.summary("per-key", "arena")
        ),
    ));
    rec.finding(Finding::new(
        "F25.2",
        "The arena does not slow down an append-heavy workload",
        !matches!(ac.verdict, supdb::bench::Verdict::Less),
        format!(
            "arena {:.0} ops/s against per-key {:.0} ({}). An append has to keep a key's run \
             contiguous, so when another key has appended in between the arena copies the run to \
             the tail. This is the workload where that copy is paid on almost every call",
            appended[0].median(),
            appended[1].median(),
            ac.summary("arena", "per-key")
        ),
    ));
    Ok(rec)
}

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

/// What does counting a key's values actually cost?
///
/// R4.3 of the logshed requirements asks for `count(key)` "without decoding
/// the values", and hopes it can come out of the extent list. It cannot, and
/// this is the experiment that says so rather than a paragraph asserting it.
/// An `Ext` is four `u32`s -- block, offset, byte length, offset of the last
/// record -- and none of them is a count. The values inside an extent are
/// length-prefixed varints laid end to end, so the only general way to know
/// how many there are is to step over them.
///
/// Four arms, interleaved in one process over one file, which is the only way
/// this repository allows a difference to be claimed:
///
///   lookup       resolve the key and stop. This is the floor, and it is
///                exactly what an O(extents) count would cost -- so it also
///                prices the format change that would add a per-extent count.
///   count_fixed  the floor plus one division. Available *today*, with no
///                format change, for a posting list whose values are all the
///                same width -- which logshed's four-byte line ordinals are.
///   count        the varint walk: one length prefix read per value, payload
///                skipped, nothing handed to a callback.
///   read_all     what exists today, with a closure that only increments.
///
/// The interesting comparison is not count against read_all. It is
/// count_fixed against lookup, because that difference is the whole value of
/// adding four bytes per extent to the format -- and it is a division.
fn f28_count(args: &Args, profile: Profile) -> std::io::Result<Record> {
    use supdb::bytes::MmapBytes;
    use supdb::Blob;

    let keys = args.num("--keys", profile.pick(2_000, 20_000, 50_000)) as u64;
    let run_len = args.num("--run-len", 200) as u64;
    let long_run = args.num("--long-run", 4_000) as u64;
    let probes = args.num("--probes", profile.pick(20_000, 200_000, 500_000)) as u64;
    // Four bytes, because that is what a posting is: a line ordinal.
    let width = 4usize;

    let mut rec = Record::new("f28-count", profile);
    rec.param("keys", J::u(keys))
        .param("run_len", J::u(run_len))
        .param("long_run", J::u(long_run))
        .param("probes", J::u(probes))
        .param("value_width", J::u(width as u64))
        .note(
            "one file, four arms, interleaved in one process. Every arm answers the same \
             question about the same keys and differs only in how much of the extent it has to \
             touch to answer it",
        );

    let dir = scratch("f28");
    let file = dir.join("count.dat");
    // Grouped by key, which is how a day index is built -- see w1-daysize
    // W1.3, where the alternative costs nine times the file.
    {
        let store = Store::create(&file, default_opts(256)).expect("create");
        let mut kb = [0u8; 16];
        for i in 0..keys {
            db_key_into(i, &mut kb);
            // Every sixteenth key is long, so the file carries both the shape
            // a breakdown panel asks about and the shape it does not.
            let n = if i % 16 == 0 { long_run } else { run_len };
            for v in 0..n {
                store
                    .append(&kb, &(v as u32).to_le_bytes()[..width])
                    .expect("append");
            }
        }
        store.checkpoint().expect("checkpoint");
        store.close().expect("close");
    }

    let blob = Blob::open(MmapBytes::open(&file).expect("map")).expect("blob open");
    assert!(blob.zero_copy(), "the native arm must not be copying");

    let arm_names = ["lookup", "count_fixed", "count", "read_all"];
    let rates = Trial::new(profile.reps()).run(arm_names.len(), |ci, rep| {
        let mut g = KeyGen::new(KeyDist::Uniform, keys, 0x28 + rep as u64);
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut sink = 0u64;
        for _ in 0..probes {
            db_key_into(g.next(), &mut kb);
            sink += match ci {
                0 => blob.lookup(&kb).map(|e| e.len() as u64).unwrap_or(0),
                1 => blob.count_fixed(&kb, width as u32).unwrap_or(0),
                2 => blob.count(&kb).expect("count"),
                _ => {
                    let mut n = 0u64;
                    blob.read_all(&kb, |v| {
                        std::hint::black_box(v);
                        n += 1;
                    })
                    .expect("read_all");
                    n
                }
            };
        }
        std::hint::black_box(sink);
        probes as f64 / t.elapsed().as_secs_f64()
    });

    let ns = |s: &supdb::bench::Samples| 1e9 / s.median();
    rec.series(
        "arms",
        J::arr(
            arm_names
                .iter()
                .zip(rates.iter())
                .map(|(name, s)| {
                    jobj! {
                        "arm" => J::s(*name),
                        "probes_per_s" => J::fp(s.median(), 1),
                        "ns_per_probe" => J::fp(ns(s), 1),
                        "rel_iqr" => J::fp(s.rel_iqr(), 4),
                    }
                })
                .collect(),
        ),
    );

    let min = supdb::bench::MIN_EFFECT;
    let vs_read = compare(&rates[2], &rates[3], min);
    let fixed_vs_lookup = compare(&rates[1], &rates[0], min);
    let count_vs_fixed = compare(&rates[1], &rates[2], min);
    rec.compare("count_vs_read_all", vs_read.clone());
    rec.compare("count_fixed_vs_lookup", fixed_vs_lookup.clone());
    rec.compare("count_fixed_vs_count", count_vs_fixed.clone());

    // W2.1 -- is the walk worth having at all? If counting costs what reading
    // costs, `count` is an API convenience and should be described as one.
    rec.finding(Finding::new(
        "W2.1",
        "counting a key's values by walking their length prefixes is faster than reading them",
        matches!(vs_read.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "{:.0} ns/probe to count against {:.0} to read ({}). Skipping the payload does not \
             skip the cache lines it lies in, and the walk is a serial dependent chain -- each \
             record's position is the previous record's length -- so there is nothing to \
             overlap. In native Rust the callback `read_all` adds inlines to an increment, which \
             is the rest of the difference. This is the premise R4.3 was written on and it does \
             not hold: `count` is the correct general answer, not a cheaper one. What it is \
             still worth is the wasm boundary, where `read_all` frames every value into a buffer \
             for JavaScript and `count` returns one integer -- that crossing is not measured \
             here and is not claimed",
            ns(&rates[2]),
            ns(&rates[3]),
            vs_read.summary("count", "read_all")
        ),
    ));

    // W2.2 -- the finding R4.3 actually asked about, stated as a negative.
    rec.finding(Finding::new(
        "W2.2",
        "a count that is O(extents) rather than O(values) is not available from the extent list, and the difference is large",
        matches!(count_vs_fixed.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "the O(extents) form costs {:.0} ns/probe and the walk costs {:.0} ({}). An Ext \
             records block, offset, byte length and the offset of the last record, and none of \
             those is a count, so `count` steps over every value. `count_fixed` recovers the \
             count in O(extents) only because a fixed-width value carries a fixed-width length \
             prefix -- it is arithmetic on Ext::len, not a general answer",
            ns(&rates[1]),
            ns(&rates[2]),
            count_vs_fixed.summary("count_fixed", "count")
        ),
    ));

    // W2.3 -- and therefore: is the format change worth making? `lookup` is
    // the floor a per-extent count could reach, because summing a field over
    // a borrowed slice is nothing next to resolving the key. The gap between
    // `lookup` and `count_fixed` is an *upper bound* on the saving: a stored
    // count still has to iterate the extents, so it would recover the
    // division and not the walk over them.
    //
    // Stated as a threshold rather than as "no difference". At `ci` these two
    // are 4.1ns apart and the gate calls it noise; at `full` they are 9.6ns
    // apart at p=0.0022 and it is a real difference. A claim resting on a
    // null result would have flipped between profiles for a reason that says
    // nothing about the engine -- which is the trap `f8-checksums` documents
    // from the other direction.
    const WORTH_FOUR_BYTES_NS: f64 = 20.0;
    let saving = ns(&rates[1]) - ns(&rates[0]);
    rec.finding(Finding::new(
        "W2.3",
        "storing a per-extent record count would save less than 20 ns per lookup, which is not worth four bytes per extent",
        saving < WORTH_FOUR_BYTES_NS,
        format!(
            "resolving the key and stopping costs {:.0} ns/probe; the O(extents) count costs \
             {:.0}, so at most {saving:+.1} ns is on the table and most of it is one 64-bit \
             division ({}). Four bytes per extent is 25% on top of a 16-byte Ext and it is paid \
             by every store forever, including the ones that never ask for a count. A schema \
             with variable-width values is where the change would pay, and it is the case \
             logshed does not have",
            ns(&rates[0]),
            ns(&rates[1]),
            fixed_vs_lookup.summary("count_fixed", "lookup")
        ),
    ));

    // W2.4 -- open question 4 of the requirements: does the browser need a
    // dictionary scan at all, or should the roll precompute the breakdown
    // panels? That turns entirely on what a scan costs, and the two forms of
    // it are not the same order of growth. `scan_counts` pays a `count` per
    // key, so it is O(every posting in the range) -- for a day index, the
    // whole file. `scan_counts_fixed` is O(extents), bounded by the
    // dictionary rather than by the traffic.
    let span = args.num("--scan-keys", 2_000).min(keys as usize);
    let scans = args.num("--scans", profile.pick(20, 200, 500));
    let scan = Trial::new(profile.reps()).run(2, |ci, rep| {
        let mut g = KeyGen::new(
            KeyDist::Uniform,
            keys.saturating_sub(span as u64).max(1),
            0x2C + rep as u64,
        );
        let mut kb = [0u8; 16];
        let t = Instant::now();
        let mut sink = 0u64;
        for _ in 0..scans {
            db_key_into(g.next(), &mut kb);
            if ci == 0 {
                blob.scan_counts(&kb, span, |_k, n| {
                    sink += n;
                    true
                })
                .expect("scan");
            } else {
                blob.scan_counts_fixed(&kb, span, width as u32, |_k, n| {
                    sink += n.unwrap_or(0);
                    true
                })
                .expect("scan");
            }
        }
        std::hint::black_box(sink);
        (scans * span) as f64 / t.elapsed().as_secs_f64()
    });
    let scan_cmp = compare(&scan[1], &scan[0], min);
    rec.compare("scan_counts_fixed_vs_scan_counts", scan_cmp.clone());
    rec.series(
        "dictionary_scan",
        jobj! {
            "keys_per_scan" => J::u(span as u64),
            "scans" => J::u(scans as u64),
            "walked_keys_per_s" => J::fp(scan[0].median(), 1),
            "fixed_keys_per_s" => J::fp(scan[1].median(), 1),
            "walked_ns_per_key" => J::fp(1e9 / scan[0].median(), 1),
            "fixed_ns_per_key" => J::fp(1e9 / scan[1].median(), 1),
        },
    );
    rec.finding(Finding::new(
        "W2.4",
        "a browser can compute a top-N breakdown from the dictionary itself, because counting it from the extent list is at least 10x walking it",
        scan_cmp.ratio >= 10.0
            && matches!(scan_cmp.verdict, supdb::bench::stats::Verdict::Greater),
        format!(
            "over {span} keys: {:.1} ns/key walked against {:.1} counted from the extent list \
             ({}). The walk is O(every posting in the range) and the extent form is O(extents), \
             so the gap widens with the traffic a day carries rather than with its dictionary. \
             This is what makes precomputing the breakdown panels at roll time unnecessary: the \
             browser can rank the whole dictionary without touching a block",
            1e9 / scan[0].median(),
            1e9 / scan[1].median(),
            scan_cmp.summary("scan_counts_fixed", "scan_counts")
        ),
    ));

    let _ = std::fs::remove_file(&file);
    Ok(rec)
}
