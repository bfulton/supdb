//! One run: the floors, then the ladder, times the arms, times the reps,
//! into a row.
//!
//! Every arm in one process, interleaved one round at a time, so a machine
//! that drifts drifts across all of them rather than into one. A rep is one
//! complete pass of every workload for one arm; rep zero is a warmup and is
//! discarded, because the first touch of a fresh file pays for allocation
//! and first-fault costs that no steady state repeats.

use crate::bound::Bound;
use crate::engines::{self, Batch, Engine, ZState};
use crate::env::{self, IoCounters};
use crate::gate;
use crate::hist::Hist;
use crate::row::{Guarantee, MachineInfo, Measurement, Row};
use crate::workload::{db_key_into, KeyDist, KeyGen, Payload, Permutation, Rng};
use crate::{ladder_from, Scale};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::FileExt as _;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Every record the suite writes is this key and this value, so a figure
/// can turn a byte rate into an entry rate and a key count into bytes.
pub const KEY_SIZE: usize = 16;
pub const VALUE_SIZE: usize = 100;

/// The arm name the floors are recorded under: no engine ran.
pub const FLOOR_ARM: &str = "floor";

/// The reader-thread counts `read` and `scan` run at beside their
/// single-threaded pass, each a quantity of its own (`reads_per_s_2t`)
/// on the same workload, so a row shows where an arm's throughput stops
/// scaling: two, doubling, and one fewer than the machine's cores as the
/// top. The top was the core count, and a pass with as many readers as
/// cores shares them with the harness's thread, the engines' own threads
/// and the host: its cells read two to four times the spread of the
/// counts below, for every engine, and our ratio to LMDB on them swung
/// from 0.65x to 2.1x across rows where the counts below sat within a
/// fifth. The counts derive from the class's core count, which the class
/// is keyed on, so a class runs one set and names one set of quantities,
/// and a machine of two cores runs none. `gate::higher_is_better` names
/// the threaded quantities by their suffix, and its test derives the
/// counts from here.
pub fn thread_counts(cpus: usize) -> Vec<usize> {
    let top = cpus.saturating_sub(1);
    let mut counts = Vec::new();
    let mut t = 2;
    while t < top {
        counts.push(t);
        t *= 2;
    }
    if top >= 2 {
        counts.push(top);
    }
    counts
}

/// The quantity `base` measured over `threads` reader threads:
/// `reads_per_s_4t`.
pub fn threaded_quantity(base: &str, threads: usize) -> String {
    format!("{base}_{threads}t")
}

/// The thread count a quantity's name carries, or none for a
/// single-threaded quantity.
pub fn threads_of(quantity: &str) -> Option<usize> {
    let (base, count) = quantity.rsplit_once('_')?;
    if !matches!(base, "reads_per_s" | "entries_per_s") {
        return None;
    }
    count.strip_suffix('t')?.parse().ok()
}

/// The five counts a bounded row records around every timed phase, per
/// operation of the phase: the operating threads' own device bytes and
/// major faults, the rest of the process's device bytes (the merges'
/// streaming, a comparator's background jobs), the group's refaults, and
/// the device's read latency over the phase. Each is one count and never
/// a sum of two, so a reader can tell the engine's reads from the
/// kernel's readahead and both from the comparator's compaction.
pub const COUNTS: [(&str, &str); 5] = [
    ("read_bytes_per_op", "B/op"),
    ("majfaults_per_op", "faults/op"),
    ("bg_read_bytes_per_op", "B/op"),
    ("refaults_per_op", "refaults/op"),
    ("read_us_per_io", "us/io"),
];

/// The count a quantity's name is, under a thread suffix (`_3t`) or a lag
/// suffix (`_lag10pct`) or none, or `None` for a quantity that is not one
/// of the five. The suffixes are the ones the rates on the same workloads
/// carry, so a count sits beside the rate it was taken around.
pub fn count_of(quantity: &str) -> Option<&'static str> {
    let base = COUNTS.iter().map(|(b, _)| *b).find(|b| {
        quantity == *b
            || quantity
                .strip_prefix(*b)
                .and_then(|rest| rest.strip_prefix('_'))
                .is_some_and(|s| {
                    s.strip_suffix('t')
                        .is_some_and(|n| n.parse::<usize>().is_ok())
                        || s.strip_prefix("lag")
                            .and_then(|s| s.strip_suffix("pct"))
                            .is_some_and(|n| n.parse::<u64>().is_ok())
                })
    })?;
    Some(base)
}

/// A count's unit, by its base name.
fn count_unit(base: &str) -> &'static str {
    COUNTS
        .iter()
        .find(|(b, _)| *b == base)
        .map_or("?", |(_, u)| u)
}

/// Which of a pass's phases run. Every phase in a row; `bench ab
/// --passes` names a subset, for a question one phase answers at a rung
/// where the whole pass is an hour. The loads are one name, since the
/// second store exists for the shuffled load and the lag sweep, and is
/// built only when one of them runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passes {
    pub load: bool,
    pub read: bool,
    pub read_threaded: bool,
    pub scan: bool,
    pub scan_threaded: bool,
    pub mixes: bool,
    pub scan_mixed: bool,
    pub lag: bool,
}

impl Passes {
    pub const NAMES: [&'static str; 8] = [
        "load",
        "read",
        "read-threaded",
        "scan",
        "scan-threaded",
        "mixes",
        "scan-mixed",
        "lag",
    ];

    pub fn all() -> Passes {
        Passes {
            load: true,
            read: true,
            read_threaded: true,
            scan: true,
            scan_threaded: true,
            mixes: true,
            scan_mixed: true,
            lag: true,
        }
    }

    /// A comma-separated list of `NAMES`; a name not among them is an
    /// error naming it, and an empty list is one too.
    pub fn parse(list: &str) -> Result<Passes, String> {
        let mut p = Passes {
            load: false,
            read: false,
            read_threaded: false,
            scan: false,
            scan_threaded: false,
            mixes: false,
            scan_mixed: false,
            lag: false,
        };
        let mut any = false;
        for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            any = true;
            match name {
                "load" => p.load = true,
                "read" => p.read = true,
                "read-threaded" => p.read_threaded = true,
                "scan" => p.scan = true,
                "scan-threaded" => p.scan_threaded = true,
                "mixes" => p.mixes = true,
                "scan-mixed" => p.scan_mixed = true,
                "lag" => p.lag = true,
                other => {
                    return Err(format!(
                        "no pass named {other:?}; the passes are {}",
                        Passes::NAMES.join(",")
                    ))
                }
            }
        }
        if !any {
            return Err(format!(
                "--passes names no pass; the passes are {}",
                Passes::NAMES.join(",")
            ));
        }
        Ok(p)
    }
}

/// The smallest bound a run accepts: the floors alone hold a 64 MiB
/// chase buffer, a prime of at least 16 MiB and the stores' own tables,
/// and a bound below this is one the floors cannot run under, refused
/// before anything is measured. It is also the known-red input the
/// wrapper proves its gate on (`scripts/ooc.sh --prove-red`).
pub const MIN_BOUND_BYTES: u64 = 256 << 20;

/// The default divisor of a bounded run's read operations
/// (`Plan::ops_div`).
pub const OOC_OPS_DIV: u64 = 30;

pub struct Plan {
    pub scale: Scale,
    pub arms: Vec<String>,
    /// The ladder's top rung, in keys. The ladder rounds up to a rung.
    pub top: u64,
    /// The ladder's bottom rung, in keys, rounded up to a rung the same way
    /// `top` is. Zero is the whole ladder. Above zero the run measures only
    /// the rungs from here up, for when the cheap ones cost hours and answer
    /// a question that was already answered.
    pub bottom: u64,
    pub reps: usize,
    pub value_size: usize,
    pub batch: usize,
    pub scan_len: usize,
    /// The reader-thread counts of this machine's class (`thread_counts`).
    pub threads: Vec<usize>,
    /// What the read passes' operation counts are divided by: one at
    /// quick and full, where every count is the rung's. Out of core a
    /// point read is a device read of a few milliseconds, so `size` of
    /// them at ten million keys is hours a pass, and a divided count
    /// (`divided`) measures the same regime in minutes; the loads and the
    /// lag bursts are never divided, since a store is built whole or not
    /// at all.
    pub ops_div: u64,
    /// The fraction of each value that is a repeated phrase and so compresses
    /// (`Payload`'s compressibility): the suite's half by default. A control at
    /// 0.0 stores the same number of bytes that nothing can shrink, which is
    /// what a compact form of the records is priced against.
    pub compress: f64,
    /// The memory bound the run measures under, entered before the plan
    /// was made (`bound::Bound`), or none.
    pub bound: Option<Bound>,
    /// Which phases a pass runs: every one for a row, since `run` records
    /// each phase's quantity whether it ran or not, and a subset only
    /// through `ab`, which pairs what ran.
    pub passes: Passes,
}

impl Plan {
    /// `cpus` is the machine's core count as the class records it.
    pub fn new(scale: Scale, arms: Vec<String>, top: u64, cpus: usize) -> Plan {
        Plan {
            scale,
            arms,
            top,
            bottom: 0,
            reps: scale.reps(),
            value_size: VALUE_SIZE,
            batch: 1_000,
            scan_len: 100,
            threads: thread_counts(cpus),
            ops_div: match scale {
                Scale::Ooc => OOC_OPS_DIV,
                Scale::Quick | Scale::Full => 1,
            },
            compress: 0.5,
            bound: None,
            passes: Passes::all(),
        }
    }

    /// `n` operations under `ops_div`: `n` itself at a divisor of one,
    /// and otherwise the quotient rounded up, floored at a thousand so
    /// no divided pass is shorter than a thread hand-off is noisy, and
    /// never more than `n`.
    pub fn divided(&self, n: u64) -> u64 {
        if self.ops_div <= 1 {
            n
        } else {
            n.div_ceil(self.ops_div).max(1_000).min(n)
        }
    }

    /// The bound's limit, as read back, or none.
    pub fn bound_bytes(&self) -> Option<u64> {
        self.bound.as_ref().map(Bound::limit)
    }
}

/// YCSB's core workloads: (letter, read %, update %, insert %, scan %,
/// read-modify-write %, key distribution). Theta 0.99 for the Zipfian, as
/// in the original. D's reads are uniform over the loaded keys rather than
/// skewed to the latest inserts: the latest distribution needs a Zipfian
/// over a count that grows with every insert, and the cost of tracking
/// that is not a cost the engines should be charged for.
const YCSB: [(char, u32, u32, u32, u32, u32, KeyDist); 6] = [
    ('A', 50, 50, 0, 0, 0, KeyDist::Zipfian),
    ('B', 95, 5, 0, 0, 0, KeyDist::Zipfian),
    ('C', 100, 0, 0, 0, 0, KeyDist::Zipfian),
    ('D', 95, 0, 5, 0, 0, KeyDist::Uniform),
    ('E', 0, 0, 5, 95, 0, KeyDist::Zipfian),
    ('F', 50, 0, 0, 0, 50, KeyDist::Zipfian),
];
/// The order the six run in on one store: the read-only mix first, on the
/// store as loaded, and the two that insert last, so no earlier workload
/// reads a store another has grown.
const YCSB_ORDER: [char; 6] = ['C', 'B', 'A', 'F', 'D', 'E'];
const YCSB_BATCH: usize = 100;
const YCSB_SCAN: usize = 50;

/// A workload's operation count at a rung: a sixth of the keys, so the six
/// mixes together cost about one read pass. At a third, `quick` took twice
/// the two minutes it is meant to take.
fn ycsb_ops(size: u64) -> u64 {
    (size / 6).max(1_000)
}

/// The store updated by this much of itself, as a percentage, before the
/// scans at each point of the lag sweep. Zero is the drained store, and
/// a hundred is every key written once with nothing merged since, which
/// at the ladder's top rung is past the seal and so is one or more
/// unmerged pieces -- which is what lag is. The points are fractions
/// rather than counts so a quantity means the same thing at every rung.
const LAG_PCT: [u64; 4] = [0, 1, 10, 100];

/// With `SUPDB_LAG_MARK` set, the monotonic clock at a lag pass's edges on
/// stderr, so a profile of the whole process can be cut to the pass:
/// `perf record -k CLOCK_MONOTONIC`, then the times `perf script` prints
/// against these. The pass itself is timed by `Instant` as before; this
/// adds two clock reads outside it.
/// Minor page faults so far of every thread of this process named `comm`,
/// from `/proc/self/task/*/stat`: the eighth field after the comm's closing
/// parenthesis. Zero for a name no thread has.
pub fn minflt_of(comm: &str) -> u64 {
    let Ok(rd) = std::fs::read_dir("/proc/self/task") else {
        return 0;
    };
    let mut total = 0u64;
    for e in rd.flatten() {
        let p = e.path();
        if std::fs::read_to_string(p.join("comm"))
            .unwrap_or_default()
            .trim()
            != comm
        {
            continue;
        }
        if let Ok(stat) = std::fs::read_to_string(p.join("stat")) {
            total += stat_minflt(&stat);
        }
    }
    total
}

/// Every thread of this process by tid: its name, and from
/// `/proc/self/task/*/schedstat` its nanoseconds on a CPU, its nanoseconds
/// waiting on a run queue and its timeslices so far. Two readings around
/// a pass give what every thread the engine runs beside the pass's own
/// cost it: a thread that ran took a core the readers had, and one that
/// waited was contending for one.
pub fn sched_of_tasks() -> Vec<(u32, String, u64, u64, u64)> {
    let Ok(rd) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let Some(tid) = p.file_name().and_then(|n| n.to_str()?.parse().ok()) else {
            continue;
        };
        let comm = std::fs::read_to_string(p.join("comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let Ok(st) = std::fs::read_to_string(p.join("schedstat")) else {
            continue;
        };
        let mut f = st.split_whitespace().map(|x| x.parse::<u64>().unwrap_or(0));
        let (run, wait, slices) = (
            f.next().unwrap_or(0),
            f.next().unwrap_or(0),
            f.next().unwrap_or(0),
        );
        v.push((tid, comm, run, wait, slices));
    }
    v
}

/// Microseconds of CPU the calling thread has used.
pub fn thread_cpu_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a clock read into a timespec on the stack.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1000
}

/// This thread's minor page faults so far.
pub fn minflt_self() -> u64 {
    std::fs::read_to_string("/proc/thread-self/stat").map_or(0, |s| stat_minflt(&s))
}

fn stat_minflt(stat: &str) -> u64 {
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(7).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

/// `MemFree` from `/proc/meminfo`, in megabytes: what the guest has not
/// handed back to its host through the balloon is what a fresh page costs
/// a guest fault rather than a host one.
pub fn mem_free_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| {
            m.lines().find(|l| l.starts_with("MemFree:")).and_then(|l| {
                l.split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
            })
        })
        .map_or(0, |kb| kb / 1024)
}

/// A phase's edge stamped on stderr in `CLOCK_MONOTONIC` seconds under
/// `SUPDB_LAG_MARK`, as `lag_mark` stamps a lag point's: what a profile
/// of the load is cut to, by thread.
pub fn phase_mark(arm: &str, phase: &str, edge: &str) {
    if std::env::var_os("SUPDB_LAG_MARK").is_none() {
        return;
    }
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a clock read into a timespec on the stack.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    eprintln!(
        "phasemark {arm} {phase} {edge} {}.{:09}",
        ts.tv_sec, ts.tv_nsec
    );
}

/// One line of the store's counters around a timed stretch, as deltas
/// of `e.counters()` read before and after it: the scans' phases before
/// their walk, the threads' time, the blocks built and the forms copied,
/// the snapshots and the walks by form, the store's shape after, and the
/// compact form's reads, decodes, scan walks and groups, memo hits and
/// misses and prefetch asks over the window, so that a mix's cost under
/// the form is read beside its time by kind. The lag sweep prints one a
/// point (`SUPDB_LAG_COUNTERS`) and the mixes one a mix
/// (`SUPDB_MIX_COUNTERS`).
fn counters_line(
    tag: &str,
    arm: &str,
    point: &str,
    (flt_up, flt_main): (u64, u64),
    (burst_ms, pass_ms): (f64, f64),
    c0: &[(&str, f64)],
    c1: &[(&str, f64)],
) {
    let get = |c: &[(&str, f64)], n: &str| c.iter().find(|(k, _)| *k == n).map_or(0.0, |(_, v)| *v);
    let d = |n: &str| get(c1, n) - get(c0, n);
    eprintln!(
        "{tag} {arm} {point} flt up {flt_up} main {flt_main} memfree_mb {} burst_ms {burst_ms:.1} pass_ms {pass_ms:.1} | ph take {:.2} sync {:.2} settle {:.2} snap {:.2} ahead {:.2} install {:.2} | thread {:.1}ms/{:.0}p maint {:.1}ms | built up {:.0} eng {:.0} rd {:.0} cloned {:.0} | snaps {:.0} refr {:.0} | walks {:.0}/{:.0}/{:.0} starts {:.0}/{:.0} | pieces {:.0} unsealed {:.0} handles-scans {:.0} | z reads {:.0} dec {:.0} scans {:.0} groups {:.0} memo {:.0}/{:.0} pf {:.0}",
        mem_free_mb(),
        d("scan_take_us") / 1e3,
        d("scan_sync_us") / 1e3,
        d("scan_settle_us") / 1e3,
        d("scan_snap_us") / 1e3,
        d("scan_ahead_us") / 1e3,
        d("scan_install_us") / 1e3,
        d("upkeep_cpu_us") / 1e3,
        d("upkeep_passes"),
        d("maint_cpu_us") / 1e3,
        d("blk_by_upkeep"),
        d("blk_by_engine"),
        d("blk_by_reader"),
        d("forms_cloned"),
        d("snapshot_builds"),
        d("snapshot_refreshes"),
        d("walk_copy"),
        d("walk_sparse"),
        d("walk_other"),
        d("start_at_rank"),
        d("start_search"),
        get(c1, "pieces"),
        get(c1, "unsealed_keys"),
        d("rd_scans"),
        d("zpage_reads"),
        d("zpage_decodes"),
        d("zpage_scans"),
        d("zpage_scan_groups"),
        d("zpage_memo_hits"),
        d("zpage_memo_misses"),
        d("zpage_prefetches")
    );
    // The process's memory beside the structures that count their own: the
    // resident bytes, what the allocator has handed out and what it holds
    // free, and the writer's tables and the published forms by their
    // counts. The gap between the resident bytes and the sum of the
    // structures is slack, retention and everything uncounted, which the
    // lag sweep's peak is made of under a memory bound.
    if tag == "lagcounters" {
        // SAFETY: mallinfo2 reads the allocator's own totals.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        let mi = unsafe { libc::mallinfo2() };
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        let (inuse, free, mapped) = (mi.uordblks as u64, mi.fordblks as u64, mi.hblkhd as u64);
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        let (inuse, free, mapped) = (0u64, 0u64, 0u64);
        let mb = |b: u64| b / (1 << 20);
        eprintln!(
            "lagmem {arm} {point} anon_mb {} rss_mb {} inuse_mb {} free_mb {} mmapped_mb {} cache_mb {} forms_mb {}",
            anon_resident_bytes().map_or(0, mb),
            resident_bytes().map_or(0, mb),
            mb(inuse),
            mb(free),
            mb(mapped),
            mb(get(c1, "cache_bytes_at_end") as u64),
            mb(get(c1, "form_bytes_at_end") as u64),
        );
    }
    // The forms' events over the window, by name, for a decomposition of
    // who built what beside whom.
    if std::env::var_os("SUPDB_MIX_EVENTS").is_some() {
        let names = [
            "table_drops",
            "ahead_starts",
            "ahead_stops",
            "ahead_installed",
            "settle_notable",
            "fill_dense",
            "fill_fresh",
            "ahead_skip_held",
            "ahead_skip_overlay",
            "ahead_built",
            "install_gen_drop",
            "install_slot_skip",
            "fill_from_form",
            "convert_total_us",
            "upkeep_partial",
            "upkeep_skipped",
            "upkeep_passes",
            "forms_rebased",
            "forms_cloned",
            "form_takes",
            "snapshot_builds",
            "snapshot_extends",
            "canon_tried",
            "canon_hit",
        ];
        let ev: Vec<String> = names.iter().map(|n| format!("{n} {:.0}", d(n))).collect();
        eprintln!("{tag}-events {arm} {point} {}", ev.join(" | "));
    }
}

