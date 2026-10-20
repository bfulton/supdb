//! The selector: which form a partition is read from, chosen from what the
//! store measures about itself and from nothing else.
//!
//! A partition may carry a second, compact form of its records beside the raw
//! one: groups of records that each compress into one page. Neither form is
//! better in itself. The compact form costs CPU on every read and removes page
//! faults; the raw form is the reverse; which one pays depends on whether the
//! raw form's footprint fits the cache the store really has, and that is a
//! property of the machine and the workload together. The choice is made per
//! workload and never per operation, because two forms that are both read
//! move each other's cliff to their sum (`ordindex`), so it is one
//! store-level word, the gate, and this file is the controller that sets it.
//!
//! **This file is the one definition of the transitions.** [`ZSelect::step`]
//! is a function of its state and its [`Input`]: it reads no clock (time is
//! [`Input::now`]), starts no thread, opens no file and names no engine type,
//! so a test feeds it a trace and reads the decisions back, and the engine's
//! tick calls the same code. Everything it wants done comes back in the
//! [`Decision`]; everything it knows came in through the `Input`.
//!
//! # The phases
//!
//! | phase | gate | meaning |
//! |---|---|---|
//! | [`Phase::Fits`] | 0 | the model says the raw form fits the cache; nothing runs, nothing is sampled |
//! | [`Phase::Suspect`] | 1 | the model says it may not; handles run the census; reads are served raw |
//! | [`Phase::Building`] | 1 | the price is paid and the form files are being built or attached; reads are served raw |
//! | [`Phase::Compact`] | 2 | covered partitions are read from the compact form |
//! | [`Phase::Trial`] | 1 | COMPACT's probe: the raw form is served again, form files attached, to see whether it fits |
//!
//! The table's three rungs are FITS, SUSPECT (with the build inside it) and
//! COMPACT (with the trial inside it); the five phases are what the gate and
//! the builder need to tell apart.
//!
//! # Two signals and a clock
//!
//! **S1, the model**, arrives on the first step of each [`S1_PERIOD`] in
//! every phase: the raw form's predicted faults a read (`phi_raw`), the
//! compact form's (`phi_z`, once a sampler has said how large it is), and the
//! group's refault rate. It is out of band and free, and it decides whether
//! anything is counted at all. A reading that comes less than a period (less
//! one census interval) after the last is kept for the rules and is not a
//! tick, so the ticks that FITS, SUSPECT and the smoothing count in are
//! seconds whatever the caller's cadence.
//!
//! **S2, the census**, arrives on every step outside FITS (the handles run
//! none in FITS, and a census, a sampler's reading or a footprint that comes
//! then is not read): the operations the handles ran and the major faults
//! they took, per class (point reads and counts; scans), summed over the
//! handles' valid windows. A major fault is the one thing the engine cannot
//! see from inside a lookup, and it is counted from the thread's own counters
//! and not timed. S1 only decides whether to count; S2 decides whether to
//! switch. A skewed workload on a store far larger than its cache is SUSPECT
//! at the price of a countdown and never builds, because its census shows no
//! capacity misses. The census times the form the gate serves, so the cost of
//! a hit is taken from it only while the raw form serves, and the cost of a
//! compact read only while the compact form does.
//!
//! **The group's refaults** are the platform's counter for the whole group
//! and the store's own refaults are in it, whichever form serves them: the
//! selector is not told how many are the store's and subtracts none. The
//! pressure term (over [`PRESSURE_REFAULTS_PER_S`]) therefore says the group
//! is refaulting, not that a neighbour is, and it moves with the gate, since
//! the compact form takes away the refaults the raw form made. A store that
//! reads fast enough to buy the form is over it by its own refaults alone:
//! the evidence needs half of its faults to be refaults, and buying needs a
//! saving of at least the entry threshold in faults a read. So the term opens
//! the build clause (the measured saving still decides) and keeps E1 from
//! acting while it is on, since the model's word is in doubt then; E3 is not
//! stopped by it, because the compact form's own faults are among the
//! refaults it counts. The model's word that the store fits is checked
//! against the raw form's measured faults when the compact form takes over
//! (see the bullet on the model's error), and a form bought against that
//! word is left by E3 or by a trial.
//!
//! The caller steps once per [`CENSUS_INTERVAL`]. The rings that hold a
//! window of history are sized for that cadence; a caller that steps faster
//! shortens the window to their capacity and changes no rule.
//!
//! # The transitions
//!
//! | from | to | condition |
//! |---|---|---|
//! | FITS | SUSPECT | `phi_raw >= theta_sus` or the pressure term, on [`FITS_TO_SUSPECT_TICKS`] consecutive S1 ticks |
//! | SUSPECT | FITS | `phi_raw < theta_sus / 2` and no pressure, on [`SUSPECT_TO_FITS_TICKS`] consecutive ticks |
//! | SUSPECT | building | all of (a) `excess >= C_build`; (b) the predicted saving `phi_raw - phi_z >= theta_enter`, unless the pressure term is on, and the measured saving `phi_meas - phi_z >= theta_enter` in some class; (c) `phi_raw >= theta_sus` or the pressure term; (d) a [`ZSelect::dwell`] since the last switch; (e) the sampler has a reading of the partitions as they are, has not declined them, and the wasted-build budget has not suspended building |
//! | building | SUSPECT | the measured saving is under `theta_enter / 2` in every class with evidence, on [`VANISH_INTERVALS`] evidence intervals; or the budget trips; or the sampler declines the partitions. The build is cancelled and **counts as a switch** |
//! | building | COMPACT | form files attached for at least [`COVERED_ENOUGH`] of the covered footprint |
//! | COMPACT | SUSPECT | **E1** the maximum of the smoothed `phi_raw`, with the part of the model's error that is positive added, over a dwell is under `theta_sus / 2`, the pressure term is off throughout, and the scans have not been held against the model; **E3** the realized saving, the raw form's believed rate less `phi_meas_z`, stays under `saving*` for a dwell; **E2** the trial says the raw form fits |
//! | COMPACT | trial | the z faults a read are under `theta_sus` and the raw footprint of the pages touched is under half the budget, on [`E2_MEASURES`] consecutive measures of [`E2_MEASURE`], a dwell since the last switch, and the back-off has run |
//! | trial | COMPACT | the refilled raw form still faults: its saving over the compact form's predicted rate, `phi_meas - phi_z`, is at least `theta_enter`. The back-off doubles |
//! | trial | SUSPECT | it does not: E2. The form files are retained |
//!
//! `saving* = (t_z - t_hit) / t_f` is the fall in faults an operation at which
//! the compact form's extra CPU is exactly repaid by the faults it removes,
//! one per class. Every threshold is a multiple of it, so nothing here is a
//! rate typed in: the costs are measured by the census and the sampler, and
//! [`Costs::PRIOR`] stands in only until they are. The extra CPU is never
//! taken as less than [`SAVING_FLOOR_SHARE`] of the prior's.
//!
//! # What each rule is for
//!
//! * **The entry is a price, not a streak.** A build is seconds of CPU and a
//!   burst of uniform reads on a skewed store is fractions of one, so a count
//!   of intervals cannot say when to buy. `excess` is the thread-seconds the
//!   form is predicted to have saved net of its extra CPU since the rung
//!   entered SUSPECT, and the form is bought when it equals the price, so the
//!   loss is at most twice the price whenever the workload stops. It cannot go
//!   below zero: a stretch where the form would have lost does not owe the
//!   next stretch anything, which is the cumulative-sum statistic and not a
//!   running balance. It halves for every dwell without evidence, so a stale
//!   rent is not a credit.
//! * **Evidence is a capacity miss, not a first touch.** After a restart every
//!   first touch is a major fault and the rate reads as a footprint loss until
//!   the hot set is resident. An interval counts only if the trailing
//!   [`REFAULT_WINDOW`] shows at least [`REFAULT_SHARE`] of the store's faults
//!   to be pages the cache once held, and holds [`MIN_EVIDENCE_OPS`] valid
//!   operations of the class.
//! * **A switch must pay back what it cost.** At most one switch in a dwell,
//!   where a switch is a build installed, a build cancelled, or COMPACT left,
//!   and the dwell is a multiple of the last build's wall time, so a build is
//!   repaid inside it. A cancelled build is a switch for this rule. The trial
//!   is not: it is the way out of COMPACT, bounded by its own back-off.
//! * **Hysteresis is the gap between the way in and the way out.** SUSPECT is
//!   entered at `theta_sus` and left at half of it, a build starts at
//!   `theta_enter` and is cancelled at half of it, and COMPACT is left only
//!   after a signal has stayed on its side for a dwell.
//! * **A controller facing constant inputs comes to rest.** Whatever the
//!   inputs depend on, the gate included, a workload that does not change
//!   makes no switch after the first dwells: a form that pays stays, one that
//!   does not is not kept, and the raw form is tried at the trial's back-off
//!   and no oftener. The tests hold this over a grid of caches, neighbours,
//!   threads and loads, with the group's refaults as the group has them.
//! * **The model's error is kept.** The model prices this store against the
//!   whole budget, and another reader of the group takes cache it does not
//!   subtract. When the compact form takes over, and again when a trial finds
//!   the raw form still faulting, the raw form's measured faults a read (the
//!   point reads of the intervals that counted as evidence, over the trailing
//!   window) are set against the model's word; where they differ by at least
//!   `theta_sus` the difference is the model's error, and it is kept for as
//!   long as the form stands. The compact form is judged against the model's
//!   word corrected by it: E3 reads the corrected rate, so a form is kept
//!   while it beats the raw form as measured and left when it does not,
//!   whether the model said more or less than the census saw, and E1 adds the
//!   positive part to the model's word, so a model that said the store fits
//!   while the raw form faulted is not believed that it fits once the form
//!   has removed the refaults that showed it. Scans have no rate in the
//!   model, only its word that the store fits: scans that fault at
//!   `theta_sus` of their class against that word keep E1 from acting, and
//!   E3, which reads the point reads, has nothing to say of a store that only
//!   scans.
//! * **A form E3 left is not bought again on the model's word.** The faults
//!   the form took through the stretch E3 acted on stand as its rate, where
//!   they are above the model's, until it is installed again or the store is
//!   FITS; the build clause and the evidence price the form at that.
//! * **The trial judges the refilled state.** The raw form's hot pages are
//!   cold after the compact form displaced them, and a cold fill faults at
//!   one a read whatever the hot set, so the trial waits for its faults to
//!   reach the pages the raw form needs, or for the time they take, or for the
//!   rate to settle under the threshold, and only then takes three evidence
//!   intervals as the verdict. A trial that has no evidence to decide on is
//!   undone after a dwell, since the compact form it came from is attached
//!   and the raw form must not be left serving a store nothing reads.
//! * **A reading is of what it was taken of.** The sampler's reading carries
//!   the generation of the partitions it was taken at and is dropped when they
//!   change; it is dropped when the form files it priced are retired; and a
//!   decline stands for a dwell, after which the sampler is asked again. The
//!   footprint is the compact form's bitmaps, and goes when the form does. A
//!   sampler that declines the partitions while a build is under way ends it.
//! * **The budget counts wasted builds.** A store written across its key
//!   space rewrites every partition each cycle, at a rate that can exceed one
//!   serial builder, so each rewrite supersedes the build of the partition it
//!   replaced. When more than [`BUDGET_SUPERSEDED`] of the last
//!   [`BUDGET_BUILDS`] were superseded, building is held off for a dwell and
//!   the form is served as it stands.
//!
//! The model's arithmetic (S1) is here too, as pure functions of byte counts:
//! [`model_raw`], [`model_z`] and [`ski_rental_price`].

use std::time::Duration;

// ---- the gate ----------------------------------------------------------

/// The gate word that serves the raw form and runs nothing.
pub const GATE_RAW: u8 = 0;
/// The gate word that serves the raw form with the census running.
pub const GATE_CENSUS: u8 = 1;
/// The gate word that serves the compact form where a partition has one.
pub const GATE_Z: u8 = 2;

// ---- thresholds: multiples of the break-even saving ---------------------

/// `theta_enter = ENTER_FACTOR * saving*`: the fall in faults an operation at
/// which the form is bought. The form's extra CPU is repaid exactly at
/// `saving*`; half again as much covers the model's error and the build's
/// one-time cost. It is tested on the saving (`phi_raw - phi_z`), because a
/// rate alone cannot tell a form that removes every fault from one that
/// removes few.
pub const ENTER_FACTOR: f64 = 1.5;
/// `theta_sus = SUSPECT_FACTOR * saving*`: the model's rate at which counting
/// the faults is worth the countdown's few instructions a read.
pub const SUSPECT_FACTOR: f64 = 0.5;
/// SUSPECT is left, and COMPACT's E1 fires, only below `theta_sus *
/// RELAX_FACTOR`: the gap between the way in and the way out.
pub const RELAX_FACTOR: f64 = 0.5;
/// A build is cancelled only when the measured saving is under `theta_enter *
/// VANISH_FACTOR`: the gap between starting a build and abandoning it.
pub const VANISH_FACTOR: f64 = 0.5;
/// The price of a build is its CPU plus the displacement it causes, and until
/// the displacement is measured it is this share of the CPU
/// ([`ski_rental_price`]).
pub const DISPLACEMENT_FACTOR: f64 = 0.5;
/// The compact form reads a page-sized group, so it costs more than a hit by
/// at least a share of what the priors say it does ([`PRIOR_T_Z`] less
/// [`PRIOR_T_HIT`]); `saving*` is made of the measured difference but never of
/// less than this share of the prior's. Two instruments measure the two costs
/// and a difference they cannot resolve is not a free form: without the floor
/// a form measured to cost no more than a hit makes every threshold zero,
/// and a model that reads exactly zero for a store that fits would be hot.
pub const SAVING_FLOOR_SHARE: f64 = 0.25;

// ---- evidence ----------------------------------------------------------

/// The share of the store's major faults that must be refaults (pages the
/// cache once held) for an interval to count as evidence: a capacity miss is
/// a refault, a first touch is not.
pub const REFAULT_SHARE: f64 = 0.5;
/// The trailing window the refault share is taken over.
pub const REFAULT_WINDOW: Duration = Duration::from_secs(5);
/// The valid operations of one class an interval must hold before its rate
/// says anything: below it a rate is a handful of faults and not a measure.
pub const MIN_EVIDENCE_OPS: u64 = 64;
/// The group's refaults a second above which the pressure term is on: the
/// group is refaulting, the store's own faults and other readers' alike, and
/// the model, which prices this store against the whole budget, cannot say
/// that the store fits.
pub const PRESSURE_REFAULTS_PER_S: f64 = 256.0;

// ---- cadence and hysteresis in ticks ------------------------------------

/// How often the model is read: it moves with the limit and the anonymous
/// memory, which move in seconds. The caller sets [`Input::model`] on the
/// first step of each period.
pub const S1_PERIOD: Duration = Duration::from_secs(1);
/// How often the census is summed and the transitions evaluated: the caller
/// steps once per interval.
pub const CENSUS_INTERVAL: Duration = Duration::from_millis(100);
/// Consecutive S1 ticks of a hot model before FITS becomes SUSPECT.
pub const FITS_TO_SUSPECT_TICKS: u32 = 2;
/// Consecutive S1 ticks of a calm model before SUSPECT becomes FITS.
pub const SUSPECT_TO_FITS_TICKS: u32 = 5;
/// Consecutive evidence intervals in which the saving has vanished before a
/// build in flight is cancelled.
pub const VANISH_INTERVALS: u32 = 3;
/// The share of the covered footprint bytes whose form files must be attached
/// for building to become COMPACT (declined partitions are excluded from the
/// footprint by the caller).
pub const COVERED_ENOUGH: f64 = 0.9;
/// The span the model's readings are smoothed over before E1 reads them: the
/// reading moves by a third for a few hundred megabytes of anonymous memory.
pub const E1_SMOOTHING: Duration = Duration::from_secs(30);

// ---- the dwell, the retention, the trial --------------------------------

/// The shortest dwell: a switch must pay back what it cost, and a build of
/// seconds is repaid in a minute.
pub const DWELL_FLOOR: Duration = Duration::from_secs(60);
/// The dwell is this many times the last build's wall time when that is more
/// than [`DWELL_FLOOR`].
pub const DWELL_BUILD_FACTOR: u32 = 10;
/// The shortest time a form file is kept after the form is left, while a
/// re-entry is likelier to be cheaper than a rebuild.
pub const RETAIN_FLOOR: Duration = Duration::from_secs(600);
/// The retention is this many times the last build's wall time when that is
/// more than [`RETAIN_FLOOR`].
pub const RETAIN_BUILD_FACTOR: u32 = 20;
/// The back-off between one trial's end and the next one's start, until a
/// wrong trial or a cancelled build doubles it: a trial that is wrong costs
/// the hot set's refault, a few seconds of one thread, and a gap this long
/// bounds that to under one percent of the time.
pub const TRIAL_BACKOFF_MIN: Duration = Duration::from_secs(10 * 60);
/// The back-off doubles on every wrong trial or cancelled build, up to this.
pub const TRIAL_BACKOFF_MAX: Duration = Duration::from_secs(4 * 60 * 60);
/// The length of one E2 measure: the raw footprint of the pages touched is
/// read this often.
pub const E2_MEASURE: Duration = Duration::from_secs(10);
/// Consecutive E2 measures that must say the raw form would fit before a
/// trial starts.
pub const E2_MEASURES: u32 = 3;
/// The raw form would fit with room to spare when the footprint of the pages
/// touched is under this share of the budget.
pub const E2_BOUND_SHARE: f64 = 0.5;
/// The trial is judged once its faults reach this multiple of the raw pages
/// it expects to need: the cold fill is over, whatever the rate.
pub const TRIAL_FAULT_FACTOR: f64 = 1.2;
/// The trial is judged once the rate has been under `theta_enter` for this
/// many evidence intervals running: the fill settled.
pub const TRIAL_LOW_INTERVALS: u32 = 5;
/// The trial is judged once this multiple of the time the expected pages take
/// to fault in has passed, capped at [`TRIAL_WALL_CAP`].
pub const TRIAL_WALL_FACTOR: f64 = 3.0;
/// The most a trial waits for the fill.
pub const TRIAL_WALL_CAP: Duration = Duration::from_secs(30);
/// Evidence intervals the verdict is pooled over once the trial is judged.
pub const TRIAL_DECIDE_INTERVALS: u32 = 3;

// ---- the wasted-build budget --------------------------------------------

/// The builds the budget remembers.
pub const BUDGET_BUILDS: u32 = 8;
/// Building is suspended for a dwell when MORE than this many of the
/// remembered builds were superseded (their target retired by a merge before
/// they installed): a store written across its key space rewrites every
/// partition each cycle, at a rate that can exceed one serial builder.
pub const BUDGET_SUPERSEDED: u32 = 4;
/// The bits of the outcomes word that are the budget's window.
const BUDGET_WINDOW: u32 = if BUDGET_BUILDS >= u32::BITS {
    u32::MAX
} else {
    (1u32 << BUDGET_BUILDS) - 1
};
const _: () = assert!(BUDGET_BUILDS >= 1 && BUDGET_BUILDS <= u32::BITS);

// ---- the unit the model counts in ---------------------------------------

/// The page the cache and the model are counted in.
pub const PAGE: u64 = 4096;

// ---- priors -------------------------------------------------------------

/// The cost of a major fault, seconds, until the census or the sampler has
/// measured it: what the rates and faults a read of the banked out-of-core
/// rung imply of each arm, `(1 / rate - t_hit) / faults` (`docs/engine.md`,
/// "Out of core"), taken between the two.
pub const PRIOR_T_F: f64 = 41.5e-6;
/// The cost of a point read that hits, seconds: the arm's read of a store its
/// cache holds whole, from the in-core rung.
pub const PRIOR_T_HIT: f64 = 0.73e-6;
/// The cost of a point read through the compact form, seconds: the decode and
/// the walk of a group, a cold page's allowance, and the fixed cost.
pub const PRIOR_T_Z: f64 = 4.4e-6;
/// The cost of a scan of a hundred that hits, seconds.
pub const PRIOR_T_HIT_SCAN: f64 = 3.3e-6;
/// The cost of a scan of a hundred through the compact form, seconds.
pub const PRIOR_T_Z_SCAN: f64 = 11.3e-6;

// ---- S1's arithmetic ----------------------------------------------------

/// The raw form's faults a point read, by Che's approximation to LRU over the
/// two populations a read touches uniformly: one page of the hash region and
/// one page of the record region.
///
/// `T` solves `n_h (1 - e^(-T/n_h)) + n_r (1 - e^(-T/n_r)) = B / 4096` for
/// `n_h = hash / 4096` and `n_r = records / 4096`, and the rate is
/// `e^(-T/n_h) + e^(-T/n_r)`. It is 0 while both fit, and it exceeds 1 once
/// the hash stops fitting: a model with one population saturates at one fault
/// a read, and a store far past its cache would then look as if the compact
/// form saved nothing, when the raw form is taking two faults a read.
pub fn model_raw(budget: u64, hash: u64, records: u64) -> f64 {
    let page = PAGE as f64;
    let (c, nh, nr) = (
        budget as f64 / page,
        hash as f64 / page,
        records as f64 / page,
    );
    if nh + nr <= c {
        return 0.0;
    }
    let populations = f64::from(u8::from(nh > 0.0)) + f64::from(u8::from(nr > 0.0));
    if c <= 0.0 {
        return populations;
    }
    let filled = |t: f64| {
        let part = |n: f64| {
            if n > 0.0 {
                n * (1.0 - (-t / n).exp())
            } else {
                0.0
            }
        };
        part(nh) + part(nr)
    };
    // The bracket: `filled` rises from 0 to nh + nr, which is over `c`.
    let (mut lo, mut hi) = (0.0f64, c.max(1.0));
    for _ in 0..200 {
        if filled(hi) >= c {
            break;
        }
        lo = hi;
        hi *= 2.0;
    }
    for _ in 0..64 {
        let mid = 0.5 * (lo + hi);
        if filled(mid) < c {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = 0.5 * (lo + hi);
    let miss = |n: f64| if n > 0.0 { (-t / n).exp() } else { 0.0 };
    (miss(nh) + miss(nr)).clamp(0.0, populations)
}

/// The compact form's faults a point read: one population read uniformly,
/// `1 - (B - f) / W_z` clamped to a rate, where `f` is the part of the form file
/// that is resident whatever the reads do (the fence and the filter) and `W_z`
/// the pages.
pub fn model_z(budget: u64, resident: u64, z_bytes: u64) -> f64 {
    if z_bytes == 0 {
        return 0.0;
    }
    let room = budget.saturating_sub(resident) as f64;
    (1.0 - room / z_bytes as f64).clamp(0.0, 1.0)
}

/// `C_build`, seconds: what buying the form costs, the CPU the build is
/// estimated to take plus the displacement of the cache it causes. Until the
/// displacement is measured it is [`DISPLACEMENT_FACTOR`] of the CPU. For a
/// form file already on disk with a matching identity the price is the attach's
/// cost, and the caller passes that instead.
pub fn ski_rental_price(build_cpu: f64, displacement: Option<f64>) -> f64 {
    let displacement = displacement
        .filter(|d| d.is_finite() && *d >= 0.0)
        .unwrap_or(DISPLACEMENT_FACTOR * build_cpu);
    build_cpu + displacement
}

/// Seconds as a duration, total: a negative or NaN count is zero and a count
/// beyond the range is the longest duration.
fn secs(x: f64) -> Duration {
    Duration::try_from_secs_f64(x).unwrap_or(if x > 0.0 {
        Duration::MAX
    } else {
        Duration::ZERO
    })
}

// ---- what the caller passes in ------------------------------------------

/// A class of operation. The rates and the break-evens are per class because
/// a scan of a hundred touches three pages where a read touches one, and a
/// store that is only scanned has no read windows at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Point reads and counts.
    Read,
    /// Scans.
    Scan,
}

impl Class {
    const ALL: [Class; 2] = [Class::Read, Class::Scan];

    fn ix(self) -> usize {
        self as usize
    }
}

/// The operations a class ran in one census interval and the major faults they
/// took, over the handles' valid windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    /// Valid operations.
    pub ops: u64,
    /// Major faults those operations took.
    pub faults: u64,
}

impl Window {
    fn add(&mut self, other: Window) {
        self.ops = self.ops.saturating_add(other.ops);
        self.faults = self.faults.saturating_add(other.faults);
    }

    /// Faults an operation; 0 over no operations.
    pub fn phi(self) -> f64 {
        if self.ops == 0 {
            0.0
        } else {
            self.faults as f64 / self.ops as f64
        }
    }
}

/// The costs `saving*` is made of, seconds. Each is measured by a different
/// instrument at a different time, so each is optional, and [`Costs::or`]
/// takes the best one there is: the census's, then the sampler's, then
/// [`Costs::PRIOR`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Costs {
    /// A major fault, as this machine charges it: `(d wall - d cpu) / d faults`
    /// over windows with no commit in them, or the sampler's timed cold
    /// touches where none has been seen.
    pub t_f: Option<f64>,
    /// A point read that hits: the least CPU a read has taken in any window
    /// the raw form served. A census that measured it under the compact form
    /// has measured something else, and the selector does not take it there.
    pub t_hit: Option<f64>,
    /// A scan that hits, as `t_hit`.
    pub t_hit_scan: Option<f64>,
    /// A point read through the compact form, measured in windows it served.
    pub t_z: Option<f64>,
    /// A scan through the compact form, as `t_z`.
    pub t_z_scan: Option<f64>,
}

impl Costs {
    /// The stand-in until anything is measured. FITS never samples, so this is
    /// what its threshold is made of.
    pub const PRIOR: Costs = Costs {
        t_f: Some(PRIOR_T_F),
        t_hit: Some(PRIOR_T_HIT),
        t_hit_scan: Some(PRIOR_T_HIT_SCAN),
        t_z: Some(PRIOR_T_Z),
        t_z_scan: Some(PRIOR_T_Z_SCAN),
    };

    /// The priors of a point read and a scan that hit, by [`Class`].
    const PRIOR_HIT: [f64; 2] = [PRIOR_T_HIT, PRIOR_T_HIT_SCAN];
    /// The priors of a point read and a scan through the compact form.
    const PRIOR_Z: [f64; 2] = [PRIOR_T_Z, PRIOR_T_Z_SCAN];

    /// Each field of `self` where it is a usable cost (finite and positive),
    /// else the field of `other`.
    pub fn or(self, other: Costs) -> Costs {
        let pick = |a: Option<f64>, b: Option<f64>| a.filter(|v| v.is_finite() && *v > 0.0).or(b);
        Costs {
            t_f: pick(self.t_f, other.t_f),
            t_hit: pick(self.t_hit, other.t_hit),
            t_hit_scan: pick(self.t_hit_scan, other.t_hit_scan),
            t_z: pick(self.t_z, other.t_z),
            t_z_scan: pick(self.t_z_scan, other.t_z_scan),
        }
    }
}

/// One reading of the model (S1), on the first step of each [`S1_PERIOD`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    /// The raw form's predicted faults a point read ([`model_raw`]).
    pub phi_raw: f64,
    /// The compact form's, once a sampler has said how large it is
    /// ([`model_z`]); `None` before.
    pub phi_z: Option<f64>,
    /// The compact form's predicted faults a scan, from the same size and the
    /// pages a scan touches; `None` before the sampler.
    pub phi_z_scan: Option<f64>,
    /// The group's refaults a second over the tick (`workingset_refault_file`
    /// where the platform has it); the pressure term compares it with
    /// [`PRESSURE_REFAULTS_PER_S`]. It is the group's as the platform counts
    /// it: the store's own refaults are in it, and none are subtracted, since
    /// the counter does not say whose they are.
    pub refault_rate: f64,
    /// The cache budget `B`, bytes: the limit less the anonymous memory less
    /// the margin.
    pub budget: u64,
}

impl Model {
    fn sanitized(self) -> Option<Model> {
        let ok = |x: f64| x.is_finite();
        if !(ok(self.phi_raw) && ok(self.refault_rate))
            || self.phi_z.is_some_and(|z| !ok(z))
            || self.phi_z_scan.is_some_and(|z| !ok(z))
        {
            return None;
        }
        Some(Model {
            phi_raw: self.phi_raw.max(0.0),
            phi_z: self.phi_z.map(|z| z.max(0.0)),
            phi_z_scan: self.phi_z_scan.map(|z| z.max(0.0)),
            refault_rate: self.refault_rate.max(0.0),
            budget: self.budget,
        })
    }
}

/// What the build sampler said about the partitions as they stand. The caller
/// sets [`Input::sampler`] after [`Decision::sample`] asks; the reading is of
/// the partitions at [`Sampler::generation`], and a change of
/// [`Input::generation`] is the selector's reason to ask again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampler {
    /// The sampler's cold estimates of the costs.
    pub costs: Costs,
    /// `C_build`, seconds ([`ski_rental_price`]); the attach's cost where the
    /// form files are already on disk.
    pub c_build: f64,
    /// The sampler declined the partitions: the form would not pay for its
    /// footprint, or the data does not compress.
    pub declined: bool,
    /// The [`Input::generation`] the reading was taken at: the partitions it
    /// is a reading of.
    pub generation: u64,
}

impl Sampler {
    fn sanitized(self) -> Option<Sampler> {
        (self.c_build.is_finite() && self.c_build >= 0.0).then_some(self)
    }
}

/// One census interval: S2, summed over the handles' valid windows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Census {
    /// Point reads and counts.
    pub reads: Window,
    /// Scans.
    pub scans: Window,
    /// Windows dropped as mixed (reads and scans together). Counted, not used.
    pub mixed: u64,
    /// Pages of the group's file cache that were read back after being
    /// reclaimed, over the interval; `None` where the platform has no counter,
    /// and the evidence rule then falls back to the faults taken since
    /// SUSPECT against the cache's size in pages. The group's, as
    /// [`Model::refault_rate`] is: a neighbour's refaults are counted, and
    /// the share the evidence rule takes is the group's refaults against the
    /// store's faults.
    pub refaults: Option<u64>,
    /// Threads that contributed valid windows: the trial's wall limit divides
    /// by it. 0 keeps the last non-zero count.
    pub threads: u32,
    /// The costs the interval measured (None where it measured none).
    pub costs: Costs,
}

impl Census {
    fn window(&self, class: Class) -> Window {
        match class {
            Class::Read => self.reads,
            Class::Scan => self.scans,
        }
    }
}

/// The builder's report to the selector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Build {
    /// The share of the covered footprint bytes whose form files are attached.
    pub coverage: f64,
    /// Builds collected since the last step that installed.
    pub installed: u32,
    /// Builds collected since the last step whose target a merge retired
    /// before they installed.
    pub superseded: u32,
    /// Builds collected since the last step that failed.
    pub failed: u32,
}

/// Everything one step knows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Time since an epoch the caller chose, from a monotonic clock. The only
    /// clock the selector reads; one that steps back is held at the latest.
    pub now: Duration,
    /// The model, on the first step of each [`S1_PERIOD`] and `None` between.
    pub model: Option<Model>,
    /// The sampler's reading, when the caller has a new one. The selector
    /// keeps it while it stands for the partitions as they are, and drops it
    /// when they change, when the form files it priced are retired, and, if it
    /// declined, after a dwell: each time [`Decision::sample`] asks for
    /// another. A reading taken at another [`Input::generation`] is of other
    /// partitions and is not kept.
    pub sampler: Option<Sampler>,
    /// The identity of what the sampler's reading is a reading of, an opaque
    /// number: it changes when a partition is made, rewritten or retired (a
    /// merge, a promotion) and when a form file is attached or retired, since
    /// the price of buying the form is what is built and what is not; it does
    /// not change for a piece a seal adds. It is compared for equality and
    /// nothing else, and the caller sets it at every step.
    pub generation: u64,
    /// This interval's census.
    pub census: Census,
    /// The builder's report.
    pub build: Build,
    /// E2's bound: the raw footprint, bytes, of the compact pages the handles
    /// have touched, from their bitmaps. The last reading is kept while the
    /// compact form stands, and goes with it: the bitmaps are of its pages.
    pub footprint: Option<u64>,
}

// ---- what comes back ----------------------------------------------------

/// The phase the selector is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The model says the raw form fits.
    Fits,
    /// The model says it may not; the census runs.
    Suspect,
    /// The price is paid; form files are being built or attached.
    Building,
    /// Covered partitions are read from the compact form.
    Compact,
    /// COMPACT's probe of the raw form.
    Trial,
}

impl Phase {
    /// The gate word this phase publishes.
    pub fn gate(self) -> u8 {
        match self {
            Phase::Fits => GATE_RAW,
            Phase::Compact => GATE_Z,
            Phase::Suspect | Phase::Building | Phase::Trial => GATE_CENSUS,
        }
    }

    /// Whether the handles run the census.
    pub fn census(self) -> bool {
        self != Phase::Fits
    }

    /// The phase as a number, for a gauge: FITS 0, SUSPECT 1, building 2,
    /// COMPACT 3, the trial 4.
    pub fn code(self) -> u8 {
        match self {
            Phase::Fits => 0,
            Phase::Suspect => 1,
            Phase::Building => 2,
            Phase::Compact => 3,
            Phase::Trial => 4,
        }
    }
}

/// Why COMPACT was left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The limit was raised or the store shrank: the model fits for a dwell.
    E1,
    /// The trial says the raw form fits.
    E2,
    /// The form is not paying: the realized saving stays under break-even.
    E3,
}

/// The transition a step made. At most one a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// FITS to SUSPECT.
    Suspect,
    /// SUSPECT to FITS.
    Relax,
    /// SUSPECT to building: the price is paid.
    BuildStart,
    /// Building to SUSPECT: the evidence vanished. A switch; the trial
    /// back-off doubles.
    BuildCancelled,
    /// Building to SUSPECT: the build was given up, because the wasted-build
    /// budget tripped (and building is suspended for a dwell) or because the
    /// sampler declined the partitions. A switch.
    BuildAbandoned,
    /// Building to COMPACT. A switch.
    Install,
    /// COMPACT: the wasted-build budget tripped and building is suspended for
    /// a dwell. The form is still served.
    BuildsSuspended,
    /// COMPACT to SUSPECT. A switch; the form files are retained.
    Exit(Exit),
    /// COMPACT to the trial: the gate goes back to the raw form.
    TrialStart,
    /// The trial to COMPACT: the raw form still faults.
    TrialBack,
}

impl Event {
    /// Whether this event counts as a switch for the dwell.
    pub fn is_switch(self) -> bool {
        matches!(
            self,
            Event::BuildCancelled | Event::BuildAbandoned | Event::Install | Event::Exit(_)
        )
    }
}

