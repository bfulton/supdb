//! External benchmarks: Supdb entered into other projects' evaluations.
//!
//! The internal suite measures Supdb against itself on workloads chosen here.
//! That is necessary and insufficient: a suite written by the engine's author
//! tests what the author thought to test. These are the shapes the rest of the
//! field is evaluated on, with Supdb added as another entrant.
//!
//!   kv        redb's own benchmark shape -- bulk load, individual and batched
//!             writes, random reads, range scans, removals -- against redb,
//!             LMDB and sled.
//!   ycsb      YCSB core workloads A-F (Cooper et al., SoCC'10), uniform and
//!             Zipfian. The standard no key-value store is taken seriously
//!             without.
//!
//! Two rules make the comparison honest rather than flattering:
//!
//!   * Batch size and value shape are identical for every engine.
//!   * Every result carries each engine's feature score, because Supdb
//!     provides one of six guarantees the others provide five or six of, and
//!     a throughput number that does not say so is comparing promises.

mod engines;

use engines::{Engine, Features, Lmdb, Redb, Sled, Supdb};
use std::path::PathBuf;
use std::time::Instant;
use supdb::bench::{
    compare, db_key_into, Finding, Hist, KeyDist, KeyGen, Payload, Profile, Record, Rng, Samples,
    Verdict, J,
};
use supdb::jobj;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-external-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

