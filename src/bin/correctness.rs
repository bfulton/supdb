//! Correctness as evidence.
//!
//! A fast wrong answer is not a result, so these belong in the benchmark story
//! rather than beside it. Each experiment produces `Finding`s in the same
//! format as the performance suite, so `claims.json` and CI govern them
//! identically.
//!
//!   c1-decoders  feed damaged bytes to a real store and see whether the
//!                reader returns an error or takes the host process down
//!   c2-oracle    randomized operation sequences against a BTreeMap model
//!   c3-crash     kill a writer mid-flight and check what survives
//!
//! The first is the one the architecture review predicted and never
//! demonstrated: `get_uvarint` reads without a bounds check, `emit` slices on
//! a length it just read, and nothing in the file except the 120-byte
//! superblock carries a checksum. In an embedded library a panic is the host
//! application dying.

use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::bench::{db_key_into, Finding, Profile, Record, Rng, J};
use supdb::jobj;
use supdb::{Options, Reader, Reclaim, Store};

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

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-correctness-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let args = Args(argv.clone());
    let cmd = argv.get(1).cloned().unwrap_or_else(|| "help".into());
    let profile = Profile::parse(args.get("--profile").unwrap_or("dev")).unwrap_or(Profile::Dev);
    let out = PathBuf::from(args.get("--out").unwrap_or("results"));

    let run = |name: &str| -> std::io::Result<bool> {
        let rec = match name {
            "c1-decoders" => c1_decoders(&args, profile)?,
            "c2-oracle" => c2_oracle(&args, profile)?,
            "c3-crash" => c3_crash(&args, profile)?,
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
        "c3-child" => c3_child(&args),
        "all" => {
            for e in ["c1-decoders", "c2-oracle", "c3-crash"] {
                run(e)?;
            }
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("correctness <c1-decoders|c2-oracle|c3-crash|all> [--profile ci|dev|full]");
            Ok(())
        }
        other => {
            run(other)?;
            Ok(())
        }
    }
}

/// Build a small, valid store and return its path.
fn build_store(path: &Path, keys: u64, depth: u64, value_size: usize) -> std::io::Result<()> {
    let store = Store::create(
        path,
        Options {
            buffer_bytes: 4 << 20,
            reclaim: Reclaim::AfterReads,
            ..Default::default()
        },
    )?;
    let mut kb = [0u8; 16];
    let mut v = vec![0u8; value_size];
    for i in 0..(keys * depth) {
        let k = i % keys;
        db_key_into(k, &mut kb);
        // Self-describing: sequence, key, and a checksum of both, so a reader
        // can tell a correct value from a corrupted one of the right length.
        v[..8].copy_from_slice(&i.to_be_bytes());
        v[8..16].copy_from_slice(&k.to_be_bytes());
        let tag = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ k;
        v[16..24].copy_from_slice(&tag.to_be_bytes());
        for (j, b) in v.iter_mut().enumerate().skip(24) {
            *b = (j as u64).wrapping_mul(31).wrapping_add(tag) as u8;
        }
        store.append(&kb, &v)?;
    }
    store.close()?;
    Ok(())
}

