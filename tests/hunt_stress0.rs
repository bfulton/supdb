//! Hunt stress0: a Snapshot handle reads a write made after it.
//!
//! The scenario is the windows test of the compact form's branch with the
//! form taken away (it only changes which pages a scan reads): an ordered
//! load of suite-shaped keys, a flush, a settle, a seal held short of its
//! landing, three rounds of random puts, deletes and inserts between
//! records with a seal after the second, scans of every window through the
//! writer and through a `Latest` handle held since before the rounds, and
//! then a `Snapshot` handle whose scans must answer the model as it was
//! when it was taken, whatever the writer puts and commits after.
//!
//! It is a loop target and not a test that is expected to be green: every
//! knob is an environment variable (`HUNT_*`), the test runs the scenario
//! `HUNT_ITERS` times in one process and panics with the engine's account of
//! the key at the first failure, and a shell loop over processes with
//! different `TMPDIR`s and seeds reads the rate. See `hunt_cfg` for the
//! knobs; the defaults are the original scenario.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use supdb::{Db, Options, Reader, Upkeep};

type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;
type Window = (Vec<(Vec<u8>, Vec<u8>)>, usize);

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

fn env_str(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn env_usize(name: &str, default: usize) -> usize {
    env_str(name).map_or(default, |s| s.parse().expect(name))
}

fn env_flag(name: &str, default: bool) -> bool {
    env_str(name).map_or(default, |s| s != "0")
}

#[derive(Clone, Debug)]
struct Cfg {
    /// Scenario repetitions in this process.
    iters: usize,
    /// Xored into the load's and the rounds' seeds; 0 is the original's.
    seed: u64,
    nkeys: usize,
    part_kib: usize,
    upkeep: Upkeep,
    upkeep_name: String,
    /// `Db::hold_seal_landing(true)` before the rounds.
    hold: bool,
    /// `Db::settle()` after the load, the "settled start".
    settle: bool,
    rounds: usize,
    ops: usize,
    /// The round after which the table is sealed.
    seal_after: usize,
    /// The window scans through the writer and the `Latest` handle.
    windows: bool,
    /// Rank stride of the start keys, where the original took every
    /// seventh fence key of the compact form's sidecars.
    stride: usize,
    /// Snapshot cycles per scenario.
    snaps: usize,
    after_n: usize,
    after_step: usize,
    /// The "after the snapshot" puts committed in this many commits, a
    /// scan pass through the snapshot handle after each.
    after_commits: usize,
    pause_ms: u64,
    /// Extra threads scanning through handles of their own.
    readers: usize,
    /// Threads spinning at normal priority, to starve the idle-class
    /// upkeep thread.
    hogs: usize,
    /// Compare the snapshot's answers to the model strictly, visited count
    /// included; without a held seal a merge can drop tombstones under the
    /// model, so the default is the lenient check then.
    strict: bool,
    snap_step: usize,
    snap_limits: Vec<usize>,
    /// Scans through the snapshot handle before the writer's puts (a warm
    /// handle), by the same starts.
    prewarm: bool,
    /// Drop the Latest handle before the snapshot phase.
    drop_latest: bool,
}

fn hunt_cfg() -> Cfg {
    let upkeep_name = env_str("HUNT_UPKEEP").unwrap_or_else(|| "bg1".to_string());
    let upkeep = match upkeep_name.as_str() {
        "inline" => Upkeep::Inline,
        "bg1" => Upkeep::Background(1),
        "bg2" => Upkeep::Background(2),
        "bg3" => Upkeep::Background(3),
        other => panic!("HUNT_UPKEEP {other}"),
    };
    let hold = env_flag("HUNT_HOLD", true);
    Cfg {
        iters: env_usize("HUNT_ITERS", 1),
        seed: env_usize("HUNT_SEED", 0) as u64,
        nkeys: env_usize("HUNT_NKEYS", 5000),
        part_kib: env_usize("HUNT_PART_KIB", 160),
        upkeep,
        upkeep_name,
        hold,
        settle: env_flag("HUNT_SETTLE", true),
        rounds: env_usize("HUNT_ROUNDS", 3),
        ops: env_usize("HUNT_OPS", 120),
        seal_after: env_usize("HUNT_SEAL_AFTER", 1),
        windows: env_flag("HUNT_WINDOWS", true),
        stride: env_usize("HUNT_STRIDE", 238),
        snaps: env_usize("HUNT_SNAPS", 1),
        after_n: env_usize("HUNT_AFTER_N", 200),
        after_step: env_usize("HUNT_AFTER_STEP", 5),
        after_commits: env_usize("HUNT_AFTER_COMMITS", 1),
        pause_ms: env_usize("HUNT_PAUSE_MS", 0) as u64,
        readers: env_usize("HUNT_READERS", 0),
        hogs: env_usize("HUNT_HOGS", 0),
        strict: env_flag("HUNT_STRICT", hold),
        snap_step: env_usize("HUNT_SNAP_STEP", 3),
        snap_limits: env_str("HUNT_SNAP_LIMITS")
            .unwrap_or_else(|| "3,57,114".to_string())
            .split(',')
            .map(|s| s.parse().unwrap())
            .collect(),
        prewarm: env_flag("HUNT_PREWARM", false),
        drop_latest: env_flag("HUNT_DROP_LATEST", false),
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-hunt0-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

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

fn window(model: &Model, start: &[u8], limit: usize) -> Window {
    let mut out = Vec::new();
    let mut visited = 0;
    for (k, vs) in model.range(start.to_vec()..).take(limit) {
        visited += 1;
        out.extend(vs.iter().map(|v| (k.clone(), v.clone())));
    }
    (out, visited)
}

fn scanned(r: &Reader, start: &[u8], limit: usize) -> Window {
    let mut got = Vec::new();
    let n = r
        .scan(start, limit, |k, v| got.push((k.to_vec(), v.to_vec())))
        .unwrap();
    (got, n)
}

fn show(k: &[u8]) -> String {
    String::from_utf8_lossy(k).into_owned()
}

fn vals(r: &Reader, key: &[u8]) -> Vec<String> {
    let mut v = Vec::new();
    r.read_all(key, |x| v.push(String::from_utf8_lossy(&x[..x.len().min(24)]).into_owned()))
        .unwrap();
    v
}

fn short(v: &[u8]) -> String {
    format!("{}..({}b)", show(&v[..v.len().min(20)]), v.len())
}

/// Where two answers part: the first differing entry's key, and a line.
fn parting(got: &Window, want: &Window) -> Option<(Vec<u8>, String)> {
    if got == want {
        return None;
    }
    let at = got
        .0
        .iter()
        .zip(&want.0)
        .position(|(a, b)| a != b)
        .unwrap_or(got.0.len().min(want.0.len()));
    let key = got
        .0
        .get(at)
        .or_else(|| want.0.get(at))
        .map(|(k, _)| k.clone())
        .unwrap_or_default();
    let ent = |w: &Window| match w.0.get(at) {
        Some((k, v)) => format!("{} = {}", show(k), short(v)),
        None => "the end".to_string(),
    };
    Some((
        key,
        format!(
            "{} entries over {} keys, wanted {} over {}; they part at entry {at}: got {}, wanted {}",
            got.0.len(),
            got.1,
            want.0.len(),
            want.1,
            ent(got),
            ent(want)
        ),
    ))
}

/// Whether a later write is in `got`: any value from a put after the
/// snapshot, or any value the snapshot's model never held for its key.
fn foreign(got: &Window, at_snapshot: &Model) -> Option<(Vec<u8>, Vec<u8>)> {
    for (k, v) in &got.0 {
        let known = at_snapshot.get(k).is_some_and(|vs| vs.contains(v));
        if !known {
            return Some((k.clone(), v.clone()));
        }
    }
    None
}

struct Threads {
    stop: Arc<AtomicBool>,
    joins: Vec<std::thread::JoinHandle<u64>>,
}

impl Threads {
    fn start(db: &Db, cfg: &Cfg, starts: &[Vec<u8>]) -> Threads {
        let stop = Arc::new(AtomicBool::new(false));
        let mut joins = Vec::new();
        for t in 0..cfg.readers {
            let r = db.reader().unwrap();
            let stop = stop.clone();
            let starts = starts.to_vec();
            joins.push(std::thread::spawn(move || {
                let mut rng = Rng(0x5EED_0000 + t as u64 * 7919 + 1);
                let limits = [1usize, 3, 17, 57, 114, 400];
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let s = &starts[rng.below(starts.len())];
                    let l = limits[rng.below(limits.len())];
                    let mut last: Option<Vec<u8>> = None;
                    r.scan(s, l, |k, _| {
                        if let Some(p) = &last {
                            assert!(p.as_slice() <= k, "a scan out of key order");
                        }
                        last = Some(k.to_vec());
                    })
                    .unwrap();
                    n += 1;
                }
                n
            }));
        }
        for _ in 0..cfg.hogs {
            let stop = stop.clone();
            joins.push(std::thread::spawn(move || {
                let mut x = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    }
                    std::hint::black_box(x);
                }
                0
            }));
        }
        Threads { stop, joins }
    }

    fn finish(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        self.joins.into_iter().map(|j| j.join().unwrap()).sum()
    }
}

