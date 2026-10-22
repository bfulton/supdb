//! A differential fuzz of the engine against a model.
//!
//! A seed draws a configuration (tiny seals and partitions, inline limits,
//! the upkeep and the segment work inline or on threads, the sync policy,
//! the block cache) and a pool of keys, then drives two stores of that
//! configuration through the same random operations while a model of keys
//! and values says what every read must answer: appends, runs of values,
//! deletes and puts, seals, flushes and settles, ordered bursts that become
//! direct runs with commit markers, claimed handles at `Latest` and at a
//! `Snapshot`, clean closes and crashes with the reopen. `z` is the store
//! the handles read through and `r` its twin, so a failure that depends on
//! a schedule has two chances in a seed; the checks name the store.
//!
//! The answers are held to the model through the writer, through fresh and
//! claimed handles, by count, and by a raw scan of the whole store, whose
//! keys must come in order with each key's values together. A failure
//! names its seed, the step it was found at and the configuration, and one
//! seed is rerun alone:
//!
//! ```text
//! ZFUZZ_FROM=36 ZFUZZ_TO=37 cargo test --profile checked --test fuzz \
//!     fuzz_range -- --exact --nocapture
//! ```
//!
//! A long range is one process per seed, each in a `TMPDIR` of its own and
//! under a timeout, so that a hang is a result and not a stall.
//!
//! Knobs, all environment variables:
//!
//! - `ZFUZZ_FROM`, `ZFUZZ_TO`: the seeds, half open. The default is a few
//!   seeds that run in seconds.
//! - `ZFUZZ_SEEDS`: a comma separated list of seeds in place of the range.
//! - `ZFUZZ_CACHE`: `1` turns the block cache on for the one seed in five
//!   that asks for it, `all` for every seed, and `off` for none while
//!   drawing as `1` does, so that a seed that failed under `1` or `all`
//!   reruns as the same configuration and the same operations with the
//!   cache off. Unset, no seed has the cache and the draw is not made, so a
//!   seed's other choices are not those it makes under the three.
//! - `ZFUZZ_PROMOTE`: set, with the form forced and the cache on, the `z`
//!   store holds a block read a few entries' worth as a copy in its slot
//!   under a small budget (`Options::zpages_promote_copies`).
//! - `ZFUZZ_OPTVARY`: set, a seed turns compaction or promotion off, each
//!   with a chance of 15%.
//! - `ZFUZZ_SHAPES`: the key shapes a seed may draw from, as indexes from 0
//!   to 7 separated by commas.
//! - `ZFUZZ_NOPUT`, `ZFUZZ_NOBURST`: set, no puts, or no ordered bursts.
//! - `ZFUZZ_SEEDKEY=1`: the store's first write is a key of its own, and
//!   one goes ahead of every write of the empty key, so that no write of
//!   the empty key is the first of its memtable.
//! - `ZFUZZ_VERBOSE=1`: print each seed's configuration. `ZFUZZ_TRACE=1`:
//!   print each event as it is reached, for the last line of a hang.
//! - `ZFUZZ_SCANEVERY=1`: scan the whole store through the writer after
//!   every step that leaves nothing staged, so a scan that goes wrong is
//!   caught at the step it does and not at the next full check; a
//!   mismatch names the first key that differs and the writer's every
//!   source for it (`Reader::debug_key`).
//! - `ZFUZZ_OPS` (4000), `ZFUZZ_SNAPS=1`: the writer's steps in
//!   `concurrent_range`, and whether its readers claim snapshots.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use supdb::{Db, Options, Reader, SyncPolicy, Upkeep};

/// The seeds a run with no range takes: few, because the test lives in the gate.
const DEFAULT_SEEDS: u64 = 4;

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| env_flag("ZFUZZ_TRACE"))
}

macro_rules! trace {
    ($($arg:tt)*) => {
        if trace_on() {
            eprintln!($($arg)*);
        }
    };
}

thread_local! {
    /// The step a seed has reached. The checks that fail and the calls that
    /// return an error are far from the loop that knows it, and the failure
    /// line names it.
    static STEP: std::cell::Cell<i64> = const { std::cell::Cell::new(-1) };
}

type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-fuzz-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..8 {
            r.next();
        }
        r
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

