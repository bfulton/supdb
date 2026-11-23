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
//! A seed keeps a record of what it did (`Log`): every write with its step,
//! whether a commit took it into the model or a crash dropped it, and every
//! commit, seal, flush, settle, burst, check, close, crash and reopen, each
//! with both stores' counts of seals, merges, promotions, manifests, runs
//! and levels at that moment. A wrong answer for a key prints, beside the
//! failure line, the values got and wanted in hex, the earlier write each
//! wrong value equals -- an older value of the key, another key's, or none
//! -- the store's sources for the key (`Reader::debug_key`) in full, and the
//! key's history from that record (`DETAIL`, `SOURCES` and `HISTORY` lines).
//! A failure that comes once in a long run is read from those: the record
//! is a function of the draws, so a rerun of the seed gives it back whole
//! where it does not give back the schedule that failed.
//!
//! Knobs, all environment variables:
//!
//! - `ZFUZZ_FROM`, `ZFUZZ_TO`: the seeds, half open. The default is a few
//!   seeds that run in seconds.
//! - `ZFUZZ_SEEDS`: a comma separated list of seeds in place of the range.
//! - `ZFUZZ_PIN_CFG=n`: every seed runs seed `n`'s configuration (its
//!   `StrideSteer` with it) and draws its own operations, the ones that
//!   follow its own configuration's draws, so that a range is that many
//!   schedules of one failing shape; the pinned seed itself runs as it
//!   does unpinned. Unset, no seed's draws move. A failure line names the
//!   pin.
//! - `ZFUZZ_HISTORY=<hex key>`: print the key's history at the seed's end,
//!   passed or failed (`ZFUZZ_HISTORY=` for the empty key).
//! - `ZFUZZ_CACHE`: `1` turns the block cache on for the one seed in five
//!   that asks for it, `all` for every seed, and `off` for none while
//!   drawing as `1` does, so that a seed that failed under `1` or `all`
//!   reruns as the same configuration and the same operations with the
//!   cache off. Unset, no seed has the cache and the draw is not made, so a
//!   seed's other choices are not those it makes under the three.
//! - `ZFUZZ_PROMOTE`: set, with the form forced and the cache on, the `z`
//!   store holds a block read a few entries' worth as a copy in its slot
//!   under a small budget (`Options::zpages_promote_copies`).
//! - `ZFUZZ_OPTVARY`: set, a seed turns compaction, promotion, the trusted
//!   page views of the compact form or its block cursor off, or its
//!   memo-first ask on, each with a chance of 15%.
//! - `ZFUZZ_SHAPES`: the key shapes a seed may draw from, as indexes from 0
//!   to 7 separated by commas.
//! - `ZFUZZ_SETTLEBUILDS=1`: the `z` store's commits near reads file their
//!   batches and build the blocks they filed into, under a small cache
//!   budget (`Options::forms_settle_every`, `forms_settle_builds`).
//! - `ZFUZZ_BOTH=1`, with `ZFUZZ_ZPAGES`: the `z` store keeps both codings
//!   of every partition and re-codes by exchanging them
//!   (`Options::zpages_both`).
//! - `ZFUZZ_FORMSOFF=1`, with `ZFUZZ_ZPAGES`: the `z` store's handles give
//!   their block forms up for the merge path and take them back every few
//!   scans (`Options::scan_forms_off_unreused`), a flag and not a draw.
//! - `ZFUZZ_SNAPKEPT=1`, with `ZFUZZ_FORMSOFF`: the commits' maintenance
//!   keeps the merge path's snapshot while the forms are off
//!   (`Options::forms_off_snapshot_kept`), with the stale exit off so that
//!   the forms stay off through the commits between two flips; a flag and
//!   not a draw.
//! - `ZFUZZ_PARSE=1`: the `z` store's block-coded groups are written by the
//!   parsing encoder (`Options::zpages_parse`), a flag and not a draw, so
//!   that every seed's operations are the ones it had without it.
//! - `ZFUZZ_FLEXDEC=1`: the `z` store's block-coded groups are decoded by
//!   `lz4_flex` and not the engine's own decoder (`Options::zpages_own_decoder`
//!   off), a flag and not a draw for the same reason.
//! - `ZFUZZ_KEEPONE=1`: both stores keep one table, and a byte's worth, for
//!   the upkeep's look (`Options::upkeep_keep_bytes`), so that a look a
//!   table behind and a pass with a table landed under its pin give the
//!   forms up (`Options::upkeep_abandons`) and the store reads as far from
//!   reads until its next scan; a flag and not a draw.
//! - `ZFUZZ_AUTOCACHE=1`: both stores' caches budget themselves at 64 KB
//!   (`Options::scan_cache_auto_bytes`), so the budget binds part way
//!   through most seeds with the cache on; a flag and not a draw.
//! - `ZFUZZ_MERGEPAR=n`: both stores run up to `n` partition merges at
//!   once over disjoint ranges (`Options::merge_parallel`); a flag and not
//!   a draw.
//! - `ZFUZZ_CENSUS=1`: a store whose form is not forced runs no controller
//!   and reads as out of core from its open on (the census gate, set
//!   through the test's seam, which a controller's tick would write over),
//!   so the paths a store takes there over the raw form -- the asks ahead
//!   of a settle and a build, the partitions' ordered-index heads held in
//!   anonymous memory (`Options::heads_owned`) -- are read against the
//!   model; the summary counts the copies made and given back.
//! - `ZFUZZ_HEADSFLIP=1`, with `ZFUZZ_ZPAGES`: the forced store's gate is
//!   moved between the form's and the census every few dozen steps, so
//!   its partitions' heads are copied where the raw form serves and given
//!   back where the files do, over and over under the reads, the handles'
//!   snapshots, the seals and the merges; `concurrent_range` runs its
//!   store forced under it and flips beside its reader threads, each of
//!   which may hold a copy's address when the copy is given up.
//! - `ZFUZZ_STRIDE=1`, with `ZFUZZ_ZPAGES`: the `z` store asks every build
//!   of a form file for the stride form (`Options::zpages_stride`
//!   `Forced`), trusting its own builds in some seeds and not in others
//!   (`zpages_stride_trust_built`), and the seed's keys and values are
//!   steered so that partitions qualify for it and decline it
//!   (`StrideSteer`); the summary counts the stride builds, the files, the
//!   reads they served and the declines by reason. A seed whose form files
//!   failed -- a read fallen back, an attach refused, a build failed, a
//!   page found damaged -- fails, since the raw form answers in their place
//!   and the model sees nothing wrong; and a range in which the stride form
//!   served no read fails, since a steer that reaches no partition is a run
//!   that tested nothing. Refused beside `ZFUZZ_BOTH`, whose re-code the
//!   store refuses beside the stride form. `concurrent_range` runs its store
//!   forced with the stride form under it, its keys one run of numbers at
//!   one width, each written once before the readers start and put again,
//!   never appended or deleted, with a value of one width that says how many
//!   times it was put, so that its partitions qualify through every merge;
//!   a range whose stride files answered no read fails there too.
//! - `ZFUZZ_SPARSE=1`, with `ZFUZZ_ZPAGES` and `ZFUZZ_STRIDE`: the `z`
//!   store's stride form takes a partition whose keys are numbers with
//!   gaps between them (`Options::zpages_stride_sparse`), so that what the
//!   steer made a partition decline for its holes -- a delete, a number
//!   taken out of the pool -- makes it a sparse file, read by a search of
//!   its keys' numbers, and the seals and merges rewrite partitions
//!   between the form that holds no number, the sparse one and the block
//!   form as the holes come and are filled. Nothing is drawn for it: in
//!   the seeds whose decimal field is sixteen digits or fewer and whose
//!   value width is odd the keys are hex digits, of the case the seed's
//!   trust says, and in the seeds with no number taken out and a field to
//!   spare the pool is numbers scattered over the field, as ids are, not
//!   one run of them. The summary counts the sparse builds, and a range
//!   with none fails.
//! - `ZFUZZ_CONVERT=1`, with `ZFUZZ_ZPAGES` and `ZFUZZ_STRIDE`: every few
//!   dozen steps a partition a form file serves is asked to move to its
//!   other form (`Db::zpages_convert_ask`) -- a stride file's to the block
//!   form by a build beside the reads, and back by a build that asks for
//!   the stride form alone where the partition kept no reason against it
//!   -- so that a store in two forms, its partitions changing
//!   form under the reads, the handles' snapshots, the seals and the
//!   merges, is read against the model; the summary counts the moves each
//!   way, and a range in which none went out of the stride form fails.
//!   `concurrent_range` asks beside its reader threads, each of which may
//!   be in the form file a swap replaces.
//! - `ZFUZZ_NOPUT`, `ZFUZZ_NOBURST`: set, no puts, or no ordered bursts.
//! - `ZFUZZ_SEEDKEY=1`: the store's first write is a key of its own, and
//!   one goes ahead of every write of the empty key, so that no write of
//!   the empty key is the first of its memtable.
//! - `ZFUZZ_VERBOSE=1`: print each seed's configuration, and at its end a
//!   hash of its record's writes and events, which the draws alone decide,
//!   so that two builds' streams are compared seed by seed.
//!   `ZFUZZ_TRACE=1`: print each event as it is reached, for the last line
//!   of a hang.
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
    /// A verified form page's directory read trusted
    /// (`Options::zpages_trusted_views`); off in some seeds under
    /// `ZFUZZ_OPTVARY`, so the shape before is read against the model too.
    trusted: bool,
    /// A verified page's group asked of the memo before any view
    /// (`Options::zpages_memo_first`, off as shipped); on in some seeds
    /// under `ZFUZZ_OPTVARY`.
    memo_first: bool,
    /// A block of a covered partition read through one cursor
    /// (`Options::zpages_block_cursor`); off in some seeds under
    /// `ZFUZZ_OPTVARY`, so the shape before is read against the model too.
    block_cursor: bool,
    /// The form file's groups coded record by record
    /// (`Options::zpages_record_coding`, off as shipped); on in a quarter of
    /// the seeds under `ZFUZZ_OPTVARY`, so the record coding is read against
    /// the model too.
    record_coding: bool,
    /// The fence searched through its tree (`Options::zpages_fence_tree`,
    /// off as shipped); on in a quarter of the seeds under `ZFUZZ_OPTVARY`,
    /// so the tree is read against the model too.
    fence_tree: bool,
    /// A decode asking ahead for the memo entry its put displaces
    /// (`Options::zpages_memo_ahead`); off in some seeds under
    /// `ZFUZZ_OPTVARY`.
    memo_ahead: bool,
    /// The scans' start keys kept with their blocks
    /// (`Options::scan_start_cache`, off); on in some seeds under
    /// `ZFUZZ_OPTVARY`, so a scan that takes its block from the cache is
    /// read against the model too.
    start_cache: bool,
    /// Every group of the form stored verbatim (`Options::zpages_store_all`,
    /// off); on in some seeds under `ZFUZZ_OPTVARY`, so the stored form is
    /// read against the model too.
    store_all: bool,
    /// The record coding's tails stored raw (`Options::zpages_raw_tails`,
    /// off), with the record coding on; on in some seeds under
    /// `ZFUZZ_OPTVARY`.
    raw_tails: bool,
    /// A point read's first touch of a group decoded in part with the
    /// memo's admission at the second (`Options::zpages_point_part`), and
    /// the table of where the keys read through the memo lie
    /// (`Options::zpages_point_keys`): each off in a quarter of the seeds
    /// under `ZFUZZ_OPTVARY`, so the shape before is read against the model
    /// too. Drawn from a generator of their own, seeded from the seed's
    /// and leaving it where it stood, so that no seed's other draws moved
    /// when these were added.
    point_part: bool,
    point_keys: bool,
    /// A walk's first touch of a group decoded into its cursor's own
    /// buffer (`Options::zpages_scan_admit`): off in a quarter of the seeds
    /// under `ZFUZZ_OPTVARY`, from the same side generator.
    scan_admit: bool,
    /// A walk's own decode of a group taken only as far as the walk reads
    /// (`Options::zpages_walk_part`): off in a quarter of the seeds under
    /// `ZFUZZ_OPTVARY`, the side generator's next draw.
    walk_part: bool,
    /// The scans started in a block before its scans hold the blocks they
    /// walk as copies under the form (`Options::promote_starts`): two in
    /// a quarter of the seeds under `ZFUZZ_OPTVARY`, the side generator's
    /// next draw, and the engine's otherwise.
    hot_starts: u32,
    /// The block path's cursor steps the fence to the next entry
    /// (`Options::zpages_cursor_steps`) and a sparse stride walk steps its
    /// key by the field (`Options::zpages_stride_step_direct`): each on in
    /// half the seeds under `ZFUZZ_OPTVARY`, the side generator's next two
    /// draws, and off otherwise, as the engine has them.
    cursor_steps: bool,
    stride_direct: bool,
    /// A bounds walk places a block boundary by its head
    /// (`Options::bounds_by_heads`) and a landing leaves its pieces' bounds
    /// where no table stands to read them (`Options::land_bounds_read`):
    /// each off in half the seeds under `ZFUZZ_OPTVARY`, the side
    /// generator's next two draws, and on otherwise, as the engine has
    /// them.
    bounds_heads: bool,
    land_bounds: bool,
    /// A third merge in flight while the writer's commits see no read
    /// (`Options::merge_parallel_quiet` at three): off in half the seeds
    /// under `ZFUZZ_OPTVARY`, the side generator's next draw, and on
    /// otherwise, as the engine has it.
    merge_quiet: bool,
    /// That room at any gate of the controller and not only where the
    /// compact form is installed (`Options::merge_quiet_any_gate`): in half
    /// the seeds that draw the third merge.
    merge_quiet_any: bool,
    /// Under `ZFUZZ_STRIDE`, the stride form's steer; `None` without it.
    stride: Option<StrideSteer>,
}