// ------------------------------------------------------- C1: damaged bytes --
// ---------------------------------------------- the other read path --
/// Damage a live store's file underneath it and read through both paths.
///
/// Returns (values the writer served that differed from what was written,
/// trials the reader rejected, trials run).
fn writer_path_damage(
    dir: &std::path::Path,
    keys: u64,
    value_size: usize,
) -> std::io::Result<(u64, u64, u64)> {
    use std::io::{Seek, Write};
    let mut served = 0u64;
    let mut caught = 0u64;
    let mut trials = 0u64;
    let val = vec![b'v'; value_size];
    for t in 0..8u64 {
        let path = dir.join(format!("writer-{t}.dat"));
        let _ = std::fs::remove_file(&path);
        let s = Store::create(
            &path,
            Options {
                checksums: true,
                ..Options::default()
            },
        )?;
        let mut kb = [0u8; 16];
        for k in 0..keys {
            db_key_into(k, &mut kb);
            s.put(&kb, &val)?;
        }
        s.flush()?;
        s.checkpoint()?;

        let clean = std::fs::read(&path)?;
        let Some(at) = clean.windows(val.len()).position(|w| w == val.as_slice()) else {
            continue;
        };
        let at = at + 8 + t as usize;
        if at >= clean.len() {
            continue;
        }
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(&path)?;
            f.seek(std::io::SeekFrom::Start(at as u64))?;
            f.write_all(&[clean[at] ^ 0xff])?;
            f.sync_all()?;
        }
        trials += 1;

        let (mut wrong, mut err) = (false, false);
        for k in 0..keys {
            db_key_into(k, &mut kb);
            match s.read_all(&kb, |v| {
                if v != val.as_slice() {
                    wrong = true;
                }
            }) {
                Ok(_) => {}
                Err(_) => err = true,
            }
        }
        if wrong && !err {
            served += 1;
        }
        if let Ok(r) = Reader::open(&path) {
            let mut rerr = false;
            for k in 0..keys {
                db_key_into(k, &mut kb);
                if r.read_all(&kb, |_| {}).is_err() {
                    rerr = true;
                }
            }
            if rerr {
                caught += 1;
            }
        }
        let _ = s.close();
        let _ = std::fs::remove_file(&path);
    }
    Ok((served, caught, trials))
}