/// Everything the engine can say about `key` through a handle that has just
/// answered wrongly, in the order that disturbs it least.
#[allow(clippy::too_many_arguments)]
fn capture(
    db: &Db,
    snap: &Reader,
    latest: Option<&Reader>,
    key: &[u8],
    start: &[u8],
    limit: usize,
    at_snapshot: &Model,
    now: &Model,
    why: &str,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "HUNT-CAPTURE {why}");
    let _ = writeln!(
        s,
        "key {} scan from {} limit {limit}; isolation {:?}",
        show(key),
        show(start),
        snap.isolation()
    );
    let _ = writeln!(
        s,
        "model at the snapshot: {:?}; model now: {:?}",
        at_snapshot
            .get(key)
            .map(|v| v.iter().map(|x| short(x)).collect::<Vec<_>>()),
        now.get(key)
            .map(|v| v.iter().map(|x| short(x)).collect::<Vec<_>>())
    );
    // The handle's account first, as it stands after the wrong answer.
    let _ = writeln!(s, "snap.debug_key: {}", snap.debug_key(key));
    let _ = writeln!(s, "snap.canonical_forms: {:?}", snap.canonical_forms());
    let _ = writeln!(s, "snap.block_cache_kinds: {:?}", snap.block_cache_kinds());
    let _ = writeln!(s, "snap point read: {:?}", vals(snap, key));
    // Persistent or transient: the same scan again, then a scan from the key.
    let again = scanned(snap, start, limit);
    let _ = writeln!(
        s,
        "same scan again: {} entries over {}; foreign {:?}",
        again.0.len(),
        again.1,
        foreign(&again, at_snapshot).map(|(k, v)| (show(&k), short(&v)))
    );
    let from_key = scanned(snap, key, 1);
    let _ = writeln!(
        s,
        "scan from the key, limit 1: {:?}",
        from_key
            .0
            .iter()
            .map(|(k, v)| (show(k), short(v)))
            .collect::<Vec<_>>()
    );
    let _ = writeln!(s, "snap.debug_key after: {}", snap.debug_key(key));
    if let Some(l) = latest {
        let _ = writeln!(s, "latest point read: {:?}", vals(l, key));
        let _ = writeln!(s, "latest.debug_key: {}", l.debug_key(key));
    }
    let _ = writeln!(s, "writer point read: {:?}", vals(db, key));
    let _ = writeln!(s, "writer.debug_key: {}", db.debug_key(key));
    let _ = writeln!(
        s,
        "snapshot_builds {} extends {} refreshes {} switches {} kept {:?} upkeep_counts {:?}",
        db.snapshot_builds(),
        db.snapshot_extends(),
        db.snapshot_refreshes(),
        db.snapshot_switches(),
        db.snapshot_kept(),
        db.upkeep_counts()
    );
    let _ = writeln!(s, "writer.canonical_forms: {:?}", db.canonical_forms());
    s
}

