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
//! `hunt_scenario` is a loop target and not a test that is expected to be
//! green: every knob is an environment variable (`HUNT_*`, see `hunt_cfg`),
//! the test runs the scenario `HUNT_ITERS` times in one process and panics
//! with the engine's account of the key at the first failure, and a shell
//! loop over processes with different `TMPDIR`s reads the rate
//! (`tests/hunt_stress0/loop.sh`). The defaults are the original scenario.
//!
//! `hunt_forced` runs the same store through a schedule forced by a hook
//! in the engine (`dbgtrace::hold_forms_position`): the forms publisher is
//! parked between its slot swaps and its store of the position the slots
//! are current to, and the snapshot handle scans there.
//!
//! Knobs, all environment variables: `HUNT_ITERS` scenario runs in the
//! process; `HUNT_SEED` xored into the seeds (0 is the original's);
//! `HUNT_NKEYS` (5000), `HUNT_PART_KIB` (160); `HUNT_UPKEEP`
//! (inline|bg1|bg2|bg3); `HUNT_HOLD` (1: the seal is held), `HUNT_SETTLE`
//! (1: a settle after the load), `HUNT_ROUNDS` (3), `HUNT_OPS` (120),
//! `HUNT_SEAL_AFTER` (1), `HUNT_WINDOWS` (1: the window scans before the
//! snapshot), `HUNT_STRIDE` (238: rank stride of the start keys),
//! `HUNT_SNAPS` (1: snapshot cycles), `HUNT_AFTER_N` (200) and
//! `HUNT_AFTER_STEP` (5) the puts after the snapshot, `HUNT_AFTER_COMMITS`
//! (1), `HUNT_PAUSE_MS` (0: a sleep between the commit and the scans),
//! `HUNT_READERS`, `HUNT_SNAP_THREADS`, `HUNT_TORN_THREADS`, `HUNT_HOGS`
//! (threads of the kinds named, started with the puts), `HUNT_STRICT`,
//! `HUNT_SNAP_STEP` (3), `HUNT_SNAP_LIMITS` (3,57,114), `HUNT_PREWARM`,
//! `HUNT_DROP_LATEST`, `HUNT_PUBLISH_DELAY_US` (a sleep in the publisher
//! between the swaps and the position), `HUNT_READER_DELAY_US` (a sleep in
//! a handle's scan between its decision and its first slot load),
//! `HUNT_OPT=name=value,...` (an engine option turned, see `apply_opts`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use supdb::db::dbgtrace;
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
    /// Extra `Latest` handles scanning on threads of their own.
    readers: usize,
    /// Extra `Snapshot` handles, pinned with the main one, scanning on
    /// threads of their own through the whole phase.
    snap_threads: usize,
    /// Extra `Latest` handles that scan the whole range of the first
    /// cycle's puts and require all of the commit or none of it.
    torn_threads: usize,
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
    /// The forms publisher sleeps this long between its slot swaps and
    /// its store of the position (a deliberately placed yield).
    publish_delay_us: u64,
    /// A handle's scan sleeps this long between deciding the forms are
    /// current to its bound and loading the first slot.
    reader_delay_us: u64,
}

fn hunt_cfg() -> Cfg {
    let upkeep = match env_str("HUNT_UPKEEP").as_deref().unwrap_or("bg1") {
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
        snap_threads: env_usize("HUNT_SNAP_THREADS", 0),
        torn_threads: env_usize("HUNT_TORN_THREADS", 0),
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
        publish_delay_us: env_usize("HUNT_PUBLISH_DELAY_US", 0) as u64,
        reader_delay_us: env_usize("HUNT_READER_DELAY_US", 0) as u64,
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
    r.read_all(key, |x| {
        v.push(String::from_utf8_lossy(&x[..x.len().min(24)]).into_owned())
    })
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

/// Whether a later write is in `got`: any value the snapshot's model never
/// held for its key.
fn foreign(got: &Window, at_snapshot: &Model) -> Option<(Vec<u8>, Vec<u8>)> {
    for (k, v) in &got.0 {
        let known = at_snapshot.get(k).is_some_and(|vs| vs.contains(v));
        if !known {
            return Some((k.clone(), v.clone()));
        }
    }
    None
}

/// The first failure any thread found, for the main thread to print.
type Failure = Arc<Mutex<Option<String>>>;

fn note(fail: &Failure, text: String) {
    let mut f = fail.lock().unwrap();
    if f.is_none() {
        *f = Some(text);
    }
}

struct Threads {
    stop: Arc<AtomicBool>,
    joins: Vec<std::thread::JoinHandle<u64>>,
}

impl Threads {
    fn finish(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        self.joins.into_iter().map(|j| j.join().unwrap()).sum()
    }
}

/// Threads that do not need to know about a snapshot: `Latest` readers
/// checking key order, and spinners.
fn start_background(db: &Db, cfg: &Cfg, starts: &[Vec<u8>], stop: &Arc<AtomicBool>) -> Threads {
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
                    x = x
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                }
                std::hint::black_box(x);
            }
            0
        }));
    }
    Threads {
        stop: stop.clone(),
        joins,
    }
}

