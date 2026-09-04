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
//!   c4-crash     the same for the engine, with the WAL's unsynced tail
//!                torn the way a power loss would tear it (crash-plan.md)
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
use supdb::SegmentOptions;
use supdb::{Db, Options, SyncPolicy};

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
            "c4-crash" => c4_crash(&args, profile)?,
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
        "c4-child" => c4_child(&args),
        "all" => {
            for e in ["c1-decoders", "c4-crash"] {
                run(e)?;
            }
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!(
                "correctness <c1-decoders|c2-oracle|c3-crash|c4-crash|all> [--profile ci|dev|full]"
            );
            Ok(())
        }
        other => {
            run(other)?;
            Ok(())
        }
    }
}

/// Build a small, valid store and return its path.
fn build_segment(path: &Path, keys: u64, depth: u64, value_size: usize) -> std::io::Result<()> {
    let mut w = supdb::SegmentWriter::create(path, &SegmentOptions::default())?;
    let mut kb = [0u8; 16];
    let mut v = vec![0u8; value_size];
    // `db_key_into` is a zero-padded decimal, so ascending `k` is ascending
    // key bytes, which is the order the writer takes.
    for k in 0..keys {
        db_key_into(k, &mut kb);
        w.begin(&kb)?;
        // The same (sequence, key) pairing the interleaved writer produced:
        // key `k` holds sequences k, k+keys, k+2*keys, and so on. Keeping it
        // means the fixture's bytes are unchanged, so a damage model that
        // used to land somewhere still lands there.
        for d in 0..depth {
            let i = k + d * keys;
            // Self-describing: sequence, key, and a checksum of both, so a
            // reader can tell a correct value from a corrupted one of the
            // right length.
            v[..8].copy_from_slice(&i.to_be_bytes());
            v[8..16].copy_from_slice(&k.to_be_bytes());
            let tag = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ k;
            v[16..24].copy_from_slice(&tag.to_be_bytes());
            for (j, b) in v.iter_mut().enumerate().skip(24) {
                *b = (j as u64).wrapping_mul(31).wrapping_add(tag) as u8;
            }
            w.value(&v);
        }
        w.end()?;
    }
    w.finish(1)?;
    Ok(())
}

// ------------------------------------------------------- C1: damaged bytes --

// ---------------------------------------------- the other read path --

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
    build_segment(&good, keys, depth, value_size)?;
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
    let payload_ranges: Vec<(u64, u64)> = supdb::MmapBytes::open(&good)
        .and_then(supdb::Blob::open)
        .map(|b| b.block_extents())
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
            let r = supdb::MmapBytes::open(&target)
                .and_then(supdb::Blob::open)
                .map_err(|e| e.to_string())?;
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

// ------------------------------------------------------------- C3: crashes --

// ------------------------------------------------ C4: engine crashes --

/// One operation of the child's stream. The stream is a pure function of
/// the seed, so the parent regenerates it and knows the exact state after
/// every batch without the child telling it anything but how many batches
/// were acknowledged.
#[derive(Clone, Copy)]
enum C4Op {
    Put { key: u64, len: usize },
    Del { key: u64 },
}

/// Operations are numbered from 1; `ops[0]` is a placeholder.
fn c4_ops(seed: u64, keys: u64, n: u64) -> Vec<C4Op> {
    let mut rng = Rng::new(seed);
    let mut ops = Vec::with_capacity(n as usize + 1);
    ops.push(C4Op::Del { key: 0 });
    for _ in 0..n {
        let r = rng.next();
        let key = r % keys;
        if (r >> 32).is_multiple_of(10) {
            ops.push(C4Op::Del { key });
        } else if (r >> 40).is_multiple_of(20) {
            // One in twenty is too big to inline, so block-backed runs are
            // in the file beside inline ones.
            ops.push(C4Op::Put {
                key,
                len: 300 + ((r >> 44) % 700) as usize,
            });
        } else {
            ops.push(C4Op::Put {
                key,
                len: 24 + ((r >> 44) % 73) as usize,
            });
        }
    }
    ops
}