pub fn lag_mark(arm: &str, pct: u64, edge: &str) {
    if std::env::var_os("SUPDB_LAG_MARK").is_none() {
        return;
    }
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a clock read into a timespec on the stack.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    eprintln!("lagmark {arm} {pct} {edge} {}.{:09}", ts.tv_sec, ts.tv_nsec);
}

/// What a millisecond of a lag point's scans weighs against one of its
/// burst in `bench ab`'s decision quantity for the sweep: the store is
/// read-optimized, and its owner weights read time twice. It is printed
/// beside the two rates it is made of and gates nothing.
const READ_WEIGHT: f64 = 2.0;

/// The scan floor's file: the top rung's bytes, capped, so `full` on a
/// large machine does not spend its disk on a file that is not a store.
const SCAN_FLOOR_CAP: u64 = 4 << 30;
const WAL_FLOOR_BATCHES: u64 = 100;

/// The memory floor's buffer and how many dependent loads it makes. The
/// buffer is past every L2 this suite runs on and small enough to cost
/// milliseconds; the chase length is what makes the figure a rate rather
/// than one sample of a latency.
const MEM_FLOOR_BYTES: usize = 64 << 20;
const MEM_FLOOR_CHASES: usize = 1 << 20;

/// The fresh-page floor's sample: how many pages of a fresh anonymous
/// mapping one sample first-touches, in the system's page size. A
/// thousand is a millisecond where the guest backs the pages and five to
/// fifteen where its host has to, enough to time either, and a few
/// megabytes beside the pool of recently freed pages it draws from --
/// the pool the next pass's load draws from too, so the sample takes
/// little of it and keeps what it took (`FreshPages`).
const PAGE_FLOOR_PAGES: usize = 1024;

/// The pool a pass's store loads into, primed before the store opens
/// (`prime_fresh_pool`): bytes per key of the rung, between a floor and a
/// cap. The largest fresh footprint a load on the quick ladder takes is
/// about 340 bytes a key -- RocksDB's forty-two megabytes of memtable
/// and its files at three hundred thousand keys, or the supdb arms'
/// shuffled load -- so 512 covers every arm with margin; a load at the
/// full scale is gigabytes, which no prime covers, and the cap says so.
const PRIME_BYTES_PER_KEY: u64 = 512;
const PRIME_MIN_BYTES: u64 = 16 << 20;
const PRIME_MAX_BYTES: u64 = 512 << 20;

type Key = (String, Option<u64>, String, String); // workload, size, arm, quantity

struct Samples {
    map: BTreeMap<Key, (Guarantee, &'static str, Vec<f64>)>,
}

impl Samples {
    fn push(
        &mut self,
        workload: &str,
        size: Option<u64>,
        arm: &str,
        guarantee: Guarantee,
        (quantity, unit): (&str, &'static str),
        v: f64,
    ) {
        // A quantity the gate has no direction for is a quantity the gate
        // refuses to judge, and it refuses at gate time -- after the run
        // that produced it. This is the one place a quantity enters a row,
        // so it is the place to find out, and there is no second list to
        // drift from: `higher_is_better` is the list.
        assert!(
            gate::higher_is_better(quantity).is_some(),
            "quantity {quantity:?} has no direction; add it to gate::higher_is_better"
        );
        self.map
            .entry((
                workload.to_string(),
                size,
                arm.to_string(),
                quantity.to_string(),
            ))
            .or_insert_with(|| (guarantee, unit, Vec::new()))
            .2
            .push(v);
    }
}

/// The directory a run builds its stores under, made fresh: every stale
/// working directory goes first, not just this process's. The store a
/// run builds is removed when `run` returns, so a run that is KILLED
/// leaves it -- and at full scale that is tens of gigabytes. Two
/// cancelled runs on the self-hosted M2 stranded their stores and
/// nothing reclaimed them, because the cleanup here only knew its own
/// pid. Two timing runs may not share a machine anyway, so anything
/// matching the pattern is garbage from a run that is over. Apart from
/// `run` so that a bounded run can write the scan floor's file here
/// before it enters its bound (`write_scan_floor`).
pub fn prepare_root() -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir();
    if let Ok(entries) = std::fs::read_dir(&tmp) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("supdb-bench-")
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let root = tmp.join(format!("supdb-bench-{}", std::process::id()));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(root)
}

/// The scan floor's file size for a ladder topping at `top`: the top
/// rung's records, capped (`SCAN_FLOOR_CAP`).
pub fn scan_floor_bytes(top: u64, value_size: usize) -> u64 {
    let rung = crate::ladder(top).last().copied().unwrap_or(0);
    (rung * (KEY_SIZE + value_size) as u64).min(SCAN_FLOOR_CAP)
}

pub fn run(
    plan: &Plan,
    machine: MachineInfo,
    log: &mut dyn FnMut(&str),
    bank: Option<&Path>,
    root: &Path,
) -> Result<Row, String> {
    let utc = crate::row::utc_now();
    engines::check_matched(&plan.arms, root)?;

    let rungs = ladder_from(plan.bottom, plan.top);
    let payload = Payload::new(plan.value_size, plan.compress, 0xE1);
    let mut s = Samples {
        map: BTreeMap::new(),
    };
    let started = Instant::now();

    // The floors first: what the device does with no engine in the way.
    let floor_bytes = scan_floor_bytes(*rungs.last().unwrap_or(&0), plan.value_size);
    let floor = root.join("scan-floor.dat");
    // The page floor's mappings, held to the row's end (`FreshPages`).
    let mut fresh: Vec<FreshPages> = Vec::new();
    // The seek floors, under a bound: the device's random read over the
    // scan floor's file, cold, and again over the same offsets.
    let bounded = plan.bound.is_some();
    let seek_floors = |s: &mut Samples, rep: usize| -> Result<String, String> {
        if !bounded {
            return Ok(String::new());
        }
        drop_cache(&floor)?;
        let first = seek_floor(&floor)?;
        drop_cache(&floor)?;
        let again = seek_floor(&floor)?;
        if rep > 0 {
            for (name, f) in [("seek-floor", &first), ("seek-floor-again", &again)] {
                s.push(
                    name,
                    None,
                    FLOOR_ARM,
                    Guarantee::Buffered,
                    ("p50_us", "us"),
                    f.p50_us,
                );
                s.push(
                    name,
                    None,
                    FLOOR_ARM,
                    Guarantee::Buffered,
                    ("p99_us", "us"),
                    f.p99_us,
                );
            }
        }
        Ok(format!(
            "  seek p50 {:>7.0} p99 {:>7.0} us x{:.2}  again p50 {:>7.0} p99 {:>7.0} us x{:.2}",
            first.p50_us,
            first.p99_us,
            first.amplification,
            again.p50_us,
            again.p99_us,
            again.amplification
        ))
    };
    for rep in 0..=plan.reps {
        let wal = wal_floor(root, plan, &payload)?;
        let scan = scan_floor(root, floor_bytes)?;
        let seek = seek_floors(&mut s, rep)?;
        let mem = mem_floor();
        let (page, held) = page_floor()?;
        fresh.push(held);
        if rep > 0 {
            s.push(
                "wal-floor",
                None,
                FLOOR_ARM,
                Guarantee::Durable,
                ("ops_per_s", "ops/s"),
                wal,
            );
            // No guarantee applies to a read of a file; the field is
            // required, and buffered is the one that claims nothing.
            s.push(
                "scan-floor",
                None,
                FLOOR_ARM,
                Guarantee::Buffered,
                ("bytes_per_s", "B/s"),
                scan,
            );
            s.push(
                "mem-floor",
                None,
                FLOOR_ARM,
                Guarantee::Buffered,
                ("ops_per_s", "chases/s"),
                mem,
            );
            s.push(
                "page-floor",
                None,
                FLOOR_ARM,
                Guarantee::Buffered,
                ("pages_per_s", "pages/s"),
                page,
            );
        }
        log(&format!(
            "{:>7}s  floors rep {rep}{}  wal {wal:>10.0} ops/s  mmap {:>8.2} GB/s  chase {:>9.0}/s  page {:>5.2} us{seek}",
            started.elapsed().as_secs(),
            if rep == 0 { " (warmup)" } else { "" },
            scan / 1e9,
            mem,
            1e6 / page,
        ));
    }
    // The scan floor's file is dead weight from here: it is the top rung's
    // bytes capped at four gibibytes, and at `full` it sat on all four for
    // the whole rung phase -- the days-long part, on the host that ran out
    // of disk. Reclaimed here rather than by the cleanup at the end, which
    // a killed run never reaches. The WAL floor already removes its own.
    // Under a bound the file stays: the seek floors read it after every
    // rep of every rung, as the page floor is sampled, and a bounded
    // ladder's top is a gigabyte or so of it.
    if bounded {
        log(&format!(
            "{:>7}s  the scan floor's file stays for the seek floors, {:.1} GB",
            started.elapsed().as_secs(),
            floor_bytes as f64 / 1e9
        ));
    } else if let Ok(m) = std::fs::metadata(&floor) {
        let bytes = m.len();
        if std::fs::remove_file(&floor).is_ok() {
            log(&format!(
                "{:>7}s  reclaimed the scan floor's file, {:.1} GB",
                started.elapsed().as_secs(),
                bytes as f64 / 1e9
            ));
        }
    }

    for &size in &rungs {
        // Refuse a rung that cannot fit before starting it, rather than
        // dying part way through. A pass rebuilds its store and the store is
        // removed after, so the peak is one arm's records plus what its
        // compaction holds while rewriting them. Three times, measured
        // rather than guessed: 3.12x at 300k keys and 3.09x at a million,
        // sampled while the pass ran. It is the small rungs that set that
        // ratio -- at a hundred million the same measurement read about
        // 1.9x, because a seal of fixed size amortises over more keys -- so
        // this is conservative for the expensive rungs and exact for the
        // cheap ones. If it refuses a rung you believe fits, the message
        // names both numbers. The host carrying these runs reached 967 of
        // 994 GB used, and a rung that dies on ENOSPC costs every hour it
        // had already spent.
        let need = size * (KEY_SIZE + plan.value_size) as u64 * 3;
        if let Some(free) = free_bytes(root) {
            if free < need {
                return Err(format!(
                    "rung {size} needs about {:.0} GB and {:.0} GB is free where the \
                     stores are built ({}); the rungs below it are already written",
                    need as f64 / 1e9,
                    free as f64 / 1e9,
                    root.display()
                ));
            }
        }
        let map_gb = lmdb_map_gb(size, plan.value_size);
        for rep in 0..=plan.reps {
            for arm in &plan.arms {
                let g = engines::guarantee(arm).expect("checked at start");
                let dir = root.join(format!("{arm}-{size}-{rep}"));
                let one = one_pass(arm, &dir, size, map_gb, plan, &payload)?;
                let _ = std::fs::remove_dir_all(&dir);
                if rep > 0 {
                    let sz = Some(size);
                    s.push("load", sz, arm, g, ("ops_per_s", "ops/s"), one.load_ops_s);
                    s.push(
                        "load",
                        sz,
                        arm,
                        g,
                        ("device_bytes_per_byte", "B/B"),
                        one.load_bpb,
                    );
                    s.push(
                        "load",
                        sz,
                        arm,
                        g,
                        ("bytes_on_disk_per_byte", "B/B"),
                        one.disk_bpb,
                    );
                    s.push(
                        "load-shuffled",
                        sz,
                        arm,
                        g,
                        ("ops_per_s", "ops/s"),
                        one.shuffled_ops_s,
                    );
                    s.push("read", sz, arm, g, ("reads_per_s", "reads/s"), one.reads_s);
                    s.push("read", sz, arm, g, ("p99_us", "µs"), one.p99_us);
                    s.push(
                        "scan",
                        sz,
                        arm,
                        g,
                        ("entries_per_s", "entries/s"),
                        one.scan_entries_s,
                    );
                    for (pct, v) in &one.scan_lag {
                        let q = format!("entries_per_s_lag{pct}pct");
                        s.push("scan-lag", sz, arm, g, (q.as_str(), "entries/s"), *v);
                    }
                    for (pct, v) in &one.burst_lag {
                        let q = format!("updates_per_s_lag{pct}pct");
                        s.push("scan-lag", sz, arm, g, (q.as_str(), "ops/s"), *v);
                    }
                    for (threads, v) in &one.reads_s_threaded {
                        let q = threaded_quantity("reads_per_s", *threads);
                        s.push("read", sz, arm, g, (q.as_str(), "reads/s"), *v);
                    }
                    for (threads, v) in &one.scan_entries_s_threaded {
                        let q = threaded_quantity("entries_per_s", *threads);
                        s.push("scan", sz, arm, g, (q.as_str(), "entries/s"), *v);
                    }
                    for (letter, ops_s) in &one.ycsb_ops_s {
                        s.push(
                            &format!("ycsb-{letter}"),
                            sz,
                            arm,
                            g,
                            ("ops_per_s", "ops/s"),
                            *ops_s,
                        );
                    }
                    for (threads, v) in &one.scan_mixed_threaded {
                        let q = threaded_quantity("entries_per_s", *threads);
                        s.push("scan-mixed", sz, arm, g, (q.as_str(), "entries/s"), *v);
                    }
                    // The counts around each phase, a bounded row's only.
                    for (workload, suffix, c) in &one.phase_counts {
                        for (base, v) in c.each() {
                            let Some(v) = v else { continue };
                            let q = format!("{base}{suffix}");
                            s.push(workload, sz, arm, g, (q.as_str(), count_unit(base)), v);
                        }
                    }
                }
                let top = *plan.threads.last().unwrap_or(&1);
                let at_top = |v: &[(usize, f64)]| {
                    v.iter()
                        .find(|(t, _)| *t == top)
                        .map(|(_, v)| *v)
                        .unwrap_or(0.0)
                };
                // What the process holds as the next pass opens: a figure
                // that climbs from pass to pass is memory the passes before
                // left behind, and it comes out of the next store's cache.
                let rss = resident_bytes()
                    .map(|b| format!("{:.2} GB", b as f64 / 1e9))
                    .unwrap_or_else(|| "-".into());
                // A bounded pass's self-check per phase: the operating
                // threads' device bytes cover their major faults, which
                // cover the group's refaults, or the counts are odd and
                // the line says so before anyone reads the cells.
                let counts = one.counts_verdicts();
                log(&format!(
                    "{:>7}s  size {size:>9}  rep {rep}{}  {arm:<15} load {:>10.0} ops/s  read {:>10.0}/s  {top}t {:>10.0}/s  scan {:>11.0}/s  {top}t {:>11.0}/s  ycsb-A {:>9.0}/s  prime {:>5.2} us/pg  rss {rss}{counts}",
                    started.elapsed().as_secs(),
                    if rep == 0 { " (warmup)" } else { "" },
                    one.load_ops_s,
                    one.reads_s,
                    at_top(&one.reads_s_threaded),
                    one.scan_entries_s,
                    at_top(&one.scan_entries_s_threaded),
                    one.ycsb_ops_s.iter().find(|(l, _)| *l == 'A').map(|(_, v)| *v).unwrap_or(0.0),
                    one.prime_us_per_page,
                ));
            }
            // The price of a fresh page as this rep leaves it for the next
            // (`page_floor`): what the pass after it will pay for its
            // tables and its files' pages, read where the loads are and not
            // only at the row's start. Under a bound the seek floors are
            // sampled here too, since the device a rep leaves behind --
            // its queue, the host's cache of what the rep read -- is the
            // device the next pass reads from.
            let (page, held) = page_floor()?;
            fresh.push(held);
            if rep > 0 {
                s.push(
                    "page-floor",
                    None,
                    FLOOR_ARM,
                    Guarantee::Buffered,
                    ("pages_per_s", "pages/s"),
                    page,
                );
            }
            let seek = seek_floors(&mut s, rep)?;
            if !seek.is_empty() {
                log(&format!(
                    "{:>7}s  size {size:>9}  rep {rep}  floors{seek}",
                    started.elapsed().as_secs()
                ));
            }
        }
        // Bank the row now that this rung is whole. A rung past the memory
        // line is measured in days and the job carrying it can be killed by
        // a timeout, so the rungs that finished must not depend on the ones
        // that did not.
        if let Some(dir) = bank {
            match row_of(&utc, &machine, plan, &s).write(dir) {
                Ok(p) => log(&format!(
                    "{:>7}s  banked rungs through {size} to {}",
                    started.elapsed().as_secs(),
                    p.display()
                )),
                Err(e) => log(&format!("could not bank the row: {e}")),
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(row_of(&utc, &machine, plan, &s))
}

/// The row for the samples taken so far.
///
/// Taken by reference so a run can write it after every rung and again at
/// the end. The file name is derived from the run's utc and sha, so each
/// write replaces the last and the row on disk always holds every rung that
/// has finished. Two `full` runs were lost whole because this was built once,
/// after the last rung: the artifact step never ran, and the only survivor
/// was what the log happened to print.
fn row_of(utc: &str, machine: &MachineInfo, plan: &Plan, s: &Samples) -> Row {
    let measurements = s
        .map
        .iter()
        .map(
            |((workload, size, arm, quantity), (guarantee, unit, samples))| Measurement {
                workload: workload.clone(),
                arm: arm.clone(),
                guarantee: *guarantee,
                size: *size,
                quantity: quantity.clone(),
                unit: unit.to_string(),
                samples: samples.clone(),
            },
        )
        .collect();
    Row {
        utc: utc.to_string(),
        sha: option_env!("SUPDB_SHA").unwrap_or("unknown").to_string(),
        rustc: option_env!("SUPDB_RUSTC").unwrap_or("unknown").to_string(),
        scale: plan.scale,
        machine: machine.clone(),
        measurements,
    }
}

struct OnePass {
    /// The dearer of the pass's two primes, microseconds a page: the
    /// host's price for a load-sized footprint this pass, logged and
    /// paired, never banked.
    prime_us_per_page: f64,
    load_ops_s: f64,
    load_bpb: f64,
    /// Bytes the store's files hold on disk after the load, per byte
    /// stored: what a user pays in space, beside what the device was made
    /// to write to get there.
    disk_bpb: f64,
    shuffled_ops_s: f64,
    reads_s: f64,
    p99_us: f64,
    scan_entries_s: f64,
    /// Scans over a store with this much of itself written and nothing
    /// merged since: (percent updated, entries per second). The axis the
    /// engine's shape lives on -- one fully merged form reads the same
    /// at every point, and a store that defers merging trades this curve
    /// for its write cost.
    scan_lag: Vec<(u64, f64)>,
    /// Each lag point's burst: the updates it wrote over the time it took
    /// to write them, its last batch flushed.
    burst_lag: Vec<(u64, f64)>,
    /// The same reads and scans again over each count in `Plan::threads`:
    /// (threads, aggregate throughput).
    reads_s_threaded: Vec<(usize, f64)>,
    scan_entries_s_threaded: Vec<(usize, f64)>,
    ycsb_ops_s: Vec<(char, f64)>,
    /// The threaded scans again on the store the mixes leave, where the
    /// keys they updated and inserted are unsealed: (threads, aggregate
    /// throughput).
    scan_mixed_threaded: Vec<(usize, f64)>,
    /// What the engine says it did, rather than how fast; see
    /// `Engine::counters`.
    counters: Vec<(&'static str, f64)>,
    /// A bounded pass's counts around each timed phase (`PhaseCounts`):
    /// the phase's workload, the suffix its quantities carry (`_3t`,
    /// `_lag10pct`, or none), and the counts. Empty unbounded.
    phase_counts: Vec<(String, String, PhaseCounts)>,
}

impl OnePass {
    /// Every quantity of the pass under one name, for `ab` to pair. A
    /// phase the plan's passes left out is a NaN here and is skipped, so
    /// a pairing over a subset pairs the subset.
    fn flat(&self) -> Vec<(String, f64)> {
        let mut v = vec![
            ("prime us_per_page".into(), self.prime_us_per_page),
            ("load ops_per_s".into(), self.load_ops_s),
            ("load device_bytes_per_byte".into(), self.load_bpb),
            ("load bytes_on_disk_per_byte".into(), self.disk_bpb),
            ("load-shuffled ops_per_s".into(), self.shuffled_ops_s),
            ("read reads_per_s".into(), self.reads_s),
            ("read p99_us".into(), self.p99_us),
            ("scan entries_per_s".into(), self.scan_entries_s),
        ];
        v.retain(|(_, x)| !x.is_nan());
        for (pct, x) in &self.scan_lag {
            v.push((format!("scan-lag entries_per_s_lag{pct}pct"), *x));
        }
        for (pct, x) in &self.burst_lag {
            v.push((format!("scan-lag updates_per_s_lag{pct}pct"), *x));
        }
        for (t, x) in &self.reads_s_threaded {
            v.push((format!("read reads_per_s_{t}t"), *x));
        }
        for (t, x) in &self.scan_entries_s_threaded {
            v.push((format!("scan entries_per_s_{t}t"), *x));
        }
        for (t, x) in &self.scan_mixed_threaded {
            v.push((format!("scan-mixed entries_per_s_{t}t"), *x));
        }
        for (c, x) in &self.ycsb_ops_s {
            v.push((format!("ycsb-{c} ops_per_s"), *x));
        }
        for (n, x) in &self.counters {
            v.push((format!("count {n}"), *x));
        }
        for (workload, suffix, c) in &self.phase_counts {
            for (base, x) in c.each() {
                if let Some(x) = x {
                    v.push((format!("{workload} {base}{suffix}"), x));
                }
            }
        }
        v
    }

    /// The pass line's self-check: one word a phase with the three raw
    /// counts it was judged on (the threads' device bytes, their major
    /// faults, the group's refaults), or nothing for an unbounded pass.
    fn counts_verdicts(&self) -> String {
        if self.phase_counts.is_empty() {
            return String::new();
        }
        let words: Vec<String> = self
            .phase_counts
            .iter()
            .map(|(w, suffix, c)| {
                format!(
                    "{w}{suffix} {} ({}B/{}mf/{}rf)",
                    if c.plausible { "ok" } else { "odd" },
                    c.own_read,
                    c.own_majflt,
                    c.refaults
                )
            })
            .collect();
        format!("  counts {}", words.join(", "))
    }
}

/// The counts a bounded row records around one timed phase, per
/// operation of the phase (`COUNTS`), from readings taken before its
/// first operation and after its last and never between.
#[derive(Clone, Copy, Debug)]
struct PhaseCounts {
    /// The operating threads' own device bytes read, from each thread's
    /// `/proc/thread-self/io`.
    read_bytes_per_op: f64,
    /// The operating threads' own major faults, from each thread's
    /// `/proc/thread-self/stat`.
    majfaults_per_op: f64,
    /// The whole process's device bytes read less the operating threads':
    /// the merges' streaming and a comparator's background jobs, and, for
    /// a threaded phase, the readers' opening before their release, named
    /// rather than blended into the readers'.
    bg_read_bytes_per_op: f64,
    /// The group's `workingset_refault_file`, which is every thread's.
    refaults_per_op: f64,
    /// The device's read ticks over its read I/Os across the phase,
    /// device-wide; none where the device's stat file is unreadable.
    read_us_per_io: Option<f64>,
    /// `read_bytes / 4096 >= majfaults >= refaults`: every major fault
    /// read at least a page from the device, and a refault is a major
    /// fault of a page the group had and lost. A phase that fails it has
    /// a count that is not what its name says, and the pass line says so.
    plausible: bool,
    /// The three raw counts the check was made on -- the operating
    /// threads' device bytes, their major faults and the group's refaults
    /// over the phase -- printed beside the verdict so a reader of the
    /// pass line sees what was judged.
    own_read: u64,
    own_majflt: u64,
    refaults: u64,
}

impl PhaseCounts {
    /// Each count under its base name, in `COUNTS` order.
    fn each(&self) -> [(&'static str, Option<f64>); 5] {
        [
            (COUNTS[0].0, Some(self.read_bytes_per_op)),
            (COUNTS[1].0, Some(self.majfaults_per_op)),
            (COUNTS[2].0, Some(self.bg_read_bytes_per_op)),
            (COUNTS[3].0, Some(self.refaults_per_op)),
            (COUNTS[4].0, self.read_us_per_io),
        ]
    }
}

/// One reading of everything a phase's counts are deltas of.
struct CountSnap {
    /// The whole process's device bytes.
    process: IoCounters,
    /// This thread's own device bytes and major faults, which are the
    /// operating thread's for a single-threaded phase.
    own_read: u64,
    own_majflt: u64,
    /// The group's refaults.
    refaults: u64,
    /// The device's (read I/Os, read ticks in ms), where readable.
    dev: Option<(u64, u64)>,
}

/// Takes the readings around a bounded pass's phases, and nothing for
/// an unbounded one. The device is the temp directory's, falling back to
/// `vda`, and its stat file read once to decide whether it is there.
struct Counting<'a> {
    bound: Option<&'a Bound>,
    dev_stat: Option<PathBuf>,
    out: Vec<(String, String, PhaseCounts)>,
}

impl<'a> Counting<'a> {
    /// Bounded, the three counters a phase's counts are read from are
    /// probed once here (`probe_counts`), so a file or a field that is
    /// missing refuses the pass by name before anything is measured
    /// rather than recording a zero that reads as a store that never
    /// left the cache. Unbounded, none of them is read at all.
    fn new(bound: Option<&'a Bound>, dir: &Path) -> Result<Counting<'a>, String> {
        if let Some(b) = bound {
            probe_counts(b)?;
        }
        let dev_stat = bound.map(|_| {
            let dev = env::block_device_of(dir).unwrap_or_else(|| "vda".to_string());
            PathBuf::from(format!("/sys/block/{dev}/stat"))
        });
        let dev_stat = dev_stat.filter(|p| dev_read_stat(p).is_some());
        Ok(Counting {
            bound,
            dev_stat,
            out: Vec::new(),
        })
    }

    /// The readings before a phase's first operation, or none unbounded.
    /// A counter that cannot be read after the probe leaves the phase
    /// uncounted, never counted as zero.
    fn begin(&self) -> Option<CountSnap> {
        let bound = self.bound?;
        Some(CountSnap {
            process: IoCounters::read_now(),
            own_read: thread_read_bytes()?,
            own_majflt: majflt_self()?,
            refaults: bound.stat().ok()?.workingset_refault_file,
            dev: self.dev_stat.as_deref().and_then(dev_read_stat),
        })
    }

    /// The readings after a phase's last operation, against `begun`, over
    /// `ops` operations; `workers` is the operating threads' own (device
    /// bytes, major faults) summed where the phase ran on threads of its
    /// own, and this thread's delta otherwise.
    fn end(
        &mut self,
        begun: Option<CountSnap>,
        workload: &str,
        suffix: &str,
        workers: Option<(u64, u64)>,
        ops: u64,
    ) {
        let (Some(b), Some(bound)) = (begun, self.bound) else {
            return;
        };
        let process = IoCounters::read_now().since(&b.process).read_bytes;
        let own = match workers {
            Some(w) => Some(w),
            None => match (thread_read_bytes(), majflt_self()) {
                (Some(r), Some(m)) => {
                    Some((r.saturating_sub(b.own_read), m.saturating_sub(b.own_majflt)))
                }
                _ => None,
            },
        };
        let (Some((own_read, own_majflt)), Ok(stat)) = (own, bound.stat()) else {
            return;
        };
        let refaults = stat.workingset_refault_file.saturating_sub(b.refaults);
        let per = |n: u64| n as f64 / ops.max(1) as f64;
        let read_us_per_io = match (b.dev, self.dev_stat.as_deref().and_then(dev_read_stat)) {
            (Some((ios0, ticks0)), Some((ios1, ticks1))) if ios1 > ios0 => {
                Some((ticks1.saturating_sub(ticks0) * 1000) as f64 / (ios1 - ios0) as f64)
            }
            _ => None,
        };
        self.out.push((
            workload.to_string(),
            suffix.to_string(),
            PhaseCounts {
                read_bytes_per_op: per(own_read),
                majfaults_per_op: per(own_majflt),
                bg_read_bytes_per_op: per(process.saturating_sub(own_read)),
                refaults_per_op: per(refaults),
                read_us_per_io,
                plausible: own_read / 4096 >= own_majflt && own_majflt >= refaults,
                own_read,
                own_majflt,
                refaults,
            },
        ));
    }
}

/// The refusals for a thread's own counters, named by the file.
const THREAD_IO_MISSING: &str = "/proc/thread-self/io has no read_bytes field or cannot be read";
const THREAD_STAT_MISSING: &str = "/proc/thread-self/stat has no majflt field or cannot be read";

/// Read each of the three counters a bounded phase's counts come from
/// once -- the group's `memory.stat` with its two counter files, this
/// thread's `/proc/thread-self/io` and `/proc/thread-self/stat` -- and
/// refuse, naming the file, where one cannot be read: a bounded run or
/// pairing calls it before its floors, and `Counting::new` again for
/// every pass.
pub fn probe_counts(bound: &Bound) -> Result<(), String> {
    bound
        .stat()
        .map_err(|e| format!("the bound's counters: {e}"))?;
    thread_read_bytes().ok_or_else(|| THREAD_IO_MISSING.to_string())?;
    majflt_self().ok_or_else(|| THREAD_STAT_MISSING.to_string())?;
    Ok(())
}

/// `/sys/block/<dev>/stat`'s (read I/Os, read ticks in ms): the first and
/// fourth fields (`Documentation/block/stat.rst`).
fn dev_read_stat(path: &Path) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut f = text.split_whitespace().map(|x| x.parse::<u64>().ok());
    let ios = f.next()??;
    let ticks = f.nth(2)??;
    Some((ios, ticks))
}

/// This thread's own bytes read from the device so far, from
/// `/proc/thread-self/io`; `None` where the file or the field is
/// missing, never a zero that reads as a count.
fn thread_read_bytes() -> Option<u64> {
    std::fs::read_to_string("/proc/thread-self/io")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("read_bytes:"))
                .and_then(|v| v.trim().parse().ok())
        })
}

/// This thread's major page faults so far: the tenth field after the
/// comm's closing parenthesis, two past the minor faults; `None` where
/// the file or the field is missing.
fn majflt_self() -> Option<u64> {
    std::fs::read_to_string("/proc/thread-self/stat")
        .ok()
        .and_then(|s| stat_majflt(&s))
}

fn stat_majflt(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(9).and_then(|v| v.parse().ok()))
}

/// Where a pass asks whether the second form is what the arm says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ZStage {
    /// After the ordered load, whose `sync` makes the form files.
    Loaded,
    /// After the point reads.
    Read,
    /// After the shuffled load into the pass's second store, whose `sync`
    /// makes that store's form files as the first's made the first's: the
    /// partitions a shuffled load leaves are the ones merges wrote, and a
    /// settle that ran out of rounds before the merges did leaves some raw.
    Shuffled,
}

