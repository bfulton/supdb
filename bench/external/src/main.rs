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
    compare, db_key_into, Comparison, Finding, Hist, KeyDist, KeyGen, Payload, Profile, Record,
    Rng, Samples, Verdict, J,
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
                "external <kv|ycsb|sweep|all> [--profile ci|dev|full] [--engines supdb,redb,lmdb,sled]"
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
        let mut buf: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(batch);
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