fn show(k: &[u8]) -> String {
    k.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[derive(Clone, Debug)]
struct Cfg {
    seal_bytes: usize,
    partition_bytes: Option<usize>,
    l0_trigger: usize,
    inline_bytes: Option<usize>,
    cache: bool,
    upkeep: Upkeep,
    bg: bool,
    sync: SyncPolicy,
    ops: usize,
    keys: usize,
    big: bool,
    compact: bool,
    promote: bool,
}

/// Whether a seed runs with the block cache on. The chance is drawn whenever
/// the cache is wanted, so `all` and `off` leave the rest of a seed's stream
/// as `1` has it.
fn cache_asked(rng: &mut Rng, want_cache: bool) -> bool {
    if !want_cache {
        return false;
    }
    let drawn = rng.chance(20);
    match std::env::var("ZFUZZ_CACHE").as_deref() {
        Ok("all") => true,
        Ok("off") => false,
        _ => drawn,
    }
}

fn pick_cfg(rng: &mut Rng, want_cache: bool) -> Cfg {
    Cfg {
        seal_bytes: [2 << 10, 4 << 10, 8 << 10, 16 << 10, 32 << 10][rng.below(5)],
        partition_bytes: match rng.below(4) {
            0 => None,
            1 => Some(1 << 10),
            2 => Some(4 << 10),
            _ => Some(16 << 10),
        },
        l0_trigger: 2 + rng.below(3),
        inline_bytes: if rng.chance(30) { Some(1 << 16) } else { None },
        cache: cache_asked(rng, want_cache),
        upkeep: match rng.below(5) {
            0 => Upkeep::Inline,
            1 => Upkeep::Background(2),
            2 => Upkeep::Background(3),
            _ => Upkeep::Background(1),
        },
        bg: rng.chance(75),
        sync: if rng.chance(50) {
            SyncPolicy::Always
        } else {
            SyncPolicy::EveryN(8)
        },
        ops: 600 + rng.below(1800),
        keys: 40 + rng.below(500),
        big: rng.chance(25),
        compact: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
        promote: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
    }
}

fn opts(c: &Cfg) -> Options {
    let mut o = Options {
        scan_block_cache: c.cache,
        seal_bytes: c.seal_bytes,
        partition_bytes: c.partition_bytes,
        l0_trigger: c.l0_trigger,
        upkeep: c.upkeep,
        publish_in_background: c.bg,
        sync: c.sync,
        compact: c.compact,
        promote: c.promote,
        ..Options::default()
    };
    if let Some(i) = c.inline_bytes {
        o.inline_bytes = i;
    }
    o
}

/// The `z` store's options: the pair's, and under `ZFUZZ_ZPAGES` the
/// compact form forced on with no size floor, so that every partition of
/// `z` is read and scanned through a form file while `r` reads the raw form
/// over the same writes -- the strongest check a second read path has.
fn opts_z(c: &Cfg) -> Options {
    let mut o = opts(c);
    if env_flag("ZFUZZ_ZPAGES") {
        o.zpages = supdb::ZPages::Always;
        o.zpages_min_bytes = 0;
    }
    // Under `ZFUZZ_PROMOTE` with the form forced and the cache on, a block
    // read a few entries' worth is held as a copy in its slot and the
    // cache sheds under a small budget (`Options::zpages_promote_copies`),
    // so the copies, the sheds and the writes spliced into copies are read
    // against the raw store over the same writes.
    if env_flag("ZFUZZ_PROMOTE") && c.cache {
        o.promote_entries = 8;
        o.zpages_promote_copies = true;
        o.scan_cache_bytes = 32 << 10;
        o.forms_to_fit = true;
    }
    o
}

fn key_pool(rng: &mut Rng, n: usize) -> Vec<Vec<u8>> {
    let mut set: BTreeSet<Vec<u8>> = BTreeSet::new();
    let shapes = 1 + rng.below(3);
    let allowed: Vec<usize> = std::env::var("ZFUZZ_SHAPES")
        .ok()
        .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| (0..8).collect());
    let chosen: Vec<usize> = (0..shapes)
        .map(|_| allowed[rng.below(allowed.len())])
        .collect();
    while set.len() < n {
        let k: Vec<u8> = match chosen[rng.below(chosen.len())] {
            0 => format!("k{:05}", rng.below(100_000)).into_bytes(),
            1 => format!("{:016}", rng.below(1 << 40)).into_bytes(),
            2 => {
                let l = rng.below(40);
                (0..l).map(|_| b"ab"[rng.below(2)]).collect()
            }
            3 => {
                let l = rng.below(24);
                let mut v = vec![1u8];
                v.extend((0..l).map(|_| [0u8, 1, 0xfe, 0xff, b'a'][rng.below(5)]));
                v
            }
            4 => {
                let l = 15 + rng.below(3);
                (0..l).map(|_| b'0' + rng.below(10) as u8).collect()
            }
            5 => {
                let mut v = b"a-very-long-common-prefix-for-every-key-in-this-shape/".repeat(2);
                v.extend(format!("{:06}", rng.below(1_000_000)).bytes());
                v
            }
            6 => {
                if rng.chance(5) {
                    Vec::new()
                } else {
                    format!("{}", rng.below(1000)).into_bytes()
                }
            }
            _ => format!("user{:08}", rng.below(100_000_000)).into_bytes(),
        };
        set.insert(k);
    }
    set.into_iter().collect()
}

fn value(rng: &mut Rng, step: u64, big: bool) -> Vec<u8> {
    match rng.below(24) {
        0 => Vec::new(),
        1..=9 => format!("v{step}-{}", "x".repeat(rng.below(60))).into_bytes(),
        10..=13 => (0..1 + rng.below(100))
            .map(|_| (rng.next() >> 24) as u8)
            .collect(),
        14..=17 => (rng.below(1 << 20) as u32).to_le_bytes().to_vec(),
        18 if big => (0..300 + rng.below(2700))
            .map(|_| (rng.next() >> 24) as u8)
            .collect(),
        19 => b"the quick brown fox jumps over the lazy dog 0123456789 "
            .repeat(2 + rng.below(8))
            .to_vec(),
        _ => format!("{:08}", rng.below(100_000_000)).into_bytes(),
    }
}

#[derive(Clone)]
enum Op {
    Append(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
    Put(Vec<u8>, Vec<u8>),
}

fn apply(model: &mut Model, op: &Op) {
    match op {
        Op::Append(k, v) => model.entry(k.clone()).or_default().push(v.clone()),
        Op::Delete(k) => {
            model.entry(k.clone()).or_default().clear();
        }
        Op::Put(k, v) => {
            let e = model.entry(k.clone()).or_default();
            e.clear();
            e.push(v.clone());
        }
    }
}

fn read_vec(r: &Reader, key: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut v = Vec::new();
    let n = r
        .read_all(key, |x| v.push(x.to_vec()))
        .map_err(|e| format!("read error {e}"))?;
    if n as usize != v.len() {
        return Err(format!("read_all returned {n} but emitted {}", v.len()));
    }
    Ok(v)
}

/// The elements of the sorted `a` that the sorted `b` does not match, as multisets.
fn minus<'a>(a: &[&'a [u8]], b: &[&[u8]]) -> Vec<&'a [u8]> {
    let (mut out, mut j) = (Vec::new(), 0);
    for x in a {
        while j < b.len() && b[j] < *x {
            j += 1;
        }
        if j < b.len() && b[j] == *x {
            j += 1;
        } else {
            out.push(*x);
        }
    }
    out
}