impl ZStage {
    /// The moment, as the refusal words it.
    fn when(self) -> &'static str {
        match self {
            ZStage::Loaded => "after the load",
            ZStage::Read => "after the reads",
            ZStage::Shuffled => "after the shuffled load",
        }
    }
}

/// The share of a pass's point reads, in percent, that a forced arm may
/// have served from the raw form before the pass is not a measurement of the
/// form: the stop the arm's pre-registration names, `zpage_rawserves` over a
/// hundredth of the reads. The allowance is for what the engine decides on
/// purpose and counts -- a sliver of a partition under the floor, which is
/// read raw by design -- and not for a form file that went out of service,
/// which is refused whatever its share.
const Z_RAW_SHARE_MAX_PCT: u64 = 1;

/// The refusal of an arm whose second form is not what its name says.
/// A forced arm with no form file, or with the partitions it could cover
/// raw, or with a form file out of service, or whose reads went to the raw
/// form beyond the share that is the engine's own decision, posts the raw
/// form's throughput under the form's name; an arm that does not force the
/// form and has read from a form file has been priced as something else.
/// Either is a measurement of the wrong store, found after the load that
/// made it. A partition left raw because it is under the engine's floor is
/// the engine's own decision, counted and printed
/// (`zpage_declined_small_bytes`), and covers nothing the form is for;
/// every other partition without a form file was declined for what it holds
/// or failed, and refuses the pass. `None` is an engine with no such form,
/// which nothing is asked of.
fn zpages_refusal(arm: &str, size: u64, stage: ZStage, z: Option<ZState>) -> Result<(), String> {
    let Some(z) = z else { return Ok(()) };
    let wrong = |what: String| {
        Err(format!(
            "{arm} at {size}: {what}, so this is not a measurement of the form"
        ))
    };
    let when = stage.when();
    match (stage, z.forced) {
        (ZStage::Loaded | ZStage::Shuffled, true) if z.form_files == 0 => wrong(format!(
            "the form is forced and none of {} partitions has a form file {when} \
             ({} are under the floor; raise the key count)",
            z.partitions, z.small
        )),
        (ZStage::Loaded | ZStage::Shuffled, true) if z.form_files + z.small != z.partitions => {
            wrong(format!(
                "the form is forced and {} of {} partitions have a form file {when} \
                 ({} are under the floor); the rest were declined or failed",
                z.form_files, z.partitions, z.small
            ))
        }
        (ZStage::Loaded | ZStage::Shuffled, true) if z.files != z.form_files || z.stray != 0 => {
            wrong(format!(
                "the form is forced and the store's directory holds {} form files and {} \
                 other files named for the form beside {} live form files {when}: a file no \
                 partition claims is disk the row counts",
                z.files, z.stray, z.form_files
            ))
        }
        (_, true) if z.damaged > 0 => wrong(format!(
            "the form is forced and {} of {} form files are out of service {when}: a check \
             of one of their pages failed, and a partition is read raw from then on",
            z.damaged, z.form_files
        )),
        (ZStage::Read, true) if z.reads == 0 => wrong(
            "the form is forced and no point read of the pass was answered from a form file".into(),
        ),
        (ZStage::Read, true) if z.raw * 100 > (z.reads + z.raw) * Z_RAW_SHARE_MAX_PCT => {
            wrong(format!(
                "the form is forced and {} of {} reads and counts went to the raw form \
                 (over {Z_RAW_SHARE_MAX_PCT}%)",
                z.raw,
                z.reads + z.raw
            ))
        }
        (ZStage::Read, false) if z.reads != 0 => wrong(format!(
            "the form is not forced and {} point reads were answered from a form file",
            z.reads
        )),
        _ => Ok(()),
    }
}

/// The single-threaded point reads of a pass: `reads` of them, each over a
/// key drawn uniformly from the loaded set and timed into `h`, the phase
/// counted around, and its seconds returned. Then the question whether a
/// forced arm read through its form, asked here and not left to the row:
/// reads that went to the raw form post its throughput under the form's name.
fn read_phase(
    e: &mut dyn Engine,
    arm: &str,
    size: u64,
    reads: u64,
    h: &mut Hist,
    counting: &mut Counting<'_>,
) -> Result<f64, String> {
    let mut kb = [0u8; KEY_SIZE];
    let mut g = KeyGen::new(KeyDist::Uniform, size, 7);
    let c0 = counting.begin();
    phase_mark(arm, "read", "start");
    let t = Instant::now();
    for _ in 0..reads {
        db_key_into(g.next(), &mut kb);
        let t1 = Instant::now();
        e.get(&kb)?;
        h.record(t1.elapsed().as_nanos() as u64);
    }
    let read_s = t.elapsed().as_secs_f64();
    phase_mark(arm, "read", "end");
    counting.end(c0, "read", "", None, reads);
    zpages_refusal(arm, size, ZStage::Read, e.zpages_state())?;
    Ok(read_s)
}

/// The pass's second store loaded in a shuffled order, its seconds returned.
/// Then the question whether a forced arm's form covers it: the store is the
/// one a shuffled load's merges wrote, and the form's `sync` gives up on a
/// store whose merges outlast its rounds, so the lag sweep that follows
/// would be over the raw form under the form's name.
fn shuffled_load(
    e: &mut dyn Engine,
    arm: &str,
    size: u64,
    plan: &Plan,
    payload: &Payload,
    counting: &mut Counting<'_>,
) -> Result<f64, String> {
    let perm = Permutation::new(size, 0x5EED);
    let c0 = counting.begin();
    let (secs, _) = load(e, size, plan, payload, |i| perm.at(i))?;
    counting.end(c0, "load-shuffled", "", None, size);
    zpages_refusal(arm, size, ZStage::Shuffled, e.zpages_state())?;
    Ok(secs)
}