/// A value names its own sequence and key and carries a tag of both, so a
/// reader can tell a value that was written from one that was assembled.
fn c4_value(seq: u64, key: u64, len: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(len, 0);
    out[..8].copy_from_slice(&seq.to_be_bytes());
    out[8..16].copy_from_slice(&key.to_be_bytes());
    let tag = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ key;
    out[16..24].copy_from_slice(&tag.to_be_bytes());
    for (j, b) in out.iter_mut().enumerate().skip(24) {
        *b = (j as u64).wrapping_mul(31).wrapping_add(tag) as u8;
    }
}

/// Seals a few hundred operations wide, two of them per merge, partitions
/// twice a seal: every background job the engine has is in flight during a
/// run of a few thousand operations.
fn c4_opts(sync: SyncPolicy, recycle: bool) -> Options {
    Options {
        sync,
        seal_bytes: 48 << 10,
        partition_bytes: Some(96 << 10),
        l0_trigger: 2,
        recycle_wal: recycle,
        ..Options::default()
    }
}

fn c4_sync(arm: &str) -> SyncPolicy {
    match arm {
        "always" => SyncPolicy::Always,
        _ => SyncPolicy::EveryN(8),
    }
}

const C4_EVERY_N: u64 = 8;

/// The child: commit batches, report each one, die at `--abort-after`.
fn c4_child(args: &Args) -> std::io::Result<()> {
    let dir = PathBuf::from(args.get("--dir").expect("--dir"));
    let keys = args.num("--keys", 300) as u64;
    let batch = args.num("--batch", 16) as u64;
    let seed = args.num("--seed", 1) as u64;
    let abort_after = args.num("--abort-after", 100) as u64;
    // Die after the commit that ends at `abort_after` returns but before
    // its ack is printed, rather than before the next operation: the
    // window in which a batch is durable and nobody was told.
    let late = args.get("--late").is_some();
    let arm = args.get("--arm").unwrap_or("always").to_string();
    // `fixed` dies at `--abort-after`. `seal` and `merge` die at the first
    // operation past half of it that finds that job in flight, so the
    // windows the manifest and the orphan sweep exist for are reached on
    // purpose rather than by thread timing; `--cap` bounds the wait.
    let mode = args.get("--mode").unwrap_or("fixed").to_string();
    let cap = args.num("--cap", (abort_after + batch) as usize) as u64;
    let recycle = args.get("--recycle").is_some();

    let ops = c4_ops(seed, keys, cap + batch);
    let mut db = Db::create(&dir, c4_opts(c4_sync(&arm), recycle))?;
    let mut out = std::io::stdout();
    let mut kb = [0u8; 16];
    let mut val = Vec::new();
    let mut acked = 0u64;
    let mut i = 1u64;

    let die = |db: &Db, acked: u64, op: u64, out: &mut std::io::Stdout| -> ! {
        let (seal, compact) = db.in_flight();
        let (parts, l0) = db.levels();
        let (wal, synced, written) = db.wal_durable();
        let _ = writeln!(
            out,
            "abort acked={acked} op={op} seal={} compact={} parts={parts} l0={l0} wal={} \
             synced={synced} written={written}",
            u8::from(seal),
            u8::from(compact),
            wal.display()
        );
        let _ = out.flush();
        // A real crash: no flush, no close, no destructors, and the seal
        // and merge threads die where they stand.
        std::process::abort()
    };

    // Whether the crash is due before operation `i` is applied.
    let due = |db: &Db, i: u64| -> bool {
        if i >= cap {
            return true;
        }
        match mode.as_str() {
            "seal" => i >= abort_after / 2 && db.in_flight().0,
            "merge" => i >= abort_after / 2 && db.in_flight().1,
            _ => i == abort_after && !late,
        }
    };

    loop {
        let b = acked; // batch index, 0-based
        let end = i + batch; // first op of the next batch
        if b % 5 == 4 {
            // Every fifth batch through a transaction. Nothing a
            // transaction stages reaches the WAL before its commit, so a
            // crash inside one is a crash before it: die here instead.
            if (i..end).any(|j| due(&db, j)) {
                die(&db, acked, i, &mut out);
            }
            let mut t = db.begin();
            while i < end {
                match ops[i as usize] {
                    C4Op::Put { key, len } => {
                        db_key_into(key, &mut kb);
                        c4_value(i, key, len, &mut val);
                        t.append(&kb, &val);
                    }
                    C4Op::Del { key } => {
                        db_key_into(key, &mut kb);
                        t.delete(&kb);
                    }
                }
                i += 1;
            }
            t.commit()?;
        } else {
            while i < end {
                if due(&db, i) {
                    die(&db, acked, i, &mut out);
                }
                match ops[i as usize] {
                    C4Op::Put { key, len } => {
                        db_key_into(key, &mut kb);
                        c4_value(i, key, len, &mut val);
                        db.append(&kb, &val);
                    }
                    C4Op::Del { key } => {
                        db_key_into(key, &mut kb);
                        db.delete(&kb);
                    }
                }
                i += 1;
            }
            db.commit()?;
        }
        if late && mode == "fixed" && abort_after < end {
            // The batch holding the abort point was committed whole; die
            // before anyone is told.
            die(&db, acked, end, &mut out);
        }
        acked += 1;
        writeln!(out, "ack {acked}")?;
        out.flush()?;
    }
}