/// What kind of wrong an answer is, so that failures sort by what failed and
/// not by which key or how many values.
fn shape(got: &[Vec<u8>], want: &[Vec<u8>]) -> String {
    if want.is_empty() {
        return "a deleted or absent key listed".into();
    }
    if got.is_empty() {
        return "a present key read as empty".into();
    }
    let first = got.iter().zip(want).position(|(g, w)| g != w);
    let mut g: Vec<&[u8]> = got.iter().map(|v| v.as_slice()).collect();
    let mut w: Vec<&[u8]> = want.iter().map(|v| v.as_slice()).collect();
    g.sort_unstable();
    w.sort_unstable();
    let (extra, missing) = (minus(&g, &w), minus(&w, &g));
    let kind = if extra.is_empty() && missing.is_empty() {
        "values out of order".to_string()
    } else if missing.is_empty() {
        if extra.iter().all(|x| w.binary_search(x).is_ok()) {
            format!("{} value(s) duplicated", extra.len())
        } else {
            format!("{} value(s) not in the model's list", extra.len())
        }
    } else if extra.is_empty() {
        format!("{} value(s) missing", missing.len())
    } else {
        format!(
            "{} value(s) missing and {} not in the model's list",
            missing.len(),
            extra.len()
        )
    };
    match first {
        Some(i) => format!("{kind}, first difference at {i}"),
        None => format!("{kind}, one a prefix of the other"),
    }
}

/// Record a failed check, and say so at once: a seed that goes on to hang,
/// abort or fail a call has still named the first wrong answer it was given.
fn fail(fails: &mut Vec<String>, msg: String) {
    let shown: String = msg.chars().take(600).collect();
    eprintln!("CHECK FAILED [step {}]: {shown}", STEP.get());
    fails.push(msg);
}

fn check_keys(label: &str, r: &Reader, model: &Model, keys: &[Vec<u8>], fails: &mut Vec<String>) {
    for k in keys {
        let want = model.get(k).cloned().unwrap_or_default();
        match read_vec(r, k) {
            Ok(got) if got == want => {}
            Ok(got) => {
                fail(fails, format!(
                    "{label}: read of key {} (len {}): {}: got {} values {:?}.., want {} values {:?}..",
                    show(k),
                    k.len(),
                    shape(&got, &want),
                    got.len(),
                    got.iter().take(3).map(|v| v.len()).collect::<Vec<_>>(),
                    want.len(),
                    want.iter().take(3).map(|v| v.len()).collect::<Vec<_>>(),
                ));
            }
            Err(e) => fail(fails, format!("{label}: read of key {}: {e}", show(k))),
        }
        match r.count(k) {
            Ok(n) if n as usize == want.len() => {}
            Ok(n) => fail(
                fails,
                format!(
                    "{label}: count of key {} (len {}): got {n}, want {}",
                    show(k),
                    k.len(),
                    want.len()
                ),
            ),
            Err(e) => fail(fails, format!("{label}: count of key {}: {e}", show(k))),
        }
        if fails.len() > 20 {
            return;
        }
    }
}