/// Everything the engine can say about `key` through a handle that has just
/// answered wrongly, in the order that disturbs it least. The writer's and
/// the Latest handle's accounts are left out where the caller has none.
#[allow(clippy::too_many_arguments)]
fn capture(
    db: Option<&Db>,
    snap: &Reader,
    latest: Option<&Reader>,
    key: &[u8],
    start: &[u8],
    limit: usize,
    at_snapshot: &Model,
    now: Option<&Model>,
    why: &str,
    t_from: u64,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "HUNT-CAPTURE {why}");
    let t_fail = dbgtrace::now_ns();
    for line in dbgtrace::dump(t_from.saturating_sub(2_000_000)) {
        let _ = writeln!(s, "TRACE {line}");
    }
    let _ = writeln!(s, "TRACE (failure noticed at {:.3} us)", t_fail as f64 / 1000.0);
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
        now.and_then(|m| m.get(key))
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
    if let Some(db) = db {
        let _ = writeln!(s, "writer point read: {:?}", vals(db, key));
        let _ = writeln!(s, "writer.debug_key: {}", db.debug_key(key));
        let _ = writeln!(
            s,
            "forms_position {} snapshot_builds {} extends {} refreshes {} switches {} kept {:?} \
             upkeep_counts {:?}",
            db.forms_position(),
            db.snapshot_builds(),
            db.snapshot_extends(),
            db.snapshot_refreshes(),
            db.snapshot_switches(),
            db.snapshot_kept(),
            db.upkeep_counts()
        );
        let _ = writeln!(s, "writer.canonical_forms: {:?}", db.canonical_forms());
    }
    s
}

struct Tally {
    windows: u64,
    snap_scans: u64,
    thread_scans: u64,
}

/// The store the scenario starts from: loaded, flushed, settled, a seal in
/// flight (held), the rounds written, the start keys, the `Latest` handle,
/// and the model of all of it.
struct World {
    dir: PathBuf,
    db: Db,
    model: Model,
    keys: Vec<Vec<u8>>,
    starts: Vec<Vec<u8>>,
    latest: Option<Reader>,
}

/// `HUNT_OPT=name=value,name=value`: engine options taken away or turned,
/// to test a suspect mechanism by its absence. Booleans are 0 or 1.
fn apply_opts(o: &mut Options) {
    let Some(list) = env_str("HUNT_OPT") else {
        return;
    };
    for kv in list.split(',') {
        let (k, v) = kv.split_once('=').unwrap_or_else(|| panic!("HUNT_OPT {kv}"));
        let flag = v != "0";
        let n: usize = v.parse().unwrap_or(0);
        match k {
            "share_snapshot" => o.share_snapshot = flag,
            "commit_forms" => o.commit_forms = flag,
            "commit_forms_build" => o.commit_forms_build = flag,
            "forms_carry" => o.forms_carry = flag,
            "forms_rebase" => o.forms_rebase = flag,
            "forms_publish_lazily" => o.forms_publish_lazily = flag,
            "upkeep_alert" => o.upkeep_alert = flag,
            "upkeep_inline_bursts" => o.upkeep_inline_bursts = flag,
            "upkeep_settle_after_scans" => o.upkeep_settle_after_scans = flag,
            "forms_convert_unread" => o.forms_convert_unread = flag,
            "forms_convert_yield" => o.forms_convert_yield = flag,
            "scan_cache_ahead" => o.scan_cache_ahead = flag,
            "frozen_snaps" => o.frozen_snaps = flag,
            "overlay_merge" => o.overlay_merge = flag,
            "seal_snapshot" => o.seal_snapshot = flag,
            "snapshot_carry" => o.snapshot_carry = flag,
            "forms_pieces_only" => o.forms_pieces_only = flag,
            "forms_to_fit" => o.forms_to_fit = flag,
            "copy_exact" => o.copy_exact = flag,
            "publish_in_background" => o.publish_in_background = flag,
            "snapshot_adopt_behind" => o.snapshot_adopt_behind = n,
            "upkeep_inline_min" => o.upkeep_inline_min = n,
            "upkeep_alert_poll_us" => o.upkeep_alert_poll_us = n as u32,
            "forms_settle_backlog_pct" => o.forms_settle_backlog_pct = n,
            "upkeep_pass_pct" => o.upkeep_pass_pct = n,
            other => panic!("HUNT_OPT: unknown option {other}"),
        }
    }
}