/// Whether the stride form is forced on the `z` store and the seed steered
/// toward it (`ZFUZZ_STRIDE`, with the form forced).
fn stride_on() -> bool {
    env_flag("ZFUZZ_STRIDE") && env_flag("ZFUZZ_ZPAGES")
}

/// Whether the `z` store's stride form takes keys with gaps between their
/// numbers, in the sparse form (`ZFUZZ_SPARSE`, with the stride form
/// forced).
fn sparse_on() -> bool {
    stride_on() && env_flag("ZFUZZ_SPARSE")
}

/// Where a stride seed's run of numbers starts: at zero, at the field's top
/// less the run, which a key past the run's last would overflow, or
/// anywhere in the field.
#[derive(Clone, Copy, Debug)]
enum RunBase {
    Zero,
    Top,
    Any,
}

/// A seed's steer toward the stride form (`ZFUZZ_STRIDE`). A partition
/// qualifies where its keys are one length, a prefix and then a number of
/// one width, every number between its first and last present, and every
/// record one inline value of one width with no flag; so the pool is a run
/// of numbers at one width under one prefix -- zero-padded decimal, or
/// big-endian in a quarter of the seeds -- most values take one width, and
/// every key is written once before the steps in most seeds, in key order
/// (a direct run) or shuffled. What makes a partition decline comes at
/// rates the seed draws: deletes (holes, and a promoted piece's tombstone
/// flags), runs of values and appends to a key that has one (a record of
/// more than one value), values of another width, numbers taken out of the
/// pool and keys of another shape put into it -- none of them in some
/// seeds, a few in most, and in some enough that most partitions decline,
/// so that the seals and merges rewrite partitions from one form into the
/// other both ways. A partition's puts reach a piece as a tombstone and a
/// value, which a merge folds into one value and a promotion keeps; both
/// are read against the model through the stride files and the raw twin.
/// Drawn from a generator of its own (`pick_cfg`), so that with the switch
/// off no draw of any seed moves.
#[derive(Clone, Debug)]
struct StrideSteer {
    /// `Options::zpages_stride_trust_built`, both ways across seeds.
    trust: bool,
    /// The keys: `prefix`, then the number in `d` bytes, big-endian where
    /// `be`, zero-padded decimal otherwise, from `base`.
    be: bool,
    prefix: Vec<u8>,
    d: usize,
    base: RunBase,
    /// The width most values take: one byte to a page, the inline limit's
    /// either side among them, so that a value past it, not inline, makes
    /// its partition decline in the seeds that keep the default limit.
    w: usize,
    /// Chances in a thousand that a write deletes, appends a run of
    /// values, appends one value rather than putting it, or that a value
    /// takes another width.
    delete: usize,
    multi: usize,
    append: usize,
    odd: usize,
    /// Numbers taken out of the run, and keys of another shape put into
    /// the pool among the run's.
    gaps: usize,
    foreign: usize,
    /// Every key of the pool written once before the steps, and whether in
    /// key order.
    preload: bool,
    ordered: bool,
    /// The ordered bursts' keys put rather than appended.
    burst_puts: bool,
}

impl StrideSteer {
    fn pick(r: &mut Rng) -> StrideSteer {
        let be = r.chance(25);
        let d = if be {
            [1, 2, 3, 4, 6, 8][r.below(6)]
        } else {
            [3, 4, 5, 6, 8, 12, 16, 19][r.below(8)]
        };
        let prefix: &[u8] = if be {
            [&b""[..], b"\x01", b"b/", b"\x00\x00"][r.below(4)]
        } else {
            [&b""[..], b"k", b"user", b"a/long/common/prefix/", b"\x01"][r.below(5)]
        };
        let base = match r.below(5) {
            0 | 1 => RunBase::Zero,
            2 => RunBase::Top,
            _ => RunBase::Any,
        };
        let w = [1, 3, 4, 8, 8, 8, 13, 16, 16, 24, 100, 256, 257, 1000, 4096][r.below(15)];
        let (delete, multi, append, odd) = match r.below(20) {
            0..=6 => (0, 0, 0, 0),
            7..=14 => (8, 4, 15, 4),
            _ => (40, 25, 80, 30),
        };
        let gaps = if r.chance(15) { 1 + r.below(3) } else { 0 };
        let foreign = if r.chance(15) { 1 + r.below(3) } else { 0 };
        let preload = r.chance(85);
        let ordered = r.chance(60);
        let burst_puts = r.chance(30);
        StrideSteer {
            trust: r.chance(50),
            be,
            prefix: prefix.to_vec(),
            d,
            base,
            w,
            delete,
            multi,
            append,
            odd,
            gaps,
            foreign,
            preload,
            ordered,
            burst_puts,
        }
    }

    /// Under `ZFUZZ_SPARSE`, whether the seed's keys are hex digits and
    /// not decimal ones, and of which case: from what the seed drew
    /// already, so that the switch adds no draw -- a decimal field a u64 of
    /// hex digits fits, an odd value width, and the case the trust says.
    fn hex(&self) -> Option<bool> {
        (sparse_on() && !self.be && self.d <= 16 && self.w % 2 == 1).then_some(self.trust)
    }

    /// The largest number the field holds.
    fn top(&self) -> u64 {
        if self.be {
            u64::MAX >> (8 * (8 - self.d))
        } else if self.hex().is_some() {
            u64::MAX >> (4 * (16 - self.d))
        } else {
            10u64.pow(self.d as u32) - 1
        }
    }

    fn key(&self, x: u64) -> Vec<u8> {
        let mut k = self.prefix.clone();
        if self.be {
            k.extend_from_slice(&x.to_be_bytes()[8 - self.d..]);
        } else {
            match self.hex() {
                Some(true) => k.extend(format!("{x:0w$X}", w = self.d).bytes()),
                Some(false) => k.extend(format!("{x:0w$x}", w = self.d).bytes()),
                None => k.extend(format!("{x:0w$}", w = self.d).bytes()),
            }
        }
        k
    }

