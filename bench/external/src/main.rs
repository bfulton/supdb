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
//!   analytics the day-index scorecard against LMDB's genuinely best shape
//!             for the same data (MDB_DUPSORT|MDB_DUPFIXED), because W2.2 and
//!             W2.4 were measured against Supdb's own varint walk and a claim
//!             measured against yourself is not yet a claim about the field.
//!
//! Two rules make the comparison honest rather than flattering:
//!
//!   * Batch size and value shape are identical for every engine.
//!   * Every result carries each engine's feature score, because Supdb
//!     provides one of six guarantees the others provide five or six of, and
//!     a throughput number that does not say so is comparing promises.

mod engines;

use engines::{Batch, Engine, Features, Lmdb, LmdbDup, Next, Redb, Sled, Supdb};
#[cfg(feature = "rocksdb")]
use engines::Rocks;
use std::path::PathBuf;
use std::time::Instant;
use supdb::bench::{
    compare, db_key_into, Comparison, Finding, Hist, KeyDist, KeyGen, Payload, Profile, Record,
    Rng, Samples, Trial, Verdict, J,
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
    /// A bare flag, with no value after it. `get` would return whatever
    /// followed, which for a trailing flag is nothing at all.
    fn has(&self, n: &str) -> bool {
        self.0.iter().any(|a| a == n)
    }
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
    /// A comma-separated list of integers, e.g. `--keys-list 100000,1000000`.
    /// Entries that do not parse are dropped rather than defaulted, so a typo
    /// shrinks the sweep visibly instead of silently substituting a shape.
    fn list(&self, n: &str, d: &str) -> Vec<u64> {
        self.get(n)
            .unwrap_or(d)
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    }
}