fn build_world(cfg: &Cfg, name: &str) -> World {
    let dir = scratch(name);
    let mut opts = Options {
        partition_bytes: Some(cfg.part_kib << 10),
        scan_block_cache: true,
        upkeep: cfg.upkeep,
        ..Options::default()
    };
    apply_opts(&mut opts);
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
    let latest = Some(db.reader().unwrap());
    World {
        dir,
        db,
        model,
        keys,
        starts,
        latest,
    }
}

const LIMITS: [usize; 10] = [1, 2, 3, 5, 17, 56, 57, 58, 114, usize::MAX];

fn windows_phase(cfg: &Cfg, w: &World, tally: &mut Tally) -> Result<(), String> {
    for start in &w.starts {
        for &limit in &LIMITS {
            let cap = limit.min(1500);
            let want = window(&w.model, start, cap);
            let got = scanned(&w.db, start, cap);
            if cfg.strict {
                if let Some((key, line)) = parting(&got, &want) {
                    return Err(format!(
                        "HUNT-FAIL windows-writer from {} for {cap}: {line}\nkey {}",
                        show(start),
                        show(&key)
                    ));
                }
            }
            let got = scanned(w.latest.as_ref().unwrap(), start, cap);
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
    Ok(())
}

/// One cycle's writes: the keys it puts and the value it puts to each.
fn cycle_keys(cfg: &Cfg, keys: &[Vec<u8>], cycle: usize) -> (Vec<Vec<u8>>, Vec<u8>) {
    let step = cfg.after_step.max(1);
    let ks: Vec<Vec<u8>> = keys
        .iter()
        .skip(cycle % step)
        .step_by(step)
        .take(cfg.after_n)
        .cloned()
        .collect();
    (ks, format!("after the snapshot {cycle}").into_bytes())
}

/// Extra `Snapshot` handles and `Latest` handles that check the commit's
/// atomicity, started before the puts.
fn start_watchers(
    w: &World,
    cfg: &Cfg,
    at_snapshot: &Arc<Model>,
    targets: &[Vec<u8>],
    tag: &[u8],
    stop: &Arc<AtomicBool>,
    fail: &Failure,
) -> Threads {
    let mut joins = Vec::new();
    for t in 0..cfg.snap_threads {
        let r = w.db.reader().unwrap();
        r.snapshot();
        let (stop, fail, at, starts) = (
            stop.clone(),
            fail.clone(),
            at_snapshot.clone(),
            w.starts.clone(),
        );
        let limits = cfg.snap_limits.clone();
        joins.push(std::thread::spawn(move || {
            let mut rng = Rng(0xA11CE + t as u64 * 104729 + 1);
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let s = &starts[rng.below(starts.len())];
                let l = limits[rng.below(limits.len())];
                let got = scanned(&r, s, l);
                n += 1;
                if let Some((k, v)) = foreign(&got, &at) {
                    let why = format!(
                        "snapshot thread {t}: a value the snapshot never held: {} = {}",
                        show(&k),
                        short(&v)
                    );
                    let cap = capture(
                        None,
                        &r,
                        None,
                        &k,
                        s,
                        l,
                        &at,
                        None,
                        &why,
                        dbgtrace::now_ns().saturating_sub(30_000_000),
                    );
                    note(&fail, format!("HUNT-FAIL thread-later-write {why}\n{cap}"));
                    stop.store(true, Ordering::Relaxed);
                }
            }
            n
        }));
    }
    // A commit is all or none: a `Latest` handle's scan over every key the
    // first cycle puts finds the cycle's value under all of them or under
    // none.
    if cfg.torn_threads > 0 && !targets.is_empty() {
        let first = targets.first().unwrap().clone();
        let limit = w
            .model
            .range(first.clone()..=targets.last().unwrap().clone())
            .count();
        for t in 0..cfg.torn_threads {
            let r = w.db.reader().unwrap();
            let (stop, fail, first, tag) = (stop.clone(), fail.clone(), first.clone(), tag.to_vec());
            let want = targets.len();
            joins.push(std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let got = scanned(&r, &first, limit);
                    n += 1;
                    let seen = got.0.iter().filter(|(_, v)| *v == tag).count();
                    if seen != 0 && seen != want {
                        let missing = want - seen;
                        let why = format!(
                            "torn thread {t}: a Latest scan found {seen} of the commit's {want} \
                             puts ({missing} missing)"
                        );
                        note(
                            &fail,
                            format!(
                                "HUNT-FAIL torn {why}\n{}",
                                dbgtrace::dump(dbgtrace::now_ns().saturating_sub(30_000_000))
                                    .iter()
                                    .map(|l| format!("TRACE {l}"))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            ),
                        );
                        stop.store(true, Ordering::Relaxed);
                    }
                }
                n
            }));
        }
    }
    Threads {
        stop: stop.clone(),
        joins,
    }
}