fn one_pass(
    arm: &str,
    dir: &Path,
    size: u64,
    map_gb: usize,
    plan: &Plan,
    payload: &Payload,
) -> Result<OnePass, String> {
    // Ordered load, then reads, scans and the YCSB mixes over it. The
    // store opens into a primed pool (`prime_fresh_pool`), as every store
    // a pass opens does. Under a bound every timed phase has its counts
    // read around it (`Counting`), and the plan's passes say which phases
    // run at all; a phase left out leaves a NaN where its rate would be.
    let passes = plan.passes;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // The directory first, since the counting finds the device through it.
    let mut counting = Counting::new(plan.bound.as_ref(), dir)?;
    let prime_a = prime_fresh_pool(prime_bytes_for(size, plan.bound_bytes()))?;
    let mut e = engines::open(arm, dir, map_gb)?;
    let c0 = counting.begin();
    let (load_s, wrote) = load(e.as_mut(), size, plan, payload, |i| i)?;
    counting.end(c0, "load", "", None, size);
    // The form's form files are made by the `sync` the load ends in: a forced
    // arm that is not covered is refused here and not after the hour its
    // reads would take.
    zpages_refusal(arm, size, ZStage::Loaded, e.zpages_state())?;
    let stored = size as f64 * (KEY_SIZE + plan.value_size) as f64;
    // Read here so that this and the device bytes describe one moment: what
    // the engine wrote to get there, and what it occupies having got there.
    // Neither follows from the other -- ordered ingest cut supdb's device
    // bytes from 2.63 to 1.48 per byte stored and left its 1.44 on disk
    // exactly where it was.
    //
    // Through the engine rather than over its directory, because
    // `Engine::size_bytes` is already the trait's answer to this and every
    // adapter implements it; a second walk here would be a second
    // definition of what a store weighs, and an engine that one day keeps
    // a file somewhere else would have to be taught twice.
    //
    // It is what the arm has on disk when it says the load is durable, not
    // what it settles to: `rocksdb-tuned` holds its memtable rather than
    // flushing it, on purpose, because a flush would charge that arm a
    // compaction the others do not pay here. Measured at 300k keys that is
    // 1.03 against the 1.00 its flushed SST weighs, so the point costs it
    // about 3% and is not where a size comparison turns.
    let on_disk = e.size_bytes();

    // The point reads: `size` of them, or the divided count out of core
    // (`Plan::divided`), each a key drawn uniformly over the loaded set.
    let reads = plan.divided(size);
    let mut kb = [0u8; KEY_SIZE];
    let mut h = Hist::new();
    let mut read_s = f64::NAN;
    if passes.read {
        read_s = read_phase(e.as_mut(), arm, size, reads, &mut h, &mut counting)?;
    }

    // The same reads over each thread count, on the store as loaded and
    // with the writer idle: the single-threaded pass's operations on every
    // thread, so a pass on four threads runs about as long as the pass on
    // one and not a quarter of it -- split four ways, the smallest rung's
    // scans were over in the time a thread takes to wake. Every key was
    // loaded with one value of `value_size` bytes, and that is what each
    // thread must read back.
    let mut reads_s_threaded = Vec::with_capacity(plan.threads.len());
    for &threads in &plan.threads {
        if !passes.read_threaded {
            break;
        }
        let ops = reads * threads as u64;
        let c0 = counting.begin();
        let got = threaded(
            e.as_ref(),
            "read",
            threads,
            ops,
            size,
            0x7EAD,
            Op::Get,
            c0.is_some(),
        )?;
        counting.end(c0, "read", &format!("_{threads}t"), Some(got.own), ops);
        let want = ops * plan.value_size as u64;
        if got.bytes != want {
            return Err(format!(
                "{arm} at {size}: point reads on {threads} threads read back {} bytes \
                 and the store holds {want}; a reader over a different store is not a measurement",
                got.bytes
            ));
        }
        reads_s_threaded.push((threads, ops as f64 / got.secs));
    }

    let scans = plan.divided((size / plan.scan_len as u64).max(1));
    let scan_keys = size.saturating_sub(plan.scan_len as u64).max(1);
    let mut g2 = KeyGen::new(KeyDist::Uniform, scan_keys, 11);
    // With `SUPDB_SCAN_OPS` set, every scan of the pass is timed on its
    // own, for every arm, and the pass line names its first scans beside
    // the rest: a pass of a hundred scans at the smallest rung is a few
    // hundred microseconds, and a first scan that builds what the pass
    // needs once is a share of it no rate shows. The clock reads around
    // every scan are a cost the rows must not carry.
    let per_op = std::env::var_os("SUPDB_SCAN_OPS").is_some();
    let mut op_us: Vec<f64> = Vec::new();
    let mut scan_s = f64::NAN;
    if passes.scan {
        let c0 = counting.begin();
        phase_mark(arm, "scan", "start");
        let t = Instant::now();
        for _ in 0..scans {
            db_key_into(g2.next(), &mut kb);
            if per_op {
                let t0 = Instant::now();
                e.range(&kb, plan.scan_len)?;
                op_us.push(t0.elapsed().as_secs_f64() * 1e6);
            } else {
                e.range(&kb, plan.scan_len)?;
            }
        }
        scan_s = t.elapsed().as_secs_f64();
        phase_mark(arm, "scan", "end");
        counting.end(c0, "scan", "", None, scans);
    }
    if per_op && !op_us.is_empty() {
        let mut sorted = op_us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let med = sorted[sorted.len() / 2];
        let head: Vec<String> = op_us.iter().take(4).map(|u| format!("{u:.1}")).collect();
        eprintln!(
            "scanops {} size {} scans {} first {} med {:.3} max {:.1} sum_us {:.1} pass_us {:.1}",
            e.name(),
            size,
            scans,
            head.join("/"),
            med,
            sorted[sorted.len() - 1],
            op_us.iter().sum::<f64>(),
            scan_s * 1e6
        );
    }

    // And the scans, `size` of them a thread -- the threaded read pass's
    // count, a hundred times the single-threaded scan pass's. That pass
    // is a few milliseconds at the small rungs, and a threaded pass that
    // short is within the host's own unevenness between vCPUs: its span
    // is the slowest thread's, and the slowest thread was whichever vCPU
    // the host had shorted, for every engine alike (bench/CLAUDE.md, the
    // vCPU entry). The read cells, tens of milliseconds, are tight in
    // every row; the scans run as long now. Every start is at least
    // `scan_len` below the top, so every scan walks `scan_len` loaded
    // entries.
    let mut scan_entries_s_threaded = Vec::with_capacity(plan.threads.len());
    for &threads in &plan.threads {
        if !passes.scan_threaded {
            break;
        }
        let ops = plan.divided(size) * threads as u64;
        let c0 = counting.begin();
        let got = threaded(
            e.as_ref(),
            "scan",
            threads,
            ops,
            scan_keys,
            0x5CA0,
            Op::Range(plan.scan_len),
            c0.is_some(),
        )?;
        counting.end(c0, "scan", &format!("_{threads}t"), Some(got.own), ops);
        let want = ops * (plan.scan_len as u64).min(size) * plan.value_size as u64;
        if got.bytes != want {
            return Err(format!(
                "{arm} at {size}: scans on {threads} threads read back {} bytes \
                 and the store holds {want}; a reader over a different store is not a measurement",
                got.bytes
            ));
        }
        scan_entries_s_threaded.push((threads, (ops * plan.scan_len as u64) as f64 / got.secs));
    }

    let mut ycsb_ops_s = Vec::with_capacity(YCSB.len());
    let mut inserted = 0u64;
    let mix_ops = plan.divided(ycsb_ops(size));
    // With `SUPDB_MIX_COUNTERS` set, the store's counters around each mix,
    // as `SUPDB_LAG_COUNTERS` reads them around a lag point: one
    // `mixcounters` line a mix on stderr, read before the mix and after it
    // and never between. A mix's commits are inside its timed region, so
    // the line's burst is zero and its pass is the whole mix.
    let mix_counting = std::env::var_os("SUPDB_MIX_COUNTERS").is_some();
    for letter in YCSB_ORDER {
        if !passes.mixes {
            break;
        }
        let w = YCSB.iter().find(|w| w.0 == letter).unwrap();
        let c0 = if mix_counting {
            e.counters()
        } else {
            Vec::new()
        };
        let (flt_up0, flt_main0) = if mix_counting {
            (minflt_of("supdb-upkeep"), minflt_self())
        } else {
            (0, 0)
        };
        let mix = format!("ycsb-{letter}");
        let cb = counting.begin();
        phase_mark(arm, &mix, "start");
        let secs = ycsb(e.as_mut(), arm, w, size, mix_ops, &mut inserted, payload)?;
        phase_mark(arm, &mix, "end");
        counting.end(cb, &mix, "", None, mix_ops);
        ycsb_ops_s.push((letter, mix_ops as f64 / secs));
        if mix_counting {
            let c1 = e.counters();
            counters_line(
                "mixcounters",
                arm,
                &letter.to_string(),
                (
                    minflt_of("supdb-upkeep").saturating_sub(flt_up0),
                    minflt_self().saturating_sub(flt_main0),
                ),
                (0.0, secs * 1e3),
                &c0,
                &c1,
            );
        }
    }

    // The threaded scans again, on the store the mixes leave: the keys
    // they updated and inserted are unsealed, so a handle's pass builds
    // the blocks it walks, where the pass on the store as loaded walks
    // clean partitions. Every thread's bytes are at least the loaded
    // store's, since a mix only adds to a key; what an engine adds for
    // an update is its own and is not held to. `size` scans a thread, as
    // above.
    let mut scan_mixed_threaded = Vec::with_capacity(plan.threads.len());
    for &threads in &plan.threads {
        if !passes.scan_mixed {
            break;
        }
        let ops = plan.divided(size) * threads as u64;
        let c0 = counting.begin();
        let got = threaded(
            e.as_ref(),
            "scan-mixed",
            threads,
            ops,
            scan_keys,
            0x5CA1,
            Op::Range(plan.scan_len),
            c0.is_some(),
        )?;
        counting.end(
            c0,
            "scan-mixed",
            &format!("_{threads}t"),
            Some(got.own),
            ops,
        );
        let want = ops * (plan.scan_len as u64).min(size) * plan.value_size as u64;
        if got.bytes < want {
            return Err(format!(
                "{arm} at {size}: scans on {threads} threads after the mixes read back {} bytes \
                 and the loaded store alone holds {want}; a reader over a different store is not a measurement",
                got.bytes
            ));
        }
        scan_mixed_threaded.push((threads, (ops * plan.scan_len as u64) as f64 / got.secs));
    }
    // A pass opens two stores -- this one, and a fresh one below for the
    // shuffled load and the lag sweep -- so the counts of the first are
    // taken before it goes. Read from the second alone they described the
    // lag store and nothing else: no reader handle scans there, so they
    // said no read ever took a canonical form when the question had not
    // been asked of the store where the reads happen.
    let mut counters = e.counters();
    drop(e);
    release_freed_memory();
    let _ = std::fs::remove_dir_all(dir);

    // The second store, for the shuffled load and the lag sweep, built
    // only when one of them runs: the same keys in a shuffled order, on a
    // fresh store, into a primed pool again.
    let mut scan_lag = Vec::with_capacity(LAG_PCT.len());
    let mut burst_lag = Vec::with_capacity(LAG_PCT.len());
    let mut prime_b = f64::NAN;
    let mut shuf_s = f64::NAN;
    if passes.load || passes.lag {
        prime_b = prime_fresh_pool(prime_bytes_for(size, plan.bound_bytes()))?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut e = engines::open(arm, dir, map_gb)?;
        shuf_s = shuffled_load(e.as_mut(), arm, size, plan, payload, &mut counting)?;
        if passes.lag {
            lag_sweep(
                e.as_mut(),
                arm,
                size,
                scans,
                scan_keys,
                plan,
                payload,
                &mut counting,
                &mut scan_lag,
                &mut burst_lag,
            )?;
        }
        for (name, v) in e.counters() {
            match counters.iter_mut().find(|(n, _)| *n == name) {
                Some((_, had)) => *had += v,
                None => counters.push((name, v)),
            }
        }
        drop(e);
        release_freed_memory();
    }

    Ok(OnePass {
        prime_us_per_page: prime_a.max(prime_b),
        load_ops_s: if passes.load {
            size as f64 / load_s
        } else {
            f64::NAN
        },
        load_bpb: if passes.load {
            wrote as f64 / stored
        } else {
            f64::NAN
        },
        disk_bpb: if passes.load {
            on_disk as f64 / stored
        } else {
            f64::NAN
        },
        shuffled_ops_s: if passes.load {
            size as f64 / shuf_s
        } else {
            f64::NAN
        },
        scan_lag,
        burst_lag,
        reads_s: reads as f64 / read_s,
        p99_us: if passes.read {
            h.percentile(99.0) as f64 / 1000.0
        } else {
            f64::NAN
        },
        scan_entries_s: (scans * plan.scan_len as u64) as f64 / scan_s,
        reads_s_threaded,
        scan_entries_s_threaded,
        ycsb_ops_s,
        scan_mixed_threaded,
        counters,
        phase_counts: counting.out,
    })
}

/// The lag sweep, on the store the shuffled load just left, so it
/// costs no load of its own: the same scans as the scan workload, at
/// each depth of unmerged writes. The first point is the drained
/// store; each point after it writes another slice of the store and
/// scans without merging in between, so the curve is scan throughput
/// against merge lag rather than one number at whatever lag a
/// workload happened to leave. The bursts are never divided: a store is
/// updated by its slice whole or the point is another store's. Each
/// point's scans are counted around under `scan-lag` and its burst
/// under `lag-burst`, both with the point's suffix.
#[allow(clippy::too_many_arguments)]
fn lag_sweep(
    e: &mut dyn Engine,
    arm: &str,
    size: u64,
    scans: u64,
    scan_keys: u64,
    plan: &Plan,
    payload: &Payload,
    counting: &mut Counting<'_>,
    scan_lag: &mut Vec<(u64, f64)>,
    burst_lag: &mut Vec<(u64, f64)>,
) -> Result<(), String> {
    let mut kb = [0u8; KEY_SIZE];
    let mut vrng = Rng::new(0x1A5);
    let mut updated = 0u64;
    let mut ug = KeyGen::new(KeyDist::Uniform, size, 0x1A6);
    let mut buf = Batch::with_capacity(plan.batch, payload.value_size());
    for pct in LAG_PCT {
        let want = size * pct / 100;
        let suffix = format!("_lag{pct}pct");
        // The burst is timed as well as the scans after it: an engine can
        // make its scans fast by converting the burst's writes while it
        // takes them, and a point that times only the scans would credit
        // the conversion without charging for it.
        let wrote = want.saturating_sub(updated);
        // With `SUPDB_LAG_COUNTERS` set, the store's counters around each
        // point, read before its burst and after its pass and never
        // between, since a counter read takes the writer's upkeep home.
        // The deltas go to stderr as `lagcounters`, one line a point: the
        // scans' phases before their walk, the threads' time, the blocks
        // built and the forms copied, which say what a pass was made of
        // in the suite's own shape rather than a probe's.
        let counting_store = std::env::var_os("SUPDB_LAG_COUNTERS").is_some();
        let c0 = if counting_store {
            e.counters()
        } else {
            Vec::new()
        };
        let (flt_up0, flt_main0) = if counting_store {
            (minflt_of("supdb-upkeep"), minflt_self())
        } else {
            (0, 0)
        };
        let cb = if wrote > 0 { counting.begin() } else { None };
        let tw = Instant::now();
        while updated < want {
            db_key_into(ug.next(), &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            updated += 1;
            if buf.len() == plan.batch {
                buf.flush_updates(e)?;
            }
        }
        if !buf.is_empty() {
            buf.flush_updates(e)?;
        }
        let burst_secs = tw.elapsed().as_secs_f64();
        if wrote > 0 {
            counting.end(cb, "lag-burst", &suffix, None, wrote);
            burst_lag.push((pct, wrote as f64 / burst_secs));
        }
        // The process's anonymous bytes where the burst ends, before a scan
        // has shed or built anything: what a burst leaves in the forms'
        // memory is the point the pass after it cannot see, since the
        // `lagmem` line is read at the pass's end. Read from the kernel and
        // not from the engine, so it takes nothing back from the upkeep.
        // It does stand between the burst and the scans, which the suite
        // otherwise reads nothing between: a read of a `/proc` file, and
        // under `SUPDB_LAG_ARENAS` also the allocator's report by arena
        // (`malloc_info`), which walks every free chunk of every arena
        // under the arena's lock, milliseconds with tens of thousands of
        // chunks and the upkeep thread's allocations among the waits. That
        // lengthens the window the thread files the burst in, the same for
        // every arm; the rates of a run that counts are not read for it.
        if counting_store {
            eprintln!(
                "lagburst {arm} {pct} anon_mb {}",
                anon_resident_bytes().map_or(0, |b| b >> 20)
            );
            if std::env::var_os("SUPDB_LAG_ARENAS").is_some() {
                lag_arenas("lagburstarenas", arm, &pct.to_string());
            }
        }
        if pct == 0 {
            // Nothing unmerged: the shuffled load's own tail is drained
            // here, so the first point is the fully merged store.
            e.sync()?;
        }
        let mut g3 = KeyGen::new(KeyDist::Uniform, scan_keys, 0x1A7);
        let cs = counting.begin();
        lag_mark(arm, pct, "start");
        let t = Instant::now();
        for _ in 0..scans {
            db_key_into(g3.next(), &mut kb);
            e.range(&kb, plan.scan_len)?;
        }
        let secs = t.elapsed().as_secs_f64();
        lag_mark(arm, pct, "end");
        counting.end(cs, "scan-lag", &suffix, None, scans);
        scan_lag.push((pct, (scans * plan.scan_len as u64) as f64 / secs));
        if counting_store {
            let c1 = e.counters();
            counters_line(
                "lagcounters",
                arm,
                &pct.to_string(),
                (
                    minflt_of("supdb-upkeep").saturating_sub(flt_up0),
                    minflt_self().saturating_sub(flt_main0),
                ),
                (burst_secs * 1e3, secs * 1e3),
                &c0,
                &c1,
            );
            lag_census(arm, &pct.to_string(), e);
        }
    }
    Ok(())
}

/// With `SUPDB_LAG_COUNTERS`, the lines after a point's `lagmem`: the
/// engine's census of what it holds (`Engine::memory_census`), the bytes
/// in megabytes and the counts as they are, beside the process's
/// anonymous bytes and the allocator's in use and free bytes taken again
/// at the same moment; and the allocator's bytes by arena, which says
/// which thread's frees the free bytes are. Read after `lagmem`, so the
/// census's own allocations are in neither.
fn lag_census(arm: &str, point: &str, e: &dyn Engine) {
    let census = e.memory_census();
    if census.is_empty() {
        return;
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let mi = {
        // SAFETY: mallinfo2 reads the allocator's own totals.
        let mi = unsafe { libc::mallinfo2() };
        (mi.uordblks as u64, mi.fordblks as u64)
    };
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    let mi = (0u64, 0u64);
    let mb = |b: u64| b as f64 / (1u64 << 20) as f64;
    let fields: Vec<String> = census
        .iter()
        .map(|(k, v)| {
            if k.ends_with("_n") {
                format!("{k} {v}")
            } else {
                format!("{k} {:.1}", mb(*v))
            }
        })
        .collect();
    let counters = e.counters();
    let counter = |name: &str| {
        counters
            .iter()
            .find(|(k, _)| *k == name)
            .map_or(0.0, |(_, v)| *v)
    };
    eprintln!(
        "lagcensus {arm} {point} anon_mb {} inuse_mb {:.0} free_mb {:.0} trims {:.0} looks {:.0} look_ms {:.1} | {}",
        anon_resident_bytes().map_or(0, |b| b >> 20),
        mb(mi.0),
        mb(mi.1),
        counter("forms_trims"),
        counter("forms_trim_looks"),
        counter("forms_trim_us") / 1e3,
        fields.join(" ")
    );
    lag_arenas("lagarenas", arm, point);
    // The anonymous mappings of a megabyte and more, by size: what is
    // resident in each size class says which allocations they are (a
    // thread's heap, a memtable's arena block, a snapshot's, a slab's).
    let maps = anon_maps();
    if !maps.is_empty() {
        let total: u64 = maps.iter().map(|m| m.2).sum();
        let top: Vec<String> = maps
            .iter()
            .take(10)
            .map(|(size, count, rss)| format!("{:.1}mb x{count} rss {:.1}mb", mb(*size), mb(*rss)))
            .collect();
        eprintln!(
            "lagmaps {arm} {point} anon_rss_mb {:.0} | {}",
            mb(total),
            top.join(" | ")
        );
    }
    // The probe of what a trim would give back, for the pairing that asks:
    // the freed memory returned to the system and the process's anonymous
    // bytes read again. It changes the points after it, so it is its own
    // variable and off by default.
    if std::env::var_os("SUPDB_LAG_TRIM").is_some() {
        let before = anon_resident_bytes().unwrap_or(0);
        let t = Instant::now();
        release_freed_memory();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        eprintln!(
            "lagtrim {arm} {point} anon_mb {} -> {} trim_ms {ms:.1}",
            before >> 20,
            anon_resident_bytes().unwrap_or(0) >> 20
        );
    }
}

/// The allocator's bytes by arena (`malloc_arenas`) as one line: the
/// bytes in use and free over the process, then each arena's bytes taken
/// from the system, its free bytes and the chunks they are in, and the
/// chunks mapped on their own. Read from the allocator and not from the
/// engine, so it takes nothing back from the upkeep; it still takes each
/// arena's lock to walk its free chunks, so where a burst ends it is
/// asked for (`SUPDB_LAG_ARENAS`) and not given.
fn lag_arenas(tag: &str, arm: &str, point: &str) {
    let mb = |b: u64| b as f64 / (1u64 << 20) as f64;
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let mi = {
        // SAFETY: mallinfo2 reads the allocator's own totals.
        let mi = unsafe { libc::mallinfo2() };
        (mi.uordblks as u64, mi.fordblks as u64)
    };
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    let mi = (0u64, 0u64);
    let (heaps, mmapped) = malloc_arenas();
    let arenas: Vec<String> = heaps
        .iter()
        .map(|(nr, system, free, chunks)| {
            format!(
                "a{nr} system_mb {:.1} free_mb {:.1} chunks {chunks}",
                mb(*system),
                mb(*free)
            )
        })
        .collect();
    if !arenas.is_empty() {
        eprintln!(
            "{tag} {arm} {point} inuse_mb {:.0} free_mb {:.0} | {} | mmapped chunks {} mb {:.1}",
            mb(mi.0),
            mb(mi.1),
            arenas.join(" | "),
            mmapped.0,
            mb(mmapped.1)
        );
    }
}

/// This process's anonymous mappings of at least a megabyte from
/// `/proc/self/smaps`, grouped by size to a tenth of a megabyte: the size,
/// how many, and their resident bytes, the most resident first.
fn anon_maps() -> Vec<(u64, u64, u64)> {
    let Ok(text) = std::fs::read_to_string("/proc/self/smaps") else {
        return Vec::new();
    };
    let mut groups: std::collections::BTreeMap<u64, (u64, u64)> = Default::default();
    let mut current: Option<(bool, u64, u64)> = None;
    let flush = |cur: &mut Option<(bool, u64, u64)>,
                 groups: &mut std::collections::BTreeMap<u64, (u64, u64)>| {
        if let Some((anon, size, rss)) = cur.take() {
            if anon && size >= 1 << 20 {
                // To 128 KB.
                let key = (size + (1 << 16)) >> 17 << 17;
                let g = groups.entry(key).or_default();
                g.0 += 1;
                g.1 += rss;
            }
        }
    };
    for line in text.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.contains('-') && !first.ends_with(':') {
            flush(&mut current, &mut groups);
            let fields: Vec<&str> = line.split_whitespace().collect();
            let anon = fields.len() == 5 || fields.get(5) == Some(&"[heap]");
            current = Some((anon, 0, 0));
        } else if let Some(cur) = current.as_mut() {
            let kb = |l: &str| -> u64 {
                l.split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
                    * 1024
            };
            if line.starts_with("Size:") {
                cur.1 = kb(line);
            } else if line.starts_with("Rss:") {
                cur.2 = kb(line);
            }
        }
    }
    flush(&mut current, &mut groups);
    let mut out: Vec<(u64, u64, u64)> = groups
        .into_iter()
        .map(|(size, (count, rss))| (size, count, rss))
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.2));
    out
}