/// What a step wants done. The levels hold until the next decision; the
/// pulses (`retire`, `event`) are for this step alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The phase after this step.
    pub phase: Phase,
    /// The gate word to publish ([`Phase::gate`]). The builder's cells are
    /// installed before the step that moves it to [`GATE_Z`], so the gate is
    /// stored last.
    pub gate: u8,
    /// Whether handles run the census.
    pub census: bool,
    /// Whether the sampler's reading is wanted: SUSPECT without one of the
    /// partitions as they are.
    pub sample: bool,
    /// Whether uncovered partitions may be built: building, or COMPACT unless
    /// the wasted-build budget has suspended it.
    pub build: bool,
    /// Pulse: the retention ended with the form unused; drop the form files.
    pub retire: bool,
    /// Pulse: the transition this step made; a cancel is the event
    /// [`Event::BuildCancelled`] or [`Event::BuildAbandoned`].
    pub event: Option<Event>,
}

/// Counters of what the selector has done, for the engine's own counters and
/// for the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Steps taken.
    pub steps: u64,
    /// Model readings accepted.
    pub s1_ticks: u64,
    /// Times the sampler's reading was asked for.
    pub samples_wanted: u64,
    /// Builds started.
    pub builds: u64,
    /// Builds cancelled (the evidence vanished, or the budget tripped).
    pub build_cancelled: u64,
    /// Times the budget suspended building.
    pub build_suspended: u64,
    /// Builds that became COMPACT.
    pub installs: u64,
    /// Switches (installs, cancelled builds and exits).
    pub switches: u64,
    /// Exits from COMPACT, by [`Exit`] (E1, E2, E3).
    pub exits: [u64; 3],
    /// Trials started.
    pub trials: u64,
    /// Trials that went back to COMPACT.
    pub trials_back: u64,
    /// Retention windows that ended with the form files unused.
    pub retired: u64,
    /// Mixed windows the census dropped.
    pub mixed_windows: u64,
    /// Readings the selector refused (not finite).
    pub ignored: u64,
}

// ---- the state ----------------------------------------------------------

/// Why a build in flight was given up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cancel {
    /// The measured saving has gone.
    Vanished,
    /// The wasted-build budget tripped.
    Budget,
    /// The sampler declined the partitions.
    Declined,
}

/// A window of history: the values of the last stretch of time, oldest first,
/// in a fixed array. Holding `N` and no more means a caller that steps faster
/// than the cadence shortens the window and cannot grow the selector.
#[derive(Clone, Debug)]
struct Ring<T: Copy, const N: usize> {
    buf: [(Duration, T); N],
    head: usize,
    len: usize,
}

impl<T: Copy + Default, const N: usize> Ring<T, N> {
    fn new() -> Ring<T, N> {
        const { assert!(N > 0) };
        Ring {
            buf: [(Duration::ZERO, T::default()); N],
            head: 0,
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    fn push(&mut self, at: Duration, v: T) {
        if self.len == N {
            self.head = (self.head + 1) % N;
            self.len -= 1;
        }
        self.buf[(self.head + self.len) % N] = (at, v);
        self.len += 1;
    }

    /// Drops what is a horizon old or older.
    fn expire(&mut self, now: Duration, horizon: Duration) {
        while self.len > 0 && now.saturating_sub(self.buf[self.head].0) >= horizon {
            self.head = (self.head + 1) % N;
            self.len -= 1;
        }
    }

    fn iter(&self) -> impl Iterator<Item = &(Duration, T)> + '_ {
        (0..self.len).map(move |k| &self.buf[(self.head + k) % N])
    }

    /// The newest entry.
    fn last_mut(&mut self) -> Option<&mut (Duration, T)> {
        (self.len > 0).then(|| &mut self.buf[(self.head + self.len - 1) % N])
    }
}

/// What one census interval adds to the trailing window the evidence rule
/// and the raw form's measured rate read.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    /// The group's refaults over the interval.
    refaults: u64,
    /// The store's major faults over the interval, both classes.
    faults: u64,
    /// The point reads of the interval.
    reads: Window,
    /// The scans of the interval.
    scans: Window,
    /// The refault share held when the interval arrived: its faults were
    /// capacity misses, and the rate it measured is the raw form's and not a
    /// cold start's.
    counted: bool,
}

/// The refault window at the census cadence, with room to spare.
const SHARE_CAP: usize = 64;
/// The smoothing window at the S1 cadence, with room to spare.
const SMOOTH_CAP: usize = 40;

/// What one interval's census says, class by class.
#[derive(Clone, Copy, Debug, Default)]
struct Evidence {
    /// The class had at least [`MIN_EVIDENCE_OPS`] valid operations, the refault
    /// share held, and the model has a compact rate for it.
    valid: [bool; 2],
    /// The interval's operations and faults.
    win: [Window; 2],
    /// `phi_meas - phi_z`: the saving the compact form is measured to offer.
    saving: [f64; 2],
}

impl Evidence {
    fn any(&self) -> bool {
        self.valid[0] || self.valid[1]
    }
}

/// The costs resolved: every one has a value.
#[derive(Clone, Copy, Debug)]
struct Priced {
    t_f: f64,
    hit: [f64; 2],
    z: [f64; 2],
}

/// What COMPACT watches: E3's run, E2's measures.
#[derive(Clone, Debug)]
struct Watch {
    /// Since when the realized saving has stayed under `saving*`, every S1
    /// tick of the stretch having had evidence (E3).
    unpaying_since: Option<Duration>,
    /// The reads of that stretch, pooled: the faults the compact form really
    /// took while it was not paying.
    stretch: Window,
    /// Reads since the last S1 tick (E3's reading).
    tick_reads: Window,
    /// When the current E2 measure began.
    e2_start: Duration,
    /// The reads and scans of the current E2 measure.
    e2_win: [Window; 2],
    /// Consecutive E2 measures that said the raw form would fit.
    e2_run: u32,
}

impl Watch {
    fn new(now: Duration) -> Watch {
        Watch {
            unpaying_since: None,
            stretch: Window::default(),
            tick_reads: Window::default(),
            e2_start: now,
            e2_win: [Window::default(); 2],
            e2_run: 0,
        }
    }
}

/// A trial in flight.
#[derive(Clone, Debug)]
struct Trial {
    started: Duration,
    /// The raw pages the trial expects to fault in: E2's bound over a page.
    pages: u64,
    /// Faults since the gate went back to the raw form.
    faults: u64,
    /// Consecutive evidence intervals under `theta_enter`.
    low_run: u32,
    /// When the fill was judged over; the verdict is pooled from then.
    judged: Option<Duration>,
    pooled: [Window; 2],
    intervals: u32,
}

/// The selector. One per store; two stores share nothing.
#[derive(Clone, Debug)]
pub struct ZSelect {
    phase: Phase,
    /// The latest `now` seen.
    now: Duration,
    model: Option<Model>,
    /// When the last model reading that counted as a tick arrived.
    last_tick: Option<Duration>,
    sampler: Option<Sampler>,
    /// When the sampler's reading arrived.
    sampler_at: Duration,
    /// The generation of the partitions, as the input last said.
    generation: u64,
    /// The compact form's faults a read as the stretch E3 acted on measured
    /// them: kept after the exit as a floor under the model's prediction,
    /// until the form is installed again or the store is FITS.
    z_realized: Option<f64>,
    /// The model's error about the raw form's point reads, as the census
    /// found it when the compact form took over or a trial last tried the raw
    /// form: what it measured less what the model said, where they differed by
    /// at least `theta_sus`, else nothing. The compact form is judged against
    /// the model's word corrected by it ([`ZSelect::raw_belief`]), so a model
    /// that over-predicted does not keep a form the raw form beat, and one
    /// that said the store fits while a neighbour took the cache is not
    /// believed once the neighbour's refaults are gone from the counter: E1
    /// reads only the part of the error that is positive.
    bias: f64,
    /// The census saw the raw form's scans fault while the model said the
    /// store fits: its word that the store fits is not believed (E1).
    scans_against: bool,
    measured: Costs,
    footprint: Option<u64>,
    threads: u32,
    /// Consecutive hot S1 ticks (FITS).
    hot_ticks: u32,
    /// Consecutive calm S1 ticks (SUSPECT).
    calm_ticks: u32,
    smooth: Ring<f64, SMOOTH_CAP>,
    /// Since when the smoothed model, with the positive part of its error
    /// added, has been under `theta_sus / 2` and the group has not been
    /// refaulting (E1).
    low_since: Option<Duration>,
    /// What the census saw a step, over the trailing [`REFAULT_WINDOW`].
    share: Ring<Seen, SHARE_CAP>,
    faults_since_suspect: u64,
    excess: f64,
    /// When evidence was last added or the excess last halved.
    excess_anchor: Duration,
    last_switch: Option<Duration>,
    /// The wall time of the last build or cancelled build; the dwell and the
    /// retention are multiples of it.
    last_build_wall: Duration,
    build_started: Duration,
    vanished: u32,
    /// The last [`BUDGET_BUILDS`] builds, a bit each: set if superseded.
    outcomes: u32,
    suspended_until: Option<Duration>,
    retain_until: Option<Duration>,
    watch: Watch,
    backoff: Duration,
    last_trial_end: Option<Duration>,
    trial: Option<Trial>,
    /// The last decision asked for a sample (to count asking once).
    asked: bool,
    counts: Counts,
}

impl Default for ZSelect {
    fn default() -> ZSelect {
        ZSelect::new()
    }
}

impl ZSelect {
    /// A selector in FITS with nothing measured.
    pub fn new() -> ZSelect {
        ZSelect {
            phase: Phase::Fits,
            now: Duration::ZERO,
            model: None,
            last_tick: None,
            sampler: None,
            sampler_at: Duration::ZERO,
            generation: 0,
            z_realized: None,
            bias: 0.0,
            scans_against: false,
            measured: Costs::default(),
            footprint: None,
            threads: 1,
            hot_ticks: 0,
            calm_ticks: 0,
            smooth: Ring::new(),
            low_since: None,
            share: Ring::new(),
            faults_since_suspect: 0,
            excess: 0.0,
            excess_anchor: Duration::ZERO,
            last_switch: None,
            last_build_wall: Duration::ZERO,
            build_started: Duration::ZERO,
            vanished: 0,
            outcomes: 0,
            suspended_until: None,
            retain_until: None,
            watch: Watch::new(Duration::ZERO),
            backoff: TRIAL_BACKOFF_MIN,
            last_trial_end: None,
            trial: None,
            asked: false,
            counts: Counts::default(),
        }
    }

    /// The phase the last step left.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The counters.
    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// The costs the census has measured, kept across steps
    /// ([`Costs::or`] over each interval's).
    pub fn measured(&self) -> Costs {
        self.measured
    }

    /// The threads the last census with any counted.
    pub fn threads(&self) -> u32 {
        self.threads
    }

    /// The build's price (`C_build`), seconds, as the sampler's reading
    /// has it; 0 without one.
    pub fn c_build(&self) -> f64 {
        self.sampler.map_or(0.0, |s| s.c_build)
    }

    /// The dwell: `max(DWELL_FLOOR, DWELL_BUILD_FACTOR * the last build's
    /// wall)`. A switch must pay back what it cost, and a long build costs
    /// more.
    pub fn dwell(&self) -> Duration {
        DWELL_FLOOR.max(self.last_build_wall.saturating_mul(DWELL_BUILD_FACTOR))
    }

    /// How long form files are kept after the form is left:
    /// `max(RETAIN_FLOOR, RETAIN_BUILD_FACTOR * the last build's wall)`.
    pub fn retain(&self) -> Duration {
        RETAIN_FLOOR.max(self.last_build_wall.saturating_mul(RETAIN_BUILD_FACTOR))
    }

    /// The gap before the next trial may start; it doubles on a wrong trial
    /// or a cancelled build and is reset when the store is FITS again.
    pub fn backoff(&self) -> Duration {
        self.backoff
    }

    /// The thread-seconds the form is predicted to have saved net of its
    /// extra CPU since SUSPECT was entered.
    pub fn excess(&self) -> f64 {
        self.excess
    }

    /// Whether form files are being kept for a re-entry: the form was left (or a
    /// build cancelled) and the retention has not ended.
    pub fn retained(&self) -> bool {
        self.retain_until.is_some()
    }

    /// Whether the wasted-build budget is holding building off at `now`.
    pub fn suspended(&self, now: Duration) -> bool {
        self.suspended_until.is_some_and(|t| now < t)
    }

    /// `saving*` for a class: the fall in faults an operation at which the
    /// compact form's extra CPU is exactly repaid by the faults it removes.
    /// The extra CPU is the measured difference, and never less than
    /// [`SAVING_FLOOR_SHARE`] of the prior's.
    pub fn saving_star(&self, class: Class) -> f64 {
        let p = self.priced();
        let i = class.ix();
        let floor = SAVING_FLOOR_SHARE * (Costs::PRIOR_Z[i] - Costs::PRIOR_HIT[i]);
        (p.z[i] - p.hit[i]).max(floor) / p.t_f
    }

    /// `theta_enter` for a class: [`ENTER_FACTOR`] times `saving*`.
    pub fn theta_enter(&self, class: Class) -> f64 {
        ENTER_FACTOR * self.saving_star(class)
    }

    /// `theta_sus` for a class: [`SUSPECT_FACTOR`] times `saving*`.
    pub fn theta_sus(&self, class: Class) -> f64 {
        SUSPECT_FACTOR * self.saving_star(class)
    }

    /// Takes one interval's inputs and returns what to do.
    pub fn step(&mut self, input: &Input) -> Decision {
        let now = input.now.max(self.now);
        self.now = now;
        self.counts.steps += 1;
        let tick = self.absorb(input, now);
        if tick {
            self.read_s1(now);
        }
        // The budget sees every build whatever the phase: one collected late,
        // after the build it belonged to was cancelled, was wasted all the
        // same.
        let tripped = self.record_outcomes(&input.build);
        let phase = self.phase;
        let event = match phase {
            Phase::Fits => self.fits(now, tick),
            Phase::Suspect => self.suspect(input, now, tick),
            Phase::Building => self.building(input, now, tripped),
            Phase::Compact => self.compact(input, now, tick, tripped),
            Phase::Trial => self.trial_step(input, now),
        };
        if tripped && !matches!(phase, Phase::Building | Phase::Compact) {
            // Not building, so there is nothing to cancel: building is held
            // off for a dwell, which is all the budget asks.
            self.suspend(now);
        }
        let retire = self.retire(now);
        // A decline is a statement about the partitions and the budget as they
        // were, and neither stands still: it holds for a dwell, and then the
        // sampler is asked again.
        if self
            .sampler
            .is_some_and(|s| s.declined && now.saturating_sub(self.sampler_at) >= self.dwell())
        {
            self.sampler = None;
        }
        self.decide(now, event, retire)
    }

    // ---- readings ------------------------------------------------------

    /// Keeps what the input carries that outlives the step. True if a model
    /// reading arrived that counts as a tick: one a period after the last, to
    /// within a census interval. A caller that supplies the model faster than
    /// that has its readings kept and its ticks counted at the period, so the
    /// ticks that FITS, SUSPECT and the smoothing count in are seconds.
    fn absorb(&mut self, input: &Input, now: Duration) -> bool {
        self.generation = input.generation;
        if self
            .sampler
            .is_some_and(|s| s.generation != self.generation)
        {
            self.sampler = None;
        }
        // In FITS the handles run no census, nothing is sampled and no
        // footprint is kept: whatever the input carries of them is not a
        // reading of anything. Only the model is read.
        if self.phase != Phase::Fits {
            self.absorb_census(input);
        }
        match input.model {
            None => false,
            Some(m) => match m.sanitized() {
                Some(m) => {
                    self.model = Some(m);
                    let due = self.last_tick.is_none_or(|t| {
                        now.saturating_sub(t).saturating_add(CENSUS_INTERVAL) >= S1_PERIOD
                    });
                    if due {
                        self.last_tick = Some(now);
                    }
                    due
                }
                None => {
                    self.counts.ignored += 1;
                    false
                }
            },
        }
    }

    /// Keeps what the census, the sampler and the footprint carry that
    /// outlives the step. The census measured the form the gate served: its
    /// cost of a hit is a measure of the raw form's and its cost of a compact
    /// read of the compact form's, and each is taken in no other phase.
    fn absorb_census(&mut self, input: &Input) {
        let c = input.census.costs;
        let raw_served = matches!(self.phase, Phase::Suspect | Phase::Building | Phase::Trial);
        let z_served = self.phase == Phase::Compact;
        let served = Costs {
            t_f: c.t_f,
            t_hit: c.t_hit.filter(|_| raw_served),
            t_hit_scan: c.t_hit_scan.filter(|_| raw_served),
            t_z: c.t_z.filter(|_| z_served),
            t_z_scan: c.t_z_scan.filter(|_| z_served),
        };
        self.measured = served.or(self.measured);
        if input.census.threads > 0 {
            self.threads = input.census.threads;
        }
        self.counts.mixed_windows = self.counts.mixed_windows.saturating_add(input.census.mixed);
        if let Some(s) = input.sampler {
            match s.sanitized() {
                Some(s) if s.generation == self.generation => {
                    self.sampler = Some(s);
                    self.sampler_at = self.now;
                }
                _ => self.counts.ignored += 1,
            }
        }
        if let Some(f) = input.footprint {
            self.footprint = Some(f);
        }
    }

    /// The model's tick: smoothing, and the stretch E1 reads.
    fn read_s1(&mut self, now: Duration) {
        let Some(m) = self.model else { return };
        self.counts.s1_ticks += 1;
        self.smooth.push(now, m.phi_raw);
        self.smooth.expire(now, E1_SMOOTHING);
        let (sum, n) = self
            .smooth
            .iter()
            .fold((0.0f64, 0u32), |(s, n), &(_, v)| (s + v, n + 1));
        let mean = sum / f64::from(n.max(1));
        // E1 is the model's word that the store fits. It is believed unless
        // it was caught saying less than the census saw, and not while the
        // group is refaulting: where it is, the model's rate is the thing
        // that is in doubt. A model that over-predicted is not made more
        // certain by it, so only the positive error counts.
        let below = mean + self.bias.max(0.0) < self.theta_sus(Class::Read) * RELAX_FACTOR
            && !self.pressure();
        self.low_since = if below {
            self.low_since.or(Some(now))
        } else {
            None
        };
    }

    /// The costs, each from the best instrument there is.
    fn priced(&self) -> Priced {
        let c = self
            .measured
            .or(self.sampler.map_or(Costs::default(), |s| s.costs))
            .or(Costs::PRIOR);
        Priced {
            t_f: c.t_f.unwrap_or(PRIOR_T_F),
            hit: [
                c.t_hit.unwrap_or(PRIOR_T_HIT),
                c.t_hit_scan.unwrap_or(PRIOR_T_HIT_SCAN),
            ],
            z: [
                c.t_z.unwrap_or(PRIOR_T_Z),
                c.t_z_scan.unwrap_or(PRIOR_T_Z_SCAN),
            ],
        }
    }

    /// The compact form's predicted faults an operation of a class.
    fn phi_z(&self, class: Class) -> Option<f64> {
        let m = self.model?;
        match class {
            Class::Read => m.phi_z.map(|z| z.max(self.z_realized.unwrap_or(0.0))),
            Class::Scan => m.phi_z_scan,
        }
    }

    /// The faults an operation of a class over the trailing window, as the
    /// census saw them while the raw form served: what the raw form takes,
    /// measured, over the intervals the evidence rule took (a first touch is
    /// not what the raw form takes once the cache is full). None under
    /// [`MIN_EVIDENCE_OPS`] operations.
    fn raw_seen_in(&self, class: Class) -> Option<f64> {
        let mut w = Window::default();
        for (_, s) in self.share.iter().filter(|(_, s)| s.counted) {
            w.add(match class {
                Class::Read => s.reads,
                Class::Scan => s.scans,
            });
        }
        (w.ops >= MIN_EVIDENCE_OPS).then(|| w.phi())
    }

    /// Files what the census saw the raw form take against the model's word.
    /// The model's word is about the point read: the reads' measured rate
    /// less what it said is the model's error, kept where it is at least
    /// `theta_sus`, the rate below which counting is not worth it, and
    /// otherwise the model was right to within what is worth acting on. The
    /// scans have no rate of their own in the model, only its word that the
    /// store fits: a scan that faults at `theta_sus` of its class while the
    /// model says the store fits is held against that word. Both hold for as
    /// long as the form stands.
    fn note_raw(&mut self, reads: Option<f64>, scans: Option<f64>) {
        let said = self.model.map_or(0.0, |m| m.phi_raw);
        let sus = self.theta_sus(Class::Read);
        if let Some(r) = reads {
            let off = r - said;
            self.bias = if off.abs() >= sus { off } else { 0.0 };
            // The stretch E1 has under way was measured without it.
            self.low_since = None;
        }
        if said < sus && scans.is_some_and(|r| r >= self.theta_sus(Class::Scan)) {
            self.scans_against = true;
        }
    }

    /// What the raw form is believed to take a read, for judging the compact
    /// form: the model's word for it, corrected by the model's error as the
    /// census found it.
    fn raw_belief(&self, said: f64) -> f64 {
        (said + self.bias).max(0.0)
    }

    /// The pressure term: the group's refaults are above the threshold at the
    /// latest reading.
    fn pressure(&self) -> bool {
        self.model
            .is_some_and(|m| m.refault_rate > PRESSURE_REFAULTS_PER_S)
    }

    /// Whether a dwell has passed since the last switch.
    fn switch_allowed(&self, now: Duration) -> bool {
        self.last_switch
            .is_none_or(|t| now.saturating_sub(t) >= self.dwell())
    }

    /// Whether the trial back-off has run.
    fn trial_allowed(&self, now: Duration) -> bool {
        self.last_trial_end
            .is_none_or(|t| now.saturating_sub(t) >= self.backoff)
    }

    fn double_backoff(&mut self) {
        self.backoff = self.backoff.saturating_mul(2).min(TRIAL_BACKOFF_MAX);
    }

    // ---- the census ------------------------------------------------------

    /// What the interval says, class by class, and the bookkeeping the rule
    /// for evidence needs: the trailing refault share, and the faults since
    /// SUSPECT where the platform has no refault counter.
    fn evidence(&mut self, census: &Census, now: Duration) -> Evidence {
        let faults = census.reads.faults.saturating_add(census.scans.faults);
        self.faults_since_suspect = self.faults_since_suspect.saturating_add(faults);
        self.share.push(
            now,
            Seen {
                refaults: census.refaults.unwrap_or(0),
                faults,
                reads: census.reads,
                scans: census.scans,
                counted: false,
            },
        );
        self.share.expire(now, REFAULT_WINDOW);
        let (refaults, total) = self.share.iter().fold((0u64, 0u64), |(r, f), (_, s)| {
            (r.saturating_add(s.refaults), f.saturating_add(s.faults))
        });
        let share_ok = if census.refaults.is_some() {
            refaults as f64 >= REFAULT_SHARE * total as f64
        } else {
            self.model
                .is_some_and(|m| self.faults_since_suspect >= m.budget / PAGE)
        };
        if share_ok {
            if let Some((_, newest)) = self.share.last_mut() {
                newest.counted = true;
            }
        }
        let mut ev = Evidence::default();
        if !share_ok {
            return ev;
        }
        for class in Class::ALL {
            let w = census.window(class);
            let Some(z) = self.phi_z(class) else { continue };
            if w.ops < MIN_EVIDENCE_OPS {
                continue;
            }
            let i = class.ix();
            ev.valid[i] = true;
            ev.win[i] = w;
            ev.saving[i] = w.phi() - z;
        }
        ev
    }

    /// Adds the interval to `excess`, or ages it.
    fn accumulate(&mut self, ev: &Evidence, now: Duration) {
        if ev.any() {
            let t_f = self.priced().t_f;
            let mut delta = 0.0;
            for class in Class::ALL {
                let i = class.ix();
                if ev.valid[i] {
                    delta += (ev.saving[i] - self.saving_star(class)) * t_f * ev.win[i].ops as f64;
                }
            }
            self.excess = (self.excess + delta).max(0.0);
            self.excess_anchor = now;
            return;
        }
        // No evidence: a stale rent is not a credit. It halves for every
        // whole dwell since it was last added to.
        let dwell = self.dwell();
        let idle = now.saturating_sub(self.excess_anchor);
        if self.excess > 0.0 && idle >= dwell {
            let n = (idle.as_nanos() / dwell.as_nanos().max(1)).min(1100) as u32;
            self.excess *= 0.5f64.powi(n as i32);
            self.excess_anchor = self.excess_anchor.saturating_add(dwell.saturating_mul(n));
        }
    }

    // ---- transitions -----------------------------------------------------

    fn fits(&mut self, now: Duration, tick: bool) -> Option<Event> {
        if !tick {
            return None;
        }
        let m = self.model?;
        let hot = m.phi_raw >= self.theta_sus(Class::Read) || self.pressure();
        self.hot_ticks = if hot {
            self.hot_ticks.saturating_add(1)
        } else {
            0
        };
        if self.hot_ticks >= FITS_TO_SUSPECT_TICKS {
            self.enter_suspect(now);
            return Some(Event::Suspect);
        }
        None
    }

    /// SUSPECT, entered from FITS or from a COMPACT that was left: the rent
    /// starts again, and so does the evidence.
    fn enter_suspect(&mut self, now: Duration) {
        self.phase = Phase::Suspect;
        self.hot_ticks = 0;
        self.calm_ticks = 0;
        self.excess = 0.0;
        self.excess_anchor = now;
        self.faults_since_suspect = 0;
        self.share.clear();
        self.vanished = 0;
        self.trial = None;
        self.bias = 0.0;
        self.scans_against = false;
        self.footprint = None;
    }

    fn enter_fits(&mut self) {
        self.phase = Phase::Fits;
        self.hot_ticks = 0;
        self.calm_ticks = 0;
        self.excess = 0.0;
        self.faults_since_suspect = 0;
        self.share.clear();
        self.vanished = 0;
        self.trial = None;
        // The next episode is a fresh one: the partitions may have changed
        // before the sampler is asked again, and a wrong guess about a skewed
        // workload is forgiven.
        self.sampler = None;
        self.z_realized = None;
        self.backoff = TRIAL_BACKOFF_MIN;
        self.last_trial_end = None;
    }

    fn suspect(&mut self, input: &Input, now: Duration, tick: bool) -> Option<Event> {
        if tick {
            let calm = self
                .model
                .is_some_and(|m| m.phi_raw < self.theta_sus(Class::Read) * RELAX_FACTOR)
                && !self.pressure();
            self.calm_ticks = if calm {
                self.calm_ticks.saturating_add(1)
            } else {
                0
            };
            if self.calm_ticks >= SUSPECT_TO_FITS_TICKS {
                self.enter_fits();
                return Some(Event::Relax);
            }
        }
        let ev = self.evidence(&input.census, now);
        self.accumulate(&ev, now);
        if self.build_clause(now, &ev) {
            self.phase = Phase::Building;
            self.build_started = now;
            self.vanished = 0;
            self.retain_until = None;
            self.counts.builds += 1;
            return Some(Event::BuildStart);
        }
        None
    }

    /// Clauses (a) to (e) of the build row.
    fn build_clause(&self, now: Duration, ev: &Evidence) -> bool {
        let (Some(m), Some(s)) = (self.model, self.sampler) else {
            return false;
        };
        // (e), and the budget: the sampler declined, or building is held off.
        if s.declined || self.suspended(now) {
            return false;
        }
        // (d)
        if !self.switch_allowed(now) {
            return false;
        }
        // (a) the price is paid. (Only evidence adds to the rent, and the
        // measured saving below is a statement about this interval's: a step
        // without evidence buys nothing however much rent stands.)
        if self.excess < s.c_build {
            return false;
        }
        // (c) the model is not contradicting: it does not say FITS, or the
        // group's pressure overrules it. Given (b) this is implied, since the
        // predicted saving is at most `phi_raw` and `theta_enter` is over
        // `theta_sus`; it stays as its own row so that a change to (b) cannot
        // quietly drop it.
        let pressure = self.pressure();
        if !(pressure || m.phi_raw >= self.theta_sus(Class::Read)) {
            return false;
        }
        // (b) the saving, predicted and measured. The predicted saving is a
        // statement about the footprint against the budget, and a neighbour's
        // page cache is not in the budget, so the pressure term stands for it
        // where the model cannot see why the cache is short; the measured
        // saving still decides.
        let predicted = pressure
            || self
                .phi_z(Class::Read)
                .is_some_and(|z| m.phi_raw - z >= self.theta_enter(Class::Read));
        predicted
            && Class::ALL
                .iter()
                .any(|&c| ev.valid[c.ix()] && ev.saving[c.ix()] >= self.theta_enter(c))
    }

    fn building(&mut self, input: &Input, now: Duration, tripped: bool) -> Option<Event> {
        let ev = self.evidence(&input.census, now);
        // Clause (e) holds throughout: a sampler that declines the partitions
        // ends the build, finished or not.
        if self.sampler.is_some_and(|s| s.declined) {
            return Some(self.cancel(now, Cancel::Declined));
        }
        if input.build.coverage >= COVERED_ENOUGH {
            let installed = self.install(now);
            if tripped {
                // The form is attached; what the budget holds off is the
                // building of whatever a merge makes next.
                self.suspend(now);
            }
            return Some(installed);
        }
        if tripped {
            return Some(self.cancel(now, Cancel::Budget));
        }
        if ev.any() {
            let vanished = Class::ALL.iter().all(|&c| {
                !ev.valid[c.ix()] || ev.saving[c.ix()] < self.theta_enter(c) * VANISH_FACTOR
            });
            self.vanished = if vanished {
                self.vanished.saturating_add(1)
            } else {
                0
            };
            if self.vanished >= VANISH_INTERVALS {
                return Some(self.cancel(now, Cancel::Vanished));
            }
        }
        None
    }

    fn install(&mut self, now: Duration) -> Event {
        self.last_build_wall = now.saturating_sub(self.build_started);
        self.phase = Phase::Compact;
        self.last_switch = Some(now);
        self.retain_until = None;
        self.z_realized = None;
        // SUSPECT is entered clear of the model's error and of the scans held
        // against it, and a form's footprint begins here.
        debug_assert!(self.bias == 0.0 && !self.scans_against);
        self.footprint = None;
        self.note_raw(self.raw_seen_in(Class::Read), self.raw_seen_in(Class::Scan));
        self.excess = 0.0;
        self.watch = Watch::new(now);
        self.counts.installs += 1;
        self.counts.switches += 1;
        Event::Install
    }

    /// A build given up: the evidence vanished (and the back-off doubles),
    /// the budget tripped (and building is suspended), or the sampler declined
    /// the partitions (and the build clause refuses them until it does not).
    fn cancel(&mut self, now: Duration, why: Cancel) -> Event {
        self.last_build_wall = now.saturating_sub(self.build_started);
        self.last_switch = Some(now);
        self.counts.switches += 1;
        self.counts.build_cancelled += 1;
        self.retain_until = Some(now.saturating_add(self.retain()));
        self.enter_suspect(now);
        match why {
            Cancel::Vanished => {
                self.double_backoff();
                Event::BuildCancelled
            }
            Cancel::Budget => {
                self.suspend(now);
                Event::BuildAbandoned
            }
            Cancel::Declined => Event::BuildAbandoned,
        }
    }

    fn suspend(&mut self, now: Duration) {
        self.suspended_until = Some(now.saturating_add(self.dwell()));
        self.counts.build_suspended += 1;
    }

    /// Files the builds collected this step in the budget; true if a
    /// superseded one put it over. Only a superseded build trips it: a build
    /// that installed while the old supersessions are still in the window is
    /// the budget recovering, and suspending on it would hold building off
    /// for as long as the window remembers what the builds before it did.
    fn record_outcomes(&mut self, b: &Build) -> bool {
        for _ in 0..b.installed.saturating_add(b.failed).min(BUDGET_BUILDS) {
            self.outcomes <<= 1;
        }
        let superseded = b.superseded.min(BUDGET_BUILDS);
        for _ in 0..superseded {
            self.outcomes = (self.outcomes << 1) | 1;
        }
        self.outcomes &= BUDGET_WINDOW;
        superseded > 0 && self.outcomes.count_ones() > BUDGET_SUPERSEDED
    }

    fn compact(
        &mut self,
        input: &Input,
        now: Duration,
        tick: bool,
        tripped: bool,
    ) -> Option<Event> {
        let mut event = None;
        if tripped {
            self.suspend(now);
            event = Some(Event::BuildsSuspended);
        }
        self.watch.tick_reads.add(input.census.reads);
        self.watch.e2_win[Class::Read.ix()].add(input.census.reads);
        self.watch.e2_win[Class::Scan.ix()].add(input.census.scans);
        if tick {
            self.update_e3(now);
        }
        self.update_e2(now);
        if self.switch_allowed(now) {
            if !self.scans_against
                && self
                    .low_since
                    .is_some_and(|t| now.saturating_sub(t) >= self.dwell())
            {
                return Some(self.exit(Exit::E1, now));
            }
            if self
                .watch
                .unpaying_since
                .is_some_and(|t| now.saturating_sub(t) >= self.dwell())
            {
                return Some(self.exit(Exit::E3, now));
            }
            if self.watch.e2_run >= E2_MEASURES && self.trial_allowed(now) {
                return Some(self.start_trial(now));
            }
        }
        event
    }

    /// E3's reading, once an S1 tick: is the form paying, over the second the
    /// census just closed? A tick without evidence breaks the stretch: an exit
    /// is acted on only for a form that has been seen not to pay throughout.
    /// The form is judged against [`ZSelect::raw_belief`], the model's word
    /// for the raw form corrected by its error, and the pressure term does not
    /// stop the reading: the compact form's own faults are among the refaults
    /// it counts.
    fn update_e3(&mut self, now: Duration) {
        let w = std::mem::take(&mut self.watch.tick_reads);
        let Some(m) = self.model else { return };
        if w.ops < MIN_EVIDENCE_OPS {
            self.watch.unpaying_since = None;
            self.watch.stretch = Window::default();
            return;
        }
        let realized = self.raw_belief(m.phi_raw) - w.phi();
        if realized < self.saving_star(Class::Read) {
            self.watch.unpaying_since = self.watch.unpaying_since.or(Some(now));
            self.watch.stretch.add(w);
        } else {
            self.watch.unpaying_since = None;
            self.watch.stretch = Window::default();
        }
    }

    /// E2's measure, once every [`E2_MEASURE`]: would the raw form fit? Every
    /// class with data must fault under `theta_sus` through the compact form,
    /// and the raw footprint of the pages touched must be under half the
    /// budget. A measure with no data is not a yes.
    fn update_e2(&mut self, now: Duration) {
        if now.saturating_sub(self.watch.e2_start) < E2_MEASURE {
            return;
        }
        let win = std::mem::take(&mut self.watch.e2_win);
        self.watch.e2_start = now;
        let mut data = false;
        let mut low = true;
        for class in Class::ALL {
            let w = win[class.ix()];
            if w.ops >= MIN_EVIDENCE_OPS {
                data = true;
                low &= w.phi() < self.theta_sus(class);
            }
        }
        let fits = match (self.footprint, self.model) {
            (Some(bound), Some(m)) => (bound as f64) < E2_BOUND_SHARE * m.budget as f64,
            _ => false,
        };
        self.watch.e2_run = if data && low && fits {
            self.watch.e2_run.saturating_add(1)
        } else {
            0
        };
    }