    /// The seed's pool: `n` numbers from the run's base, as many as the
    /// field holds, less the gaps -- never the run's ends, so that the gap
    /// is a hole and not a shorter run -- and with the foreign keys among
    /// them: a run key with a byte more, a byte short, or its last byte
    /// changed -- past a digit's in decimal, to the byte's top in
    /// big-endian, another number there and maybe one past the run -- each
    /// sorting among the run's keys or beside them.
    fn pool(&self, rng: &mut Rng, n: usize) -> Vec<Vec<u8>> {
        let top = self.top();
        let n = (n as u64).min(top.saturating_add(1)).max(1);
        let last_base = top - (n - 1);
        let base = match self.base {
            RunBase::Zero => 0,
            RunBase::Top => last_base,
            RunBase::Any => rng.next() % last_base.saturating_add(1),
        };
        let mut nums: Vec<u64> = (0..n).map(|i| base + i).collect();
        // Under `ZFUZZ_SPARSE`, where the seed takes no number out of a run
        // and the field has room, no run at all: numbers scattered over the
        // field, as random ids are over theirs.
        if sparse_on() && self.gaps == 0 && top / 8 > n {
            let mut scattered: BTreeSet<u64> = BTreeSet::new();
            while (scattered.len() as u64) < n {
                scattered.insert(rng.next() % top.saturating_add(1).max(1));
            }
            nums = scattered.into_iter().collect();
        }
        for _ in 0..self.gaps {
            if nums.len() > 2 {
                nums.remove(1 + rng.below(nums.len() - 2));
            }
        }
        let mut set: BTreeSet<Vec<u8>> = nums.iter().map(|&x| self.key(x)).collect();
        for _ in 0..self.foreign {
            let mut k = self.key(nums[rng.below(nums.len())]);
            match rng.below(3) {
                0 => k.push(if self.be { 0 } else { b'x' }),
                1 => *k.last_mut().unwrap() = if self.be { 0xff } else { b':' },
                _ => {
                    k.pop();
                }
            }
            set.insert(k);
        }
        set.into_iter().collect()
    }

    /// A value of the seed's width, or of another at the seed's rate: one
    /// byte either side of it, none, or past twice it. Random bytes, so
    /// that a value read from the wrong slot is not the one asked for.
    fn value(&self, rng: &mut Rng) -> Vec<u8> {
        let w = if rng.below(1000) < self.odd {
            match rng.below(4) {
                0 => self.w + 1,
                1 => self.w - 1,
                2 => 0,
                _ => 2 * self.w + 3,
            }
        } else {
            self.w
        };
        let mut v = Vec::with_capacity(w + 8);
        while v.len() < w {
            v.extend_from_slice(&rng.next().to_le_bytes());
        }
        v.truncate(w);
        v
    }

    /// A step's writes to `k`: a put of one value, or at the seed's rates a
    /// delete, a run of values, or one value appended. Under `ZFUZZ_NOPUT`
    /// the put is an append.
    fn ops(&self, rng: &mut Rng, k: &[u8]) -> Vec<Op> {
        let r = rng.below(1000);
        let k = k.to_vec();
        if r < self.delete {
            vec![Op::Delete(k)]
        } else if r < self.delete + self.multi {
            (0..2 + rng.below(3))
                .map(|_| Op::Append(k.clone(), self.value(rng)))
                .collect()
        } else if r < self.delete + self.multi + self.append || std::env::var("ZFUZZ_NOPUT").is_ok()
        {
            vec![Op::Append(k, self.value(rng))]
        } else {
            vec![Op::Put(k, self.value(rng))]
        }
    }
}