/// One arena as `malloc_info` reports it (its number, the bytes it has
/// taken from the system, the bytes free in it and the chunks those are
/// in) and, after the arenas, the chunks the allocator mapped on their own
/// (how many and their bytes).
type MallocArena = (u64, u64, u64, u64);
type MallocArenas = (Vec<MallocArena>, (u64, u64));

/// The allocator's arenas as `malloc_info` reports them: for each, its
/// number, the bytes it has taken from the system, the bytes it holds
/// free (the fast bins, the other bins and its top) and the free chunks
/// those are in. The main arena is 0 and a thread's frees go back to the
/// arena its allocation came from, which is how a thread that builds and
/// another that drops leave one arena's memory free and resident.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn malloc_arenas() -> MallocArenas {
    // SAFETY: a temporary file the allocator writes its report to and this
    // function reads back and closes; nothing else holds it.
    let text = unsafe {
        let f = libc::tmpfile();
        if f.is_null() {
            return (Vec::new(), (0, 0));
        }
        libc::malloc_info(0, f);
        let n = libc::ftell(f).max(0) as usize;
        libc::rewind(f);
        let mut buf = vec![0u8; n];
        let got = libc::fread(buf.as_mut_ptr().cast(), 1, n, f);
        libc::fclose(f);
        buf.truncate(got);
        String::from_utf8_lossy(&buf).into_owned()
    };
    // The attribute `name` of the first `<tag ...>` in `s` whose opening
    // starts with `open`, as a number.
    let attr = |s: &str, open: &str, name: &str| -> u64 {
        let Some(at) = s.find(open) else { return 0 };
        let rest = &s[at..];
        let rest = &rest[..rest.find("/>").unwrap_or(rest.len())];
        let key = format!("{name}=\"");
        rest.find(&key)
            .and_then(|i| {
                let v = &rest[i + key.len()..];
                v[..v.find('"')?].parse().ok()
            })
            .unwrap_or(0)
    };
    let mut out = Vec::new();
    for heap in text.split("<heap nr=\"").skip(1) {
        let heap = &heap[..heap.find("</heap>").unwrap_or(heap.len())];
        let nr: u64 = heap
            .split('"')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let fast = attr(heap, "<total type=\"fast\"", "size");
        let rest = attr(heap, "<total type=\"rest\"", "size");
        let chunks = attr(heap, "<total type=\"fast\"", "count")
            + attr(heap, "<total type=\"rest\"", "count");
        let system = attr(heap, "<system type=\"current\"", "size");
        out.push((nr, system, fast + rest, chunks));
    }
    // After the last heap: the chunks the allocator mapped on their own.
    let tail = text.rfind("</heap>").map_or("", |i| &text[i..]);
    let mapped = (
        attr(tail, "<total type=\"mmap\"", "count"),
        attr(tail, "<total type=\"mmap\"", "size"),
    );
    (out, mapped)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn malloc_arenas() -> MallocArenas {
    (Vec::new(), (0, 0))
}