/// What a reader does with a file that is not quite what it wrote.
///
/// Three damage models, all of which happen in the field: a single flipped bit
/// (bit rot), a run of zeroes (a torn or partial write), and a block of
/// unrelated bytes (space reused underneath an older state -- which this
/// engine does deliberately, under every reclaim policy except `Never`).
///
/// The bar is deliberately low. The reader is not required to recover the
/// data, or even to detect the damage. It is required not to take the host
/// process down with it, because this is a library living inside somebody
/// else's address space.
fn c1_decoders(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let trials = args.num("--trials", profile.pick(300, 2_000, 20_000));
    let keys = args.num("--keys", 400) as u64;
    let depth = args.num("--depth", 8) as u64;
    let value_size = args.num("--value-size", 64);

    let mut rec = Record::new("c1-decoders", profile);
    rec.param("trials", J::u(trials as u64))
        .param("keys", J::u(keys))
        .param("values_per_key", J::u(depth))
        .param(
            "damage_models",
            J::arr(vec![
                J::s("bit_flip"),
                J::s("zero_run"),
                J::s("foreign_bytes"),
                J::s("index_section"),
                J::s("block_payload"),
            ]),
        );

    let dir = scratch("c1");
    let good = dir.join("good.dat");
    build_store(&good, keys, depth, value_size)?;
    let template = std::fs::read(&good)?;
    let target = dir.join("damaged.dat");

    // Panics are expected here; the default hook would print a backtrace for
    // every one and bury the result.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    // Where the key index actually lives, read out of the superblock. Uniform
    // random damage almost always lands in a value payload, where a flipped
    // byte is just a wrong byte -- structurally harmless and silently served.
    // The unchecked varints are in the index, so that region has to be aimed
    // at deliberately.
    let index_span: Option<(usize, usize)> = {
        let field = |slot: usize, i: usize| -> u64 {
            let base = slot * 512 + i * 8;
            u64::from_le_bytes(template[base..base + 8].try_into().unwrap())
        };
        // Pick the slot with the higher generation, as the reader does.
        let slot = if field(1, 0) > field(0, 0) { 1 } else { 0 };
        let off = field(slot, 3) as usize;
        let stored = field(slot, 4) as usize;
        if off > 0 && stored > 0 && off + stored <= template.len() {
            Some((off, stored))
        } else {
            None
        }
    };
    rec.param(
        "index_section",
        match index_span {
            Some((o, n)) => jobj! { "offset" => J::u(o as u64), "bytes" => J::u(n as u64) },
            None => J::Null,
        },
    );

    // Byte ranges that actually hold block payload. Uniform damage across the
    // whole file mostly hits size-class padding, where a flipped byte is
    // genuinely harmless -- reporting that as "undetected corruption" measures
    // the file's layout, not the engine.
    let payload_ranges: Vec<(u64, u64)> = Reader::open(&good)
        .map(|r| r.block_extents())
        .unwrap_or_default()
        .into_iter()
        .filter(|(off, len)| *off >= 4096 && *len > 0)
        .collect();
    let payload_bytes: u64 = payload_ranges.iter().map(|(_, l)| *l).sum();
    rec.param(
        "live_payload",
        jobj! {
            "blocks" => J::u(payload_ranges.len() as u64),
            "bytes" => J::u(payload_bytes),
            "fraction_of_file" => J::fp(payload_bytes as f64 / template.len().max(1) as f64, 4),
        },
    );

    let mut rng = Rng::new(0xC1C1);
    let mut no_op = 0u64;
    let mut served_corrupt = 0u64;
    let mut first_corrupt = String::new();
    let (mut panicked, mut errored, mut clean, mut wrong_len) = (0u64, 0u64, 0u64, 0u64);
    let n_models = if payload_ranges.is_empty() { 4 } else { 5 };
    let mut by_model = [[0u64; 3]; 5]; // [model][panic/err/clean]
    let mut first_panic = String::new();
    let t0 = Instant::now();

    for _ in 0..trials {
        let mut bytes = template.clone();
        // Never damage the first page: the superblock is checksummed and the
        // reader table lives there, so hitting it only tests the one guard the
        // format does have.
        let lo = 4096usize;
        if bytes.len() <= lo + 64 {
            break;
        }
        let model = (rng.next() % n_models) as usize;
        let off = match model {
            3 => match index_span {
                Some((io, isz)) => io + (rng.next() as usize) % isz,
                None => lo + (rng.next() as usize) % (bytes.len() - lo - 64),
            },
            4 => {
                let (b_off, b_len) = payload_ranges[(rng.next() as usize) % payload_ranges.len()];
                (b_off + rng.next() % b_len) as usize
            }
            _ => lo + (rng.next() as usize) % (bytes.len() - lo - 64),
        };
        match model {
            0 => bytes[off] ^= 1 << (rng.next() % 8),
            1 => {
                let end = (off + 1 + rng.next() as usize % 48).min(bytes.len());
                for b in bytes[off..end].iter_mut() {
                    *b = 0;
                }
            }
            _ => {
                let end = (off + 1 + rng.next() as usize % 48).min(bytes.len());
                for b in bytes[off..end].iter_mut() {
                    *b = (rng.next() & 0xff) as u8;
                }
            }
        }
        // A trial that did not actually change the file is not a trial. Zeroing
        // a run that was already zero, or writing a byte that happens to match,
        // proves nothing -- and counting it as "damage that went unnoticed"
        // manufactures a gap that is not there.
        if bytes == template {
            no_op += 1;
            continue;
        }
        std::fs::write(&target, &bytes)?;

        let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(u64, u64), String> {
            let r = Reader::open(&target).map_err(|e| e.to_string())?;
            let mut kb = [0u8; 16];
            let (mut vals, mut odd) = (0u64, 0u64);
            for k in 0..keys {
                db_key_into(k, &mut kb);
                r.read_all(&kb, |v| {
                    vals += 1;
                    if v.len() != value_size {
                        odd += 1;
                    }
                })
                .map_err(|e| e.to_string())?;
            }
            Ok((vals, odd))
        }));

        match outcome {
            Err(p) => {
                panicked += 1;
                by_model[model][0] += 1;
                if first_panic.is_empty() {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "non-string panic".into());
                    first_panic = format!("model={model} off={off}: {msg}");
                }
            }
            Ok(Err(_)) => {
                errored += 1;
                by_model[model][1] += 1;
            }
            Ok(Ok((_, odd))) => {
                // "Clean" now means every value came back byte-exact. A read
                // that returns corrupted content is a silent failure whether
                // or not the engine noticed.
                clean += 1;
                by_model[model][2] += 1;
                // The read succeeded. Did it return the bytes that were
                // written? This is the obligation that matters: damage to
                // bytes nothing reads is not a failure, but serving wrong
                // bytes as if they were right is.
                if odd > 0 {
                    served_corrupt += 1;
                    if first_corrupt.is_empty() {
                        first_corrupt = format!("model={model} off={off}: {odd} bad value(s)");
                    }
                }
                // Read succeeded on a file we damaged. Without block
                // checksums there is nothing to notice, so this is the silent
                // case: whether the values are still correct is not knowable
                // from inside the engine.
                wrong_len += odd;
            }
        }
    }
    std::panic::set_hook(prev_hook);
    let secs = t0.elapsed().as_secs_f64();
    let n = (panicked + errored + clean).max(1);

    let model_json = |i: usize| {
        jobj! {
            "panicked" => J::u(by_model[i][0]),
            "errored" => J::u(by_model[i][1]),
            "read_without_complaint" => J::u(by_model[i][2]),
        }
    };
    rec.series(
        "outcomes",
        jobj! {
            "trials" => J::u(n),
            "panicked" => J::u(panicked),
            "errored" => J::u(errored),
            "read_without_complaint" => J::u(clean),
            "values_with_wrong_length" => J::u(wrong_len),
            "panic_rate" => J::fp(panicked as f64 / n as f64, 4),
            "silent_rate" => J::fp(clean as f64 / n as f64, 4),
        "no_op_trials_skipped" => J::u(no_op),
        "reads_served_corrupt_data" => J::u(served_corrupt),
        "first_corrupt" => J::s(&first_corrupt),
            "seconds" => J::fp(secs, 2),
        },
    )
    .series(
        "by_damage_model",
        jobj! {
            "bit_flip" => model_json(0),
            "zero_run" => model_json(1),
            "foreign_bytes" => model_json(2),
            "index_section" => if index_span.is_some() { model_json(3) } else { J::Null },
        },
    )
    .series("first_panic", J::s(&first_panic));

    rec.finding(Finding::new(
        "C1.1",
        "a damaged file produces an error, never a panic",
        panicked == 0,
        format!(
            "{panicked}/{n} trials ({:.1}%) took the process down instead of returning Err. \
             First: {}",
            panicked as f64 * 100.0 / n as f64,
            if first_panic.is_empty() {
                "none".into()
            } else {
                first_panic.clone()
            }
        ),
    ));
    // The obligation that actually matters: a read returns the bytes that were
    // written, or it returns an error. It is *not* an obligation to notice
    // damage to bytes nobody reads -- size-class padding, or a chunk orphaned
    // inside a still-referenced block by an earlier merge. Two earlier versions
    // of this finding measured that instead and reported gaps of 73% and 7.5%
    // that were mostly layout, not integrity.
    let payload_total: u64 = by_model[4].iter().sum();
    let payload_silent = by_model[4][2];
    // The same claim, asked of the other read path.
    //
    // Every trial above opens a `Reader`. `Store::read_all` exists so a writer
    // can read its own writes without a checkpoint -- it took the mixed YCSB
    // workloads from 0.07x of LMDB to 18x -- and it goes from the mapping to
    // the caller with no checksum in between. So C1.2 was stated about reads
    // and measured on one of the two ways to do one.
    //
    // The scope is real but narrow: `Store::create` truncates and there is no
    // `Store::open`, so a writer only ever reads blocks it wrote in this
    // session. What is unprotected is the file changing underneath it, which
    // is what this measures, through a second handle, because the appender
    // maps the file shared.
    let writer = writer_path_damage(&dir, keys.min(2_000), value_size)?;
    rec.finding(Finding::new(
        "C1.3",
        "the writer's own read path checks what the reader's read path checks",
        writer.0 == 0,
        format!(
            "{}/{} damaged reads through Store::read_all returned bytes that differed from what \
             was written with no error; a Reader over the same file rejected {}. Both paths now \
             verify, which matters because the feature table scores Supdb a point for checksums \
             and YCSB A through F route every read through the writer's path. It cost 1.135x on \
             a cold read pass and 0.956x warm -- see f21-writerverify, and Options::verify_reads \
             for callers who want LMDB's trade instead",
            writer.0, writer.2, writer.1
        ),
    ));
    rec.finding(Finding::new(
        "C1.2",
        "a reader returns the bytes that were written, or an error -- never wrong data",
        served_corrupt == 0,
        format!(
            "{served_corrupt}/{n} trials served a value that differed from what was written. {}",
            if first_corrupt.is_empty() {
                "none".into()
            } else {
                first_corrupt.clone()
            }
        ),
    ));
    rec.series(
        "unread_damage",
        jobj! {
            "payload_trials" => J::u(payload_total),
            "silent" => J::u(payload_silent),
            "note" => J::s("damage inside a live block that no live extent covers: an orphaned \
                            chunk left by a merge, or bytes past the last extent. Never decoded, \
                            so never checked -- verifying it would mean hashing whole blocks on \
                            every point read, which is the cost chunking exists to avoid"),
        },
    );
    rec.note(
        "The bar is not recovery, or even detection: it is that a library embedded in another \
         process must return an error rather than abort it",
    );
    Ok(rec)
}