/// Build the field. Supdb first, then the comparators.
fn build(root: &std::path::Path, which: &[&str], buffer_mb: usize) -> Vec<Box<dyn Engine>> {
    let mut out: Vec<Box<dyn Engine>> = Vec::new();
    for name in which {
        let dir = root.join(name);
        let e: Result<Box<dyn Engine>, String> = match *name {
            "supdb" => Supdb::create(&dir, buffer_mb).map(|e| Box::new(e) as Box<dyn Engine>),
            "next" => Next::create(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
            "next-ingest" => {
                Next::create_ingest(&dir).map(|e| Box::new(e) as Box<dyn Engine>)
            }
            "supdb-durable" => {
                Supdb::create_durable(&dir, buffer_mb).map(|e| Box::new(e) as Box<dyn Engine>)
            }
            "supdb-buffered" => {
                Supdb::create_buffered(&dir, buffer_mb).map(|e| Box::new(e) as Box<dyn Engine>)
            }
            "lmdb-nosync" => Lmdb::create_nosync(&dir, 8).map(|e| Box::new(e) as Box<dyn Engine>),
            "redb" => Redb::create(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
            "lmdb" => Lmdb::create(&dir, 8).map(|e| Box::new(e) as Box<dyn Engine>),
            "sled" => Sled::create(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
            #[cfg(feature = "rocksdb")]
            "rocksdb" => Rocks::create(&dir, true).map(|e| Box::new(e) as Box<dyn Engine>),
            #[cfg(feature = "rocksdb")]
            "rocksdb-nosync" => {
                Rocks::create(&dir, false).map(|e| Box::new(e) as Box<dyn Engine>)
            }
            #[cfg(feature = "rocksdb")]
            "rocksdb-tuned" => Rocks::create_tuned(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
            #[cfg(feature = "rocksdb")]
            "rocksdb-tuned-drain" => {
                Rocks::create_tuned_drain(&dir).map(|e| Box::new(e) as Box<dyn Engine>)
            }
            #[cfg(not(feature = "rocksdb"))]
            "rocksdb" | "rocksdb-nosync" | "rocksdb-tuned" | "rocksdb-tuned-drain" => Err(
                "built without the rocksdb feature: cargo build -p supdb-external --features rocksdb"
                    .to_string(),
            ),
            "next-nodrain" => Next::create_nodrain(&dir).map(|e| Box::new(e) as Box<dyn Engine>),
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
        "sweep" => suite_sweep(&args, profile, &engines)?,
        "readdecomp" => suite_readdecomp(&args, profile, &engines)?,
        "analytics" => suite_analytics(&args, profile)?,
        "loadprof" => return load_profile(&args, &engines),
        "loadshape" => suite_loadshape(&args, profile, &engines)?,
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
                "external <kv|ycsb|sweep|readdecomp|analytics|all|loadshape|loadprof> \
                 [--profile ci|dev|full] [--engines supdb,redb,lmdb,sled] \
                 (analytics fields its own arms and ignores --engines; readdecomp \
                 wants --engines supdb-buffered,lmdb)"
            );
            return Ok(());
        }
    };
    rec.print_summary();
    rec.write(&out)?;
    Ok(())
}

/// Does the load comparison depend on the order the keys arrive in?
///
/// `EXT.10` has Supdb loading at 0.529x of an LMDB that is not syncing either,
/// and it has read 0.542x, 0.623x and 0.529x across three runs, so it is not
/// drift. An append-structured store losing bulk ingest to a B-tree is the one
/// result this design should not produce, and one thing about how it is
/// measured has never been varied: every load phase in this suite walks `i` in
/// `0..n`. `KeyDist::Sequential`'s own documentation calls that "the best case
/// for any structure with sorted layout", which is what a B-tree is and what
/// an append store is not.
///
/// So load the same keys in a shuffled order and see whether the ordering
/// survives. Both shapes are reported. The point is not to find a workload
/// Supdb wins -- it is that a claim measured on one arrival order is a claim
/// about that order, and the suite has been making it about loads in general.
fn suite_loadshape(args: &Args, profile: Profile, which: &[&str]) -> std::io::Result<Record> {
    let n = args.num("--keys", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let batch = args.num("--batch", 1_000);
    let reps = args.num("--reps", profile.reps());

    let mut rec = Record::new("ext-loadshape", profile);
    rec.param("keys", J::u(n))
        .param("value_size", J::u(value_size as u64))
        .param("batch", J::u(batch as u64))
        .param("reps", J::u(reps as u64))
        .note("the same key set both ways: sequential is 0..n, shuffled is a permutation of it")
        .note(
            "engines and orders interleaved round-robin over reps, one warmup discarded, every \
             ordering gated on stats::compare",
        );

    let payload = Payload::new(value_size, 0.5, 0xE4);
    let orders = [false, true]; // false = sequential, true = shuffled
    let ne = which.len() * 2;
    let mut load: Vec<Samples> = vec![Samples::default(); ne];
    let mut feats: Vec<Option<Features>> = vec![None; ne];
    let warmup = 1usize;

    for rep in 0..(warmup + reps) {
        for (ei, (name, shuffled)) in which
            .iter()
            .flat_map(|nm| orders.iter().map(move |o| (nm, *o)))
            .enumerate()
        {
            let root = scratch(&format!("shape-{name}-{}-{rep}", shuffled as u8));
            let Some(mut e) = build(&root, &[name], 256).into_iter().next() else {
                continue;
            };
            feats[ei] = Some(e.features());
            // The same keys either way. A permutation rather than random
            // draws, so both arms insert exactly one of each and the two files
            // hold the same thing.
            let mut order: Vec<u64> = (0..n).collect();
            if shuffled {
                let mut r = Rng::new(0xE4 + rep as u64);
                for i in (1..order.len()).rev() {
                    order.swap(i, (r.next() % (i as u64 + 1)) as usize);
                }
            }
            let mut vrng = Rng::new(0xE4);
            let mut kb = [0u8; 16];
            let mut buf = Batch::with_capacity(batch, payload.value_size());
            let t = Instant::now();
            for i in &order {
                db_key_into(*i, &mut kb);
                buf.push(&kb, payload.get(&mut vrng));
                if buf.len() == batch {
                    buf.flush(e.as_mut()).expect("write");
                }
            }
            if !buf.is_empty() {
                buf.flush(e.as_mut()).expect("write");
            }
            e.sync().expect("sync");
            let secs = t.elapsed().as_secs_f64();
            if rep >= warmup {
                load[ei].push(n as f64 / secs);
            }
            drop(e);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    let label = |ei: usize| {
        format!(
            "{}-{}",
            which[ei / 2],
            if ei.is_multiple_of(2) { "seq" } else { "shuffled" }
        )
    };
    let mut rows = Vec::new();
    for ei in 0..ne {
        if load[ei].is_empty() {
            continue;
        }
        println!("  {:<24} load {:>10.0}/s", label(ei), load[ei].median());
        rows.push(jobj! {
            "arm" => J::s(label(ei)),
            "engine" => J::s(which[ei / 2]),
            "shuffled" => J::Bool(ei % 2 == 1),
            "load_ops_per_s" => J::fp(load[ei].median(), 1),
            "load" => load[ei].to_json()
        });
    }
    rec.series("arms", J::arr(rows));

    let idx = |name: &str, shuffled: bool| {
        which
            .iter()
            .position(|w| *w == name)
            .map(|i| i * 2 + shuffled as usize)
    };
    // How much each engine cares about arrival order, which is the property
    // rather than the ranking. Same engine, same guarantees, same key set --
    // so nothing needs matching and there is no residual to bound.
    for name in [
        "supdb-buffered",
        "lmdb-nosync",
        "supdb",
        "lmdb",
        "next",
        "next-nodrain",
        "rocksdb",
        "rocksdb-tuned",
    ] {
        let (Some(a), Some(b)) = (idx(name, false), idx(name, true)) else {
            continue;
        };
        if load[a].is_empty() || load[b].is_empty() {
            continue;
        }
        rec.compare(
            &format!("{name}_seq_vs_shuffled"),
            compare(&load[a], &load[b], supdb::bench::MIN_EFFECT),
        );
    }
    if let (Some(sa), Some(sb), Some(la), Some(lb)) = (
        idx("supdb-buffered", false),
        idx("supdb-buffered", true),
        idx("lmdb-nosync", false),
        idx("lmdb-nosync", true),
    ) {
        if !load[sa].is_empty() && !load[lb].is_empty() {
            let supdb_swing = load[sa].median() / load[sb].median().max(1e-9);
            let lmdb_swing = load[la].median() / load[lb].median().max(1e-9);
            rec.finding(Finding::new(
                "EXT.14",
                "Supdb's load rate depends less on key arrival order than LMDB's",
                supdb_swing < lmdb_swing,
                format!(
                    "Supdb loads at {:.0}/s in order and {:.0} shuffled, a factor of \
                     {supdb_swing:.2}; LMDB at {:.0} and {:.0}, a factor of {lmdb_swing:.2}. This \
                     is the architectural difference rather than a ranking: a B-tree writing keys \
                     in order fills pages left to right and splits almost never, and the same \
                     B-tree taking them shuffled splits constantly, while an append-structured \
                     store writes where the cursor already is either way. Both engines are \
                     measured on the same permutation of the same keys",
                    load[sa].median(),
                    load[sb].median(),
                    load[la].median(),
                    load[lb].median()
                ),
            ));
        }
    }
    // The next engine against LMDB, matched on durability and transactions:
    // the same pair as EXT.22, whose canonical load arrives in order and is
    // mostly piece promotion (F55.3). Shuffled arrival is the shape promotion
    // cannot help, and this is where it is recorded rather than inferred.
    if let (Some(ns), Some(ls)) = (idx("next", true), idx("lmdb", true)) {
        if !load[ns].is_empty() && !load[ls].is_empty() {
            if let (Some(fa), Some(fb)) = (feats[ns], feats[ls]) {
                let gap = fa.unmatched(&fb, true);
                if !gap.is_empty() {
                    rec.finding(Finding::not_exercised(
                        "EXT.27",
                        "the next engine, durable per batch, loads a shuffled key set at least as \
                         fast as LMDB",
                        format!("not an ordering: the arms differ on {}", gap.join(", ")),
                    ));
                } else {
                    let cmp = compare(&load[ns], &load[ls], supdb::bench::MIN_EFFECT);
                    rec.compare("EXT.27_shuffled", cmp.clone());
                    let seq = idx("next", false).zip(idx("lmdb", false));
                    let seq_ratio = seq
                        .map(|(a, b)| load[a].median() / load[b].median().max(1e-9))
                        .unwrap_or(f64::NAN);
                    rec.finding(Finding::new(
                        "EXT.27",
                        "the next engine, durable per batch, loads a shuffled key set at least as \
                         fast as LMDB",
                        !matches!(cmp.verdict, Verdict::Less),
                        format!(
                            "shuffled, {:.0} ops/s against {:.0} ({}). Sequential, in the same \
                             run, is {seq_ratio:.3}x -- EXT.22's shape, where the seals are \
                             promoted by rename. Both commit per batch and both are \
                             transactional, so nothing leans",
                            load[ns].median(),
                            load[ls].median(),
                            cmp.summary("next", "lmdb")
                        ),
                    ));
                }
            }
        }
    }
    // And against RocksDB, which an arrival order should not move much: the
    // comparison EXT.27 needed before it could mean more than "an LSM beats
    // a B-tree under per-batch fsync".
    for (id, mine, rocks) in [
        ("EXT.31", "next", "rocksdb"),
        ("EXT.35", "next", "rocksdb-tuned"),
        ("EXT.41", "next-nodrain", "rocksdb-tuned"),
    ] {
        let (Some(ns), Some(rs)) = (idx(mine, true), idx(rocks, true)) else {
            continue;
        };
        if !load[ns].is_empty() && !load[rs].is_empty() {
            if let (Some(fa), Some(fb)) = (feats[ns], feats[rs]) {
                let gap = fa.unmatched(&fb, true);
                if !gap.is_empty() {
                    rec.finding(Finding::not_exercised(
                        id,
                        "the next engine, syncing per batch, loads a shuffled key set at least as \
                         fast as RocksDB",
                        format!("not an ordering: the arms differ on {}", gap.join(", ")),
                    ));
                } else {
                    let cmp = compare(&load[ns], &load[rs], supdb::bench::MIN_EFFECT);
                    rec.compare(&format!("{id}_shuffled"), cmp.clone());
                    let seq = idx(mine, false).zip(idx(rocks, false));
                    let seq_ratio = seq
                        .map(|(a, b)| load[a].median() / load[b].median().max(1e-9))
                        .unwrap_or(f64::NAN);
                    rec.finding(Finding::new(
                        id,
                        "the next engine, syncing per batch, loads a shuffled key set at least as \
                         fast as RocksDB",
                        !matches!(cmp.verdict, Verdict::Less),
                        format!(
                            "shuffled, {:.0} ops/s against {:.0} ({}). Sequential, in the same \
                             run, is {seq_ratio:.3}x. Both sync the WAL per batch and both apply \
                             a batch whole; an LSM against an LSM, so the arrival order should \
                             move neither much",
                            load[ns].median(),
                            load[rs].median(),
                            cmp.summary(mine, rocks)
                        ),
                    ));
                }
            }
        }
    }
    if let (Some(ss), Some(sl)) = (
        idx("supdb-buffered", true),
        idx("lmdb-nosync", true),
    ) {
        if !load[ss].is_empty() && !load[sl].is_empty() {
            let (Some(fa), Some(fb)) = (feats[ss], feats[sl]) else {
                return Ok(rec);
            };
            let gap = fa.unmatched(&fb, true);
            if !gap.is_empty() {
                rec.finding(Finding::not_exercised(
                    "EXT.13",
                    "Supdb loads faster than LMDB when the keys do not arrive in order",
                    format!("not an ordering: the arms differ on {}", gap.join(", ")),
                ));
                return Ok(rec);
            }
            let cmp = compare(&load[ss], &load[sl], supdb::bench::MIN_EFFECT);
            rec.compare("EXT.13_shuffled", cmp.clone());
            let seq = idx("supdb-buffered", false).zip(idx("lmdb-nosync", false));
            let seq_ratio = seq
                .map(|(a, b)| load[a].median() / load[b].median().max(1e-9))
                .unwrap_or(f64::NAN);
            rec.finding(Finding::new(
                "EXT.13",
                "Supdb loads faster than LMDB when the keys do not arrive in order",
                matches!(cmp.verdict, Verdict::Greater),
                format!(
                    "shuffled, {:.0} ops/s against {:.0} ({}). Sequential, in the same run, is \
                     {seq_ratio:.3}x -- which is what EXT.10 measures and the only arrival order \
                     this suite had ever used. Neither commits to the device and neither \
                     checksums; lmdb-nosync is still transactional, so read this as a bound",
                    load[ss].median(),
                    load[sl].median(),
                    cmp.summary("supdb-buffered", "lmdb-nosync")
                ),
            ));
        }
    }
    Ok(rec)
}

/// One engine, one bulk load, then exit -- the shape a profiler can attribute.
///
/// callgrind and cachegrind attribute to the process, so a driver that also
/// reads and scans mixes three access patterns into one instruction count.
/// This does the load and nothing else. Run it once with `--keys 0` and
/// subtract to remove store creation and the payload generator, exactly as
/// `docs/profiling.md` does with `indexlab probe --lookups 0`.
///
/// It exists because EXT.10 says Supdb loads at 0.54x of an LMDB that is not
/// syncing either -- a B-tree beating an append-structured store at bulk
/// ingest, which is the one thing this design is supposed to win. That is a
/// defect to find rather than a tradeoff to accept, and no timing harness can
/// say where it went.
fn load_profile(args: &Args, which: &[&str]) -> std::io::Result<()> {
    let n = args.num("--keys", 200_000) as u64;
    let value_size = args.num("--value-size", 100);
    let batch = args.num("--batch", 1_000).max(1);
    let name = which.first().copied().unwrap_or("supdb-buffered");
    let root = scratch(&format!("loadprof-{name}"));
    let Some(mut e) = build(&root, &[name], 256).into_iter().next() else {
        eprintln!("# no engine {name}");
        return Ok(());
    };
    let payload = Payload::new(value_size, 0.5, 0xE1);
    let mut vrng = Rng::new(0xE1);
    let mut kb = [0u8; 16];
    let mut buf = Batch::with_capacity(batch, payload.value_size());
    let t = Instant::now();
    for i in 0..n {
        db_key_into(i, &mut kb);
        buf.push(&kb, payload.get(&mut vrng));
        if buf.len() == batch {
            buf.flush(e.as_mut()).expect("write");
        }
    }
    if !buf.is_empty() {
        buf.flush(e.as_mut()).expect("write");
    }
    // Split the two halves. cachegrind put a third of this workload's
    // last-level misses in `checkpoint_inner`, `seal_shard` and the memcpy
    // inside them, and only 1% in the hash probe -- so where the time goes is
    // worth measuring directly rather than inferring from a miss profile.
    let puts = t.elapsed().as_secs_f64();
    let ts = Instant::now();
    // `--skip-sync` leaves the flush and the checkpoint out entirely, because
    // cachegrind attributes to the process: a run that syncs mixes the put
    // path with `checkpoint_inner` and `seal_shard`, and those dominate. The
    // first attempt at this profile did exactly that and had to be thrown
    // away -- the giveaway was `checkpoint_inner` at 13.6% of write misses in
    // what was supposed to be a put-only trace.
    if !args.has("--skip-sync") {
        e.sync().expect("sync");
    }
    let sync = ts.elapsed().as_secs_f64();
    let secs = puts + sync;
    println!(
        "{name} loaded {n} keys in {secs:.3}s ({:.0} ops/s), {:.1} MB \
         [puts {puts:.3}s {:.0}%, sync {sync:.3}s {:.0}%]",
        n as f64 / secs.max(1e-9),
        e.size_bytes() as f64 / 1048576.0,
        100.0 * puts / secs.max(1e-9),
        100.0 * sync / secs.max(1e-9)
    );
    drop(e);
    let _ = std::fs::remove_dir_all(&root);
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
        .note(format!(
            "load_rss_mb is the delta of current RSS across the load, not a peak: VmHWM never \
             falls, so with several engines interleaved in one process a high-water mark set by \
             one contaminates every engine after it. load_device_write_mb comes from \
             {} and is a different quantity from file size",
            supdb::bench::env::device_write_counter_source()
        ))
        .note(
            "engines interleaved round-robin over reps, one warmup round discarded; medians \
             reported, and every ordering gated on stats::compare",
        );

    let payload = Payload::new(value_size, 0.5, 0xE1);
    let ne = which.len();
    let mut load: Vec<Samples> = vec![Samples::default(); ne];
    let mut read: Vec<Samples> = vec![Samples::default(); ne];
    let mut scan: Vec<Samples> = vec![Samples::default(); ne];
    // Rule 4: throughput never travels alone. This suite has reported load
    // rates, read latency and file size since it was written, and never the
    // other two the rule names. That mattered more than it looked: `Store::put`
    // does not seal when the shard buffer fills -- only `append` does -- so a
    // load buffers every key in memory and flushes once at the end, and the
    // load figure has partly been measuring "defer everything, then do it at
    // once" with nothing beside it to say what that costs.
    //
    // RSS is the delta of *current* RSS across the load, not the peak: VmHWM
    // never falls, so with six engines interleaved in one process a high-water
    // mark set by the first contaminates every one after it.
    let mut rss: Vec<Samples> = vec![Samples::default(); ne];
    let mut wrote: Vec<Samples> = vec![Samples::default(); ne];
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
            let rss0 = supdb::bench::env::rss_bytes();
            let io0 = supdb::bench::IoCounters::read_now();
            let t = Instant::now();
            let mut buf = Batch::with_capacity(batch, payload.value_size());
            let mut kb = [0u8; 16];
            for i in 0..n {
                db_key_into(i, &mut kb);
                buf.push(&kb, payload.get(&mut vrng));
                if buf.len() == batch {
                    buf.flush(e.as_mut()).expect("write");
                }
            }
            if !buf.is_empty() {
                buf.flush(e.as_mut()).expect("write");
            }
            e.sync().expect("sync");
            let load_s = t.elapsed().as_secs_f64();
            let load_rss = supdb::bench::env::rss_bytes().saturating_sub(rss0);
            let load_wrote = supdb::bench::IoCounters::read_now().since(&io0).write_bytes;

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
                rss[ei].push(load_rss as f64 / 1048576.0);
                wrote[ei].push(load_wrote as f64 / 1048576.0);
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
            "load_rss_mb" => J::fp(rss[ei].median(), 1),
            "load_rss" => rss[ei].to_json(),
            // From the device-level counter (named per platform in this
            // record's env block and note), never inferred from file size:
            // the two are different quantities and the rule says so.
            "load_device_write_mb" => J::fp(wrote[ei].median(), 1),
            "load_write_amp" => J::fp(
                wrote[ei].median() * 1048576.0 / (n as f64 * (16.0 + value_size as f64)).max(1.0),
                3
            ),
            "scan_entries_per_s" => J::fp(scan[ei].median(), 1),
            "scan" => scan[ei].to_json(),
            "read_latency" => hists[ei].to_json(),
            "size_mb" => J::fp(size[ei], 2)
        });
        println!(
            "  {name:14} load {:>9.0}/s  read {:>9.0}/s  scan {:>10.0}/s  {:>7.1} MB  \
             rss {:>7.1} MB  wrote {:>7.1} MB  features {}/6",
            load[ei].median(),
            read[ei].median(),
            scan[ei].median(),
            size[ei],
            rss[ei].median(),
            wrote[ei].median(),
            f.score()
        );
    }
    rec.series("engines", J::arr(rows.clone()));

    let idx = |name: &str| which.iter().position(|w| *w == name);
    // `mine` is the left-hand engine. It is a parameter rather than always
    // "supdb" because EXT.9 compares the durable arm, and an engine comparing
    // itself against a comparator on a boundary the comparator does not use is
    // the thing EXT.9 exists to stop.
    // `writes` says whether the metric touches the write path, which decides
    // whether the durability axis has to match for the ordering to mean
    // anything. Everything else that can be equalized must match on every
    // metric; when it does not, the pair is `not_exercised` rather than
    // ranked. That is the whole of the fix: the features table used to be a
    // note printed beside a number, and it is a precondition now.
    let ordering_of = |rec: &mut Record,
                       id: &str,
                       title: &str,
                       mine: &str,
                       other: &str,
                       s: &[Samples],
                       unit: &str,
                       writes: bool| {
        let (Some(si), Some(oi)) = (idx(mine), idx(other)) else {
            return;
        };
        if s[si].is_empty() || s[oi].is_empty() {
            return;
        }
        let (Some(fa), Some(fb)) = (feats[si], feats[oi]) else {
            return;
        };
        let gap = fa.unmatched(&fb, writes);
        if !gap.is_empty() {
            rec.finding(Finding::not_exercised(
                id,
                title,
                format!(
                    "not an ordering: {mine} and {other} do not promise the same thing on {}, and \
                     each of those could have been equalized. {mine} measured {:.0} {unit} and \
                     {other} {:.0}, which is recorded because it is what the run did, not because \
                     it ranks them. Use the matched arms",
                    gap.join(", "),
                    s[si].median(),
                    s[oi].median()
                ),
            ));
            return;
        }
        let cmp = compare(&s[si], &s[oi], supdb::bench::MIN_EFFECT);
        let holds = matches!(cmp.verdict, Verdict::Greater);
        rec.compare(&format!("{id}_{mine}_vs_{other}"), cmp.clone());
        // Transactions are the one axis that cannot be equalized, so say which
        // way the remainder leans instead of pretending it is not there.
        let residual = if fa.free_ride(&fb) {
            format!(
                ". {other} is still transactional and {mine} is not, which no configuration can \
                 equalize, so read this as a bound: a loss here is at least this large and a win \
                 is not yet a win"
            )
        } else if fb.free_ride(&fa) {
            format!(
                ". {mine} is still transactional and {other} is not, so this understates {mine}"
            )
        } else {
            String::new()
        };
        rec.finding(Finding::new(
            id,
            title,
            holds,
            format!(
                "{mine} {:.0} {unit} vs {other} {:.0} {unit} ({}){residual}",
                s[si].median(),
                s[oi].median(),
                cmp.summary(mine, other)
            ),
        ));
    };
    let ordering =
        |rec: &mut Record,
         id: &str,
         title: &str,
         other: &str,
         s: &[Samples],
         unit: &str,
         writes: bool| { ordering_of(rec, id, title, "supdb", other, s, unit, writes) };

    // EXT.1, EXT.4 and EXT.5 are kept, and all three are now `not_exercised`
    // against LMDB: plain `supdb` checksums and LMDB does not, and on load it
    // also does not commit durably. Their numbers stay in the record because
    // they are what the run did. They were never orderings.
    ordering(
        &mut rec,
        "EXT.1",
        "Supdb loads faster than LMDB, the architecture it is modelled on",
        "lmdb",
        &load,
        "ops/s",
        true,
    );
    ordering(
        &mut rec,
        "EXT.2",
        "Supdb reads faster than redb, the closest non-mmap sibling",
        "redb",
        &read,
        "reads/s",
        false,
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
        false,
    );
    ordering(
        &mut rec,
        "EXT.5",
        "Supdb scans faster than LMDB when both are measured natively",
        "lmdb",
        &scan,
        "entries/s",
        false,
    );

    // ---- the matched comparisons -------------------------------------------
    //
    // Four claims that actually rank the engines, because on each one the two
    // arms promise the same thing. The load axis is measured at both levels of
    // promise rather than at neither: EXT.9 has both committing to the device
    // per batch, EXT.10 has neither. Whichever guarantee a reader cares about,
    // one of these is the comparison for it, and EXT.1 is not.
    ordering_of(
        &mut rec,
        "EXT.9",
        "Supdb loads faster than LMDB when both commit durably per batch",
        "supdb-durable",
        "lmdb",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.10",
        "Supdb loads faster than LMDB when neither commits to the device",
        "supdb-buffered",
        "lmdb-nosync",
        &load,
        "ops/s",
        true,
    );
    // The next engine (supdb::next), measured on the same three axes against
    // the same LMDB in the same process. Its commit is a WAL append plus one
    // fdatasync per batch -- LMDB's own boundary -- so the load comparison is
    // matched the way EXT.9 is, with the same transactional residual.
    ordering_of(
        &mut rec,
        "EXT.22",
        "The next engine loads faster than LMDB when both commit durably per batch",
        "next",
        "lmdb",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.23",
        "The next engine reads faster than LMDB",
        "next",
        "lmdb",
        &read,
        "reads/s",
        false,
    );
    // The read-for-write trade, measured in one run rather than across
    // two: same engine, same guarantees, one policy bit apart, so nothing
    // needs matching and there is no residual to bound.
    ordering_of(
        &mut rec,
        "EXT.25",
        "Leaving partitioning to background compaction ingests faster than doing it at flush",
        "next-ingest",
        "next",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.26",
        "and it costs the ordered scan",
        "next",
        "next-ingest",
        &scan,
        "entries/s",
        false,
    );
    ordering_of(
        &mut rec,
        "EXT.24",
        "The next engine scans no slower than LMDB",
        "next",
        "lmdb",
        &scan,
        "entries/s",
        false,
    );
    // The same three axes against RocksDB, the engine the next engine is
    // shaped like: both sync the WAL per batch, both apply a batch whole,
    // neither verifies a checksum on read (Features::unmatched decides the
    // rest). This is the pair that says whether the next engine is fast or
    // an LSM is; the LMDB pair cannot.
    ordering_of(
        &mut rec,
        "EXT.28",
        "The next engine loads faster than RocksDB when both sync the WAL per batch",
        "next",
        "rocksdb",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.29",
        "The next engine reads faster than RocksDB",
        "next",
        "rocksdb",
        &read,
        "reads/s",
        false,
    );
    ordering_of(
        &mut rec,
        "EXT.30",
        "The next engine scans no slower than RocksDB",
        "next",
        "rocksdb",
        &scan,
        "entries/s",
        false,
    );
    // And against RocksDB tuned as it is deployed -- a block cache the data
    // fits in, a Bloom filter, four background threads -- which is the pair
    // the read numbers above may be quoted from.
    ordering_of(
        &mut rec,
        "EXT.32",
        "The next engine loads faster than tuned RocksDB when both sync the WAL per batch",
        "next",
        "rocksdb-tuned",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.33",
        "The next engine reads faster than tuned RocksDB",
        "next",
        "rocksdb-tuned",
        &read,
        "reads/s",
        false,
    );
    ordering_of(
        &mut rec,
        "EXT.34",
        "The next engine scans no slower than tuned RocksDB",
        "next",
        "rocksdb-tuned",
        &scan,
        "entries/s",
        false,
    );
    // The drain matched both ways (f60, drain-plan.md). Default `next`
    // seals and partitions inside its load window; RocksDB's sync is an
    // fsync. So: both drained -- RocksDB flushed and compacted at sync --
    // and neither drained -- next's sync an fsync, its tail read out of the
    // memtable and the unrouted level as RocksDB's is.
    ordering_of(
        &mut rec,
        "EXT.36",
        "The next engine loads faster than tuned RocksDB when both drain at sync",
        "next",
        "rocksdb-tuned-drain",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.37",
        "The next engine loads faster than tuned RocksDB when neither drains at sync",
        "next-nodrain",
        "rocksdb-tuned",
        &load,
        "ops/s",
        true,
    );
    ordering_of(
        &mut rec,
        "EXT.38",
        "The next engine reads faster than tuned RocksDB when neither drained",
        "next-nodrain",
        "rocksdb-tuned",
        &read,
        "reads/s",
        false,
    );
    ordering_of(
        &mut rec,
        "EXT.39",
        "The next engine scans no slower than tuned RocksDB when neither drained",
        "next-nodrain",
        "rocksdb-tuned",
        &scan,
        "entries/s",
        false,
    );
    ordering_of(
        &mut rec,
        "EXT.40",
        "The next engine reads faster than tuned RocksDB when both drained",
        "next",
        "rocksdb-tuned-drain",
        &read,
        "reads/s",
        false,
    );
    // Durability does not touch a read or a scan, so these need only the
    // checksum axis matched -- and that one was costing Supdb 8.5% on every
    // EXT.4 figure ever recorded, in the direction nobody was watching.
    ordering_of(
        &mut rec,
        "EXT.11",
        "Supdb reads faster than LMDB when neither verifies checksums",
        "supdb-buffered",
        "lmdb",
        &read,
        "reads/s",
        false,
    );
    ordering_of(
        &mut rec,
        "EXT.12",
        "Supdb scans faster than LMDB when neither verifies checksums",
        "supdb-buffered",
        "lmdb",
        &scan,
        "entries/s",
        false,
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

/// Decompose the point-read comparison against LMDB into its candidate
/// mechanisms.
///
/// The fact this exists to split: EXT.11 (supdb-buffered vs lmdb, uniform
/// point reads at 1M keys) is a tie on the x86 host -- 1.243x p=0.37 and
/// 1.179x p=0.13 across two full runs, unable to separate -- and a replicated
/// 2.42x/2.41x win on Apple Silicon at p=0.0022 with rel_iqr under 1.3%
/// (`results/apple-silicon/ext-kv-buffered-read.run{1,2}.json`). Nothing on
/// the books says *why*. The candidate mechanisms:
///
///   (a) 128-byte cache lines: Supdb's flatindex probe touches ~1 line where
///       LMDB's descent touches several per node, so a wider line forgives
///       LMDB's node search less than it forgives a single probe.
///   (b) 16 KiB pages: fewer TLB entries cover the same file, and a descent
///       touches ~depth distinct pages per lookup where a hash probe touches
///       ~2, so TLB relief compounds differently.
///   (c) O(1) probe vs O(log n) descent: depth itself, priced differently
///       per level on the two memory systems.
///   (d) something else -- value handling, memory bandwidth, mmap fault
///       behavior.
///
/// None of these can be toggled without recompiling LMDB, so the split is by
/// workload shape, three axes in one process, every arm interleaved:
///
///   * **key count** (100k / 1M / 4M at `full`): descent depth grows with
///     log n and a hash probe does not. If (c) is the mechanism, the
///     supdb/lmdb ratio grows with n -- on both architectures.
///   * **hot subset** (uniform over the first 4k / 256k key ids at the anchor
///     count): a contiguous-id hot set is compact in both engines -- adjacent
///     leaves for LMDB, adjacent value blocks for Supdb -- so at 4k keys the
///     touched data fits in cache and the memory system leaves the picture.
///     If the lead needs DRAM misses to exist (a/b), it shrinks here; if it
///     is the work itself (c-as-compute, d), it survives. The residual leans
///     against Supdb and is recorded: its hash probe scatters the hot keys
///     across the whole index section, so Supdb keeps a TLB cost in the hot
///     cell that LMDB's clustered leaves shed -- a hot-cell lead is therefore
///     conservative.
///   * **value size** (8B / 100B / 1KB at the anchor count): the read cost is
///     lookup plus value bytes. If the lead lives in the lookup, shrinking
///     the value widens the ratio and growing it compresses the ratio toward
///     the bandwidth bound; a ratio flat in value size says the differential
///     is not in the structure walk at all.
///
/// What this deliberately is not: `ext-kv` loads a fresh store per rep and
/// reads it once; this builds each store once and sweeps it warm, the
/// `ext-sweep` precedent, because rebuilding a 4M-key LMDB store per rep does
/// not fit any host's budget. Compare shapes *within* this record; do not
/// average its absolute ratios with EXT.11's, they are different experiments.
/// The prediction table -- which outcome convicts which mechanism, written
/// before the first run -- is `read-decomposition-plan.md` at the repo root.
fn suite_readdecomp(
    args: &Args,
    profile: Profile,
    engines_arg: &[&str],
) -> std::io::Result<Record> {
    // The pair the findings are about. `main` defaults --engines to the
    // four-engine field, which is not what a decomposition of EXT.11 wants,
    // so an absent flag means the matched pair rather than the field.
    let which: Vec<&str> = if args.get("--engines").is_some() {
        engines_arg.to_vec()
    } else {
        vec!["supdb-buffered", "lmdb"]
    };
    let keys_list = args.list(
        "--keys-list",
        profile.pick(
            "2000,8000,32000",
            "50000,200000,800000",
            "100000,1000000,4000000",
        ),
    );
    let hot_list = args.list(
        "--hot-list",
        profile.pick("64,512", "1024,65536", "4096,262144"),
    );
    let extra_values = args.list("--value-sizes", "8,1024");
    let base_value = args.num("--value-size", 100);
    let reads = args.num("--reads", profile.pick(2_000, 100_000, 500_000)) as u64;
    let batch = args.num("--batch", 1_000).max(1);
    let reps = args.num("--reps", profile.reps());
    // The anchor: the key count the hot and value axes pivot on. The middle
    // of the list, which at the defaults is 1M -- EXT.11's own shape.
    let anchor = keys_list[keys_list.len() / 2];

    let mut rec = Record::new("ext-readdecomp", profile);
    rec.param(
        "keys_list",
        J::arr(keys_list.iter().map(|k| J::u(*k)).collect()),
    )
    .param("anchor_keys", J::u(anchor))
    .param(
        "hot_list",
        J::arr(hot_list.iter().map(|h| J::u(*h)).collect()),
    )
    .param("value_size", J::u(base_value as u64))
    .param(
        "extra_value_sizes",
        J::arr(extra_values.iter().map(|v| J::u(*v)).collect()),
    )
    .param("reads_per_cell", J::u(reads))
    .param("batch", J::u(batch as u64))
    .param("reps", J::u(reps as u64))
    .note(
        "stores built once per (keys, value_size) and swept warm, the ext-sweep precedent; \
             compare shapes within this record, never its absolute ratios against ext-kv's, \
             which rebuilds per rep",
    )
    .note(
        "hot cells draw uniformly from the first K key ids: contiguous ids are adjacent \
             leaves for LMDB and adjacent value blocks for Supdb, so both engines' touched data \
             is compact. The residual leans against Supdb -- its hash probe scatters K keys \
             across the whole index section, so it keeps a TLB cost in the hot cell that LMDB \
             sheds -- and a hot-cell lead is therefore conservative",
    )
    .note(
        "cells and engines interleaved round-robin over reps, engine innermost, one warmup \
             round discarded, every ordering gated on stats::compare. Per-read latency is \
             sampled 1-in-8 so the Instant overhead stays out of the throughput it decorates; \
             the sampling is identical for every arm",
    )
    .note(
        "point reads move no device bytes; latency distributions travel per cell and store \
             sizes per arm, and the load phase's RSS and device-write accounting for this \
             workload shape live in ext-kv's record",
    );

    // ---- the stores: one per (key count, value size) per engine ------------
    let mut pairs: Vec<(u64, usize)> = keys_list.iter().map(|k| (*k, base_value)).collect();
    for v in &extra_values {
        let v = *v as usize;
        if v != base_value && !pairs.contains(&(anchor, v)) {
            pairs.push((anchor, v));
        }
    }
    let anchor_pair = pairs
        .iter()
        .position(|p| *p == (anchor, base_value))
        .expect("anchor comes from keys_list");

    let ne = which.len();
    let mut stores: Vec<Vec<Option<Box<dyn Engine>>>> = Vec::with_capacity(pairs.len());
    let mut feats: Vec<Option<Features>> = vec![None; ne];
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut store_rows: Vec<J> = Vec::new();
    for (n, vs) in &pairs {
        let mut row: Vec<Option<Box<dyn Engine>>> = Vec::with_capacity(ne);
        for (ei, name) in which.iter().enumerate() {
            let root = scratch(&format!("rdec-{name}-{n}-{vs}"));
            let e = build(&root, &[name], 256).into_iter().next();
            let e = e.map(|mut e| {
                feats[ei] = Some(e.features());
                let payload = Payload::new(*vs, 0.5, 0xD3);
                let mut vrng = Rng::new(0xD3);
                let mut kb = [0u8; 16];
                let mut buf = Batch::with_capacity(batch, payload.value_size());
                for i in 0..*n {
                    db_key_into(i, &mut kb);
                    buf.push(&kb, payload.get(&mut vrng));
                    if buf.len() == batch {
                        buf.flush(e.as_mut()).expect("load");
                    }
                }
                if !buf.is_empty() {
                    buf.flush(e.as_mut()).expect("load");
                }
                e.sync().expect("sync");
                store_rows.push(jobj! {
                    "engine" => J::s(*name),
                    "keys" => J::u(*n),
                    "value_size" => J::u(*vs as u64),
                    "size_mb" => J::fp(e.size_bytes() as f64 / 1048576.0, 2)
                });
                e
            });
            row.push(e);
            roots.push(root);
        }
        stores.push(row);
    }
    rec.series("stores", J::arr(store_rows));

    // ---- the cells: (store, key span to draw from) --------------------------
    struct Cell {
        label: String,
        pair: usize,
        span: u64,
    }
    let mut cells: Vec<Cell> = Vec::new();
    for (pi, k) in keys_list.iter().enumerate() {
        cells.push(Cell {
            label: format!("n{k}"),
            pair: pi,
            span: *k,
        });
    }
    for h in &hot_list {
        if *h >= anchor {
            eprintln!("# SKIPPED hot={h}: not a subset of the {anchor}-key anchor store");
            continue;
        }
        cells.push(Cell {
            label: format!("hot{h}"),
            pair: anchor_pair,
            span: *h,
        });
    }
    for (pi, (n, vs)) in pairs.iter().enumerate() {
        if pi >= keys_list.len() {
            debug_assert_eq!(*n, anchor);
            cells.push(Cell {
                label: format!("v{vs}"),
                pair: pi,
                span: anchor,
            });
        }
    }

    // ---- measure -------------------------------------------------------------
    let nc = cells.len();
    let mut rate: Vec<Vec<Samples>> = (0..nc).map(|_| vec![Samples::default(); ne]).collect();
    let mut hists: Vec<Vec<Hist>> = (0..nc)
        .map(|_| (0..ne).map(|_| Hist::new()).collect())
        .collect();
    let mut miss = vec![vec![0u64; ne]; nc];
    let si = which.iter().position(|w| *w == "supdb-buffered");
    let li = which.iter().position(|w| *w == "lmdb");
    let mut ratio: Vec<Samples> = (0..nc).map(|_| Samples::default()).collect();

    let warmup = 1usize;
    for rep in 0..(warmup + reps) {
        for (ci, cell) in cells.iter().enumerate() {
            let mut rep_rate = vec![f64::NAN; ne];
            for (ei, slot) in stores[cell.pair].iter_mut().enumerate() {
                let Some(e) = slot.as_mut() else { continue };
                // The same key sequence for every engine in a cell, varied
                // across reps so a rep is not a replay of the last one.
                let mut g = KeyGen::new(KeyDist::Uniform, cell.span, 0xD3C0 + rep as u64);
                let mut kb = [0u8; 16];
                let mut misses = 0u64;
                let t = Instant::now();
                for i in 0..reads {
                    db_key_into(g.next(), &mut kb);
                    if i % 8 == 0 {
                        let t1 = Instant::now();
                        let got = e.get(&kb).expect("get");
                        if rep >= warmup {
                            hists[ci][ei].record(t1.elapsed().as_nanos() as u64);
                        }
                        if got == 0 {
                            misses += 1;
                        }
                    } else if e.get(&kb).expect("get") == 0 {
                        misses += 1;
                    }
                }
                let secs = t.elapsed().as_secs_f64();
                rep_rate[ei] = reads as f64 / secs.max(1e-12);
                if rep >= warmup {
                    rate[ci][ei].push(rep_rate[ei]);
                    miss[ci][ei] += misses;
                }
            }
            if rep >= warmup {
                if let (Some(s), Some(l)) = (si, li) {
                    if rep_rate[s].is_finite() && rep_rate[l].is_finite() {
                        // Paired within the rep, so drift moves both arms of a
                        // ratio together -- the same reason the engines are
                        // interleaved at all.
                        ratio[ci].push(rep_rate[s] / rep_rate[l]);
                    }
                }
            }
        }
    }

    // Everything is measured; the stores can go before the record is built,
    // because at full this is several GB of scratch.
    drop(stores);
    for root in &roots {
        let _ = std::fs::remove_dir_all(root);
    }

    // ---- record ---------------------------------------------------------------
    let mut rows = Vec::new();
    for (ci, cell) in cells.iter().enumerate() {
        let mut arms = Vec::new();
        for (ei, name) in which.iter().enumerate() {
            if rate[ci][ei].is_empty() {
                continue;
            }
            let total = reads * reps as u64;
            arms.push(jobj! {
                "engine" => J::s(*name),
                "read_ops_per_s" => J::fp(rate[ci][ei].median(), 1),
                "read" => rate[ci][ei].to_json(),
                "read_hit_rate" => J::fp(1.0 - miss[ci][ei] as f64 / total.max(1) as f64, 6),
                "read_latency" => hists[ci][ei].to_json()
            });
        }
        println!(
            "  {:<10} {}",
            cell.label,
            which
                .iter()
                .enumerate()
                .filter(|(ei, _)| !rate[ci][*ei].is_empty())
                .map(|(ei, name)| format!("{name} {:>9.0}/s", rate[ci][ei].median()))
                .collect::<Vec<_>>()
                .join("  ")
        );
        rows.push(jobj! {
            "cell" => J::s(&cell.label),
            "keys" => J::u(pairs[cell.pair].0),
            "value_size" => J::u(pairs[cell.pair].1 as u64),
            "span" => J::u(cell.span),
            "engines" => J::arr(arms),
            "ratio" => ratio[ci].to_json()
        });
    }
    rec.series("cells", J::arr(rows));

    // Per-cell orderings for the pair, and each engine's own cross-shape
    // sensitivity -- which is what says *who* moved when a ratio moves.
    let cell_of = |label: &str| cells.iter().position(|c| c.label == label);
    if let (Some(s), Some(l)) = (si, li) {
        for (ci, cell) in cells.iter().enumerate() {
            if !rate[ci][s].is_empty() && !rate[ci][l].is_empty() {
                rec.compare(
                    &format!("read_{}", cell.label),
                    compare(&rate[ci][s], &rate[ci][l], supdb::bench::MIN_EFFECT),
                );
            }
        }
        let n_lo = keys_list.iter().copied().min().unwrap_or(anchor);
        let n_hi = keys_list.iter().copied().max().unwrap_or(anchor);
        let hot_lo = hot_list.iter().copied().filter(|h| *h < anchor).min();
        for ei in [s, l] {
            if let (Some(a), Some(b)) = (cell_of(&format!("n{n_hi}")), cell_of(&format!("n{n_lo}")))
            {
                if !rate[a][ei].is_empty() && !rate[b][ei].is_empty() {
                    rec.compare(
                        &format!("{}_n{n_hi}_vs_n{n_lo}", which[ei]),
                        compare(&rate[a][ei], &rate[b][ei], supdb::bench::MIN_EFFECT),
                    );
                }
            }
            if let Some(h) = hot_lo {
                if let (Some(a), Some(b)) =
                    (cell_of(&format!("hot{h}")), cell_of(&format!("n{anchor}")))
                {
                    if !rate[a][ei].is_empty() && !rate[b][ei].is_empty() {
                        rec.compare(
                            &format!("{}_hot{h}_vs_full", which[ei]),
                            compare(&rate[a][ei], &rate[b][ei], supdb::bench::MIN_EFFECT),
                        );
                    }
                }
            }
        }
    }

    // ---- the three findings, behind their preconditions ------------------------
    //
    // Each pins one mechanism's signature; the prediction table in
    // read-decomposition-plan.md says what each combination of verdicts
    // convicts. The statements are about *this run's host* -- the whole point
    // is that the two architectures are expected to answer differently.
    let mut blockers: Vec<String> = Vec::new();
    match (si, li) {
        (Some(s), Some(l)) => {
            match (feats[s], feats[l]) {
                (Some(fa), Some(fb)) => {
                    let gap = fa.unmatched(&fb, false);
                    if !gap.is_empty() {
                        blockers.push(format!("the arms differ on {}", gap.join(", ")));
                    }
                }
                _ => blockers
                    .push("an arm recorded no features, so matching cannot be checked".into()),
            }
            for (ci, cell) in cells.iter().enumerate() {
                for ei in [s, l] {
                    if rate[ci][ei].is_empty() {
                        blockers.push(format!("{} recorded nothing in {}", which[ei], cell.label));
                    } else if miss[ci][ei] > 0 {
                        // A miss is a different code path -- usually a shorter
                        // one -- so a cell that missed measured something else.
                        blockers.push(format!(
                            "{} missed {} of its reads in {}",
                            which[ei], miss[ci][ei], cell.label
                        ));
                    }
                }
            }
        }
        _ => {
            blockers.push(
                "the pair this decomposition is about (supdb-buffered vs lmdb) was not fielded"
                    .into(),
            );
        }
    }

    let not_yet = |rec: &mut Record, id: &str, title: &str, why: &str| {
        rec.finding(Finding::not_exercised(id, title, why.to_string()));
    };

    // EXT.19 -- the depth signature.
    let t19 = "Supdb's point-read lead over LMDB grows with key count on this host";
    let n_lo = keys_list.iter().copied().min().unwrap_or(anchor);
    let n_hi = keys_list.iter().copied().max().unwrap_or(anchor);
    let depth_cells = (cell_of(&format!("n{n_hi}")), cell_of(&format!("n{n_lo}")));
    if !blockers.is_empty() {
        not_yet(&mut rec, "EXT.19", t19, &blockers.join("; "));
    } else if n_lo == n_hi {
        not_yet(
            &mut rec,
            "EXT.19",
            t19,
            "the key-count axis has a single point, so growth in n is not testable",
        );
    } else if let (Some(hi), Some(lo)) = depth_cells {
        let cmp = compare(&ratio[hi], &ratio[lo], supdb::bench::MIN_EFFECT);
        rec.compare("EXT.19_lead_at_max_vs_min_keys", cmp.clone());
        if matches!(cmp.verdict, Verdict::Underpowered) {
            not_yet(
                &mut rec,
                "EXT.19",
                t19,
                "underpowered: too few repetitions to compare the leads",
            );
        } else {
            let per_n = keys_list
                .iter()
                .filter_map(|k| {
                    cell_of(&format!("n{k}"))
                        .map(|ci| format!("{k} keys {:.3}x", ratio[ci].median()))
                })
                .collect::<Vec<_>>()
                .join(", ");
            rec.finding(Finding::new(
                "EXT.19",
                t19,
                matches!(cmp.verdict, Verdict::Greater),
                format!(
                    "the supdb/lmdb read ratio, per rep and interleaved, across the key axis: \
                     {per_n} ({}). A B-tree descent deepens with log n and a hash probe does \
                     not, so a lead that grows with n implicates depth (mechanism c) on this \
                     host, and a flat lead says the per-lookup difference is per-access -- \
                     cache-line, TLB, or compute -- rather than per-level",
                    cmp.summary(&format!("lead@{n_hi}"), &format!("lead@{n_lo}"))
                ),
            ));
        }
    } else {
        not_yet(
            &mut rec,
            "EXT.19",
            t19,
            "the key-axis cells were not measured",
        );
    }

    // EXT.20 -- the memory-system signature.
    let t20 = "Supdb's point-read lead over LMDB survives a cache-resident working set";
    let hot_lo = hot_list.iter().copied().filter(|h| *h < anchor).min();
    if !blockers.is_empty() {
        not_yet(&mut rec, "EXT.20", t20, &blockers.join("; "));
    } else if let Some(h) = hot_lo {
        let hc = cell_of(&format!("hot{h}"));
        let ac = cell_of(&format!("n{anchor}"));
        if let ((Some(s), Some(l)), Some(hc), Some(ac)) = ((si, li), hc, ac) {
            let footprint_kb = h as f64 * (16.0 + base_value as f64 + 57.0) / 1024.0;
            let hot_cmp = compare(&rate[hc][s], &rate[hc][l], supdb::bench::MIN_EFFECT);
            let lead_cmp = compare(&ratio[hc], &ratio[ac], supdb::bench::MIN_EFFECT);
            rec.compare("EXT.20_read_hot", hot_cmp.clone());
            rec.compare("EXT.20_lead_hot_vs_uniform", lead_cmp.clone());
            if matches!(hot_cmp.verdict, Verdict::Underpowered) {
                not_yet(
                    &mut rec,
                    "EXT.20",
                    t20,
                    "underpowered: too few repetitions to order the hot cell",
                );
            } else {
                rec.finding(Finding::new(
                    "EXT.20",
                    t20,
                    matches!(hot_cmp.verdict, Verdict::Greater),
                    format!(
                        "uniform reads over the first {h} key ids of the {anchor}-key store, \
                         ~{footprint_kb:.0} KB of touched keys, values and index lines, small \
                         enough that the memory system leaves the picture: {} -- and the lead \
                         itself moved from {:.3}x uniform to {:.3}x hot ({}). A lead that needs \
                         DRAM misses to exist (cache-line width or TLB reach, mechanisms a/b) \
                         dies here; one that survives is the work itself -- fewer dependent \
                         accesses, fewer instructions (c as compute, or d). Supdb's index probes \
                         stay scattered across the whole index section even in this cell, so the \
                         residual TLB cost leans against it and a surviving lead is conservative",
                        hot_cmp.summary("supdb-buffered", "lmdb"),
                        ratio[ac].median(),
                        ratio[hc].median(),
                        lead_cmp.summary("lead@hot", "lead@uniform")
                    ),
                ));
            }
        } else {
            not_yet(
                &mut rec,
                "EXT.20",
                t20,
                "the hot or anchor cell was not measured",
            );
        }
    } else {
        not_yet(
            &mut rec,
            "EXT.20",
            t20,
            "no hot-set size below the anchor key count was requested",
        );
    }

    // EXT.21 -- the value-axis signature.
    let t21 = "Supdb's point-read lead over LMDB is independent of value size";
    let mut vs_all: Vec<usize> = extra_values.iter().map(|v| *v as usize).collect();
    vs_all.push(base_value);
    vs_all.sort_unstable();
    vs_all.dedup();
    let vcell = |v: usize| {
        if v == base_value {
            cell_of(&format!("n{anchor}"))
        } else {
            cell_of(&format!("v{v}"))
        }
    };
    if !blockers.is_empty() {
        not_yet(&mut rec, "EXT.21", t21, &blockers.join("; "));
    } else if vs_all.len() < 2 {
        not_yet(
            &mut rec,
            "EXT.21",
            t21,
            "the value axis has a single point, so independence is not testable",
        );
    } else {
        let (v_lo, v_hi) = (vs_all[0], vs_all[vs_all.len() - 1]);
        if let (Some(a), Some(b)) = (vcell(v_lo), vcell(v_hi)) {
            let cmp = compare(&ratio[a], &ratio[b], supdb::bench::MIN_EFFECT);
            rec.compare("EXT.21_lead_at_min_vs_max_value", cmp.clone());
            if matches!(cmp.verdict, Verdict::Underpowered) {
                not_yet(
                    &mut rec,
                    "EXT.21",
                    t21,
                    "underpowered: too few repetitions to compare the leads",
                );
            } else {
                let per_v = vs_all
                    .iter()
                    .filter_map(|v| vcell(*v).map(|ci| format!("{v}B {:.3}x", ratio[ci].median())))
                    .collect::<Vec<_>>()
                    .join(", ");
                rec.finding(Finding::new(
                    "EXT.21",
                    t21,
                    matches!(cmp.verdict, Verdict::NoDifference),
                    format!(
                        "the lead across the value axis at {anchor} keys: {per_v} ({}). A read \
                         is a lookup plus the value bytes, and only the lookup differs \
                         structurally between a hash table and a B-tree -- so if the lead lives \
                         in the lookup, tiny values widen it and large values compress it toward \
                         the bandwidth bound, and this finding fails in the Greater direction. \
                         Flat-in-value-size instead says the differential is not the structure \
                         walk. Failing Less -- a lead that grows with value size -- would point \
                         at value handling itself (mechanism d) and convict none of a/b/c",
                        cmp.summary(&format!("lead@{v_lo}B"), &format!("lead@{v_hi}B"))
                    ),
                ));
            }
        } else {
            not_yet(
                &mut rec,
                "EXT.21",
                t21,
                "a value-axis cell was not measured",
            );
        }
    }

    Ok(rec)
}

/// Decompose a scan into its constant and its slope, for every engine.
///
/// A scan is a seek plus a walk, and the two have completely different floors.
/// The walk is bounded by memory bandwidth -- you must touch every byte you
/// emit -- while the seek is bounded by the number of *dependent* memory
/// accesses, since probe k+1 cannot issue until probe k returns. Reporting one
/// blended entries/s figure hides which of the two an engine is losing on, and
/// this suite has been reporting exactly that: EXT.5 is a single number at one
/// scan length.
///
/// Measuring the same scan at many lengths separates them. Cost per scan is
/// `a + b*n`: `a` is the seek and everything else fixed, `b` is the marginal
/// cost of one more entry. Each repetition fits its own `a` and `b`, so the
/// two coefficients get distributions and `stats::compare` can be applied to
/// them like anything else here.
///
/// Engines are interleaved at the innermost level, and the entry budget per
/// measurement is held constant so a long scan does not get more samples than
/// a short one.
fn suite_sweep(args: &Args, profile: Profile, which: &[&str]) -> std::io::Result<Record> {
    let n = args.num("--keys", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let batch = args.num("--batch", 1_000);
    let budget = args.num("--budget", profile.pick(20_000, 100_000, 400_000)) as u64;
    let reps = args.num("--reps", profile.reps());
    let lens: Vec<usize> = vec![1, 2, 5, 10, 25, 50, 100, 200, 400];

    let mut rec = Record::new("ext-sweep", profile);
    rec.param("keys", J::u(n))
        .param("value_size", J::u(value_size as u64))
        .param("entry_budget", J::u(budget))
        .param("reps", J::u(reps as u64))
        .note(
            "cost per scan measured at each length; the floor is the observed cost at n=1 and \
             the per-entry cost is the difference quotient between the top two lengths. Neither \
             is fitted: a least-squares line over the whole range put its intercept above the \
             one-entry scan it was meant to bound, and full_range_fit keeps that on the record",
        )
        .note(
            "engines interleaved at the innermost level, one store per engine built once and \
             swept repeatedly, entry budget held constant across lengths",
        );

    let payload = Payload::new(value_size, 0.5, 0xE3);
    let root = scratch("sweep");
    let mut engines: Vec<Box<dyn Engine>> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for name in which {
        let Some(mut e) = build(&root, &[name], 256).into_iter().next() else {
            continue;
        };
        let mut vrng = Rng::new(0xE3);
        let mut kb = [0u8; 16];
        let mut buf = Batch::with_capacity(batch, payload.value_size());
        for i in 0..n {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == batch {
                buf.flush(e.as_mut()).expect("load");
            }
        }
        if !buf.is_empty() {
            buf.flush(e.as_mut()).expect("load");
        }
        e.sync().expect("sync");
        engines.push(e);
        names.push(name);
    }

    // ns per scan, indexed [engine][len], one Samples per pair.
    let mut per: Vec<Vec<Samples>> = names
        .iter()
        .map(|_| lens.iter().map(|_| Samples::default()).collect())
        .collect();
    let warmup = 1usize;
    for rep in 0..(warmup + reps) {
        for (li, len) in lens.iter().enumerate() {
            let scans = (budget / *len as u64).max(1);
            for (ei, e) in engines.iter_mut().enumerate() {
                let mut g = KeyGen::new(
                    KeyDist::Uniform,
                    n.saturating_sub(*len as u64).max(1),
                    0xE3 + rep as u64,
                );
                let mut kb = [0u8; 16];
                let t = Instant::now();
                for _ in 0..scans {
                    db_key_into(g.next(), &mut kb);
                    let _ = e.range(&kb, *len).expect("range");
                }
                if rep >= warmup {
                    per[ei][li].push(t.elapsed().as_secs_f64() * 1e9 / scans as f64);
                }
            }
        }
    }

    // The earlier version of this experiment fitted ns_per_scan = a + b*n by
    // least squares over the whole range 1..400 and reported both coefficients
    // as quantities. The model is testable and it is false: the marginal cost
    // of one more entry falls from about 89ns to about 15 before it settles
    // near 20, so a straight line through the whole curve lands its intercept
    // ABOVE the measured cost of a one-entry scan -- 952ns of "fixed cost" for
    // a scan observed to finish in 692, and 812 against 665 for LMDB. A
    // constant greater than the floor it claims to be is not a constant, and
    // two engines' versions of it are not a comparison.
    //
    // Both quantities are measurable without the model, so measure them:
    //
    //   floor    the cost of the shortest scan the sweep performs, observed at
    //            n=1. What an engine pays before anyone asks for a second entry.
    //   walk     the cost of one more entry at the top of the range, as the
    //            difference quotient between the last two lengths. What an
    //            entry costs once the per-scan work is amortised away.
    //
    // The whole-range fit is kept in the record as a diagnostic, so the reason
    // it was abandoned stays visible rather than only the fact of it.

    // A difference quotient at the top of the range is only a property of the
    // engine if the curve has stopped bending by then. So measure the last two
    // quotients as distributions and put them through the same gate as every
    // other comparison here: if they are distinguishable, the sweep did not
    // reach the regime it is trying to describe and there is no marginal cost
    // to report. The first version of this check compared the two medians
    // against a hand-picked 10% -- a hand-rolled comparison of exactly the kind
    // `stats::compare` exists to stop, and on this data it was reading noise as
    // curvature: LMDB's tail quotients run 22.4, 21.7, 23.7 ns/entry, bouncing
    // either side of settled rather than climbing towards it.
    let last = lens.len() - 1;
    let quotient =
        |ys: &[f64], hi: usize| -> f64 { (ys[hi] - ys[hi - 1]) / (lens[hi] - lens[hi - 1]) as f64 };

    let mut floor: Vec<Samples> = names.iter().map(|_| Samples::default()).collect();
    let mut walk: Vec<Samples> = names.iter().map(|_| Samples::default()).collect();
    let mut below: Vec<Samples> = names.iter().map(|_| Samples::default()).collect();
    let mut settled: Vec<Comparison> = Vec::with_capacity(names.len());
    let mut full_fit: Vec<(f64, f64)> = vec![(0.0, 0.0); names.len()];
    for ei in 0..names.len() {
        let med: Vec<f64> = (0..lens.len()).map(|li| per[ei][li].median()).collect();
        let all: Vec<f64> = lens.iter().map(|l| *l as f64).collect();
        full_fit[ei] = supdb::bench::stats::affine_fit(&all, &med);
        for r in 0..reps {
            let ys: Vec<f64> = (0..lens.len()).map(|li| per[ei][li].values[r]).collect();
            floor[ei].push(ys[0]);
            walk[ei].push(quotient(&ys, last));
            below[ei].push(quotient(&ys, last - 1));
        }
        settled.push(compare(&below[ei], &walk[ei], supdb::bench::MIN_EFFECT));
    }

    let mut rows = Vec::new();
    for (ei, name) in names.iter().enumerate() {
        let points: Vec<J> = lens
            .iter()
            .enumerate()
            .map(|(li, len)| {
                let ns = per[ei][li].median();
                jobj! {
                    "n" => J::u(*len as u64),
                    "ns_per_scan" => J::fp(ns, 1),
                    "ns_per_entry" => J::fp(ns / *len as f64, 2),
                    "entries_per_s" => J::fp(*len as f64 * 1e9 / ns.max(1e-9), 1)
                }
            })
            .collect();
        let (fa, fb) = full_fit[ei];
        println!(
            "  {name:6} floor {:>8.0} ns/scan   per-entry {:>6.2} ns at n={}   ({})",
            floor[ei].median(),
            walk[ei].median(),
            lens[last],
            if matches!(settled[ei].verdict, Verdict::NoDifference) {
                "settled"
            } else {
                "STILL BENDING"
            }
        );
        rows.push(jobj! {
            "engine" => J::s(*name),
            "floor_ns" => J::fp(floor[ei].median(), 1),
            "floor" => floor[ei].to_json(),
            "per_entry_ns" => J::fp(walk[ei].median(), 3),
            "per_entry" => walk[ei].to_json(),
            "per_entry_measured_over" => J::s(format!("n={}..{}", lens[last - 1], lens[last])),
            "settled" => settled[ei].to_json(),
            // Why the whole-range fit was dropped, kept as evidence rather
            // than as a claim: an intercept this far above the measured floor
            // cannot be a per-scan constant.
            "full_range_fit" => jobj! {
                "fixed_ns" => J::fp(fa, 1),
                "per_entry_ns" => J::fp(fb, 3),
                "intercept_over_measured_floor_ns" => J::fp(fa - floor[ei].median(), 1)
            },
            "points" => J::arr(points)
        });
    }
    rec.series("sweep", J::arr(rows));

    let idx = |name: &str| names.iter().position(|w| *w == name);
    if let (Some(s), Some(l)) = (idx("supdb"), idx("lmdb")) {
        // Lower is better for both quantities, so the comparisons are the
        // other way round from a throughput one.
        let walk_cmp = compare(&walk[l], &walk[s], supdb::bench::MIN_EFFECT);
        let floor_cmp = compare(&floor[l], &floor[s], supdb::bench::MIN_EFFECT);
        rec.compare("walk_lmdb_vs_supdb", walk_cmp.clone());
        rec.compare("floor_lmdb_vs_supdb", floor_cmp.clone());

        let unsettled = [s, l]
            .iter()
            .filter(|ei| !matches!(settled[**ei].verdict, Verdict::NoDifference))
            .map(|ei| format!("{} ({})", names[*ei], settled[*ei].summary("below", "top")))
            .collect::<Vec<_>>();
        if unsettled.is_empty() {
            rec.finding(Finding::new(
                "EXT.7",
                "Supdb walks a scan with less work per entry than LMDB",
                matches!(walk_cmp.verdict, Verdict::Greater),
                format!(
                    "supdb {:.2} ns/entry against lmdb {:.2} ({}), measured as the difference \
                     quotient between scans of {} and {} entries rather than fitted. The walk is \
                     bounded by memory bandwidth rather than by structure, so this is the half of \
                     a scan where there is little left to win. The fit this replaces put the \
                     slope at {:.2} against {:.2}, so on this coefficient it was close -- it was \
                     the intercept the straight line destroyed, not the slope",
                    walk[s].median(),
                    walk[l].median(),
                    walk_cmp.summary("lmdb", "supdb"),
                    lens[last - 1],
                    lens[last],
                    full_fit[s].1,
                    full_fit[l].1
                ),
            ));
        } else {
            rec.finding(Finding::not_exercised(
                "EXT.7",
                "Supdb walks a scan with less work per entry than LMDB",
                format!(
                    "the cost curve has not stopped bending by n={}: the marginal cost over \
                     {}..{} is still distinguishable from the one over {}..{} for {}. A marginal \
                     cost taken where the curve is still turning is a property of the sweep's \
                     range rather than of the engine, so this run declines to report one",
                    lens[last],
                    lens[last - 1],
                    lens[last],
                    lens[last - 2],
                    lens[last - 1],
                    unsettled.join(" and ")
                ),
            ));
        }

        rec.finding(Finding::new(
            "EXT.8",
            "Supdb pays no more fixed cost per scan than LMDB",
            // Lower is better, so this holds when LMDB's floor is the greater
            // one or the two cannot be told apart. The first version of this
            // line accepted `Less` as well, which is LMDB winning, and it duly
            // reported a hold on a run where Supdb was 1.24x worse.
            matches!(floor_cmp.verdict, Verdict::Greater | Verdict::NoDifference),
            format!(
                "supdb {:.0} ns against lmdb {:.0} ({}). This is the observed cost of a \
                 one-entry scan, not a fitted intercept: what an engine pays before anyone asks \
                 it for a second entry. For Supdb that is the seek plus resolving the first \
                 block; for LMDB and redb it was, until this commit, opening a read transaction \
                 per call. The fitted version of this number read {:.0}ns against {:.0} -- each \
                 above the one-entry scan it was supposed to bound, which is how a straight line \
                 through a bent curve reports a floor that no measurement ever touched",
                floor[s].median(),
                floor[l].median(),
                floor_cmp.summary("lmdb", "supdb"),
                full_fit[s].0,
                full_fit[l].0
            ),
        ));
    }
    Ok(rec)
}

/// YCSB core workloads (Cooper et al., SoCC'10).
fn suite_ycsb(args: &Args, profile: Profile, which: &[&str]) -> std::io::Result<Record> {
    let n = args.num("--keys", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let ops = args.num("--ops", profile.pick(20_000, 200_000, 1_000_000)) as u64;
    let value_size = args.num("--value-size", 100);
    let batch = args.num("--batch", 100);
    // Fewer repetitions than the kv suite: each is a fresh load of `n`
    // records per engine per workload, and the gate wants at least three.
    let reps = args.num("--reps", profile.pick(2, 3, 5));

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
        .param("reps", J::u(reps as u64))
        .note("YCSB core workloads A-F; Zipfian theta 0.99 as in the original")
        .note(
            "engines interleaved round-robin over reps within each workload, a fresh load per \
             rep, medians reported and every pair gated on stats::compare. It ran each engine \
             once until it did not; the matched pairs below are the ones that rank",
        )
        .note(
            "read the unmatched rows against the feature table: LMDB commits durably on every \
             batch where Supdb buffers and publishes without an fsync, so the mixed workloads \
             across those two compare an engine that promises power-loss durability against \
             one that does not. The next and RocksDB arms commit durably per batch",
        );

    let payload = Payload::new(value_size, 0.5, 0xE2);
    let mut rows = Vec::new();
    // Per workload, per engine: the samples the pairs are gated on.
    let mut per_workload: Vec<Vec<Samples>> = Vec::new();
    let mut feats: Vec<Option<Features>> = vec![None; which.len()];

    for (wname, pread, pupd, pscan, prmw, dist) in workloads {
        let root = scratch(&format!("ycsb-{wname}"));
        let hists: std::sync::Mutex<Vec<Option<(Hist, f64)>>> =
            std::sync::Mutex::new(vec![None; which.len()]);
        let featc: std::sync::Mutex<Vec<Option<Features>>> =
            std::sync::Mutex::new(vec![None; which.len()]);
        // The engine's own name for the row, as every other suite records it.
        let names: std::sync::Mutex<Vec<&'static str>> =
            std::sync::Mutex::new(vec![""; which.len()]);
        let rates = Trial::new(reps).run(which.len(), |ci, rep| {
            let dir = root.join(format!("{}-{rep}", which[ci]));
            let _ = std::fs::remove_dir_all(&dir);
            let Some(mut e) = build(&dir, &[which[ci]], 256).into_iter().next() else {
                return f64::NAN;
            };
            featc.lock().unwrap()[ci] = Some(e.features());
            names.lock().unwrap()[ci] = e.name();
            let mut vrng = Rng::new(0xE2);
            let mut kb = [0u8; 16];

            // Load phase.
            let mut buf = Batch::with_capacity(batch, payload.value_size());
            for i in 0..n {
                db_key_into(i, &mut kb);
                buf.push(&kb, payload.get(&mut vrng));
                if buf.len() == batch {
                    buf.flush(e.as_mut()).expect("load");
                }
            }
            if !buf.is_empty() {
                buf.flush(e.as_mut()).expect("load");
            }
            e.sync().expect("sync");

            // Transaction phase.
            let mut g = KeyGen::new(*dist, n, 0x9C5B);
            let mut pick = Rng::new(0x5EED);
            let mut h = Hist::new();
            let mut wbuf = Batch::with_capacity(batch, payload.value_size());
            let t = Instant::now();
            for _ in 0..ops {
                let roll = (pick.next() % 100) as u32;
                db_key_into(g.next(), &mut kb);
                let t1 = Instant::now();
                if roll < *pread {
                    let _ = e.get(&kb).expect("read");
                } else if roll < pread + pupd {
                    wbuf.push(&kb, payload.get(&mut vrng));
                    if wbuf.len() >= batch {
                        wbuf.flush_updates(e.as_mut()).expect("update");
                    }
                } else if roll < pread + pupd + pscan {
                    let _ = e.range(&kb, 50).expect("scan");
                } else if *prmw > 0 {
                    let _ = e.get(&kb).expect("rmw read");
                    wbuf.push(&kb, payload.get(&mut vrng));
                    if wbuf.len() >= batch {
                        wbuf.flush_updates(e.as_mut()).expect("rmw write");
                    }
                }
                h.record(t1.elapsed().as_nanos() as u64);
            }
            if !wbuf.is_empty() {
                wbuf.flush_updates(e.as_mut()).expect("tail");
            }
            let secs = t.elapsed().as_secs_f64();
            let size_mb = e.size_bytes() as f64 / 1048576.0;
            drop(e);
            let _ = std::fs::remove_dir_all(&dir);
            hists.lock().unwrap()[ci] = Some((h, size_mb));
            ops as f64 / secs
        });
        let hists = hists.into_inner().unwrap();
        let featc = featc.into_inner().unwrap();
        let names = names.into_inner().unwrap();
        for (ci, name) in names.iter().enumerate() {
            let Some((h, size_mb)) = &hists[ci] else { continue };
            if feats[ci].is_none() {
                feats[ci] = featc[ci];
            }
            rows.push(jobj! {
                "workload" => J::s(*wname),
                "engine" => J::s(*name),
                "distribution" => J::s(dist.as_str()),
                "ops_per_s" => J::fp(rates[ci].median(), 1),
                "rel_iqr" => J::fp(rates[ci].rel_iqr(), 4),
                "latency" => h.to_json(),
                "size_mb" => J::fp(*size_mb, 2),
                "feature_score" => J::u(featc[ci].map(|f| f.score() as u64).unwrap_or(0)),
            });
            println!(
                "  {wname:22} {name:14} {:>10.0} ops/s  p99 {:>8.3} ms",
                rates[ci].median(),
                h.percentile(99.0) as f64 / 1e6
            );
        }
        per_workload.push(rates);
    }
    rec.series("workloads", J::arr(rows.clone()));

    // The finding this suite existed for. A mixed read/write workload is
    // the shape no benchmark in the design document contains, and Supdb's
    // snapshot read model used to have to checkpoint and rebuild a reader to
    // serve one. `Store::read_all` removed that, so the ratio this reports is
    // now the cost of the write itself rather than the cost of publishing it.
    let idx = |name: &str| which.iter().position(|w| *w == name);
    let wl = |prefix: char| workloads.iter().position(|w| w.0.starts_with(prefix));
    if let (Some(si), Some(a), Some(c)) = (idx("supdb"), wl('A'), wl('C')) {
        let (a, c) = (per_workload[a][si].median(), per_workload[c][si].median());
        if a.is_finite() && c.is_finite() {
            rec.finding(Finding::new(
                "EXT.3",
                "Supdb sustains a mixed read/write workload within 10x of a read-only one",
                c / a.max(1e-9) < 10.0,
                format!(
                    "YCSB-A (50/50) {a:.0} ops/s against YCSB-C (100% read) {c:.0} ops/s -> \
                     {:.1}x. A read after a write is served from the writer's own state; when \
                     this ratio was 13.5x it needed a checkpoint and a fresh Reader, both O(key \
                     count)",
                    c / a.max(1e-9)
                ),
            ));
        }
    }

    // The matched pairs: the next engine, undrained after its load as
    // RocksDB is, against RocksDB tuned as deployed. Both commit durably
    // per batch, both apply a batch whole, neither verifies checksums on
    // read; `Features::unmatched` refuses the ordering if that ever stops
    // being so. One claim per workload that has a distinct shape.
    let pairs: [(&str, char, &str); 4] = [
        ("EXT.42", 'A', "an update-heavy mix (YCSB-A)"),
        ("EXT.43", 'C', "a read-only Zipfian workload (YCSB-C)"),
        ("EXT.44", 'E', "short scans with inserts (YCSB-E)"),
        ("EXT.45", 'F', "read-modify-write (YCSB-F)"),
    ];
    for (id, w, what) in pairs {
        let title = format!("the next engine sustains {what} at least as fast as tuned RocksDB");
        let (Some(wi), Some(ni), Some(ri)) = (wl(w), idx("next-nodrain"), idx("rocksdb-tuned"))
        else {
            continue;
        };
        let (a, b) = (&per_workload[wi][ni], &per_workload[wi][ri]);
        if a.is_empty() || b.is_empty() || !a.median().is_finite() || !b.median().is_finite() {
            continue;
        }
        let (Some(fa), Some(fb)) = (feats[ni], feats[ri]) else { continue };
        let gap = fa.unmatched(&fb, true);
        if !gap.is_empty() {
            rec.finding(Finding::not_exercised(
                id,
                &title,
                format!("not an ordering: the arms differ on {}", gap.join(", ")),
            ));
            continue;
        }
        let cmp = compare(a, b, supdb::bench::MIN_EFFECT);
        rec.compare(&format!("{id}_next-nodrain_vs_rocksdb-tuned"), cmp.clone());
        rec.finding(Finding::new(
            id,
            &title,
            !matches!(cmp.verdict, Verdict::Less),
            format!(
                "{:.0} ops/s against {:.0} ({}), {ops} operations over {n} records in \
                 {batch}-record batches, each batch durable",
                a.median(),
                b.median(),
                cmp.summary("next-nodrain", "rocksdb-tuned")
            ),
        ));
    }
    Ok(rec)
}

// ------------------------------------------------------------- analytics --

/// logshed's term key shape: field name, '=', eight zero-padded digits, so
/// the dictionary sorts the way a scan wants it. Copied from
/// `src/bin/logshed.rs` rather than imported, because that file is a binary.
fn term_key(field: &str, i: usize, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(field.as_bytes());
    out.push(b'=');
    let mut buf = [0u8; 8];
    let mut v = i;
    for slot in buf.iter_mut().rev() {
        *slot = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out.extend_from_slice(&buf);
}

/// logshed's zipf: u^2 concentrates mass at the head without needing a
/// table. `status=200` takes most of the traffic and the tail is nearly
/// empty, and that skew is the shape q1's ranking exists to answer over.
fn zipf_pick(rng: &mut Rng, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let u = rng.unit();
    let i = (u * u * n as f64) as usize;
    i.min(n - 1)
}

/// Fixed-capacity top-N accumulator. Both engines' q1 arms feed this same
/// struct, so everything outside the engine -- the compare, the occasional
/// key copy when a candidate enters -- costs both sides identically.
struct TopN {
    cap: usize,
    entries: Vec<(u64, Vec<u8>)>,
    min: u64,
}

impl TopN {
    fn new(cap: usize) -> TopN {
        TopN {
            cap,
            entries: Vec::with_capacity(cap),
            min: 0,
        }
    }
    fn reset(&mut self) {
        self.entries.clear();
        self.min = 0;
    }
    fn offer(&mut self, key: &[u8], count: u64) {
        if self.entries.len() < self.cap {
            self.entries.push((count, key.to_vec()));
            if self.entries.len() == self.cap {
                self.min = self.entries.iter().map(|e| e.0).min().unwrap_or(0);
            }
            return;
        }
        if count <= self.min {
            return;
        }
        let i = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.0)
            .map(|(i, _)| i)
            .expect("capacity is nonzero");
        let slot = &mut self.entries[i];
        slot.0 = count;
        // Reuse the evicted entry's buffer rather than allocating.
        slot.1.clear();
        slot.1.extend_from_slice(key);
        self.min = self.entries.iter().map(|e| e.0).min().unwrap_or(0);
    }
    fn counts_sorted(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.entries.iter().map(|e| e.0).collect();
        v.sort_unstable();
        v
    }
    fn sum(&self) -> u64 {
        self.entries.iter().map(|e| e.0).sum()
    }
}

/// Decode one key's postings into a reused buffer, through the shipped read
/// path. The buffer amortises to no allocation per value; the decode itself
/// is the cost q4's finding is about.
fn decode_postings(blob: &supdb::Blob<supdb::MmapBytes>, key: &[u8], out: &mut Vec<u32>) {
    out.clear();
    blob.read_all(key, |v| {
        out.push(u32::from_be_bytes(v.try_into().expect("4-byte posting")));
    })
    .expect("read_all");
}

/// q4's comparison arm, and deliberately the naive one: decode both lists in
/// full, then count matches with a two-pointer walk. It is what an application
/// wrote before `Blob::intersect_fixed` existed, and it stays in the checksums-on
/// arm so the kernel is priced against it in the same process rather than
/// against a memory.
fn naive_merge(
    blob: &supdb::Blob<supdb::MmapBytes>,
    ka: &[u8],
    kb: &[u8],
    bufa: &mut Vec<u32>,
    bufb: &mut Vec<u32>,
) -> u64 {
    decode_postings(blob, ka, bufa);
    decode_postings(blob, kb, bufb);
    intersect_sorted(bufa, bufb)
}

fn intersect_sorted(a: &[u32], b: &[u32]) -> u64 {
    let (mut i, mut j, mut n) = (0usize, 0usize, 0u64);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    n
}

/// The day-index scorecard: Supdb's analytics read paths against LMDB's
/// genuinely best shape for the same data.
///
/// W2.2 (`count_fixed`, 27.1x) and W2.4 (`scan_counts_fixed`, 283x) are the
/// flashiest numbers in this repository, and both were measured against
/// Supdb's own varint walk. That establishes the fixed-width arithmetic
/// beats the general answer *inside this engine* and says nothing about the
/// field. LMDB's best for a posting list is `MDB_DUPSORT|MDB_DUPFIXED`:
/// packed fixed-width dups, a stored per-key count behind
/// `mdb_cursor_count`, a page of postings per `MDB_GET_MULTIPLE` call. This
/// suite runs the two against each other so the numbers either become
/// cross-engine claims or get retired; an expected loss is recorded as
/// permanently as a win.
///
/// Four queries, each engine doing it the best way it can through shipped
/// read paths:
///
///   q1  rank the whole dictionary by posting count, top-N out.
///       Supdb: `scan_counts_fixed`. LMDB: NEXT_NODUP + cursor_count.
///   q2  the count of one key, many probes.
///       Supdb: `count_fixed`. LMDB: MDB_SET + cursor_count.
///   q3  read every posting of one key -- the baseline that keeps q1 and q2
///       honest, and the one DUPFIXED is genuinely built for.
///   q4  intersect two keys' posting lists. Supdb's arm is the naive
///       decode-both merge and the finding says so; LMDB merges in place
///       across GET_MULTIPLE pages.
///
/// The dataset is one synthetic day in logshed's shape (`src/bin/logshed.rs`):
/// two fields of zipf-skewed terms, one 4-byte line-ordinal posting per field
/// per line, appended grouped by term because W1.3 showed the naive roll
/// costs 22.6x the file. ~2,000 term keys and ~1M postings at `full`.
///
/// Read-only over immutable stores built once and probed repeatedly, so
/// every number is warm, like ext-sweep's -- EXT.12 owns cold -- and
/// durability does not bind. The checksum axis does: supdb-nocksum is built
/// without checksums and read without verification, which is the read-side
/// counterpart of ext-kv's supdb-buffered arm, because LMDB has none to turn
/// on. Plain supdb (checksums on, the shipping default) is recorded beside
/// it and gates nothing.
fn suite_analytics(args: &Args, profile: Profile) -> std::io::Result<Record> {
    const WIDTH: usize = 4;
    let lines = args.num("--lines", profile.pick(20_000, 150_000, 500_000)) as u64;
    let fields: [(&str, usize); 2] = [("path", 1600), ("ua", 400)];
    let top_n = args.num("--top-n", 10);
    let rank_budget = args.num("--rank-keys", profile.pick(100_000, 1_000_000, 4_000_000)) as u64;
    let count_probes = args.num("--count-probes", profile.pick(20_000, 100_000, 500_000)) as u64;
    let read_probes = args.num("--read-probes", profile.pick(2_000, 20_000, 60_000)) as u64;
    let pairs = args.num("--pairs", profile.pick(1_000, 5_000, 30_000)) as u64;
    let reps = args.num("--reps", profile.reps());

    let mut rec = Record::new("ext-analytics", profile);
    rec.param("lines", J::u(lines))
        .param("fields", J::s("path:1600, ua:400"))
        .param("value_width", J::u(WIDTH as u64))
        .param("top_n", J::u(top_n as u64))
        .param("rank_key_budget", J::u(rank_budget))
        .param("count_probes", J::u(count_probes))
        .param("read_probes", J::u(read_probes))
        .param("pairs", J::u(pairs))
        .param("reps", J::u(reps as u64))
        .note(
            "one synthetic day in logshed's shape: per line, one 4-byte line-ordinal posting \
             under a zipf-picked term of each field, appended grouped by term (W1.3). Postings \
             are big-endian here where logshed writes little-endian: Supdb never compares value \
             bytes so it costs Supdb nothing, and it makes LMDB's dup comparator agree with \
             numeric order, so both engines walk ascending lists and the intersection needs no \
             comparator shim",
        )
        .note(
            "read-only over immutable stores built once and probed repeatedly: every number is \
             warm, like ext-sweep's, and EXT.12 owns cold. Durability does not bind on a read; \
             the checksum axis does, and supdb-nocksum -- built without checksums, read without \
             verification, the read-side counterpart of ext-kv's supdb-buffered -- is the \
             matched arm for every claim, since LMDB has none to turn on. Plain supdb is \
             recorded beside it and gates nothing",
        )
        .note(
            "engines and queries interleaved round-robin over reps, one warmup discarded, every \
             ordering gated on stats::compare. Before anything is timed, all three read paths \
             must agree with the generator on every key's count, on sampled posting sums, on \
             sampled intersections and on the top-N, so the arms are provably answering the \
             same question",
        )
        .note(
            "q4's matched arm (supdb-nocksum) is Blob::intersect_fixed, a two-pointer walk over \
             the two keys' fixed runs in place; the checksums-on arm keeps the naive merge -- \
             read_all both lists into reused buffers, then a two-pointer count -- so the kernel \
             is priced against the application-side merge in the same process. Values are \
             4-byte postings, so every run is written fixed-width (format v6) and neither \
             arm decodes a length prefix",
        );

    // ---- one day's postings, generated once, identical for every engine ----
    //
    // (field, term, line) packed into one u64 and sorted, exactly as
    // logshed's Order::Term roll does: one sort puts every term's postings
    // together, ascending by line within a term, and the pack order is also
    // the keys' lexicographic order ("path=" < "ua=", digits zero-padded).
    let mut rng = Rng::new(0xDA7);
    let mut recs: Vec<u64> = Vec::with_capacity((lines as usize) * fields.len());
    for line in 0..lines {
        for (f, (_, card)) in fields.iter().enumerate() {
            let i = zipf_pick(&mut rng, *card);
            recs.push(((f as u64) << 56) | ((i as u64) << 32) | line);
        }
    }
    recs.sort_unstable();

    let mut dict: Vec<Vec<u8>> = Vec::new();
    let mut counts: Vec<u64> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut postings: Vec<u32> = Vec::with_capacity(recs.len());
    let mut cur = u64::MAX;
    for p in &recs {
        let head = p >> 32;
        if head != cur {
            cur = head;
            let (f, i) = ((head >> 24) as usize, (head & 0xff_ffff) as usize);
            let mut k = Vec::with_capacity(16);
            term_key(fields[f].0, i, &mut k);
            dict.push(k);
            counts.push(0);
            starts.push(postings.len());
        }
        *counts.last_mut().expect("a key was just pushed") += 1;
        postings.push(*p as u32);
    }
    starts.push(postings.len());
    drop(recs);
    let dict_len = dict.len();
    let a_keys = dict.iter().take_while(|k| k.starts_with(b"path=")).count();
    assert!(
        a_keys > 0 && a_keys < dict_len,
        "both fields must be present for q4 to intersect across them"
    );

    // ---- build all three stores from the same stream ----
    //
    // `Options::checksums` is a process-global set by `Store::create`, so the
    // no-checksum file is built FIRST and the checksummed one second: the
    // global is then still on when the checksummed arm reads, and the nocksum
    // arm opts out per-reader with `BlobOptions::verify_checksums`.
    let root = scratch("analytics");
    let build_store = |path: &std::path::Path, checksums: bool| {
        let store = supdb::Store::create(
            path,
            supdb::Options {
                buffer_bytes: 256 << 20,
                checksums,
                ..Default::default()
            },
        )
        .expect("create");
        for (i, key) in dict.iter().enumerate() {
            for p in &postings[starts[i]..starts[i + 1]] {
                store.append(key, &p.to_be_bytes()).expect("append");
            }
        }
        store.checkpoint().expect("checkpoint");
        store.close().expect("close");
    };
    let nock_path = root.join("supdb-nocksum.dat");
    build_store(&nock_path, false);
    let ck_path = root.join("supdb.dat");
    build_store(&ck_path, true);

    let mut ldb = LmdbDup::create(&root.join("lmdb-dup"), 8).expect("lmdb-dup create");
    ldb.begin_load().expect("begin_load");
    for (i, key) in dict.iter().enumerate() {
        for p in &postings[starts[i]..starts[i + 1]] {
            ldb.put(key, &p.to_be_bytes()).expect("put");
        }
    }
    ldb.end_load().expect("end_load");

    let blob =
        supdb::Blob::open(supdb::MmapBytes::open(&ck_path).expect("map")).expect("blob open");
    let nock = supdb::Blob::open_with(
        supdb::MmapBytes::open(&nock_path).expect("map"),
        supdb::BlobOptions {
            verify_checksums: false,
            verify_index: false,
            ..Default::default()
        },
    )
    .expect("blob open");
    assert!(blob.zero_copy(), "the native arm must not be copying");
    assert!(nock.zero_copy(), "the native arm must not be copying");

    // ---- the differential check that makes the ranking mean something ----
    //
    // All three read paths against the generator, before any of them is
    // timed: every key's count, posting sums on a sample plus the smallest
    // and largest keys (the smallest exercises LMDB's single-inline-dup
    // page path), intersections across the fields, and the top-N multiset.
    // A benchmark over engines that disagree is not a benchmark.
    for (i, key) in dict.iter().enumerate() {
        assert_eq!(
            blob.count_fixed(key, WIDTH as u32),
            Some(counts[i]),
            "supdb count for key {i}"
        );
        assert_eq!(
            nock.count_fixed(key, WIDTH as u32),
            Some(counts[i]),
            "supdb-nocksum count for key {i}"
        );
        assert_eq!(
            ldb.count(key).expect("lmdb count"),
            counts[i],
            "lmdb-dup count for key {i}"
        );
    }
    let truth_sum = |i: usize| -> u64 {
        postings[starts[i]..starts[i + 1]]
            .iter()
            .map(|p| *p as u64)
            .sum()
    };
    let min_i = (0..dict_len).min_by_key(|i| counts[*i]).expect("nonempty");
    let max_i = (0..dict_len).max_by_key(|i| counts[*i]).expect("nonempty");
    let mut sample: Vec<usize> = (0..dict_len).step_by((dict_len / 29).max(1)).collect();
    sample.push(min_i);
    sample.push(max_i);
    for i in sample {
        let key = &dict[i];
        let mut s1 = 0u64;
        blob.read_all(key, |v| {
            s1 += u32::from_be_bytes(v.try_into().expect("4-byte posting")) as u64;
        })
        .expect("read_all");
        let mut s2 = 0u64;
        ldb.read_postings(key, |page| {
            for c in page.chunks_exact(WIDTH) {
                s2 += u32::from_be_bytes(c.try_into().expect("stride")) as u64;
            }
        })
        .expect("read_postings");
        assert_eq!(s1, truth_sum(i), "supdb posting sum for key {i}");
        assert_eq!(s2, truth_sum(i), "lmdb-dup posting sum for key {i}");
    }
    let (mut bufa, mut bufb): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
    for ai in [0, a_keys / 2, a_keys - 1] {
        for bi in [a_keys, a_keys + (dict_len - a_keys) / 2, dict_len - 1] {
            let want = intersect_sorted(
                &postings[starts[ai]..starts[ai + 1]],
                &postings[starts[bi]..starts[bi + 1]],
            );
            let got = naive_merge(&blob, &dict[ai], &dict[bi], &mut bufa, &mut bufb);
            assert_eq!(got, want, "supdb intersection {ai}x{bi}");
            let kernel = nock.intersect_fixed(&dict[ai], &dict[bi], WIDTH).expect("kernel");
            assert_eq!(kernel, want, "supdb in-place intersection {ai}x{bi}");
            let got = ldb
                .intersect_fixed(&dict[ai], &dict[bi], WIDTH)
                .expect("intersect");
            assert_eq!(got, want, "lmdb-dup intersection {ai}x{bi}");
        }
    }
    let mut want_top: Vec<u64> = counts.clone();
    want_top.sort_unstable();
    let want_top: Vec<u64> = want_top.into_iter().rev().take(top_n).rev().collect();
    let mut topn = TopN::new(top_n);
    blob.scan_counts_fixed(b"", dict_len, WIDTH as u32, |k, n| {
        topn.offer(k, n.expect("fixed-width by construction"));
        true
    })
    .expect("scan_counts_fixed");
    assert_eq!(topn.counts_sorted(), want_top, "supdb top-N");
    topn.reset();
    let visited = ldb.rank_pass(|k, n| topn.offer(k, n)).expect("rank_pass");
    assert_eq!(visited as usize, dict_len, "lmdb-dup dictionary size");
    assert_eq!(topn.counts_sorted(), want_top, "lmdb-dup top-N");

    // ---- the measured arms: 3 engines x 4 queries, interleaved ----
    let arm = ["supdb", "supdb-nocksum", "lmdb-dup"];
    let qname = ["q1-rank", "q2-count", "q3-read", "q4-intersect"];
    let unit = ["keys/s", "probes/s", "postings/s", "pairs/s"];
    let rank_passes = (rank_budget / dict_len as u64).max(1);
    rec.param("rank_passes_per_sample", J::u(rank_passes));

    let rates = Trial::new(reps).run(12, |ci, rep| {
        let (qi, ei) = (ci / 3, ci % 3);
        match qi {
            // q1: rank the whole dictionary, top-N maintained identically.
            0 => {
                let t = Instant::now();
                let mut sink = 0u64;
                for _ in 0..rank_passes {
                    topn.reset();
                    match ei {
                        0 | 1 => {
                            let b = if ei == 0 { &blob } else { &nock };
                            b.scan_counts_fixed(b"", dict_len, WIDTH as u32, |k, n| {
                                topn.offer(k, n.expect("fixed-width by construction"));
                                true
                            })
                            .expect("scan_counts_fixed");
                        }
                        _ => {
                            ldb.rank_pass(|k, n| topn.offer(k, n)).expect("rank_pass");
                        }
                    }
                    sink += topn.sum();
                }
                std::hint::black_box(sink);
                (rank_passes * dict_len as u64) as f64 / t.elapsed().as_secs_f64()
            }
            // q2: one key's count, uniform probes.
            1 => {
                let mut r = Rng::new(0xC0 + rep as u64);
                let t = Instant::now();
                let mut sink = 0u64;
                for _ in 0..count_probes {
                    let k = &dict[r.below(dict_len as u64) as usize];
                    sink += match ei {
                        0 => blob.count_fixed(k, WIDTH as u32).expect("fixed"),
                        1 => nock.count_fixed(k, WIDTH as u32).expect("fixed"),
                        _ => ldb.count(k).expect("count"),
                    };
                }
                std::hint::black_box(sink);
                count_probes as f64 / t.elapsed().as_secs_f64()
            }
            // q3: every posting under one key, uniform probes; the rate is
            // postings visited per second, and the probe sequence is
            // identical across arms so the visits are too.
            2 => {
                let mut r = Rng::new(0xD0 + rep as u64);
                let t = Instant::now();
                let mut sum = 0u64;
                let mut seen = 0u64;
                for _ in 0..read_probes {
                    let k = &dict[r.below(dict_len as u64) as usize];
                    match ei {
                        0 | 1 => {
                            let b = if ei == 0 { &blob } else { &nock };
                            seen += b
                                .read_all(k, |v| {
                                    sum = sum.wrapping_add(u32::from_be_bytes(
                                        v.try_into().expect("4-byte posting"),
                                    )
                                        as u64);
                                })
                                .expect("read_all");
                        }
                        _ => {
                            let bytes = ldb
                                .read_postings(k, |page| {
                                    for c in page.chunks_exact(WIDTH) {
                                        sum = sum.wrapping_add(u32::from_be_bytes(
                                            c.try_into().expect("stride"),
                                        )
                                            as u64);
                                    }
                                })
                                .expect("read_postings");
                            seen += bytes / WIDTH as u64;
                        }
                    }
                }
                std::hint::black_box(sum);
                seen as f64 / t.elapsed().as_secs_f64()
            }
            // q4: intersect one key from each field.
            _ => {
                let mut r = Rng::new(0xE0 + rep as u64);
                let t = Instant::now();
                let mut matches = 0u64;
                for _ in 0..pairs {
                    let ka = &dict[r.below(a_keys as u64) as usize];
                    let kb = &dict[a_keys + r.below((dict_len - a_keys) as u64) as usize];
                    matches += match ei {
                        // The checksums-on arm keeps the naive merge -- decode
                        // both lists, then walk -- as the comparison; the
                        // matched arm uses the in-place kernel over fixed runs.
                        0 => naive_merge(&blob, ka, kb, &mut bufa, &mut bufb),
                        1 => nock.intersect_fixed(ka, kb, WIDTH).expect("intersect"),
                        _ => ldb.intersect_fixed(ka, kb, WIDTH).expect("intersect"),
                    };
                }
                std::hint::black_box(matches);
                pairs as f64 / t.elapsed().as_secs_f64()
            }
        }
    });

    // ---- report ----
    let ns = |s: &Samples| 1e9 / s.median().max(1e-9);
    let mut rows = Vec::new();
    for qi in 0..4 {
        for ei in 0..3 {
            let s = &rates[qi * 3 + ei];
            println!(
                "  {:12} {:14} {:>13.0} {:11}  ({:>9.1} ns/unit)",
                qname[qi],
                arm[ei],
                s.median(),
                unit[qi],
                ns(s)
            );
            rows.push(jobj! {
                "engine" => J::s(arm[ei]),
                "query" => J::s(qname[qi]),
                "unit" => J::s(unit[qi]),
                "per_s" => J::fp(s.median(), 1),
                "ns_per_unit" => J::fp(ns(s), 2),
                "rel_iqr" => J::fp(s.rel_iqr(), 4),
                "samples" => s.to_json()
            });
        }
    }
    rec.series("arms", J::arr(rows));

    // The supdb rows mirror the `Supdb` adapter's features with the checksum
    // axis split across the two arms; the store behind both is durably
    // checkpointed at build, and durable=false below says no metric here
    // touches the write path anyway.
    let sup_feats = Features {
        durable_commit: true,
        transactions: false,
        checksums: true,
        reopen_for_write: true,
        read_your_writes: true,
        ordered_scan: true,
    };
    let nock_feats = Features {
        checksums: false,
        ..sup_feats
    };
    let dup_feats = ldb.features();
    let feats = [sup_feats, nock_feats, dup_feats];
    rec.series(
        "features",
        J::arr(
            arm.iter()
                .zip(feats.iter())
                .map(|(name, f)| {
                    jobj! {
                        "engine" => J::s(*name),
                        "features" => f.to_json(),
                        "feature_score" => J::u(f.score() as u64)
                    }
                })
                .collect(),
        ),
    );

    let mut med_sorted: Vec<u64> = counts.clone();
    med_sorted.sort_unstable();
    rec.series(
        "dataset",
        jobj! {
            "keys" => J::u(dict_len as u64),
            "keys_path" => J::u(a_keys as u64),
            "keys_ua" => J::u((dict_len - a_keys) as u64),
            "postings" => J::u(postings.len() as u64),
            "min_postings_per_key" => J::u(med_sorted[0]),
            "median_postings_per_key" => J::u(med_sorted[dict_len / 2]),
            "max_postings_per_key" => J::u(med_sorted[dict_len - 1]),
            "supdb_file_mb" => J::fp(
                std::fs::metadata(&ck_path).map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0, 2),
            "supdb_nocksum_file_mb" => J::fp(
                std::fs::metadata(&nock_path).map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0, 2),
            "lmdb_dup_mb" => J::fp(ldb.size_bytes() as f64 / 1048576.0, 2)
        },
    );

    // What verification costs on each query, engine against itself. q1 and
    // q2 touch no block, so their pairs double as a null check on the rig.
    for qi in 0..4 {
        rec.compare(
            &format!("{}_checksums_off_vs_on", qname[qi]),
            compare(&rates[qi * 3 + 1], &rates[qi * 3], supdb::bench::MIN_EFFECT),
        );
    }

    // ---- the claims, gated on the matched pair ----
    let gap = nock_feats.unmatched(&dup_feats, false);
    let titles = [
        (
            "EXT.15",
            "Supdb ranks a day's whole term dictionary faster than LMDB's best shape counts it",
        ),
        (
            "EXT.16",
            "Supdb answers a single term's posting count faster than LMDB's stored dup count",
        ),
        (
            "EXT.18",
            "Supdb reads a full posting list as fast as LMDB's page-at-a-time DUPFIXED reads",
        ),
        (
            "EXT.17",
            "Supdb intersects two terms' posting lists faster than LMDB walks its dup lists",
        ),
    ];
    if !gap.is_empty() {
        for (id, title) in titles {
            rec.finding(Finding::not_exercised(
                id,
                title,
                format!(
                    "not an ordering: supdb-nocksum and lmdb-dup do not promise the same thing \
                     on {}, and each of those could have been equalized",
                    gap.join(", ")
                ),
            ));
        }
        return Ok(rec);
    }
    let residual = ". lmdb-dup is still transactional and Supdb is not, which no configuration \
                    can equalize, so read a win as a bound that is not yet a win and a loss as \
                    at least that large";
    let mk = |qi: usize| {
        let c = compare(
            &rates[qi * 3 + 1],
            &rates[qi * 3 + 2],
            supdb::bench::MIN_EFFECT,
        );
        (matches!(c.verdict, Verdict::Greater), c)
    };

    let (h, c) = mk(0);
    rec.compare("EXT.15_supdb-nocksum_vs_lmdb-dup", c.clone());
    rec.finding(Finding::new(
        "EXT.15",
        titles[0].1,
        h,
        format!(
            "supdb-nocksum ranks the {dict_len}-key dictionary at {:.1} ns/key against \
             lmdb-dup's {:.1} ({}), {rank_passes} whole-dictionary passes per sample, top-{top_n} \
             maintained by the same accumulator in both arms. W2.4's 283x was scan_counts_fixed \
             against Supdb's own varint walk; this is the same walk against LMDB's best shape -- \
             a NEXT_NODUP step plus mdb_cursor_count per key, a count the dup tree stores rather \
             than computes. Supdb's arm is O(extents) arithmetic on the mapped index and touches \
             no block{residual}",
            ns(&rates[1]),
            ns(&rates[2]),
            c.summary("supdb-nocksum", "lmdb-dup")
        ),
    ));

    let (h, c) = mk(1);
    rec.compare("EXT.16_supdb-nocksum_vs_lmdb-dup", c.clone());
    rec.finding(Finding::new(
        "EXT.16",
        titles[1].1,
        h,
        format!(
            "count_fixed answers a point count in {:.1} ns/probe against MDB_SET plus \
             mdb_cursor_count's {:.1} ({}), uniform probes over the dictionary. W2.2's 27.1x was \
             count_fixed against Supdb's own O(values) walk; this is it against an engine that \
             stores the count -- which is exactly the format change W2.3 priced at 14.9 ns for \
             Supdb and declined. Whichever way this ordering reads, it is the cross-engine \
             price of that decision{residual}",
            ns(&rates[4]),
            ns(&rates[5]),
            c.summary("supdb-nocksum", "lmdb-dup")
        ),
    ));

    let (_, c) = mk(2);
    rec.compare("EXT.18_supdb-nocksum_vs_lmdb-dup", c.clone());
    rec.finding(Finding::new(
        "EXT.18",
        titles[2].1,
        // "As fast as" holds on a tie: this is the baseline LMDB is built
        // for, and the claim is parity, not a lead.
        matches!(c.verdict, Verdict::Greater | Verdict::NoDifference),
        format!(
            "supdb-nocksum reads postings at {:.2} ns/posting against lmdb-dup's {:.2} ({}), \
             uniform probes, identical probe sequences, the rate counted in postings visited. \
             This is the baseline that keeps q1 and q2 honest, and the shape DUPFIXED is \
             genuinely built for: 4-byte postings packed end to end, a page per GET_MULTIPLE \
             call, no per-value work at all. Since format v6 a run of one width is stored the \
             same way -- no length prefix, a 4-byte stride -- and the read is a memcpy-shaped \
             walk over the extent rather than the serial dependent decode W2.1 documented. \
             Claimed as parity, not a lead: holds on Greater or NoDifference{residual}",
            ns(&rates[7]),
            ns(&rates[8]),
            c.summary("supdb-nocksum", "lmdb-dup")
        ),
    ));

    let (h, c) = mk(3);
    rec.compare("EXT.17_supdb-nocksum_vs_lmdb-dup", c.clone());
    rec.finding(Finding::new(
        "EXT.17",
        titles[3].1,
        h,
        format!(
            "supdb-nocksum intersects at {:.1} us/pair against lmdb-dup's {:.1} ({}), each pair \
             one key from each field, both engines walking the same ascending lists. Supdb's \
             matched arm is Blob::intersect_fixed: a two-pointer walk over both keys' fixed \
             runs in place, comparing 4-byte values as big-endian integers, copying nothing. \
             LMDB merges in place across GET_MULTIPLE pages. The checksums-on arm keeps the \
             naive decode-both merge at {:.1} us/pair as the price of doing it application-side{residual}",
            ns(&rates[10]) / 1e3,
            ns(&rates[11]) / 1e3,
            c.summary("supdb-nocksum", "lmdb-dup"),
            ns(&rates[9]) / 1e3
        ),
    ));

    drop(blob);
    drop(nock);
    drop(ldb);
    let _ = std::fs::remove_dir_all(&root);
    Ok(rec)
}