/// Two arms in one process, alternating, paired rep by rep.
///
/// The row series answers whether a quantity moved between commits, and
/// is the wrong instrument for whether one option beats another: its arms
/// run in a fixed order, so the first pays a few milliseconds nobody else
/// does, and a median of five whole-pass throughputs carries about as
/// much spread as a fifteen percent option is worth. Both were measured.
/// A gate on the canonical forms read 1.156x and 1.186x for maintaining
/// regardless over two rows and 1.247x the other way over a third, per
/// rung 1.48, 1.48, 1.02, 0.66, and that comparison was never going to
/// resolve one row at a time.
///
/// So: one process, the two arms alternating, and the order swapped on
/// every other rep so neither stands first more often than the other.
/// Each rep is a pair over the same machine minute, and the difference
/// within a pair is what is reported -- a drift that lifts or drops both
/// cancels instead of landing on whichever arm met it. The sign test over
/// the pairs is the verdict; the ratio of medians is only its size.
pub fn ab(
    arms: (&str, &str),
    size: u64,
    reps: usize,
    plan: &Plan,
    log: &mut dyn std::io::Write,
) -> Result<Vec<AbQuantity>, String> {
    let payload = Payload::new(plan.value_size, plan.compress, 0xE1);
    let map_gb = lmdb_map_gb(size, plan.value_size);
    let root = std::env::temp_dir().join(format!("supdb-ab-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&root);
    let mut pairs: std::collections::BTreeMap<String, Vec<(f64, f64)>> = Default::default();
    let t0 = Instant::now();
    for rep in 0..=reps {
        // Alternate which arm goes first, so the cost of standing first
        // falls on each of them equally instead of on one of them always.
        // Which one is A is the order's, not the name's: an arm set
        // against itself is the control that says what a verdict looks
        // like when nothing differs, and by name it would always be A.
        let first_is_a = rep % 2 == 0;
        let order = if first_is_a {
            [arms.0, arms.1]
        } else {
            [arms.1, arms.0]
        };
        let mut got: Vec<(String, f64)> = Vec::new();
        for (i, arm) in order.iter().enumerate() {
            let dir = root.join(format!("{arm}-{rep}-{i}"));
            let one = one_pass(arm, &dir, size, map_gb, plan, &payload)?;
            let _ = std::fs::remove_dir_all(&dir);
            if i == 0 {
                got = one.flat();
            } else if rep > 0 {
                // Rep zero is the warmup, as it is for a row.
                for (name, second) in one.flat() {
                    let Some((_, first)) = got.iter().find(|(n, _)| *n == name) else {
                        continue;
                    };
                    let (a, b) = if first_is_a {
                        (*first, second)
                    } else {
                        (second, *first)
                    };
                    pairs.entry(name).or_default().push((a, b));
                }
            }
        }
        let _ = writeln!(
            log,
            "{:>6.0}s  rep {rep}/{reps}{}",
            t0.elapsed().as_secs_f64(),
            if rep == 0 { " (warmup)" } else { "" }
        );
    }
    let _ = std::fs::remove_dir_all(&root);
    // The sweep's decision quantity, per point and summed: the burst's
    // milliseconds plus `READ_WEIGHT` times the pass's, from the two
    // rates of each pair. A rate alone credits a change that moves work
    // from the pass into the burst, or the other way, without charging
    // for it; lower is better, as `p99_us` is.
    let entries = ((size / plan.scan_len as u64).max(1) * plan.scan_len as u64) as f64;
    let mut total: Vec<(f64, f64)> = Vec::new();
    for w in LAG_PCT.windows(2) {
        let (prev, pct) = (w[0], w[1]);
        let e = pairs.get(&format!("scan-lag entries_per_s_lag{pct}pct"));
        let u = pairs.get(&format!("scan-lag updates_per_s_lag{pct}pct"));
        let (Some(e), Some(u)) = (e, u) else { continue };
        let wrote = (size * (pct - prev) / 100) as f64;
        let ms = |u: f64, e: f64| 1e3 * (wrote / u + READ_WEIGHT * entries / e);
        let j: Vec<(f64, f64)> = e
            .iter()
            .zip(u)
            .map(|(&(ea, eb), &(ua, ub))| (ms(ua, ea), ms(ub, eb)))
            .collect();
        if total.is_empty() {
            total = j.clone();
        } else {
            for (t, x) in total.iter_mut().zip(&j) {
                t.0 += x.0;
                t.1 += x.1;
            }
        }
        pairs.insert(format!("scan-lag weighted_ms_lag{pct}pct"), j);
    }
    if !total.is_empty() {
        pairs.insert("scan-lag weighted_ms_lagall".into(), total);
    }
    Ok(pairs
        .into_iter()
        .map(|(quantity, v)| AbQuantity::of(quantity, v))
        .collect())
}

/// One quantity's verdict over the pairs.
pub struct AbQuantity {
    pub quantity: String,
    pub a: f64,
    pub b: f64,
    /// Pairs where B was above A, and pairs that were not a tie.
    pub up: usize,
    pub n: usize,
    /// Two-sided sign test over those pairs.
    pub p: f64,
    /// The pairs themselves, (A, B), in rep order.
    pub pairs: Vec<(f64, f64)>,
}

impl AbQuantity {
    fn of(quantity: String, v: Vec<(f64, f64)>) -> AbQuantity {
        let med = |mut x: Vec<f64>| -> f64 {
            x.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
            if x.is_empty() {
                return f64::NAN;
            }
            x[x.len() / 2]
        };
        let a = med(v.iter().map(|p| p.0).collect());
        let b = med(v.iter().map(|p| p.1).collect());
        let up = v.iter().filter(|(x, y)| y > x).count();
        let n = v.iter().filter(|(x, y)| y != x).count();
        AbQuantity {
            quantity,
            a,
            b,
            up,
            n,
            p: sign_p(up, n),
            pairs: v,
        }
    }
}

/// Which of `ps` hold at `alpha` for the family as a whole, by Holm's
/// step-down: the smallest against `alpha / m`, the next against
/// `alpha / (m - 1)`, and so on until one fails, which is valid however
/// the quantities depend on each other -- and a pass's quantities do.
pub fn holm(ps: &[f64], alpha: f64) -> Vec<bool> {
    let mut order: Vec<usize> = (0..ps.len()).collect();
    order.sort_by(|&i, &j| ps[i].total_cmp(&ps[j]));
    let m = ps.len();
    let mut held = vec![false; m];
    for (k, &i) in order.iter().enumerate() {
        if ps[i] > alpha / (m - k) as f64 {
            break;
        }
        held[i] = true;
    }
    held
}

/// Two-sided sign test: the chance of a split at least this lopsided from
/// a coin. Exact, since the pair counts here are small.
fn sign_p(up: usize, n: usize) -> f64 {
    if n == 0 {
        return 1.0;
    }
    let hi = up.max(n - up);
    let mut tail = 0f64;
    for k in hi..=n {
        let mut c = 1f64;
        for i in 0..k {
            c = c * (n - i) as f64 / (i + 1) as f64;
        }
        tail += c;
    }
    (2.0 * tail / 2f64.powi(n as i32)).min(1.0)
}

/// What a reader thread does per operation.
#[derive(Clone, Copy)]
enum Op {
    Get,
    Range(usize),
}

/// Thread `i` of `threads`' share of `ops`: as even as `ops` divides,
/// the remainder one each to the first threads, so the shares sum to
/// `ops` and the work at every count is the single-threaded pass's.
fn share(ops: u64, threads: usize, i: usize) -> u64 {
    ops / threads as u64 + u64::from((i as u64) < ops % threads as u64)
}

/// `ops` operations of `op`, keys drawn uniformly below `keys`, split
/// across `threads` threads, each through a reader of its own -- made on
/// this thread, opened on its own -- and a generator seeded apart from
/// the others'. The threads are released together and each times itself
/// from its release to its finish; the result is the aggregate, seconds
/// from the first release to the last finish, so the spawning and the
/// opening are outside the clock, and the bytes every thread read back,
/// which the caller holds to what the store holds, since a reader over a
/// different store would post a throughput like any other.
///
/// The clock is the threads' own and not this thread's. The first
/// version started it when this thread's `wait` returned, which on a
/// loaded box was tens of microseconds after the workers were running,
/// and a hundred scans on four threads at ten thousand keys were over by
/// then: the smoke run recorded them at billions of entries a second.
///
/// With `SUPDB_THREAD_SPANS` set, one `threadspans` line per pass on
/// stderr, for every arm: the span and how much of it is the threads'
/// release and finish skew, each thread's own wall and CPU time, and the
/// run and run-queue wait of every other thread of the process across
/// the pass. The span of a pass at the small rungs is under two
/// milliseconds where a thread hand-off here costs up to a fifth of one,
/// so a three-thread cell's spread is the skew's as much as the engine's,
/// and the line says which; the per-thread columns say whether a loss
/// against the comparator is every thread's rate, one thread's, or a
/// thread of the engine's on the fourth core. Off, the pass reads no
/// clock it did not; on, two thread-clock reads a thread a pass.
///
/// With `count`, each thread reads its own device bytes and major faults
/// at its release and at its finish, outside its clock, and the sum of
/// the deltas is `Threaded::own`: a bounded pass's counts are the
/// operating threads' (`Counting`), and the process's counters would
/// blend the readers' faults with the engine's merges beside them.
// Eight parameters: clippy::too_many_arguments, allowed as `lag_sweep` allows it.
#[allow(clippy::too_many_arguments)]
fn threaded(
    e: &dyn Engine,
    pass: &str,
    threads: usize,
    ops: u64,
    keys: u64,
    seed: u64,
    op: Op,
    count: bool,
) -> Result<Threaded, String> {
    if threads == 0 {
        return Err("a threaded pass needs at least one thread".to_string());
    }
    let spans = std::env::var_os("SUPDB_THREAD_SPANS").is_some();
    let mut openers = Vec::with_capacity(threads);
    for _ in 0..threads {
        openers.push(e.thread_reader()?);
    }
    let barrier = std::sync::Barrier::new(threads + 1);
    type Timed = Result<(Instant, Instant, u64, u64, (u64, u64)), String>;
    let mut sched_before = Vec::new();
    let results: Vec<Timed> = std::thread::scope(|s| {
        let workers: Vec<_> = openers
            .into_iter()
            .enumerate()
            .map(|(i, open)| {
                let barrier = &barrier;
                let share = share(ops, threads, i);
                // Distinct per thread and deterministic: one multiplicative
                // mix of the base and the thread's index, so the streams
                // do not start a few steps apart.
                let seed = (seed + i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                s.spawn(move || -> Timed {
                    // Every thread reaches the barrier whatever opening
                    // did: one that returned before it would leave the
                    // others and the clock waiting forever, and a panic
                    // in an opener is caught for the same reason.
                    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(open))
                        .unwrap_or_else(|_| Err("the reader's opener panicked".to_string()));
                    let mut g = KeyGen::new(KeyDist::Uniform, keys, seed);
                    let mut kb = [0u8; KEY_SIZE];
                    barrier.wait();
                    // This thread's own counters at its release, read
                    // before its clock starts so the two `/proc` reads
                    // are outside it; a bounded pass only.
                    let (rb0, mf0) = if count {
                        (
                            thread_read_bytes().ok_or_else(|| THREAD_IO_MISSING.to_string())?,
                            majflt_self().ok_or_else(|| THREAD_STAT_MISSING.to_string())?,
                        )
                    } else {
                        (0, 0)
                    };
                    let start = Instant::now();
                    let cpu0 = if spans { thread_cpu_us() } else { 0 };
                    let mut r = opened?;
                    let mut bytes = 0u64;
                    for _ in 0..share {
                        db_key_into(g.next(), &mut kb);
                        bytes += match op {
                            Op::Get => r.get(&kb)?,
                            Op::Range(n) => r.range(&kb, n)?,
                        } as u64;
                    }
                    let end = Instant::now();
                    let cpu = if spans { thread_cpu_us() - cpu0 } else { 0 };
                    let own = if count {
                        (
                            thread_read_bytes()
                                .ok_or_else(|| THREAD_IO_MISSING.to_string())?
                                .saturating_sub(rb0),
                            majflt_self()
                                .ok_or_else(|| THREAD_STAT_MISSING.to_string())?
                                .saturating_sub(mf0),
                        )
                    } else {
                        (0, 0)
                    };
                    Ok((start, end, bytes, cpu, own))
                })
            })
            .collect();
        // The other threads' clocks, read with the readers spawned and
        // not yet released; the readers report their own.
        if spans {
            sched_before = sched_of_tasks();
        }
        barrier.wait();
        workers
            .into_iter()
            .map(|w| {
                w.join()
                    .unwrap_or_else(|_| Err("a reader thread panicked".to_string()))
            })
            .collect()
    });
    let sched_after = if spans { sched_of_tasks() } else { Vec::new() };
    let mut timed = Vec::with_capacity(threads);
    let mut bytes = 0u64;
    let mut own = (0u64, 0u64);
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    for r in results {
        let (start, end, b, cpu, o) = r?;
        bytes += b;
        own.0 += o.0;
        own.1 += o.1;
        first = Some(first.map_or(start, |f| f.min(start)));
        last = Some(last.map_or(end, |l| l.max(end)));
        timed.push((start, end, b, cpu));
    }
    let secs = match (first, last) {
        (Some(f), Some(l)) => l.duration_since(f).as_secs_f64(),
        _ => 0.0,
    };
    if let (true, Some(first), Some(last)) = (spans, first, last) {
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6;
        let last_start = timed.iter().map(|t| t.0).max().unwrap_or(first);
        let first_end = timed.iter().map(|t| t.1).min().unwrap_or(last);
        let mut line = format!(
            "threadspans {} {pass} {threads}t span_us={:.0} start_skew_us={:.0} end_skew_us={:.0}",
            e.name(),
            secs * 1e6,
            us(last_start.duration_since(first)),
            us(last.duration_since(first_end)),
        );
        for (i, (start, end, b, cpu)) in timed.iter().enumerate() {
            line += &format!(
                " | t{i} off_us={:.0} wall_us={:.0} cpu_us={cpu} bytes={b}",
                us(start.duration_since(first)),
                us(end.duration_since(*start)),
            );
        }
        // Every thread present at both readings that ran or waited at
        // all: the readers themselves have exited by the second reading
        // and are the columns above, and this thread's own run across the
        // pass is its wake from the barrier, the join and the second
        // reading's walk of /proc, which says nothing of the pass.
        // SAFETY: a plain syscall with no arguments.
        let me = unsafe { libc::gettid() } as u32;
        for (tid, comm, run1, wait1, slices1) in &sched_after {
            if *tid == me {
                continue;
            }
            let Some((_, _, run0, wait0, slices0)) = sched_before.iter().find(|t| t.0 == *tid)
            else {
                continue;
            };
            let (run, wait, slices) = (run1 - run0, wait1 - wait0, slices1 - slices0);
            if run == 0 && wait == 0 {
                continue;
            }
            line += &format!(
                " | {comm}/{tid} run_us={} wait_us={} slices={slices}",
                run / 1000,
                wait / 1000
            );
        }
        eprintln!("{line}");
    }
    Ok(Threaded { secs, bytes, own })
}

/// What a threaded pass measured: the span from the first thread's release
/// to the last's finish, the bytes every thread read back, and under a
/// bound the threads' own (device bytes read, major faults), summed.
struct Threaded {
    secs: f64,
    bytes: u64,
    own: (u64, u64),
}

/// One YCSB workload over a loaded store: `ops` operations (`ycsb_ops`,
/// divided out of core), writes batched by `YCSB_BATCH` and the tail
/// flushed inside the timed region, inserts taking fresh keys past the
/// loaded range. Returns seconds.
/// With `SUPDB_MIX_KINDS` set, a mix's time by the kind of operation --
/// the reads, the scans and the commits -- summed over the mix and printed
/// as one `mixkinds` line a mix on stderr, for both arms alike, since the
/// clock reads around every operation are a cost the rows must not carry.
/// A mix's rate names none of its kinds; this names each.
struct MixKinds {
    on: bool,
    gets: (u64, f64),
    scans: (u64, f64),
    commits: (u64, f64),
}

impl MixKinds {
    fn new() -> MixKinds {
        MixKinds {
            on: std::env::var_os("SUPDB_MIX_KINDS").is_some(),
            gets: (0, 0.0),
            scans: (0, 0.0),
            commits: (0, 0.0),
        }
    }

    fn start(&self) -> Option<Instant> {
        self.on.then(Instant::now)
    }

    fn add(slot: &mut (u64, f64), t0: Option<Instant>) {
        if let Some(t0) = t0 {
            slot.0 += 1;
            slot.1 += t0.elapsed().as_secs_f64();
        }
    }

    fn report(&self, arm: &str, letter: char, size: u64, ops: u64, total: f64) {
        if self.on {
            eprintln!(
                "mixkinds {arm} {letter} size {size} ops {ops} gets {} {:.3} scans {} {:.3} commits {} {:.3} total {:.3}",
                self.gets.0,
                self.gets.1 * 1e3,
                self.scans.0,
                self.scans.1 * 1e3,
                self.commits.0,
                self.commits.1 * 1e3,
                total * 1e3
            );
        }
    }
}

fn ycsb(
    e: &mut dyn Engine,
    arm: &str,
    w: &(char, u32, u32, u32, u32, u32, KeyDist),
    size: u64,
    ops: u64,
    inserted: &mut u64,
    payload: &Payload,
) -> Result<f64, String> {
    let (letter, pread, pupd, pins, pscan, prmw, dist) = *w;
    let mut keys = KeyGen::new(dist, size, 0x9C5B ^ letter as u64);
    let mut pick = Rng::new(0x5EED ^ letter as u64);
    let mut vrng = Rng::new(0xE2);
    let mut kb = [0u8; KEY_SIZE];
    let mut wbuf = Batch::with_capacity(YCSB_BATCH, payload.value_size());
    let mut wbuf_is_insert = false;
    let mut kinds = MixKinds::new();
    let t = Instant::now();
    for _ in 0..ops {
        let roll = pick.below(100) as u32;
        if roll < pread {
            db_key_into(keys.next(), &mut kb);
            let t0 = kinds.start();
            e.get(&kb)?;
            MixKinds::add(&mut kinds.gets, t0);
        } else if roll < pread + pupd {
            db_key_into(keys.next(), &mut kb);
            wbuf.push(&kb, payload.get(&mut vrng));
            wbuf_is_insert = false;
        } else if roll < pread + pupd + pins {
            db_key_into(size + *inserted, &mut kb);
            *inserted += 1;
            wbuf.push(&kb, payload.get(&mut vrng));
            wbuf_is_insert = true;
        } else if roll < pread + pupd + pins + pscan {
            db_key_into(keys.next(), &mut kb);
            let t0 = kinds.start();
            e.range(&kb, YCSB_SCAN)?;
            MixKinds::add(&mut kinds.scans, t0);
        } else if prmw > 0 {
            db_key_into(keys.next(), &mut kb);
            let t0 = kinds.start();
            e.get(&kb)?;
            MixKinds::add(&mut kinds.gets, t0);
            wbuf.push(&kb, payload.get(&mut vrng));
            wbuf_is_insert = false;
        }
        if wbuf.len() >= YCSB_BATCH {
            let t0 = kinds.start();
            flush_ycsb(&mut wbuf, e, wbuf_is_insert)?;
            MixKinds::add(&mut kinds.commits, t0);
        }
    }
    let t0 = kinds.start();
    flush_ycsb(&mut wbuf, e, wbuf_is_insert)?;
    MixKinds::add(&mut kinds.commits, t0);
    let total = t.elapsed().as_secs_f64();
    kinds.report(arm, letter, size, ops, total);
    Ok(total)
}

fn flush_ycsb(b: &mut Batch, e: &mut dyn Engine, insert: bool) -> Result<(), String> {
    if insert {
        b.flush(e)
    } else {
        b.flush_updates(e)
    }
}

/// Load `size` keys through `order`, batched, ending with one `sync` so a
/// buffered arm's number includes getting its tail to the device once. Returns
/// (seconds, device bytes written).
fn load(
    e: &mut dyn Engine,
    size: u64,
    plan: &Plan,
    payload: &Payload,
    order: impl Fn(u64) -> u64,
) -> Result<(f64, u64), String> {
    let mut vrng = Rng::new(0xE1);
    let mut buf = Batch::with_capacity(plan.batch, payload.value_size());
    let mut kb = [0u8; KEY_SIZE];
    let io0 = IoCounters::read_now();
    let t = Instant::now();
    let (mut tc, mut tfirst, mut tmax, mut nc) = (0f64, 0f64, 0f64, 0usize);
    let flt0 = minflt_self();
    let cpu0 = thread_cpu_us();
    phase_mark(e.name(), "load", "start");
    for i in 0..size {
        db_key_into(order(i), &mut kb);
        buf.push(&kb, payload.get(&mut vrng));
        if buf.len() == plan.batch {
            let t0 = Instant::now();
            buf.flush(e)?;
            let d = t0.elapsed().as_secs_f64();
            if nc == 0 {
                tfirst = d;
            }
            tmax = tmax.max(d);
            tc += d;
            nc += 1;
        }
    }
    let t0 = Instant::now();
    buf.flush(e)?;
    tc += t0.elapsed().as_secs_f64();
    let flt_c = minflt_self().saturating_sub(flt0);
    let cpu_c = thread_cpu_us().saturating_sub(cpu0);
    phase_mark(e.name(), "load", "sync");
    let ts = Instant::now();
    e.sync()?;
    let tsync = ts.elapsed().as_secs_f64();
    let secs = t.elapsed().as_secs_f64();
    phase_mark(e.name(), "load", "end");
    let flt_s = minflt_self().saturating_sub(flt0 + flt_c);
    let cpu_s = thread_cpu_us().saturating_sub(cpu0 + cpu_c);
    // The load's own counters, as `SUPDB_LAG_COUNTERS` is the lag sweep's:
    // the commits' time, the first and the slowest commit, and the closing
    // sync, each with the writer's CPU and faults, which is where the
    // buffered arm's loss at the small rungs sat -- a drain of 8-9 ms
    // against a comparator's one fdatasync of 2 -- and where its loss at
    // the large ones did: commits at the same fault count in every rep
    // and two to four times the CPU in some.
    if std::env::var_os("SUPDB_LOAD_PHASES").is_some() {
        eprintln!(
            "loadphases {} size {} commits {} ms {:.2} first {:.2} max {:.2} sync {:.2} total {:.2} | commits cpu {:.1} flt {} | sync cpu {:.1} flt {}",
            e.name(), size, nc, tc * 1e3, tfirst * 1e3, tmax * 1e3, tsync * 1e3, secs * 1e3,
            cpu_c as f64 / 1e3, flt_c, cpu_s as f64 / 1e3, flt_s
        );
    }
    let wrote = IoCounters::read_now().since(&io0).write_bytes;
    Ok((secs, wrote))
}

/// The durable-write floor: the load's records, framed as length-prefixed
/// key and value, appended to one file in batches of `plan.batch` with one
/// `fdatasync` closing each -- what a WAL does with nothing around it.
/// Records per second. A store's durable load cannot beat this on this
/// device; how far below it lands is the engine's own cost.
fn wal_floor(root: &Path, plan: &Plan, payload: &Payload) -> Result<f64, String> {
    let path = root.join("wal-floor.dat");
    let _ = std::fs::remove_file(&path);
    let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut vrng = Rng::new(0xF10);
    let mut kb = [0u8; KEY_SIZE];
    let mut buf: Vec<u8> = Vec::with_capacity(plan.batch * (KEY_SIZE + plan.value_size + 8));
    let records = WAL_FLOOR_BATCHES * plan.batch as u64;
    let t = Instant::now();
    for i in 0..records {
        db_key_into(i, &mut kb);
        let v = payload.get(&mut vrng);
        buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
        buf.extend_from_slice(&kb);
        buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
        buf.extend_from_slice(v);
        if (i + 1) % plan.batch as u64 == 0 {
            f.write_all(&buf).map_err(|e| e.to_string())?;
            f.sync_data().map_err(|e| e.to_string())?;
            buf.clear();
        }
    }
    let secs = t.elapsed().as_secs_f64();
    drop(f);
    let _ = std::fs::remove_file(&path);
    Ok(records as f64 / secs)
}

/// The sequential-read floor: a file of `bytes`, mapped and walked once
/// front to back, every word touched. Bytes per second. On the second and
/// later reps a file that fits in memory is served from the page cache,
/// which is also what a store that fits in memory sees; at `full` the file
/// does not fit and neither does the store.
fn scan_floor(root: &Path, bytes: u64) -> Result<f64, String> {
    let path = write_scan_floor(root, bytes)?;
    let f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    // SAFETY: the file is private to this process and not written while mapped.
    let map = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| e.to_string())?;
    let t = Instant::now();
    let mut acc = 0u64;
    for w in map.as_chunks::<8>().0 {
        acc = acc.wrapping_add(u64::from_le_bytes(*w));
    }
    let secs = t.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    Ok(bytes as f64 / secs)
}

/// The scan floor's file under `root`, `bytes` long and synced, written
/// where it is not there at that length already. Public so that a bounded
/// run writes it before it enters its bound: the write's dirty pages would
/// be charged to the group, and a bound the floor file cannot fit is a
/// group whose first act is reclaiming what the floor is about to read.
/// The pages stay in the cache, which is the in-core floor's shape; a
/// bounded run drops them itself (`drop_cache`), since the advice refuses
/// off Linux. Returns the path.
pub fn write_scan_floor(root: &Path, bytes: u64) -> Result<PathBuf, String> {
    let path = root.join("scan-floor.dat");
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) != bytes {
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut rng = Rng::new(0xF20);
        let mut chunk = vec![0u8; 1 << 20];
        let mut left = bytes;
        while left > 0 {
            for w in chunk.as_chunks_mut::<8>().0 {
                *w = rng.next().to_le_bytes();
            }
            let n = (left as usize).min(chunk.len());
            f.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
            left -= n as u64;
        }
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
    }
    Ok(path)
}

/// The seek floors' sample: `SEEK_FLOOR_READS` reads of one page each at
/// page-aligned offsets drawn uniformly over the file, the same offsets
/// every call (a fixed seed), so the second floor reads what the first
/// read. A thousand reads is a few milliseconds warm and a second or two
/// cold, and a p99 over a thousand is the tenth-slowest read, which is a
/// latency and not one sample of the host's worst minute.
const SEEK_FLOOR_READS: usize = 1024;
const SEEK_READ_BYTES: usize = 4096;

/// One seek floor's figures: the latency of a read at the median and the
/// ninety-ninth percentile, and the device bytes the thread read over
/// the bytes the floor asked for, which is readahead's share --
/// `posix_fadvise(RANDOM)` asks for one, and the amplification says
/// whether the kernel gave it.
struct SeekFloor {
    p50_us: f64,
    p99_us: f64,
    amplification: f64,
}

/// Drop `path`'s pages from the page cache (`POSIX_FADV_DONTNEED`), so
/// the next read of them is the device's: the advice drops clean pages
/// only, which is why the writer syncs first, and it drops pages mapped
/// by nobody, which is why the scan floor's mapping is gone before the
/// seek floor runs. Public for the bounded run's drop of the floor file
/// it wrote before entering its bound; the advice refuses off Linux, so
/// an unbounded run never calls it.
pub fn drop_cache(path: &Path) -> Result<(), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    fadvise(&f, FADV_DONTNEED).map_err(|e| format!("{}: fadvise DONTNEED: {e}", path.display()))
}

/// The random-read floor: `SEEK_FLOOR_READS` preads of a page at
/// page-aligned offsets of the scan floor's file, each timed on its own,
/// under `POSIX_FADV_RANDOM` so the kernel's readahead stays out of them.
/// Run after `drop_cache` it is the device's own random read, which is
/// what every out-of-core point read costs before the engine adds
/// anything; run again over the same offsets after a second drop it says
/// whether the host serves the second read from a cache the guest cannot
/// see, which is the floor's reason for being two-sided in the gate
/// (`gate::is_seek_floor`).
fn seek_floor(path: &Path) -> Result<SeekFloor, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let pages = len / SEEK_READ_BYTES as u64;
    if pages == 0 {
        return Err(format!(
            "{}: {len} bytes is shorter than one page, so there is nothing to seek in",
            path.display()
        ));
    }
    fadvise(&f, FADV_RANDOM).map_err(|e| format!("{}: fadvise RANDOM: {e}", path.display()))?;
    let mut rng = Rng::new(0x5EEC);
    let mut buf = [0u8; SEEK_READ_BYTES];
    let mut us: Vec<f64> = Vec::with_capacity(SEEK_FLOOR_READS);
    let read0 = thread_read_bytes().ok_or(THREAD_IO_MISSING)?;
    for _ in 0..SEEK_FLOOR_READS {
        let off = (rng.next() % pages) * SEEK_READ_BYTES as u64;
        let t = Instant::now();
        f.read_exact_at(&mut buf, off).map_err(|e| e.to_string())?;
        us.push(t.elapsed().as_secs_f64() * 1e6);
        std::hint::black_box(&buf);
    }
    let read = thread_read_bytes()
        .ok_or(THREAD_IO_MISSING)?
        .saturating_sub(read0);
    us.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |pct: f64| us[((us.len() as f64 * pct / 100.0) as usize).min(us.len() - 1)];
    Ok(SeekFloor {
        p50_us: at(50.0),
        p99_us: at(99.0),
        amplification: read as f64 / (SEEK_FLOOR_READS * SEEK_READ_BYTES) as f64,
    })
}

#[cfg(target_os = "linux")]
const FADV_RANDOM: libc::c_int = libc::POSIX_FADV_RANDOM;
#[cfg(target_os = "linux")]
const FADV_DONTNEED: libc::c_int = libc::POSIX_FADV_DONTNEED;
#[cfg(not(target_os = "linux"))]
const FADV_RANDOM: libc::c_int = 0;
#[cfg(not(target_os = "linux"))]
const FADV_DONTNEED: libc::c_int = 0;

/// `posix_fadvise` over the whole of `f`. Linux only: the seek floors run
/// under a bound, which is a Linux cgroup, and on any other host a floor
/// that could not drop the cache would read the cache and call it the
/// device, so the call refuses there instead.
#[cfg(target_os = "linux")]
fn fadvise(f: &std::fs::File, advice: libc::c_int) -> std::io::Result<()> {
    // SAFETY: advice on a descriptor this process holds open; the
    // call reads nothing and writes nothing of ours.
    let rc = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, advice) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(rc))
    }
}

#[cfg(not(target_os = "linux"))]
fn fadvise(_f: &std::fs::File, _advice: libc::c_int) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "posix_fadvise is not available on this platform",
    ))
}