/// One run of the scenario. Err carries the capture of the first wrong
/// answer through the snapshot handle (or the account of a wrong answer of
/// the writer's or the Latest handle's in the window phase).
fn scenario(cfg: &Cfg, iter: usize) -> Result<Tally, String> {
    let mut w = build_world(cfg, &format!("i{iter}"));
    let stop = Arc::new(AtomicBool::new(false));
    let fail: Failure = Arc::new(Mutex::new(None));
    let background = start_background(&w.db, cfg, &w.starts, &stop);
    let mut tally = Tally {
        windows: 0,
        snap_scans: 0,
        thread_scans: 0,
    };
    let mut watchers: Option<Threads> = None;
    let result = (|| -> Result<(), String> {
        if cfg.windows {
            windows_phase(cfg, &w, &mut tally)?;
        }
        if cfg.drop_latest {
            w.latest = None;
        }
        for cycle in 0..cfg.snaps {
            let snap = w.db.reader().unwrap();
            let t_pinned = dbgtrace::now_ns();
            snap.snapshot();
            let at_snapshot = w.model.clone();
            let (ks, tag) = cycle_keys(cfg, &w.keys, cycle);
            dbgtrace::set_publish_delay_us(cfg.publish_delay_us);
            dbgtrace::set_reader_delay_us(cfg.reader_delay_us);
            if cycle == 0 {
                watchers = Some(start_watchers(
                    &w,
                    cfg,
                    &Arc::new(at_snapshot.clone()),
                    &ks,
                    &tag,
                    &stop,
                    &fail,
                ));
            }
            let per = ks.len().div_ceil(cfg.after_commits.max(1)).max(1);
            let scan_pass = |tally: &mut Tally, w: &World, t_commit: u64| -> Result<(), String> {
                for start in w.starts.iter().step_by(cfg.snap_step.max(1)) {
                    for &limit in &cfg.snap_limits {
                        dbgtrace::ev(11, 255, 0, 0);
                        let got = scanned(&snap, start, limit);
                        dbgtrace::ev(11, 255, 1, 0);
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
                                    format!(
                                        "a value the snapshot never held: {} = {}",
                                        show(k),
                                        short(v)
                                    ),
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
                                "HUNT-FAIL snapshot-{kind} cycle {cycle} from {} for {limit}: \
                                 {line}\n{}",
                                show(start),
                                capture(
                                    Some(&w.db),
                                    &snap,
                                    w.latest.as_ref(),
                                    &key,
                                    start,
                                    limit,
                                    &at_snapshot,
                                    Some(&w.model),
                                    &line,
                                    t_pinned.min(t_commit)
                                )
                            ));
                        }
                    }
                }
                Ok(())
            };
            if cfg.prewarm {
                scan_pass(&mut tally, &w, dbgtrace::now_ns())?;
            }
            for chunk in ks.chunks(per) {
                for k in chunk {
                    w.db.put(k, &tag);
                    w.model.insert(k.clone(), vec![tag.clone()]);
                }
                let t_commit = dbgtrace::now_ns();
                dbgtrace::ev(10, 255, 0, 0);
                w.db.commit().unwrap();
                dbgtrace::ev(10, 255, 1, 0);
                if cfg.pause_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(cfg.pause_ms));
                }
                scan_pass(&mut tally, &w, t_commit)?;
                if cfg.strict {
                    for start in w.starts.iter().step_by(cfg.snap_step.max(1)) {
                        for &limit in &cfg.snap_limits {
                            let now = scanned(&w.db, start, limit);
                            if let Some((key, line)) =
                                parting(&now, &window(&w.model, start, limit))
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
                if let Some(text) = fail.lock().unwrap().take() {
                    return Err(text);
                }
            }
            snap.release();
        }
        Ok(())
    })();
    tally.thread_scans = background.finish();
    if let Some(wt) = watchers {
        tally.thread_scans += wt.finish();
    }
    let result = match (result, fail.lock().unwrap().take()) {
        (Ok(()), Some(text)) => Err(text),
        (r, _) => r,
    };
    let World { dir, db, latest, .. } = w;
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
        thread_scans: 0,
    };
    for iter in 0..cfg.iters {
        match scenario(&cfg, iter) {
            Ok(t) => {
                total.windows += t.windows;
                total.snap_scans += t.snap_scans;
                total.thread_scans += t.thread_scans;
            }
            Err(e) => {
                eprintln!("{e}");
                eprintln!("HUNT-CFG {cfg:?} iter {iter}");
                panic!("hunt failure at iteration {iter}");
            }
        }
    }
    eprintln!(
        "HUNT-OK iters {} windows {} snap_scans {} thread_scans {}",
        cfg.iters, total.windows, total.snap_scans, total.thread_scans
    );
}