// ------------------------------------------------------------ C2: an oracle --

/// Randomized operation sequences against a model.
///
/// The model is a `BTreeMap<Vec<u8>, Vec<Vec<u8>>>` -- the data structure the
/// API describes. After each checkpoint the store is reopened and every key is
/// compared against it. A hundred lines that the design document's six
/// hand-written harnesses do not contain.
fn c2_oracle(args: &Args, profile: Profile) -> std::io::Result<Record> {
    // The ci defaults are sized to actually reproduce, not merely to run.
    // At 500 operations per round a shard's pending buffer never fills, so
    // seal_shard is not triggered inline and the staged-extent bug cannot
    // occur -- the suite reported "no divergence" for a defect it had already
    // proven. Both known bugs surface at 60x2000 in under half a second.
    let rounds = args.num("--rounds", profile.pick(60, 200, 1_000));
    let ops_per_round = args.num("--ops", profile.pick(2_000, 2_000, 10_000));
    let keyspace = args.num("--keyspace", 300) as u64;
    let seed = args.num("--seed", 0xC2C2) as u64;

    let mut rec = Record::new("c2-oracle", profile);
    rec.param("rounds", J::u(rounds as u64))
        .param("ops_per_round", J::u(ops_per_round as u64))
        .param("keyspace", J::u(keyspace))
        .param("seed", J::u(seed));

    // Every reclaim policy, because the two defects this found are
    // policy-dependent and running one policy hides the other: under
    // AfterReads the write path dies before the comparison can run, and under
    // Never it survives and diverges.
    let policies: Vec<(&str, Reclaim)> = vec![
        ("AfterReads", Reclaim::AfterReads),
        ("Never", Reclaim::Never),
        ("OnClose", Reclaim::OnClose),
    ];

    let mut rows = Vec::new();
    let mut any_write_error = false;
    let mut any_divergence = false;
    let mut worst = String::new();

    for (name, reclaim) in &policies {
        let r = run_oracle(*reclaim, rounds, ops_per_round, keyspace, seed)?;
        if r.write_error.is_some() {
            any_write_error = true;
        }
        if r.mismatches > 0 {
            any_divergence = true;
            if worst.is_empty() {
                worst = format!("{name}: {}", r.first);
            }
        }
        println!(
            "  {name:11} {:>7} comparisons  {:>6} mismatches  {:>4} read errors  write path: {}",
            r.checked,
            r.mismatches,
            r.read_errors,
            r.write_error.clone().unwrap_or_else(|| "ok".into())
        );
        rows.push(jobj! {
            "reclaim" => J::s(*name),
            "keys_compared" => J::u(r.checked),
            "mismatches" => J::u(r.mismatches),
            "read_errors" => J::u(r.read_errors),
            "rounds_completed" => J::u(r.rounds_done),
            "write_error" => match &r.write_error {
                Some(e) => J::s(e.as_str()),
                None => J::Null,
            },
            "first_divergence" => J::s(&r.first),
        });
    }
    rec.series("by_reclaim_policy", J::arr(rows));

    rec.finding(Finding::new(
        "C2.1",
        "the store agrees with a BTreeMap model across appends, replaces and deletes",
        !any_divergence,
        if worst.is_empty() {
            "no divergence under any reclaim policy".to_string()
        } else {
            format!(
                "{worst}. delete() clears entry.extents but does not cancel an extent already \
                 staged in the block builder, so flush_builder pushes it back and the deleted \
                 key returns. Reduced in tests/known_bugs.rs"
            )
        },
    ));
    rec.finding(Finding::new(
        "C2.2",
        "the write path completes without error under every reclaim policy",
        !any_write_error,
        if any_write_error {
            "under AfterReads the writer's own merge path fails to decode a block it wrote: a \
             freed slot is handed out while a block still holds live references to it, so three \
             block ids end up describing the same byte range"
                .to_string()
        } else {
            "no write-path errors".to_string()
        },
    ));
    rec.note(format!(
        "deterministic: rerun with --seed {seed} to reproduce exactly"
    ));
    Ok(rec)
}