/// The random-access floor: one dependent load at a time around a
/// permutation of the buffer's cache lines, which is the shape every cost
/// this engine pays in a block table, a piece's index and a memtable
/// chain. Chases per second. It is here because the other two floors are
/// both sequential and neither sees a host whose memory latency has
/// moved: a row where the engine's rates fall and the floors do not is
/// the engine's, and telling those apart is the gate's whole job.
/// A fresh anonymous mapping the page floor touched, held until the row
/// ends. Pages freed are cheap again for whoever allocates next -- that
/// is what the floor measures -- so a floor that freed its pages would
/// hand the pass after it a cheaper load than the pass before had, and a
/// control that moves what it controls is not one.
struct FreshPages {
    ptr: *mut u8,
    len: usize,
}

impl Drop for FreshPages {
    fn drop(&mut self) {
        // SAFETY: the mapping `page_floor` made, unmapped once.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// The pool to prime for a rung of `size` keys (`PRIME_BYTES_PER_KEY`),
/// and under a bound of `bound` bytes never more than an eighth of it:
/// the prime's pages are charged to the group like every other, and a
/// prime the size of the bound would reclaim the page cache the floors
/// and the store before it had warmed, which is what a bounded row
/// measures and not what the prime is for.
fn prime_bytes_for(size: u64, bound: Option<u64>) -> u64 {
    let pool = (size * PRIME_BYTES_PER_KEY).clamp(PRIME_MIN_BYTES, PRIME_MAX_BYTES);
    match bound {
        Some(limit) => pool.min(limit / 8),
        None => pool,
    }
}

/// Touch and free a fresh anonymous mapping of `bytes` before a store
/// opens, so the load that follows draws its fresh pages -- its tables,
/// its files' page cache -- from pages this guest still backs. A page
/// nobody has touched since it was freed costs about a microsecond where
/// the guest backs it and five to fifteen where its host has to, and
/// which price a load gets depends on how much the pass before it freed
/// and how long ago: two rows read every arm's load at the top rung at
/// a third to a half of the rows before, with every control in band,
/// because the arm before each had left a pool smaller than its
/// footprint. The pages stay fresh to the allocator -- the kernel zeroes
/// them on the load's first touch as before -- and the host's price is
/// paid here, once, where it is measured and not timed. Returns the
/// microseconds a page the touch cost, which the pass line prints: the
/// host's price for a load-sized footprint, read beside the page floor's
/// price inside the pool.
fn prime_fresh_pool(bytes: u64) -> Result<f64, String> {
    // SAFETY: a plain query of the system's page size.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page = usize::try_from(page).map_err(|_| "prime: no page size".to_string())?;
    let len = usize::try_from(bytes).map_err(|_| "prime: too large".to_string())?;
    let len = len.div_ceil(page) * page;
    // SAFETY: an anonymous private mapping of `len` bytes, checked below,
    // unmapped at the end of this function.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(format!(
            "prime: mmap of {len} bytes failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let t = Instant::now();
    let populated = populate_write(ptr, len);
    if !populated {
        let p: *mut u8 = ptr.cast();
        for i in (0..len).step_by(page) {
            // SAFETY: inside the mapping; volatile so the touch is a
            // store the compiler keeps.
            unsafe { std::ptr::write_volatile(p.add(i), 1) };
        }
    }
    let secs = t.elapsed().as_secs_f64();
    // SAFETY: the mapping made above, unmapped once.
    unsafe { libc::munmap(ptr, len) };
    Ok(secs * 1e6 / (len / page) as f64)
}

/// Populate a fresh mapping's pages in one call where the kernel offers
/// it (`MADV_POPULATE_WRITE`, Linux 5.14): half the price of a fault a
/// page. False where it is not offered, and the caller touches.
#[cfg(target_os = "linux")]
fn populate_write(ptr: *mut libc::c_void, len: usize) -> bool {
    // Not in every libc crate the suite builds against; the value is the
    // kernel's (`include/uapi/asm-generic/mman-common.h`).
    const MADV_POPULATE_WRITE: libc::c_int = 23;
    // SAFETY: advice over a mapping this process holds.
    unsafe { libc::madvise(ptr, len, MADV_POPULATE_WRITE) == 0 }
}

#[cfg(not(target_os = "linux"))]
fn populate_write(_ptr: *mut libc::c_void, _len: usize) -> bool {
    false
}

/// One first touch per page of a fresh anonymous mapping, timed: the
/// price this host charges for a page nobody has touched since it was
/// freed. Where the guest still backs the page it is the kernel's zeroing,
/// about a microsecond; where the guest's balloon has handed the block to
/// the host it is the host's fault and allocation, five to fifteen. Every
/// write path's fresh memory pays it -- an engine's tables and arenas,
/// the page cache's pages behind a `write` -- and which price a load gets
/// depends on how much was freed before it and how long ago, so a row's
/// loads can read at a third of the rows before with every comparator in
/// band and the other three floors within their window, which is the row
/// this floor came from. It is sampled through the row, not only at its
/// start, because the price moves within one. Pages a second, and the
/// mapping for the caller to hold.
fn page_floor() -> Result<(f64, FreshPages), String> {
    // SAFETY: a plain query of the system's page size.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page = usize::try_from(page).map_err(|_| "page floor: no page size".to_string())?;
    let len = PAGE_FLOOR_PAGES * page;
    // SAFETY: an anonymous private mapping of `len` bytes, checked below.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(format!(
            "page floor: mmap of {len} bytes failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let ptr: *mut u8 = ptr.cast();
    let t = Instant::now();
    for i in 0..PAGE_FLOOR_PAGES {
        // SAFETY: `i * page` is inside the mapping; volatile so the touch
        // is a store the compiler keeps.
        unsafe { std::ptr::write_volatile(ptr.add(i * page), 1) };
    }
    let secs = t.elapsed().as_secs_f64();
    Ok((PAGE_FLOOR_PAGES as f64 / secs, FreshPages { ptr, len }))
}

fn mem_floor() -> f64 {
    let lines = MEM_FLOOR_BYTES / 64;
    // A permutation of the lines as one cycle, so no prefetcher can
    // guess the next address and every load waits for the one before.
    let mut next: Vec<u32> = (0..lines as u32).collect();
    let mut rng = Rng::new(0xF30);
    for i in (1..lines).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        next.swap(i, j);
    }
    // `next[i]` holds a permutation; walk it as a cycle by writing each
    // line's successor into the line itself.
    let mut buf: Vec<u32> = vec![0; lines * 16];
    let mut at = 0usize;
    for _ in 0..lines {
        let to = next[at] as usize;
        buf[at * 16] = to as u32;
        at = to;
    }
    let t = Instant::now();
    let mut at = 0usize;
    for _ in 0..MEM_FLOOR_CHASES {
        at = buf[at * 16] as usize;
    }
    let secs = t.elapsed().as_secs_f64();
    std::hint::black_box(at);
    MEM_FLOOR_CHASES as f64 / secs
}

/// LMDB needs its map sized up front. Three times the raw payload, at least
/// 8 GB: sparse until used, so generosity costs nothing.
fn lmdb_map_gb(size: u64, value_size: usize) -> usize {
    let raw = size as f64 * (KEY_SIZE + value_size) as f64;
    ((raw * 3.0 / 1073741824.0).ceil() as usize).max(8)
}

/// Hand back to the system what the store just dropped had freed, so the
/// next one opens in the memory a fresh process would give it. glibc keeps
/// what a thread frees in that thread's arena and returns only the top of
/// a heap, so every store a run had opened stayed resident after its drop:
/// at the full run's 10M rung the arenas held 13 GB taken from the system
/// with 1.8 GB of it in use, the store's own files had the page cache that
/// was left, and from its third rep the rung read at a tenth to a twentieth
/// of the reps before, faulting on every page. `malloc_trim(0)` took that
/// process from 12.9 GB resident to 2.1 GB.
fn release_freed_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: takes and returns plain integers; releasing free pages is
    // what it is for, and it takes the allocator's own locks.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// This process's anonymous resident bytes (Linux `RssAnon`): the memory
/// a bound charges that the page cache cannot give back. `resident_bytes`
/// counts the mapped segments' pages too.
fn anon_resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("RssAnon:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// This process's resident bytes, where the platform says (Linux).
fn resident_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf reads a constant of the system.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(pages * u64::try_from(page).ok()?)
}

/// The top rung for `full` on this machine: the store at least 1.5x memory.
/// Free bytes on the filesystem holding `dir`, or None if it cannot be read.
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut c = Vec::with_capacity(dir.as_os_str().as_bytes().len() + 1);
    c.extend_from_slice(dir.as_os_str().as_bytes());
    c.push(0);
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr().cast(), &mut st) } != 0 {
        return None;
    }
    // Through u128 and `from` rather than `as`: the widths of these fields
    // differ between the Linux that lints this and the macOS that runs it,
    // so a cast that is redundant on one platform is narrowing on the other.
    u64::try_from(u128::from(st.f_bavail) * u128::from(st.f_frsize)).ok()
}