/// The schedule forced. The forms publisher (the upkeep thread under
/// `Background`, the writer under `Inline`) is parked after it has swapped
/// a commit's forms into the state's slots and before it stores the position
/// they are current to; a handle pinned at the commit before scans the
/// first window of the store there. It has to answer the store as it was
/// pinned, and answers the commit it was not pinned at. Under `Inline` the
/// writer is the thread parked, so the handle scans on a thread of its own.
#[test]
fn hunt_forced() {
    let mut cfg = hunt_cfg();
    // The schedule needs the first commit after the snapshot to publish.
    cfg.windows = true;
    let w = build_world(&cfg, "forced");
    let mut tally = Tally {
        windows: 0,
        snap_scans: 0,
        thread_scans: 0,
    };
    windows_phase(&cfg, &w, &mut tally).expect("the windows are right before the snapshot");
    let World {
        dir,
        mut db,
        mut model,
        keys,
        latest,
        ..
    } = w;
    let snap = db.reader().unwrap();
    snap.snapshot();
    let at_snapshot = model.clone();
    let position_before = db.forms_position();
    let (ks, tag) = cycle_keys(&cfg, &keys, 0);
    for k in &ks {
        db.put(k, &tag);
        model.insert(k.clone(), vec![tag.clone()]);
    }
    dbgtrace::hold_forms_position(true);
    let t_commit = dbgtrace::now_ns();
    let inline = cfg.upkeep == Upkeep::Inline;
    // What the handle answers while the publisher is parked: the first
    // window, as the original test's first scan.
    let scan_parked = move |snap: Reader| -> (Reader, Window, bool) {
        let t = std::time::Instant::now();
        while !dbgtrace::forms_hold_parked() && t.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        let parked = dbgtrace::forms_hold_parked();
        let got = scanned(&snap, b"", 3);
        // Under `Inline` the parked publisher is the thread that has to
        // return from the commit before the test can lift the hold.
        if inline {
            dbgtrace::hold_forms_position(false);
        }
        (snap, got, parked)
    };
    let (snap, got, parked) = if inline {
        let helper = std::thread::spawn(move || scan_parked(snap));
        db.commit().unwrap();
        helper.join().unwrap()
    } else {
        db.commit().unwrap();
        scan_parked(snap)
    };
    // Released only after the scan: the publisher stores the position and
    // the hold is over.
    let position_during = if inline { usize::MAX } else { db.forms_position() };
    dbgtrace::hold_forms_position(false);
    let want = window(&at_snapshot, b"", 3);
    let wrong = parting(&got, &want);
    // The control: the same scan once the position is stored.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let after = scanned(&snap, b"", 3);
    let after_wrong = parting(&after, &want);
    let key = got
        .0
        .iter()
        .find(|(k, v)| model.get(k).is_some_and(|vs| vs.contains(v)) && !at_snapshot[k].contains(v))
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| b"key-00000000".to_vec());
    let report = capture(
        Some(&db),
        &snap,
        latest.as_ref(),
        &key,
        b"",
        3,
        &at_snapshot,
        Some(&model),
        "the forced schedule",
        t_commit,
    );
    eprintln!("{report}");
    eprintln!(
        "HUNT-FORCED upkeep {:?} publisher parked {parked} forms_position before {} during {} \
         wrong-while-parked {:?} wrong-after-release {:?}",
        cfg.upkeep,
        position_before,
        position_during,
        wrong.as_ref().map(|(_, l)| l.clone()),
        after_wrong.as_ref().map(|(_, l)| l.clone()),
    );
    drop(latest);
    drop(snap);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(parked, "the publisher never parked: the schedule was not forced");
    assert!(
        wrong.is_none(),
        "the snapshot handle read a write made after it while the publisher was parked"
    );
    assert!(after_wrong.is_none(), "the snapshot is wrong after the release too");
}