struct Args(Vec<String>);
impl Args {
    fn get(&self, n: &str) -> Option<&str> {
        self.0
            .iter()
            .position(|a| a == n)
            .and_then(|i| self.0.get(i + 1))
            .map(|s| s.as_str())
    }
    fn num(&self, n: &str, d: usize) -> usize {
        self.get(n).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

/// Build the field. Supdb first, then the comparators.
fn build(root: &std::path::Path, which: &[&str], buffer_mb: usize) -> Vec<Box<dyn Engine>> {
    let mut out: Vec<Box<dyn Engine>> = Vec::new();
    for name in which {
        let dir = root.join(name);
        let e: Result<Box<dyn Engine>, String> = match *name {
            "supdb" => Supdb::create(&dir, buffer_mb).map(|e| Box::new(e) as Box<dyn Engine>),
            "redb" => Redb::create(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
            "lmdb" => Lmdb::create(&dir, 8).map(|e| Box::new(e) as Box<dyn Engine>),
            "sled" => Sled::create(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
            other => Err(format!("unknown engine {other}")),
        };
        match e {
            Ok(e) => out.push(e),
            // A comparator that will not start is recorded, never skipped
            // silently: an absent engine must not look like an engine that
            // lost.
            Err(err) => eprintln!("# SKIPPED {name}: {err}"),
        }
    }
    out
}

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let args = Args(argv.clone());
    let cmd = argv.get(1).cloned().unwrap_or_else(|| "help".into());
    let profile = Profile::parse(args.get("--profile").unwrap_or("dev")).unwrap_or(Profile::Dev);
    let out = PathBuf::from(args.get("--out").unwrap_or("results"));
    let engines: Vec<&str> = args
        .get("--engines")
        .map(|s| s.split(',').collect())
        .unwrap_or_else(|| vec!["supdb", "redb", "lmdb", "sled"]);

    let rec = match cmd.as_str() {
        "kv" => suite_kv(&args, profile, &engines)?,
        "ycsb" => suite_ycsb(&args, profile, &engines)?,
        "all" => {
            let a = suite_kv(&args, profile, &engines)?;
            a.print_summary();
            a.write(&out)?;
            let b = suite_ycsb(&args, profile, &engines)?;
            b.print_summary();
            b.write(&out)?;
            return Ok(());
        }
        _ => {
            println!(
                "external <kv|ycsb|all> [--profile ci|dev|full] [--engines supdb,redb,lmdb,sled]"
            );
            return Ok(());
        }
    };
    rec.print_summary();
    rec.write(&out)?;
    Ok(())
}


/// redb's benchmark shape, with Supdb added.
///
/// Every engine is measured `reps` times and the engines are interleaved, one
/// round at a time, so a machine that drifts drifts across all of them rather
/// than into one. It used to run each engine exactly once. That is the habit
/// this whole module exists to break, and it showed: EXT.1 read 0.70x, 1.03x,
/// 0.998x, 1.13x and 0.85x across five single runs and flipped between holding
/// and failing on margins as small as 0.2%. An ordering now has to clear
/// `stats::compare` -- a Mann-Whitney U test and a minimum effect size --
/// exactly as an internal experiment does.
fn suite_kv(args: &Args, profile: Profile, which: &[&str]) -> std::io::Result<Record> {
    let n = args.num("--keys", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let batch = args.num("--batch", 1_000);
    let reads = args.num("--reads", profile.pick(20_000, 100_000, 500_000)) as u64;
    let scans = args.num("--scans", profile.pick(200, 2_000, 10_000)) as u64;
    let scan_len = args.num("--scan-len", 100);
    let reps = args.num("--reps", profile.reps());

    let mut rec = Record::new("ext-kv", profile);
    rec.param("keys", J::u(n))
        .param("value_size", J::u(value_size as u64))
        .param("batch", J::u(batch as u64))
        .param("reads", J::u(reads))
        .param("scans", J::u(scans))
        .param("scan_len", J::u(scan_len as u64))
        .param("reps", J::u(reps as u64))
        .note(
            "workload shape follows redb's own benchmark; batch size is identical for every engine",
        )
        .note(
            "engines interleaved round-robin over reps, one warmup round discarded; medians \
             reported, and every ordering gated on stats::compare",
        );

    let payload = Payload::new(value_size, 0.5, 0xE1);
    let ne = which.len();
    let mut load: Vec<Samples> = vec![Samples::default(); ne];
    let mut read: Vec<Samples> = vec![Samples::default(); ne];
    let mut scan: Vec<Samples> = vec![Samples::default(); ne];
    let mut hists: Vec<Hist> = (0..ne).map(|_| Hist::new()).collect();
    let mut size = vec![0f64; ne];
    let mut hit = vec![0f64; ne];
    let mut feats: Vec<Option<Features>> = vec![None; ne];

    // One warmup round, then the measured ones. A fresh file's first touch
    // pays allocation and first-fault costs that no steady state repeats.
    let warmup = 1usize;
    for rep in 0..(warmup + reps) {
        for (ei, name) in which.iter().enumerate() {
            // The rep is in the path deliberately. heed hands back a cached
            // Env for a path it has already opened, so reusing one directory
            // per engine gave LMDB its previous env with its files unlinked
            // underneath it: the directory read as empty, `size_mb` as 0.0,
            // and every rep after the first was loading into a database that
            // already held the data. A fresh path per rep is what makes each
            // rep an independent run.
            let root = scratch(&format!("kv-{name}-{rep}"));
            let Some(mut e) = build(&root, &[name], 256).into_iter().next() else {
                continue;
            };
            feats[ei] = Some(e.features());
            let mut vrng = Rng::new(0xE1);

            // Bulk load.
            let t = Instant::now();
            let mut buf: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(batch);
            let mut kb = [0u8; 16];
            for i in 0..n {
                db_key_into(i, &mut kb);
                buf.push((kb.to_vec(), payload.get(&mut vrng).to_vec()));
                if buf.len() == batch {
                    e.write_batch(&buf).expect("write");
                    buf.clear();
                }
            }
            if !buf.is_empty() {
                e.write_batch(&buf).expect("write");
            }
            e.sync().expect("sync");
            let load_s = t.elapsed().as_secs_f64();

            // Random reads, with the distribution recorded.
            let mut g = KeyGen::new(KeyDist::Uniform, n, 7);
            let mut h = Hist::new();
            let t = Instant::now();
            let mut hits = 0u64;
            for _ in 0..reads {
                db_key_into(g.next(), &mut kb);
                let t1 = Instant::now();
                let got = e.get(&kb).expect("get");
                h.record(t1.elapsed().as_nanos() as u64);
                if got > 0 {
                    hits += 1;
                }
            }
            let read_s = t.elapsed().as_secs_f64();

            // Range scans.
            let mut g2 = KeyGen::new(
                KeyDist::Uniform,
                n.saturating_sub(scan_len as u64).max(1),
                11,
            );
            let t = Instant::now();
            for _ in 0..scans {
                db_key_into(g2.next(), &mut kb);
                let _ = e.range(&kb, scan_len).expect("range");
            }
            let scan_s = t.elapsed().as_secs_f64();

            if rep >= warmup {
                load[ei].push(n as f64 / load_s);
                read[ei].push(reads as f64 / read_s);
                scan[ei].push(scans as f64 * scan_len as f64 / scan_s);
                hists[ei] = h;
                hit[ei] = hits as f64 / reads as f64;
                size[ei] = e.size_bytes() as f64 / 1048576.0;
            }
            drop(e);
            // Four stores of this size per round filled the disk once already,
            // and every number taken that day had to be thrown away.
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    let mut rows = Vec::new();
    for (ei, name) in which.iter().enumerate() {
        let (Some(f), false) = (feats[ei], load[ei].is_empty()) else {
            continue;
        };
        rows.push(jobj! {
            "engine" => J::s(*name),
            "features" => f.to_json(),
            "feature_score" => J::u(f.score() as u64),
            "load_ops_per_s" => J::fp(load[ei].median(), 1),
            "load" => load[ei].to_json(),
            "read_ops_per_s" => J::fp(read[ei].median(), 1),
            "read" => read[ei].to_json(),
            "read_hit_rate" => J::fp(hit[ei], 4),
            "scan_entries_per_s" => J::fp(scan[ei].median(), 1),
            "scan" => scan[ei].to_json(),
            "read_latency" => hists[ei].to_json(),
            "size_mb" => J::fp(size[ei], 2)
        });
        println!(
            "  {name:6} load {:>9.0}/s  read {:>9.0}/s  scan {:>10.0}/s  {:>7.1} MB  features {}/6",
            load[ei].median(),
            read[ei].median(),
            scan[ei].median(),
            size[ei],
            f.score()
        );
    }
    rec.series("engines", J::arr(rows.clone()));

    let idx = |name: &str| which.iter().position(|w| *w == name);
    let ordering =
        |rec: &mut Record, id: &str, title: &str, other: &str, s: &[Samples], unit: &str| {
            let (Some(si), Some(oi)) = (idx("supdb"), idx(other)) else {
                return;
            };
            if s[si].is_empty() || s[oi].is_empty() {
                return;
            }
            let cmp = compare(&s[si], &s[oi], supdb::bench::MIN_EFFECT);
            let holds = matches!(cmp.verdict, Verdict::Greater);
            rec.compare(&format!("{id}_supdb_vs_{other}"), cmp.clone());
            rec.finding(Finding::new(
                id,
                title,
                holds,
                format!(
                    "supdb {:.0} {unit} vs {other} {:.0} {unit} ({})",
                    s[si].median(),
                    s[oi].median(),
                    cmp.summary("supdb", other)
                ),
            ));
        };

    ordering(
        &mut rec,
        "EXT.1",
        "Supdb loads faster than LMDB, the architecture it is modelled on",
        "lmdb",
        &load,
        "ops/s",
    );
    ordering(
        &mut rec,
        "EXT.2",
        "Supdb reads faster than redb, the closest non-mmap sibling",
        "redb",
        &read,
        "reads/s",
    );
    // The design document's headline read comparison, restated as a claim.
    // Its own figures put Supdb ahead of LMDB on warm reads (330,732/s against
    // 316,557/s on the wide shape) -- but LMDB was measured through a Java
    // harness with an adapter the document later found to allocate per value
    // and open a transaction per lookup. Measured natively, this is the claim.
    ordering(
        &mut rec,
        "EXT.4",
        "Supdb reads faster than LMDB when both are measured natively",
        "lmdb",
        &read,
        "reads/s",
    );
    ordering(
        &mut rec,
        "EXT.5",
        "Supdb scans faster than LMDB when both are measured natively",
        "lmdb",
        &scan,
        "entries/s",
    );
    if let (Some(si), Some(li)) = (idx("supdb"), idx("lmdb")) {
        if !load[si].is_empty() && !load[li].is_empty() {
            let (s, l) = (size[si], size[li]);
            rec.finding(Finding::new(
                "EXT.6",
                "Supdb stores the same data in less space than LMDB",
                s < l,
                format!(
                    "supdb {s:.1} MB vs lmdb {l:.1} MB ({:.2}x). Size is the one axis immune to \
                     drift, so it is the one that needs no repetition to be believed",
                    l / s.max(1e-9)
                ),
            ));
        }
    }
    rec.note(
        "feature_score counts durable commit, transactions, checksums, reopen-for-write, \
         read-your-writes and ordered scan. Supdb provides one of six; a throughput comparison \
         against engines providing five or six is comparing promises as much as implementations",
    );
    Ok(rec)
}

/// YCSB core workloads (Cooper et al., SoCC'10).
fn suite_ycsb(args: &Args, profile: Profile, which: &[&str]) -> std::io::Result<Record> {
    let n = args.num("--keys", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let ops = args.num("--ops", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let batch = args.num("--batch", 100);

    // (name, read %, update %, scan %, rmw %, distribution)
    let workloads: &[(&str, u32, u32, u32, u32, KeyDist)] = &[
        ("A-update-heavy", 50, 50, 0, 0, KeyDist::Zipfian),
        ("B-read-heavy", 95, 5, 0, 0, KeyDist::Zipfian),
        ("C-read-only", 100, 0, 0, 0, KeyDist::Zipfian),
        ("D-read-latest", 95, 5, 0, 0, KeyDist::Uniform),
        ("E-scan-short", 0, 5, 95, 0, KeyDist::Zipfian),
        ("F-read-modify-write", 50, 0, 0, 50, KeyDist::Zipfian),
    ];

    let mut rec = Record::new("ext-ycsb", profile);
    rec.param("record_count", J::u(n))
        .param("operation_count", J::u(ops))
        .param("value_size", J::u(value_size as u64))
        .param("batch", J::u(batch as u64))
        .note("YCSB core workloads A-F; Zipfian theta 0.99 as in the original")
        .note(
            "read this against the feature table, not on its own: LMDB commits durably on \
             every batch, and Supdb buffers and publishes without an fsync. That is what \
             durable_commit=false in its Features means and it is why it scores 3/6 against \
             LMDB's 5/6. f13-sync prices the difference at 31x on this shape, so the mixed \
             workloads are a comparison of an engine that promises power-loss durability \
             against one that does not",
        );

    let payload = Payload::new(value_size, 0.5, 0xE2);
    let mut rows = Vec::new();

    for (wname, pread, pupd, pscan, prmw, dist) in workloads {
        let root = scratch(&format!("ycsb-{wname}"));
        for e in build(&root, which, 256).iter_mut() {
            let name = e.name().to_string();
            let mut vrng = Rng::new(0xE2);
            let mut kb = [0u8; 16];

            // Load phase.
            let mut buf = Vec::with_capacity(batch);
            for i in 0..n {
                db_key_into(i, &mut kb);
                buf.push((kb.to_vec(), payload.get(&mut vrng).to_vec()));
                if buf.len() == batch {
                    e.write_batch(&buf).expect("load");
                    buf.clear();
                }
            }
            if !buf.is_empty() {
                e.write_batch(&buf).expect("load");
            }
            e.sync().expect("sync");

            // Transaction phase.
            let mut g = KeyGen::new(*dist, n, 0x9C5B);
            let mut pick = Rng::new(0x5EED);
            let mut h = Hist::new();
            let mut wbuf: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(batch);
            let t = Instant::now();
            for _ in 0..ops {
                let roll = (pick.next() % 100) as u32;
                db_key_into(g.next(), &mut kb);
                let t1 = Instant::now();
                if roll < *pread {
                    let _ = e.get(&kb).expect("read");
                } else if roll < pread + pupd {
                    wbuf.push((kb.to_vec(), payload.get(&mut vrng).to_vec()));
                    if wbuf.len() >= batch {
                        e.write_batch(&wbuf).expect("update");
                        wbuf.clear();
                    }
                } else if roll < pread + pupd + pscan {
                    let _ = e.range(&kb, 50).expect("scan");
                } else if *prmw > 0 {
                    let _ = e.get(&kb).expect("rmw read");
                    wbuf.push((kb.to_vec(), payload.get(&mut vrng).to_vec()));
                    if wbuf.len() >= batch {
                        e.write_batch(&wbuf).expect("rmw write");
                        wbuf.clear();
                    }
                }
                h.record(t1.elapsed().as_nanos() as u64);
            }
            if !wbuf.is_empty() {
                e.write_batch(&wbuf).expect("tail");
            }
            let secs = t.elapsed().as_secs_f64();

            rows.push(jobj! {
                "workload" => J::s(*wname),
                "engine" => J::s(&name),
                "distribution" => J::s(dist.as_str()),
                "ops_per_s" => J::fp(ops as f64 / secs, 1),
                "latency" => h.to_json(),
                "size_mb" => J::fp(e.size_bytes() as f64 / 1048576.0, 2),
                "feature_score" => J::u(e.features().score() as u64),
            });
            println!(
                "  {wname:22} {name:6} {:>10.0} ops/s  p99 {:>8.3} ms",
                ops as f64 / secs,
                h.percentile(99.0) as f64 / 1e6
            );
        }
    }
    rec.series("workloads", J::arr(rows.clone()));

    // The finding this suite exists to surface. A mixed read/write workload is
    // the shape no benchmark in the design document contains, and Supdb's
    // snapshot read model used to have to checkpoint and rebuild a reader to
    // serve one. `Store::read_all` removed that, so the ratio this reports is
    // now the cost of the write itself rather than the cost of publishing it.
    let mixed: Vec<f64> = rows
        .iter()
        .filter(|r| {
            r.path("engine").and_then(|v| v.as_str()) == Some("supdb")
                && r.path("workload")
                    .and_then(|v| v.as_str())
                    .is_some_and(|w| w.starts_with('A'))
        })
        .filter_map(|r| r.num("ops_per_s"))
        .collect();
    let readonly: Vec<f64> = rows
        .iter()
        .filter(|r| {
            r.path("engine").and_then(|v| v.as_str()) == Some("supdb")
                && r.path("workload")
                    .and_then(|v| v.as_str())
                    .is_some_and(|w| w.starts_with('C'))
        })
        .filter_map(|r| r.num("ops_per_s"))
        .collect();
    if let (Some(a), Some(c)) = (mixed.first(), readonly.first()) {
        rec.finding(Finding::new(
            "EXT.3",
            "Supdb sustains a mixed read/write workload within 10x of a read-only one",
            c / a.max(1e-9) < 10.0,
            format!(
                "YCSB-A (50/50) {a:.0} ops/s against YCSB-C (100% read) {c:.0} ops/s -> {:.1}x. \
                 A read after a write is served from the writer's own state; when this \
                 ratio was 13.5x it needed a checkpoint and a fresh Reader, both O(key count)",
                c / a.max(1e-9)
            ),
        ));
    }
    Ok(rec)
}