pub fn full_top(mem_total_kb: u64, value_size: usize) -> u64 {
    let need = mem_total_kb as f64 * 1024.0 * 1.5;
    (need / (KEY_SIZE + value_size) as f64).ceil() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held between the one test that counts what `release_freed_memory`
    /// returns and the tests that run a pass, which call it: the release is
    /// the process's, so a pass's `malloc_trim` in another test's thread
    /// takes the freed blocks the first is about to count, and the test
    /// failed -- on blocks that "were not resident to begin with", or on a
    /// release that returned less than most of them -- in runs of the whole
    /// binary at the default thread count once a test that runs a pass was
    /// beside it. A pass takes this shared, and the measurement
    /// exclusively. The guard is taken through `into_inner`, so a test that
    /// fails holding it fails nobody else.
    static TRIM: std::sync::RwLock<()> = std::sync::RwLock::new(());

    /// The shape that filled the full run's memory: a thread allocates
    /// small blocks in its own arena, frees them, and exits, with one block
    /// allocated last still live so the arena cannot give its heap back from
    /// the top. The freed memory stays resident until the runner releases
    /// it -- the first assertion is that the test reached that case at all,
    /// the second that the release returns most of it.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn a_dropped_stores_freed_memory_is_returned_before_the_next_pass() {
        let _alone = TRIM.write().unwrap_or_else(|e| e.into_inner());
        const BLOCKS: usize = 3_000_000;
        const SIZE: usize = 100;
        let held = BLOCKS * SIZE;
        let pin = std::thread::spawn(|| {
            let mut v: Vec<Box<[u8; SIZE]>> = Vec::with_capacity(BLOCKS);
            for i in 0..BLOCKS {
                v.push(Box::new([i as u8; SIZE]));
            }
            let pin = Box::new([1u8; SIZE]);
            std::hint::black_box(&v);
            drop(v);
            pin
        })
        .join()
        .unwrap();
        let before = resident_bytes().unwrap();
        release_freed_memory();
        let after = resident_bytes().unwrap();
        assert!(
            before >= held as u64,
            "the freed blocks were not resident to begin with: {before} bytes, {held} allocated"
        );
        assert!(
            before - after >= held as u64 / 2,
            "released {} of the {held} bytes freed ({before} -> {after})",
            before.saturating_sub(after)
        );
        drop(pin);
    }

    /// Holm's step-down against hand-worked cases: it stops at the first
    /// failure even when a later p would clear its own, looser bound, and
    /// a family of one is the plain test.
    #[test]
    fn holm_holds_what_the_family_supports_and_stops_at_the_first_miss() {
        // Sorted: 0.001 against 0.05/4, 0.01 against 0.05/3, 0.03 against
        // 0.05/2 fails, so 0.04 is not reached though it clears 0.05/1.
        assert_eq!(
            holm(&[0.03, 0.001, 0.04, 0.01], 0.05),
            vec![false, true, false, true]
        );
        assert_eq!(holm(&[0.04], 0.05), vec![true]);
        assert_eq!(holm(&[0.06], 0.05), vec![false]);
        assert!(holm(&[], 0.05).is_empty());
        // Thirty-six quantities at the sign test's floor for six pairs,
        // which is what an arm against itself marks once a run: none holds.
        let p6 = sign_p(6, 6);
        assert!((p6 - 0.03125).abs() < 1e-12);
        let mut ps = vec![0.5; 36];
        ps[7] = p6;
        assert!(holm(&ps, 0.05).iter().all(|h| !h));
    }

    /// The guard in `Samples::push` is the whole reason a quantity cannot be
    /// recorded without a direction, so it is held to firing. Written after
    /// `store_bytes_per_byte` was added and the four places it had to be
    /// named were found by grep: the gate is the one that fails a run rather
    /// than a build, and it fails it after the measuring is done.
    #[test]
    #[should_panic(expected = "has no direction")]
    fn a_quantity_the_gate_cannot_judge_is_refused_where_it_is_recorded() {
        let mut s = Samples {
            map: BTreeMap::new(),
        };
        s.push(
            "load",
            Some(10),
            "supdb",
            Guarantee::Durable,
            ("bytes_per_fortnight", "B/f"),
            1.0,
        );
    }

    #[test]
    fn every_quantity_the_runner_records_has_a_direction() {
        for q in [
            "ops_per_s",
            "device_bytes_per_byte",
            "bytes_on_disk_per_byte",
            "reads_per_s",
            "p99_us",
            "entries_per_s",
            "bytes_per_s",
        ] {
            assert!(gate::higher_is_better(q).is_some(), "{q}");
        }
    }

    #[test]
    fn shares_sum_to_the_work_and_differ_by_at_most_one() {
        for (ops, threads) in [
            (10_000u64, 4usize),
            (10_001, 4),
            (7, 4),
            (3, 4),
            (1, 2),
            (0, 2),
        ] {
            let shares: Vec<u64> = (0..threads).map(|i| share(ops, threads, i)).collect();
            assert_eq!(
                shares.iter().sum::<u64>(),
                ops,
                "{ops} over {threads}: {shares:?}"
            );
            let lo = shares.iter().min().unwrap();
            let hi = shares.iter().max().unwrap();
            assert!(hi - lo <= 1, "{ops} over {threads}: {shares:?}");
        }
    }

    /// Two, doubling, and one fewer than the cores; none on two cores.
    #[test]
    fn the_thread_counts_stop_one_short_of_the_cores() {
        assert_eq!(thread_counts(4), vec![2, 3]);
        assert_eq!(thread_counts(8), vec![2, 4, 7]);
        assert_eq!(thread_counts(16), vec![2, 4, 8, 15]);
        assert_eq!(thread_counts(3), vec![2]);
        assert_eq!(thread_counts(2), Vec::<usize>::new());
        assert_eq!(thread_counts(1), Vec::<usize>::new());
    }

    #[test]
    fn a_threaded_quantity_names_its_count_and_nothing_else_does() {
        assert_eq!(threaded_quantity("reads_per_s", 4), "reads_per_s_4t");
        assert_eq!(threads_of("reads_per_s_4t"), Some(4));
        assert_eq!(threads_of("entries_per_s_2t"), Some(2));
        assert_eq!(threads_of("reads_per_s"), None);
        assert_eq!(threads_of("entries_per_s"), None);
        assert_eq!(threads_of("p99_us"), None);
        assert_eq!(threads_of("ops_per_s_2t"), None);
    }

    /// Every engine's threaded readers read the store the writer loaded:
    /// each key's one value, every entry of every scan, on two threads.
    /// What it pins is the handle each adapter hands a thread -- supdb's
    /// under `Latest` after the load's last commit, on the durable arm
    /// and on the ingest arm whose keys took the ordered path, and on the
    /// arm whose handles read the partitions through their ordered
    /// indexes, where a key the index placed wrongly would read short and
    /// whose handles' counts must reach the store's counters from the
    /// threads that drop them; LMDB's transaction begun on the thread
    /// after the writes; RocksDB's
    /// shared handle -- since a reader over the wrong state would post a
    /// throughput like any other, and the runner's own check would be
    /// the first to say so, after the load it had paid for.
    #[test]
    fn threaded_readers_read_the_loaded_store() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-threaded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plan = Plan::new(Scale::Quick, vec![], 10_000, 4);
        let payload = Payload::new(plan.value_size, plan.compress, 0xE1);
        for arm in [
            "supdb",
            "supdb-ingest",
            "supdb-nocacheordread",
            "supdb-nocachezpages",
            "lmdb",
            "rocksdb-tuned",
        ] {
            // The arm that reads from the second form is given a store with
            // a partition over the floor, which has a form file to read from.
            let size = if arm == "supdb-nocachezpages" {
                30_000u64
            } else {
                2_000
            };
            let dir = root.join(arm);
            std::fs::create_dir_all(&dir).unwrap();
            let mut e = engines::open(arm, &dir, 1).unwrap();
            load(e.as_mut(), size, &plan, &payload, |i| i).unwrap();
            let got = threaded(e.as_ref(), "read", 2, size, size, 1, Op::Get, true).unwrap();
            assert_eq!(
                got.bytes,
                size * plan.value_size as u64,
                "{arm}: point reads"
            );
            assert!(got.secs > 0.0, "{arm}: a pass took no time");
            // In core, the threads' own device reads and major faults
            // over a store just written are nothing or next to it.
            assert!(got.own.0 < 1 << 20, "{arm}: {} device bytes", got.own.0);
            // The arm that reads through the ordered index counts what its
            // threads' handles answered, handed to the store as each
            // thread drops its handle at the end of the pass, so the
            // counters read after a pass carry the whole of it. The other
            // supdb arms report none, and the comparators no counters.
            let handles_read = e
                .counters()
                .iter()
                .find(|(n, _)| *n == "ord_point_reads_threads")
                .map(|(_, v)| *v);
            match arm {
                "supdb-nocacheordread" => assert_eq!(
                    handles_read,
                    Some(size as f64),
                    "{arm}: the threads' reads reach the counters"
                ),
                "supdb" | "supdb-ingest" | "supdb-nocachezpages" => {
                    assert_eq!(handles_read, Some(0.0), "{arm}")
                }
                _ => assert_eq!(handles_read, None, "{arm}"),
            }
            // The arm that forces the second form reads the threads' keys
            // from the form files its load's sync made, and the count of
            // them is in the store's after the threads' handles are gone.
            let from_form_files = e
                .counters()
                .iter()
                .find(|(n, _)| *n == "zpage_reads")
                .map(|(_, v)| *v);
            match arm {
                "supdb-nocachezpages" => assert_eq!(
                    from_form_files,
                    Some(size as f64),
                    "{arm}: the threads' reads are answered from the form files"
                ),
                "supdb" | "supdb-ingest" | "supdb-nocacheordread" => {
                    assert_eq!(from_form_files, Some(0.0), "{arm}")
                }
                _ => assert_eq!(from_form_files, None, "{arm}"),
            }
            let scans = size / plan.scan_len as u64;
            let got = threaded(
                e.as_ref(),
                "scan",
                2,
                scans,
                size - plan.scan_len as u64,
                2,
                Op::Range(plan.scan_len),
                false,
            )
            .unwrap();
            assert_eq!(
                got.bytes,
                scans * (plan.scan_len * plan.value_size) as u64,
                "{arm}: scans"
            );
            assert_eq!(got.own, (0, 0), "{arm}: counted without being asked");
            drop(e);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The refusal of a forced arm whose second form is not in use, held to
    /// inputs that are known red -- nothing covered, a partition left raw
    /// for what it holds beside the covered ones, no read answered from a
    /// form file after the reads, an arm that does not force the form and
    /// has read from one -- and to the ones that are green: every
    /// partition covered, a sliver under the floor beside the covered
    /// ones, an arm that reads from none, and an engine with no such form.
    #[test]
    fn a_forced_form_that_is_not_in_use_is_refused() {
        // A store settled has a file for each of its form files.
        let z = |forced, reads, partitions, form_files, small| {
            Some(ZState {
                forced,
                reads,
                partitions,
                form_files,
                small,
                files: form_files,
                ..ZState::default()
            })
        };
        let run = |stage, state| zpages_refusal("supdb-nocachezpages", 1_000, stage, state);
        assert!(run(ZStage::Loaded, z(true, 0, 18, 18, 0)).is_ok());
        assert!(
            run(ZStage::Loaded, z(true, 0, 18, 17, 1)).is_ok(),
            "a sliver under the floor is the engine's own decision"
        );
        assert!(run(ZStage::Read, z(true, 10, 18, 18, 0)).is_ok());
        assert!(run(ZStage::Read, z(false, 0, 18, 0, 0)).is_ok());
        assert!(run(ZStage::Loaded, z(false, 0, 18, 0, 0)).is_ok());
        assert!(run(ZStage::Loaded, None).is_ok(), "an engine with no form");
        assert!(run(ZStage::Read, None).is_ok());
        // The second store, after the shuffled load, is held to the first's
        // coverage rules.
        assert!(run(ZStage::Shuffled, z(true, 0, 12, 12, 0)).is_ok());
        assert!(run(ZStage::Shuffled, z(true, 0, 12, 11, 1)).is_ok());
        assert!(run(ZStage::Shuffled, z(false, 0, 12, 0, 0)).is_ok());

        let red = |stage, state| run(stage, state).unwrap_err();
        let none = red(ZStage::Loaded, z(true, 0, 18, 0, 0));
        assert!(none.contains("none of 18 partitions"), "{none}");
        assert!(none.contains("after the load"), "{none}");
        let under = red(ZStage::Loaded, z(true, 0, 1, 0, 1));
        assert!(under.contains("raise the key count"), "{under}");
        let part = red(ZStage::Loaded, z(true, 0, 18, 17, 0));
        assert!(part.contains("17 of 18"), "{part}");
        let part = red(ZStage::Loaded, z(true, 0, 18, 16, 1));
        assert!(part.contains("16 of 18"), "{part}");
        let silent = red(ZStage::Read, z(true, 0, 18, 18, 0));
        assert!(silent.contains("no point read"), "{silent}");
        let leaked = red(ZStage::Read, z(false, 3, 18, 0, 0));
        assert!(leaked.contains("3 point reads"), "{leaked}");
        assert!(none.starts_with("supdb-nocachezpages at 1000: "), "{none}");
        // The shuffled store, by the same rules and in its own words.
        let none = red(ZStage::Shuffled, z(true, 0, 12, 0, 0));
        assert!(
            none.contains("none of 12 partitions") && none.contains("after the shuffled load"),
            "{none}"
        );
        let part = red(ZStage::Shuffled, z(true, 0, 12, 9, 1));
        assert!(
            part.contains("9 of 12") && part.contains("after the shuffled load"),
            "{part}"
        );
        // The directory: one file for each form file and no other file named
        // for the form, at both loads. A leftover is disk the row counts.
        for stage in [ZStage::Loaded, ZStage::Shuffled] {
            let mut left = z(true, 0, 18, 18, 0).unwrap();
            left.files = 19;
            let why = red(stage, Some(left));
            assert!(
                why.contains("19 form files") && why.contains("18 live form files"),
                "{why}"
            );
            let mut temp = z(true, 0, 18, 18, 0).unwrap();
            temp.stray = 1;
            let why = red(stage, Some(temp));
            assert!(why.contains("1 other files"), "{why}");
            let mut lost = z(true, 0, 18, 18, 0).unwrap();
            lost.files = 17;
            assert!(red(stage, Some(lost)).contains("17 form files"));
        }
        // An unforced arm has none, and its directory is nobody's to ask.
        let mut unforced = z(false, 0, 18, 0, 0).unwrap();
        unforced.files = 3;
        assert!(run(ZStage::Loaded, Some(unforced)).is_ok());
    }

    /// A form file out of service and reads that went to the raw form are what
    /// the arm's pre-registration stops a pairing for, and a pass that has
    /// served one read from a form file has not earned the form's name with
    /// them: the share is the registered hundredth of the reads, counted
    /// with the raw ones, and it is held at its edge.
    #[test]
    fn a_forced_form_with_a_form_file_out_of_service_or_reads_gone_raw_is_refused() {
        let covered = ZState {
            forced: true,
            partitions: 18,
            form_files: 18,
            files: 18,
            ..ZState::default()
        };
        let run = |stage, z: ZState| zpages_refusal("supdb-nocachezpages", 1_000, stage, Some(z));
        // At the edge: a hundredth of the reads exactly is the allowance.
        let reads = |reads, raw| ZState {
            reads,
            raw,
            ..covered
        };
        assert!(run(ZStage::Read, reads(990, 10)).is_ok());
        assert!(run(ZStage::Read, reads(1_000, 0)).is_ok());
        let over = run(ZStage::Read, reads(989, 11)).unwrap_err();
        assert!(over.contains("11 of 1000"), "{over}");
        let mostly_raw = run(ZStage::Read, reads(1, 99)).unwrap_err();
        assert!(mostly_raw.contains("99 of 100"), "{mostly_raw}");
        // The share is of the forced arm's reads: an unforced arm's raw reads
        // are all of them.
        let unforced = ZState {
            forced: false,
            raw: 1_000,
            ..ZState::default()
        };
        assert!(run(ZStage::Read, unforced).is_ok());
        // A form file out of service is refused at every stage, however few
        // reads it cost.
        for stage in [ZStage::Loaded, ZStage::Read, ZStage::Shuffled] {
            let out = ZState {
                damaged: 1,
                reads: 1_000_000,
                ..covered
            };
            let why = run(stage, out).unwrap_err();
            assert!(
                why.contains("1 of 18 form files are out of service") && why.contains(stage.when()),
                "{stage:?}: {why}"
            );
            let sound = ZState {
                reads: 10,
                ..covered
            };
            assert!(run(stage, sound).is_ok(), "{stage:?}");
        }
    }

    /// An engine that reads nothing and says what it is told of its second
    /// form: the pass's phases asked of something that can lie, since a real
    /// engine's form is never in the state a refusal is for.
    struct Telling {
        z: Option<ZState>,
        gets: u64,
    }

    impl Engine for Telling {
        fn name(&self) -> &'static str {
            "telling"
        }
        fn features(&self) -> engines::Features {
            engines::Features {
                durable_commit: true,
                transactions: false,
                checksums: false,
                reopen_for_write: false,
                read_your_writes: true,
                ordered_scan: true,
            }
        }
        fn write_batch(&mut self, _: &[(&[u8], &[u8])]) -> engines::Res<()> {
            Ok(())
        }
        fn get(&mut self, _: &[u8]) -> engines::Res<usize> {
            self.gets += 1;
            Ok(0)
        }
        fn range(&mut self, _: &[u8], _: usize) -> engines::Res<usize> {
            Ok(0)
        }
        fn sync(&mut self) -> engines::Res<()> {
            Ok(())
        }
        fn size_bytes(&self) -> u64 {
            0
        }
        fn thread_reader(&self) -> engines::Res<engines::ReaderOpener> {
            Err("no reader".into())
        }
        fn zpages_state(&self) -> Option<ZState> {
            self.z
        }
    }

    /// The point-read phase asks, after the reads, whether a forced arm read
    /// through its form -- the call site of the refusal that no real arm
    /// reaches red, because a real forced arm reads through its form files.
    #[test]
    fn the_read_phase_asks_whether_a_forced_arm_read_through_its_form() {
        let mut counting = Counting::new(None, Path::new(".")).unwrap();
        let covered = ZState {
            forced: true,
            reads: 50,
            partitions: 3,
            form_files: 3,
            ..ZState::default()
        };
        let mut phase = |z: Option<ZState>| {
            let mut e = Telling { z, gets: 0 };
            let mut h = Hist::new();
            let r = read_phase(
                &mut e,
                "supdb-nocachezpages",
                1_000,
                50,
                &mut h,
                &mut counting,
            );
            // The reads were made whatever the answer: it is asked after.
            assert_eq!(e.gets, 50);
            r
        };
        assert!(phase(Some(covered)).unwrap() >= 0.0);
        assert!(
            phase(None).is_ok(),
            "an engine with no form is asked nothing"
        );
        let silent = phase(Some(ZState {
            reads: 0,
            ..covered
        }))
        .unwrap_err();
        assert!(silent.contains("no point read"), "{silent}");
        let raw = phase(Some(ZState {
            raw: 500,
            ..covered
        }))
        .unwrap_err();
        assert!(raw.contains("went to the raw form"), "{raw}");
        let out = phase(Some(ZState {
            damaged: 1,
            ..covered
        }))
        .unwrap_err();
        assert!(out.contains("out of service"), "{out}");
        let leaked = phase(Some(ZState {
            forced: false,
            ..covered
        }))
        .unwrap_err();
        assert!(leaked.contains("not forced"), "{leaked}");
    }

    /// The shuffled load asks, after it, whether a forced arm's form covers
    /// the pass's second store -- the call site of a refusal that a real arm
    /// reaches red only when its merges outlast the settle's rounds.
    #[test]
    fn the_shuffled_load_asks_whether_a_forced_arm_covers_the_second_store() {
        let mut counting = Counting::new(None, Path::new(".")).unwrap();
        let plan = Plan::new(Scale::Quick, vec![], 1_000, 4);
        let payload = Payload::new(plan.value_size, plan.compress, 0xE1);
        let mut phase = |z: Option<ZState>| {
            let mut e = Telling { z, gets: 0 };
            shuffled_load(
                &mut e,
                "supdb-nocachezpages",
                1_000,
                &plan,
                &payload,
                &mut counting,
            )
        };
        let covered = ZState {
            forced: true,
            partitions: 12,
            form_files: 12,
            files: 12,
            ..ZState::default()
        };
        assert!(phase(Some(covered)).unwrap() >= 0.0);
        assert!(
            phase(None).is_ok(),
            "an engine with no form is asked nothing"
        );
        let part = phase(Some(ZState {
            form_files: 9,
            files: 9,
            ..covered
        }))
        .unwrap_err();
        assert!(
            part.contains("9 of 12") && part.contains("after the shuffled load"),
            "{part}"
        );
        let stray = phase(Some(ZState {
            stray: 1,
            ..covered
        }))
        .unwrap_err();
        assert!(stray.contains("1 other files"), "{stray}");
        let out = phase(Some(ZState {
            damaged: 1,
            ..covered
        }))
        .unwrap_err();
        assert!(out.contains("out of service"), "{out}");
    }

    /// The pass itself refuses a forced arm over a store with no form file to
    /// read, and runs one that has: the same arm at a size under the floor
    /// and over it, and the unforced arm beside it, which reads from none.
    #[test]
    fn the_pass_refuses_a_forced_arm_that_covers_nothing_and_runs_one_that_does() {
        // A pass releases freed memory to the system: not while the test that
        // counts what the release returns is counting.
        let _pass = TRIM.read().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("supdb-bench-zpass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut plan = Plan::new(Scale::Quick, vec![], 30_000, 4);
        plan.passes = Passes::parse("load,read").unwrap();
        let payload = Payload::new(plan.value_size, plan.compress, 0xE1);
        let small = one_pass(
            "supdb-nocachezpages",
            &root.join("small"),
            2_000,
            1,
            &plan,
            &payload,
        );
        let why = small
            .err()
            .expect("the form forced over a store under the floor");
        assert!(
            why.contains("supdb-nocachezpages at 2000") && why.contains("none of"),
            "{why}"
        );
        let count = |p: &OnePass, name: &str| {
            p.counters
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("no counter {name}"))
                .1
        };
        let forced = one_pass(
            "supdb-nocachezpages",
            &root.join("forced"),
            30_000,
            1,
            &plan,
            &payload,
        )
        .unwrap();
        assert_eq!(count(&forced, "zpage_forced"), 2.0, "both stores forced");
        assert_eq!(count(&forced, "zpage_reads"), 30_000.0, "every read");
        assert_eq!(count(&forced, "zpage_rawserves"), 0.0);
        assert!(
            count(&forced, "zpage_form files_at_end") >= 2.0,
            "one a store"
        );
        let base = one_pass(
            "supdb-nocache",
            &root.join("base"),
            30_000,
            1,
            &plan,
            &payload,
        )
        .unwrap();
        assert_eq!(count(&base, "zpage_forced"), 0.0);
        assert_eq!(count(&base, "zpage_reads"), 0.0);
        assert_eq!(count(&base, "zpage_form files_at_end"), 0.0);
        // The counters are in the paired quantities `ab` prints.
        let flat = forced.flat();
        assert!(flat.iter().any(|(n, _)| n == "count zpage_reads"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The divisor: thirty at thirty million keys gives a million point
    /// reads, ten thousand scans, a sixth of a million mix operations
    /// and ten thousand lag scans; at ten thousand keys every divided
    /// count sits on the floor of a thousand, or at the rung where the
    /// rung is smaller; and at the two unbounded scales nothing is
    /// divided at all.
    #[test]
    fn the_divisor_floors_at_a_thousand_and_divides_nothing_at_quick_and_full() {
        let ooc = Plan::new(Scale::Ooc, vec![], 30_000_000, 4);
        assert_eq!(ooc.ops_div, OOC_OPS_DIV);
        let size = 30_000_000u64;
        assert_eq!(ooc.divided(size), 1_000_000);
        assert_eq!(ooc.divided(size / ooc.scan_len as u64), 10_000);
        assert_eq!(ooc.divided(ycsb_ops(size)), 166_667);
        // The lag scans are the scan pass's count.
        assert_eq!(ooc.divided((size / ooc.scan_len as u64).max(1)), 10_000);
        // The floor at ten thousand keys: the reads at a thousand, the
        // scans at the rung's hundred, the mixes at their own thousand.
        assert_eq!(ooc.divided(10_000), 1_000);
        assert_eq!(ooc.divided(10_000 / ooc.scan_len as u64), 100);
        assert_eq!(ooc.divided(ycsb_ops(10_000)), 1_000);
        assert_eq!(ooc.divided(0), 0);
        for scale in [Scale::Quick, Scale::Full] {
            let p = Plan::new(scale, vec![], 300_000, 4);
            assert_eq!(p.ops_div, 1);
            for n in [1u64, 999, 1_000, 300_000, 30_000_000] {
                assert_eq!(p.divided(n), n, "{scale:?} {n}");
            }
        }
    }

    /// The prime under a bound never exceeds an eighth of it, and without
    /// one is the clamp it always was.
    #[test]
    fn the_prime_under_a_bound_is_at_most_an_eighth_of_it() {
        for size in [10_000u64, 300_000, 3_000_000, 10_000_000] {
            let free = prime_bytes_for(size, None);
            assert!(
                (PRIME_MIN_BYTES..=PRIME_MAX_BYTES).contains(&free),
                "{size}"
            );
            for limit in [MIN_BOUND_BYTES, 1536 << 20, 2048 << 20, 16 << 30] {
                let bounded = prime_bytes_for(size, Some(limit));
                assert!(bounded <= limit / 8, "{size} under {limit}: {bounded}");
                assert!(
                    bounded <= free,
                    "{size} under {limit}: {bounded} over {free}"
                );
                assert_eq!(bounded, free.min(limit / 8));
            }
        }
        assert_eq!(prime_bytes_for(10_000, None), PRIME_MIN_BYTES);
        assert_eq!(prime_bytes_for(10_000_000, None), PRIME_MAX_BYTES);
    }

    /// The pass names parse to the phases they name, an unknown name is
    /// refused naming it, and an empty list is refused too.
    #[test]
    fn the_passes_parse_by_name_and_refuse_a_name_they_lack() {
        let all = Passes::parse(&Passes::NAMES.join(",")).unwrap();
        assert_eq!(all, Passes::all());
        let some = Passes::parse("read, scan-threaded,lag").unwrap();
        assert!(some.read && some.scan_threaded && some.lag);
        assert!(!some.load && !some.read_threaded && !some.scan && !some.mixes);
        assert!(!some.scan_mixed);
        let err = Passes::parse("read,scna").unwrap_err();
        assert!(err.contains("scna"), "{err}");
        assert!(Passes::parse("").is_err());
        assert!(Passes::parse(" , ").is_err());
    }

    /// The count names: each of the five under no suffix, a thread
    /// suffix or a lag suffix, and nothing else.
    #[test]
    fn a_count_is_named_under_every_suffix_a_rate_carries() {
        for (base, unit) in COUNTS {
            assert_eq!(count_of(base), Some(base));
            assert_eq!(count_of(&format!("{base}_2t")), Some(base));
            assert_eq!(count_of(&format!("{base}_lag0pct")), Some(base));
            assert_eq!(count_of(&format!("{base}_lagpct")), None);
            assert_eq!(count_of(&format!("{base}_t")), None);
            assert_eq!(count_of(&format!("{base}2t")), None);
            assert_eq!(count_unit(base), unit);
        }
        assert_eq!(count_of("reads_per_s"), None);
        assert_eq!(count_of("read_bytes"), None);
        assert_eq!(count_unit("not_a_count"), "?");
    }

    /// The thread's stat line is parsed past the comm, which may hold
    /// spaces and parentheses of its own.
    #[test]
    fn the_major_faults_are_the_tenth_field_past_the_comm() {
        let stat = "1234 (a (weird) comm) S 1 2 3 4 5 6 8 0 42 0 0 0";
        assert_eq!(stat_majflt(stat), Some(42));
        assert_eq!(stat_minflt(stat), 8);
        assert_eq!(stat_majflt("garbage"), None);
        assert!(majflt_self().is_some_and(|n| n < u64::MAX));
    }

    /// The device's stat line: read I/Os first, read ticks fourth.
    #[test]
    fn the_devices_read_ios_and_ticks_are_its_first_and_fourth_fields() {
        let dir = std::env::temp_dir().join(format!("supdb-bench-devstat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("stat");
        std::fs::write(&p, "  100 2 3000 400   5 6 7 8 0 9 10 0 0 0 0 0 0\n").unwrap();
        assert_eq!(dev_read_stat(&p), Some((100, 400)));
        std::fs::write(&p, "100 2 3000\n").unwrap();
        assert_eq!(dev_read_stat(&p), None);
        assert_eq!(dev_read_stat(&dir.join("none")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scan floor's file is written once at its length, synced, and
    /// a seek over it reads the same offsets twice: the two floors answer
    /// the same question of the same pages, and a page-aligned read of a
    /// page reads at most the file.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_seek_floor_reads_the_same_offsets_each_time() {
        let root = std::env::temp_dir().join(format!("supdb-bench-seek-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let bytes = 64 * SEEK_READ_BYTES as u64;
        let path = write_scan_floor(&root, bytes).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), bytes);
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        // A second call at the same length rewrites nothing.
        write_scan_floor(&root, bytes).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        drop_cache(&path).unwrap();
        let a = seek_floor(&path).unwrap();
        drop_cache(&path).unwrap();
        let b = seek_floor(&path).unwrap();
        for f in [&a, &b] {
            assert!(f.p50_us >= 0.0 && f.p99_us >= f.p50_us);
            assert!(f.amplification >= 0.0);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