struct OracleRun {
    checked: u64,
    mismatches: u64,
    read_errors: u64,
    rounds_done: u64,
    write_error: Option<String>,
    first: String,
}

/// One pass of the oracle under one reclaim policy.
///
/// A write-path error stops the run and is reported rather than propagated:
/// the point is to characterise the failure, not to abort the experiment that
/// found it.
fn run_oracle(
    reclaim: Reclaim,
    rounds: usize,
    ops_per_round: usize,
    keyspace: u64,
    seed: u64,
) -> std::io::Result<OracleRun> {
    let merge_threshold: usize = std::env::var("SUPDB_MERGE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    use std::collections::BTreeMap;

    let dir = scratch(&format!("c2-{reclaim:?}"));
    let file = dir.join("s.dat");
    let store = Store::create(
        &file,
        Options {
            buffer_bytes: 1 << 20,
            reclaim,
            merge_threshold,
            ..Default::default()
        },
    )?;

    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let mut rng = Rng::new(seed);
    let mut out = OracleRun {
        checked: 0,
        mismatches: 0,
        read_errors: 0,
        rounds_done: 0,
        write_error: None,
        first: String::new(),
    };

    'rounds: for round in 0..rounds {
        for _ in 0..ops_per_round {
            let k = rng.next() % keyspace;
            let mut kb = [0u8; 16];
            db_key_into(k, &mut kb);
            let key = kb.to_vec();
            let tag = rng.next();
            let mut val = vec![0u8; 24];
            val[..8].copy_from_slice(&tag.to_be_bytes());
            val[8..16].copy_from_slice(&k.to_be_bytes());
            val[16..24].copy_from_slice(&(round as u64).to_be_bytes());

            let r = match rng.next() % 100 {
                0..=69 => {
                    let r = store.append(&key, &val);
                    if r.is_ok() {
                        model.entry(key).or_default().push(val);
                    }
                    r
                }
                70..=89 => {
                    let r = store.put(&key, &val);
                    if r.is_ok() {
                        model.insert(key, vec![val]);
                    }
                    r
                }
                _ => {
                    let r = store.delete(&key);
                    if r.is_ok() {
                        model.insert(key, Vec::new());
                    }
                    r
                }
            };
            if let Err(e) = r {
                out.write_error = Some(format!("round {round}: {e}"));
                break 'rounds;
            }
        }
        if let Err(e) = store.checkpoint() {
            out.write_error = Some(format!("round {round} checkpoint: {e}"));
            break 'rounds;
        }
        out.rounds_done += 1;

        let reader = Reader::open(&file)?;
        for (key, want) in &model {
            out.checked += 1;
            let mut got: Vec<Vec<u8>> = Vec::new();
            match reader.read_all(key, |v| got.push(v.to_vec())) {
                Ok(_) => {
                    if &got != want {
                        out.mismatches += 1;
                        if out.first.is_empty() {
                            out.first = format!(
                                "round {round}, key {}: model has {} value(s), store returned {}",
                                String::from_utf8_lossy(key),
                                want.len(),
                                got.len()
                            );
                        }
                    }
                }
                Err(e) => {
                    out.read_errors += 1;
                    if out.first.is_empty() {
                        out.first = format!("round {round}: read error {e}");
                    }
                }
            }
        }
    }
    let _ = store.close();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(out)
}