    fn exit(&mut self, why: Exit, now: Duration) -> Event {
        self.last_switch = Some(now);
        self.counts.switches += 1;
        self.counts.exits[why as usize] += 1;
        self.retain_until = Some(now.saturating_add(self.retain()));
        if why == Exit::E3 {
            // The model said the form would pay and the form did not: until
            // it is installed again its faults are what it was seen to take.
            self.z_realized = Some(self.watch.stretch.phi());
        }
        self.watch = Watch::new(now);
        if why == Exit::E2 {
            // A right trial is still a trial: the next one waits out the
            // back-off too, however soon the form is bought again.
            self.last_trial_end = Some(now);
        }
        self.enter_suspect(now);
        Event::Exit(why)
    }

    fn start_trial(&mut self, now: Duration) -> Event {
        let bound = self.footprint.unwrap_or(0);
        self.trial = Some(Trial {
            started: now,
            pages: (bound / PAGE).max(1),
            faults: 0,
            low_run: 0,
            judged: None,
            pooled: [Window::default(); 2],
            intervals: 0,
        });
        self.phase = Phase::Trial;
        self.counts.trials += 1;
        Event::TrialStart
    }

    /// How long the trial waits for the fill: [`TRIAL_WALL_FACTOR`] times the
    /// time its expected pages take to fault in, across the threads reading,
    /// and no more than [`TRIAL_WALL_CAP`].
    fn trial_wall(&self, pages: u64) -> Duration {
        let t =
            TRIAL_WALL_FACTOR * pages as f64 * self.priced().t_f / f64::from(self.threads.max(1));
        secs(t).min(TRIAL_WALL_CAP)
    }

    fn trial_step(&mut self, input: &Input, now: Duration) -> Option<Event> {
        let ev = self.evidence(&input.census, now);
        let Some(mut t) = self.trial.take() else {
            // No trial in flight is a state nothing reaches; the form is still
            // attached, so serve it.
            self.phase = Phase::Compact;
            return None;
        };
        t.faults = t
            .faults
            .saturating_add(input.census.reads.faults)
            .saturating_add(input.census.scans.faults);
        match t.judged {
            None => {
                if ev.any() {
                    let low = Class::ALL
                        .iter()
                        .all(|&c| !ev.valid[c.ix()] || ev.saving[c.ix()] < self.theta_enter(c));
                    t.low_run = if low { t.low_run.saturating_add(1) } else { 0 };
                }
                let filled = t.faults as f64 >= TRIAL_FAULT_FACTOR * t.pages as f64;
                let waited = now.saturating_sub(t.started) >= self.trial_wall(t.pages);
                let settled = t.low_run >= TRIAL_LOW_INTERVALS;
                if filled || waited || settled {
                    t.judged = Some(now);
                }
            }
            Some(since) => {
                if ev.any() {
                    for class in Class::ALL {
                        if ev.valid[class.ix()] {
                            t.pooled[class.ix()].add(ev.win[class.ix()]);
                        }
                    }
                    t.intervals += 1;
                    if t.intervals >= TRIAL_DECIDE_INTERVALS {
                        let high = Class::ALL.iter().any(|&c| {
                            let w = t.pooled[c.ix()];
                            w.ops > 0
                                && self
                                    .phi_z(c)
                                    .is_some_and(|z| w.phi() - z >= self.theta_enter(c))
                        });
                        return Some(if high {
                            let seen = |c: Class| {
                                let w = t.pooled[c.ix()];
                                (w.ops > 0).then(|| w.phi())
                            };
                            self.note_raw(seen(Class::Read), seen(Class::Scan));
                            self.trial_back(now, true)
                        } else {
                            self.exit(Exit::E2, now)
                        });
                    }
                }
                // A trial that cannot be decided is undone: the form it came
                // from is attached and the raw form must not be left serving
                // a store nothing is reading.
                if now.saturating_sub(since) >= self.dwell() {
                    return Some(self.trial_back(now, false));
                }
            }
        }
        self.trial = Some(t);
        None
    }

    /// The trial ends with the compact form still wanted.
    fn trial_back(&mut self, now: Duration, wrong: bool) -> Event {
        self.phase = Phase::Compact;
        self.trial = None;
        self.last_trial_end = Some(now);
        if wrong {
            self.double_backoff();
        }
        self.watch = Watch::new(now);
        self.counts.trials_back += 1;
        Event::TrialBack
    }

    fn retire(&mut self, now: Duration) -> bool {
        match self.retain_until {
            Some(t) if now >= t && matches!(self.phase, Phase::Fits | Phase::Suspect) => {
                self.retain_until = None;
                self.counts.retired += 1;
                // The reading priced form files that are gone: a build from
                // nothing costs what the sampler says of it afresh.
                self.sampler = None;
                true
            }
            _ => false,
        }
    }

    fn decide(&mut self, now: Duration, event: Option<Event>, retire: bool) -> Decision {
        let phase = self.phase;
        let sample = phase == Phase::Suspect && self.sampler.is_none();
        if sample && !self.asked {
            self.counts.samples_wanted += 1;
        }
        self.asked = sample;
        let build = match phase {
            Phase::Building => true,
            Phase::Compact => !self.suspended(now),
            Phase::Fits | Phase::Suspect | Phase::Trial => false,
        };
        Decision {
            phase,
            gate: phase.gate(),
            census: phase.census(),
            sample,
            build,
            retire,
            event,
        }
    }
}

// ---- the platform's side of the model ------------------------------------

/// What the platform says of the memory the store's page cache may use
/// (design 4.2): the limit in effect, the anonymous memory charged against
/// it, the group's file refaults so far where the platform counts them, and
/// the budget `B = limit - anon - margin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub limit: u64,
    pub anon: u64,
    pub refaults: Option<u64>,
    pub budget: u64,
}

/// A twentieth of the limit, and sixty-four mebibytes at least: what the
/// model leaves for the kernel's own and the page cache's slack.
pub fn margin(limit: u64) -> u64 {
    (limit / 20).max(64 << 20)
}

impl Budget {
    pub fn of(limit: u64, anon: u64, refaults: Option<u64>) -> Budget {
        Budget {
            limit,
            anon,
            refaults,
            budget: limit.saturating_sub(anon).saturating_sub(margin(limit)),
        }
    }
}

/// cgroup v1's "no limit": `i64::MAX` rounded down to a page.
pub const V1_NO_LIMIT: u64 = 1 << 62;

/// The v1 memory controller's path for this process from `/proc/self/cgroup`
/// text: the line `N:memory:/path`, or one naming `memory` among several
/// controllers. A v2-only host has only the `0::/path` line, which names no
/// controller, and is `None` here.
pub fn parse_cgroup_v1_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut it = line.splitn(3, ':');
        let (Some(_id), Some(controllers), Some(path)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if controllers.split(',').any(|c| c == "memory") {
            return Some(path.to_string());
        }
    }
    None
}

/// The unified hierarchy's path for this process: the `0::/path` line.
pub fn parse_cgroup_v2_path(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("0::") {
            return Some(path.to_string());
        }
    }
    None
}

/// Where a cgroup mount of `kind` (`cgroup` with the `memory` option, or
/// `cgroup2`) is, from `/proc/mounts` text: the mount point is the second
/// field, as the text has it.
pub fn parse_mount(text: &str, kind: &str) -> Option<String> {
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 || f[2] != kind {
            continue;
        }
        if kind == "cgroup" && !f[3].split(',').any(|o| o == "memory") {
            continue;
        }
        return Some(f[1].to_string());
    }
    None
}

/// What a v1 group's `memory.stat` says: the limit in effect through the
/// hierarchy, the anonymous memory (`rss` and `rss_huge`) and the file
/// refaults, each `None` where the key is absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatV1 {
    pub limit: Option<u64>,
    pub rss: u64,
    pub rss_huge: u64,
    pub refaults: Option<u64>,
}

pub fn parse_stat_v1(text: &str) -> StatV1 {
    let mut s = StatV1::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(v) = v.parse::<u64>() else { continue };
        match k {
            "hierarchical_memory_limit" => s.limit = Some(v),
            "rss" => s.rss = v,
            "rss_huge" => s.rss_huge = v,
            "workingset_refault_file" | "workingset_refault" => s.refaults = Some(v),
            _ => {}
        }
    }
    s
}

/// What a v2 group's `memory.stat` says: the anonymous memory and the file
/// refaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatV2 {
    pub anon: u64,
    pub refaults: Option<u64>,
}

pub fn parse_stat_v2(text: &str) -> StatV2 {
    let mut s = StatV2::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(v) = v.parse::<u64>() else { continue };
        match k {
            "anon" => s.anon = v,
            "workingset_refault_file" | "workingset_refault" => s.refaults = Some(v),
            _ => {}
        }
    }
    s
}

/// A v2 `memory.max` or `memory.high`: a number, or `max` for none.
pub fn parse_limit(text: &str) -> Option<u64> {
    let t = text.trim();
    if t == "max" {
        return None;
    }
    t.parse::<u64>().ok()
}

/// `MemTotal` of `/proc/meminfo`, bytes.
pub fn parse_meminfo_total(text: &str) -> Option<u64> {
    parse_kb(text, "MemTotal:")
}

/// `RssAnon` of `/proc/self/status`, bytes.
pub fn parse_status_rss_anon(text: &str) -> Option<u64> {
    parse_kb(text, "RssAnon:")
}