struct Tally {
    windows: u64,
    snap_scans: u64,
    reader_scans: u64,
}

/// One run of the scenario. Err carries the capture of the first wrong
/// answer through the snapshot handle (or the account of a wrong answer of
/// the writer's or the Latest handle's in the window phase).
fn scenario(cfg: &Cfg, iter: usize) -> Result<Tally, String> {
    let dir = scratch(&format!("i{iter}"));
    let opts = Options {
        partition_bytes: Some(cfg.part_kib << 10),
        scan_block_cache: true,
        upkeep: cfg.upkeep,
        ..Options::default()
    };
    let mut db = Db::create(&dir, opts).unwrap();
    let mut model = ordered_load(&mut db, cfg.nkeys, 0x71D ^ cfg.seed);
    if cfg.settle {
        db.settle().unwrap();
    }
    if cfg.hold {
        db.hold_seal_landing(true);
    }
    let mut rng = Rng((0x71D1 ^ cfg.seed) | 1);
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    for round in 0..cfg.rounds {
        for _ in 0..cfg.ops {
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
        if round == cfg.seal_after {
            db.seal().unwrap();
        }
    }
    if cfg.hold && cfg.seal_after < cfg.rounds {
        assert_eq!(db.frozen_tables(), 1, "the seal is held, its table frozen");
    }
    let mut starts: Vec<Vec<u8>> =
        vec![b"".to_vec(), b"a".to_vec(), b"zzz".to_vec(), vec![0xff; 8]];
    for i in (0..cfg.nkeys).step_by(cfg.stride.max(1)) {
        starts.extend(around(&spaced_key(i)));
    }
    let limits = [1usize, 2, 3, 5, 17, 56, 57, 58, 114, usize::MAX];
    let mut latest = Some(db.reader().unwrap());
    let threads = Threads::start(&db, cfg, &starts);
    let mut tally = Tally {
        windows: 0,
        snap_scans: 0,
        reader_scans: 0,
    };
    let result = (|| -> Result<(), String> {
        if cfg.windows {
            for start in &starts {
                for &limit in &limits {
                    let cap = limit.min(1500);
                    let want = window(&model, start, cap);
                    let got = scanned(&db, start, cap);
                    if cfg.strict {
                        if let Some((key, line)) = parting(&got, &want) {
                            return Err(format!(
                                "HUNT-FAIL windows-writer from {} for {cap}: {line}\nkey {}",
                                show(start),
                                show(&key)
                            ));
                        }
                    }
                    let got = scanned(latest.as_ref().unwrap(), start, cap);
                    if cfg.strict {
                        if let Some((key, line)) = parting(&got, &want) {
                            return Err(format!(
                                "HUNT-FAIL windows-latest from {} for {cap}: {line}\nkey {}",
                                show(start),
                                show(&key)
                            ));
                        }
                    }
                    tally.windows += 1;
                }
            }
        }
        if cfg.drop_latest {
            latest = None;
        }
        for cycle in 0..cfg.snaps {
            let snap = db.reader().unwrap();
            snap.snapshot();
            let at_snapshot = model.clone();
            let tag: Vec<u8> = format!("after the snapshot {cycle}").into_bytes();
            let step = cfg.after_step.max(1);
            let ks: Vec<Vec<u8>> = keys
                .iter()
                .skip(cycle % step)
                .step_by(step)
                .take(cfg.after_n)
                .cloned()
                .collect();
            let per = ks.len().div_ceil(cfg.after_commits.max(1)).max(1);
            if cfg.prewarm {
                for start in starts.iter().step_by(cfg.snap_step.max(1)) {
                    for &limit in &cfg.snap_limits {
                        let got = scanned(&snap, start, limit);
                        tally.snap_scans += 1;
                        if let Some((k, v)) = foreign(&got, &at_snapshot) {
                            return Err(format!(
                                "HUNT-FAIL prewarm-foreign {} = {}",
                                show(&k),
                                short(&v)
                            ));
                        }
                    }
                }
            }
            for chunk in ks.chunks(per) {
                for k in chunk {
                    db.put(k, &tag);
                    model.insert(k.clone(), vec![tag.clone()]);
                }
                db.commit().unwrap();
                if cfg.pause_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(cfg.pause_ms));
                }
                for start in starts.iter().step_by(cfg.snap_step.max(1)) {
                    for &limit in &cfg.snap_limits {
                        let got = scanned(&snap, start, limit);
                        tally.snap_scans += 1;
                        let want = window(&at_snapshot, start, limit);
                        let strict_fail = if cfg.strict {
                            parting(&got, &want)
                        } else {
                            None
                        };
                        let lenient_fail = foreign(&got, &at_snapshot);
                        if strict_fail.is_some() || lenient_fail.is_some() {
                            let (key, line) = match (&lenient_fail, &strict_fail) {
                                (Some((k, v)), _) => (
                                    k.clone(),
                                    format!("a value the snapshot never held: {} = {}", show(k), short(v)),
                                ),
                                (None, Some((k, l))) => (k.clone(), l.clone()),
                                _ => unreachable!(),
                            };
                            let kind = if lenient_fail.is_some() {
                                "later-write"
                            } else {
                                "mismatch"
                            };
                            return Err(format!(
                                "HUNT-FAIL snapshot-{kind} cycle {cycle} from {} for {limit}: {line}\n{}",
                                show(start),
                                capture(
                                    &db,
                                    &snap,
                                    latest.as_ref(),
                                    &key,
                                    start,
                                    limit,
                                    &at_snapshot,
                                    &model,
                                    &line
                                )
                            ));
                        }
                        let now = scanned(&db, start, limit);
                        if cfg.strict {
                            if let Some((key, line)) = parting(&now, &window(&model, start, limit))
                            {
                                return Err(format!(
                                    "HUNT-FAIL writer-after from {} for {limit}: {line}\nkey {}",
                                    show(start),
                                    show(&key)
                                ));
                            }
                        }
                    }
                }
            }
            snap.release();
        }
        Ok(())
    })();
    tally.reader_scans = threads.finish();
    drop(latest);
    let closed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(db);
    }));
    let _ = std::fs::remove_dir_all(&dir);
    if closed.is_err() {
        return Err("HUNT-FAIL close panicked".to_string());
    }
    result.map(|()| tally)
}

#[test]
fn hunt_scenario() {
    let cfg = hunt_cfg();
    let mut total = Tally {
        windows: 0,
        snap_scans: 0,
        reader_scans: 0,
    };
    for iter in 0..cfg.iters {
        match scenario(&cfg, iter) {
            Ok(t) => {
                total.windows += t.windows;
                total.snap_scans += t.snap_scans;
                total.reader_scans += t.reader_scans;
            }
            Err(e) => {
                eprintln!("{e}");
                eprintln!("HUNT-CFG {cfg:?} iter {iter}");
                panic!("hunt failure at iteration {iter}");
            }
        }
    }
    eprintln!(
        "HUNT-OK iters {} windows {} snap_scans {} reader_scans {}",
        cfg.iters, total.windows, total.snap_scans, total.reader_scans
    );
}

#[allow(dead_code)]
fn unused(_: &AtomicU64) {}