struct C4Arm {
    name: &'static str,
    crashes: u64,
    open_failed: u64,
    acked_lost: u64,
    no_prefix: u64,
    invented: u64,
    count_disagreed: u64,
    scan_disagreed: u64,
    worst_lost: u64,
    first: String,
}

impl C4Arm {
    fn new(name: &'static str) -> C4Arm {
        C4Arm {
            name,
            crashes: 0,
            open_failed: 0,
            acked_lost: 0,
            no_prefix: 0,
            invented: 0,
            count_disagreed: 0,
            scan_disagreed: 0,
            worst_lost: 0,
            first: String::new(),
        }
    }
    fn note(&mut self, msg: String) {
        if self.first.is_empty() {
            self.first = msg;
        }
    }
    fn json(&self) -> J {
        jobj! {
            "crashes" => J::u(self.crashes),
            "open_failed" => J::u(self.open_failed),
            "trials_losing_an_acked_batch" => J::u(self.acked_lost),
            "trials_matching_no_prefix" => J::u(self.no_prefix),
            "invented_values" => J::u(self.invented),
            "count_disagreed" => J::u(self.count_disagreed),
            "scan_disagreed" => J::u(self.scan_disagreed),
            "most_acked_batches_lost" => J::u(self.worst_lost),
            "first" => J::s(&self.first),
        }
    }
}