/// A raw scan of the whole store, as the model it answers and the faults of
/// its order: keys ascending, each key's values together.
fn scan_all(r: &Reader) -> (Model, Vec<String>) {
    let mut m: Model = BTreeMap::new();
    let mut faults = Vec::new();
    let mut last: Option<Vec<u8>> = None;
    r.scan(b"", usize::MAX, |k, v| {
        match &last {
            Some(l) if l.as_slice() == k => {}
            Some(l) if l.as_slice() > k => faults.push(format!(
                "scan emitted key {} after key {}",
                show(k),
                show(l)
            )),
            _ => {
                if m.contains_key(k) {
                    faults.push(format!(
                        "scan emitted key {} again after other keys",
                        show(k)
                    ));
                }
            }
        }
        last = Some(k.to_vec());
        m.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    faults.truncate(3);
    (m, faults)
}

fn live(model: &Model) -> Model {
    model
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn absent_around(keys: &[Vec<u8>], rng: &mut Rng) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for k in keys {
        if rng.chance(30) {
            let mut a = k.clone();
            a.push(0);
            v.push(a);
            let mut b = k.clone();
            b.push(0xff);
            v.push(b);
            if let Some((&last, head)) = k.split_last() {
                v.push(head.to_vec());
                let mut c = head.to_vec();
                c.push(last.wrapping_add(1));
                v.push(c);
                let mut d = head.to_vec();
                d.push(last.wrapping_sub(1));
                v.push(d);
            }
        }
    }
    v.push(Vec::new());
    v.push(vec![0]);
    v.push(vec![0xff; 8]);
    v.push(b"zzzzzzzz".to_vec());
    v
}

/// The first key the model has that `got` answers differently.
fn first_diff_key(got: &Model, want: &Model) -> Option<Vec<u8>> {
    want.iter()
        .find(|(k, w)| got.get(*k) != Some(w))
        .map(|(k, _)| k.clone())
}

fn model_diff(got: &Model, want: &Model) -> String {
    let mut out = Vec::new();
    for (k, w) in want {
        match got.get(k) {
            None => out.push(format!(
                "present key {} missing ({} values)",
                show(k),
                w.len()
            )),
            Some(g) if g != w => out.push(format!(
                "key {}: {}: got {} values (lens {:?}), want {} (lens {:?})",
                show(k),
                shape(g, w),
                g.len(),
                g.iter().take(4).map(|v| v.len()).collect::<Vec<_>>(),
                w.len(),
                w.iter().take(4).map(|v| v.len()).collect::<Vec<_>>()
            )),
            _ => {}
        }
        if out.len() >= 3 {
            break;
        }
    }
    for k in got.keys() {
        if !want.contains_key(k) {
            out.push(format!(
                "deleted or absent key {} listed ({} values)",
                show(k),
                got[k].len()
            ));
            if out.len() >= 5 {
                break;
            }
        }
    }
    format!(
        "{} keys got, {} want; {}",
        got.len(),
        want.len(),
        out.join("; ")
    )
}

struct Handle {
    r: Reader,
    snap: Option<Model>,
}

struct Stats {
    reopens: u64,
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// One seed, whose failure line carries the step it was at and the
/// configuration it drew, whether the seed failed a check, returned an
/// error or panicked.
fn run_seed(seed: u64, want_cache: bool, verbose: bool) -> Result<Stats, String> {
    let mut rng0 = Rng::new(seed);
    let cfg0 = pick_cfg(&mut rng0, want_cache);
    STEP.set(-1);
    let run = std::panic::catch_unwind(|| run_seed_inner(seed, want_cache, verbose));
    let at = format!(
        "[step {} of {}] [cfg {}]",
        STEP.get(),
        cfg0.ops,
        cfg_line(&cfg0)
    );
    match run {
        Ok(Ok(st)) => Ok(st),
        Ok(Err(e)) => Err(format!("{e} {at}")),
        Err(p) => Err(format!("PANIC {} {at}", panic_text(p.as_ref()))),
    }
}

fn cfg_line(c: &Cfg) -> String {
    format!(
        "seal={} part={:?} l0={} inline={:?} cache={} upkeep={:?} bg={} sync={:?} big={}",
        c.seal_bytes,
        c.partition_bytes,
        c.l0_trigger,
        c.inline_bytes,
        c.cache,
        c.upkeep,
        c.bg,
        c.sync,
        c.big
    )
}

fn run_seed_inner(seed: u64, want_cache: bool, verbose: bool) -> Result<Stats, String> {
    let mut rng = Rng::new(seed);
    let cfg = pick_cfg(&mut rng, want_cache);
    let tag = format!("s{seed}");
    let (dz, dr) = (scratch(&format!("{tag}-z")), scratch(&format!("{tag}-r")));
    if verbose {
        eprintln!("seed {seed}: {cfg:?}");
    }
    let mut z = Db::create(&dz, opts_z(&cfg)).map_err(|e| format!("create z: {e}"))?;
    let mut r = Db::create(&dr, opts(&cfg)).map_err(|e| format!("create r: {e}"))?;
    let seedkey_first = env_flag("ZFUZZ_SEEDKEY");
    let mut model: Model = BTreeMap::new();
    let mut staged: Vec<Op> = Vec::new();
    let mut pool = key_pool(&mut rng, cfg.keys);
    if seedkey_first {
        let seedkey = b"\x01seed".to_vec();
        z.append(&seedkey, b"s");
        r.append(&seedkey, b"s");
        z.commit().map_err(|e| format!("commit z: {e}"))?;
        r.commit().map_err(|e| format!("commit r: {e}"))?;
        model
            .entry(seedkey.clone())
            .or_default()
            .push(b"s".to_vec());
        pool.push(seedkey);
    }
    let mut hi = 0u64;
    let mut handles: Vec<Handle> = Vec::new();
    let mut st = Stats { reopens: 0 };
    let mut fails: Vec<String> = Vec::new();
    let mut absent = absent_around(&pool, &mut rng);

    macro_rules! both {
        ($f:expr) => {{
            $f(&mut z);
            $f(&mut r);
        }};
    }
    let all_probe = |pool: &[Vec<u8>], absent: &[Vec<u8>]| -> Vec<Vec<u8>> {
        pool.iter().chain(absent.iter()).cloned().collect()
    };

    let scan_every = env_flag("ZFUZZ_SCANEVERY");
    for step in 0..cfg.ops as u64 {
        STEP.set(step as i64);
        let k = pool[rng.below(pool.len())].clone();
        if seedkey_first && k.is_empty() {
            let seedkey = b"\x01seed".to_vec();
            staged.push(Op::Append(seedkey.clone(), b"e".to_vec()));
            both!(|d: &mut Db| d.append(&seedkey, b"e"));
        }
        match rng.below(100) {
            0..=44 => {
                let v = value(&mut rng, step, cfg.big);
                staged.push(Op::Append(k.clone(), v.clone()));
                both!(|d: &mut Db| d.append(&k, &v));
            }
            45..=52 => {
                for _ in 0..2 + rng.below(8) {
                    let v = if rng.chance(50) {
                        // a fixed width run
                        ((rng.below(1 << 16)) as u32).to_le_bytes().to_vec()
                    } else {
                        value(&mut rng, step, cfg.big)
                    };
                    staged.push(Op::Append(k.clone(), v.clone()));
                    both!(|d: &mut Db| d.append(&k, &v));
                }
            }
            53..=60 => {
                staged.push(Op::Delete(k.clone()));
                both!(|d: &mut Db| d.delete(&k));
            }
            61..=65 if std::env::var("ZFUZZ_NOPUT").is_err() => {
                let v = value(&mut rng, step, cfg.big);
                staged.push(Op::Put(k.clone(), v.clone()));
                both!(|d: &mut Db| d.put(&k, &v));
            }
            _ => {}
        }
        // The writer's own reads see the staged batch.
        if rng.chance(3) {
            let mut dirty = model.clone();
            for op in &staged {
                apply(&mut dirty, op);
            }
            let sample: Vec<Vec<u8>> = (0..8)
                .map(|_| pool[rng.below(pool.len())].clone())
                .collect();
            check_keys("z writer, staged", &z, &dirty, &sample, &mut fails);
            check_keys("r writer, staged", &r, &dirty, &sample, &mut fails);
        }
        let commit_now = rng.chance(9);
        if commit_now {
            z.commit().map_err(|e| format!("commit z: {e}"))?;
            r.commit().map_err(|e| format!("commit r: {e}"))?;
            for op in staged.drain(..) {
                apply(&mut model, &op);
            }
            // The handles at their latest.
            for (i, h) in handles.iter().enumerate() {
                if h.snap.is_none() && rng.chance(20) {
                    let sample: Vec<Vec<u8>> = (0..6)
                        .map(|_| pool[rng.below(pool.len())].clone())
                        .collect();
                    check_keys(
                        &format!("handle {i} latest"),
                        &h.r,
                        &model,
                        &sample,
                        &mut fails,
                    );
                }
            }
        }
        // Shape-changing events, only when nothing is staged (a seal commits
        // what is staged and the model is committed here).
        let event = rng.below(1000);
        if staged.is_empty() || commit_now {
            if event < 170 {
                trace!("seed {seed} step {step}: event {event}");
            }
            match event {
                0..=29 => {
                    z.commit().map_err(|e| format!("commit z: {e}"))?;
                    r.commit().map_err(|e| format!("commit r: {e}"))?;
                    for op in staged.drain(..) {
                        apply(&mut model, &op);
                    }
                    z.seal().map_err(|e| format!("seal z: {e}"))?;
                    r.seal().map_err(|e| format!("seal r: {e}"))?;
                }
                30..=44 => {
                    z.commit().map_err(|e| format!("commit z: {e}"))?;
                    r.commit().map_err(|e| format!("commit r: {e}"))?;
                    for op in staged.drain(..) {
                        apply(&mut model, &op);
                    }
                    z.flush().map_err(|e| format!("flush z: {e}"))?;
                    r.flush().map_err(|e| format!("flush r: {e}"))?;
                }
                45..=64 => {
                    z.settle().map_err(|e| format!("settle z: {e}"))?;
                    r.settle().map_err(|e| format!("settle r: {e}"))?;
                }
                65..=94 => {
                    // The builder, with or without a settle's worth of quiet.
                    z.settle().map_err(|e| format!("settle z: {e}"))?;
                }
                95..=104 if std::env::var("ZFUZZ_NOBURST").is_err() => {
                    // An ordered burst: keys above the store's greatest, a
                    // direct run with markers in its record region.
                    let n = 5 + rng.below(80);
                    for i in 0..n {
                        let mut key = vec![0xff, 0xff, 0xff, 0xff];
                        key.extend(format!("{:08}", hi).bytes());
                        hi += 1;
                        let v = value(&mut rng, step, cfg.big);
                        staged.push(Op::Append(key.clone(), v.clone()));
                        both!(|d: &mut Db| d.append(&key, &v));
                        pool.push(key);
                        if rng.chance(15) || i + 1 == n {
                            z.commit().map_err(|e| format!("commit z: {e}"))?;
                            r.commit().map_err(|e| format!("commit r: {e}"))?;
                            for op in staged.drain(..) {
                                apply(&mut model, &op);
                            }
                        }
                    }
                    if rng.chance(70) {
                        z.flush().map_err(|e| format!("flush z: {e}"))?;
                        r.flush().map_err(|e| format!("flush r: {e}"))?;
                    }
                    absent = absent_around(&pool, &mut rng);
                }
                105..=119 => {
                    if handles.len() < 4 {
                        handles.push(Handle {
                            r: z.reader().map_err(|e| format!("reader: {e}"))?,
                            snap: None,
                        });
                    }
                }
                120..=139 => {
                    // A snapshot of the committed store, held while the store
                    // changes under it.
                    z.commit().map_err(|e| format!("commit z: {e}"))?;
                    r.commit().map_err(|e| format!("commit r: {e}"))?;
                    for op in staged.drain(..) {
                        apply(&mut model, &op);
                    }
                    if let Some(h) = handles.iter_mut().find(|h| h.snap.is_none()) {
                        h.r.snapshot();
                        h.snap = Some(model.clone());
                    }
                }
                140..=159 => {
                    if let Some(h) = handles.iter_mut().find(|h| h.snap.is_some()) {
                        h.r.release();
                        h.snap = None;
                    }
                }
                160..=169 => {
                    // A crash or a clean close, then the reopen.
                    handles.clear();
                    let clean = rng.chance(50) || !matches!(cfg.sync, SyncPolicy::Always);
                    if clean {
                        for op in staged.drain(..) {
                            // close commits the staged batch.
                            apply(&mut model, &op);
                        }
                        z.close().map_err(|e| format!("close z: {e}"))?;
                        r.close().map_err(|e| format!("close r: {e}"))?;
                    } else {
                        staged.clear();
                        drop(z);
                        drop(r);
                    }
                    z = Db::open(&dz, opts_z(&cfg)).map_err(|e| format!("open z: {e}"))?;
                    r = Db::open(&dr, opts(&cfg)).map_err(|e| format!("open r: {e}"))?;
                    st.reopens += 1;
                    let probe = all_probe(&pool, &absent);
                    let how = if clean { "a close" } else { "a crash" };
                    check_keys(&format!("z, after {how}"), &z, &model, &probe, &mut fails);
                    check_keys(&format!("r, after {how}"), &r, &model, &probe, &mut fails);
                }
                _ => {}
            }
        }
        if !fails.is_empty() {
            break;
        }
        if scan_every && staged.is_empty() {
            let want = live(&model);
            for (name, d) in [("z", &z), ("r", &r)] {
                let (a, a_faults) = scan_all(d);
                if !a_faults.is_empty() || a != want {
                    let sources = first_diff_key(&a, &want)
                        .map(|k| format!("; {name}'s sources of the first: {}", d.debug_key(&k)))
                        .unwrap_or_default();
                    // In full, since a failure's line is cut short.
                    eprintln!("SOURCES {name} [step {step}]{sources}");
                    fail(
                        &mut fails,
                        format!(
                            "{name} scan differs from the model after step {step} (scanned every commit): {} {}{sources}",
                            model_diff(&a, &want),
                            a_faults.join("; ")
                        ),
                    );
                }
            }
        }
        if !fails.is_empty() {
            break;
        }
        // Full checks now and then, on a quiet store.
        if step % 200 == 199 {
            z.commit().map_err(|e| format!("commit z: {e}"))?;
            r.commit().map_err(|e| format!("commit r: {e}"))?;
            for op in staged.drain(..) {
                apply(&mut model, &op);
            }
            if rng.chance(75) {
                z.settle().map_err(|e| format!("settle z: {e}"))?;
            }
            let probe = all_probe(&pool, &absent);
            check_keys("z writer", &z, &model, &probe, &mut fails);
            check_keys("r writer", &r, &model, &probe, &mut fails);
            let hz = z.reader().map_err(|e| format!("reader: {e}"))?;
            check_keys("z fresh handle", &hz, &model, &probe, &mut fails);
            for (i, h) in handles.iter().enumerate() {
                match &h.snap {
                    Some(snap) => check_keys(
                        &format!("handle {i} snapshot"),
                        &h.r,
                        snap,
                        &probe,
                        &mut fails,
                    ),
                    None => check_keys(&format!("handle {i}"), &h.r, &model, &probe, &mut fails),
                }
            }
            {
                let want = live(&model);
                let ((a, a_faults), (b, b_faults)) = (scan_all(&z), scan_all(&r));
                let tag = if cfg.cache { " (block cache on)" } else { "" };
                for x in &a_faults {
                    fail(&mut fails, format!("z {x}{tag}"));
                }
                for x in &b_faults {
                    fail(&mut fails, format!("r {x}{tag}"));
                }
                if a != want {
                    let sources = first_diff_key(&a, &want)
                        .map(|k| {
                            format!("; the writer's sources of the first: {}", z.debug_key(&k))
                        })
                        .unwrap_or_default();
                    fail(
                        &mut fails,
                        format!(
                            "z scan differs from the model{tag}: {}{sources}",
                            model_diff(&a, &want)
                        ),
                    );
                }
                if b != want {
                    let sources = first_diff_key(&b, &want)
                        .map(|k| format!("; the twin's sources of the first: {}", r.debug_key(&k)))
                        .unwrap_or_default();
                    eprintln!("SOURCES r [step {step}]{sources}");
                    fail(
                        &mut fails,
                        format!(
                            "r scan differs from the model{tag}: {}{sources}",
                            model_diff(&b, &want)
                        ),
                    );
                }
            }
        }
        if !fails.is_empty() {
            break;
        }
    }
    // Final: settle everything, and check through every path. A seed that
    // failed a check keeps the step it failed at.
    if fails.is_empty() {
        STEP.set(cfg.ops as i64);
        z.commit().map_err(|e| format!("commit z: {e}"))?;
        r.commit().map_err(|e| format!("commit r: {e}"))?;
        for op in staged.drain(..) {
            apply(&mut model, &op);
        }
        z.settle().map_err(|e| format!("settle z: {e}"))?;
        r.settle().map_err(|e| format!("settle r: {e}"))?;
        let probe = all_probe(&pool, &absent);
        check_keys("final z writer", &z, &model, &probe, &mut fails);
        check_keys("final r writer", &r, &model, &probe, &mut fails);
        let hz = z.reader().map_err(|e| format!("reader: {e}"))?;
        check_keys("final z fresh handle", &hz, &model, &probe, &mut fails);
        for (i, h) in handles.iter().enumerate() {
            match &h.snap {
                Some(snap) => check_keys(
                    &format!("final handle {i} snapshot"),
                    &h.r,
                    snap,
                    &probe,
                    &mut fails,
                ),
                None => check_keys(
                    &format!("final handle {i}"),
                    &h.r,
                    &model,
                    &probe,
                    &mut fails,
                ),
            }
        }
        let sn = z.reader().map_err(|e| format!("reader: {e}"))?;
        sn.snapshot();
        check_keys("final z snapshot", &sn, &model, &probe, &mut fails);
    }
    let ok = fails.is_empty();
    drop(handles);
    drop(z);
    drop(r);
    if ok {
        let _ = std::fs::remove_dir_all(&dz);
        let _ = std::fs::remove_dir_all(&dr);
        Ok(st)
    } else {
        Err(format!(
            "seed {seed} ({cfg:?}): {} failures, first: {:?}",
            fails.len(),
            &fails[..fails.len().min(4)]
        ))
    }
}

#[test]
fn fuzz_range() {
    let from = env_u64("ZFUZZ_FROM").unwrap_or(0);
    let to = env_u64("ZFUZZ_TO").unwrap_or(from + DEFAULT_SEEDS);
    let want_cache =
        std::env::var("ZFUZZ_CACHE").is_ok_and(|v| matches!(v.as_str(), "1" | "all" | "off"));
    let verbose = env_flag("ZFUZZ_VERBOSE");
    let list: Option<Vec<u64>> = std::env::var("ZFUZZ_SEEDS")
        .ok()
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect());
    let seeds: Vec<u64> = list.unwrap_or_else(|| (from..to).collect());
    let (mut bad, mut reopens) = (Vec::new(), 0u64);
    for seed in seeds {
        match run_seed(seed, want_cache, verbose) {
            Ok(s) => reopens += s.reopens,
            Err(e) => bad.push(format!("seed {seed}: {e}")),
        }
    }
    eprintln!(
        "seeds {from}..{to}: reopens {reopens}, FAILURES {}",
        bad.len()
    );
    for b in &bad {
        eprintln!("FAIL: {b}");
    }
    assert!(bad.is_empty(), "{} seeds failed", bad.len());
}

// ---------------------------------------------------------------------------
// Concurrency: readers on threads against a writer that appends, commits,
// seals, flushes and settles while merges and promotions land.

fn pack(epoch: u32, n: u32) -> u64 {
    ((epoch as u64) << 32) | n as u64
}

fn cvalue(epoch: u32, i: u32, pad: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&epoch.to_le_bytes());
    v.extend_from_slice(&i.to_le_bytes());
    v.extend(std::iter::repeat_n(b'.', pad));
    v
}

fn parse_c(v: &[u8]) -> (u32, u32) {
    (
        u32::from_le_bytes(v[0..4].try_into().unwrap()),
        u32::from_le_bytes(v[4..8].try_into().unwrap()),
    )
}

fn run_concurrent(seed: u64, secs_ops: usize, snapshots: bool) -> Result<(), String> {
    let mut rng = Rng::new(seed ^ 0xC0FFEE);
    let cfg = pick_cfg(&mut rng, false);
    let dir = scratch(&format!("c{seed}"));
    let mut db = Db::create(&dir, opts(&cfg)).map_err(|e| format!("create: {e}"))?;
    let nkeys = 150 + rng.below(250);
    let keys: Arc<Vec<Vec<u8>>> = Arc::new(
        (0..nkeys)
            .map(|i| {
                if i % 7 == 0 {
                    format!("c{:03}-long-key-{}", i, "x".repeat(i % 20)).into_bytes()
                } else {
                    format!("c{:05}", i * 3).into_bytes()
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    );
    // Per key: the (epoch, n) the writer has committed, and has appended.
    let committed: Arc<Vec<AtomicU64>> =
        Arc::new((0..keys.len()).map(|_| AtomicU64::new(0)).collect());
    let appended: Arc<Vec<AtomicU64>> =
        Arc::new((0..keys.len()).map(|_| AtomicU64::new(0)).collect());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let errs: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reads_done = Arc::new(AtomicU64::new(0));
    let mut threads = Vec::new();
    for t in 0..3u64 {
        let h = db.reader().map_err(|e| format!("reader: {e}"))?;
        let (keys, committed, appended, stop, errs, reads_done) = (
            keys.clone(),
            committed.clone(),
            appended.clone(),
            stop.clone(),
            errs.clone(),
            reads_done.clone(),
        );
        threads.push(std::thread::spawn(move || {
            let mut rng = Rng::new(seed * 31 + t);
            let mut last = vec![0u64; keys.len()];
            let mut snap_state: Option<Vec<u64>> = None;
            while !stop.load(Ordering::Relaxed) {
                let i = rng.below(keys.len());
                let lo = committed[i].load(Ordering::SeqCst);
                let mut vals = Vec::new();
                let res = h.read_all(&keys[i], |v| vals.push(v.to_vec()));
                let cnt = h.count(&keys[i]);
                let hi = appended[i].load(Ordering::SeqCst);
                // The count and the read are two operations and the key may
                // move between them, so the count is not held to the read.
                let n_read = match (res, cnt) {
                    (Ok(n), Ok(_)) => n,
                    (a, b) => {
                        errs.lock().unwrap().push(format!("thread {t}: read errors {a:?} {b:?}"));
                        return;
                    }
                };
                if n_read as usize != vals.len() {
                    errs.lock().unwrap().push(format!("thread {t}: n {n_read} != {}", vals.len()));
                    return;
                }
                // The values: one epoch, indexes 0..n, in order.
                let mut epoch = None;
                for (j, v) in vals.iter().enumerate() {
                    if v.len() < 8 {
                        errs.lock().unwrap().push(format!("thread {t}: short value"));
                        return;
                    }
                    let (e, idx) = parse_c(v);
                    if epoch.is_some_and(|x| x != e) || idx as usize != j {
                        errs.lock().unwrap().push(format!(
                            "thread {t}: key {}: values out of shape at {j}: epoch {e} idx {idx} (first epoch {epoch:?})",
                            show(&keys[i])
                        ));
                        return;
                    }
                    epoch = Some(e);
                }
                let got = if vals.is_empty() { None } else { Some(pack(epoch.unwrap(), vals.len() as u32)) };
                // Monotone and bounded: what was committed before the read
                // started is a floor, what was appended by its end a ceiling.
                match got {
                    Some(g) => {
                        if g < lo && snap_state.is_none() {
                            errs.lock().unwrap().push(format!(
                                "thread {t}: key {}: read {:?} below the committed floor {:?}",
                                show(&keys[i]),
                                (g >> 32, g & 0xffff_ffff),
                                (lo >> 32, lo & 0xffff_ffff)
                            ));
                            return;
                        }
                        if g > hi {
                            errs.lock().unwrap().push(format!(
                                "thread {t}: key {}: read {:?} above the appended ceiling {:?}",
                                show(&keys[i]),
                                (g >> 32, g & 0xffff_ffff),
                                (hi >> 32, hi & 0xffff_ffff)
                            ));
                            return;
                        }
                        if g < last[i] && snap_state.is_none() {
                            errs.lock().unwrap().push(format!(
                                "thread {t}: key {}: went backwards {:?} -> {:?}",
                                show(&keys[i]),
                                (last[i] >> 32, last[i] & 0xffff_ffff),
                                (g >> 32, g & 0xffff_ffff)
                            ));
                            return;
                        }
                        if snap_state.is_none() {
                            last[i] = g;
                        }
                    }
                    None => {
                        // Nothing read: nothing was committed, or the key
                        // was reset since. A floor with n > 0 at the same
                        // epoch is a lost key.
                        if (lo & 0xffff_ffff) > 0 && snap_state.is_none() && (hi >> 32) == (lo >> 32) {
                            // The key may have been deleted at a later epoch
                            // (committed floor is (epoch, n) of the latest
                            // commit, which after a delete is (epoch, 0)).
                            errs.lock().unwrap().push(format!(
                                "thread {t}: key {}: read nothing under a committed floor {:?}",
                                show(&keys[i]),
                                (lo >> 32, lo & 0xffff_ffff)
                            ));
                            return;
                        }
                    }
                }
                reads_done.fetch_add(1, Ordering::Relaxed);
                if snapshots && rng.chance(1) {
                    if snap_state.is_none() {
                        h.snapshot();
                        snap_state = Some(Vec::new());
                    } else {
                        h.release();
                        snap_state = None;
                        last = vec![0u64; keys.len()];
                    }
                }
            }
            if snap_state.is_some() {
                h.release();
            }
        }));
    }
    // The writer.
    let mut epoch = vec![0u32; keys.len()];
    let mut n = vec![0u32; keys.len()];
    let mut staged: Vec<usize> = Vec::new();
    for step in 0..secs_ops {
        let i = rng.below(keys.len());
        if rng.chance(4) {
            db.delete(&keys[i]);
            epoch[i] += 1;
            n[i] = 0;
        } else {
            let pad = rng.below(60);
            db.append(&keys[i], &cvalue(epoch[i], n[i], pad));
            n[i] += 1;
        }
        appended[i].store(pack(epoch[i], n[i]), Ordering::SeqCst);
        staged.push(i);
        if rng.chance(8) {
            db.commit().map_err(|e| format!("commit: {e}"))?;
            for &j in &staged {
                committed[j].store(pack(epoch[j], n[j]), Ordering::SeqCst);
            }
            staged.clear();
        }
        match rng.below(400) {
            0..=3 => {
                db.commit().map_err(|e| format!("commit: {e}"))?;
                for &j in &staged {
                    committed[j].store(pack(epoch[j], n[j]), Ordering::SeqCst);
                }
                staged.clear();
                db.seal().map_err(|e| format!("seal: {e}"))?;
            }
            4..=5 => {
                db.commit().map_err(|e| format!("commit: {e}"))?;
                for &j in &staged {
                    committed[j].store(pack(epoch[j], n[j]), Ordering::SeqCst);
                }
                staged.clear();
                db.flush().map_err(|e| format!("flush: {e}"))?;
            }
            6..=14 => db.settle().map_err(|e| format!("settle: {e}"))?,
            15..=18 => db.settle().map_err(|e| format!("settle: {e}"))?,
            _ => {}
        }
        if step % 64 == 0 && !errs.lock().unwrap().is_empty() {
            break;
        }
    }
    db.commit().map_err(|e| format!("commit: {e}"))?;
    for &j in &staged {
        committed[j].store(pack(epoch[j], n[j]), Ordering::SeqCst);
    }
    db.settle().map_err(|e| format!("settle: {e}"))?;
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().map_err(|_| "reader thread panicked".to_string())?;
    }
    let e = errs.lock().unwrap().clone();
    if !e.is_empty() {
        return Err(format!("seed {seed} ({cfg:?}): {:?}", &e[..e.len().min(4)]));
    }
    eprintln!(
        "concurrent seed {seed}: reads {}",
        reads_done.load(Ordering::Relaxed)
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn concurrent_range() {
    let from = env_u64("ZFUZZ_FROM").unwrap_or(0);
    let to = env_u64("ZFUZZ_TO").unwrap_or(from + 3);
    let ops = env_u64("ZFUZZ_OPS").unwrap_or(4000) as usize;
    let snaps = env_flag("ZFUZZ_SNAPS");
    let mut bad = Vec::new();
    for seed in from..to {
        match std::panic::catch_unwind(|| run_concurrent(seed, ops, snaps)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => bad.push(e),
            Err(p) => bad.push(format!("seed {seed}: PANIC {}", panic_text(p.as_ref()))),
        }
    }
    for b in &bad {
        eprintln!("FAIL: {b}");
    }
    assert!(bad.is_empty(), "{} seeds failed", bad.len());
}