/// One write, staged for the model, made to both stores and noted in the
/// seed's record.
fn write(op: Op, staged: &mut Vec<Op>, z: &mut Db, r: &mut Db) {
    for d in [z, r] {
        match &op {
            Op::Append(k, v) => d.append(k, v),
            Op::Delete(k) => d.delete(k),
            Op::Put(k, v) => d.put(k, v),
        }
    }
    note(Rec::Write {
        step: STEP.get(),
        op: op.clone(),
    });
    staged.push(op);
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
    let vary = std::env::var("ZFUZZ_OPTVARY").is_ok();
    let mut side = Rng::new(rng.0 ^ 0x9017_7A27_0000_0001);
    let (part_off, keys_off, scan_off) = (side.chance(25), side.chance(25), side.chance(25));
    let walk_off = side.chance(25);
    let hot_two = side.chance(25);
    let (steps_on, direct_on) = (side.chance(50), side.chance(50));
    let (heads_on, land_on) = (side.chance(50), side.chance(50));
    let quiet_on = side.chance(50);
    let quiet_any = side.chance(50);
    // A generator of its own, as `side` is, so that the switch moves none
    // of the draws below.
    let stride =
        stride_on().then(|| StrideSteer::pick(&mut Rng::new(rng.0 ^ 0x5791_DE57_0000_0002)));
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
        trusted: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
        memo_first: std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15),
        block_cursor: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
        record_coding: std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(25),
        fence_tree: std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(25),
        memo_ahead: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
        start_cache: std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(25),
        store_all: std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(25),
        raw_tails: std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(25),
        point_part: !(vary && part_off),
        point_keys: !(vary && keys_off),
        scan_admit: !(vary && scan_off),
        walk_part: !(vary && walk_off),
        hot_starts: if vary && hot_two {
            2
        } else {
            Options::default().promote_starts
        },
        cursor_steps: vary && steps_on,
        stride_direct: vary && direct_on,
        bounds_heads: !(vary && heads_on),
        land_bounds: !(vary && land_on),
        merge_quiet: !(vary && quiet_on),
        merge_quiet_any: vary && !quiet_on && quiet_any,
        stride,
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
        zpages_trusted_views: c.trusted,
        zpages_memo_first: c.memo_first,
        zpages_block_cursor: c.block_cursor,
        zpages_record_coding: c.record_coding || c.raw_tails,
        zpages_raw_tails: c.raw_tails,
        zpages_fence_tree: c.fence_tree,
        zpages_memo_ahead: c.memo_ahead,
        zpages_point_part: c.point_part,
        zpages_point_keys: c.point_keys,
        zpages_scan_admit: c.scan_admit,
        zpages_walk_part: c.walk_part,
        zpages_cursor_steps: c.cursor_steps,
        zpages_stride_step_direct: c.stride_direct,
        bounds_by_heads: c.bounds_heads,
        land_bounds_read: c.land_bounds,
        merge_parallel_quiet: if c.merge_quiet { 3 } else { 0 },
        merge_quiet_any_gate: c.merge_quiet_any,
        promote_starts: c.hot_starts,
        zpages_store_all: c.store_all,
        scan_start_cache: if c.start_cache { 4096 } else { 0 },
        // The block form unless the seed draws the other (`opts_z`,
        // `ZFUZZ_STRIDE`), whichever the engine ships: the pair is read
        // through both by the two draws, and a re-code policy, which the
        // engine refuses beside the stride form, is drawn only without it.
        zpages_stride: supdb::ZStride::Off,
        ..Options::default()
    };
    if let Some(i) = c.inline_bytes {
        o.inline_bytes = i;
    }
    if env_flag("ZFUZZ_CENSUS") {
        o.zpages = supdb::ZPages::Off;
    }
    if env_flag("ZFUZZ_KEEPONE") {
        o.upkeep_keep_bytes = 1;
    }
    if env_flag("ZFUZZ_AUTOCACHE") {
        o.scan_cache_auto_bytes = 64 << 10;
    }
    if let Some(n) = env_u64("ZFUZZ_MERGEPAR") {
        o.merge_parallel = n as usize;
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
    // Under `ZFUZZ_STRIDE` every build of a form file asks for the stride
    // form and builds the block body where the partition declines it.
    if let Some(s) = &c.stride {
        o.zpages_stride = supdb::ZStride::Forced;
        o.zpages_stride_trust_built = s.trust;
        // And under `ZFUZZ_SPARSE` the form takes a run with gaps.
        o.zpages_stride_sparse = sparse_on();
    }
    // Under `ZFUZZ_PROMOTE` with the form forced and the cache on, a block
    // read a few entries' worth is held as a copy in its slot and the
    // cache sheds under a small budget (`Options::zpages_promote_copies`),
    // so the copies, the sheds and the writes spliced into copies are read
    // against the raw store over the same writes.
    // Under `ZFUZZ_SETTLELATE` the commits leave their batches to the
    // backlog bound and the scans, the shape before a commit near reads
    // filed its own and built the blocks it filed into
    // (`Options::forms_settle_every`, `forms_settle_builds`), so the arm
    // that keeps it is read against the raw store too. Set, not drawn.
    if env_flag("ZFUZZ_SETTLELATE") {
        o.forms_settle_every = false;
        o.forms_settle_builds = false;
    }
    // Under `ZFUZZ_SETTLEBUILDS` every commit near reads files its batch
    // and builds the blocks it filed into that held no form
    // (`Options::forms_settle_every`, `forms_settle_builds`), under a
    // budget small enough to shed, so the forms the commits build, the
    // patches on them and the sheds are read against the raw store. Set,
    // not drawn: every seed's stream stays as it was.
    if env_flag("ZFUZZ_SETTLEBUILDS") {
        o.forms_settle_every = true;
        o.forms_settle_builds = true;
        if c.cache {
            o.scan_cache_bytes = 64 << 10;
            o.forms_to_fit = true;
        }
    }
    // Under `ZFUZZ_PARSE` the form's block-coded groups are the parsing
    // encoder's (`Options::zpages_parse`): the same format in other
    // sequences, read against the raw store.
    if env_flag("ZFUZZ_PARSE") {
        o.zpages_parse = true;
    }
    // Under `ZFUZZ_FORMSOFF`, with the form forced so that the gate is the
    // compact form's, the `z` store's handles give their forms up for the
    // merge path and take them back every three scans whatever their
    // starts (`Options::scan_forms_off_unreused`, `scan_forms_off_flip`),
    // so every switch, with the writes, seals and landings between them,
    // is read against the raw store; and a writer whose forms are off
    // takes them back at the commit that makes three writes in a row with
    // no scan between (`Options::scan_forms_off_stale_writes`), the exit a
    // commit makes. A flag and not a draw.
    if env_flag("ZFUZZ_FORMSOFF") {
        o.scan_forms_off_unreused = true;
        o.scan_forms_off_window = 3;
        o.scan_forms_off_flip = true;
        o.scan_forms_off_stale_writes = 3;
    }
    // Under `ZFUZZ_SNAPKEPT` the commit's maintenance keeps the merge
    // path's snapshot to the commit while the forms are off
    // (`Options::forms_off_snapshot_kept`), and the stale exit is off, so
    // that every commit between two flips of the forms is one the
    // maintenance keeps at: the freezes, landings and merges between them
    // are read against the raw store through a snapshot the scan did not
    // make. A flag and not a draw.
    if env_flag("ZFUZZ_SNAPKEPT") {
        o.forms_off_snapshot_kept = true;
        o.scan_forms_off_stale_writes = 0;
    }
    // Under `ZFUZZ_FLEXDEC` they are decoded by `lz4_flex` and not the
    // engine's own decoder (`Options::zpages_own_decoder`), read against the
    // raw store.
    if env_flag("ZFUZZ_FLEXDEC") {
        o.zpages_own_decoder = false;
    }
    // Under `ZFUZZ_BOTH`, with the form forced, every partition keeps its
    // form file in both codings and the re-code policy exchanges them
    // (`Options::zpages_both`): the spares' builds, their collect beside the
    // served file and their retirement with a merged segment, and whatever
    // the reads heat into a flip, are read against the raw store.
    if env_flag("ZFUZZ_BOTH") && env_flag("ZFUZZ_ZPAGES") {
        o.zpages_recode = true;
        o.zpages_recode_keep = true;
        o.zpages_both = true;
        o.zpages_drop_raw = true;
    }
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

thread_local! {
    /// The seed's record, for the reason `STEP` is kept: a check that fails
    /// is far from the loop that made the writes it is about.
    static LOG: std::cell::RefCell<Log> = std::cell::RefCell::new(Log::default());
}

/// What a seed did, in the order it did it. Every entry but the counts is
/// a function of the draws, so the record is the same in every run of a
/// seed, whatever the schedule did in it.
#[derive(Default)]
struct Log {
    recs: Vec<Rec>,
    /// Keys whose history a failure has printed: a seed stops at the step
    /// a check failed at, so every check that finds the key wrong after the
    /// first would print the same history.
    shown: BTreeSet<Vec<u8>>,
}

/// One entry of the record, at the step it was made: -1 before the steps
/// and the seed's `ops` in the final check.
enum Rec {
    /// A write staged to both stores.
    Write { step: i64, op: Op },
    /// The writes staged since the last `Commit` or `Lost` taken into the
    /// model: both stores' commits (`how` "commit"), or the commit a clean
    /// close makes ("close").
    Commit {
        step: i64,
        how: &'static str,
        counts: Counts,
    },
    /// The staged writes dropped by a crash.
    Lost { step: i64 },
    /// Anything else that changes the stores' shape or reads them whole:
    /// `what` begins with `check` at a full check, whose line in a key's
    /// history carries the model's value there, and with `reopen` at a
    /// reopen, whose stores count from zero.
    Event {
        step: i64,
        what: String,
        counts: Counts,
    },
}

/// Both stores' counts at an entry, `z` then `r`, under `COUNT_NAMES`.
type Counts = [[u64; 10]; 2];

/// Seals joined, partition merges landed, merges planned, pieces promoted,
/// manifests published, WAL rotations, directory barriers made for the
/// temp names of runs of ordered ingest -- at most one a run, and none for
/// a run whose name a rotation carried, so not a count of the runs -- then
/// whether a run of ordered ingest is open, which a burst's keys go to only
/// where the memtable was empty, and the partitions and the pieces
/// standing.
const COUNT_NAMES: [&str; 10] = [
    "seals",
    "merges",
    "plans",
    "promotions",
    "manifests",
    "rotations",
    "run-names",
    "run-open",
    "partitions",
    "pieces",
];

/// A store's counts: atomic loads, a field of the writer's, and the levels
/// under the writer's own pin, as any of its reads takes it. Nothing here
/// takes the upkeep home or draws, so a seed's schedule and stream are
/// what they were without the record.
fn counts_of(d: &Db) -> [u64; 10] {
    let w = d.seal_waits();
    let (parts, pieces) = d.levels();
    [
        w.joins,
        w.merges,
        w.plans,
        d.promotions().0,
        w.publishes,
        w.wal_rotations,
        w.run_names,
        u64::from(d.runs_unsynced().0.is_some()),
        parts as u64,
        pieces as u64,
    ]
}

fn note(rec: Rec) {
    LOG.with_borrow_mut(|l| l.recs.push(rec));
}

fn note_commit(how: &'static str, z: &Db, r: &Db) {
    let counts = [counts_of(z), counts_of(r)];
    note(Rec::Commit {
        step: STEP.get(),
        how,
        counts,
    });
}

fn note_event(what: impl Into<String>, z: &Db, r: &Db) {
    let counts = [counts_of(z), counts_of(r)];
    note(Rec::Event {
        step: STEP.get(),
        what: what.into(),
        counts,
    });
}

/// Both stores' commits, and the staged writes taken into the model.
fn commit_both(
    z: &mut Db,
    r: &mut Db,
    model: &mut Model,
    staged: &mut Vec<Op>,
) -> Result<(), String> {
    z.commit().map_err(|e| format!("commit z: {e}"))?;
    r.commit().map_err(|e| format!("commit r: {e}"))?;
    for op in staged.drain(..) {
        apply(model, &op);
    }
    note_commit("commit", z, r);
    Ok(())
}

fn op_key(op: &Op) -> &[u8] {
    match op {
        Op::Append(k, _) | Op::Delete(k) | Op::Put(k, _) => k,
    }
}

fn op_value(op: &Op) -> Option<&[u8]> {
    match op {
        Op::Append(_, v) | Op::Put(_, v) => Some(v),
        Op::Delete(_) => None,
    }
}

/// A value in hex with its length, cut to its first 32 bytes past 48.
fn hexv(v: &[u8]) -> String {
    if v.len() <= 48 {
        format!("{}({})", show(v), v.len())
    } else {
        format!("{}..({})", show(&v[..32]), v.len())
    }
}

fn hexvs(vs: &[Vec<u8>]) -> String {
    let all: Vec<String> = vs.iter().map(|v| hexv(v)).collect();
    format!("[{}]", all.join(", "))
}

fn op_line(op: &Op) -> String {
    match op {
        Op::Append(_, v) => format!("Append {}", hexv(v)),
        Op::Put(_, v) => format!("Put {}", hexv(v)),
        Op::Delete(_) => "Delete".into(),
    }
}

/// For each entry, the `Commit` or `Lost` that ended it where it is a
/// write; `None` for a write still staged at the record's end.
fn fates(recs: &[Rec]) -> Vec<Option<usize>> {
    let mut out = vec![None; recs.len()];
    let mut pending = Vec::new();
    for (i, rec) in recs.iter().enumerate() {
        match rec {
            Rec::Write { .. } => pending.push(i),
            Rec::Commit { .. } | Rec::Lost { .. } => {
                for w in pending.drain(..) {
                    out[w] = Some(i);
                }
            }
            Rec::Event { .. } => {}
        }
    }
    out
}

fn fate_line(recs: &[Rec], end: Option<usize>) -> String {
    match end.map(|j| &recs[j]) {
        Some(Rec::Commit { step, how, .. }) if *how == "close" => {
            format!("committed by the close at s{step}")
        }
        Some(Rec::Commit { step, .. }) => format!("committed at s{step}"),
        Some(Rec::Lost { step }) => format!("dropped by the crash at s{step}"),
        _ => "staged at the record's end".into(),
    }
}

/// Both stores' counts where they moved since `prev`, or whole where
/// there is nothing to move from: a reopen's stores count from zero.
fn counts_line(prev: Option<&Counts>, now: &Counts) -> String {
    let mut parts = Vec::new();
    for (s, name) in ["z", "r"].iter().enumerate() {
        let mut moved = Vec::new();
        for (c, n) in COUNT_NAMES.iter().enumerate() {
            let v = now[s][c] as i64;
            match prev {
                Some(p) if v != p[s][c] as i64 => {
                    moved.push(format!("{n}{:+}", v - p[s][c] as i64));
                }
                None if v != 0 => moved.push(format!("{n} {v}")),
                _ => {}
            }
        }
        if !moved.is_empty() {
            parts.push(format!("{name} {}", moved.join(" ")));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join("; "))
    }
}

/// The key's history from the seed's record, one line an entry that
/// touches it: each write with its step and its fate, each commit that
/// took one with the model's value after it, each crash that dropped one,
/// and every check with the model's value there and both stores' counts
/// whole. The entries between are one line each gap -- the seals, flushes,
/// settles, bursts, closes, crashes and reopens, and the commits where the
/// counts moved -- each with the counts that moved since the entry before.
fn history_of(recs: &[Rec], key: &[u8]) -> String {
    let ends = fates(recs);
    let mut lines: Vec<String> = Vec::new();
    let mut between: Vec<String> = Vec::new();
    // The model's value of the key, replayed by the model's own `apply`.
    let mut model: Model = BTreeMap::new();
    let mut staged: Vec<&Op> = Vec::new();
    let mut prev: Option<&Counts> = None;
    let mut writes = 0usize;
    let value = |m: &Model| hexvs(m.get(key).map_or(&[][..], |v| v.as_slice()));
    let gap = |between: &mut Vec<String>, lines: &mut Vec<String>| {
        if !between.is_empty() {
            lines.push(format!("    .. {}", between.join(", ")));
            between.clear();
        }
    };
    for (i, rec) in recs.iter().enumerate() {
        match rec {
            Rec::Write { step, op } => {
                if op_key(op) == key {
                    gap(&mut between, &mut lines);
                    writes += 1;
                    lines.push(format!(
                        "  s{step} {} -- {}",
                        op_line(op),
                        fate_line(recs, ends[i])
                    ));
                    staged.push(op);
                }
            }
            Rec::Commit { step, how, counts } => {
                let moved = counts_line(prev, counts);
                prev = Some(counts);
                if staged.is_empty() {
                    if !moved.is_empty() || *how != "commit" {
                        between.push(format!("s{step} {how}{moved}"));
                    }
                } else {
                    for op in staged.drain(..) {
                        apply(&mut model, op);
                    }
                    gap(&mut between, &mut lines);
                    lines.push(format!(
                        "  s{step} {how}{moved}: the model's value {}",
                        value(&model)
                    ));
                }
            }
            Rec::Lost { step } => {
                if !staged.is_empty() {
                    gap(&mut between, &mut lines);
                    lines.push(format!(
                        "  s{step} crash: {} staged write(s) of the key dropped",
                        staged.len()
                    ));
                    staged.clear();
                }
            }
            Rec::Event { step, what, counts } => {
                let whole = what.starts_with("reopen");
                let moved = counts_line(if whole { None } else { prev }, counts);
                prev = Some(counts);
                if what.starts_with("check") {
                    gap(&mut between, &mut lines);
                    lines.push(format!(
                        "  s{step} {what}: the model's value {}; counts{}",
                        value(&model),
                        counts_line(None, counts)
                    ));
                } else {
                    between.push(format!("s{step} {what}{moved}"));
                }
            }
        }
    }
    gap(&mut between, &mut lines);
    format!(
        "key {} (len {}): {writes} write(s), the model's value at the record's end {}\n{}",
        show(key),
        key.len(),
        value(&model),
        lines.join("\n")
    )
}

fn history(key: &[u8]) -> String {
    LOG.with_borrow(|l| history_of(&l.recs, key))
}

/// The writes of the seed's record that wrote `v`: the key's own, then
/// other keys', a few of each, newest first.
fn provenance(key: &[u8], v: &[u8]) -> String {
    LOG.with_borrow(|l| {
        let ends = fates(&l.recs);
        let (mut own, mut other) = (Vec::new(), Vec::new());
        for (i, rec) in l.recs.iter().enumerate().rev() {
            let Rec::Write { step, op } = rec else {
                continue;
            };
            if op_value(op) != Some(v) {
                continue;
            }
            let kind = match op {
                Op::Append(..) => "Append",
                Op::Put(..) => "Put",
                Op::Delete(..) => unreachable!("a delete writes no value"),
            };
            let what = format!("{kind} at s{step}, {}", fate_line(&l.recs, ends[i]));
            if op_key(op) == key {
                own.push(format!("the key's own {what}"));
            } else {
                other.push(format!("key {}'s {what}", show(op_key(op))));
            }
        }
        if own.is_empty() && other.is_empty() {
            return "no write of the seed's".into();
        }
        let (n_own, n_other) = (own.len(), other.len());
        own.truncate(4);
        other.truncate(4);
        let mut out = own;
        out.extend(other);
        format!(
            "{} ({n_own} of the key's own, {n_other} of other keys')",
            out.join("; ")
        )
    })
}

/// A key a check found wrong, said in full on stderr, since a failure's
/// line is cut short: what was read and what the model wants, in hex; the
/// earlier write each value read and not wanted equals; the reader's
/// sources for the key; and, the first time a seed's check finds the key
/// wrong, its history. `got` is `None` where the check read no values.
fn explain(label: &str, src: &Reader, key: &[u8], got: Option<&[Vec<u8>]>, want: &[Vec<u8>]) {
    let step = STEP.get();
    eprintln!(
        "DETAIL {label} [step {step}] key {} (len {}): got {}, want {}",
        show(key),
        key.len(),
        got.map_or_else(|| "-".to_string(), hexvs),
        hexvs(want)
    );
    for (i, v) in got.unwrap_or_default().iter().enumerate() {
        if !want.contains(v) {
            eprintln!(
                "DETAIL {label} [step {step}] got value {i} {} is {}",
                hexv(v),
                provenance(key, v)
            );
        }
    }
    eprintln!(
        "SOURCES {label} [step {step}] key {}: {}",
        show(key),
        src.debug_key(key)
    );
    if LOG.with_borrow_mut(|l| l.shown.insert(key.to_vec())) {
        eprintln!("HISTORY [step {step}] {}", history(key));
    }
}

/// The record's writes and events hashed (FNV-1a), the counts left out:
/// a function of the draws alone.
fn log_hash() -> (usize, u64) {
    LOG.with_borrow(|l| {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |b: &[u8]| {
            for &x in b {
                h = (h ^ x as u64).wrapping_mul(0x0100_0000_01b3);
            }
            // A separator no length ambiguity crosses.
            h = (h ^ 0x1ff).wrapping_mul(0x0100_0000_01b3);
        };
        for rec in &l.recs {
            match rec {
                Rec::Write { step, op } => {
                    let kind: &[u8] = match op {
                        Op::Append(..) => b"A",
                        Op::Put(..) => b"P",
                        Op::Delete(..) => b"D",
                    };
                    eat(kind);
                    eat(&step.to_le_bytes());
                    eat(op_key(op));
                    eat(op_value(op).unwrap_or_default());
                }
                Rec::Commit { step, how, .. } => {
                    eat(how.as_bytes());
                    eat(&step.to_le_bytes());
                }
                Rec::Lost { step } => {
                    eat(b"lost");
                    eat(&step.to_le_bytes());
                }
                Rec::Event { step, what, .. } => {
                    eat(what.as_bytes());
                    eat(&step.to_le_bytes());
                }
            }
        }
        (l.recs.len(), h)
    })
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
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

/// Keys a check explains in full (`explain`) before it only counts them.
const EXPLAINED: usize = 3;

fn check_keys(label: &str, r: &Reader, model: &Model, keys: &[Vec<u8>], fails: &mut Vec<String>) {
    let mut explained = 0;
    for k in keys {
        let want = model.get(k).cloned().unwrap_or_default();
        let before = fails.len();
        let got = match read_vec(r, k) {
            Ok(got) if got == want => Some(got),
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
                Some(got)
            }
            Err(e) => {
                fail(fails, format!("{label}: read of key {}: {e}", show(k)));
                None
            }
        };
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
        if fails.len() > before && explained < EXPLAINED {
            explained += 1;
            explain(label, r, k, got.as_deref(), &want);
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

/// The first key, in key order, that `got` answers differently from the
/// model: one the model has and `got` answers otherwise or not at all, or
/// one `got` lists that the model does not.
fn first_diff_key(got: &Model, want: &Model) -> Option<Vec<u8>> {
    let differs = want
        .iter()
        .find(|(k, w)| got.get(*k) != Some(w))
        .map(|(k, _)| k);
    let extra = got.keys().find(|k| !want.contains_key(*k));
    match (differs, extra) {
        (Some(a), Some(b)) => Some(a.min(b).clone()),
        (a, b) => a.or(b).cloned(),
    }
}

/// The first key a whole scan answered wrongly, said in full (`explain`).
fn explain_scan(label: &str, src: &Reader, got: &Model, want: &Model) {
    if let Some(k) = first_diff_key(got, want) {
        let none = Vec::new();
        explain(
            label,
            src,
            &k,
            Some(got.get(&k).unwrap_or(&none)),
            want.get(&k).unwrap_or(&none),
        );
    }
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
    /// The times either store's upkeep thread gave its forms up
    /// (`Options::upkeep_abandons`), summed over the store's opens: what
    /// says a run under `ZFUZZ_KEEPONE` reached the path it is for.
    abandons: u64,
    /// The times either store's own cache budget bound
    /// (`Options::scan_cache_auto`), summed as `abandons` is: what says a
    /// run under `ZFUZZ_AUTOCACHE` reached the binding.
    bindings: u64,
    /// Partition merges started beside another in flight, summed as
    /// `abandons` is: what says a run under `ZFUZZ_MERGEPAR` reached them.
    beside: u64,
    /// The `z` store's switches between the block path and the merge path
    /// with no forms (`Options::scan_forms_off_unreused`), summed as
    /// `abandons` is: what says a run under `ZFUZZ_FORMSOFF` switched.
    switches: u64,
    /// The commits whose maintenance kept the `z` store's merge-path
    /// snapshot with the forms off (`Options::forms_off_snapshot_kept`),
    /// summed as `abandons` is: what says a run under `ZFUZZ_SNAPKEPT`
    /// kept one.
    kept: u64,
    /// Sparse forms either store kept across a partition's rewrite with
    /// their cuts resolved again (`Options::rebase_recuts_sparse`), summed
    /// as `abandons` is: what says a run with the cache on reached them.
    recuts: u64,
    /// Copies of partitions' ordered-index heads either store made, and
    /// gave back (`Options::heads_owned`), summed as `abandons` is: what
    /// says a run under `ZFUZZ_CENSUS` or `ZFUZZ_ZPAGES` reached them.
    heads: [u64; 2],
    /// Through either store's form files: groups a point read decoded in
    /// part, groups the memo took at a second touch, reads the key table
    /// sent to their record, groups a walk decoded into its own buffer and
    /// of those the groups decoded in part (`Options::zpages_point_part`,
    /// `zpages_point_keys`, `zpages_scan_admit`, `zpages_walk_part`),
    /// summed as `recuts` is: what says a run under `ZFUZZ_ZPAGES` reached
    /// each; then the writers' scans that started in a block gone hot, and
    /// on a copy (`Options::promote_starts`); then the times a writer took
    /// its forms back at a commit, its reading of the scans gone stale
    /// (`Options::scan_forms_off_stale_writes`), what says a run under
    /// `ZFUZZ_FORMSOFF` reached that exit; then the fence entries the
    /// cursors stepped to with no search (`Options::zpages_cursor_steps`),
    /// what says a run under `ZFUZZ_OPTVARY` and `ZFUZZ_ZPAGES` reached the
    /// step; then the block boundaries a bounds walk placed by a head and
    /// the landings that left their bounds (`Options::bounds_by_heads`,
    /// `land_bounds_read`), what says a run under `ZFUZZ_OPTVARY` reached
    /// each, the second with `ZFUZZ_FORMSOFF`; then the merges started
    /// past `merge_parallel` in a stretch of writes nobody read
    /// (`Options::merge_parallel_quiet`).
    point: [u64; 12],
    /// The `z` store's stride form under `ZFUZZ_STRIDE`.
    stride: StrideStats,
}

/// What the stride form did in a seed (`ZCounts`' stride counts), summed
/// over the `z` store's opens as `recuts` is, but for the files, the most
/// partitions a stride file served at once at any of the counts.
#[derive(Clone, Debug, Default)]
struct StrideStats {
    builds: u64,
    files: u64,
    /// Point reads a stride file answered, the values its cursor handed
    /// over, and the value pages checked against the row at a first touch.
    reads: u64,
    cursor: u64,
    checks: u64,
    /// The builds that asked for the stride form and declined it, by
    /// reason, in `ZCounts::stride_declines`' order.
    declines: Vec<u64>,
    /// The form's failures of any kind: reads that fell back to the raw
    /// form after a check, attaches refused as damaged or as another
    /// segment's, builds that failed, and files or pages found damaged.
    /// None is a wrong answer, which the model would see; each is a form
    /// file the store could not serve from, which a seed should not make.
    failures: u64,
    /// Form files built of either body, and the partitions whose values
    /// lie in blocks, which neither body holds: under `ZStride::Forced` the
    /// stride form's decline for a run in a block is counted there, once
    /// (`ZCounts::declined_block_backed`), and a seed whose values are past
    /// the inline limit reaches no stride file for that reason.
    form_builds: u64,
    block_backed: u64,
    /// Partitions moved into the stride form while served and out of it
    /// (`ZFUZZ_CONVERT`).
    converts: u64,
    converts_back: u64,
    /// The stride builds that made a sparse file (`ZFUZZ_SPARSE`).
    sparse: u64,
}

impl StrideStats {
    fn add(&mut self, c: &supdb::db::ZCounts) {
        self.form_builds += c.builds;
        self.block_backed += c.declined_block_backed;
        self.converts += c.stride_converts;
        self.converts_back += c.stride_converts_back;
        self.sparse += c.stride_sparse_builds;
        self.builds += c.stride_builds;
        self.files = self.files.max(c.stride_files);
        self.reads += c.stride_reads;
        self.cursor += c.stride_cursor_reads;
        self.checks += c.stride_page_checks;
        if self.declines.len() < c.stride_declines.len() {
            self.declines.resize(c.stride_declines.len(), 0);
        }
        for (s, d) in self.declines.iter_mut().zip(c.stride_declines) {
            *s += d;
        }
        self.failures += c.fallbacks
            + c.rejected_damage
            + c.rejected_identity
            + c.build_failed
            + c.damaged
            + c.bad_pages;
    }

    fn merge(&mut self, o: &StrideStats) {
        self.builds += o.builds;
        self.files += o.files;
        self.reads += o.reads;
        self.cursor += o.cursor;
        self.checks += o.checks;
        if self.declines.len() < o.declines.len() {
            self.declines.resize(o.declines.len(), 0);
        }
        for (s, d) in self.declines.iter_mut().zip(&o.declines) {
            *s += d;
        }
        self.failures += o.failures;
        self.form_builds += o.form_builds;
        self.block_backed += o.block_backed;
        self.converts += o.converts;
        self.converts_back += o.converts_back;
        self.sparse += o.sparse;
    }
}

impl std::fmt::Display for StrideStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "form builds {} block-backed {}, moved in {} out {}, \
             stride builds {} (sparse {}) files {} reads {} cursor {} \
             page checks {} failures {}, \
             declines [shape, holes, values, width, tomb, block, small, readback, deferred] {:?}",
            self.form_builds,
            self.block_backed,
            self.converts,
            self.converts_back,
            self.builds,
            self.sparse,
            self.files,
            self.reads,
            self.cursor,
            self.checks,
            self.failures,
            self.declines
        )
    }
}

/// Whether the `z` store's served partitions are asked to change form as
/// the seed runs (`ZFUZZ_CONVERT`, with the stride form forced).
fn convert_on() -> bool {
    stride_on() && env_flag("ZFUZZ_CONVERT")
}

/// A store's point-read and walk counts through its form files, added to
/// the seed's.
fn count_point(st: &mut Stats, db: &Db) {
    let c = db.zpage_counts();
    st.point[0] += c.point_parts;
    st.point[1] += c.point_admits;
    st.point[2] += c.point_key_hits;
    st.point[3] += c.walk_owned;
    st.point[4] += c.walk_parts;
    let h = db.hot_starts();
    st.point[5] += h[0];
    st.point[6] += h[1];
    st.point[7] += db.forms_off_stale_exits();
    st.point[8] += c.cursor_steps;
    st.point[9] += c.bounds_by_head;
    st.point[10] += c.land_bounds_left;
    st.point[11] += db.seal_waits().merges_quiet;
}

/// Under `ZFUZZ_CENSUS`, the store read as out of core: the census gate,
/// which a store whose form is forced does not take.
fn census_gate(db: &Db, forced: bool) {
    if env_flag("ZFUZZ_CENSUS") && !forced {
        db.zgate_set(1);
    }
}

/// A store's heads copies made and given back, added to the seed's.
fn count_heads(st: &mut Stats, db: &Db) {
    let (made, given, _) = db.heads_owned_counts();
    st.heads[0] += made;
    st.heads[1] += given;
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// The seed whose configuration every seed runs under `ZFUZZ_PIN_CFG`.
fn pinned() -> Option<u64> {
    env_u64("ZFUZZ_PIN_CFG")
}

/// " (cfg of seed n)" where the configuration is pinned, for the lines
/// that name a seed's configuration.
fn pin_note() -> String {
    pinned().map_or_else(String::new, |p| format!(" (cfg of seed {p})"))
}

/// A seed's configuration and the generator its operations are drawn
/// from, past its own configuration's draws. Under `ZFUZZ_PIN_CFG` the
/// configuration is the pinned seed's, drawn from a generator of its own,
/// and the seed's own is drawn all the same and thrown away, so that the
/// operations are the ones the seed draws unpinned: the pinned seed runs as
/// it does without the pin, and every other seed is another schedule of
/// its shape.
fn seed_draws(seed: u64, want_cache: bool) -> (Cfg, Rng) {
    let mut rng = Rng::new(seed);
    let own = pick_cfg(&mut rng, want_cache);
    let cfg = match pinned() {
        Some(p) => pick_cfg(&mut Rng::new(p), want_cache),
        None => own,
    };
    (cfg, rng)
}

/// One seed, whose failure line carries the step it was at and the
/// configuration it drew, whether the seed failed a check, returned an
/// error or panicked. The record is the seed's from here, whatever ended
/// it, so `ZFUZZ_HISTORY` reads it after a panic as after a pass.
fn run_seed(seed: u64, want_cache: bool, verbose: bool) -> Result<Stats, String> {
    let (cfg0, _) = seed_draws(seed, want_cache);
    STEP.set(-1);
    LOG.with_borrow_mut(|l| *l = Log::default());
    let run = std::panic::catch_unwind(|| run_seed_inner(seed, want_cache, verbose));
    let at = format!(
        "[step {} of {}] [cfg {}{}]",
        STEP.get(),
        cfg0.ops,
        cfg_line(&cfg0),
        pin_note()
    );
    if verbose {
        let (n, h) = log_hash();
        eprintln!("seed {seed}: record {n} entries, draws hash {h:016x}");
    }
    if let Ok(hex) = std::env::var("ZFUZZ_HISTORY") {
        match unhex(&hex) {
            Some(k) => eprintln!("HISTORY seed {seed} {}", history(&k)),
            None => eprintln!("ZFUZZ_HISTORY={hex} is not a key in hex"),
        }
    }
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
    let (cfg, mut rng) = seed_draws(seed, want_cache);
    let tag = format!("s{seed}");
    let (dz, dr) = (scratch(&format!("{tag}-z")), scratch(&format!("{tag}-r")));
    if verbose {
        eprintln!("seed {seed}{}: {cfg:?}", pin_note());
    }
    let mut z = Db::create(&dz, opts_z(&cfg)).map_err(|e| format!("create z: {e}"))?;
    let mut r = Db::create(&dr, opts(&cfg)).map_err(|e| format!("create r: {e}"))?;
    census_gate(&z, env_flag("ZFUZZ_ZPAGES"));
    census_gate(&r, false);
    let seedkey_first = env_flag("ZFUZZ_SEEDKEY");
    let mut model: Model = BTreeMap::new();
    let mut staged: Vec<Op> = Vec::new();
    let mut pool = match &cfg.stride {
        Some(s) => s.pool(&mut rng, cfg.keys),
        None => key_pool(&mut rng, cfg.keys),
    };
    if seedkey_first {
        let seedkey = b"\x01seed".to_vec();
        write(
            Op::Append(seedkey.clone(), b"s".to_vec()),
            &mut staged,
            &mut z,
            &mut r,
        );
        commit_both(&mut z, &mut r, &mut model, &mut staged)?;
        pool.push(seedkey);
    }
    // Under `ZFUZZ_STRIDE`, every key of the pool written once before the
    // steps in most seeds, in key order -- a direct run past the store's
    // greatest -- or shuffled, and made partitions in most of those.
    if let Some(s) = cfg.stride.as_ref().filter(|s| s.preload) {
        let mut order = pool.clone();
        if !s.ordered {
            for i in (1..order.len()).rev() {
                order.swap(i, rng.below(i + 1));
            }
        }
        for (i, k) in order.iter().enumerate() {
            write(
                Op::Append(k.clone(), s.value(&mut rng)),
                &mut staged,
                &mut z,
                &mut r,
            );
            if i % 97 == 96 || i + 1 == order.len() {
                commit_both(&mut z, &mut r, &mut model, &mut staged)?;
            }
        }
        if rng.chance(70) {
            z.flush().map_err(|e| format!("flush z: {e}"))?;
            r.flush().map_err(|e| format!("flush r: {e}"))?;
            note_event("flush", &z, &r);
        }
    }
    let mut hi = 0u64;
    let mut handles: Vec<Handle> = Vec::new();
    let mut st = Stats {
        reopens: 0,
        abandons: 0,
        bindings: 0,
        beside: 0,
        switches: 0,
        kept: 0,
        recuts: 0,
        heads: [0; 2],
        point: [0; 12],
        stride: StrideStats::default(),
    };
    let mut fails: Vec<String> = Vec::new();
    let mut absent = absent_around(&pool, &mut rng);

    let all_probe = |pool: &[Vec<u8>], absent: &[Vec<u8>]| -> Vec<Vec<u8>> {
        pool.iter().chain(absent.iter()).cloned().collect()
    };

    let scan_every = env_flag("ZFUZZ_SCANEVERY");
    let heads_flip = env_flag("ZFUZZ_HEADSFLIP") && env_flag("ZFUZZ_ZPAGES");
    let convert = convert_on();
    for step in 0..cfg.ops as u64 {
        STEP.set(step as i64);
        // By the step and not by a draw: every seed's stream stays as it
        // was.
        if heads_flip && step.is_multiple_of(37) {
            z.zgate_set(if (step / 37) % 2 == 0 { 1 } else { 2 });
        }
        if convert && step.is_multiple_of(29) {
            z.zpages_convert_ask((step / 29) as usize);
        }
        let k = pool[rng.below(pool.len())].clone();
        if seedkey_first && k.is_empty() {
            let seedkey = b"\x01seed".to_vec();
            write(
                Op::Append(seedkey, b"e".to_vec()),
                &mut staged,
                &mut z,
                &mut r,
            );
        }
        if let Some(s) = &cfg.stride {
            for op in s.ops(&mut rng, &k) {
                write(op, &mut staged, &mut z, &mut r);
            }
        } else {
            match rng.below(100) {
                0..=44 => {
                    let v = value(&mut rng, step, cfg.big);
                    write(Op::Append(k.clone(), v), &mut staged, &mut z, &mut r);
                }
                45..=52 => {
                    for _ in 0..2 + rng.below(8) {
                        let v = if rng.chance(50) {
                            // a fixed width run
                            ((rng.below(1 << 16)) as u32).to_le_bytes().to_vec()
                        } else {
                            value(&mut rng, step, cfg.big)
                        };
                        write(Op::Append(k.clone(), v), &mut staged, &mut z, &mut r);
                    }
                }
                53..=60 => write(Op::Delete(k.clone()), &mut staged, &mut z, &mut r),
                61..=65 if std::env::var("ZFUZZ_NOPUT").is_err() => {
                    let v = value(&mut rng, step, cfg.big);
                    write(Op::Put(k.clone(), v), &mut staged, &mut z, &mut r);
                }
                _ => {}
            }
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
            commit_both(&mut z, &mut r, &mut model, &mut staged)?;
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
                    commit_both(&mut z, &mut r, &mut model, &mut staged)?;
                    z.seal().map_err(|e| format!("seal z: {e}"))?;
                    r.seal().map_err(|e| format!("seal r: {e}"))?;
                    note_event("seal", &z, &r);
                }
                30..=44 => {
                    commit_both(&mut z, &mut r, &mut model, &mut staged)?;
                    z.flush().map_err(|e| format!("flush z: {e}"))?;
                    r.flush().map_err(|e| format!("flush r: {e}"))?;
                    note_event("flush", &z, &r);
                }
                45..=64 => {
                    z.settle().map_err(|e| format!("settle z: {e}"))?;
                    r.settle().map_err(|e| format!("settle r: {e}"))?;
                    note_event("settle", &z, &r);
                }
                65..=94 => {
                    // The builder, with or without a settle's worth of quiet.
                    z.settle().map_err(|e| format!("settle z: {e}"))?;
                    note_event("settle z", &z, &r);
                    // With the segment work inline no form file's build
                    // starts in the fuzz's stores -- the builds start at
                    // the turns of the segment work's thread's loop
                    // (`Maint::z_step`) -- so under `ZFUZZ_STRIDE` such a
                    // seed gives its partitions theirs here, on the
                    // writer's thread (`Db::zpages_settle`), or reaches no
                    // stride file at all.
                    if cfg.stride.is_some() && !cfg.bg {
                        z.zpages_settle()
                            .map_err(|e| format!("zpages settle z: {e}"))?;
                        note_event("zpages settle z", &z, &r);
                    }
                }
                95..=104 if std::env::var("ZFUZZ_NOBURST").is_err() => {
                    // An ordered burst: keys above the store's greatest, a
                    // direct run with markers in its record region.
                    let n = 5 + rng.below(80);
                    note_event(format!("burst of {n}"), &z, &r);
                    for i in 0..n {
                        let mut key = vec![0xff, 0xff, 0xff, 0xff];
                        key.extend(format!("{:08}", hi).bytes());
                        hi += 1;
                        // A run of numbers already: under `ZFUZZ_STRIDE`
                        // its values take the seed's width, so that a run
                        // a burst makes qualifies as the pool's do, and in
                        // the seeds that put them (`StrideSteer::burst_puts`)
                        // the keys leave the direct run for the hash table,
                        // whose seal flags each record a put wrote, and the
                        // piece above the partitions is promoted with its
                        // flags: a partition the stride form declines for
                        // them.
                        let op = match &cfg.stride {
                            Some(s) if s.burst_puts => Op::Put(key.clone(), s.value(&mut rng)),
                            Some(s) => Op::Append(key.clone(), s.value(&mut rng)),
                            None => Op::Append(key.clone(), value(&mut rng, step, cfg.big)),
                        };
                        write(op, &mut staged, &mut z, &mut r);
                        pool.push(key);
                        if rng.chance(15) || i + 1 == n {
                            commit_both(&mut z, &mut r, &mut model, &mut staged)?;
                        }
                    }
                    if rng.chance(70) {
                        z.flush().map_err(|e| format!("flush z: {e}"))?;
                        r.flush().map_err(|e| format!("flush r: {e}"))?;
                        note_event("flush", &z, &r);
                    }
                    absent = absent_around(&pool, &mut rng);
                }
                105..=119 => {
                    if handles.len() < 4 {
                        handles.push(Handle {
                            r: z.reader().map_err(|e| format!("reader: {e}"))?,
                            snap: None,
                        });
                        note_event("claim z", &z, &r);
                    }
                }
                120..=139 => {
                    // A snapshot of the committed store, held while the store
                    // changes under it.
                    commit_both(&mut z, &mut r, &mut model, &mut staged)?;
                    if let Some(h) = handles.iter_mut().find(|h| h.snap.is_none()) {
                        h.r.snapshot();
                        h.snap = Some(model.clone());
                        note_event("snapshot z", &z, &r);
                    }
                }
                140..=159 => {
                    if let Some(h) = handles.iter_mut().find(|h| h.snap.is_some()) {
                        h.r.release();
                        h.snap = None;
                        note_event("release z", &z, &r);
                    }
                }
                160..=169 => {
                    // A crash or a clean close, then the reopen.
                    handles.clear();
                    st.abandons += z.upkeep_abandons() + r.upkeep_abandons();
                    st.bindings += z.cache_bindings() + r.cache_bindings();
                    st.beside += z.seal_waits().merges_beside + r.seal_waits().merges_beside;
                    st.switches += z.forms_off_counts()[..2].iter().sum::<u64>();
                    st.kept += z.snapshots_kept();
                    st.recuts += z.forms_recut().0 + r.forms_recut().0;
                    count_point(&mut st, &z);
                    count_point(&mut st, &r);
                    count_heads(&mut st, &z);
                    count_heads(&mut st, &r);
                    st.stride.add(&z.zpage_counts());
                    let clean = rng.chance(50) || !matches!(cfg.sync, SyncPolicy::Always);
                    if clean {
                        for op in staged.drain(..) {
                            // close commits the staged batch.
                            apply(&mut model, &op);
                        }
                        note_commit("close", &z, &r);
                        z.close().map_err(|e| format!("close z: {e}"))?;
                        r.close().map_err(|e| format!("close r: {e}"))?;
                    } else {
                        note(Rec::Lost { step: STEP.get() });
                        note_event("crash", &z, &r);
                        staged.clear();
                        drop(z);
                        drop(r);
                    }
                    z = Db::open(&dz, opts_z(&cfg)).map_err(|e| format!("open z: {e}"))?;
                    r = Db::open(&dr, opts(&cfg)).map_err(|e| format!("open r: {e}"))?;
                    census_gate(&z, env_flag("ZFUZZ_ZPAGES"));
                    census_gate(&r, false);
                    st.reopens += 1;
                    let probe = all_probe(&pool, &absent);
                    let how = if clean { "a close" } else { "a crash" };
                    note_event(format!("reopen after {how}"), &z, &r);
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
                    explain_scan(&format!("{name} scan"), d, &a, &want);
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
            commit_both(&mut z, &mut r, &mut model, &mut staged)?;
            if rng.chance(75) {
                z.settle().map_err(|e| format!("settle z: {e}"))?;
                note_event("check, z settled", &z, &r);
            } else {
                note_event("check", &z, &r);
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
                    explain_scan("z scan", &z, &a, &want);
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
                    explain_scan("r scan", &r, &b, &want);
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
        commit_both(&mut z, &mut r, &mut model, &mut staged)?;
        z.settle().map_err(|e| format!("settle z: {e}"))?;
        r.settle().map_err(|e| format!("settle r: {e}"))?;
        note_event("check, final, both settled", &z, &r);
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
    let mut ok = fails.is_empty();
    drop(handles);
    st.abandons += z.upkeep_abandons() + r.upkeep_abandons();
    st.bindings += z.cache_bindings() + r.cache_bindings();
    st.beside += z.seal_waits().merges_beside + r.seal_waits().merges_beside;
    st.switches += z.forms_off_counts()[..2].iter().sum::<u64>();
    st.kept += z.snapshots_kept();
    st.recuts += z.forms_recut().0 + r.forms_recut().0;
    count_point(&mut st, &z);
    count_point(&mut st, &r);
    count_heads(&mut st, &z);
    count_heads(&mut st, &r);
    st.stride.add(&z.zpage_counts());
    // A form file the store refused, took out of service or failed to
    // build answers nothing wrong -- the raw form answers in its place --
    // so the model cannot see it; under the stride form each is a defect
    // of the form the seed was steered to reach.
    if cfg.stride.is_some() && st.stride.failures > 0 && ok {
        fail(
            &mut fails,
            format!(
                "z's form files failed {} times: {}",
                st.stride.failures, st.stride
            ),
        );
        ok = false;
    }
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
    if env_flag("ZFUZZ_STRIDE") {
        assert!(
            env_flag("ZFUZZ_ZPAGES"),
            "ZFUZZ_STRIDE forces the stride form on the store whose form is forced: set ZFUZZ_ZPAGES"
        );
        assert!(
            !env_flag("ZFUZZ_BOTH"),
            "ZFUZZ_STRIDE beside ZFUZZ_BOTH: the store refuses the stride form beside the re-code"
        );
    }
    let (mut bad, mut reopens, mut abandons, mut bindings, mut beside, mut switches) =
        (Vec::new(), 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut recuts = 0u64;
    let mut kept = 0u64;
    let mut point = [0u64; 12];
    let mut heads = [0u64; 2];
    let mut stride = StrideStats::default();
    let ran = seeds.len();
    for seed in seeds {
        match run_seed(seed, want_cache, verbose) {
            Ok(s) => {
                if verbose && stride_on() {
                    eprintln!("seed {seed}: {}", s.stride);
                }
                stride.merge(&s.stride);
                reopens += s.reopens;
                abandons += s.abandons;
                bindings += s.bindings;
                beside += s.beside;
                switches += s.switches;
                recuts += s.recuts;
                kept += s.kept;
                for (a, b) in point.iter_mut().zip(s.point) {
                    *a += b;
                }
                heads[0] += s.heads[0];
                heads[1] += s.heads[1];
            }
            Err(e) => bad.push(format!("seed {seed}: {e}")),
        }
    }
    eprintln!(
        "seeds {from}..{to}: reopens {reopens}, abandons {abandons}, bindings {bindings}, merges beside {beside}, forms-off switches {switches}, sparse forms recut {recuts}, heads owned {} given back {}, point parts {} admits {} key hits {} walk owned {} walk parts {} hot starts {} copy starts {} stale exits {} cursor steps {} bounds by head {} landings left {} quiet merges {}, snapshots kept {kept}, FAILURES {}",
        heads[0],
        heads[1],
        point[0],
        point[1],
        point[2],
        point[3],
        point[4],
        point[5],
        point[6],
        point[7],
        point[8],
        point[9],
        point[10],
        point[11],
        bad.len()
    );
    if stride_on() {
        // Summed over the seeds, the files as each seed's most at once.
        eprintln!("seeds {from}..{to}: {stride}");
        // A steer that reaches no partition passes every check over the
        // block form alone, so a range in which no stride file answered a
        // read is a run that did not test what it was asked to.
        if stride.reads == 0 && ran > 0 {
            bad.push(format!(
                "the stride form served no read in {ran} seeds: the steer reached no partition ({stride})"
            ));
        }
        // Asked for the sparse form, a range that built none read no key
        // by a search of its numbers.
        if sparse_on() && stride.sparse == 0 && ran > 0 {
            bad.push(format!(
                "no sparse stride file was built in {ran} seeds: the steer made no run with gaps ({stride})"
            ));
        }
        // And asked to move its partitions between the forms, a range in
        // which none left the stride form moved nothing.
        if convert_on() && stride.converts_back == 0 && ran > 0 {
            bad.push(format!(
                "no partition left the stride form in {ran} seeds: the asks reached none ({stride})"
            ));
        }
    }
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

/// A concurrent seed's store, run with its readers on threads: `Ok` with the
/// point reads its stride files answered.
fn run_concurrent(seed: u64, secs_ops: usize, snapshots: bool) -> Result<(u64, u64), String> {
    let mut rng = Rng::new(seed ^ 0xC0FFEE);
    let cfg = pick_cfg(&mut rng, false);
    let dir = scratch(&format!("c{seed}"));
    let heads_flip = env_flag("ZFUZZ_HEADSFLIP") && env_flag("ZFUZZ_ZPAGES");
    let convert = convert_on();
    // Under `ZFUZZ_STRIDE` the store forced with the stride form under it,
    // and the writes shaped so that every partition qualifies: one run of
    // numbers at one width, every key written before the readers start, and
    // a key's later writes puts of one value of one width -- the value its
    // count of puts -- with no delete, which would leave a hole.
    let stride = stride_on();
    let made = if heads_flip || stride {
        opts_z(&cfg)
    } else {
        opts(&cfg)
    };
    let mut db = Db::create(&dir, made).map_err(|e| format!("create: {e}"))?;
    census_gate(&db, heads_flip || stride);
    let nkeys = 150 + rng.below(250);
    let keys: Arc<Vec<Vec<u8>>> = Arc::new(
        (0..nkeys)
            .map(|i| {
                if stride {
                    format!("c{i:05}").into_bytes()
                } else if i % 7 == 0 {
                    format!("c{:03}-long-key-{}", i, "x".repeat(i % 20)).into_bytes()
                } else {
                    format!("c{:05}", i * 3).into_bytes()
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    );
    // Per key: the (epoch, n) the writer has committed, and has appended;
    // under the stride form n is how many times the key was put.
    let committed: Arc<Vec<AtomicU64>> =
        Arc::new((0..keys.len()).map(|_| AtomicU64::new(0)).collect());
    let appended: Arc<Vec<AtomicU64>> =
        Arc::new((0..keys.len()).map(|_| AtomicU64::new(0)).collect());
    let mut epoch = vec![0u32; keys.len()];
    let mut n = vec![0u32; keys.len()];
    if stride {
        for (i, k) in keys.iter().enumerate() {
            n[i] = 1;
            db.put(k, &cvalue(0, 1, STRIDE_PAD));
        }
        db.commit().map_err(|e| format!("commit: {e}"))?;
        db.flush().map_err(|e| format!("flush: {e}"))?;
        db.zpages_settle()
            .map_err(|e| format!("zpages settle: {e}"))?;
        for i in 0..keys.len() {
            committed[i].store(pack(0, 1), Ordering::SeqCst);
            appended[i].store(pack(0, 1), Ordering::SeqCst);
        }
    }
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
            let mut turn = 0u64;
            while !stop.load(Ordering::Relaxed) {
                // Under `ZFUZZ_HEADSFLIP` a short scan every few reads: a
                // scan's start seeks its partition's ordered index, whose
                // heads the flips copy and give back beside it, and the
                // store's scans are what the copies wait for. Held to
                // order alone, the values being the writer's to move.
                turn += 1;
                if heads_flip && turn.is_multiple_of(8) {
                    let from = &keys[(turn as usize / 8) % keys.len()];
                    let mut prev: Option<Vec<u8>> = None;
                    let mut bad = None;
                    let done = h.scan(from, 8, |k, _| {
                        if k < from.as_slice() || prev.as_deref().is_some_and(|p| p > k) {
                            bad = Some(k.to_vec());
                        }
                        prev = Some(k.to_vec());
                    });
                    if let Err(e) = done {
                        errs.lock().unwrap().push(format!("thread {t}: scan: {e}"));
                        return;
                    }
                    if let Some(k) = bad {
                        errs.lock().unwrap().push(format!(
                            "thread {t}: a scan from {} out of order at {}",
                            show(from),
                            show(&k)
                        ));
                        return;
                    }
                }
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
                // The values: one epoch, indexes 0..n, in order; under the
                // stride form one value, whose index is the key's count of
                // puts.
                let mut epoch = None;
                if stride && vals.len() > 1 {
                    errs.lock().unwrap().push(format!(
                        "thread {t}: key {}: {} values of a key only ever put",
                        show(&keys[i]),
                        vals.len()
                    ));
                    return;
                }
                for (j, v) in vals.iter().enumerate() {
                    if v.len() < 8 {
                        errs.lock().unwrap().push(format!("thread {t}: short value"));
                        return;
                    }
                    let (e, idx) = parse_c(v);
                    if epoch.is_some_and(|x| x != e) || (!stride && idx as usize != j) {
                        errs.lock().unwrap().push(format!(
                            "thread {t}: key {}: values out of shape at {j}: epoch {e} idx {idx} (first epoch {epoch:?})",
                            show(&keys[i])
                        ));
                        return;
                    }
                    epoch = Some(e);
                }
                let got = match vals.first() {
                    None => None,
                    Some(v) if stride => Some(pack(epoch.unwrap(), parse_c(v).1)),
                    Some(_) => Some(pack(epoch.unwrap(), vals.len() as u32)),
                };
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
    let mut staged: Vec<usize> = Vec::new();
    for step in 0..secs_ops {
        if heads_flip && step.is_multiple_of(61) {
            db.zgate_set(if (step / 61) % 2 == 0 { 1 } else { 2 });
        }
        if convert && step.is_multiple_of(53) {
            db.zpages_convert_ask(step / 53);
        }
        let i = rng.below(keys.len());
        if stride {
            n[i] += 1;
            db.put(&keys[i], &cvalue(epoch[i], n[i], STRIDE_PAD));
        } else if rng.chance(4) {
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
    let (owned, given, _) = db.heads_owned_counts();
    let c = db.zpage_counts();
    if stride && (c.fallbacks, c.damaged, c.build_failed) != (0, 0, 0) {
        return Err(format!(
            "seed {seed} ({cfg:?}): the form files failed: {c:?}"
        ));
    }
    eprintln!(
        "concurrent seed {seed}: reads {}, heads owned {owned} given back {given}, stride files {} builds {} reads {}, moved in {} out {}",
        reads_done.load(Ordering::Relaxed),
        c.stride_files,
        c.stride_builds,
        c.stride_reads,
        c.stride_converts,
        c.stride_converts_back
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok((c.stride_reads, c.stride_converts_back))
}

/// The padding of a concurrent seed's values under the stride form: one
/// width for every value, the epoch and the count before it.
const STRIDE_PAD: usize = 8;

#[test]
fn concurrent_range() {
    let from = env_u64("ZFUZZ_FROM").unwrap_or(0);
    let to = env_u64("ZFUZZ_TO").unwrap_or(from + 3);
    let ops = env_u64("ZFUZZ_OPS").unwrap_or(4000) as usize;
    let snaps = env_flag("ZFUZZ_SNAPS");
    if env_flag("ZFUZZ_STRIDE") {
        assert!(
            env_flag("ZFUZZ_ZPAGES"),
            "ZFUZZ_STRIDE forces the stride form on the store whose form is forced: set ZFUZZ_ZPAGES"
        );
        assert!(
            !env_flag("ZFUZZ_BOTH"),
            "ZFUZZ_STRIDE beside ZFUZZ_BOTH: the store refuses the stride form beside the re-code"
        );
    }
    let mut bad = Vec::new();
    let (mut stride_reads, mut moved_out) = (0u64, 0u64);
    for seed in from..to {
        match std::panic::catch_unwind(|| run_concurrent(seed, ops, snaps)) {
            Ok(Ok((reads, out))) => {
                stride_reads += reads;
                moved_out += out;
            }
            Ok(Err(e)) => bad.push(e),
            Err(p) => bad.push(format!("seed {seed}: PANIC {}", panic_text(p.as_ref()))),
        }
    }
    // As the sequential range's: a run whose stride files answered nothing
    // passed over the block form alone and tested nothing it was asked to.
    if stride_on() && stride_reads == 0 && to > from {
        bad.push(format!(
            "the stride form served no read in seeds {from}..{to}"
        ));
    }
    if convert_on() && moved_out == 0 && to > from {
        bad.push(format!(
            "no partition left the stride form in seeds {from}..{to}"
        ));
    }
    for b in &bad {
        eprintln!("FAIL: {b}");
    }
    assert!(bad.is_empty(), "{} seeds failed", bad.len());
}