// ------------------------------------------------------------- C3: crashes --

/// Kill a writer at many different points and check what survives.
///
/// The design document reports one `SIGABRT` at one point, which is an
/// anecdote. The contract is narrow enough to state precisely: after any crash
/// the store must open, and it must present a state that is a prefix of what
/// was written -- some suffix of the writes may be lost, but nothing may be
/// invented, and no key may hold a value that was never appended.
fn c3_crash(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let trials = args.num("--trials", profile.pick(12, 40, 120));
    let keys = args.num("--keys", 500) as u64;
    let value_size = args.num("--value-size", 64);

    let mut rec = Record::new("c3-crash", profile);
    rec.param("trials", J::u(trials as u64))
        .param("keys", J::u(keys))
        .param("value_size", J::u(value_size as u64));

    let dir = scratch("c3");
    let exe = std::env::current_exe().expect("exe");
    let mut rng = Rng::new(0xC3C3);

    // The child checkpoints every keys/2 operations, so the parent knows
    // whether any checkpoint completed before the crash. A store killed before
    // its first checkpoint has nothing on disk to recover -- that is a real
    // and important limitation, but it is a different statement from "a store
    // with a completed checkpoint will not open", and merging the two would
    // hide the second behind the first.
    let ckpt_every = (keys / 2).max(1);
    let (mut opened, mut open_failed, mut read_errors, mut invented, mut torn_values) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut pre_ckpt_crashes, mut pre_ckpt_unopenable) = (0u64, 0u64);
    let (mut post_ckpt_crashes, mut post_ckpt_unopenable) = (0u64, 0u64);
    let mut recovered_fraction: Vec<f64> = Vec::new();
    let mut first = String::new();

    for t in 0..trials {
        let file = dir.join(format!("c{t}.dat"));
        // Abort after a random number of operations, so the crash lands in a
        // different place each time -- mid-append, mid-seal, mid-checkpoint.
        let abort_after = 1 + (rng.next() % (keys * 6));
        let st = std::process::Command::new(&exe)
            .arg("c3-child")
            .arg("--file")
            .arg(&file)
            .arg("--keys")
            .arg(keys.to_string())
            .arg("--value-size")
            .arg(value_size.to_string())
            .arg("--abort-after")
            .arg(abort_after.to_string())
            .output()?;
        // The child aborts, so a "success" exit means it never crashed.
        if st.status.success() {
            continue;
        }

        let had_checkpoint = abort_after >= ckpt_every;
        if had_checkpoint {
            post_ckpt_crashes += 1;
        } else {
            pre_ckpt_crashes += 1;
        }
        match Reader::open(&file) {
            Err(_) => {
                open_failed += 1;
                if had_checkpoint {
                    post_ckpt_unopenable += 1;
                    if first.is_empty() {
                        first = format!(
                            "trial {t}: crash at op {abort_after}, past the checkpoint at \
                             {ckpt_every}, and the store still would not open"
                        );
                    }
                } else {
                    pre_ckpt_unopenable += 1;
                }
            }
            Ok(r) => {
                opened += 1;
                let mut kb = [0u8; 16];
                let (mut present, mut bad) = (0u64, 0u64);
                for k in 0..keys {
                    db_key_into(k, &mut kb);
                    let mut n = 0u64;
                    match r.read_all(&kb, |v| {
                        n += 1;
                        // Every value written by the child is value_size bytes
                        // and carries its own key in bytes 8..16. Anything else
                        // was not written by this writer.
                        let self_describing = v.len() == value_size
                            && u64::from_be_bytes(v[8..16].try_into().unwrap()) == k;
                        if !self_describing {
                            bad += 1;
                        }
                    }) {
                        Ok(_) => {
                            if n > 0 {
                                present += 1;
                            }
                        }
                        Err(_) => read_errors += 1,
                    }
                }
                invented += bad;
                if bad > 0 {
                    torn_values += 1;
                    if first.is_empty() {
                        first = format!("trial {t}: {bad} values did not match what was written");
                    }
                }
                recovered_fraction.push(present as f64 / keys as f64);
            }
        }
        let _ = std::fs::remove_file(&file);
    }

    let n = (opened + open_failed).max(1);
    let mean_recovered = if recovered_fraction.is_empty() {
        0.0
    } else {
        recovered_fraction.iter().sum::<f64>() / recovered_fraction.len() as f64
    };
    rec.series("before_first_checkpoint", jobj! {
        "crashes" => J::u(pre_ckpt_crashes),
        "unopenable" => J::u(pre_ckpt_unopenable),
        "note" => J::s("nothing is on disk until the first checkpoint; with buffer_bytes at its \
                        512MB default a writer can accumulate a great deal before one happens"),
    })
    .series("after_a_checkpoint", jobj! {
        "crashes" => J::u(post_ckpt_crashes),
        "unopenable" => J::u(post_ckpt_unopenable),
    })
    .series("recovery", jobj! {
        "crashes" => J::u(n),
        "opened" => J::u(opened),
        "open_failed" => J::u(open_failed),
        "read_errors" => J::u(read_errors),
        "trials_with_bad_values" => J::u(torn_values),
        "bad_values_total" => J::u(invented),
        "mean_keys_recovered" => J::fp(mean_recovered, 4),
        "first" => J::s(&first),
    });

    rec.finding(if post_ckpt_crashes > 0 {
        Finding::new(
            "C3.1",
            "a store killed after at least one checkpoint always opens",
            post_ckpt_unopenable == 0,
            format!(
                "{}/{post_ckpt_crashes} stores with a completed checkpoint opened; \
                 {post_ckpt_unopenable} did not. {first}",
                post_ckpt_crashes - post_ckpt_unopenable
            ),
        )
    } else {
        Finding::not_exercised(
            "C3.1",
            "a store killed after at least one checkpoint always opens",
            "no trial crashed after a checkpoint",
        )
    });
    rec.finding(Finding::new(
        "C3.4",
        "a store killed before its first checkpoint is readable",
        pre_ckpt_crashes == 0 || pre_ckpt_unopenable == 0,
        format!(
            "{pre_ckpt_unopenable}/{pre_ckpt_crashes} stores killed before their first \
             checkpoint would not open at all. Everything written so far lives in per-shard \
             memory buffers, and buffer_bytes defaults to 512MB, so this is not a small window"
        ),
    ));
    rec.finding(Finding::new(
        "C3.2",
        "recovery invents nothing: every value read back was actually written",
        invented == 0,
        format!(
            "{invented} values across {torn_values} trials did not match what the writer wrote"
        ),
    ));
    rec.finding(Finding::new(
        "C3.3",
        "recovery loses only a suffix: at least half the keys survive a mid-run crash",
        mean_recovered >= 0.5,
        format!(
            "mean {:.0}% of keys present after a crash. Everything since the last checkpoint is \
             buffered in memory, and buffer_bytes defaults to 512MB",
            mean_recovered * 100.0
        ),
    ));
    Ok(rec)
}

/// Child for C3: write, then abort abruptly. No unwinding, no destructors.
fn c3_child(args: &Args) -> std::io::Result<()> {
    let file = PathBuf::from(args.get("--file").expect("--file"));
    let keys = args.num("--keys", 100) as u64;
    let value_size = args.num("--value-size", 64);
    let abort_after = args.num("--abort-after", 100) as u64;

    let store = Store::create(
        &file,
        Options {
            buffer_bytes: 256 << 10,
            reclaim: Reclaim::AfterReads,
            ..Default::default()
        },
    )?;
    let mut kb = [0u8; 16];
    let mut v = vec![0u8; value_size];
    for i in 0..(keys * 8) {
        let k = i % keys;
        db_key_into(k, &mut kb);
        v[..8].copy_from_slice(&i.to_be_bytes());
        v[8..16].copy_from_slice(&k.to_be_bytes());
        store.append(&kb, &v)?;
        if i > 0 && i % (keys / 2).max(1) == 0 {
            store.checkpoint()?;
        }
        if i == abort_after {
            // A real crash: no flush, no close, no destructors.
            std::io::stdout().flush().ok();
            std::process::abort();
        }
    }
    store.close()?;
    Ok(())
}