fn parse_kb(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let n: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return n.checked_mul(1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the platform's side ------------------------------------------

    #[test]
    fn the_cgroup_parsers_read_this_hosts_shapes_and_a_v2_only_host() {
        let v1 = "12:pids:/a\n4:memory:/process_api/x/claude-code-bash\n0::/\n";
        assert_eq!(
            parse_cgroup_v1_path(v1).as_deref(),
            Some("/process_api/x/claude-code-bash")
        );
        assert_eq!(parse_cgroup_v2_path(v1).as_deref(), Some("/"));
        let several = "3:cpu,memory:/g\n";
        assert_eq!(parse_cgroup_v1_path(several).as_deref(), Some("/g"));
        let v2only = "0::/user.slice/s\n";
        assert_eq!(parse_cgroup_v1_path(v2only), None);
        assert_eq!(
            parse_cgroup_v2_path(v2only).as_deref(),
            Some("/user.slice/s")
        );
        let mounts = "sysfs /sys sysfs rw 0 0\n\
                      cgroup /sys/fs/cgroup/cpu cgroup rw,cpu 0 0\n\
                      cgroup /sys/fs/cgroup/memory cgroup rw,memory 0 0\n\
                      cgroup2 /sys/fs/cgroup/unified cgroup2 rw,nsdelegate 0 0\n";
        assert_eq!(
            parse_mount(mounts, "cgroup").as_deref(),
            Some("/sys/fs/cgroup/memory")
        );
        assert_eq!(
            parse_mount(mounts, "cgroup2").as_deref(),
            Some("/sys/fs/cgroup/unified")
        );
        assert_eq!(parse_mount("none /x tmpfs rw 0 0\n", "cgroup"), None);
        let stat = "cache 1024\nrss 2048\nrss_huge 4096\nmapped_file 7\n\
                    workingset_refault_file 99\nhierarchical_memory_limit 1073741824\n";
        assert_eq!(
            parse_stat_v1(stat),
            StatV1 {
                limit: Some(1 << 30),
                rss: 2048,
                rss_huge: 4096,
                refaults: Some(99),
            }
        );
        assert_eq!(parse_stat_v1("rss 5\n").refaults, None);
        assert_eq!(
            parse_stat_v2("anon 123\nfile 9\nworkingset_refault_file 4\n"),
            StatV2 {
                anon: 123,
                refaults: Some(4)
            }
        );
        assert_eq!(parse_limit("max\n"), None);
        assert_eq!(parse_limit("1073741824\n"), Some(1 << 30));
        assert_eq!(parse_limit("junk"), None);
        assert_eq!(
            parse_meminfo_total("MemTotal:       16479232 kB\nMemFree: 1 kB\n"),
            Some(16479232 * 1024)
        );
        assert_eq!(
            parse_status_rss_anon("Name:\tx\nRssAnon:\t  4096 kB\nRssFile: 1 kB\n"),
            Some(4096 * 1024)
        );
        assert_eq!(parse_status_rss_anon("Name:\tx\n"), None);
    }

    #[test]
    fn the_budget_takes_the_margin_and_the_anon_off_the_limit() {
        // A small limit keeps the sixty-four mebibyte floor; a large one a
        // twentieth.
        assert_eq!(margin(1 << 30), 64 << 20);
        assert_eq!(margin(16 << 30), (16 << 30) / 20);
        let b = Budget::of(1 << 30, 100 << 20, Some(7));
        assert_eq!(b.budget, (1 << 30) - (100 << 20) - (64 << 20));
        assert_eq!(b.refaults, Some(7));
        // The anon past the limit leaves nothing, never a wrap.
        assert_eq!(Budget::of(1 << 30, 2 << 30, None).budget, 0);
    }

    #[test]
    fn the_phase_has_a_code_for_the_gauge() {
        assert_eq!(Phase::Fits.code(), 0);
        assert_eq!(Phase::Trial.code(), 4);
    }

    const DT: Duration = CENSUS_INTERVAL;
    /// Steps in an S1 period.
    const TICK: u64 = (S1_PERIOD.as_millis() / CENSUS_INTERVAL.as_millis()) as u64;

    fn secs_f(x: f64) -> Duration {
        Duration::from_secs_f64(x)
    }

    // ---- the world a trace runs in ----------------------------------------
    //
    // A trace is not a list of numbers written down in advance: what the
    // census reads depends on the form the gate serves, and the gate is what
    // the selector decides. So a trace is a store, a workload and a builder,
    // stepped once per census interval, the decision of each step setting the
    // world the next one is read from. The store is the cell of the banked
    // ten-million-key rung under a gibibyte (`docs/engine.md`, "Out of core").

    /// Operations a second and faults an operation of one class through one
    /// form.
    #[derive(Clone, Copy, Debug)]
    struct Rate {
        ops_per_s: f64,
        phi: f64,
    }

    #[derive(Clone, Debug)]
    struct Cell {
        /// What the kernel really leaves the store.
        cache: u64,
        /// What S1 takes off the cache: the margin.
        margin: u64,
        hash: u64,
        records: u64,
        z: u64,
        resident: u64,
        threads: u32,
        t_f: f64,
        t_hit: f64,
        t_z: f64,
        t_hit_scan: f64,
        t_z_scan: f64,
        /// Faults a scan of a hundred takes in the raw form.
        scan_raw: f64,
        /// Pages a scan of a hundred touches in the compact form.
        scan_pages: f64,
        /// Seconds a whole build takes at raw rates.
        build_wall: f64,
        /// `C_build` from nothing, and from form files already attached. From
        /// nothing it is what [`ski_rental_price`] says of the build's CPU,
        /// which at the cell is eight seconds.
        price: f64,
        attach: f64,
        /// Refaults a second of other readers in the group.
        neighbours: f64,
    }

    impl Cell {
        fn ten_million() -> Cell {
            Cell {
                cache: 937_000_000,
                margin: 67_000_000,
                hash: 190_000_000,
                records: 1_300_000_000,
                z: 720_000_000,
                resident: 17_400_000,
                threads: 1,
                t_f: PRIOR_T_F,
                t_hit: PRIOR_T_HIT,
                t_z: PRIOR_T_Z,
                t_hit_scan: PRIOR_T_HIT_SCAN,
                t_z_scan: PRIOR_T_Z_SCAN,
                scan_raw: 2.9,
                scan_pages: 2.74,
                build_wall: 30.0,
                price: ski_rental_price(8.0, None),
                attach: 0.2,
                neighbours: 0.0,
            }
        }

        /// The same store at three million keys, which the cache holds.
        fn three_million() -> Cell {
            Cell {
                hash: 57_000_000,
                records: 390_000_000,
                z: 216_000_000,
                resident: 5_200_000,
                ..Cell::ten_million()
            }
        }

        fn budget(&self) -> u64 {
            self.cache.saturating_sub(self.margin)
        }

        /// What the raw form really takes a read at the cache the kernel
        /// leaves; S1 predicts it at the budget, a margin under.
        fn truth_raw(&self) -> f64 {
            model_raw(self.cache, self.hash, self.records)
        }

        fn truth_z(&self) -> f64 {
            model_z(self.cache, self.resident, self.z)
        }

        /// The raw footprint of `z_pages` compact pages touched: a page's keys
        /// hash to up to 57 distinct hash pages, bounded by the hash file, and
        /// to 1.7 record pages.
        fn bound(&self, z_pages: f64) -> u64 {
            let page = PAGE as f64;
            ((self.hash as f64).min(z_pages * 57.0 * page) + z_pages * 1.7 * page) as u64
        }

        fn rate(&self, t_op: f64, phi: f64) -> Rate {
            Rate {
                ops_per_s: f64::from(self.threads) / (t_op + phi * self.t_f),
                phi,
            }
        }

        /// Uniform point reads over the whole store.
        fn uniform(&self) -> Load {
            Load::Rated {
                class: Class::Read,
                raw: self.rate(self.t_hit, self.truth_raw()),
                z: self.rate(self.t_z, self.truth_z()),
                refaults: true,
                bound: self.bound(self.z as f64 / PAGE as f64),
            }
        }

        /// Uniform scans of a hundred over the whole store.
        fn scans(&self) -> Load {
            Load::Rated {
                class: Class::Scan,
                raw: self.rate(self.t_hit_scan, self.scan_raw),
                z: self.rate(self.t_z_scan, self.truth_z() * self.scan_pages),
                refaults: true,
                bound: self.bound(self.z as f64 / PAGE as f64),
            }
        }

        /// Reads at fixed fault rates, whatever the geometry says.
        fn fixed(&self, raw: f64, z: f64, bound: u64) -> Load {
            Load::Rated {
                class: Class::Read,
                raw: self.rate(self.t_hit, raw),
                z: self.rate(self.t_z, z),
                refaults: true,
                bound,
            }
        }

        /// A hot set of `mb` megabytes of raw records, which the compact form
        /// holds in 55% of the pages.
        fn hot(&self, mb: u64) -> Load {
            let raw_pages = (mb * 1_000_000) as f64 / PAGE as f64;
            Load::Hot {
                raw_pages,
                z_pages: raw_pages * 0.55,
                plateau: 0.0,
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Load {
        /// Nothing runs.
        Idle,
        /// One class served at fixed rates, one for each form.
        Rated {
            class: Class,
            raw: Rate,
            z: Rate,
            refaults: bool,
            bound: u64,
        },
        /// Both classes at once, each at fixed rates `(raw, z)`: the reads and
        /// the scans of one store, as a mix of the two has them.
        Both {
            read: (Rate, Rate),
            scan: (Rate, Rate),
            refaults: bool,
            bound: u64,
        },
        /// Reads of a hot set. The compact form serves it from its own
        /// resident pages and takes no fault; the raw form faults in every
        /// page of it the compact form's pages displaced, one fault a page,
        /// and then faults at `plateau` (a set that does not fit takes
        /// faults for ever).
        Hot {
            raw_pages: f64,
            z_pages: f64,
            plateau: f64,
        },
        /// Uniform reads of a store the cache holds whole, with the cache cold:
        /// every first touch faults, and `refaults` says whether the kernel
        /// calls it a read-back of a page it once held.
        ColdFill { pages: f64, refaults: bool },
    }

    /// What the world served in one interval.
    #[derive(Clone, Copy, Debug, Default)]
    struct Served {
        reads: Window,
        scans: Window,
        refaults: u64,
        footprint: Option<u64>,
    }

    /// One recorded step.
    #[derive(Clone, Copy, Debug)]
    struct Rec {
        t: Duration,
        d: Decision,
        dwell: Duration,
        backoff: Duration,
        excess: f64,
        /// The form files attached, as the builder had them when the step began.
        built: f64,
        /// The major faults the interval took, through whichever form served it.
        faults: u64,
    }

    /// The rules every trace obeys, checked on every step of every trace: the
    /// level a phase publishes, the edges between phases and the events that
    /// make them, and the spacing of switches, builds and trials.
    #[derive(Default)]
    struct Checker {
        prev: Option<Rec>,
        /// When the last switch was, and the dwell it left.
        last_switch: Option<(Duration, Duration)>,
        trial_end: Option<Duration>,
    }

    impl Checker {
        fn see(&mut self, r: &Rec) -> Result<(), String> {
            let d = r.d;
            let fail = |why: &str| Err(format!("at {:?}: {why}: {d:?}", r.t));
            if d.gate != d.phase.gate() || d.census != (d.phase != Phase::Fits) {
                return fail("the levels do not follow the phase");
            }
            if (d.sample && d.phase != Phase::Suspect)
                || (d.build && !matches!(d.phase, Phase::Building | Phase::Compact))
                || (d.retire && !matches!(d.phase, Phase::Fits | Phase::Suspect))
            {
                return fail("a level or a pulse outside its phase");
            }
            let before = self.prev.map_or(Phase::Fits, |p| p.d.phase);
            match d.event {
                None if before != d.phase => return fail("a phase changed without an event"),
                None => {}
                Some(e) => {
                    let (from, to) = match e {
                        Event::Suspect => (Phase::Fits, Phase::Suspect),
                        Event::Relax => (Phase::Suspect, Phase::Fits),
                        Event::BuildStart => (Phase::Suspect, Phase::Building),
                        Event::BuildCancelled | Event::BuildAbandoned => {
                            (Phase::Building, Phase::Suspect)
                        }
                        Event::Install => (Phase::Building, Phase::Compact),
                        Event::BuildsSuspended => (Phase::Compact, Phase::Compact),
                        Event::Exit(Exit::E2) => (Phase::Trial, Phase::Suspect),
                        Event::Exit(_) => (Phase::Compact, Phase::Suspect),
                        Event::TrialStart => (Phase::Compact, Phase::Trial),
                        Event::TrialBack => (Phase::Trial, Phase::Compact),
                    };
                    if before != from || d.phase != to {
                        return fail("an event on an edge it does not make");
                    }
                }
            }
            // A build and a trial start no sooner than a dwell after the last
            // switch, and a switch no sooner than a dwell after the one before.
            let starts = matches!(d.event, Some(Event::BuildStart | Event::TrialStart));
            if starts || d.event.is_some_and(Event::is_switch) {
                if let Some((t0, dwell)) = self.last_switch {
                    if r.t - t0 < dwell {
                        return fail(&format!("inside the dwell of {dwell:?} after {t0:?}"));
                    }
                }
            }
            if d.event == Some(Event::TrialStart) {
                if let (Some(end), Some(p)) = (self.trial_end, self.prev) {
                    if r.t - end < p.backoff {
                        return fail(&format!("inside the back-off of {:?}", p.backoff));
                    }
                }
            }
            if d.event.is_some_and(Event::is_switch) {
                self.last_switch = Some((r.t, r.dwell));
            }
            match d.event {
                Some(Event::TrialBack | Event::Exit(Exit::E2)) => self.trial_end = Some(r.t),
                Some(Event::Relax) => self.trial_end = None,
                _ => {}
            }
            self.prev = Some(*r);
            Ok(())
        }
    }

    struct Sim {
        cell: Cell,
        sel: ZSelect,
        now: Duration,
        n: u64,
        load: Load,
        last: Decision,
        /// The share of the covered footprint with a form file attached.
        built: f64,
        /// Pages not yet resident (a hot set's raw pages, a cold fill).
        cold: f64,
        /// The selector asked for a sample, so the caller has one to give.
        sampler_ready: bool,
        /// The caller gives the reading when it is asked for and not
        /// otherwise, where it would give it at every step: the selector must
        /// know by itself when the reading it holds has gone stale.
        sampler_once: bool,
        declined: bool,
        /// What the partitions are, as the caller numbers them
        /// ([`Input::generation`]); a test that merges them bumps it.
        generation: u64,
        /// The caller numbers the partitions again when a form file is attached
        /// or retired, as the input's documentation asks.
        numbers_form_files: bool,
        /// Every n-th window has no commit in it (0: none): where there is
        /// none, the census cannot measure the cost of a fault.
        commit_free_every: u64,
        /// What S1 reports for the raw form instead of the geometry's.
        phi_raw_override: Option<f64>,
        /// Builds the builder reports collected at the next step.
        collected: Build,
        /// Hand the selector a census even in FITS, where the engine runs none.
        garbage: bool,
        /// The platform has a refault counter.
        counter: bool,
        /// The share of the faults the load calls refaults that the kernel
        /// counts as such.
        refault_scale: f64,
        /// The cost of a fault as the sampler's cold touches read it.
        sampler_t_f: f64,
        /// The refaults the store itself took since the last S1 tick. The
        /// group's counter holds them beside the neighbours', whichever form
        /// serves.
        own_refaults: u64,
        log: Vec<Rec>,
        checker: Checker,
    }

    impl Sim {
        fn new(cell: Cell, load: Load) -> Sim {
            Sim {
                cell,
                sel: ZSelect::new(),
                now: Duration::ZERO,
                n: 0,
                load,
                last: Decision {
                    phase: Phase::Fits,
                    gate: GATE_RAW,
                    census: false,
                    sample: false,
                    build: false,
                    retire: false,
                    event: None,
                },
                built: 0.0,
                cold: 0.0,
                sampler_ready: false,
                sampler_once: false,
                declined: false,
                generation: 0,
                numbers_form_files: true,
                commit_free_every: 1,
                phi_raw_override: None,
                collected: Build::default(),
                garbage: false,
                counter: true,
                refault_scale: 1.0,
                sampler_t_f: PRIOR_T_F,
                own_refaults: 0,
                log: Vec::new(),
                checker: Checker::default(),
            }
        }

        fn model(&self) -> Model {
            let c = &self.cell;
            let b = c.budget();
            let phi_z = self.sampler_ready.then(|| model_z(b, c.resident, c.z));
            Model {
                phi_raw: self
                    .phi_raw_override
                    .unwrap_or_else(|| model_raw(b, c.hash, c.records)),
                phi_z,
                phi_z_scan: phi_z.map(|z| z * c.scan_pages),
                refault_rate: c.neighbours + self.own_refaults as f64 / S1_PERIOD.as_secs_f64(),
                budget: b,
            }
        }

        fn sampler(&self) -> Sampler {
            let c = &self.cell;
            Sampler {
                costs: Costs {
                    t_f: Some(self.sampler_t_f),
                    t_hit: Some(c.t_hit),
                    t_hit_scan: Some(c.t_hit_scan),
                    t_z: Some(c.t_z),
                    t_z_scan: Some(c.t_z_scan),
                },
                c_build: c.price * (1.0 - self.built) + c.attach * self.built,
                declined: self.declined,
                generation: self.generation,
            }
        }

        fn serve(&mut self, z_form: bool) -> Served {
            let dt = DT.as_secs_f64();
            let window = |r: Rate| {
                let ops = (r.ops_per_s * dt).round();
                Window {
                    ops: ops as u64,
                    faults: (ops * r.phi).round() as u64,
                }
            };
            match self.load {
                Load::Idle => Served::default(),
                Load::Rated {
                    class,
                    raw,
                    z,
                    refaults,
                    bound,
                } => {
                    let w = window(if z_form { z } else { raw });
                    let mut s = Served {
                        refaults: if refaults { w.faults } else { 0 },
                        footprint: Some(bound),
                        ..Served::default()
                    };
                    match class {
                        Class::Read => s.reads = w,
                        Class::Scan => s.scans = w,
                    }
                    s
                }
                Load::Both {
                    read,
                    scan,
                    refaults,
                    bound,
                } => {
                    let r = window(if z_form { read.1 } else { read.0 });
                    let s = window(if z_form { scan.1 } else { scan.0 });
                    Served {
                        reads: r,
                        scans: s,
                        refaults: if refaults { r.faults + s.faults } else { 0 },
                        footprint: Some(bound),
                    }
                }
                Load::Hot {
                    raw_pages,
                    z_pages,
                    plateau,
                } => {
                    let c = &self.cell;
                    let footprint = Some(c.bound(z_pages));
                    if z_form {
                        // The compact pages displace the raw ones.
                        self.cold = raw_pages;
                        Served {
                            reads: window(c.rate(c.t_z, 0.0)),
                            footprint,
                            ..Served::default()
                        }
                    } else {
                        let phi = (self.cold / raw_pages).min(1.0).max(plateau);
                        let w = window(c.rate(c.t_hit, phi));
                        self.cold = (self.cold - w.faults as f64).max(0.0);
                        Served {
                            reads: w,
                            refaults: w.faults,
                            footprint,
                            ..Served::default()
                        }
                    }
                }
                Load::ColdFill { pages, refaults } => {
                    let c = &self.cell;
                    let phi = (self.cold / pages).min(1.0);
                    let w = window(c.rate(c.t_hit, phi));
                    self.cold = (self.cold - w.faults as f64).max(0.0);
                    Served {
                        reads: w,
                        refaults: if refaults { w.faults } else { 0 },
                        ..Served::default()
                    }
                }
            }
        }

        fn step(&mut self) {
            self.n += 1;
            self.now += DT;
            let dt = DT.as_secs_f64();
            let z_form = self.last.gate == GATE_Z;
            if self.last.build {
                let before = self.built;
                self.built = (self.built + dt / self.cell.build_wall).min(1.0);
                if before < 1.0 && self.built >= 1.0 && self.numbers_form_files {
                    // The last form file is attached.
                    self.generation += 1;
                }
            }
            let served = self.serve(z_form);
            if self.counter {
                self.own_refaults = self.own_refaults.saturating_add(served.refaults);
            }
            let c = &self.cell;
            let mut input = Input {
                now: self.now,
                ..Input::default()
            };
            if self.last.census || self.garbage {
                let commit_free =
                    self.commit_free_every > 0 && self.n.is_multiple_of(self.commit_free_every);
                input.census = Census {
                    reads: served.reads,
                    scans: served.scans,
                    mixed: 0,
                    refaults: self
                        .counter
                        .then_some((served.refaults as f64 * self.refault_scale).round() as u64),
                    threads: c.threads,
                    costs: Costs {
                        t_f: commit_free.then_some(c.t_f),
                        t_hit: Some(c.t_hit),
                        t_hit_scan: Some(c.t_hit_scan),
                        t_z: z_form.then_some(c.t_z),
                        t_z_scan: z_form.then_some(c.t_z_scan),
                    },
                };
            }
            input.build = Build {
                coverage: self.built,
                ..std::mem::take(&mut self.collected)
            };
            input.footprint = served.footprint;
            if self.last.sample {
                self.sampler_ready = true;
            }
            if self.last.phase == Phase::Fits {
                self.sampler_ready = false;
            }
            if self.n.is_multiple_of(TICK) {
                input.model = Some(self.model());
                self.own_refaults = 0;
            }
            input.generation = self.generation;
            if self.sampler_ready && (!self.sampler_once || self.last.sample) {
                input.sampler = Some(self.sampler());
            }
            let built = self.built;
            let d = self.sel.step(&input);
            if d.retire {
                self.built = 0.0;
                self.generation += u64::from(self.numbers_form_files);
            }
            let rec = Rec {
                t: self.now,
                d,
                dwell: self.sel.dwell(),
                backoff: self.sel.backoff(),
                excess: self.sel.excess(),
                built,
                faults: served.reads.faults + served.scans.faults,
            };
            if let Err(why) = self.checker.see(&rec) {
                panic!("{why}");
            }
            self.log.push(rec);
            self.last = d;
        }

        /// Steps until `secs` seconds more have passed.
        fn run(&mut self, secs: f64) {
            self.run_with(secs, |_| {});
        }

        /// As `run`, calling `each` before every step.
        fn run_with(&mut self, secs: f64, mut each: impl FnMut(&mut Sim)) {
            let end = self.now + secs_f(secs);
            while self.now < end {
                each(self);
                self.step();
            }
        }

        /// Steps until the phase is `phase`, for at most `secs` seconds.
        fn run_until(&mut self, phase: Phase, secs: f64) {
            let end = self.now + secs_f(secs);
            while self.now < end && self.last.phase != phase {
                self.step();
            }
        }

        fn events(&self) -> Vec<(Duration, Event)> {
            self.log
                .iter()
                .filter_map(|r| r.d.event.map(|e| (r.t, e)))
                .collect()
        }

        fn times(&self, which: Event) -> Vec<Duration> {
            self.events()
                .into_iter()
                .filter(|&(_, e)| e == which)
                .map(|(t, _)| t)
                .collect()
        }

        fn first(&self, which: Event) -> Duration {
            *self
                .times(which)
                .first()
                .unwrap_or_else(|| panic!("no {which:?} in {:?}", self.events()))
        }

        fn rec_at(&self, t: Duration) -> &Rec {
            self.log.iter().find(|r| r.t == t).expect("a step at t")
        }

        fn count(&self, which: Event) -> usize {
            self.times(which).len()
        }
    }

    /// Asserts a time is within `tol` seconds of `want`.
    fn near(got: Duration, want: f64, tol: f64) {
        let g = got.as_secs_f64();
        assert!(
            (g - want).abs() <= tol,
            "{g:.2} s is not within {tol} s of {want:.2} s"
        );
    }

    /// How fast the rent grows, in thread-seconds a second, for uniform reads
    /// at the raw form's rate: `(phi - phi_z - saving*) * t_f` an operation,
    /// that is the faults the compact form removes less the CPU it costs.
    fn rent_rate(c: &Cell) -> f64 {
        let star = (c.t_z - c.t_hit) / c.t_f;
        let phi = c.truth_raw();
        (phi - c.truth_z() - star) * c.t_f * f64::from(c.threads) / (c.t_hit + phi * c.t_f)
    }

    // ---- the pieces ----------------------------------------------------------

    #[test]
    fn the_model_reproduces_the_cell_and_the_window() {
        // The raw form at the cell, at the cache the kernel leaves and at the
        // budget S1 prices it at.
        let c = Cell::ten_million();
        assert!((model_raw(c.cache, c.hash, c.records) - 0.4278).abs() < 5e-4);
        assert!((model_raw(c.budget(), c.hash, c.records) - 0.4822).abs() < 5e-4);
        assert_eq!(model_z(c.cache, c.resident, c.z), 0.0);
        // The window: keys, raw rate, compact rate, from ten million keys to
        // two hundred million under the same cache. The raw rate passes one
        // once the hash stops fitting, and the saving between the two does not
        // fall as the store grows.
        let b = 937_000_000u64;
        for (keys, raw, z) in [
            (10.0e6, 0.428, 0.0),
            (12.7e6, 0.598, 0.0),
            (15.0e6, 0.714, 0.155),
            (18.7e6, 0.871, 0.327),
            (30.0e6, 1.200, 0.590),
            (50.0e6, 1.483, 0.764),
            (90.0e6, 1.700, 0.879),
            (200.0e6, 1.862, 0.959),
        ] {
            let (h, r, w, f) = (19.0 * keys, 130.0 * keys, 71.9 * keys, 1.74 * keys);
            let got_raw = model_raw(b, h as u64, r as u64);
            let got_z = model_z(b, f as u64, w as u64);
            assert!((got_raw - raw).abs() < 2e-3, "{keys}: raw {got_raw}");
            assert!((got_z - z).abs() < 2e-3, "{keys}: z {got_z}");
        }
    }

    #[test]
    fn the_model_is_total_and_monotone_in_the_budget() {
        // Nothing fits: every read faults on each population it touches.
        assert_eq!(model_raw(0, 1 << 20, 1 << 30), 2.0);
        assert_eq!(model_raw(0, 0, 1 << 30), 1.0);
        assert_eq!(model_raw(0, 1 << 20, 0), 1.0);
        assert_eq!(model_raw(0, 0, 0), 0.0);
        // Everything fits, to the byte: a store that fills the cache exactly
        // takes no fault and not a rounding of one.
        assert_eq!(model_raw(1 << 31, 1 << 20, 1 << 30), 0.0);
        assert_eq!(model_raw((1 << 20) + (1 << 30), 1 << 20, 1 << 30), 0.0);
        // One population.
        let one = model_raw(1 << 29, 0, 1 << 30);
        assert!(one > 0.0 && one < 1.0, "{one}");
        // A bigger cache never faults more, to the edge of fitting and past it.
        let (h, r) = (190_000_000u64, 1_300_000_000u64);
        let mut last = f64::MAX;
        for mb in (0..=1600).step_by(8) {
            let phi = model_raw(mb * 1_000_000, h, r);
            assert!(phi.is_finite() && (0.0..=2.0).contains(&phi), "{mb}: {phi}");
            assert!(phi <= last + 1e-12, "{mb}: {phi} after {last}");
            last = phi;
        }
        assert_eq!(model_z(0, 0, 0), 0.0);
        assert_eq!(model_z(10, 20, 100), 1.0);
        assert_eq!(model_z(u64::MAX, 0, 1 << 30), 0.0);
    }

    #[test]
    fn a_ring_keeps_a_window_and_forgets_the_rest() {
        let mut r: Ring<u64, 4> = Ring::new();
        for k in 0..6u64 {
            r.push(Duration::from_secs(k), k);
        }
        let vals = |r: &Ring<u64, 4>| r.iter().map(|&(_, v)| v).collect::<Vec<_>>();
        // Four fit, oldest first.
        assert_eq!(vals(&r), [2, 3, 4, 5]);
        // What is a horizon old or older goes: an entry exactly that old too,
        // so a window of a horizon at one entry a second holds that many.
        r.expire(Duration::from_secs(5), Duration::from_secs(2));
        assert_eq!(vals(&r), [4, 5]);
        r.push(Duration::from_secs(6), 6);
        assert_eq!(vals(&r), [4, 5, 6]);
        r.expire(Duration::from_secs(100), Duration::from_secs(2));
        assert!(vals(&r).is_empty());
        r.push(Duration::from_secs(101), 7);
        r.clear();
        assert!(vals(&r).is_empty());
    }

    #[test]
    fn costs_take_the_best_instrument_there_is() {
        let census = Costs {
            t_f: Some(50e-6),
            ..Costs::default()
        };
        let sampler = Costs {
            t_f: Some(30e-6),
            t_z: Some(5e-6),
            ..Costs::default()
        };
        let got = census.or(sampler).or(Costs::PRIOR);
        assert_eq!(got.t_f, Some(50e-6), "the census over the sampler");
        assert_eq!(got.t_z, Some(5e-6), "the sampler over the prior");
        assert_eq!(
            got.t_hit,
            Some(PRIOR_T_HIT),
            "the prior where nothing measured"
        );
        // A value that is not a cost is not taken.
        let bad = Costs {
            t_f: Some(f64::NAN),
            t_hit: Some(0.0),
            t_z: Some(-1.0),
            t_hit_scan: Some(f64::INFINITY),
            t_z_scan: None,
        };
        assert_eq!(bad.or(Costs::PRIOR), Costs::PRIOR);
    }

    #[test]
    fn every_threshold_is_a_multiple_of_the_break_even() {
        let s = ZSelect::new();
        let star = (PRIOR_T_Z - PRIOR_T_HIT) / PRIOR_T_F;
        assert!((0.079..=0.096).contains(&star), "{star}");
        assert_eq!(s.saving_star(Class::Read), star);
        assert_eq!(s.theta_enter(Class::Read), ENTER_FACTOR * star);
        assert_eq!(s.theta_sus(Class::Read), SUSPECT_FACTOR * star);
        assert!(s.theta_sus(Class::Read) < star && star < s.theta_enter(Class::Read));
        // A scan has its own break-even, against its own costs.
        let scan = (PRIOR_T_Z_SCAN - PRIOR_T_HIT_SCAN) / PRIOR_T_F;
        assert_eq!(s.saving_star(Class::Scan), scan);
        assert!((scan - 0.19).abs() < 0.01, "{scan}");
        // The thresholds follow the costs the census measures.
        let mut s = ZSelect::new();
        s.measured = Costs {
            t_f: Some(2.0 * PRIOR_T_F),
            ..Costs::default()
        };
        assert_eq!(s.saving_star(Class::Read), star / 2.0);
        // A form measured to cost no more CPU than a hit is not free: two
        // instruments measured the two costs, and a difference they cannot
        // resolve stands for a quarter of the prior's. (The share is the
        // registered one, written out.)
        assert_eq!(SAVING_FLOOR_SHARE, 0.25);
        let mut s = ZSelect::new();
        s.sampler = Some(Sampler {
            costs: Costs {
                t_z: Some(PRIOR_T_HIT / 2.0),
                t_z_scan: Some(PRIOR_T_HIT_SCAN / 2.0),
                ..Costs::default()
            },
            c_build: 1.0,
            declined: false,
            generation: 0,
        });
        assert_eq!(s.saving_star(Class::Read), star / 4.0);
        assert_eq!(s.saving_star(Class::Scan), scan / 4.0);
        assert_eq!(s.theta_sus(Class::Read), SUSPECT_FACTOR * star / 4.0);
        assert_eq!(s.theta_enter(Class::Read), ENTER_FACTOR * star / 4.0);
        // The floor binds from below only: a difference over it is the
        // difference, and a fault that costs more makes it smaller.
        s.measured = Costs {
            t_f: Some(2.0 * PRIOR_T_F),
            t_z: Some(2.0 * PRIOR_T_Z),
            ..Costs::default()
        };
        s.sampler = None;
        assert_eq!(
            s.saving_star(Class::Read),
            (2.0 * PRIOR_T_Z - PRIOR_T_HIT) / (2.0 * PRIOR_T_F)
        );
    }

    #[test]
    fn the_constants_are_the_quantities_they_stand_for() {
        // The table of thresholds and the numbers of the transitions, as
        // registered: a change to one of them is a change to a rule, and
        // names the quantity it stands for in the line above it.
        assert_eq!(
            (ENTER_FACTOR, SUSPECT_FACTOR, RELAX_FACTOR, VANISH_FACTOR),
            (1.5, 0.5, 0.5, 0.5)
        );
        assert_eq!(DISPLACEMENT_FACTOR, 0.5);
        assert_eq!(
            (REFAULT_SHARE, REFAULT_WINDOW),
            (0.5, Duration::from_secs(5))
        );
        assert_eq!((MIN_EVIDENCE_OPS, PRESSURE_REFAULTS_PER_S), (64, 256.0));
        assert_eq!(
            (S1_PERIOD, CENSUS_INTERVAL),
            (Duration::from_secs(1), Duration::from_millis(100))
        );
        assert_eq!(
            (
                FITS_TO_SUSPECT_TICKS,
                SUSPECT_TO_FITS_TICKS,
                VANISH_INTERVALS
            ),
            (2, 5, 3)
        );
        assert_eq!(
            (COVERED_ENOUGH, E1_SMOOTHING),
            (0.9, Duration::from_secs(30))
        );
        assert_eq!(
            (DWELL_FLOOR, DWELL_BUILD_FACTOR),
            (Duration::from_secs(60), 10)
        );
        assert_eq!(
            (RETAIN_FLOOR, RETAIN_BUILD_FACTOR),
            (Duration::from_secs(600), 20)
        );
        assert_eq!(
            (TRIAL_BACKOFF_MIN, TRIAL_BACKOFF_MAX),
            (Duration::from_secs(10 * 60), Duration::from_secs(4 * 3600))
        );
        assert_eq!(
            (E2_MEASURE, E2_MEASURES, E2_BOUND_SHARE),
            (Duration::from_secs(10), 3, 0.5)
        );
        assert_eq!(
            (TRIAL_FAULT_FACTOR, TRIAL_LOW_INTERVALS, TRIAL_WALL_FACTOR),
            (1.2, 5, 3.0)
        );
        assert_eq!(
            (TRIAL_WALL_CAP, TRIAL_DECIDE_INTERVALS),
            (Duration::from_secs(30), 3)
        );
        assert_eq!((BUDGET_BUILDS, BUDGET_SUPERSEDED, PAGE), (8, 4, 4096));
        assert_eq!(SAVING_FLOOR_SHARE, 0.25);
        // The budget remembers its builds in a byte.
        assert_eq!(BUDGET_BUILDS, u8::BITS);
        // The priors: the fault cost the banked rung implies, the in-core
        // read, and the compact form's decode and walk.
        assert_eq!(
            (PRIOR_T_F, PRIOR_T_HIT, PRIOR_T_Z),
            (41.5e-6, 0.73e-6, 4.4e-6)
        );
        assert_eq!((PRIOR_T_HIT_SCAN, PRIOR_T_Z_SCAN), (3.3e-6, 11.3e-6));
        // The rings are sized for the cadence they are read at.
        assert!(SHARE_CAP as u128 >= REFAULT_WINDOW.as_millis() / CENSUS_INTERVAL.as_millis());
        assert!(SMOOTH_CAP as u128 >= E1_SMOOTHING.as_millis() / S1_PERIOD.as_millis());
    }

    #[test]
    fn the_dwell_and_the_retention_follow_the_last_build() {
        let mut s = ZSelect::new();
        assert_eq!(
            (s.dwell(), s.retain()),
            (Duration::from_secs(60), Duration::from_secs(600))
        );
        s.last_build_wall = Duration::from_secs(20);
        assert_eq!(s.dwell(), Duration::from_secs(200));
        assert_eq!(
            s.retain(),
            Duration::from_secs(600),
            "20 x 20 s is under the floor"
        );
        s.last_build_wall = Duration::from_secs(45);
        assert_eq!(s.dwell(), Duration::from_secs(450));
        assert_eq!(s.retain(), Duration::from_secs(900));
        s.last_build_wall = Duration::MAX;
        assert_eq!(s.dwell(), Duration::MAX, "saturates");
    }

    #[test]
    fn the_ski_rental_price_adds_the_displacement() {
        assert_eq!(
            ski_rental_price(8.0, None),
            8.0 * (1.0 + DISPLACEMENT_FACTOR)
        );
        assert_eq!(ski_rental_price(8.0, Some(1.0)), 9.0);
        // A displacement measured at nothing is a measure.
        assert_eq!(ski_rental_price(8.0, Some(0.0)), 8.0);
        assert_eq!(ski_rental_price(8.0, Some(f64::NAN)), 12.0);
        assert_eq!(ski_rental_price(8.0, Some(-1.0)), 12.0);
    }

    #[test]
    fn the_selector_names_no_clock_thread_file_or_engine_type() {
        // The core is testable by replay because it reaches nothing: the
        // part of this file before its tests uses the one std type that is
        // only a number, and no sibling.
        let src = include_str!("zselect.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        let banned = [
            "Instant",
            "SystemTime",
            "std::thread",
            "std::fs",
            "std::io",
            "std::sync",
            "std::env",
            "std::process",
            "libc",
            "crate::",
            "super::",
            "unsafe",
            "static ",
        ];
        for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for b in banned {
                assert!(!line.contains(b), "`{b}` in `{line}`");
            }
            if line.starts_with("use ") {
                assert_eq!(line, "use std::time::Duration;");
            }
        }
    }

    // ---- the harness holds the rules, and is shown to ------------------------

    fn rec(t: u64, phase: Phase, event: Option<Event>) -> Rec {
        Rec {
            t: Duration::from_secs(t),
            d: Decision {
                phase,
                gate: phase.gate(),
                census: phase != Phase::Fits,
                sample: false,
                build: matches!(phase, Phase::Building | Phase::Compact),
                retire: false,
                event,
            },
            dwell: DWELL_FLOOR,
            backoff: TRIAL_BACKOFF_MIN,
            excess: 0.0,
            built: 0.0,
            faults: 0,
        }
    }

    #[test]
    fn the_checker_fires_on_a_red_log() {
        let entered = |ck: &mut Checker| {
            for r in [
                rec(1, Phase::Suspect, Some(Event::Suspect)),
                rec(2, Phase::Building, Some(Event::BuildStart)),
                rec(100, Phase::Compact, Some(Event::Install)),
            ] {
                ck.see(&r).unwrap();
            }
        };
        // Green: leaves COMPACT a dwell after it was entered.
        let mut ck = Checker::default();
        entered(&mut ck);
        ck.see(&rec(100 + 60, Phase::Suspect, Some(Event::Exit(Exit::E1))))
            .unwrap();
        // Red: a second switch inside the dwell, and a trial or a build.
        for (phase, event) in [
            (Phase::Suspect, Event::Exit(Exit::E3)),
            (Phase::Trial, Event::TrialStart),
        ] {
            let mut ck = Checker::default();
            entered(&mut ck);
            assert!(ck.see(&rec(130, phase, Some(event))).is_err(), "{event:?}");
        }
        let mut ck = Checker::default();
        entered(&mut ck);
        ck.see(&rec(160, Phase::Suspect, Some(Event::Exit(Exit::E1))))
            .unwrap();
        assert!(ck
            .see(&rec(170, Phase::Building, Some(Event::BuildStart)))
            .is_err());
        // Red: a trial inside the back-off of the one before.
        let mut ck = Checker::default();
        entered(&mut ck);
        ck.see(&rec(160, Phase::Trial, Some(Event::TrialStart)))
            .unwrap();
        ck.see(&rec(170, Phase::Compact, Some(Event::TrialBack)))
            .unwrap();
        assert!(ck
            .see(&rec(300, Phase::Trial, Some(Event::TrialStart)))
            .is_err());
        // Green: the same trial after the back-off.
        ck.see(&rec(170 + 600, Phase::Trial, Some(Event::TrialStart)))
            .unwrap();
        // Red: a phase that changed with no event, an event on an edge it does
        // not make, a gate that does not follow the phase, FITS that samples.
        let mut ck = Checker::default();
        assert!(ck.see(&rec(1, Phase::Suspect, None)).is_err());
        assert!(Checker::default()
            .see(&rec(1, Phase::Compact, Some(Event::Install)))
            .is_err());
        let mut bad = rec(1, Phase::Fits, None);
        bad.d.gate = GATE_CENSUS;
        assert!(Checker::default().see(&bad).is_err());
        let mut bad = rec(1, Phase::Fits, None);
        bad.d.sample = true;
        assert!(Checker::default().see(&bad).is_err());
    }

    #[test]
    fn two_selectors_share_nothing() {
        let (ten, three) = (Cell::ten_million(), Cell::three_million());
        let mut a = Sim::new(ten.clone(), ten.uniform());
        let mut b = Sim::new(three.clone(), three.uniform());
        let mut alone = Sim::new(ten.clone(), ten.uniform());
        for _ in 0..600 {
            a.step();
            b.step();
            alone.step();
        }
        assert_ne!(a.last.phase, Phase::Fits);
        assert_eq!(b.last.phase, Phase::Fits);
        let decisions = |s: &Sim| s.log.iter().map(|r| r.d).collect::<Vec<_>>();
        assert_eq!(decisions(&a), decisions(&alone));
    }

    // ---- in memory -------------------------------------------------------------

    #[test]
    fn in_memory_never_leaves_fits_and_never_asks_for_a_sample() {
        let four_gib = Cell {
            cache: 4 << 30,
            ..Cell::ten_million()
        };
        for cell in [Cell::three_million(), four_gib] {
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            // A census nobody ran is ignored: the engine runs none in FITS.
            sim.garbage = true;
            sim.run(2.0 * 3600.0);
            let silent = Decision {
                phase: Phase::Fits,
                gate: GATE_RAW,
                census: false,
                sample: false,
                build: false,
                retire: false,
                event: None,
            };
            assert!(sim.log.iter().all(|r| r.d == silent));
            let c = sim.sel.counts();
            assert_eq!((c.samples_wanted, c.builds, c.switches), (0, 0, 0));
            assert_eq!(c.s1_ticks, 2 * 3600);
            assert_eq!(sim.sel.excess(), 0.0);
        }
    }

    // ---- the cell --------------------------------------------------------------

    #[test]
    fn the_price_is_paid_at_the_rate_the_form_saves_net_of_its_cpu() {
        // The design registers the price paid in about nine seconds on one
        // thread and three on three. That is the build's CPU, eight
        // thread-seconds, at the gross rate the raw form stalls (0.96
        // thread-seconds a second). The rent is net of the extra CPU the
        // form costs, 0.76 thread-seconds a second at the cell, and the price
        // the module asks is the CPU and the displacement it adds until it
        // is measured, twelve seconds: the build starts three seconds after
        // SUSPECT (the evidence needs the sampler's reading in the model) and
        // then at 13.4 s on one thread and 6.4 on three if the price is the
        // CPU alone, 18.7 and 8.2 if it is the module's.
        let cpu = 8.0;
        assert_eq!(Cell::ten_million().price, ski_rental_price(cpu, None));
        assert_eq!(Cell::ten_million().price, 1.5 * cpu);
        for (threads, alone, priced) in [(1u32, 13.4, 18.7), (3, 6.4, 8.2)] {
            for (price, want) in [(cpu, alone), (ski_rental_price(cpu, None), priced)] {
                let cell = Cell {
                    threads,
                    price,
                    ..Cell::ten_million()
                };
                let mut sim = Sim::new(cell.clone(), cell.uniform());
                sim.run(60.0);
                near(sim.first(Event::BuildStart), want, 0.25);
            }
        }
    }

    #[test]
    fn the_ten_million_cell_pays_the_price_builds_and_installs() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run(120.0);
        // SUSPECT within two S1 ticks of the first read, the sampler asked
        // for once and not again once its reading is in.
        assert_eq!(sim.first(Event::Suspect), 2 * S1_PERIOD);
        assert_eq!(sim.sel.counts().samples_wanted, 1);
        assert_eq!(sim.log.iter().filter(|r| r.d.sample).count(), 1);
        // The census has evidence once the model carries the compact form's
        // rate, a tick after the sampler's reading; the price is paid at the
        // rate the form is predicted to save net of its CPU.
        let start = sim.first(Event::BuildStart);
        near(start, 3.0 + cell.price / rent_rate(&cell), 0.3);
        assert!(sim.rec_at(start).excess >= cell.price, "bought on the rent");
        // The build is COMPACT once the covered share is attached, and the
        // dwell is ten of the wall it took.
        let install = sim.first(Event::Install);
        near(
            install,
            start.as_secs_f64() + COVERED_ENOUGH * cell.build_wall,
            0.3,
        );
        let dwell = sim.rec_at(install).dwell;
        near(dwell, 10.0 * (install - start).as_secs_f64(), 0.01);
        assert_eq!(sim.rec_at(install).excess, 0.0, "the rent is spent");
        assert_eq!((sim.last.phase, sim.last.gate), (Phase::Compact, GATE_Z));
        // And it stays: the form pays, so nothing leaves it.
        let c = sim.sel.counts();
        assert_eq!((c.builds, c.installs, c.switches), (1, 1, 1));
        assert_eq!(sim.events().len(), 3);
    }

    #[test]
    fn three_threads_pay_the_price_in_a_third_of_the_time() {
        let cell = Cell {
            threads: 3,
            ..Cell::ten_million()
        };
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run(120.0);
        assert_eq!(sim.first(Event::Suspect), 2 * S1_PERIOD);
        let start = sim.first(Event::BuildStart);
        near(start, 3.0 + cell.price / rent_rate(&cell), 0.3);
        let one = Cell::ten_million();
        assert!(3.0 * rent_rate(&one) < rent_rate(&cell) * 1.01);
        assert!(start.as_secs_f64() < 3.0 + 0.4 * one.price / rent_rate(&one));
        assert_eq!(sim.last.phase, Phase::Compact);
    }

    #[test]
    fn a_restart_with_valid_form_files_reaches_compact_in_about_a_second() {
        // The form files are on disk: the price is the attach's, the builder
        // has nothing to build, and the form is bought on the first rent.
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.built = 1.0;
        sim.run(30.0);
        let suspect = sim.first(Event::Suspect);
        let start = sim.first(Event::BuildStart);
        let install = sim.first(Event::Install);
        assert!(start - suspect <= Duration::from_millis(1500), "{start:?}");
        assert_eq!(install - start, DT);
        assert!(
            install - suspect <= Duration::from_millis(2000),
            "{install:?}"
        );
        assert_eq!(sim.last.phase, Phase::Compact);
        // A build that took a step makes a dwell of the floor.
        assert_eq!(sim.sel.dwell(), DWELL_FLOOR);
    }

    // ---- evidence ----------------------------------------------------------------

    #[test]
    fn a_cold_start_is_not_a_capacity_miss() {
        // After a restart every first touch faults and the rate reads as a
        // footprint loss until the store is resident. The model, which prices
        // the store against a budget that is short of it, says SUSPECT; the
        // fill stalls for several prices; and still the form is not bought,
        // because none of those faults is a page the cache once held. The
        // same fill with its faults counted as refaults is bought, so it is
        // the rule and not the rate that tells the two apart.
        let cell = Cell {
            price: 3.0,
            ..Cell::ten_million()
        };
        let pages = (cell.hash + cell.records) as f64 / PAGE as f64;
        assert!(
            pages * cell.t_f > 4.0 * cell.price,
            "a fill is several prices"
        );
        for refaults in [false, true] {
            let mut sim = Sim::new(cell.clone(), Load::ColdFill { pages, refaults });
            sim.cold = pages;
            sim.phi_raw_override = Some(0.48);
            sim.run(120.0);
            assert_eq!(sim.first(Event::Suspect), 2 * S1_PERIOD);
            let peak = sim.log.iter().map(|r| r.excess).fold(0.0, f64::max);
            if refaults {
                assert_eq!(sim.sel.counts().builds, 1, "a fill that refaults is bought");
                assert!(peak >= cell.price);
            } else {
                assert_eq!(sim.sel.counts().builds, 0);
                assert_eq!(peak, 0.0, "no first touch is evidence");
                assert_eq!(sim.last.phase, Phase::Suspect);
            }
        }
    }

    #[test]
    fn an_interval_is_evidence_at_half_refaults_and_not_under_it() {
        // A capacity miss is a page the cache once held, and a first touch is
        // not: the share of an interval's faults that the kernel calls
        // refaults has to reach half. (The share is the registered one,
        // written out.)
        let cell = Cell::ten_million();
        for (share, evidence) in [(0.45, false), (0.55, true)] {
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            sim.refault_scale = share;
            sim.run(60.0);
            let peak = sim.log.iter().map(|r| r.excess).fold(0.0, f64::max);
            assert_eq!(peak > 0.0, evidence, "{share}");
        }
    }

    #[test]
    fn the_refault_share_is_taken_over_the_last_five_seconds() {
        // First touches for longer than the window is long, and then the same
        // faults as refaults: evidence begins when refaults are half of what
        // the window holds, which is half a window after the change. (The
        // window is the registered one, written out.)
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let faulting = |refaults| Load::Rated {
            class: Class::Read,
            raw: Rate {
                ops_per_s: 51_000.0,
                phi: 0.45,
            },
            z: Rate {
                ops_per_s: 200_000.0,
                phi: 0.0,
            },
            refaults,
            bound,
        };
        let mut sim = Sim::new(cell.clone(), faulting(false));
        sim.run(10.0);
        assert!(sim.log.iter().all(|r| r.excess == 0.0));
        sim.load = faulting(true);
        let change = sim.now;
        sim.run(10.0);
        let began = sim
            .log
            .iter()
            .find(|r| r.t > change && r.excess > 0.0)
            .expect("refaults are evidence")
            .t;
        near(began - change, 2.5, 0.15);
    }

    #[test]
    fn the_evidence_starts_again_when_a_build_is_cancelled() {
        // The seconds before a cancel are the build's own, and their faults
        // were refaults. A window that carried them into SUSPECT would vouch
        // for the first touches of a cache that has gone cold, so the share
        // is taken of what SUSPECT has seen. The same fill counted as
        // refaults is evidence at once, so it is the window and not the load
        // that tells the two apart.
        let cell = Cell::ten_million();
        let star = (cell.t_z - cell.t_hit) / cell.t_f;
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let pages = (cell.hash + cell.records) as f64 / PAGE as f64;
        for refaults in [false, true] {
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            sim.run_until(Phase::Building, 60.0);
            sim.load = cell.fixed(0.2 * star, 0.0, bound);
            sim.run_until(Phase::Suspect, 60.0);
            assert_eq!(sim.count(Event::BuildCancelled), 1);
            let cancelled = sim.now;
            sim.cold = pages;
            sim.load = Load::ColdFill { pages, refaults };
            sim.run(10.0);
            let rents: Vec<f64> = sim
                .log
                .iter()
                .filter(|r| r.t > cancelled)
                .map(|r| r.excess)
                .collect();
            assert!(rents.len() >= 100);
            if refaults {
                assert!(
                    rents[0] > 0.0,
                    "a refault is evidence from the first interval"
                );
            } else {
                assert!(
                    rents.iter().all(|&e| e == 0.0),
                    "no first touch is evidence, whatever came before"
                );
            }
        }
    }

    #[test]
    fn where_there_is_no_refault_counter_the_faults_since_suspect_stand_in() {
        // Without the counter an interval is evidence once the faults taken
        // since SUSPECT reach the cache's size in pages: a cache that holds
        // the store takes no more first touches than the store has pages, and
        // one that does not holds the pages it reads only until it has read
        // its size. (The price is small: what is left of a fill after the
        // cache is full is a second or two of stall, net of the form's CPU.)
        let cell = Cell {
            price: 1.0,
            ..Cell::ten_million()
        };
        let cache_pages = cell.budget() / PAGE;
        for (pages, builds) in [(0.7 * cache_pages as f64, 0), (1.7 * cache_pages as f64, 1)] {
            let mut sim = Sim::new(
                cell.clone(),
                Load::ColdFill {
                    pages,
                    refaults: false,
                },
            );
            sim.counter = false;
            sim.cold = pages;
            sim.phi_raw_override = Some(0.48);
            let mut crossed = None;
            while sim.now < Duration::from_secs(120) {
                sim.step();
                if crossed.is_none() && sim.sel.excess() > 0.0 {
                    crossed = Some(sim.sel.faults_since_suspect);
                }
            }
            assert_eq!(sim.sel.counts().builds, builds, "{pages} pages");
            if builds == 1 {
                let at = crossed.expect("evidence began");
                assert!((cache_pages..cache_pages + 5000).contains(&at), "{at}");
            } else {
                assert_eq!(crossed, None);
            }
        }
    }

    #[test]
    fn a_burst_of_uniform_reads_on_a_skewed_store_does_not_build() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.hot(100));
        // The model prices the store, not the workload on it.
        sim.phi_raw_override = Some(0.48);
        sim.run(30.0);
        assert_eq!(sim.last.phase, Phase::Suspect);
        sim.load = cell.uniform();
        sim.run(0.3);
        let peak = sim.sel.excess();
        assert!(peak > 0.0, "the burst is evidence");
        assert!(
            peak < cell.price / 20.0,
            "{peak} s of a price of {} s",
            cell.price
        );
        sim.load = cell.hot(100);
        sim.run(120.0);
        assert_eq!(sim.sel.counts().builds, 0);
        assert_eq!(sim.sel.excess(), 0.0, "the reads that follow pay it back");
    }

    #[test]
    fn a_skewed_workload_that_fits_never_builds() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.hot(100));
        sim.phi_raw_override = Some(0.48);
        sim.run(3600.0);
        let c = sim.sel.counts();
        assert_eq!(
            (c.builds, c.installs, c.switches, c.samples_wanted),
            (0, 0, 0, 1)
        );
        // SUSPECT, which costs a countdown: no build, no file, and the
        // sampler asked once.
        assert_eq!(sim.last.phase, Phase::Suspect);
        assert!(sim.log.iter().all(|r| r.excess == 0.0 && !r.d.build));
        assert_eq!(sim.events().len(), 1);
    }

    #[test]
    fn a_long_hot_stretch_does_not_owe_the_next_entry() {
        // The form would have cost CPU for an hour of reads that never
        // faulted. A running balance would have to climb out of that debt; the
        // rent is the best net saving since any moment, which an hour of
        // losses sets back to nothing and no further.
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.hot(100));
        sim.phi_raw_override = Some(0.48);
        sim.run(3600.0);
        assert_eq!(sim.sel.excess(), 0.0);
        let t0 = sim.now;
        sim.load = cell.uniform();
        sim.run(60.0);
        let start = sim.first(Event::BuildStart);
        near(start - t0, cell.price / rent_rate(&cell), 0.3);
    }

    #[test]
    fn the_rent_halves_for_every_dwell_without_evidence() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run(8.0);
        let rent = sim.sel.excess();
        assert!(rent > 1.0 && rent < cell.price, "{rent}");
        // Nothing reads: no interval is evidence, and a stale rent is not a
        // credit.
        sim.load = Load::Idle;
        let anchor = sim.now;
        let dwell = sim.sel.dwell();
        sim.run_with(dwell.as_secs_f64() - 0.1, |_| {});
        assert_eq!(sim.now - anchor, dwell - DT);
        assert_eq!(sim.sel.excess(), rent, "not before a whole dwell");
        sim.run(0.1);
        assert_eq!(sim.sel.excess(), rent / 2.0);
        sim.run(dwell.as_secs_f64());
        assert_eq!(sim.sel.excess(), rent / 4.0);
        assert_eq!(sim.sel.counts().builds, 0);
    }

    // ---- hysteresis ----------------------------------------------------------------

    /// Steps a selector once an S1 period through `secs` periods of a model
    /// the closure describes -- `(phi_raw, refault rate)` at the tick, `t`
    /// counting from the start of this stretch -- and returns the events and
    /// the seconds they came at. Time goes on from where the selector left it.
    fn drive(sel: &mut ZSelect, secs: u64, model: impl Fn(u64) -> (f64, f64)) -> Vec<(u64, Event)> {
        let mut events = Vec::new();
        let base = sel.now.as_secs();
        for t in 0..secs {
            let (phi_raw, refault_rate) = model(t);
            let d = sel.step(&Input {
                now: Duration::from_secs(base + t + 1),
                model: Some(Model {
                    phi_raw,
                    phi_z: None,
                    phi_z_scan: None,
                    refault_rate,
                    budget: 870_000_000,
                }),
                ..Input::default()
            });
            if let Some(e) = d.event {
                events.push((base + t + 1, e));
            }
        }
        events
    }

    fn kinds(events: &[(u64, Event)]) -> Vec<Event> {
        events.iter().map(|&(_, e)| e).collect()
    }

    #[test]
    fn a_flapping_signal_changes_nothing() {
        let sus = ZSelect::new().theta_sus(Class::Read);
        let hour = 3600;
        // Either side of the threshold every tick: never two hot ticks running.
        let ev = drive(&mut ZSelect::new(), hour, |t| {
            (if t % 2 == 0 { 1.1 * sus } else { 0.9 * sus }, 0.0)
        });
        assert!(ev.is_empty(), "{ev:?}");
        // Two hot and two just under: it enters once, and just under is not
        // calm, which is under half.
        let ev = drive(&mut ZSelect::new(), hour, |t| {
            (if t % 4 < 2 { 1.1 * sus } else { 0.9 * sus }, 0.0)
        });
        assert_eq!(kinds(&ev), [Event::Suspect]);
        // Three hot and three calm: enters once, and three is not five.
        let ev = drive(&mut ZSelect::new(), hour, |t| {
            (if t % 6 < 3 { 1.5 * sus } else { 0.1 * sus }, 0.0)
        });
        assert_eq!(kinds(&ev), [Event::Suspect]);
        // Four calm and one hot: never five calm running.
        let ev = drive(&mut ZSelect::new(), hour, |t| {
            (
                if t < 2 || t % 5 == 4 {
                    1.5 * sus
                } else {
                    0.1 * sus
                },
                0.0,
            )
        });
        assert_eq!(kinds(&ev), [Event::Suspect]);
        // The pressure term is the same: above its threshold every other tick.
        let ev = drive(&mut ZSelect::new(), hour, |t| {
            (0.0, if t % 2 == 0 { 300.0 } else { 200.0 })
        });
        assert!(ev.is_empty(), "{ev:?}");
        // Held for two ticks it enters, and held calm for five it leaves.
        let ev = drive(&mut ZSelect::new(), 20, |t| {
            (0.0, if t < 2 { 300.0 } else { 0.0 })
        });
        assert_eq!(ev, [(2, Event::Suspect), (7, Event::Relax)]);
        // A signal that really moves is followed, a transition for each way.
        let ev = drive(&mut ZSelect::new(), hour, |t| {
            (if t % 40 < 20 { 1.5 * sus } else { 0.1 * sus }, 0.0)
        });
        assert_eq!(ev.len(), 2 * (hour as usize / 40));
    }

    #[test]
    fn suspect_is_entered_at_half_the_break_even_and_not_under_it() {
        // `theta_sus` is half of `saving*`, written out: a model that reads
        // 45% of the break-even is a store that fits, and one that reads 55%
        // is not sure to.
        let star = (PRIOR_T_Z - PRIOR_T_HIT) / PRIOR_T_F;
        let ev = drive(&mut ZSelect::new(), 3600, |_| (0.45 * star, 0.0));
        assert!(ev.is_empty());
        let ev = drive(&mut ZSelect::new(), 10, |_| (0.55 * star, 0.0));
        assert_eq!(kinds(&ev), [Event::Suspect]);
    }

    #[test]
    fn a_signal_between_the_two_thresholds_holds_its_phase() {
        let sus = ZSelect::new().theta_sus(Class::Read);
        let hour = 3600;
        // Under the way in: FITS stays FITS.
        let ev = drive(&mut ZSelect::new(), hour, |_| (0.9 * sus, 0.0));
        assert!(ev.is_empty());
        // Over the way in for two ticks and then in the band for an hour:
        // SUSPECT is entered and not left, and four ticks under half are not
        // five.
        let mut s = ZSelect::new();
        let ev = drive(&mut s, hour, |t| {
            (if t < 2 { 1.5 * sus } else { 0.9 * sus }, 0.0)
        });
        assert_eq!(kinds(&ev), [Event::Suspect]);
        let ev = drive(&mut s, 4, |_| (0.4 * sus, 0.0));
        assert!(ev.is_empty());
        // Five are.
        let ev = drive(&mut s, 1, |_| (0.4 * sus, 0.0));
        assert_eq!(kinds(&ev), [Event::Relax]);
        // COMPACT is the same: a model that is in the band for hours does not
        // take the form back, and one under half of it for a dwell does.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.load = Load::Idle;
        sim.phi_raw_override = Some(0.9 * sus);
        sim.run(5.0 * sim.sel.dwell().as_secs_f64());
        assert_eq!(sim.last.phase, Phase::Compact);
        sim.phi_raw_override = Some(0.4 * sus);
        sim.run(2.0 * sim.sel.dwell().as_secs_f64());
        assert_eq!(sim.count(Event::Exit(Exit::E1)), 1);
    }

    #[test]
    fn e1s_stretch_is_the_maximum_over_a_dwell_of_the_smoothed_model() {
        // The rule: the maximum of the smoothed model over the last dwell is
        // under half the threshold. The selector keeps where the current
        // stretch under it began and not a window of readings; this holds the
        // two to each other, on readings that run at nothing and at half a
        // fault for stretches a minute and more long.
        let mut rng = Rng::new(7);
        let mut sel = ZSelect::new();
        let thr = sel.theta_sus(Class::Read) * RELAX_FACTOR;
        let dwell = sel.dwell().as_secs();
        let window = E1_SMOOTHING.as_secs() as usize;
        let mut raw: Vec<f64> = Vec::new();
        let mut level = 0.0;
        let mut fired = 0;
        for k in 1..=20_000u64 {
            if rng.below(150) == 0 {
                level = if rng.below(2) == 0 { 0.0 } else { 0.5 };
            }
            raw.push(level);
            sel.step(&Input {
                now: Duration::from_secs(k),
                model: Some(Model {
                    phi_raw: level,
                    phi_z: None,
                    phi_z_scan: None,
                    refault_rate: 0.0,
                    budget: 870_000_000,
                }),
                ..Input::default()
            });
            // The smoothed series by its definition: the mean of the readings
            // of the last thirty seconds, one a second.
            let smooth = |j: usize| {
                let from = (j + 1).saturating_sub(window);
                raw[from..=j].iter().sum::<f64>() / (j + 1 - from) as f64
            };
            let literal = k > dwell && (k - dwell - 1..k).all(|j| smooth(j as usize) < thr);
            let kept = sel
                .low_since
                .is_some_and(|t| Duration::from_secs(k) - t >= sel.dwell());
            assert_eq!(literal, kept, "tick {k}");
            fired += usize::from(literal);
        }
        assert!(
            fired > 1000,
            "the series did not run long enough under: {fired}"
        );
    }

    #[test]
    fn a_square_wave_switches_at_most_once_per_dwell() {
        // Uniform reads and a hot set of one percent, half a period each.
        // The model prices the store and says SUSPECT throughout; the census
        // is what moves. The checker holds a dwell between every two switches,
        // builds and trials on every step; here it is also counted.
        for period in [10.0, 60.0, 300.0] {
            let cell = Cell::ten_million();
            let (uniform, hot) = (cell.uniform(), cell.hot(13));
            let mut sim = Sim::new(cell.clone(), uniform);
            sim.run_with(3600.0, |s| {
                let half = (s.now.as_secs_f64() / (period / 2.0)) as u64;
                s.load = if half.is_multiple_of(2) { uniform } else { hot };
            });
            let c = sim.sel.counts();
            assert!(
                c.switches <= 3600 / DWELL_FLOOR.as_secs() + 1,
                "{period}: {c:?}"
            );
            // A build from nothing once for each entry: after the form files are
            // retired and not before.
            let scratch = sim
                .log
                .iter()
                .filter(|r| r.d.event == Some(Event::BuildStart) && r.built == 0.0)
                .count() as u64;
            assert!(
                scratch <= 1 + c.retired,
                "{period}: {scratch} builds from nothing"
            );
            match period as u64 {
                10 => assert_eq!(c.builds, 0, "a uniform half under the price"),
                300 => assert!(c.installs >= 1, "{period}: {c:?}"),
                _ => {}
            }
        }
    }

    // ---- the exits ---------------------------------------------------------------

    /// A store that has bought the form on uniform reads and is COMPACT.
    fn compact(cell: &Cell) -> Sim {
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Compact, 120.0);
        assert_eq!(sim.last.phase, Phase::Compact);
        sim
    }

    #[test]
    fn e1_leaves_when_the_limit_is_lifted_and_the_form_files_go_after_the_retention() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        let dwell = sim.sel.dwell();
        // The limit is lifted to four gibibytes and nothing reads, so E3,
        // which reads the form's own faults, has nothing to act on.
        sim.cell.cache = 4 << 30;
        sim.load = Load::Idle;
        let lifted = sim.now;
        sim.run(1200.0);
        // The model's readings are smoothed over thirty seconds, so the mean
        // is under half the threshold from the twenty-ninth reading of zero;
        // E1 wants a dwell of that.
        let exit = sim.first(Event::Exit(Exit::E1));
        near(exit - lifted, 29.0 + dwell.as_secs_f64(), 2.0);
        // FITS within five S1 ticks of leaving.
        let relax = sim.first(Event::Relax);
        assert!(
            relax > exit && relax - exit <= 6 * S1_PERIOD,
            "{exit:?} {relax:?}"
        );
        assert_eq!(sim.last.phase, Phase::Fits);
        // The form files are kept for the retention and retired once, after it.
        let retired: Vec<Duration> = sim.log.iter().filter(|r| r.d.retire).map(|r| r.t).collect();
        assert_eq!(retired.len(), 1);
        near(retired[0] - exit, sim.sel.retain().as_secs_f64(), 0.2);
        assert!(!sim.sel.retained());
        let c = sim.sel.counts();
        assert_eq!((c.exits, c.retired, c.builds), ([1, 0, 0], 1, 1));
    }

    #[test]
    fn e1_waits_for_a_dwell_of_the_model_saying_it_fits() {
        // Not a swing: the limit is lifted and lowered again every half
        // minute. The model fits for a stretch that never reaches a dwell, so
        // the form is not left on a signal that is gone before the build would
        // have been repaid.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.load = Load::Idle;
        sim.run_with(3600.0, |s| {
            let lifted = ((s.now.as_secs_f64() / 30.0) as u64).is_multiple_of(2);
            s.cell.cache = if lifted { 4 << 30 } else { 937_000_000 };
        });
        assert_eq!(sim.last.phase, Phase::Compact);
        assert_eq!(sim.sel.counts().switches, 1);
    }

    #[test]
    fn e3_leaves_a_form_that_is_not_paying_and_stays_with_one_that_is() {
        // The sampler's thirty-two groups said the compact form would hold
        // the store; the pages it really needs do not fit, and it faults at
        // 0.40 a read where the raw form faults at 0.45. The saving it
        // realizes, the model's 0.48 less 0.40, is under the break-even of
        // 0.088, though the raw rate alone says the form is worth having. At
        // 0.30 it pays.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        for (z, leaves) in [(0.40, true), (0.30, false)] {
            let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, z, bound));
            sim.run_until(Phase::Compact, 120.0);
            let install = sim.first(Event::Install);
            let dwell = sim.sel.dwell();
            sim.run(if leaves {
                2.0 * dwell.as_secs_f64()
            } else {
                3.0 * 3600.0
            });
            if leaves {
                // From the first S1 tick after the install, for a dwell.
                let exit = sim.first(Event::Exit(Exit::E3));
                near(exit - install, dwell.as_secs_f64(), 2.0);
                assert!(sim.sel.retained());
            } else {
                assert_eq!(sim.last.phase, Phase::Compact);
                assert_eq!(sim.sel.counts().switches, 1);
            }
        }
    }

    #[test]
    fn a_form_seen_not_to_pay_is_not_bought_again_on_the_models_word() {
        // The model predicts the compact form free of faults and it takes 0.40
        // a read where the raw form takes 0.45. Left by E3, it is not bought
        // again for as long as the model's word is all there is: without the
        // form's measured rate kept, the store installs and leaves it every
        // other dwell, an hour long and more.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 0.40, bound));
        sim.run_until(Phase::Compact, 120.0);
        sim.run(3.0 * 3600.0);
        assert_eq!(sim.count(Event::Install), 1);
        assert_eq!(sim.count(Event::Exit(Exit::E3)), 1);
        assert_eq!(sim.count(Event::BuildStart), 1);
        assert_eq!(sim.last.phase, Phase::Suspect);
        let kept = sim.sel.z_realized.expect("the rate is kept");
        assert!((kept - 0.40).abs() < 0.005, "{kept}");
        // The raw form now faults at 0.9, which the model has not heard of.
        // Its own refaults put the pressure term on, which stands for a model
        // that cannot see, so the clause asks no more of the model, and the
        // measured saving, 0.9 against the form's 0.40, buys it again.
        sim.load = cell.fixed(0.9, 0.40, bound);
        sim.run(600.0);
        assert_eq!(sim.count(Event::BuildStart), 2);
        assert_eq!(sim.last.phase, Phase::Compact);
        assert_eq!(sim.sel.z_realized, None, "a new install is judged afresh");
        // And it stays: judged against the 0.9 the census saw and not the
        // model's 0.48, which less its 0.40 is under the break-even.
        assert!(
            (sim.sel.bias - (0.9 - 0.4822)).abs() < 0.01,
            "{}",
            sim.sel.bias
        );
        sim.run(2.0 * 3600.0);
        assert_eq!(sim.last.phase, Phase::Compact);
        assert_eq!(sim.count(Event::Exit(Exit::E3)), 1);
    }

    #[test]
    fn the_predicted_saving_gates_a_store_too_slow_to_raise_the_pressure_itself() {
        // Seven hundred reads a second at 0.3 faults a read is 210 refaults a
        // second, under the pressure term, and an interval of seventy reads is
        // evidence. The model says the raw form takes a tenth of a fault, so
        // the saving it predicts, a tenth, is under the entry threshold
        // however the census reads: the clause asks the model, and refuses.
        // With a neighbour's refaults added the pressure term is on, it asks
        // no more, and the measured saving buys.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let slow = Load::Rated {
            class: Class::Read,
            raw: Rate {
                ops_per_s: 700.0,
                phi: 0.3,
            },
            z: Rate {
                ops_per_s: 3_000.0,
                phi: 0.0,
            },
            refaults: true,
            bound,
        };
        const { assert!(700.0 * 0.3 < PRESSURE_REFAULTS_PER_S) };
        let enter = ZSelect::new().theta_enter(Class::Read);
        assert!(0.1 < enter && enter < 0.3);
        for (neighbours, builds) in [(0.0, 0), (400.0, 1)] {
            let mut sim = Sim::new(
                Cell {
                    neighbours,
                    ..cell.clone()
                },
                slow,
            );
            sim.phi_raw_override = Some(0.1);
            sim.run(3600.0);
            assert_eq!(sim.sel.counts().builds, builds, "{neighbours}");
            if builds == 0 {
                assert!(
                    sim.log.iter().any(|r| r.excess >= cell.price),
                    "the rent was paid"
                );
                assert_eq!(sim.last.phase, Phase::Suspect);
            }
        }
    }

    #[test]
    fn the_rate_kept_goes_with_the_episode() {
        // Left by E3 and then the store fits (the limit is lifted): the next
        // episode starts from the model's word again.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 0.40, bound));
        sim.run_until(Phase::Compact, 120.0);
        sim.run(2.0 * sim.sel.dwell().as_secs_f64());
        assert!(sim.sel.z_realized.is_some());
        sim.phi_raw_override = Some(0.0);
        sim.load = Load::Idle;
        sim.run(60.0);
        assert_eq!(sim.last.phase, Phase::Fits);
        assert_eq!(sim.sel.z_realized, None);
    }

    #[test]
    fn the_rate_kept_is_the_whole_stretchs_and_not_the_last_second() {
        // Seconds at 0.40 and seconds at 0.50 a read, neither paying: what is
        // kept is what the stretch took, weighted by the reads each second
        // made, and not the second the exit happened to fall in.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 0.40, bound));
        sim.run_until(Phase::Compact, 120.0);
        sim.run_with(2.0 * sim.sel.dwell().as_secs_f64(), |s| {
            let z = if (s.n / TICK).is_multiple_of(2) {
                0.40
            } else {
                0.50
            };
            s.load = s.cell.fixed(0.45, z, bound);
        });
        sim.first(Event::Exit(Exit::E3));
        let (a, b) = (cell.rate(cell.t_z, 0.40), cell.rate(cell.t_z, 0.50));
        let want = (a.ops_per_s * 0.40 + b.ops_per_s * 0.50) / (a.ops_per_s + b.ops_per_s);
        let kept = sim.sel.z_realized.expect("the rate is kept");
        assert!((kept - want).abs() < 0.01, "{kept} against {want}");
    }

    #[test]
    fn a_form_that_took_few_faults_leaves_no_floor_under_the_model() {
        // E3 can fire because the raw form stopped faulting (the cache grew)
        // and not because the compact form does: what it took is then under
        // the model's prediction, and the model's word stands.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Compact, 120.0);
        sim.phi_raw_override = Some(0.05);
        sim.load = cell.fixed(0.05, 0.0, bound);
        sim.run(2.0 * sim.sel.dwell().as_secs_f64());
        sim.first(Event::Exit(Exit::E3));
        assert_eq!(sim.sel.z_realized, Some(0.0));
    }

    #[test]
    fn e2_a_hot_set_that_fits_returns_to_raw_after_a_trial_on_the_refilled_state() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        let install = sim.first(Event::Install);
        let dwell = sim.sel.dwell();
        // The workload narrows to a hot set of a hundred megabytes. The
        // compact form serves it without a fault, and the raw footprint of
        // the pages it touches is a fraction of the budget.
        sim.load = cell.hot(100);
        let Load::Hot {
            raw_pages, z_pages, ..
        } = sim.load
        else {
            unreachable!()
        };
        assert!((cell.bound(z_pages) as f64) < E2_BOUND_SHARE * cell.budget() as f64);
        sim.run(2.0 * dwell.as_secs_f64());
        let trial = sim.first(Event::TrialStart);
        let exit = sim.first(Event::Exit(Exit::E2));
        // Not inside the dwell of the install.
        assert!(trial - install >= dwell);
        // The compact pages displaced the raw ones, so the raw form reads the
        // hot set in again at a fault a page; the verdict came after that,
        // and not on the first intervals of the fill.
        let faults: u64 = sim
            .log
            .iter()
            .filter(|r| r.t > trial && r.t <= exit)
            .map(|r| r.faults)
            .sum();
        assert!(
            faults as f64 >= raw_pages,
            "{faults} faults of {raw_pages} pages"
        );
        assert!(exit - trial >= (TRIAL_LOW_INTERVALS + TRIAL_DECIDE_INTERVALS) * DT);
        assert!(exit - trial < 5 * S1_PERIOD, "{:?}", exit - trial);
        // The form is left: the raw form serves, the form files are kept.
        assert_eq!(
            (sim.last.phase, sim.last.gate),
            (Phase::Suspect, GATE_CENSUS)
        );
        assert!(sim.sel.retained());
        let c = sim.sel.counts();
        assert_eq!((c.trials, c.trials_back, c.exits), (1, 0, [0, 1, 0]));
    }

    #[test]
    fn e2_a_hot_set_the_bound_does_not_admit_never_trials() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.load = cell.hot(600);
        let Load::Hot { z_pages, .. } = sim.load else {
            unreachable!()
        };
        assert!(cell.bound(z_pages) as f64 >= E2_BOUND_SHARE * cell.budget() as f64);
        sim.run(3600.0);
        assert_eq!(
            (sim.last.phase, sim.sel.counts().trials),
            (Phase::Compact, 0)
        );
    }

    #[test]
    fn a_trial_that_finds_the_raw_form_still_faulting_returns_and_doubles_the_backoff() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        // A hot set the bound admits and the raw form cannot hold: it faults
        // at 0.3 a read however long it is given.
        sim.load = Load::Hot {
            raw_pages: 24_000.0,
            z_pages: 13_000.0,
            plateau: 0.3,
        };
        sim.run(14.0 * 3600.0);
        let starts = sim.times(Event::TrialStart);
        let backs = sim.times(Event::TrialBack);
        assert!(starts.len() >= 7, "{} trials", starts.len());
        assert_eq!(
            backs.len() + usize::from(sim.last.phase == Phase::Trial),
            starts.len()
        );
        assert_eq!(sim.count(Event::Exit(Exit::E2)), 0);
        // A wrong trial doubles the back-off from ten minutes as it ends, so
        // the next waited twenty, then forty, and so on to four hours, no
        // more; and the measures were waiting, so each came as soon as it
        // was allowed.
        let mut backoff = Duration::from_secs(10 * 60);
        for (end, next) in backs.iter().zip(starts.iter().skip(1)) {
            backoff = (backoff * 2).min(Duration::from_secs(4 * 60 * 60));
            let gap = *next - *end;
            assert!(gap >= backoff, "{gap:?} after {backoff:?}");
            assert!(gap < backoff + 2 * E2_MEASURE, "{gap:?} after {backoff:?}");
        }
        assert_eq!(sim.sel.backoff(), Duration::from_secs(4 * 60 * 60));
        // And each was judged on the faults it took: the compact form came
        // back after the trial's faults reached the raw pages it expected.
        for (start, back) in starts.iter().zip(&backs) {
            let faults: u64 = sim
                .log
                .iter()
                .filter(|r| r.t > *start && r.t <= *back)
                .map(|r| r.faults)
                .sum();
            assert!(faults > 60_000, "{faults} faults in a trial");
        }
    }

    /// A store in COMPACT with a footprint of `bound` bytes whose compact form
    /// faults nothing, stepped until its trial has started: the next step is
    /// the trial's first, served by the raw form.
    fn trial_started(cell: &Cell, bound: u64) -> Sim {
        trial_from(compact(cell), cell, bound)
    }

    /// As `trial_started`, from a store already in COMPACT.
    fn trial_from(mut sim: Sim, cell: &Cell, bound: u64) -> Sim {
        // Past the dwell of the install, so that only the measures bind.
        sim.run(sim.sel.dwell().as_secs_f64() + 100.0);
        sim.load = cell.fixed(0.0, 0.0, bound);
        sim.run_until(Phase::Trial, 120.0);
        assert_eq!(sim.last.phase, Phase::Trial);
        sim
    }

    /// The raw form's reads for an interval: `ops` of them, `phi` faults each.
    fn raw_interval(ops: f64, phi: f64, bound: u64) -> Load {
        Load::Rated {
            class: Class::Read,
            raw: Rate {
                ops_per_s: ops / DT.as_secs_f64(),
                phi,
            },
            z: Rate {
                ops_per_s: ops / DT.as_secs_f64(),
                phi: 0.0,
            },
            refaults: true,
            bound,
        }
    }

    #[test]
    fn a_trial_is_decided_on_three_intervals_pooled_and_not_on_the_first() {
        // One page of footprint is filled by the wall at the trial's first
        // step, so the three intervals after it are the verdict's. A quiet
        // first and two that fault are a pool that faults; a first that
        // faults on each of its sixty-four reads and two that are quiet are a
        // pool that does not. The counts are the registered ones, written out.
        let cell = Cell::ten_million();
        let bound = PAGE;
        let cases = [
            ([(510.0, 0.0), (5_100.0, 0.45), (5_100.0, 0.45)], true),
            ([(64.0, 1.0), (137_000.0, 0.0), (137_000.0, 0.0)], false),
        ];
        for (intervals, back) in cases {
            let mut sim = trial_started(&cell, bound);
            sim.load = raw_interval(137_000.0, 0.0, bound);
            sim.step();
            assert!(sim.sel.trial.as_ref().is_some_and(|t| t.judged.is_some()));
            for (k, (ops, phi)) in intervals.into_iter().enumerate() {
                assert_eq!(sim.last.phase, Phase::Trial, "{k} of {intervals:?}");
                sim.load = raw_interval(ops, phi, bound);
                sim.step();
            }
            let want = if back {
                Event::TrialBack
            } else {
                Event::Exit(Exit::E2)
            };
            assert_eq!(sim.last.event, Some(want), "{intervals:?}");
        }
    }

    #[test]
    fn a_trial_waits_for_the_fill_unless_the_rate_settles_for_five_intervals_running() {
        // A footprint of two fifths of the budget is eighty-five thousand
        // pages: the faults the fill wants and the wall it is given are both
        // out of reach of a second of intervals. Intervals under the entry
        // threshold judge the fill over only when five run together, and an
        // interval over it starts the count again. (The count is the
        // registered one, written out.)
        let cell = Cell::ten_million();
        let bound = (0.4 * cell.budget() as f64) as u64;
        let judged = |sim: &Sim| sim.sel.trial.as_ref().is_some_and(|t| t.judged.is_some());
        let quiet = raw_interval(137_000.0, 0.0, bound);
        let faulting = raw_interval(5_100.0, 0.45, bound);
        for (run, interrupted) in [(5usize, false), (10, true)] {
            let mut sim = trial_started(&cell, bound);
            for k in 1..=run {
                sim.load = if interrupted && k == 5 {
                    faulting
                } else {
                    quiet
                };
                sim.step();
                let after_the_interruption = k.saturating_sub(5 * usize::from(interrupted));
                assert_eq!(judged(&sim), after_the_interruption >= 5, "step {k}");
            }
            assert!(judged(&sim));
        }
    }

    #[test]
    fn a_trial_waits_three_times_the_fill_for_the_wall_and_no_more_than_thirty_seconds() {
        // Intervals of a thousand reads that fault on 45% of them are neither
        // quiet nor enough to fill the footprint, so the wall says when the
        // fill is over: three times what the expected pages take to fault in
        // at a fault apiece, capped. A cache of eight gibibytes admits a
        // footprint whose pages take longer than the cap. (Both numbers are
        // the registered ones, written out.)
        let small = Cell::ten_million();
        let big = Cell {
            cache: 8 << 30,
            margin: 0,
            ..Cell::ten_million()
        };
        let cases = [
            (small.clone(), (0.45 * small.budget() as f64) as u64, 119),
            (big, 3 << 30, 300),
        ];
        for (cell, bound, steps) in cases {
            let pages = (bound / PAGE) as f64;
            let wall = (3.0 * pages * cell.t_f / f64::from(cell.threads)).min(30.0);
            assert_eq!((wall * 10.0).ceil() as u64, steps);
            // (A footprint as big as the budget keeps the trial from starting
            // before the test wants it.)
            let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 0.0, cell.budget()));
            sim.phi_raw_override = Some(0.45);
            sim.run_until(Phase::Compact, 120.0);
            assert_eq!(sim.last.phase, Phase::Compact);
            let mut sim = trial_from(sim, &cell, bound);
            sim.load = raw_interval(1_000.0, 0.45, bound);
            let mut judged_at = None;
            for k in 1..=steps + 10 {
                sim.step();
                let judged = sim.sel.trial.as_ref().is_some_and(|t| t.judged.is_some());
                if judged && judged_at.is_none() {
                    judged_at = Some(k);
                }
            }
            assert_eq!(judged_at, Some(steps), "{bound} bytes of footprint");
        }
    }

    #[test]
    fn a_trial_is_judged_when_its_faults_reach_one_and_a_fifth_of_its_pages() {
        // Intervals that fault on 45% of their reads are neither quiet nor
        // long enough to run out the wall, so the fill is over when the
        // faults taken reach what the footprint expects. (The factor is the
        // registered one, written out.)
        let cell = Cell::ten_million();
        let bound = (0.4 * cell.budget() as f64) as u64;
        let pages = (bound / PAGE) as f64;
        let per_interval = 5_100.0 * 0.45;
        let want = (1.2 * pages / per_interval).ceil() as u64;
        let mut sim = trial_started(&cell, bound);
        sim.load = raw_interval(5_100.0, 0.45, bound);
        let mut judged_at = None;
        for k in 1..=2 * want {
            sim.step();
            let judged = sim.sel.trial.as_ref().is_some_and(|t| t.judged.is_some());
            if judged && judged_at.is_none() {
                judged_at = Some(k);
            }
        }
        assert_eq!(judged_at, Some(want));
    }

    #[test]
    fn a_trial_that_cannot_be_decided_is_undone_without_blaming_the_form() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.load = cell.hot(100);
        let dwell = sim.sel.dwell();
        sim.run_until(Phase::Trial, 2.0 * dwell.as_secs_f64());
        assert_eq!(sim.last.phase, Phase::Trial);
        // Nothing reads: no interval is evidence, so no verdict. The fill is
        // judged over by the wall it was given, which is three times what its
        // expected pages take to fault in; the dwell is counted from there.
        sim.load = Load::Idle;
        let trial = sim.first(Event::TrialStart);
        sim.run(dwell.as_secs_f64() + 60.0);
        let back = sim.first(Event::TrialBack);
        let Load::Hot { z_pages, .. } = cell.hot(100) else {
            unreachable!()
        };
        let pages = cell.bound(z_pages) / PAGE;
        let wall = TRIAL_WALL_FACTOR * pages as f64 * cell.t_f;
        assert!(wall < TRIAL_WALL_CAP.as_secs_f64());
        near(back - trial, wall + dwell.as_secs_f64(), 0.5);
        assert_eq!((sim.last.phase, sim.last.gate), (Phase::Compact, GATE_Z));
        assert_eq!(
            sim.sel.backoff(),
            TRIAL_BACKOFF_MIN,
            "an idle store is nobody's mistake"
        );
    }

    // ---- other workloads and other signals -------------------------------------------

    #[test]
    fn a_scan_only_workload_reaches_evidence() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.scans());
        sim.run(120.0);
        // The scans alone pay the price: no read window ever.
        let star = (cell.t_z_scan - cell.t_hit_scan) / cell.t_f;
        let scans = 1.0 / (cell.t_hit_scan + cell.scan_raw * cell.t_f);
        let rent = (cell.scan_raw - star) * cell.t_f * scans;
        let start = sim.first(Event::BuildStart);
        near(start, 3.0 + cell.price / rent, 0.3);
        assert_eq!(sim.last.phase, Phase::Compact);
        // E3 reads point reads and E2 finds the footprint too big: it stays.
        sim.run(3600.0);
        assert_eq!(
            (sim.last.phase, sim.sel.counts().switches),
            (Phase::Compact, 1)
        );
    }

    /// Both classes of a uniform store: the reads at the geometry's rates and
    /// scans of `scan` operations a second that fault `phi` a scan in either
    /// form.
    fn reads_and_scans(cell: &Cell, ops_per_s: f64, phi: f64, bound: u64) -> Load {
        let scan = Rate { ops_per_s, phi };
        Load::Both {
            read: (
                cell.rate(cell.t_hit, cell.truth_raw()),
                cell.rate(cell.t_z, cell.truth_z()),
            ),
            scan: (scan, scan),
            refaults: true,
            bound,
        }
    }

    #[test]
    fn a_class_the_form_loses_on_pays_the_rent_down() {
        // The rent is the sum over the classes of what the form saves net of
        // what it costs. Scans that fault a twentieth of a fault each save
        // nothing and pay the form's extra CPU, which comes off the reads'.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), reads_and_scans(&cell, 20_000.0, 0.05, bound));
        sim.run(60.0);
        let star_scan = (cell.t_z_scan - cell.t_hit_scan) / cell.t_f;
        let loss = (0.05 - star_scan) * cell.t_f * 20_000.0;
        assert!(loss < -0.1, "{loss}");
        let start = sim.first(Event::BuildStart);
        near(start, 3.0 + cell.price / (rent_rate(&cell) + loss), 0.3);
        assert!(start.as_secs_f64() > 3.0 + cell.price / rent_rate(&cell) + 1.0);
    }

    #[test]
    fn a_class_that_pays_keeps_the_build_going_whatever_the_other_does() {
        // Reads that fault at the raw rate and scans that have nothing to
        // save: a build is cancelled only when the saving has vanished in
        // every class that has evidence, and the reads' has not.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), reads_and_scans(&cell, 2_000.0, 0.05, bound));
        sim.run(90.0);
        assert_eq!(sim.count(Event::BuildStart), 1);
        assert_eq!(sim.count(Event::BuildCancelled), 0);
        assert_eq!(sim.count(Event::Install), 1);
    }

    #[test]
    fn e2_wants_every_class_with_data_quiet() {
        // The reads are quiet through the compact form and the footprint is
        // small. Scans that still fault through it say the raw form is not
        // shown to fit.
        let cell = Cell::ten_million();
        let small = cell.budget() / 4;
        let reads = (cell.rate(cell.t_hit, 0.45), cell.rate(cell.t_z, 0.0));
        for (scan_z, trials) in [(0.5, 0), (0.0, 1)] {
            let mut sim = compact(&cell);
            sim.run(sim.sel.dwell().as_secs_f64() + 100.0);
            sim.load = Load::Both {
                read: reads,
                scan: (
                    Rate {
                        ops_per_s: 2_000.0,
                        phi: 0.5,
                    },
                    Rate {
                        ops_per_s: 2_000.0,
                        phi: scan_z,
                    },
                ),
                refaults: true,
                bound: small,
            };
            sim.run(120.0);
            assert_eq!(sim.count(Event::TrialStart), trials, "{scan_z}");
        }
    }

    #[test]
    fn a_trial_is_wrong_if_any_class_still_faults() {
        // The reads are quiet through the raw form; the scans are not, or
        // are. The verdict is for the form if either class pays for it.
        let cell = Cell::ten_million();
        let bound = PAGE;
        let quiet = Rate {
            ops_per_s: 1_370_000.0,
            phi: 0.0,
        };
        for (scan_phi, back) in [(0.6, true), (0.02, false)] {
            let mut sim = trial_started(&cell, bound);
            let scan = Rate {
                ops_per_s: 2_000.0,
                phi: scan_phi,
            };
            sim.load = Load::Both {
                read: (quiet, quiet),
                scan: (scan, scan),
                refaults: true,
                bound,
            };
            // The fill, judged by the wall at once, and three intervals.
            for _ in 0..4 {
                sim.step();
            }
            let want = if back {
                Event::TrialBack
            } else {
                Event::Exit(Exit::E2)
            };
            assert_eq!(sim.last.event, Some(want), "{scan_phi}");
        }
    }

    #[test]
    fn commits_in_every_window_still_reach_evidence() {
        // The mixes that commit every hundred operations have a commit in
        // every 100 ms window. A commit costs a sync and a write and no fault,
        // so the window counts for the rate; it cannot say what a fault costs,
        // and that is the sampler's cold touches until a window without a
        // commit appears.
        let cell = Cell::ten_million();
        let clean = {
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            sim.run(60.0);
            sim.first(Event::BuildStart)
        };
        // One window in ten without: the census measures it from the first.
        let mut b = Sim::new(cell.clone(), cell.uniform());
        b.commit_free_every = 10;
        b.run(60.0);
        assert_eq!(b.first(Event::BuildStart), clean);
        // None without: the sampler's reading prices the saving and the rent.
        let mut af = Sim::new(cell.clone(), cell.uniform());
        af.commit_free_every = 0;
        af.sampler_t_f = 60e-6;
        af.run(60.0);
        let star = (cell.t_z - cell.t_hit) / af.sampler_t_f;
        let phi = cell.truth_raw();
        let ops = 1.0 / (cell.t_hit + phi * cell.t_f);
        let rent = (phi - star) * af.sampler_t_f * ops;
        near(af.first(Event::BuildStart), 3.0 + cell.price / rent, 0.3);
        // A window without a commit, once seen, is kept. (The census is
        // read where the handles run it, and that is not FITS.)
        let mut s = ZSelect::new();
        assert_eq!(drive(&mut s, 2, |_| (0.5, 0.0)).len(), 1);
        assert_eq!(s.phase(), Phase::Suspect);
        let with = |t: u64, t_f: Option<f64>| Input {
            now: Duration::from_secs(t),
            census: Census {
                costs: Costs {
                    t_f,
                    ..Costs::default()
                },
                ..Census::default()
            },
            ..Input::default()
        };
        s.step(&with(3, Some(50e-6)));
        s.step(&with(4, None));
        assert_eq!(
            s.saving_star(Class::Read),
            (PRIOR_T_Z - PRIOR_T_HIT) / 50e-6
        );
    }

    #[test]
    fn the_banked_rungs_pass_means_build_and_the_in_core_control_does_not() {
        // The passes of the banked rung, each replayed at its mean rate: the
        // log holds one rate and one fault count a pass and no interval at
        // all, so the windows are the means. At ten
        // million keys under a gibibyte the point read pass of the banked rep
        // ran 53,642 a second and took 0.4625 faults a read, every one a
        // refault, and the scan pass 7,634 scans a second at 3.04 faults a
        // scan (`docs/engine.md`, "Out of core", rounds them to 54 thousand,
        // 0.46, 0.76 million entries and 3.0). The compact form's rates are
        // the sampler's. At three million keys the same arm read 1,367,050 a
        // second and took none.
        let recorded = |class, ops_per_s, phi| Load::Rated {
            class,
            raw: Rate { ops_per_s, phi },
            z: Rate {
                ops_per_s: 20.0 * ops_per_s,
                phi: 0.0,
            },
            refaults: true,
            bound: 1_400_000_000,
        };
        let ten = Cell::ten_million();
        for load in [
            recorded(Class::Read, 53_642.0, 0.4625),
            recorded(Class::Scan, 7_634.0, 3.0447),
        ] {
            let mut sim = Sim::new(ten.clone(), load);
            sim.run(180.0);
            let c = sim.sel.counts();
            assert_eq!((c.builds, c.installs), (1, 1), "{load:?}");
            assert_eq!(sim.last.phase, Phase::Compact);
        }
        let three = Cell::three_million();
        let mut sim = Sim::new(three, recorded(Class::Read, 1_367_050.0, 0.0));
        sim.garbage = true;
        sim.run(180.0);
        assert!(sim.events().is_empty());
        assert_eq!(sim.last.phase, Phase::Fits);
    }

    #[test]
    fn a_neighbours_pressure_opens_the_build_clause() {
        // The cache is large and the model, which prices this store against
        // all of it, says it fits. Another reader of the group is taking part
        // of it, and the store's raw reads fault at 0.3 where the compact form
        // takes none. The pressure term enters SUSPECT and opens the build
        // clause; the census, not the pressure, decides to buy.
        let big = Cell {
            cache: 4 << 30,
            neighbours: 400.0,
            ..Cell::ten_million()
        };
        let bound = big.bound(big.z as f64 / PAGE as f64);
        let mut sim = Sim::new(big.clone(), big.fixed(0.3, 0.0, bound));
        sim.run(120.0);
        assert!(sim.sel.theta_sus(Class::Read) > model_raw(big.budget(), big.hash, big.records));
        assert_eq!(sim.first(Event::Suspect), 2 * S1_PERIOD);
        let c = sim.sel.counts();
        assert_eq!((c.builds, c.installs), (1, 1));
        // The model said the store fits and the census saw the raw form
        // fault: the model's error is kept for as long as the form stands, so
        // its word that the store fits is no reason to leave it, and the form
        // is judged against what the census saw and keeps paying.
        assert!((sim.sel.bias - 0.3).abs() < 0.01, "{}", sim.sel.bias);
        sim.run(5.0 * sim.sel.dwell().as_secs_f64());
        assert_eq!(
            (sim.last.phase, sim.sel.counts().switches),
            (Phase::Compact, 1)
        );
        // The compact form is quiet and what it touches is small, so the raw
        // form is tried, and found faulting with the neighbour there.
        let c = sim.sel.counts();
        assert_eq!((c.trials, c.trials_back), (1, 1));
        // The neighbour goes and the raw form would fault at nothing. The
        // model is no more to be believed than it was, and the form is left
        // by the next trial, which asks the raw form itself.
        sim.cell.neighbours = 0.0;
        sim.load = big.fixed(0.0, 0.0, bound);
        sim.run(600.0);
        let c = sim.sel.counts();
        assert_eq!((c.trials, c.exits), (2, [0, 1, 0]));
        // The model's error went with the form.
        assert_eq!(sim.sel.bias, 0.0);
        sim.run(60.0);
        assert_eq!(sim.last.phase, Phase::Fits);
        // A store whose faults are first touches, which no refault is counted
        // for, raises no pressure of its own; without a neighbour it never
        // leaves FITS, whatever the census it is handed says.
        let alone = Cell {
            neighbours: 0.0,
            ..big
        };
        let first_touches = Load::Rated {
            class: Class::Read,
            raw: alone.rate(alone.t_hit, 0.3),
            z: alone.rate(alone.t_z, 0.0),
            refaults: false,
            bound,
        };
        let mut sim = Sim::new(alone.clone(), first_touches);
        sim.garbage = true;
        sim.run(600.0);
        assert!(sim.events().is_empty());
    }

    // ---- the window's edges ---------------------------------------------------------

    #[test]
    fn a_ramp_through_the_windows_edge_enters_once_and_leaves_once() {
        // The cache the store is given falls from twice the store to
        // two-thirds of its records over ten minutes, and rises back over
        // ten, as another process grows and shrinks beside it; then it stays.
        // The model crosses every threshold on the way down and on the way up,
        // slowly, and the rung follows it once each way.
        let cell = Cell::ten_million();
        let (hi, lo) = (2 * (cell.hash + cell.records), 2 * cell.records / 3);
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_with(2400.0, |s| {
            let t = s.now.as_secs_f64();
            let down = if t < 600.0 {
                t / 600.0
            } else if t < 1200.0 {
                2.0 - t / 600.0
            } else {
                0.0
            };
            s.cell.cache = hi - ((hi - lo) as f64 * down) as u64;
            s.load = s.cell.uniform();
        });
        let kinds: Vec<Event> = sim.events().into_iter().map(|(_, e)| e).collect();
        assert_eq!(kinds.len(), 5, "{kinds:?}");
        assert_eq!(kinds[0], Event::Suspect);
        assert_eq!(kinds[1], Event::BuildStart);
        assert_eq!(kinds[2], Event::Install);
        assert!(
            matches!(kinds[3], Event::Exit(Exit::E1 | Exit::E3)),
            "{kinds:?}"
        );
        assert_eq!(kinds[4], Event::Relax);
        assert_eq!(sim.last.phase, Phase::Fits);
        // Entered after the model crossed its threshold on the way down, not
        // before it and not once the ramp had bottomed out.
        let suspect = sim.first(Event::Suspect).as_secs_f64();
        assert!(suspect > 100.0 && suspect < 600.0, "{suspect}");
    }

    #[test]
    fn an_anon_swing_with_the_cache_near_the_records_does_not_flap() {
        // The process's anonymous memory swings by 400 MB, so the cache the
        // store is given swings by 200 MB either side of the cell's, every
        // forty seconds, for an hour. The model moves by more than 0.3 with
        // it. The form is not left on any of its readings, because none of
        // them lasts a dwell.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        let swing = |s: &mut Sim| {
            let high = (s.now.as_secs_f64() / 20.0) as u64 % 2 == 1;
            s.cell.cache = if high { 1_137_000_000 } else { 737_000_000 };
            s.load = s.cell.uniform();
        };
        sim.run_with(3600.0, swing);
        let (lo, hi) = (
            model_raw(1_137_000_000 - cell.margin, cell.hash, cell.records),
            model_raw(737_000_000 - cell.margin, cell.hash, cell.records),
        );
        assert!(hi - lo > 0.3, "{lo} {hi}");
        assert_eq!(sim.last.phase, Phase::Compact);
        assert_eq!(sim.sel.counts().switches, 1);
        assert_eq!(sim.sel.counts().exits, [0, 0, 0]);
    }

    #[test]
    fn a_store_fifty_times_the_cache_still_builds() {
        // Records fifty times the cache. The raw form takes nearly two faults
        // a read, because the hash misses too, and the compact form nearly
        // one: the saving between them is as large as it is at the cell, and
        // the form is bought. A model with one population saturates at a
        // fault a read and would see no saving at all.
        let base = Cell::ten_million();
        let records = 50.0 * base.cache as f64;
        let cell = Cell {
            hash: (records * 19.0 / 130.0) as u64,
            records: records as u64,
            z: (records * 71.9 / 130.0) as u64,
            resident: (records * 1.74 / 130.0) as u64,
            ..base
        };
        let raw = model_raw(cell.budget(), cell.hash, cell.records);
        let z = model_z(cell.budget(), cell.resident, cell.z);
        let enter = ZSelect::new().theta_enter(Class::Read);
        assert!(raw > 1.9 && z > 0.98, "{raw} {z}");
        assert!(
            raw - z > 6.0 * enter,
            "the saving does not fall as the store grows"
        );
        assert!(raw.min(1.0) - z < enter, "a one-population model sees none");
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run(3600.0);
        let c = sim.sel.counts();
        assert_eq!((c.builds, c.installs, c.switches), (1, 1, 1));
        assert_eq!(sim.last.phase, Phase::Compact);
        // The rent is the saving between the two forms, not the raw form's
        // faults: the compact form takes nearly a fault a read here.
        near(
            sim.first(Event::BuildStart),
            3.0 + cell.price / rent_rate(&cell),
            0.5,
        );
        // Where the platform counts no refaults there is no pressure term,
        // and the model's word is what the clause has beside the census: the
        // one-population model, handed the same store and the same census,
        // never builds.
        let mut one = Sim::new(cell.clone(), cell.uniform());
        one.counter = false;
        one.phi_raw_override = Some(raw.min(1.0));
        one.run(3600.0);
        assert_eq!(one.sel.counts().builds, 0);
        assert_eq!(one.last.phase, Phase::Suspect);
        assert!(
            one.log.iter().any(|r| r.excess >= cell.price),
            "the rent was paid"
        );
        // Where it counts them, the store's own refaults are in the group's
        // counter and over the pressure term, which stands for a model that
        // cannot see: the measured saving decides, and the one-population
        // model buys as the other does.
        let mut one = Sim::new(cell.clone(), cell.uniform());
        one.phi_raw_override = Some(raw.min(1.0));
        one.run(3600.0);
        assert_eq!(one.sel.counts().builds, 1);
        assert_eq!(one.last.phase, Phase::Compact);
    }

    // ---- builds that waste -----------------------------------------------------------

    #[test]
    fn a_build_whose_evidence_vanishes_is_cancelled_and_counts_as_a_switch() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Building, 60.0);
        let start = sim.first(Event::BuildStart);
        // The form is bought on rent at least the price, so if the workload
        // stops the moment the build starts the loss is the build, which is
        // no more than the rent already paid.
        assert!(sim.rec_at(start).excess >= cell.price);
        sim.run(12.0);
        sim.load = cell.hot(100);
        let changed = sim.now;
        sim.run(5.0);
        let cancel = sim.first(Event::BuildCancelled);
        assert!(cancel - changed >= VANISH_INTERVALS * DT);
        assert!(cancel - changed <= (VANISH_INTERVALS + 1) * DT);
        assert_eq!(sim.last.phase, Phase::Suspect);
        // It is a switch for the dwell, which is ten of the wall the build
        // had when it was cancelled; the back-off before a trial has doubled;
        // the form files it made are kept.
        let r = *sim.rec_at(cancel);
        near(r.dwell, 10.0 * (cancel - start).as_secs_f64(), 0.01);
        assert!(r.dwell > DWELL_FLOOR);
        assert_eq!(r.backoff, 2 * TRIAL_BACKOFF_MIN);
        assert!(sim.sel.retained());
        let c = sim.sel.counts();
        assert_eq!((c.build_cancelled, c.switches), (1, 1));
        // The rent is paid again at once when the reads are uniform again,
        // and the build waits out the dwell all the same.
        sim.load = cell.uniform();
        sim.run(250.0);
        let again = sim.times(Event::BuildStart)[1];
        near(again - cancel, r.dwell.as_secs_f64(), 0.2);
    }

    #[test]
    fn a_model_that_says_it_fits_ends_the_episode_and_its_back_off() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Building, 60.0);
        sim.run(5.0);
        sim.load = cell.hot(100);
        sim.run_until(Phase::Suspect, 5.0);
        assert_eq!(sim.sel.backoff(), 2 * TRIAL_BACKOFF_MIN);
        // The store fits: the episode is over, and the next is a fresh one,
        // with no memory of a wrong guess about a skewed workload.
        sim.phi_raw_override = Some(0.0);
        sim.run_until(Phase::Fits, 120.0);
        assert_eq!(sim.last.phase, Phase::Fits);
        assert_eq!(sim.sel.backoff(), TRIAL_BACKOFF_MIN);
    }

    #[test]
    fn a_build_is_cancelled_below_half_the_entry_threshold_and_not_above_it() {
        // The saving falls to the break-even: under the entry threshold,
        // which started the build, and over half of it, which cancels. The
        // build goes on. Below half of it, it does not.
        let cell = Cell::ten_million();
        let star = (cell.t_z - cell.t_hit) / cell.t_f;
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        for (raw, cancelled) in [(1.0 * star, false), (0.6 * star, true)] {
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            sim.run_until(Phase::Building, 60.0);
            sim.load = cell.fixed(raw, 0.0, bound);
            sim.run(60.0);
            assert_eq!(
                sim.count(Event::BuildCancelled),
                usize::from(cancelled),
                "{raw}"
            );
            assert_eq!(sim.count(Event::Install), usize::from(!cancelled), "{raw}");
        }
    }

    #[test]
    fn a_build_is_cancelled_on_the_third_interval_under_half_and_not_the_second() {
        // Evidence that dips for two intervals and comes back is the noise of
        // a window of a tenth of a second; three running is a workload that
        // has moved. (The counts are the registered ones, written out.)
        let cell = Cell::ten_million();
        let star = (cell.t_z - cell.t_hit) / cell.t_f;
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let low = cell.fixed(0.2 * star, 0.0, bound);
        let high = cell.uniform();
        for (dip, cancelled) in [(2u32, false), (3, true)] {
            let mut sim = Sim::new(cell.clone(), high);
            sim.run_until(Phase::Building, 60.0);
            let mut k = 0u32;
            sim.run_with(20.0, |s| {
                s.load = if k % (dip + 1) < dip { low } else { high };
                k += 1;
            });
            assert_eq!(
                sim.count(Event::BuildCancelled),
                usize::from(cancelled),
                "{dip} intervals running"
            );
        }
    }

    #[test]
    fn intervals_without_evidence_do_not_break_the_run_of_vanished_ones() {
        // The run counts evidence intervals, and an interval in which nothing
        // reads is not one: low, silent, low, silent, low is three running.
        let cell = Cell::ten_million();
        let star = (cell.t_z - cell.t_hit) / cell.t_f;
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let low = cell.fixed(0.2 * star, 0.0, bound);
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Building, 60.0);
        let mut k = 0u32;
        sim.run_with(2.0, |s| {
            s.load = if k.is_multiple_of(2) { low } else { Load::Idle };
            k += 1;
        });
        assert_eq!(sim.count(Event::BuildCancelled), 1);
    }

    #[test]
    fn the_form_is_bought_at_half_again_the_break_even_and_not_before() {
        // A saving 20% over the break-even repays the form's CPU and pays a
        // rent that grows without end, and is not bought: the margin covers
        // the model's error. One 60% over is.
        let cell = Cell::ten_million();
        let star = (cell.t_z - cell.t_hit) / cell.t_f;
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        for (raw, buys) in [(1.2 * star, false), (1.6 * star, true)] {
            let mut sim = Sim::new(cell.clone(), cell.fixed(raw, 0.0, bound));
            sim.run(3600.0);
            assert_eq!(sim.sel.counts().builds, usize::from(buys) as u64, "{raw}");
            if !buys {
                assert!(
                    sim.log.iter().any(|r| r.excess >= cell.price),
                    "the rent was paid"
                );
            }
        }
    }

    #[test]
    fn an_interval_under_sixty_four_operations_is_not_evidence() {
        let cell = Cell::ten_million();
        let slow = |ops_per_s| Load::Rated {
            class: Class::Read,
            raw: Rate {
                ops_per_s,
                phi: 1.0,
            },
            z: Rate {
                ops_per_s: 4.0 * ops_per_s,
                phi: 0.0,
            },
            refaults: true,
            bound: 1_400_000_000,
        };
        // Sixty-three reads an interval, every one a fault: nothing to go on.
        let mut sim = Sim::new(cell.clone(), slow(630.0));
        sim.run(1800.0);
        assert!(sim.log.iter().all(|r| r.excess == 0.0));
        assert_eq!(sim.sel.counts().builds, 0);
        // Sixty-four is.
        let mut sim = Sim::new(cell.clone(), slow(640.0));
        sim.run(1800.0);
        assert_eq!(sim.sel.counts().builds, 1);
    }

    #[test]
    fn e3_acts_only_on_a_form_seen_not_to_pay_throughout() {
        // A second in which nothing reads, part way through the dwell, breaks
        // the stretch: the exit is for a form that was watched the whole time.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let load = cell.fixed(0.45, 0.40, bound);
        let mut sim = Sim::new(cell.clone(), load);
        sim.run_until(Phase::Compact, 120.0);
        let dwell = sim.sel.dwell();
        sim.run(100.0);
        sim.load = Load::Idle;
        sim.run(2.0);
        sim.load = load;
        let resumed = sim.now;
        sim.run(2.0 * dwell.as_secs_f64());
        let exit = sim.first(Event::Exit(Exit::E3));
        assert!(exit - resumed >= dwell, "{:?}", exit - resumed);
        assert!(exit - resumed <= dwell + 3 * S1_PERIOD);
    }

    #[test]
    fn an_idle_store_is_not_a_yes_for_e2() {
        // The compact form faults nothing because nothing reads it. That
        // says nothing about whether the raw form would fit what is read.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.run(sim.sel.dwell().as_secs_f64() + 100.0);
        sim.load = cell.hot(100);
        while sim.sel.watch.e2_run < E2_MEASURES - 1 {
            sim.step();
            assert!(sim.now < Duration::from_secs(10_000));
        }
        sim.load = Load::Idle;
        sim.run(3600.0);
        assert_eq!(sim.sel.counts().trials, 0);
        assert_eq!(sim.sel.watch.e2_run, 0);
    }

    #[test]
    fn e2_wants_three_measures_running_before_it_tries() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        // Past the dwell of the install, so that only the measures bind.
        sim.run(sim.sel.dwell().as_secs_f64() + 100.0);
        let narrowed = sim.now;
        sim.load = cell.hot(100);
        sim.run(120.0);
        let waited = (sim.first(Event::TrialStart) - narrowed).as_secs_f64();
        // Three measures of ten seconds, written out.
        assert!(waited > 20.0, "{waited}");
        assert!(waited <= 30.2, "{waited}");
    }

    #[test]
    fn e2_admits_a_footprint_under_half_the_budget_and_not_over_it() {
        // Hot sets whose raw footprint is 45% and 55% of the budget. (The
        // half is the registered one, written out.)
        let cell = Cell::ten_million();
        for (mb, share, trials) in [(215, 0.45, true), (308, 0.55, false)] {
            let mut sim = compact(&cell);
            sim.load = cell.hot(mb);
            let Load::Hot { z_pages, .. } = sim.load else {
                unreachable!()
            };
            let got = cell.bound(z_pages) as f64 / cell.budget() as f64;
            assert!((got - share).abs() < 0.02, "{mb} MB is {got} of the budget");
            sim.run(2.0 * sim.sel.dwell().as_secs_f64());
            assert_eq!(sim.count(Event::TrialStart) > 0, trials, "{mb} MB");
        }
    }

    #[test]
    fn e2_wants_the_compact_form_quiet_as_well_as_the_footprint_small() {
        // The footprint is what the reads touched, and says nothing of what
        // was resident. A compact form that faults at the threshold or over
        // has not shown that the raw form would hold what is read, however
        // few pages that is; under the threshold it has, and the trial
        // follows.
        let cell = Cell::ten_million();
        let star = (cell.t_z - cell.t_hit) / cell.t_f;
        let small = cell.budget() / 4;
        assert!((small as f64) < E2_BOUND_SHARE * cell.budget() as f64);
        for (z, trials) in [
            (1.1 * SUSPECT_FACTOR * star, 0),
            (0.9 * SUSPECT_FACTOR * star, 1),
        ] {
            let mut sim = compact(&cell);
            // Past the dwell of the install, so that only the measures bind.
            sim.run(sim.sel.dwell().as_secs_f64() + 100.0);
            sim.load = cell.fixed(0.45, z, small);
            sim.run(120.0);
            assert_eq!(sim.count(Event::TrialStart), trials, "{z}");
            if trials == 0 {
                assert_eq!(sim.sel.watch.e2_run, 0, "{z}");
                assert_eq!(sim.last.phase, Phase::Compact);
            }
        }
    }

    #[test]
    fn a_right_trial_still_starts_the_back_off() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.load = cell.hot(100);
        sim.run_until(Phase::Suspect, 1200.0);
        let exit = sim.first(Event::Exit(Exit::E2));
        // The workload widens again and the form is bought again, from the
        // form files that were kept; then it narrows once more.
        sim.load = cell.uniform();
        sim.run_until(Phase::Compact, 900.0);
        assert_eq!(sim.last.phase, Phase::Compact);
        sim.load = cell.hot(100);
        sim.run(1500.0);
        let trials = sim.times(Event::TrialStart);
        assert!(trials.len() >= 2, "{trials:?}");
        // The dwell would let it try at once; the back-off does not.
        assert!(
            trials[1] - exit >= TRIAL_BACKOFF_MIN,
            "{:?}",
            trials[1] - exit
        );
        assert_eq!(
            sim.sel.backoff(),
            TRIAL_BACKOFF_MIN,
            "a right trial does not double it"
        );
    }

    #[test]
    fn a_reading_that_is_not_a_number_is_ignored() {
        let mut sel = ZSelect::new();
        for t in 1..=20u64 {
            let d = sel.step(&Input {
                now: Duration::from_secs(t),
                model: Some(Model {
                    phi_raw: if t % 2 == 0 { f64::NAN } else { f64::INFINITY },
                    phi_z: None,
                    phi_z_scan: None,
                    refault_rate: 0.0,
                    budget: 1 << 30,
                }),
                ..Input::default()
            });
            assert_eq!(d.phase, Phase::Fits);
        }
        let c = sel.counts();
        assert_eq!((c.ignored, c.s1_ticks), (20, 0));
    }

    fn building(cell: &Cell) -> Sim {
        // A build that does not finish by itself, so that what the builder
        // reports is what the test says.
        let slow = Cell {
            build_wall: 1e9,
            ..cell.clone()
        };
        let mut sim = Sim::new(slow.clone(), slow.uniform());
        sim.run_until(Phase::Building, 60.0);
        assert_eq!(sim.last.phase, Phase::Building);
        sim
    }

    #[test]
    fn building_is_suspended_when_more_than_four_of_the_last_eight_builds_are_superseded() {
        let mut sim = building(&Cell::ten_million());
        let start = sim.first(Event::BuildStart);
        // Four installed, then four superseded: four of the last eight, the
        // most the budget allows.
        sim.collected.installed = 4;
        sim.step();
        sim.collected.superseded = 4;
        sim.step();
        assert_eq!(sim.last.phase, Phase::Building);
        assert_eq!(sim.sel.counts().build_suspended, 0);
        // The fifth is more than four of eight: the build is abandoned as a
        // switch, and building is held off for a dwell.
        sim.collected.superseded = 1;
        sim.step();
        let abandoned = sim.first(Event::BuildAbandoned);
        assert_eq!(sim.last.phase, Phase::Suspect);
        assert!(sim.sel.suspended(sim.now));
        let c = sim.sel.counts();
        assert_eq!(
            (c.build_suspended, c.build_cancelled, c.switches),
            (1, 1, 1)
        );
        // The reads are still uniform: the rent is paid in full, and the
        // dwell and the suspension are both a dwell long.
        sim.run(120.0);
        let resumed = sim.times(Event::BuildStart)[1];
        assert!(sim
            .log
            .iter()
            .any(|r| r.excess >= Cell::ten_million().price && r.t < abandoned + DWELL_FLOOR));
        near(resumed - abandoned, DWELL_FLOOR.as_secs_f64(), 0.2);
        assert!(resumed - start > DWELL_FLOOR);
        // Builds that install while the window still remembers the
        // supersessions do not suspend it again.
        sim.collected.installed = 3;
        sim.step();
        assert_eq!(sim.sel.counts().build_suspended, 1);
        // A superseded one does.
        sim.collected.superseded = 1;
        sim.step();
        assert_eq!(sim.sel.counts().build_suspended, 2);
        // And a run of builds that installed ages the supersessions out of
        // the window: one superseded after four installed is two of eight.
        sim.collected.installed = 4;
        sim.step();
        sim.collected.superseded = 1;
        sim.step();
        assert_eq!(sim.sel.counts().build_suspended, 2);
    }

    #[test]
    fn the_budget_remembers_eight_builds_and_not_six_or_ten() {
        // The window's width, written out. Four superseded, four that
        // installed and one more superseded is four of the last eight, the
        // first having gone; ten would still hold it and be five. Two
        // superseded, three that installed and three more is five of the
        // last eight, though a window of six would see three.
        let cell = Cell::ten_million();
        let cases: [(&[(u32, u32)], bool); 2] = [
            (&[(0, 4), (4, 0), (0, 1)], false),
            (&[(0, 2), (3, 0), (0, 3)], true),
        ];
        for (history, trips) in cases {
            let mut sim = building(&cell);
            for &(installed, superseded) in history {
                sim.collected.installed = installed;
                sim.collected.superseded = superseded;
                sim.step();
            }
            assert_eq!(
                sim.sel.counts().build_suspended,
                u64::from(trips),
                "{history:?}"
            );
        }
    }

    #[test]
    fn a_build_collected_late_still_counts_against_the_budget() {
        // A merge retired the targets of five builds in flight, and their
        // collection came after the build they belonged to was cancelled:
        // the selector is SUSPECT by then, and they were wasted all the same.
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Building, 60.0);
        sim.run(12.0);
        sim.load = cell.hot(100);
        sim.run_until(Phase::Suspect, 5.0);
        assert_eq!(sim.last.phase, Phase::Suspect);
        let cancelled = sim.now;
        sim.load = cell.uniform();
        sim.collected.superseded = 5;
        sim.step();
        assert_eq!(sim.last.event, None, "nothing to cancel");
        assert_eq!(sim.last.phase, Phase::Suspect);
        assert!(sim.sel.suspended(sim.now));
        assert_eq!(sim.sel.counts().build_suspended, 1);
        // The dwell the cancel left is the one the suspension lasts.
        sim.run(300.0);
        let again = sim.times(Event::BuildStart)[1];
        assert!(again - cancelled >= sim.rec_at(cancelled).dwell);
    }

    #[test]
    fn the_budget_holds_off_a_build_the_dwell_alone_would_allow() {
        // No switch has happened, so the dwell is no obstacle, and the rent
        // is paid by thirteen seconds. Five superseded builds are in the
        // window all the same, left from an earlier life of the store, and it
        // is the suspension that holds the build off for a dwell.
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run(4.0);
        assert_eq!(sim.last.phase, Phase::Suspect);
        sim.collected.superseded = 5;
        sim.step();
        let tripped = sim.now;
        assert!(sim.sel.suspended(sim.now));
        sim.run(120.0);
        let start = sim.first(Event::BuildStart);
        near(start - tripped, DWELL_FLOOR.as_secs_f64(), 0.3);
        assert!(sim
            .log
            .iter()
            .any(|r| r.t < start && r.excess >= cell.price));
    }

    #[test]
    fn an_install_that_comes_with_a_trip_still_holds_off_the_next_building() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run_until(Phase::Building, 60.0);
        sim.built = 0.95;
        sim.collected.superseded = 5;
        sim.step();
        assert_eq!(sim.last.event, Some(Event::Install));
        assert_eq!((sim.last.phase, sim.last.gate), (Phase::Compact, GATE_Z));
        assert!(
            !sim.last.build,
            "the form is served and building is held off"
        );
        assert_eq!(sim.sel.counts().build_suspended, 1);
    }

    #[test]
    fn the_budget_holds_off_building_in_compact_too() {
        let mut sim = compact(&Cell::ten_million());
        assert!(sim.last.build, "COMPACT builds the partitions a merge made");
        // Four superseded: allowed.
        sim.collected.superseded = 4;
        sim.step();
        assert!(sim.last.build);
        assert_eq!(sim.last.event, None);
        // A fifth: building is suspended for a dwell and the form is served.
        sim.collected.superseded = 1;
        sim.step();
        assert_eq!(sim.last.event, Some(Event::BuildsSuspended));
        assert!(!sim.last.build);
        assert_eq!((sim.last.phase, sim.last.gate), (Phase::Compact, GATE_Z));
        // For a dwell, which is ten of the wall the build that made the form
        // took: not a switch, and not the end of the form.
        let dwell = sim.sel.dwell().as_secs_f64();
        assert!(dwell > DWELL_FLOOR.as_secs_f64());
        sim.run(dwell - 0.2);
        assert!(!sim.last.build);
        sim.run(0.2);
        assert!(sim.last.build);
        assert_eq!(sim.sel.counts().switches, 1, "not a switch");
    }

    #[test]
    fn a_sampler_that_declines_is_not_overruled_by_the_rent() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.declined = true;
        sim.run(600.0);
        // The rent is paid many times over and nothing is built.
        assert!(sim.log.iter().any(|r| r.excess >= 10.0 * cell.price));
        assert_eq!(sim.sel.counts().builds, 0);
        assert_eq!(sim.last.phase, Phase::Suspect);
        // And once the sampler changes its mind, the rent that stands pays.
        sim.declined = false;
        sim.run(1.0);
        assert_eq!(sim.last.phase, Phase::Building);
    }

    #[test]
    fn nothing_is_built_without_the_samplers_reading_and_it_is_asked_for_once() {
        let mut sel = ZSelect::new();
        let tick = |t: u64| Input {
            now: Duration::from_millis(100 * t),
            model: t.is_multiple_of(10).then_some(Model {
                phi_raw: 0.5,
                phi_z: Some(0.0),
                phi_z_scan: Some(0.0),
                refault_rate: 0.0,
                budget: 870_000_000,
            }),
            census: Census {
                reads: Window {
                    ops: 5000,
                    faults: 2000,
                },
                refaults: Some(2000),
                ..Census::default()
            },
            ..Input::default()
        };
        let mut asked = 0;
        for t in 1..=6000 {
            let d = sel.step(&tick(t));
            asked += usize::from(d.sample);
            assert!(d.phase != Phase::Building, "no reading, no build");
        }
        assert_eq!(sel.phase(), Phase::Suspect);
        assert!(asked > 5000, "it asks on every step it is without one");
        assert_eq!(sel.counts().samples_wanted, 1, "and counts asking once");
        assert!(sel.excess() > 100.0, "the rent was paid many times over");
    }

    #[test]
    fn re_entry_within_the_retention_attaches_what_was_kept() {
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.cell.cache = 4 << 30;
        sim.load = Load::Idle;
        sim.run_until(Phase::Fits, 900.0);
        let exit = sim.first(Event::Exit(Exit::E1));
        let dwell = sim.sel.dwell();
        // Back under the bound before the retention is over.
        sim.run(100.0);
        assert!(sim.sel.retained());
        sim.cell.cache = cell.cache;
        sim.load = sim.cell.uniform();
        sim.run(dwell.as_secs_f64() + 60.0);
        // The form files stood: the price was the attach's, the builder had
        // nothing to build, and no retirement came for them.
        let starts = sim.times(Event::BuildStart);
        assert_eq!(starts.len(), 2);
        assert!(starts[1] - exit >= dwell, "inside the dwell");
        assert!(starts[1] - exit < sim.sel.retain(), "inside the retention");
        assert_eq!(sim.built, 1.0);
        assert_eq!(sim.sel.counts().retired, 0);
        assert!(!sim.sel.retained());
        // FITS forgot the sampler's reading, so the second episode asked for
        // another: the partitions may have changed.
        assert_eq!(sim.sel.counts().samples_wanted, 2);
        let installs = sim.times(Event::Install);
        assert_eq!(installs.len(), 2);
        assert_eq!(installs[1] - starts[1], DT);
    }

    // ---- totality ---------------------------------------------------------------------

    #[test]
    fn a_clock_that_steps_back_is_held_at_the_latest() {
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.run(30.0);
        let (phase, dwell, excess) = (sim.last.phase, sim.sel.dwell(), sim.sel.excess());
        let d = sim.sel.step(&Input {
            now: Duration::from_secs(1),
            ..Input::default()
        });
        assert_eq!(d.phase, phase);
        assert_eq!(sim.sel.dwell(), dwell);
        assert_eq!(sim.sel.excess(), excess);
        assert_eq!(sim.sel.now, sim.now);
    }

    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    #[test]
    fn workloads_that_change_without_warning_obey_the_rules() {
        // Hours of workloads picked at random and changed every few minutes,
        // the cache moved, a neighbour coming and going, the sampler
        // declining, builds superseded: the checker holds the levels, the
        // edges and the spacing on every step. The seeds cover every exit.
        let mut seen_events: Vec<Event> = Vec::new();
        let mut seen_phases: Vec<Phase> = Vec::new();
        for seed in 1..=12u64 {
            let mut rng = Rng::new(seed);
            let cell = Cell::ten_million();
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            let mut next = 0.0;
            sim.run_with(8.0 * 3600.0, |s| {
                let t = s.now.as_secs_f64();
                if t >= next {
                    next = t + 10.0 + 600.0 * rng.unit();
                    s.cell.cache =
                        [937_000_000, 700_000_000, 1_300_000_000, 4 << 30][rng.below(4) as usize];
                    s.cell.threads = 1 + rng.below(3) as u32;
                    s.cell.neighbours = if rng.below(6) == 0 { 400.0 } else { 0.0 };
                    s.declined = rng.below(12) == 0;
                    let bound = s.cell.bound(s.cell.z as f64 / PAGE as f64 * rng.unit());
                    s.load = match rng.below(8) {
                        0 | 1 => s.cell.uniform(),
                        2 => s.cell.scans(),
                        3 => s.cell.hot(1 + rng.below(700)),
                        4 => Load::Idle,
                        5 => s.cell.fixed(rng.unit(), rng.unit() * rng.unit(), bound),
                        6 => Load::Hot {
                            raw_pages: 24_000.0,
                            z_pages: 13_000.0,
                            plateau: 0.3 * rng.unit(),
                        },
                        _ => Load::ColdFill {
                            pages: 200_000.0,
                            refaults: rng.below(2) == 0,
                        },
                    };
                    s.cold = 200_000.0 * rng.unit();
                }
                if rng.below(2000) == 0 {
                    s.collected.superseded = rng.below(4) as u32;
                    s.collected.installed = rng.below(3) as u32;
                }
            });
            for (_, e) in sim.events() {
                if !seen_events.contains(&e) {
                    seen_events.push(e);
                }
            }
            for r in &sim.log {
                if !seen_phases.contains(&r.d.phase) {
                    seen_phases.push(r.d.phase);
                }
            }
        }
        for e in [
            Event::Suspect,
            Event::Relax,
            Event::BuildStart,
            Event::BuildCancelled,
            Event::Install,
            Event::Exit(Exit::E1),
            Event::Exit(Exit::E2),
            Event::Exit(Exit::E3),
            Event::TrialStart,
            Event::TrialBack,
        ] {
            assert!(
                seen_events.contains(&e),
                "{e:?} not reached in {seen_events:?}"
            );
        }
        assert_eq!(seen_phases.len(), 5, "{seen_phases:?}");
    }

    #[test]
    fn mixes_of_reads_and_scans_obey_the_rules_too() {
        // The same hours of changing workloads, each a mix of both classes at
        // rates and faults picked at random, so that the rules that take the
        // classes together are crossed by traces and held by the checker.
        let mut installs = 0;
        let mut trials = 0;
        let mut exits = 0;
        for seed in 21..=28u64 {
            let mut rng = Rng::new(seed);
            let cell = Cell::ten_million();
            let mut sim = Sim::new(cell.clone(), cell.uniform());
            let mut next = 0.0;
            sim.run_with(6.0 * 3600.0, |s| {
                let t = s.now.as_secs_f64();
                if t >= next {
                    next = t + 10.0 + 600.0 * rng.unit();
                    s.cell.cache =
                        [937_000_000, 700_000_000, 1_300_000_000, 4 << 30][rng.below(4) as usize];
                    s.cell.threads = 1 + rng.below(3) as u32;
                    let c = &s.cell;
                    let bound = c.bound(c.z as f64 / PAGE as f64 * rng.unit());
                    let read = (
                        c.rate(c.t_hit, rng.unit()),
                        c.rate(c.t_z, rng.unit() * rng.unit()),
                    );
                    let scan = (
                        c.rate(c.t_hit_scan, 2.0 * rng.unit()),
                        c.rate(c.t_z_scan, 2.0 * rng.unit() * rng.unit()),
                    );
                    s.load = Load::Both {
                        read,
                        scan,
                        refaults: rng.below(4) != 0,
                        bound,
                    };
                }
            });
            installs += sim.count(Event::Install);
            trials += sim.count(Event::TrialStart);
            exits += sim
                .events()
                .iter()
                .filter(|(_, e)| matches!(e, Event::Exit(_)))
                .count();
        }
        assert!(
            installs >= 10 && trials >= 1 && exits >= 10,
            "{installs} installs, {trials} trials, {exits} exits"
        );
    }

    #[test]
    fn whatever_it_is_fed_nothing_panics_and_the_rules_hold() {
        // Garbage in every field, including numbers that are not numbers,
        // a clock that stops, steps back and jumps by hours. Under the checked
        // profile an overflow anywhere in the selector is a panic here.
        let weird = |rng: &mut Rng, base: f64| match rng.below(40) {
            0 => f64::NAN,
            1 => f64::INFINITY,
            2 => f64::NEG_INFINITY,
            3 => -base,
            4 => 0.0,
            5 => 1e300,
            _ => base * 2.0 * rng.unit(),
        };
        for seed in 1..=8u64 {
            let mut rng = Rng::new(100 + seed);
            let mut sel = ZSelect::new();
            let mut ck = Checker::default();
            let mut now = Duration::ZERO;
            let mut latest = Duration::ZERO;
            for step in 0..80_000u64 {
                now = match rng.below(100) {
                    0 => now.saturating_sub(secs_f(5.0 * rng.unit())),
                    1 | 2 => now.saturating_add(Duration::from_secs(rng.below(7200))),
                    3..=5 => now,
                    _ => now + DT,
                };
                latest = latest.max(now);
                let model = (step % 10 == 0 || rng.below(60) == 0).then(|| Model {
                    phi_raw: if rng.below(3) == 0 {
                        0.5
                    } else {
                        weird(&mut rng, 1.0)
                    },
                    phi_z: (rng.below(3) != 0).then(|| weird(&mut rng, 1.0)),
                    phi_z_scan: (rng.below(3) != 0).then(|| weird(&mut rng, 3.0)),
                    refault_rate: weird(&mut rng, 500.0),
                    budget: rng.next() >> rng.below(40),
                });
                let sampler = (rng.below(20) == 0).then(|| Sampler {
                    costs: Costs {
                        t_f: (rng.below(2) == 0).then(|| weird(&mut rng, 1e-4)),
                        t_hit: (rng.below(2) == 0).then(|| weird(&mut rng, 1e-6)),
                        t_hit_scan: None,
                        t_z: (rng.below(2) == 0).then(|| weird(&mut rng, 1e-5)),
                        t_z_scan: (rng.below(2) == 0).then(|| weird(&mut rng, 1e-5)),
                    },
                    c_build: weird(&mut rng, 10.0),
                    declined: rng.below(10) == 0,
                    generation: rng.below(3),
                });
                let window = |rng: &mut Rng| {
                    let ops = if rng.below(5) == 0 {
                        rng.next()
                    } else {
                        rng.below(20_000)
                    };
                    Window {
                        ops,
                        faults: if rng.below(8) == 0 {
                            rng.next()
                        } else {
                            rng.below(ops.max(1).saturating_mul(2))
                        },
                    }
                };
                let input = Input {
                    now,
                    model,
                    sampler,
                    census: Census {
                        reads: window(&mut rng),
                        scans: window(&mut rng),
                        mixed: rng.below(3),
                        refaults: (rng.below(4) != 0).then(|| rng.below(20_000)),
                        threads: rng.below(5) as u32,
                        costs: Costs {
                            t_f: (rng.below(3) == 0).then(|| weird(&mut rng, 1e-4)),
                            ..Costs::default()
                        },
                    },
                    build: Build {
                        coverage: weird(&mut rng, 1.0),
                        installed: rng.below(3) as u32,
                        superseded: if rng.below(10) == 0 {
                            rng.below(12) as u32
                        } else {
                            0
                        },
                        failed: rng.below(2) as u32,
                    },
                    footprint: (rng.below(3) == 0).then(|| rng.next() >> rng.below(40)),
                    generation: rng.below(3),
                };
                let d = sel.step(&input);
                let r = Rec {
                    t: latest,
                    d,
                    dwell: sel.dwell(),
                    backoff: sel.backoff(),
                    excess: sel.excess(),
                    built: 0.0,
                    faults: 0,
                };
                if let Err(why) = ck.see(&r) {
                    panic!("seed {seed} step {step}: {why}");
                }
                assert!(!sel.excess().is_nan() && sel.excess() >= 0.0);
                assert_eq!(d.phase == Phase::Fits, !d.census);
            }
        }
    }

    // ---- what a census nobody ran, and a reading of other partitions, may not say ----

    #[test]
    fn a_census_nobody_ran_prices_nothing_in_memory() {
        // The model reads exactly nothing for a store that fits. The handles
        // run no census in FITS, and one that arrives anyway -- costs that put
        // the compact form under a hit, a sampler that costs nothing, a
        // footprint -- is not a reading of anything: without that, a
        // difference of costs of zero is a break-even of zero and a model of
        // zero is over it.
        let polluted = Costs {
            t_f: Some(2e-6),
            t_hit: Some(5e-6),
            t_hit_scan: Some(30e-6),
            t_z: Some(1e-9),
            t_z_scan: Some(1e-9),
        };
        let mut sel = ZSelect::new();
        for step in 1..=72_000u64 {
            let d = sel.step(&Input {
                now: DT * step as u32,
                model: step.is_multiple_of(10).then_some(Model {
                    phi_raw: 0.0,
                    phi_z: None,
                    phi_z_scan: None,
                    refault_rate: 0.0,
                    budget: 4 << 30,
                }),
                census: Census {
                    reads: Window {
                        ops: 5000,
                        faults: 0,
                    },
                    refaults: Some(0),
                    threads: 7,
                    mixed: 3,
                    costs: polluted,
                    ..Census::default()
                },
                sampler: Some(Sampler {
                    costs: polluted,
                    c_build: 0.0,
                    declined: false,
                    generation: 0,
                }),
                footprint: Some(1),
                ..Input::default()
            });
            assert_eq!(d.phase, Phase::Fits, "step {step}");
            assert!(!d.sample && !d.census);
        }
        let c = sel.counts();
        assert_eq!((c.samples_wanted, c.builds, c.switches), (0, 0, 0));
        assert_eq!((c.s1_ticks, c.mixed_windows, c.ignored), (7200, 0, 0));
        // Nothing of it was kept.
        assert_eq!(sel.measured, Costs::default());
        assert_eq!((sel.threads, sel.footprint), (1, None));
        assert!(sel.sampler.is_none());
    }

    #[test]
    fn a_cost_the_census_could_not_have_measured_in_the_form_that_served_is_not_taken() {
        // The census times the form the gate serves: its cost of a hit is the
        // raw form's, and its cost of a compact read the compact form's. In
        // SUSPECT the raw form serves, and a census that claims a compact
        // read's cost has claimed something it did not measure; in COMPACT
        // the reverse; in the trial the raw form serves again.
        let census = |t_hit: f64, t_z: f64| Census {
            costs: Costs {
                t_f: Some(30e-6),
                t_hit: Some(t_hit),
                t_hit_scan: Some(t_hit),
                t_z: Some(t_z),
                t_z_scan: Some(t_z),
            },
            ..Census::default()
        };
        let feed = |sel: &mut ZSelect, c: Census| {
            let now = sel.now + DT;
            sel.step(&Input {
                now,
                census: c,
                ..Input::default()
            });
        };
        let cell = Cell::ten_million();
        // SUSPECT: the hit is taken, the compact read is not.
        let mut sel = ZSelect::new();
        drive(&mut sel, 2, |_| (0.5, 0.0));
        assert_eq!(sel.phase(), Phase::Suspect);
        feed(&mut sel, census(0.5e-6, 1e-9));
        assert_eq!(
            (
                sel.measured.t_f,
                sel.measured.t_hit,
                sel.measured.t_hit_scan
            ),
            (Some(30e-6), Some(0.5e-6), Some(0.5e-6))
        );
        assert_eq!((sel.measured.t_z, sel.measured.t_z_scan), (None, None));
        // The build serves the raw form too.
        let mut sim = building(&cell);
        feed(&mut sim.sel, census(0.4e-6, 1e-9));
        let after = sim.sel.measured;
        assert_eq!(
            (after.t_hit, after.t_hit_scan),
            (Some(0.4e-6), Some(0.4e-6))
        );
        assert_eq!((after.t_z, after.t_z_scan), (None, None));
        // COMPACT: the compact read is taken, the hit is not.
        let mut sim = compact(&cell);
        let before = sim.sel.measured;
        feed(&mut sim.sel, census(1e-9, 7e-6));
        let after = sim.sel.measured;
        assert_eq!(
            (after.t_hit, after.t_hit_scan),
            (before.t_hit, before.t_hit_scan)
        );
        assert_eq!((after.t_z, after.t_z_scan), (Some(7e-6), Some(7e-6)));
        // The trial: the raw form serves.
        let mut sim = trial_started(&cell, PAGE);
        let before = sim.sel.measured;
        feed(&mut sim.sel, census(0.6e-6, 1e-9));
        let after = sim.sel.measured;
        assert_eq!(
            (after.t_hit, after.t_hit_scan),
            (Some(0.6e-6), Some(0.6e-6))
        );
        assert_eq!((after.t_z, after.t_z_scan), (before.t_z, before.t_z_scan));
    }

    #[test]
    fn a_hit_measured_over_the_compact_read_does_not_make_every_threshold_zero() {
        // The two costs are measured by two instruments, and here the census's
        // hit is over the prior's compact read. The break-even keeps its floor,
        // so the thresholds stay over zero and a model reading of nothing is
        // under the way in however the costs fall.
        let mut sel = ZSelect::new();
        drive(&mut sel, 2, |_| (0.5, 0.0));
        assert_eq!(sel.phase(), Phase::Suspect);
        let now = sel.now + DT;
        sel.step(&Input {
            now,
            census: Census {
                costs: Costs {
                    t_hit: Some(PRIOR_T_Z + 1e-6),
                    t_hit_scan: Some(PRIOR_T_Z_SCAN + 1e-6),
                    ..Costs::default()
                },
                ..Census::default()
            },
            ..Input::default()
        });
        let floor = SAVING_FLOOR_SHARE * (PRIOR_T_Z - PRIOR_T_HIT) / PRIOR_T_F;
        let scan_floor = SAVING_FLOOR_SHARE * (PRIOR_T_Z_SCAN - PRIOR_T_HIT_SCAN) / PRIOR_T_F;
        assert_eq!(sel.saving_star(Class::Read), floor);
        assert_eq!(sel.saving_star(Class::Scan), scan_floor);
        // The floor written out: a quarter of the priors' gaps, 3.67 us and
        // 8 us, over the 41.5 us a fault costs.
        assert!((floor - 0.022_108).abs() < 1e-6, "{floor}");
        assert!((scan_floor - 0.048_193).abs() < 1e-6, "{scan_floor}");
        assert!(sel.theta_sus(Class::Read) > 0.0 && sel.theta_enter(Class::Scan) > 0.0);
        // A model of nothing is under the way out, so the rung is left, and
        // one over the floor's threshold is not.
        let floor_sus = SUSPECT_FACTOR * floor;
        let mut calm = sel.clone();
        assert_eq!(kinds(&drive(&mut calm, 5, |_| (0.0, 0.0))), [Event::Relax]);
        let mut hot = sel.clone();
        assert!(drive(&mut hot, 60, |_| (1.1 * floor_sus, 0.0)).is_empty());
    }

    /// A selector fed by hand in SUSPECT with a hot model, a census that has
    /// evidence in every step, and a sampler it has not heard from.
    fn suspect_by_hand() -> (ZSelect, u64) {
        let mut sel = ZSelect::new();
        for step in 1..=100 {
            hand(&mut sel, step, 0, None);
            if sel.phase() == Phase::Suspect {
                return (sel, step);
            }
        }
        panic!("a hot model for ten seconds is not SUSPECT");
    }

    /// One step by hand: reads that fault on 40% of them and refault, the
    /// model on the tick, and what the caller gives of a sampler.
    fn hand(sel: &mut ZSelect, step: u64, generation: u64, sampler: Option<Sampler>) -> Decision {
        let reads = Window {
            ops: 5000,
            faults: 2000,
        };
        hand_with(sel, step, generation, sampler, (reads, 2000), 0.0)
    }

    /// One step by hand as [`hand`], with the interval's reads and the
    /// group's refaults given, and the share of the footprint the builder has
    /// attached.
    fn hand_with(
        sel: &mut ZSelect,
        step: u64,
        generation: u64,
        sampler: Option<Sampler>,
        (reads, refaults): (Window, u64),
        coverage: f64,
    ) -> Decision {
        sel.step(&Input {
            now: DT * step as u32,
            model: step.is_multiple_of(10).then_some(Model {
                phi_raw: 0.5,
                phi_z: Some(0.0),
                phi_z_scan: Some(0.0),
                refault_rate: 0.0,
                budget: 870_000_000,
            }),
            census: Census {
                reads,
                refaults: Some(refaults),
                ..Census::default()
            },
            build: Build {
                coverage,
                ..Build::default()
            },
            sampler,
            generation,
            ..Input::default()
        })
    }

    fn reading(c_build: f64, declined: bool, generation: u64) -> Sampler {
        Sampler {
            costs: Costs::default(),
            c_build,
            declined,
            generation,
        }
    }

    #[test]
    fn a_reading_of_other_partitions_is_dropped_and_asked_for_again() {
        let (mut sel, mut step) = suspect_by_hand();
        // The reading arrives and stands.
        step += 1;
        let d = hand(&mut sel, step, 0, Some(reading(0.1, false, 0)));
        assert!(!d.sample && sel.sampler.is_some());
        // A merge lands: the partitions are other, the reading is of the old
        // ones, and it is asked for again. Nothing is built meanwhile,
        // though the rent is paid many times over.
        let mut asked = 0;
        for _ in 0..200 {
            step += 1;
            let d = hand(&mut sel, step, 1, None);
            asked += usize::from(d.sample);
            assert!(
                d.phase != Phase::Building,
                "no reading of these partitions, no build"
            );
        }
        assert_eq!(asked, 200);
        assert!(sel.sampler.is_none());
        assert_eq!(sel.counts().samples_wanted, 2);
        assert!(sel.excess() > 10.0);
        // A reading that arrives for the old partitions is not kept, and
        // counted.
        step += 1;
        let ignored = sel.counts().ignored;
        hand(&mut sel, step, 1, Some(reading(0.1, false, 0)));
        assert!(sel.sampler.is_none());
        assert_eq!(sel.counts().ignored, ignored + 1);
        // One for these is, and the rent pays for the build at once.
        step += 1;
        let d = hand(&mut sel, step, 1, Some(reading(0.1, false, 1)));
        assert_eq!(d.event, Some(Event::BuildStart));
    }

    #[test]
    fn the_models_error_is_measured_over_capacity_misses_and_not_a_cold_start() {
        // A restart reads a store nothing has touched: every read faults, and
        // none of the faults is a page the cache once held. The evidence rule
        // takes no such interval, and neither does the measure of the raw
        // form's rate that the model is judged by when the form takes over:
        // taken over them it would read the cold fill into the raw form's rate
        // and file an error of the wrong sign.
        let cold = Window {
            ops: 5000,
            faults: 5000,
        };
        let warm = Window {
            ops: 5000,
            faults: 2000,
        };
        let (mut sel, mut step) = suspect_by_hand();
        for _ in 0..6 {
            step += 1;
            hand_with(&mut sel, step, 0, None, (cold, 0), 0.0);
        }
        assert_eq!(sel.excess(), 0.0, "a cold start is not evidence");
        // The reads that follow refault and fault on 40% of them, the model
        // said 50%. The share holds only once the refaults are half of the
        // window's faults, which is fifteen intervals on.
        let mut started = None;
        for k in 1..=40 {
            step += 1;
            let d = hand_with(
                &mut sel,
                step,
                0,
                Some(reading(0.1, false, 0)),
                (warm, 2000),
                0.0,
            );
            if d.event == Some(Event::BuildStart) {
                started = Some(k);
                break;
            }
        }
        assert_eq!(started, Some(16));
        step += 1;
        let d = hand_with(&mut sel, step, 0, None, (warm, 2000), 1.0);
        assert_eq!(d.event, Some(Event::Install));
        assert!((sel.bias - (0.4 - 0.5)).abs() < 1e-9, "{}", sel.bias);
    }

    /// The costs at round numbers, so that the thresholds are numbers a float
    /// holds: a fault costs a second, a hit a quarter and a compact read three
    /// quarters, which makes the break-even half a fault a read, the entry
    /// threshold three quarters and the way in a quarter.
    fn round_costs() -> Costs {
        Costs {
            t_f: Some(1.0),
            t_hit: Some(0.25),
            t_hit_scan: Some(0.25),
            t_z: Some(0.75),
            t_z_scan: Some(0.75),
        }
    }

    /// A selector in COMPACT whose model says half a fault a read, with the
    /// bitmaps of what the handles touched read.
    fn compact_selector() -> ZSelect {
        let mut sel = ZSelect::new();
        sel.model = Some(model_of(0.5));
        sel.phase = Phase::Compact;
        sel.footprint = Some(1 << 20);
        sel
    }

    /// A selector fed by hand to the step its build starts at.
    fn building_by_hand() -> (ZSelect, u64) {
        let (mut sel, mut step) = suspect_by_hand();
        for _ in 0..200 {
            step += 1;
            let d = hand(&mut sel, step, 0, Some(reading(0.1, false, 0)));
            if d.event == Some(Event::BuildStart) {
                return (sel, step);
            }
        }
        panic!("the rent was not paid");
    }

    #[test]
    fn a_build_is_installed_at_the_covered_share_itself_and_not_a_hair_under() {
        let reads = Window {
            ops: 5000,
            faults: 2000,
        };
        for (coverage, installs) in [
            (COVERED_ENOUGH * (1.0 - 1e-9), false),
            (COVERED_ENOUGH, true),
        ] {
            let (mut sel, step) = building_by_hand();
            let d = hand_with(&mut sel, step + 1, 0, None, (reads, 2000), coverage);
            assert_eq!(d.event == Some(Event::Install), installs, "{coverage}");
        }
    }

    #[test]
    fn a_build_is_cancelled_under_half_the_entry_threshold_and_not_at_it() {
        // With the costs at round numbers the build is given up under three
        // eighths of a fault a read, half of the entry threshold.
        let reads = |faults: u64| Window {
            ops: 10_000,
            faults,
        };
        for (faults, cancelled) in [(3_750, false), (3_749, true)] {
            let (mut sel, mut step) = building_by_hand();
            sel.measured = round_costs();
            assert_eq!(sel.theta_enter(Class::Read) * VANISH_FACTOR, 0.375);
            let mut event = None;
            for _ in 0..2 * VANISH_INTERVALS {
                step += 1;
                event = event
                    .or(hand_with(&mut sel, step, 0, None, (reads(faults), faults), 0.0).event);
            }
            assert_eq!(event == Some(Event::BuildCancelled), cancelled, "{faults}");
        }
    }

    #[test]
    fn the_compact_forms_readings_need_sixty_four_reads_and_a_measure_ten_seconds() {
        // E3 reads a second of the form's reads: sixty-four are a reading
        // and a form that faults on every one of them is not paying, and
        // sixty-three break the stretch.
        for (ops, unpaying) in [(63, false), (64, true)] {
            let mut sel = compact_selector();
            sel.watch.tick_reads = Window { ops, faults: ops };
            sel.update_e3(Duration::from_secs(5));
            assert_eq!(sel.watch.unpaying_since.is_some(), unpaying, "{ops}");
        }
        // E2's measure ends ten seconds on, to the step, and holds data from
        // sixty-four reads.
        let quiet = |ops| Window { ops, faults: 0 };
        let mut sel = compact_selector();
        sel.watch.e2_win[Class::Read.ix()] = quiet(64);
        sel.update_e2(E2_MEASURE - Duration::from_nanos(1));
        assert_eq!(sel.watch.e2_start, Duration::ZERO);
        sel.update_e2(E2_MEASURE);
        assert_eq!((sel.watch.e2_start, sel.watch.e2_run), (E2_MEASURE, 1));
        let mut sel = compact_selector();
        sel.watch.e2_win[Class::Read.ix()] = quiet(63);
        sel.update_e2(E2_MEASURE);
        assert_eq!(sel.watch.e2_run, 0);
        // The pages touched fit when they are under half the budget: half of
        // it exactly does not.
        for (bound, fits) in [(435_000_000, false), (434_999_999, true)] {
            let mut sel = compact_selector();
            sel.footprint = Some(bound);
            sel.watch.e2_win[Class::Read.ix()] = quiet(64);
            sel.update_e2(E2_MEASURE);
            assert_eq!(sel.watch.e2_run, u32::from(fits), "{bound}");
        }
    }

    /// A selector in a trial of the raw form that expects `pages` raw pages.
    fn in_a_trial(pages: u64) -> ZSelect {
        let mut sel = ZSelect::new();
        sel.model = Some(model_of(0.5));
        sel.phase = Phase::Trial;
        sel.trial = Some(Trial {
            started: Duration::ZERO,
            pages,
            faults: 0,
            low_run: 0,
            judged: None,
            pooled: [Window::default(); 2],
            intervals: 0,
        });
        sel
    }

    fn judged(sel: &ZSelect) -> bool {
        sel.trial.as_ref().expect("the trial runs").judged.is_some()
    }

    #[test]
    fn a_trial_is_judged_at_the_expected_pages_the_wall_and_the_settled_run_themselves() {
        // The fill is over when the faults reach a fifth over the pages the
        // raw form needs: six of five, and not five.
        for (faults, over) in [(5, false), (6, true)] {
            let mut sel = in_a_trial(5);
            let census = Census {
                reads: Window { ops: 0, faults },
                ..Census::default()
            };
            sel.trial_step(
                &Input {
                    census,
                    ..Input::default()
                },
                Duration::ZERO,
            );
            assert_eq!(judged(&sel), over, "{faults}");
        }
        // Or when the wall has passed, to the step, and the wall is capped.
        for pages in [1000, 100_000_000] {
            let wall = ZSelect::new().trial_wall(pages);
            assert!(wall <= TRIAL_WALL_CAP);
            let mut sel = in_a_trial(pages);
            sel.trial_step(&Input::default(), wall - Duration::from_nanos(1));
            assert!(!judged(&sel), "{pages}");
            sel.trial_step(&Input::default(), wall);
            assert!(judged(&sel), "{pages}");
        }
        // Or when the rate has settled under the entry threshold, which with
        // the costs at round numbers is three quarters of a fault a read: a
        // run of intervals at it is not a settled one.
        for (faults, low_run) in [(7_500, 0), (7_499, 3)] {
            let mut sel = in_a_trial(1 << 40);
            sel.measured = round_costs();
            sel.trial.as_mut().expect("the trial runs").low_run = 2;
            let census = Census {
                reads: Window {
                    ops: 10_000,
                    faults,
                },
                refaults: Some(faults),
                ..Census::default()
            };
            sel.trial_step(
                &Input {
                    census,
                    ..Input::default()
                },
                Duration::ZERO,
            );
            assert_eq!(
                sel.trial.as_ref().expect("still").low_run,
                low_run,
                "{faults}"
            );
        }
    }

    #[test]
    fn a_trial_is_wrong_when_the_raw_form_saves_the_entry_threshold_itself() {
        // Three intervals pooled at three quarters of a fault a read, over a
        // compact form that faults at nothing: the saving is the entry
        // threshold to the bit and the form is wanted, and a hair under it the
        // raw form fits.
        for (faults, wrong) in [(7_500, true), (7_499, false)] {
            let mut sel = in_a_trial(5);
            sel.measured = round_costs();
            let trial = sel.trial.as_mut().expect("the trial runs");
            trial.judged = Some(Duration::ZERO);
            trial.intervals = TRIAL_DECIDE_INTERVALS - 1;
            trial.pooled[Class::Read.ix()] = Window {
                ops: 20_000,
                faults: 15_000,
            };
            let census = Census {
                reads: Window {
                    ops: 10_000,
                    faults,
                },
                refaults: Some(faults),
                ..Census::default()
            };
            let event = sel.trial_step(
                &Input {
                    census,
                    ..Input::default()
                },
                DT,
            );
            let want = if wrong {
                Event::TrialBack
            } else {
                Event::Exit(Exit::E2)
            };
            assert_eq!(event, Some(want), "{faults}");
        }
    }

    #[test]
    fn e3_and_e2_hold_the_break_even_and_the_way_in_themselves() {
        // E3: a form that saves the break-even, half a fault a read, pays; a
        // hair short of it does not.
        for (faults, unpaying) in [(25, false), (26, true)] {
            let mut sel = compact_selector();
            sel.measured = round_costs();
            sel.model = Some(model_of(0.75));
            sel.watch.tick_reads = Window { ops: 100, faults };
            sel.update_e3(Duration::from_secs(5));
            assert_eq!(sel.watch.unpaying_since.is_some(), unpaying, "{faults}");
        }
        // E2: the compact form's faults are low under the way in, a quarter
        // of a fault a read, and not at it.
        for (faults, low) in [(25, false), (24, true)] {
            let mut sel = compact_selector();
            sel.measured = round_costs();
            sel.watch.e2_win[Class::Read.ix()] = Window { ops: 100, faults };
            sel.update_e2(E2_MEASURE);
            assert_eq!(sel.watch.e2_run, u32::from(low), "{faults}");
        }
    }

    #[test]
    fn form_files_are_retired_when_the_retention_ends_to_the_step() {
        let mut sel = ZSelect::new();
        sel.phase = Phase::Suspect;
        let end = Duration::from_secs(600);
        sel.retain_until = Some(end);
        assert!(!sel.retire(end - Duration::from_nanos(1)));
        assert!(sel.retire(end));
        assert!(!sel.retained());
    }

    #[test]
    fn a_trial_nobody_can_decide_is_undone_a_dwell_after_the_fill_to_the_step() {
        let mut sel = in_a_trial(5);
        let since = Duration::from_secs(10);
        sel.trial.as_mut().expect("the trial runs").judged = Some(since);
        let late = since + sel.dwell();
        let quiet = Input::default();
        assert_eq!(sel.trial_step(&quiet, late - Duration::from_nanos(1)), None);
        assert_eq!(sel.trial_step(&quiet, late), Some(Event::TrialBack));
    }

    #[test]
    fn a_trial_decided_on_scans_alone_files_no_error_of_the_reads() {
        // The pooled intervals hold scans and no point reads, and the scans
        // say the raw form still faults: the form is wanted, and the model's
        // word about the point read is not contradicted by reads that were
        // never made.
        let mut sel = in_a_trial(5);
        let trial = sel.trial.as_mut().expect("the trial runs");
        trial.judged = Some(Duration::ZERO);
        trial.intervals = TRIAL_DECIDE_INTERVALS - 1;
        let scans = Window {
            ops: 1000,
            faults: 1000,
        };
        trial.pooled[Class::Scan.ix()] = scans;
        let census = Census {
            scans,
            refaults: Some(1000),
            ..Census::default()
        };
        let event = sel.trial_step(
            &Input {
                census,
                ..Input::default()
            },
            DT,
        );
        assert_eq!(event, Some(Event::TrialBack));
        assert_eq!(sel.bias, 0.0);
    }

    #[test]
    fn e1_and_e3_act_on_a_stretch_a_dwell_old_to_the_step() {
        let began = Duration::from_secs(10);
        for exit in [Exit::E1, Exit::E3] {
            let mut sel = ZSelect::new();
            sel.model = Some(model_of(0.5));
            sel.phase = Phase::Compact;
            match exit {
                Exit::E1 => sel.low_since = Some(began),
                _ => sel.watch.unpaying_since = Some(began),
            }
            let late = began + sel.dwell();
            let input = Input::default();
            assert_eq!(
                sel.compact(&input, late - Duration::from_nanos(1), false, false),
                None,
                "{exit:?}"
            );
            assert_eq!(
                sel.compact(&input, late, false, false),
                Some(Event::Exit(exit)),
                "{exit:?}"
            );
        }
    }

    /// The model's reading with only the raw form's rate given.
    fn model_of(phi_raw: f64) -> Model {
        Model {
            phi_raw,
            phi_z: Some(0.0),
            phi_z_scan: Some(0.0),
            refault_rate: 0.0,
            budget: 870_000_000,
        }
    }

    #[test]
    fn the_models_error_and_the_scans_held_against_it_begin_at_the_threshold_and_not_over_it() {
        let mut sel = ZSelect::new();
        let sus = sel.theta_sus(Class::Read);
        let sus_scan = sel.theta_sus(Class::Scan);
        // The model said nothing and the raw form took exactly the rate worth
        // counting, or the model said it and the raw form took nothing: both
        // are errors, and a hair under is the model right to within what is
        // worth acting on.
        sel.model = Some(model_of(0.0));
        sel.note_raw(Some(sus), None);
        assert_eq!(sel.bias, sus);
        sel.note_raw(Some(sus * (1.0 - 1e-9)), None);
        assert_eq!(sel.bias, 0.0);
        sel.model = Some(model_of(sus));
        sel.note_raw(Some(0.0), None);
        assert_eq!(sel.bias, -sus);
        // A scan that faults at its class's threshold against a model that
        // says the store fits is held against it, and one a hair under is not.
        sel.model = Some(model_of(0.0));
        sel.note_raw(None, Some(sus_scan * (1.0 - 1e-9)));
        assert!(!sel.scans_against);
        sel.note_raw(None, Some(sus_scan));
        assert!(sel.scans_against);
        // A model that already says the rate worth counting has not said the
        // store fits: there is nothing to hold the scans against.
        sel.scans_against = false;
        sel.model = Some(model_of(sus));
        sel.note_raw(None, Some(10.0 * sus_scan));
        assert!(!sel.scans_against);
    }

    #[test]
    fn the_raw_forms_rate_needs_sixty_four_reads_in_the_intervals_that_counted() {
        let seen = |reads: Window, scans: Window, counted: bool| Seen {
            reads,
            scans,
            counted,
            ..Seen::default()
        };
        let w = |ops: u64, faults: u64| Window { ops, faults };
        let mut sel = ZSelect::new();
        sel.share.push(DT, seen(w(63, 21), w(0, 0), true));
        assert_eq!(sel.raw_seen_in(Class::Read), None);
        sel.share.push(2 * DT, seen(w(1, 0), w(64, 32), true));
        assert_eq!(sel.raw_seen_in(Class::Read), Some(21.0 / 64.0));
        // Each class has its own measure.
        assert_eq!(sel.raw_seen_in(Class::Scan), Some(0.5));
        // An interval the evidence rule did not take is not in either.
        sel.share
            .push(3 * DT, seen(w(1000, 1000), w(1000, 1000), false));
        assert_eq!(sel.raw_seen_in(Class::Read), Some(21.0 / 64.0));
        assert_eq!(sel.raw_seen_in(Class::Scan), Some(0.5));
    }

    #[test]
    fn the_faults_stand_in_for_the_refault_counter_from_the_interval_that_fills_the_cache() {
        // Where the platform counts no refaults the faults taken since SUSPECT
        // are measured against the cache's size in pages: the interval that
        // reaches it is evidence and the one before it is not.
        let mut sel = ZSelect::new();
        sel.model = Some(Model {
            budget: 1000 * PAGE,
            ..model_of(0.5)
        });
        let census = Census {
            reads: Window {
                ops: 100,
                faults: 1,
            },
            refaults: None,
            ..Census::default()
        };
        sel.faults_since_suspect = 998;
        assert!(!sel.evidence(&census, DT).any());
        assert!(sel.evidence(&census, 2 * DT).valid[Class::Read.ix()]);
    }

    #[test]
    fn the_rungs_are_entered_and_left_at_the_thresholds_themselves() {
        let mut sel = ZSelect::new();
        let sus = sel.theta_sus(Class::Read);
        let at = |k: u32, phi_raw: f64| Input {
            now: k * S1_PERIOD,
            model: Some(model_of(phi_raw)),
            ..Input::default()
        };
        // The way in is the threshold: a model reading it, two ticks running,
        // is SUSPECT.
        assert_eq!(sel.step(&at(1, sus)).event, None);
        assert_eq!(sel.step(&at(2, sus)).event, Some(Event::Suspect));
        // The way out is under half of it: at half of it the rung holds,
        // however long, and a hair under it, five ticks running, is FITS.
        let half = sus * RELAX_FACTOR;
        for k in 3..=20 {
            assert_eq!(sel.step(&at(k, half)).event, None, "{k}");
        }
        assert_eq!(sel.phase(), Phase::Suspect);
        for k in 21..=25 {
            sel.step(&at(k, half * (1.0 - 1e-9)));
        }
        assert_eq!(sel.phase(), Phase::Fits);
    }

    #[test]
    fn the_build_clause_is_open_at_each_threshold_itself() {
        // The price paid to the last bit, the predicted saving and the
        // measured saving each exactly at the entry threshold: all three are
        // "at least", and a hair short of any one of them refuses.
        let mut sel = ZSelect::new();
        let enter = sel.theta_enter(Class::Read);
        sel.sampler = Some(Sampler {
            costs: Costs::default(),
            c_build: 5.0,
            declined: false,
            generation: 0,
        });
        let short = 1.0 - 1e-9;
        let mut open = |excess: f64, raw: f64, measured: f64| {
            sel.excess = excess;
            sel.model = Some(Model {
                phi_z: Some(0.0),
                ..model_of(raw)
            });
            let ev = Evidence {
                valid: [true, false],
                saving: [measured, 0.0],
                ..Evidence::default()
            };
            sel.build_clause(DT, &ev)
        };
        assert!(open(5.0, enter, enter));
        assert!(!open(5.0 * short, enter, enter));
        assert!(!open(5.0, enter * short, enter));
        assert!(!open(5.0, enter, enter * short));
    }

    #[test]
    fn a_dwell_and_a_backoff_are_over_at_their_last_step_and_not_the_one_after() {
        let mut sel = ZSelect::new();
        let t = Duration::from_secs(100);
        let ns = Duration::from_nanos(1);
        sel.last_switch = Some(t);
        assert!(!sel.switch_allowed(t + sel.dwell() - ns));
        assert!(sel.switch_allowed(t + sel.dwell()));
        sel.last_trial_end = Some(t);
        assert!(!sel.trial_allowed(t + sel.backoff() - ns));
        assert!(sel.trial_allowed(t + sel.backoff()));
    }

    #[test]
    fn the_pressure_term_is_over_its_threshold_and_not_at_it() {
        let mut sel = ZSelect::new();
        let at = |refault_rate| Model {
            refault_rate,
            ..model_of(0.0)
        };
        sel.model = Some(at(PRESSURE_REFAULTS_PER_S));
        assert!(!sel.pressure());
        sel.model = Some(at(PRESSURE_REFAULTS_PER_S * (1.0 + 1e-9)));
        assert!(sel.pressure());
    }

    #[test]
    fn the_models_word_is_calm_under_half_the_threshold_and_not_at_it() {
        for (scale, calm) in [(1.0 - 1e-9, true), (1.0, false)] {
            let mut sel = ZSelect::new();
            let edge = sel.theta_sus(Class::Read) * RELAX_FACTOR;
            sel.step(&Input {
                now: S1_PERIOD,
                model: Some(model_of(edge * scale)),
                ..Input::default()
            });
            assert_eq!(sel.low_since.is_some(), calm, "{scale}");
        }
    }

    #[test]
    fn a_reading_whose_price_is_not_a_price_is_not_kept() {
        let (mut sel, mut step) = suspect_by_hand();
        for (k, price) in [f64::NAN, f64::INFINITY, -1.0].into_iter().enumerate() {
            step += 1;
            hand(&mut sel, step, 0, Some(reading(price, false, 0)));
            assert!(sel.sampler.is_none(), "{price}");
            assert_eq!(sel.counts().ignored, k as u64 + 1);
        }
        // A build that costs nothing has a price.
        step += 1;
        hand(&mut sel, step, 0, Some(reading(0.0, false, 0)));
        assert!(sel.sampler.is_some());
    }

    #[test]
    fn a_decline_is_asked_again_after_a_dwell() {
        // The sampler declined the partitions at the budget they had. The
        // budget and the partitions move, and a decline that stood for the
        // episode would stand for ever; it stands for a dwell, and the
        // sampler is asked again.
        let cell = Cell::ten_million();
        let mut sim = Sim::new(cell.clone(), cell.uniform());
        sim.sampler_once = true;
        sim.declined = true;
        sim.run(600.0);
        assert_eq!(sim.sel.counts().builds, 0);
        assert!(sim.log.iter().any(|r| r.excess >= 10.0 * cell.price));
        let asked = sim.sel.counts().samples_wanted;
        assert!((9..=11).contains(&asked), "{asked} asks in ten minutes");
        // The wait from one ask to the next is the dwell to the step: the
        // reading arrives the step after the ask and stands for a dwell.
        let asks: Vec<Duration> = sim
            .log
            .windows(2)
            .filter(|w| w[1].d.sample && !w[0].d.sample)
            .map(|w| w[1].t)
            .collect();
        assert_eq!(asks.len() as u64, asked);
        for pair in asks.windows(2) {
            assert_eq!(pair[1] - pair[0], DWELL_FLOOR + DT);
        }
        // It changes its mind; the next ask is the one that finds out.
        sim.declined = false;
        let changed = sim.now;
        sim.run(2.0 * DWELL_FLOOR.as_secs_f64());
        let start = sim.first(Event::BuildStart);
        assert!(
            start - changed <= DWELL_FLOOR + 3 * DT,
            "{:?}",
            start - changed
        );
        // A reading that does not decline is not asked for again.
        assert!(sim.sel.counts().samples_wanted <= asked + 1);
    }

    #[test]
    fn the_form_files_retire_with_the_reading_that_priced_them() {
        // A form left by E3 keeps its form files for the retention, and the
        // reading taken then prices a re-entry at the attach. When they are
        // retired a build costs what a build from nothing costs, and the
        // reading that said otherwise is gone with them.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 0.40, bound));
        sim.sampler_once = true;
        sim.run_until(Phase::Compact, 120.0);
        // The first reading priced a build from nothing, and the last
        // form file's attach moves the partitions' identity: the reading is
        // dropped.
        assert!(sim.sel.sampler.is_some());
        sim.run(5.0);
        assert!(sim.sel.sampler.is_none());
        sim.run_until(Phase::Suspect, 2.0 * sim.sel.dwell().as_secs_f64());
        assert_eq!(sim.count(Event::Exit(Exit::E3)), 1);
        sim.run(1.0);
        let attach = sim.sel.sampler.expect("asked for after the exit").c_build;
        assert_eq!(attach, cell.attach);
        // The retention ends: the form files go, and so does the price.
        let exit = sim.first(Event::Exit(Exit::E3));
        sim.run_with(sim.sel.retain().as_secs_f64(), |_| {});
        let retired = sim.log.iter().find(|r| r.d.retire).expect("retired").t;
        near(retired - exit, sim.sel.retain().as_secs_f64(), 0.2);
        sim.run(1.0);
        let scratch = sim
            .sel
            .sampler
            .expect("asked for after the retirement")
            .c_build;
        assert_eq!(scratch, cell.price);
        assert_eq!(sim.sel.counts().samples_wanted, 3);
    }

    #[test]
    fn the_retire_pulse_takes_the_reading_with_it_whoever_numbers_the_partitions() {
        // A caller that does not number the partitions again when the
        // form files go leaves the selector with a price for form files that are
        // not there; the pulse that retires them is its reason to ask again.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 0.40, bound));
        sim.sampler_once = true;
        sim.numbers_form_files = false;
        sim.run_until(Phase::Compact, 120.0);
        sim.run_until(Phase::Suspect, 2.0 * sim.sel.dwell().as_secs_f64());
        assert_eq!(sim.count(Event::Exit(Exit::E3)), 1);
        assert!(sim.sel.sampler.is_some(), "nothing moved the reading");
        let asked = sim.sel.counts().samples_wanted;
        sim.run(sim.sel.retain().as_secs_f64() + 5.0);
        let pulse = sim.log.iter().find(|r| r.d.retire).expect("retired");
        assert!(pulse.d.sample, "the pulse asks for another reading");
        assert_eq!(sim.sel.counts().samples_wanted, asked + 1);
        assert!(sim.sel.sampler.is_some(), "and gets it");
    }

    #[test]
    fn the_footprint_goes_with_the_form_it_was_taken_of() {
        // The bitmaps are of the compact form's pages; a form installed again
        // starts with none, and a bound taken of the form before it must not
        // admit a trial of this one.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        sim.step();
        assert!(sim.sel.footprint.is_some());
        sim.load = cell.hot(100);
        sim.run_until(Phase::Suspect, 2.0 * sim.sel.dwell().as_secs_f64() + 100.0);
        assert_eq!(sim.count(Event::Exit(Exit::E2)), 1);
        assert_eq!(sim.sel.footprint, None, "the form was left");
        sim.load = cell.uniform();
        sim.run_until(Phase::Compact, 600.0);
        assert_eq!(sim.last.phase, Phase::Compact);
        assert_eq!(sim.count(Event::Install), 2);
        assert_eq!(sim.sel.footprint, None, "the form was installed");
        // So a trial is admitted by what the new form's bitmaps say and by
        // nothing taken before them.
        sim.step();
        assert!(sim.sel.footprint.is_some());
    }

    #[test]
    fn a_decline_that_arrives_while_building_ends_the_build() {
        // Clause (e) is a condition of the build and not only of its start:
        // the sampler's reading changes while the builder works, and a build
        // for partitions it now declines is not finished, however much of it
        // is.
        let cell = Cell::ten_million();
        for finished in [false, true] {
            let mut sim = building(&cell);
            if finished {
                sim.built = 0.95;
            }
            sim.declined = true;
            sim.step();
            let abandoned = sim.first(Event::BuildAbandoned);
            assert_eq!(sim.last.phase, Phase::Suspect, "{finished}");
            let c = sim.sel.counts();
            assert_eq!((c.installs, c.build_cancelled, c.switches), (0, 1, 1));
            assert_eq!(c.build_suspended, 0, "the budget did not trip");
            assert_eq!(
                sim.sel.backoff(),
                TRIAL_BACKOFF_MIN,
                "the saving did not vanish"
            );
            assert!(sim.sel.retained());
            // It stays declined: the rent is paid and nothing is built for
            // as long as the sampler says so.
            sim.run(30.0);
            assert_eq!(sim.sel.counts().builds, 1);
            // And a sampler that changes its mind is believed after the dwell.
            sim.declined = false;
            sim.run(2.0 * DWELL_FLOOR.as_secs_f64());
            assert!(sim.times(Event::BuildStart).len() == 2, "{abandoned:?}");
        }
    }

    #[test]
    fn a_model_supplied_faster_than_the_period_is_ticks_at_the_period() {
        // The ticks FITS, SUSPECT and the smoothing count in are seconds.
        // A caller that supplies the model at every step has it read at every
        // step and counted a period apart, to within a census interval.
        let mut sel = ZSelect::new();
        let hot = Model {
            phi_raw: 0.5,
            phi_z: None,
            phi_z_scan: None,
            refault_rate: 0.0,
            budget: 870_000_000,
        };
        let mut first = None;
        for step in 1..=100u64 {
            let d = sel.step(&Input {
                now: DT * step as u32,
                model: Some(hot),
                ..Input::default()
            });
            if d.event == Some(Event::Suspect) {
                first = Some(step);
            }
        }
        // Ticks at 0.1 s and, a period less an interval later, at 1.0 s.
        assert_eq!(first, Some(10));
        assert_eq!(sel.counts().s1_ticks, 12, "twelve ticks in ten seconds");
        // Readings a period apart with a little jitter each are all ticks.
        let mut sel = ZSelect::new();
        let mut at = Duration::ZERO;
        for k in 0..20u32 {
            at += S1_PERIOD - Duration::from_millis(50 * u64::from(k % 2));
            sel.step(&Input {
                now: at,
                model: Some(hot),
                ..Input::default()
            });
        }
        assert_eq!(sel.counts().s1_ticks, 20);
        // Readings closer than that are kept for the rules but not counted.
        let mut sel = ZSelect::new();
        for k in 1..=40u32 {
            sel.step(&Input {
                now: Duration::from_millis(500) * k,
                model: Some(hot),
                ..Input::default()
            });
        }
        assert_eq!(sel.counts().s1_ticks, 20, "twenty seconds, twenty ticks");
        assert_eq!(sel.model, Some(hot));
    }

    // ---- holes found by breaking the rules one at a time ----------------------------

    #[test]
    fn a_scan_only_cold_start_is_not_a_capacity_miss() {
        // A cold start read by scans alone: the store's faults are the scans',
        // and the evidence rule has to count them. First touches are not
        // refaults and buy nothing; the same fill counted as refaults is bought.
        let cell = Cell {
            price: 3.0,
            ..Cell::ten_million()
        };
        for (refaults, builds) in [(false, 0), (true, 1)] {
            let load = Load::Rated {
                class: Class::Scan,
                raw: cell.rate(cell.t_hit_scan, cell.scan_raw),
                z: cell.rate(cell.t_z_scan, 0.0),
                refaults,
                bound: cell.bound(cell.z as f64 / PAGE as f64),
            };
            let mut sim = Sim::new(cell.clone(), load);
            sim.phi_raw_override = Some(0.48);
            sim.run(120.0);
            assert_eq!(sim.sel.counts().builds, builds, "{refaults}");
            if builds == 0 {
                assert!(sim.log.iter().all(|r| r.excess == 0.0));
            }
        }
    }

    #[test]
    fn each_phase_publishes_its_gate_word() {
        // The checker compares a decision's gate with `Phase::gate`, that is,
        // with itself; the words are written out here. (The numbers are the
        // ones the readers will load.)
        assert_eq!((GATE_RAW, GATE_CENSUS, GATE_Z), (0, 1, 2));
        for (phase, gate, census) in [
            (Phase::Fits, 0, false),
            (Phase::Suspect, 1, true),
            (Phase::Building, 1, true),
            (Phase::Compact, 2, true),
            (Phase::Trial, 1, true),
        ] {
            assert_eq!((phase.gate(), phase.census()), (gate, census), "{phase:?}");
        }
    }

    #[test]
    fn the_trials_wall_is_divided_among_the_threads() {
        // The pages the trial expects take a fault apiece, and the threads
        // fault them in together.
        let wall = |threads: u32| {
            let mut sel = ZSelect::new();
            drive(&mut sel, 2, |_| (0.5, 0.0));
            let now = sel.now + DT;
            sel.step(&Input {
                now,
                census: Census {
                    threads,
                    ..Census::default()
                },
                ..Input::default()
            });
            sel.trial_wall(30_000)
        };
        let one = wall(1);
        assert!(one > Duration::ZERO && one < TRIAL_WALL_CAP, "{one:?}");
        let three = wall(3);
        assert!(
            (three.as_secs_f64() * 3.0 - one.as_secs_f64()).abs() < 1e-6,
            "{one:?} {three:?}"
        );
        // A census that names no thread leaves the last count alone.
        let mut sel = ZSelect::new();
        drive(&mut sel, 2, |_| (0.5, 0.0));
        for (k, threads) in [(1u64, 3u32), (2, 0)] {
            let now = sel.now + DT * k as u32;
            sel.step(&Input {
                now,
                census: Census {
                    threads,
                    ..Census::default()
                },
                ..Input::default()
            });
        }
        assert_eq!(sel.threads, 3);
    }

    #[test]
    fn mixed_windows_are_counted() {
        let mut sel = ZSelect::new();
        drive(&mut sel, 2, |_| (0.5, 0.0));
        for (k, mixed) in [3u64, 4].into_iter().enumerate() {
            let now = sel.now + DT;
            sel.step(&Input {
                now,
                census: Census {
                    mixed,
                    ..Census::default()
                },
                ..Input::default()
            });
            assert_eq!(sel.counts().mixed_windows, [3, 7][k]);
        }
    }

    #[test]
    fn a_reading_with_any_field_not_a_number_is_ignored() {
        let good = Model {
            phi_raw: 0.5,
            phi_z: Some(0.0),
            phi_z_scan: Some(0.0),
            refault_rate: 0.0,
            budget: 1 << 30,
        };
        let bad = [
            Model {
                phi_raw: f64::NAN,
                ..good
            },
            Model {
                phi_z: Some(f64::NAN),
                ..good
            },
            Model {
                phi_z_scan: Some(f64::NAN),
                ..good
            },
            Model {
                phi_z_scan: Some(f64::INFINITY),
                ..good
            },
            Model {
                refault_rate: f64::NAN,
                ..good
            },
            Model {
                refault_rate: f64::INFINITY,
                ..good
            },
        ];
        for (k, m) in bad.into_iter().enumerate() {
            let mut sel = ZSelect::new();
            sel.step(&Input {
                now: Duration::from_secs(1),
                model: Some(m),
                ..Input::default()
            });
            assert_eq!(
                (sel.counts().ignored, sel.counts().s1_ticks),
                (1, 0),
                "case {k}"
            );
        }
        // A rate that is negative is a rate of nothing, as a model's is.
        let mut sel = ZSelect::new();
        let negative = Model {
            phi_z: Some(-1.0),
            phi_z_scan: Some(-1.0),
            refault_rate: -5.0,
            ..good
        };
        sel.step(&Input {
            now: Duration::from_secs(1),
            model: Some(negative),
            ..Input::default()
        });
        let m = sel.model.expect("a negative rate is a reading");
        assert_eq!(
            (m.phi_z, m.phi_z_scan, m.refault_rate),
            (Some(0.0), Some(0.0), 0.0)
        );
    }

    #[test]
    fn a_reading_that_is_not_a_number_is_not_a_tick_once_there_has_been_a_good_one() {
        // With no model yet a reading taken for a tick would find nothing to
        // read; after a good one it would count the good one twice.
        let hot = |phi_raw: f64| Model {
            phi_raw,
            phi_z: None,
            phi_z_scan: None,
            refault_rate: 0.0,
            budget: 870_000_000,
        };
        let mut sel = ZSelect::new();
        for (k, m) in [hot(0.5), hot(f64::NAN), hot(f64::NAN)]
            .into_iter()
            .enumerate()
        {
            let d = sel.step(&Input {
                now: Duration::from_secs(k as u64 + 1),
                model: Some(m),
                ..Input::default()
            });
            assert_eq!(
                d.phase,
                Phase::Fits,
                "tick {k}: one good reading is not two"
            );
        }
        assert_eq!((sel.counts().s1_ticks, sel.counts().ignored), (1, 2));
    }

    #[test]
    fn failed_builds_are_builds_and_not_supersessions() {
        // Four superseded, four that failed and one superseded: four of the
        // last eight, the most the budget allows. A failure is not a
        // supersession, and it is a build that ages one out of the window.
        let mut sel = ZSelect::new();
        for (k, build) in [
            Build {
                superseded: 4,
                ..Build::default()
            },
            Build {
                failed: 4,
                ..Build::default()
            },
            Build {
                superseded: 1,
                ..Build::default()
            },
        ]
        .into_iter()
        .enumerate()
        {
            sel.step(&Input {
                now: Duration::from_secs(k as u64 + 1),
                build,
                ..Input::default()
            });
        }
        assert_eq!(sel.counts().build_suspended, 0);
    }

    #[test]
    fn a_model_that_says_it_fits_holds_the_build_off_though_the_rent_is_paid() {
        // Clause (c) alone. The store is too slow to raise the pressure term
        // itself, the rent is paid and the census says the form saves, the
        // sampler has read and no switch has been made; the model, between
        // the way in and half of it, does not say the store may not fit, and
        // nothing is built.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let slow = Load::Rated {
            class: Class::Read,
            raw: Rate {
                ops_per_s: 700.0,
                phi: 0.3,
            },
            z: Rate {
                ops_per_s: 3_000.0,
                phi: 0.0,
            },
            refaults: true,
            bound,
        };
        let mut sim = Sim::new(cell.clone(), slow);
        sim.run_until(Phase::Suspect, 10.0);
        let sus = sim.sel.theta_sus(Class::Read);
        sim.phi_raw_override = Some(0.7 * sus);
        sim.run(3600.0);
        assert_eq!(sim.last.phase, Phase::Suspect);
        assert_eq!(sim.sel.counts().builds, 0);
        assert!(
            sim.log.iter().any(|r| r.excess >= cell.price),
            "the rent was paid"
        );
    }

    #[test]
    fn e2_without_a_footprint_reading_is_not_a_yes() {
        let mut sel = ZSelect::new();
        sel.model = Some(Model {
            phi_raw: 0.5,
            phi_z: Some(0.0),
            phi_z_scan: Some(0.0),
            refault_rate: 0.0,
            budget: 870_000_000,
        });
        sel.phase = Phase::Compact;
        // A measure in which the compact form faults nothing, over plenty of
        // reads.
        let quiet = Window {
            ops: 100_000,
            faults: 0,
        };
        sel.watch.e2_win[Class::Read.ix()] = quiet;
        sel.update_e2(Duration::from_secs(11));
        assert_eq!(sel.watch.e2_run, 0, "no reading of the footprint, no yes");
        sel.footprint = Some(1 << 20);
        sel.watch.e2_win[Class::Read.ix()] = quiet;
        sel.update_e2(Duration::from_secs(22));
        assert_eq!(sel.watch.e2_run, 1);
    }

    #[test]
    fn a_trial_is_wrong_only_when_the_raw_form_saves_the_entry_threshold_over_the_compact_forms_rate(
    ) {
        // At 588 MB the model predicts the compact form takes 0.3 a read. A
        // hot set the compact form serves without a fault is tried on the raw
        // form: at 0.4 a read the raw form saves a tenth over the predicted
        // 0.3, under the entry threshold, and the form is left; at 0.6 it
        // saves three tenths, over it, and the form is wanted.
        let cell = Cell {
            cache: 588_000_000,
            ..Cell::ten_million()
        };
        let z = model_z(cell.budget(), cell.resident, cell.z);
        let enter = ZSelect::new().theta_enter(Class::Read);
        assert!((z - 0.3).abs() < 0.01, "{z}");
        assert!(0.4 - z < enter && enter < 0.6 - z);
        for (plateau, wrong) in [(0.4, false), (0.6, true)] {
            let mut sim = compact(&cell);
            sim.load = Load::Hot {
                raw_pages: 9_000.0,
                z_pages: 5_000.0,
                plateau,
            };
            sim.run_until(Phase::Trial, 2400.0);
            assert_eq!(sim.last.phase, Phase::Trial, "{plateau}");
            sim.run(120.0);
            let c = sim.sel.counts();
            assert_eq!(
                (c.trials_back, c.exits[Exit::E2 as usize]),
                if wrong { (1, 0) } else { (0, 1) },
                "{plateau}"
            );
        }
    }

    // ---- the controller's own output moves what it reads ---------------------------
    //
    // The group's refault counter holds the refaults of the store itself, and
    // which form serves decides how many there are. A rule that reads it is
    // reading its own answer, so the replay feeds it as the group has it, and
    // a controller facing constant inputs has to come to rest.

    #[test]
    fn the_replays_pressure_term_is_the_groups_counter_the_stores_own_refaults_included() {
        let cell = Cell {
            cache: 4 << 30,
            neighbours: 100.0,
            ..Cell::ten_million()
        };
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let raw = cell.rate(cell.t_hit, 0.3);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.3, 0.0, bound));
        sim.run(3.0);
        let read = sim.sel.model.expect("a model reading").refault_rate;
        assert!(
            (read - (100.0 + 0.3 * raw.ops_per_s)).abs() < 0.01 * read,
            "{read}"
        );
        // Served by the compact form, which faults nothing, only the
        // neighbour's are left.
        sim.run_until(Phase::Compact, 120.0);
        sim.run(3.0);
        assert_eq!(sim.sel.model.expect("a model reading").refault_rate, 100.0);
    }

    /// The loads of the fixed-point checks: the geometry's reads and scans,
    /// two hot sets, and reads at fixed rates through each form.
    fn constant_loads(cell: &Cell) -> Vec<(String, Load)> {
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut loads: Vec<(String, Load)> = vec![
            ("uniform reads".into(), cell.uniform()),
            ("uniform scans".into(), cell.scans()),
            ("a hot set of 100 MB".into(), cell.hot(100)),
            ("a hot set of 300 MB".into(), cell.hot(300)),
        ];
        for raw in [0.05, 0.1, 0.3, 0.6] {
            for z in [0.0, 0.05, 0.2, 0.4, 0.7] {
                loads.push((
                    format!("reads at {raw} faults raw and {z} compact"),
                    cell.fixed(raw, z, bound),
                ));
            }
        }
        loads
    }

    /// A controller facing constant inputs reaches a fixed point: no switch in
    /// the last two of three hours, whatever the cache, the neighbours and the
    /// threads, and where the numbers pick a place it rests in it -- a form
    /// that saves the entry threshold is kept, one that loses is not.
    fn constant_workloads_come_to_rest(cache: u64) {
        let base = Cell::ten_million();
        let sel = ZSelect::new();
        let (mut kept, mut suspect) = (0, 0);
        for neighbours in [0.0, 400.0] {
            for threads in [1u32, 3] {
                let cell = Cell {
                    cache,
                    neighbours,
                    threads,
                    ..base.clone()
                };
                for (name, load) in constant_loads(&cell) {
                    let mut sim = Sim::new(cell.clone(), load);
                    sim.run(3600.0);
                    let before = sim.sel.counts().switches;
                    sim.run(2.0 * 3600.0);
                    let at = format!(
                        "{} MB, {neighbours} refaults a second beside it, {threads} threads, {name}",
                        cache / 1_000_000
                    );
                    let late = sim.sel.counts().switches - before;
                    assert_eq!(late, 0, "switches after the first hour, {at}");
                    let held = matches!(sim.last.phase, Phase::Compact | Phase::Trial);
                    if let Load::Rated { class, raw, z, .. } = load {
                        let saving = raw.phi - z.phi;
                        if saving >= sel.theta_enter(class) {
                            assert!(held, "a saving of {saving} is kept, {at}");
                        }
                        if saving < 0.0 {
                            assert!(!held, "a loss of {} is not, {at}", -saving);
                        }
                    }
                    kept += usize::from(held);
                    suspect += usize::from(sim.last.phase == Phase::Suspect);
                }
            }
        }
        assert!(kept > 0 && suspect > 0, "{kept} kept, {suspect} suspect");
    }

    #[test]
    fn constant_workloads_come_to_rest_at_700_mb() {
        constant_workloads_come_to_rest(700_000_000);
    }

    #[test]
    fn constant_workloads_come_to_rest_at_937_mb() {
        constant_workloads_come_to_rest(937_000_000);
    }

    #[test]
    fn constant_workloads_come_to_rest_at_1300_mb() {
        constant_workloads_come_to_rest(1_300_000_000);
    }

    #[test]
    fn constant_workloads_come_to_rest_at_2_gb() {
        constant_workloads_come_to_rest(2_000_000_000);
    }

    #[test]
    fn constant_workloads_come_to_rest_at_4_gib() {
        constant_workloads_come_to_rest(4 << 30);
    }

    #[test]
    fn a_form_bought_against_the_models_word_stays_while_it_pays() {
        // The cache is large, the model says the store fits, and the raw form
        // faults at 0.3 a read (a reader of the group the model cannot see)
        // where the compact form faults at none. The store's own refaults
        // open the clause, the census buys, and the compact form's quiet takes
        // the refaults away. The model still says the store fits, and its
        // word is what was contradicted: the form is not left on it, nor on
        // the pressure it no longer sees. The raw form is tried when a trial
        // is admitted and found faulting, and each wrong trial doubles the
        // wait.
        let big = Cell {
            cache: 4 << 30,
            ..Cell::ten_million()
        };
        let bound = big.bound(big.z as f64 / PAGE as f64);
        let mut sim = Sim::new(big.clone(), big.fixed(0.3, 0.0, bound));
        sim.run(3.0 * 3600.0);
        let c = sim.sel.counts();
        assert_eq!((c.builds, c.installs, c.switches), (1, 1, 1));
        assert_eq!(c.exits, [0, 0, 0]);
        assert!(c.trials >= 3, "{} trials", c.trials);
        assert!(c.trials_back + 1 >= c.trials);
        assert!((sim.sel.bias - 0.3).abs() < 0.01, "{}", sim.sel.bias);
        let served = sim.log.iter().filter(|r| r.d.gate == GATE_Z).count();
        assert!(served as f64 > 0.95 * sim.log.len() as f64, "{served}");
    }

    #[test]
    fn a_form_that_faults_more_than_the_raw_form_is_left_though_its_faults_raise_the_pressure() {
        // The compact form takes a fault a read where the raw form took 0.45,
        // because a raw path beside it enlarged the union of pages the two
        // forms need. Its own refaults are in the group's counter, so the
        // pressure term is on through the whole of the stretch; the exit is
        // for a form that is not paying, and does not wait for the pressure
        // to fall.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.45, 1.0, bound));
        sim.run_until(Phase::Compact, 120.0);
        let install = sim.first(Event::Install);
        let dwell = sim.sel.dwell();
        let mut pressed = 0;
        sim.run_with(dwell.as_secs_f64() + 10.0, |s| {
            pressed += usize::from(s.sel.pressure());
        });
        assert!(pressed > 1000, "{pressed} steps under pressure");
        let exit = sim.first(Event::Exit(Exit::E3));
        near(exit - install, dwell.as_secs_f64(), 2.0);
        let kept = sim.sel.z_realized.expect("the rate is kept");
        assert!((kept - 1.0).abs() < 0.01, "{kept}");
        // And it stays left: the form faults more than the raw form does.
        sim.run(3.0 * 3600.0);
        assert_eq!(sim.sel.counts().installs, 1);
        assert_eq!(sim.last.phase, Phase::Suspect);
    }

    #[test]
    fn a_trial_that_finds_the_raw_form_faulting_contradicts_a_model_that_said_it_fits() {
        // The form was bought with the model's agreement. The limit is lifted,
        // the model says the store fits, and the raw form, which a trial is
        // admitted to try because what the compact form touches is small,
        // faults at 0.3 all the same. The model is contradicted by that, and
        // its word, which would have taken the form back a dwell after the
        // trial, no longer does.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        assert!(sim.sel.bias <= 0.0, "{}", sim.sel.bias);
        sim.cell.cache = 4 << 30;
        sim.load = Load::Hot {
            raw_pages: 24_000.0,
            z_pages: 13_000.0,
            plateau: 0.3,
        };
        sim.run_until(Phase::Trial, 1200.0);
        sim.run_until(Phase::Compact, 120.0);
        assert_eq!(sim.sel.counts().trials_back, 1);
        assert!((sim.sel.bias - 0.3).abs() < 0.02, "{}", sim.sel.bias);
        sim.run(2.0 * sim.sel.dwell().as_secs_f64() + E1_SMOOTHING.as_secs_f64());
        assert_eq!(sim.sel.counts().exits, [0, 0, 0]);
        assert_eq!(sim.last.phase, Phase::Compact);
    }

    #[test]
    fn a_trial_that_straddles_the_end_of_e1s_stretch_still_disarms_it() {
        // E1 acts a dwell after the model began to say the store fits. The
        // trial starts inside that dwell and ends after it, and finds the raw
        // form faulting at 0.3: too slow a store to raise the pressure term
        // itself, so nothing else has broken the stretch. The model is
        // contradicted by the trial, and the stretch it began is no reason to
        // leave the form.
        let cell = Cell::ten_million();
        let mut sim = compact(&cell);
        let dwell = sim.sel.dwell();
        sim.run(dwell.as_secs_f64() + 100.0);
        sim.cell.cache = 4 << 30;
        sim.load = Load::Idle;
        while sim.sel.low_since.is_none() {
            sim.step();
            assert!(sim.now < Duration::from_secs(10_000));
        }
        let began = sim.sel.low_since.expect("the stretch began");
        let slow = Load::Rated {
            class: Class::Read,
            raw: Rate {
                ops_per_s: 700.0,
                phi: 0.3,
            },
            z: Rate {
                ops_per_s: 3_000.0,
                phi: 0.0,
            },
            refaults: true,
            bound: 700_000_000,
        };
        // The compact form's quiet takes three measures of ten seconds to
        // admit the trial: it arrives a little under a dwell into the stretch.
        let arrives = began + dwell - Duration::from_secs(35);
        while sim.now < arrives {
            sim.step();
        }
        sim.load = slow;
        sim.run_until(Phase::Trial, 60.0);
        assert_eq!(sim.last.phase, Phase::Trial);
        assert!(sim.now + 3 * S1_PERIOD < began + dwell, "inside the dwell");
        sim.run_until(Phase::Compact, 60.0);
        assert!(sim.now > began + dwell, "and ending after it");
        sim.run(30.0);
        assert_eq!(sim.sel.counts().exits, [0, 0, 0]);
        assert!((sim.sel.bias - 0.3).abs() < 0.02, "{}", sim.sel.bias);
    }

    #[test]
    fn the_models_word_that_the_store_fits_leaves_a_form_it_backed_and_not_one_it_was_contradicted_about(
    ) {
        // The same quiet, the same model and the same hours: a form the model
        // agreed to buy is left a dwell after the model says the store fits,
        // and one it was contradicted about is not.
        let backed = Cell::ten_million();
        let mut sim = compact(&backed);
        assert!(sim.sel.bias <= 0.0, "{}", sim.sel.bias);
        sim.cell.cache = 4 << 30;
        sim.load = Load::Idle;
        sim.run(3.0 * 3600.0);
        assert_eq!(sim.sel.counts().exits, [1, 0, 0]);
        // The model said fits and a neighbour's refaults and the store's own
        // said otherwise.
        let big = Cell {
            cache: 4 << 30,
            neighbours: 400.0,
            ..Cell::ten_million()
        };
        let heavy = big.budget();
        let mut sim = Sim::new(big.clone(), big.fixed(0.3, 0.0, heavy));
        sim.run_until(Phase::Compact, 120.0);
        assert!((sim.sel.bias - 0.3).abs() < 0.01, "{}", sim.sel.bias);
        sim.cell.neighbours = 0.0;
        sim.load = Load::Idle;
        sim.run(3.0 * 3600.0);
        assert_eq!(sim.sel.counts().exits, [0, 0, 0]);
        assert_eq!(sim.last.phase, Phase::Compact);
    }

    #[test]
    fn scans_that_fault_where_the_model_says_the_store_fits_contradict_it() {
        // The model has no rate for a scan, only its word that the store fits.
        // A scan that faults at the scan break-even's half against that word
        // contradicts it, and one against a model that says the store does not
        // fit contradicts nothing.
        let fits = Cell {
            cache: 2_000_000_000,
            ..Cell::ten_million()
        };
        let mut sim = Sim::new(fits.clone(), fits.scans());
        sim.run_until(Phase::Compact, 120.0);
        assert!(sim.sel.scans_against);
        sim.run(3.0 * 3600.0);
        assert_eq!(sim.sel.counts().exits, [0, 0, 0]);
        let tight = Cell::ten_million();
        let mut sim = Sim::new(tight.clone(), tight.scans());
        sim.run_until(Phase::Compact, 120.0);
        assert!(!sim.sel.scans_against);
        // Lift the limit with nothing reading: the model's word is believed.
        sim.cell.cache = 4 << 30;
        sim.load = Load::Idle;
        sim.run(3.0 * 3600.0);
        assert_eq!(sim.sel.counts().exits, [1, 0, 0]);
    }

    #[test]
    fn the_models_error_is_kept_from_the_smallest_rate_worth_counting_in_either_direction() {
        // The census saw the raw form take 0.3 a read. A model that said a
        // little more or less than that, by under the least rate worth
        // counting, was right to within what is worth acting on, and one that
        // said more or less by over it was not.
        let cell = Cell::ten_million();
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let sus = ZSelect::new().theta_sus(Class::Read);
        for (said, kept) in [
            (0.3 - 0.9 * sus, false),
            (0.3 - 1.1 * sus, true),
            (0.3 + 0.9 * sus, false),
            (0.3 + 1.1 * sus, true),
        ] {
            let mut sim = Sim::new(cell.clone(), cell.fixed(0.3, 0.0, bound));
            sim.phi_raw_override = Some(said);
            sim.run_until(Phase::Compact, 120.0);
            let want = if kept { 0.3 - said } else { 0.0 };
            assert!(
                (sim.sel.bias - want).abs() < 0.005,
                "{said}: {}",
                sim.sel.bias
            );
        }
    }

    #[test]
    fn a_model_that_over_predicts_does_not_keep_a_form_the_raw_form_beat() {
        // The cache is small and the model says the raw form takes 0.75 a
        // read; the workload is skewed, and the census saw it take 0.3. The
        // compact form takes 0.4, which is worse, and 0.35 under the model's
        // word, which would call it paying. The model's error is carried, so
        // the form is judged against the 0.3 and left; at a cache where the
        // model's word is close to the census's the same form is left too.
        let cell = Cell {
            cache: 700_000_000,
            ..Cell::ten_million()
        };
        let bound = cell.bound(cell.z as f64 / PAGE as f64);
        let model = model_raw(cell.budget(), cell.hash, cell.records);
        assert!(model > 0.7 && model < 0.8, "{model}");
        let mut sim = Sim::new(cell.clone(), cell.fixed(0.3, 0.4, bound));
        sim.run_until(Phase::Compact, 120.0);
        assert!(
            (sim.sel.bias - (0.3 - model)).abs() < 0.01,
            "{}",
            sim.sel.bias
        );
        sim.run(2.0 * sim.sel.dwell().as_secs_f64());
        assert_eq!(sim.sel.counts().exits, [0, 0, 1]);
        assert_eq!(sim.last.phase, Phase::Suspect);
    }
}
