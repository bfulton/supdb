//! Reproducers for the hunt: a Snapshot handle reads a write made after it
//! (with the block cache on), and the forms upkeep's `cut_known` assertion.
//!
//! Both are the scenarios of the second form's tests with the second form
//! taken out: it is derived data and changes only which pages a scan reads.
//! The file uses only the crate's long-standing public surface so that it
//! builds at every commit of the history a bisect walks.
//!
//!  * `snapshot_reads_nothing_after_it` is
//!    `scans_agree_with_the_model_over_every_window_of_a_covered_store`:
//!    an ordered load, three rounds of random writes with a seal held short
//!    of its landing after the second, scans of every window through the
//!    writer and a Latest handle, then a Snapshot handle, 200 more writes,
//!    and the snapshot's scans held to the model as it was.
//!  * `the_oracle_holds_with_the_block_cache` is the oracle: puts,
//!    multi-value keys, deletes, seals, merges, promotions and reopens on
//!    tiny partitions, checked against a model after every settle.
//!
//! A failure of the first is a snapshot scan that answers a value written
//! after the snapshot, and the message says `AFTER THE SNAPSHOT`; a failure
//! of the second is a panic in the `supdb-upkeep` thread ("an overlay key's
//! cut is behind the walk" and its kin), which unwinds the lent upkeep and
//! aborts the process (exit 134) when the store is dropped.
//!
//! Environment (all optional):
//!  * `HUNT_CYCLES`  how many times the snapshot, the 200 writes, the commit
//!                   and the first scans repeat in one process (default 1,
//!                   the original test; each cycle takes a fresh handle,
//!                   which is the case that fails, so 30 cycles a process
//!                   measures a rate of one in a few hundred in minutes).
//!  * `HUNT_SCANS`   scans a cycle makes through the snapshot (default all).
//!  * `HUNT_ITERS`   scenario iterations in one process (default 1).
//!  * `HUNT_SEED`    xor into the scenario's seeds (default 0, the original).
//!  * `HUNT_KEEP`    keep the store of a failing iteration.
//!  * `HUNT_VARIANT` letters: `w` the writer's windows before the snapshot,
//!                   `l` the Latest handle's (default `wl`, the original),
//!                   `x` leaves the writer's scans out of the snapshot loop,
//!                   which would otherwise take the lent upkeep back after
//!                   the first snapshot scan.
//!  * `HUNT_THREADED=1` runs the first snapshot scan on a thread of its own
//!                   (`HUNT_READER_DELAY_MS` after the writes begin), for a
//!                   forcing schedule on commits whose publishing is the
//!                   writer's own thread.
//!  * `HUNT_SNAP_DELAY_MS` sleeps this long after the commit, before the
//!                   first snapshot scan.
//!
//! Compile-time switches, for commits that lack what a call needs:
//! `--cfg hunt_old` leaves out `hold_seal_landing` and `frozen_tables`,
//! `--cfg hunt_nofrozen` only `frozen_tables` (b6b259d .. 549244a .. 9764e28), and
//! `hunt_share`, `hunt_forms`, `hunt_upkeep` (together `hunt_opts`),
//! `hunt_alert` and `hunt_rebase` turn on the switches `hunt_opts` lists,
//! which set options by name (both scenarios).

use std::collections::BTreeMap;
use std::path::PathBuf;
use supdb::{Db, Options, Reader};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn scratch(name: &str, iter: u64) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "supdb-next-hunt0-{name}-{}-{iter}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fixed xorshift stream: the values written and the shuffle.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A key's values, in the order the store hands them.
type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

fn read_vec(r: &Reader, key: &[u8]) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let n = r.read_all(key, |x| v.push(x.to_vec())).unwrap();
    assert_eq!(n as usize, v.len(), "the count read_all returns");
    v
}

/// Every key in `keys` read and counted through `r`, as `model` has it.
fn check(r: &Reader, model: &Model, keys: &[Vec<u8>], what: &str) {
    for k in keys {
        let want = model.get(k).cloned().unwrap_or_default();
        assert_eq!(
            read_vec(r, k),
            want,
            "{what}: read of {:?}",
            String::from_utf8_lossy(k)
        );
        assert_eq!(
            r.count(k).unwrap() as usize,
            want.len(),
            "{what}: count of {:?}",
            String::from_utf8_lossy(k)
        );
    }
}