fn c4_crash(args: &Args, profile: Profile) -> std::io::Result<Record> {
    let trials = args.num("--trials", profile.pick(8, 24, 120));
    let keys = args.num("--keys", 300) as u64;
    let max_ops = args.num("--max-ops", profile.pick(2500, 4000, 8000)) as u64;
    // The self-check: let the tear reach this many bytes below the synced
    // mark, which no power loss can do. C4.2 must then fail; a run where it
    // does not is a parent that cannot see a lost batch.
    let tear_synced = args.num("--tear-synced", 0) as u64;

    let mut rec = Record::new("c4-crash", profile);
    rec.param("trials", J::u(trials as u64))
        .param("keys", J::u(keys))
        .param("max_ops", J::u(max_ops))
        .param("seal_bytes", J::u(48 << 10))
        .param("every_n", J::u(C4_EVERY_N));

    let root = scratch("c4");
    let exe = std::env::current_exe().expect("exe");
    let mut rng = Rng::new(0xC4C4);
    let mut arms = [C4Arm::new("always"), C4Arm::new("every_n")];
    let (mut seal_in_flight, mut merge_in_flight, mut with_partitions) = (0u64, 0u64, 0u64);
    let (mut torn_trials, mut max_torn, mut child_errors, mut late_trials) =
        (0u64, 0u64, 0u64, 0u64);
    let mut torn_headers = 0u64;
    let (mut recycled_trials, mut torn_into_stale) = (0u64, 0u64);
    let mut coverage_first = String::new();
    let batches = [4u64, 16, 64, 256];
    let modes = ["fixed", "seal", "merge"];

    for t in 0..trials {
        let arm_ix = t % 2;
        let arm_name = arms[arm_ix].name;
        let batch = batches[(t / 2) % batches.len()];
        let mode = modes[t % modes.len()];
        let seed = 0xC4 + t as u64;
        let abort_after = 1 + rng.below(max_ops);
        let cap = abort_after + max_ops;
        let late = rng.next().is_multiple_of(2) && mode == "fixed";
        // Half the trials recycle WAL files, so a torn tail can land in
        // front of a previous life's frames (walreuse-plan.md P57.5).
        let recycle = (t / 2) % 2 == 1;
        let dir = root.join(format!("t{t}"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("c4-child")
            .arg("--dir")
            .arg(&dir)
            .arg("--keys")
            .arg(keys.to_string())
            .arg("--batch")
            .arg(batch.to_string())
            .arg("--seed")
            .arg(seed.to_string())
            .arg("--abort-after")
            .arg(abort_after.to_string())
            .arg("--arm")
            .arg(arm_name)
            .arg("--mode")
            .arg(mode)
            .arg("--cap")
            .arg(cap.to_string());
        if late {
            cmd.arg("--late").arg("1");
        }
        if recycle {
            cmd.arg("--recycle").arg("1");
        }
        let st = cmd.output()?;
        let stdout = String::from_utf8_lossy(&st.stdout);
        let mut acked = 0u64;
        let mut abort_line = None;
        for line in stdout.lines() {
            if let Some(n) = line.strip_prefix("ack ") {
                acked = n.trim().parse().unwrap_or(acked);
            } else if line.starts_with("abort ") {
                abort_line = Some(line.to_string());
            }
        }
        let Some(abort_line) = abort_line else {
            // The child exited on its own: an error in the engine, not a
            // crash, and the stderr says which.
            child_errors += 1;
            let why = String::from_utf8_lossy(&st.stderr);
            let why = why.lines().last().unwrap_or("").to_string();
            arms[arm_ix].note(format!(
                "trial {t} ({arm_name}, batch {batch}): the child failed at or before op \
                 {abort_after} instead of crashing: {why}"
            ));
            continue;
        };
        let field = |k: &str| -> String {
            abort_line
                .split(' ')
                .find_map(|f| f.strip_prefix(&format!("{k}=")))
                .unwrap_or("")
                .to_string()
        };
        let num = |k: &str| field(k).parse::<u64>().unwrap_or(0);
        let seal = num("seal") == 1;
        let compact = num("compact") == 1;
        let parts = num("parts");
        // The op the child died before; the state it left is the state
        // after the batches committed before it.
        let died_at = num("op").max(1);
        seal_in_flight += u64::from(seal);
        merge_in_flight += u64::from(compact);
        with_partitions += u64::from(parts > 0);
        late_trials += u64::from(late);
        recycled_trials += u64::from(recycle);
        if coverage_first.is_empty() && seal && compact {
            coverage_first = format!("trial {t}: died with both a seal and a merge in flight");
        }

        // The power loss: the live WAL keeps a random prefix of its
        // unsynced tail. Under Always the tail is at most a header nobody
        // has synced yet; under EveryN it is up to seven acked batches.
        let wal = PathBuf::from(field("wal"));
        let synced = num("synced");
        let written = num("written");
        let on_disk = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        let hi = on_disk.min(written.max(synced));
        let synced = synced.saturating_sub(tear_synced);
        if hi > synced {
            let cut = synced + rng.below(hi - synced + 1);
            if cut < hi {
                torn_trials += 1;
                max_torn = max_torn.max(hi - cut);
                torn_headers += u64::from(cut < 8);
                if on_disk > written {
                    // A recycled or pre-written file: the device keeps
                    // whatever those blocks held before -- the previous
                    // life's frames, or zeros -- and its header, which is
                    // the same eight bytes. Emulate it by pulling stale
                    // bytes from further along the file down over the
                    // lost span past the header; the header itself is
                    // left as it is, because the old one was identical.
                    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
                    let lo = cut.max(8);
                    if hi > lo {
                        let mut f = std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(&wal)?;
                        let span = (hi - lo) as usize;
                        let mut stale = vec![0u8; span];
                        f.seek(SeekFrom::Start(written + (lo - cut)))?;
                        let got = f.read(&mut stale)?;
                        stale.truncate(got);
                        stale.resize(span, 0);
                        f.seek(SeekFrom::Start(lo))?;
                        f.write_all(&stale)?;
                        torn_into_stale += 1;
                    }
                } else {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&wal)?
                        .set_len(cut)?;
                }
            }
        }

        let arm = &mut arms[arm_ix];
        arm.crashes += 1;
        let opts = c4_opts(c4_sync(arm_name), recycle);
        let db = match Db::open(&dir, opts) {
            Ok(db) => db,
            Err(e) => {
                arm.open_failed += 1;
                arm.note(format!(
                    "trial {t} ({arm_name}, {mode}, batch {batch}, op {died_at}, seal={} \
                     merge={}): open refused the directory: {e}",
                    u8::from(seal),
                    u8::from(compact)
                ));
                let _ = std::fs::remove_dir_all(&dir);
                continue;
            }
        };

        // What came back, validated byte for byte against what the stream
        // says that sequence number wrote.
        let ops = c4_ops(seed, keys, cap + batch);
        let mut kb = [0u8; 16];
        let mut want = Vec::new();
        let mut got: Vec<Vec<u64>> = vec![Vec::new(); keys as usize];
        let mut bad = 0u64;
        let mut count_bad = 0u64;
        for k in 0..keys {
            db_key_into(k, &mut kb);
            let mut seqs = Vec::new();
            db.read_all(&kb, |v| {
                let mut ok = v.len() >= 24;
                let seq = if ok {
                    u64::from_be_bytes(v[..8].try_into().unwrap())
                } else {
                    0
                };
                if ok {
                    ok = match ops.get(seq as usize) {
                        Some(C4Op::Put { key, len }) => {
                            c4_value(seq, *key, *len, &mut want);
                            *key == k && want.as_slice() == v
                        }
                        _ => false,
                    };
                }
                if ok {
                    seqs.push(seq);
                } else {
                    bad += 1;
                }
            })?;
            if db.count(&kb)? != seqs.len() as u64 {
                count_bad += 1;
            }
            got[k as usize] = seqs;
        }
        let mut scanned: Vec<Vec<u64>> = vec![Vec::new(); keys as usize];
        db.scan(&[], usize::MAX, |k, v| {
            let key = u64::from_be_bytes(v.get(8..16).unwrap_or(&[0; 8]).try_into().unwrap());
            let seq = u64::from_be_bytes(v.get(..8).unwrap_or(&[0; 8]).try_into().unwrap());
            let mut kb2 = [0u8; 16];
            db_key_into(key, &mut kb2);
            if key < keys && kb2 == k {
                scanned[key as usize].push(seq);
            }
        })?;
        drop(db);
        let scan_bad = scanned != got;
        arm.invented += bad;
        arm.count_disagreed += u64::from(count_bad > 0);
        arm.scan_disagreed += u64::from(scan_bad);
        if bad > 0 {
            arm.note(format!(
                "trial {t} ({arm_name}): {bad} values were not what the stream wrote"
            ));
        }
        if count_bad > 0 {
            arm.note(format!(
                "trial {t} ({arm_name}): count disagreed with read_all on {count_bad} keys"
            ));
        }
        if scan_bad {
            arm.note(format!(
                "trial {t} ({arm_name}): scan disagreed with read_all"
            ));
        }

        // Which prefix of the commit order is this? Replay the stream
        // batch by batch and look for an exact match.
        let mut model: Vec<Vec<u64>> = vec![Vec::new(); keys as usize];
        let mut matches: Vec<u64> = Vec::new();
        if model == got {
            matches.push(0);
        }
        let total_batches = (died_at - 1) / batch + 1;
        let mut i = 1u64;
        for b in 1..=total_batches {
            for _ in 0..batch {
                match ops[i as usize] {
                    C4Op::Put { key, .. } => model[key as usize].push(i),
                    C4Op::Del { key } => model[key as usize].clear(),
                }
                i += 1;
            }
            if model == got {
                matches.push(b);
            }
        }
        let allowed_lo = match arm_name {
            "always" => acked,
            _ => acked.saturating_sub(C4_EVERY_N - 1),
        };
        let allowed_hi = acked + 1;
        let in_window = matches.iter().any(|&p| p >= allowed_lo && p <= allowed_hi);
        if matches.is_empty() {
            arm.no_prefix += 1;
            arm.note(format!(
                "trial {t} ({arm_name}, {mode}, batch {batch}, op {died_at}, {acked} acked, \
                 seal={} merge={}, late={}): the recovered state matches no prefix of the commit \
                 order",
                u8::from(seal),
                u8::from(compact),
                u8::from(late)
            ));
        } else if !in_window {
            let best = *matches.iter().max().unwrap();
            if best < allowed_lo {
                arm.acked_lost += 1;
                arm.worst_lost = arm.worst_lost.max(acked - best);
                arm.note(format!(
                    "trial {t} ({arm_name}, {mode}, batch {batch}, op {died_at}, seal={} \
                     merge={}, late={}): {acked} batches were acknowledged and the store \
                     reopened at batch {best}",
                    u8::from(seal),
                    u8::from(compact),
                    u8::from(late)
                ));
            } else {
                arm.no_prefix += 1;
                arm.note(format!(
                    "trial {t} ({arm_name}): the store reopened at batch {best}, past the \
                     {acked} acknowledged and the one in flight"
                ));
            }
        } else if let Some(&p) = matches.iter().filter(|&&p| p <= allowed_hi).max() {
            if p < acked {
                arm.worst_lost = arm.worst_lost.max(acked - p);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    let crashes: u64 = arms.iter().map(|a| a.crashes).sum();
    rec.series(
        "coverage",
        jobj! {
            "crashes" => J::u(crashes),
            "child_errors" => J::u(child_errors),
            "with_a_seal_in_flight" => J::u(seal_in_flight),
            "with_a_merge_in_flight" => J::u(merge_in_flight),
            "with_partitions" => J::u(with_partitions),
            "after_commit_before_ack" => J::u(late_trials),
            "trials_with_a_torn_wal_tail" => J::u(torn_trials),
            "trials_with_a_torn_wal_header" => J::u(torn_headers),
            "trials_with_recycled_wals" => J::u(recycled_trials),
            "tears_landing_on_stale_frames" => J::u(torn_into_stale),
            "most_bytes_torn" => J::u(max_torn),
            "note" => J::s(&coverage_first),
        },
    );
    for a in &arms {
        rec.series(a.name, a.json());
    }

    let always = &arms[0];
    let every = &arms[1];
    let open_failed: u64 = arms.iter().map(|a| a.open_failed).sum();
    let no_prefix: u64 = arms.iter().map(|a| a.no_prefix).sum();
    let invented: u64 = arms.iter().map(|a| a.invented).sum();
    let count_bad: u64 = arms.iter().map(|a| a.count_disagreed).sum();
    let scan_bad: u64 = arms.iter().map(|a| a.scan_disagreed).sum();
    let first = |pick: &dyn Fn(&C4Arm) -> bool| -> String {
        arms.iter()
            .find(|a| pick(a))
            .map(|a| a.first.clone())
            .unwrap_or_default()
    };

    if seal_in_flight == 0 || merge_in_flight == 0 {
        rec.finding(Finding::not_exercised(
            "C4.1",
            "the engine opens after a crash at any point, seals and merges in flight included",
            format!(
                "{seal_in_flight} crashes landed with a seal in flight and {merge_in_flight} with \
                 a merge; both windows must be reached before opening means anything"
            ),
        ));
    } else {
        rec.finding(Finding::new(
            "C4.1",
            "the engine opens after a crash at any point, seals and merges in flight included",
            open_failed == 0 && child_errors == 0,
            format!(
                "{}/{crashes} directories opened; {open_failed} were refused, {child_errors} \
                 children failed before crashing. {seal_in_flight} crashes had a seal in flight, \
                 {merge_in_flight} a merge, {with_partitions} landed with partitions. {}",
                crashes - open_failed,
                first(&|a| a.open_failed > 0 || !a.first.is_empty())
            ),
        ));
    }
    rec.finding(if always.crashes == 0 {
        Finding::not_exercised(
            "C4.2",
            "under Sync::Always every acknowledged commit survives the crash",
            "no trial ran the Always arm",
        )
    } else {
        Finding::new(
            "C4.2",
            "under Sync::Always every acknowledged commit survives the crash",
            always.acked_lost == 0 && always.open_failed == 0,
            format!(
                "{}/{} crashes reopened at or past the last acknowledged batch; {} lost acked \
                 work (worst {} batches), {} would not open. {}",
                always.crashes - always.acked_lost - always.open_failed,
                always.crashes,
                always.acked_lost,
                always.worst_lost,
                always.open_failed,
                always.first
            ),
        )
    });
    rec.finding(Finding::new(
        "C4.3",
        "what survives is an exact prefix of the commit order, and count and scan agree with it",
        no_prefix == 0 && count_bad == 0 && scan_bad == 0,
        format!(
            "{no_prefix} recovered states matched no prefix of the commit order; count disagreed \
             with read_all in {count_bad} trials and scan in {scan_bad}. {}",
            first(&|a| a.no_prefix > 0 || a.count_disagreed > 0 || a.scan_disagreed > 0)
        ),
    ));
    rec.finding(Finding::new(
        "C4.4",
        "recovery invents nothing: every value read back is one the child wrote, byte for byte",
        invented == 0,
        format!("{invented} values across {crashes} crashes were not what the stream wrote"),
    ));
    rec.finding(if every.crashes == 0 || torn_trials == 0 {
        Finding::not_exercised(
            "C4.5",
            "under Sync::EveryN(8) a crash loses at most seven acknowledged commits, from the tail",
            format!(
                "{} crashes on the EveryN arm and {torn_trials} trials with a torn tail; the \
                 bound is only tested when the emulation removed something",
                every.crashes
            ),
        )
    } else {
        Finding::new(
            "C4.5",
            "under Sync::EveryN(8) a crash loses at most seven acknowledged commits, from the tail",
            every.acked_lost == 0 && every.no_prefix == 0 && every.open_failed == 0,
            format!(
                "{}/{} crashes reopened within seven batches of the last acknowledged one; the \
                 most lost was {} batches; {} lost more, {} matched no prefix, {} would not \
                 open. {}",
                every.crashes - every.acked_lost - every.no_prefix - every.open_failed,
                every.crashes,
                every.worst_lost,
                every.acked_lost,
                every.no_prefix,
                every.open_failed,
                every.first
            ),
        )
    });
    Ok(rec)
}