/// Every key the model holds, scanned, is what the model says.
fn check_scan(r: &Reader, model: &Model, what: &str) {
    let mut scanned: Model = BTreeMap::new();
    r.scan(b"", usize::MAX, |k, v| {
        scanned.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    let live: Model = model
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if scanned != live {
        // Say where the maps part, not what they hold.
        let show = |m: &Model, k: &Vec<u8>| match m.get(k) {
            Some(vs) => format!(
                "{} values {:?}",
                vs.len(),
                vs.iter().map(|v| v.len()).collect::<Vec<_>>()
            ),
            None => "absent".to_string(),
        };
        let first = scanned
            .keys()
            .chain(live.keys())
            .filter(|k| scanned.get(*k) != live.get(*k))
            .min()
            .cloned()
            .unwrap_or_default();
        panic!(
            "{what}: the scan parts at key {:?}: scanned {}, model {} ({} keys scanned, {} in the model)",
            String::from_utf8_lossy(&first),
            show(&scanned, &first),
            show(&live, &first),
            scanned.len(),
            live.len()
        );
    }
}

fn apply(model: &mut Model, batch: &mut Vec<(Vec<u8>, Option<Vec<u8>>)>) {
    for (k, v) in batch.drain(..) {
        let e = model.entry(k).or_default();
        match v {
            Some(v) => e.push(v),
            None => e.clear(),
        }
    }
}

/// A value of the suite's shape: a hundred bytes, half a repeated phrase at
/// a phase drawn for each and half entropy.
fn suite_value(rng: &mut Rng) -> Vec<u8> {
    let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
    let phase = rng.below(motif.len());
    let mut v: Vec<u8> = (0..50).map(|j| motif[(phase + j) % motif.len()]).collect();
    v.extend((0..50).map(|_| (rng.next() >> 24) as u8));
    v
}

fn spaced_key(i: usize) -> Vec<u8> {
    format!("key-{:08}", i * 3).into_bytes()
}

/// `n` keys of the suite's shape in ascending order, three apart so that
/// there is a key between any two, committed in batches and flushed into
/// partitions, and the model of them.
fn ordered_load(db: &mut Db, n: usize, seed: u64) -> Model {
    let mut rng = Rng(seed);
    let mut model = Model::new();
    for chunk in (0..n).collect::<Vec<_>>().chunks(500) {
        for &i in chunk {
            let (k, v) = (spaced_key(i), suite_value(&mut rng));
            db.append(&k, &v);
            model.insert(k, vec![v]);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    model
}

/// Keys around `k`: it, one byte either side of it, a proper prefix of it,
/// and two keys that extend it.
fn around(k: &[u8]) -> Vec<Vec<u8>> {
    let mut v = vec![
        k.to_vec(),
        [k, &[0u8][..]].concat(),
        [k, &[0xffu8][..]].concat(),
    ];
    if let Some((&last, head)) = k.split_last() {
        v.push(head.to_vec());
        v.push([head, &[last.wrapping_sub(1)][..]].concat());
        v.push([head, &[last.wrapping_add(1)][..]].concat());
    }
    v
}

/// The entries of a scan and the count of keys it returned.
type Window = (Vec<(Vec<u8>, Vec<u8>)>, usize);

/// What a scan of `limit` keys from `start` answers, as the model has it,
/// and how many keys it visited. A scan visits every key an unsealed source
/// has touched whether or not a value is left to it, and its limit counts
/// the visit, so the model keeps a deleted key as a key with no values until
/// a flush drops it.
fn window(model: &Model, start: &[u8], limit: usize) -> Window {
    let mut out = Vec::new();
    let mut visited = 0;
    for (k, vs) in model.range(start.to_vec()..).take(limit) {
        visited += 1;
        out.extend(vs.iter().map(|v| (k.clone(), v.clone())));
    }
    (out, visited)
}

/// What `r` answers for the same scan, and the count it returns.
fn scanned(r: &Reader, start: &[u8], limit: usize) -> Window {
    let mut got = Vec::new();
    let n = r
        .scan(start, limit, |k, v| got.push((k.to_vec(), v.to_vec())))
        .unwrap();
    (got, n)
}

/// Two answers to a scan are the same, or an error that says where they part.
fn same_window(got: &Window, want: &Window, what: &str) -> Result<(), String> {
    if got == want {
        return Ok(());
    }
    let at = got
        .0
        .iter()
        .zip(&want.0)
        .position(|(a, b)| a != b)
        .unwrap_or(got.0.len().min(want.0.len()));
    let show = |w: &Window| match w.0.get(at) {
        Some((k, v)) => format!(
            "{:?} ({} bytes{})",
            String::from_utf8_lossy(k),
            v.len(),
            if v.starts_with(b"after the snapshot") {
                ", AFTER THE SNAPSHOT"
            } else {
                ""
            }
        ),
        None => "the end".to_string(),
    };
    Err(format!(
        "{what}: {} entries over {} keys, wanted {} over {}; they part at entry {at}, \
         got {}, wanted {}",
        got.0.len(),
        got.1,
        want.0.len(),
        want.1,
        show(got),
        show(want)
    ))
}

// ---- the snapshot scenario ------------------------------------------------

/// Option switches for isolating a mechanism, each on the commits that have
/// the option it sets; with nothing compiled in this does nothing.
///
///  * `--cfg hunt_share`: `HUNT_NO_SHARE=1` leaves the published scan
///    snapshot out.
///  * `--cfg hunt_forms`: `HUNT_NO_FORMS=1` leaves the published block forms
///    out, `HUNT_FORMS_ON=1` turns them on where an arm has them off.
///  * `--cfg hunt_upkeep`: `HUNT_UPKEEP=inline|bg1|bg2` sets the writer's
///    upkeep.
///  * `--cfg hunt_opts` is the three together.
///  * `--cfg hunt_alert`: `HUNT_NO_ALERT=1` is the upkeep thread's back-off
///    as it was before it stayed alert.
///  * `--cfg hunt_rebase`: `HUNT_NO_REBASE=1` drops the tables at a merge's
///    rewrite of a partition, as before they were carried.
///  * `--cfg hunt_lazy`: `HUNT_EAGER=1` carries the tables inside the publish
///    (on the commit that added the lazy carry beside it).
#[allow(unused_variables)]
fn hunt_opts(o: &mut Options) {
    #[cfg(any(hunt_opts, hunt_share))]
    if env_u64("HUNT_NO_SHARE", 0) == 1 {
        o.share_snapshot = false;
    }
    #[cfg(any(hunt_opts, hunt_forms))]
    {
        if env_u64("HUNT_NO_FORMS", 0) == 1 {
            o.commit_forms = false;
        }
        if env_u64("HUNT_FORMS_ON", 0) == 1 {
            o.commit_forms = true;
        }
    }
    #[cfg(hunt_rebase)]
    if env_u64("HUNT_NO_REBASE", 0) == 1 {
        o.forms_rebase = false;
    }
    // The writer's scan snapshot carried across a publish (`--cfg
    // hunt_carry`, from the commit that made it, off by default).
    #[cfg(hunt_carry)]
    if env_u64("HUNT_SNAP_CARRY", 0) == 1 {
        o.snapshot_carry = true;
    }
    // The carry before the lazy one, on the one commit that has both
    // (`--cfg hunt_lazy`): the writer's tables and the published forms
    // moved together inside the publish.
    #[cfg(hunt_lazy)]
    if env_u64("HUNT_EAGER", 0) == 1 {
        o.forms_carry_lazily = false;
    }
    #[cfg(any(hunt_opts, hunt_upkeep))]
    match std::env::var("HUNT_UPKEEP").as_deref() {
        Ok("inline") => o.upkeep = supdb::db::Upkeep::Inline,
        Ok("bg1") => o.upkeep = supdb::db::Upkeep::Background(1),
        Ok("bg2") => o.upkeep = supdb::db::Upkeep::Background(2),
        _ => {}
    }
    #[cfg(hunt_alert)]
    if env_u64("HUNT_NO_ALERT", 0) == 1 {
        o.upkeep_alert = false;
    }
}

fn snapshot_scenario(iter: u64) -> Result<(), String> {
    let xor = env_u64("HUNT_SEED", 0);
    let variant = std::env::var("HUNT_VARIANT").unwrap_or_else(|_| "wl".to_string());
    let dir = scratch("windows", iter);
    let mut opts = Options {
        partition_bytes: Some(160 << 10),
        scan_block_cache: true,
        ..Options::default()
    };
    hunt_opts(&mut opts);
    let mut db = Db::create(&dir, opts).unwrap();
    let mut model = ordered_load(&mut db, 5000, 0x71D ^ xor);
    db.settle().unwrap();
    // The seal is held short of its landing: a piece landed is merged back
    // into its partition by the segment work, which drops the tombstones
    // that the scans are held to counting, and the model would be racing
    // the merge.
    // (Commits before the seals in flight were a queue have neither call;
    // `--cfg hunt_old` leaves them out and the seal lands as it may.)
    #[cfg(not(hunt_old))]
    db.hold_seal_landing(true);
    let mut rng = Rng(0x71D1 ^ xor);
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    // Updates and deletes of records, and keys between records.
    for round in 0..3 {
        for _ in 0..120 {
            let k = keys[rng.below(keys.len())].clone();
            match rng.below(6) {
                0 => {
                    db.delete(&k);
                    model.insert(k, Vec::new());
                }
                1 => {
                    let new = [k.as_slice(), b"+"].concat();
                    let v = suite_value(&mut rng);
                    db.append(&new, &v);
                    model.entry(new).or_default().push(v);
                }
                _ => {
                    let v = suite_value(&mut rng);
                    db.put(&k, &v);
                    model.insert(k, vec![v]);
                }
            }
        }
        db.commit().unwrap();
        if round == 1 {
            // The first two rounds' writes are in a frozen table when the
            // third begins.
            db.seal().unwrap();
            // Where the seal cannot be held, it lands now, so that when it
            // lands is not what the snapshot's cycles depend on: a commit
            // that joins a finished seal publishes the forms into the state
            // a handle holds, and whether the seal had finished by the
            // first cycle's commit was a matter of the machine's load.
            #[cfg(hunt_old)]
            db.settle().unwrap();
        }
    }
    #[cfg(not(any(hunt_old, hunt_nofrozen)))]
    assert_eq!(db.frozen_tables(), 1, "the seal is held, its table frozen");
    let mut starts: Vec<Vec<u8>> =
        vec![b"".to_vec(), b"a".to_vec(), b"zzz".to_vec(), vec![0xff; 8]];
    // The second form's tests began at its fence keys; a record in every
    // few hundred stands in for them.
    for fk in keys.iter().step_by(399) {
        starts.extend(around(fk));
    }
    let limits = [1usize, 2, 3, 5, 17, 56, 57, 58, 114, usize::MAX];
    let latest = db.reader().unwrap();
    for start in &starts {
        for &limit in &limits {
            let cap = limit.min(1500);
            let want = window(&model, start, cap);
            let what = format!("from {:?} for {cap}", String::from_utf8_lossy(start));
            if variant.contains('w') {
                same_window(&scanned(&db, start, cap), &want, &format!("writer, {what}"))?;
            }
            if variant.contains('l') {
                same_window(
                    &scanned(&latest, start, cap),
                    &want,
                    &format!("handle, {what}"),
                )?;
            }
        }
    }
    // A snapshot keeps the answers it was taken at, whatever the writer
    // does after. Cycle 0 is the original test; HUNT_CYCLES repeats the
    // snapshot, the writes and the first scans with a fresh handle each
    // time, since every failure seen so far is the first scan of a fresh
    // handle.
    let cycles = env_u64("HUNT_CYCLES", 1);
    let max_scans = env_u64("HUNT_SCANS", 0);
    let mut failure: Option<String> = None;
    let mut failed_cycles = 0u64;
    for c in 0..cycles {
        let snap = db.reader().unwrap();
        snap.snapshot();
        let at_snapshot = model.clone();
        let tag = if c == 0 {
            "after the snapshot".to_string()
        } else {
            format!("after the snapshot {c}")
        };
        // HUNT_THREADED=1: the handle's first scan runs on a thread of its
        // own that wakes HUNT_READER_DELAY_MS after the puts began, so the
        // writer's commit and the scan overlap on commits where the
        // publishing is the writer's own. This is a branch of its own: a
        // handle that is moved after it is taken (`snapshot`) changes what
        // the oldest commits of the long history do, so the default path
        // below leaves the handle where the original test did.
        if env_u64("HUNT_THREADED", 0) == 1 {
            let wait = env_u64("HUNT_READER_DELAY_MS", 20);
            let want = window(&at_snapshot, b"", 3);
            let reader = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                scanned(&snap, b"", 3)
            });
            for k in keys.iter().skip((c % 5) as usize).step_by(5).take(200) {
                db.put(k, tag.as_bytes());
                model.insert(k.clone(), vec![tag.as_bytes().to_vec()]);
            }
            db.commit().unwrap();
            let got = reader.join().unwrap();
            if let Err(e) = same_window(
                &got,
                &want,
                &format!("snapshot, cycle {c}, threaded first scan from \"\" for 3"),
            ) {
                failed_cycles += 1;
                eprintln!("HUNT-CYCLE-FAIL {e}");
                failure.get_or_insert(e);
            }
            continue;
        }
        for k in keys.iter().skip((c % 5) as usize).step_by(5).take(200) {
            db.put(k, tag.as_bytes());
            model.insert(k.clone(), vec![tag.as_bytes().to_vec()]);
        }
        db.commit().unwrap();
        // A forcing schedule's knob: the first snapshot scan begins this
        // long after the commit that lent the upkeep to its thread.
        let delay = env_u64("HUNT_SNAP_DELAY_MS", 0);
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        let mut this = None;
        let mut done_scans = 0u64;
        'scans: for start in starts.iter().step_by(3) {
            for &limit in &[3usize, 57, 114] {
                if max_scans > 0 && done_scans >= max_scans {
                    break 'scans;
                }
                done_scans += 1;
                let what = format!("from {:?} for {limit}", String::from_utf8_lossy(start));
                if let Err(e) = same_window(
                    &scanned(&snap, start, limit),
                    &window(&at_snapshot, start, limit),
                    &format!("snapshot, cycle {c}, {what}"),
                ) {
                    this = Some(e);
                    break 'scans;
                }
                // The writer's scan takes the lent upkeep back, which waits
                // out a pass in flight: with it in the loop only the first
                // snapshot scan can meet the pass. 'x' leaves it out.
                if !variant.contains('x') {
                    if let Err(e) = same_window(
                        &scanned(&db, start, limit),
                        &window(&model, start, limit),
                        &format!("writer after, cycle {c}, {what}"),
                    ) {
                        this = Some(e);
                        break 'scans;
                    }
                }
            }
        }
        if let Some(e) = this {
            failed_cycles += 1;
            eprintln!("HUNT-CYCLE-FAIL {e}");
            failure.get_or_insert(e);
        }
        drop(snap);
    }
    eprintln!("HUNT-CYCLES {failed_cycles} of {cycles} cycles failed");
    // How many block forms scans took from the state's published forms, to
    // show a take-away that stops the failures has not stopped the taking
    // (`--cfg hunt_takes`, on commits that count them).
    #[cfg(hunt_takes)]
    eprintln!("HUNT-TAKES {}", db.canonical_forms().2);
    if let Some(e) = failure {
        if std::env::var("HUNT_KEEP").is_err() {
            drop(latest);
            drop(db);
            let _ = std::fs::remove_dir_all(&dir);
        }
        return Err(e);
    }
    check(&db, &model, &keys, "after the windows");
    drop(latest);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn snapshot_reads_nothing_after_it() {
    let iters = env_u64("HUNT_ITERS", 1);
    let mut failures = Vec::new();
    for i in 0..iters {
        if let Err(e) = snapshot_scenario(i) {
            eprintln!("HUNT-FAIL iteration {i}: {e}");
            failures.push(e);
        }
    }
    eprintln!(
        "HUNT-SUMMARY snapshot: {} failures of {} iterations",
        failures.len(),
        iters
    );
    assert!(failures.is_empty(), "{}", failures[0]);
}

// ---- the oracle -----------------------------------------------------------

const ORACLE_KEYS: usize = 240;

fn oracle_opts(inline: bool) -> Options {
    let mut o = Options {
        scan_block_cache: true,
        seal_bytes: 8 << 10,
        partition_bytes: Some(4 << 10),
        inline_bytes: if inline {
            1 << 16
        } else {
            Options::default().inline_bytes
        },
        l0_trigger: 2,
        ..Options::default()
    };
    hunt_opts(&mut o);
    o
}

fn oracle_keys() -> Vec<Vec<u8>> {
    // Past the keys written too: a key nobody wrote is absent.
    (0..ORACLE_KEYS + 20)
        .map(|i| format!("k{i:04}").into_bytes())
        .collect()
}

fn oracle(inline: bool, iter: u64) {
    let xor = env_u64("HUNT_SEED", 0);
    let tag = if inline { "inline" } else { "blocks" };
    let (dz, dr) = (
        scratch(&format!("oracle-z-{tag}"), iter),
        scratch(&format!("oracle-raw-{tag}"), iter),
    );
    let mut z = Db::create(&dz, oracle_opts(inline)).unwrap();
    let mut raw = Db::create(&dr, oracle_opts(inline)).unwrap();
    let mut model: Model = BTreeMap::new();
    let mut pending: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
    let mut rng = Rng(0x0AC1E ^ xor);
    let keys = oracle_keys();
    let both = |z: &mut Db, raw: &mut Db, f: &dyn Fn(&mut Db)| {
        f(z);
        f(raw);
    };
    for step in 0..3000u64 {
        let key = format!("k{:04}", rng.below(ORACLE_KEYS)).into_bytes();
        match rng.below(20) {
            0 => {
                both(&mut z, &mut raw, &|d| d.delete(&key));
                pending.push((key, None));
            }
            1..=3 => {
                // A key with several values at once, some of them empty.
                for j in 0..2 + rng.below(4) {
                    let v = if j == 1 {
                        Vec::new()
                    } else {
                        format!("m{step}.{j}").into_bytes()
                    };
                    both(&mut z, &mut raw, &|d| d.append(&key, &v));
                    pending.push((key.clone(), Some(v)));
                }
            }
            _ => {
                let pad = rng.below(80);
                let v = format!("v{step}-{}", "p".repeat(pad)).into_bytes();
                both(&mut z, &mut raw, &|d| d.append(&key, &v));
                pending.push((key, Some(v)));
            }
        }
        match rng.below(100) {
            0..=9 => {
                z.commit().unwrap();
                raw.commit().unwrap();
                apply(&mut model, &mut pending);
            }
            10..=12 => {
                z.commit().unwrap();
                raw.commit().unwrap();
                apply(&mut model, &mut pending);
                z.seal().unwrap();
                raw.seal().unwrap();
            }
            13 if step % 5 == 0 => {
                // A crash: what was not committed vanishes, on both.
                drop(z);
                drop(raw);
                pending.clear();
                z = Db::open(&dz, oracle_opts(inline)).unwrap();
                raw = Db::open(&dr, oracle_opts(inline)).unwrap();
                check(&z, &model, &keys, "after a crash");
                check(&raw, &model, &keys, "after a crash, raw");
            }
            _ => {}
        }
        if step % 250 == 249 {
            z.commit().unwrap();
            raw.commit().unwrap();
            apply(&mut model, &mut pending);
            if step % 500 == 499 {
                // A drain: the memtable into partitions, by promotion where
                // the engine can.
                z.flush().unwrap();
                raw.flush().unwrap();
            }
            z.settle().unwrap();
            raw.settle().unwrap();
            check(&z, &model, &keys, "z, writer");
            check(&raw, &model, &keys, "raw, writer");
            let (hz, hr) = (z.reader().unwrap(), raw.reader().unwrap());
            check(&hz, &model, &keys, "z, handle");
            check(&hr, &model, &keys, "raw, handle");
            check_scan(&z, &model, "z");
        }
    }
    z.commit().unwrap();
    raw.commit().unwrap();
    apply(&mut model, &mut pending);
    z.settle().unwrap();
    raw.settle().unwrap();
    check(&z, &model, &keys, "z, at the end");
    check(&raw, &model, &keys, "raw, at the end");
    let snap = z.reader().unwrap();
    snap.snapshot();
    check(&snap, &model, &keys, "z, a snapshot");
    drop(snap);
    drop(z);
    drop(raw);
    let _ = std::fs::remove_dir_all(&dz);
    let _ = std::fs::remove_dir_all(&dr);
}

#[test]
fn the_oracle_holds_with_the_block_cache() {
    for i in 0..env_u64("HUNT_ITERS", 1) {
        oracle(true, i);
    }
}

#[test]
fn the_oracle_holds_with_the_block_cache_and_runs_in_blocks() {
    for i in 0..env_u64("HUNT_ITERS", 1) {
        oracle(false, i);
    }
}
