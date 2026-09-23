//! The gate: is this row worse than its own history?
//!
//! For each (class, workload, arm, size, quantity), take the last `WINDOW`
//! rows at the same scale in `runs/` for that class. The new row regresses
//! if its CI lies entirely on the worse side of every one of those rows'
//! CIs. A row better than every prior CI is flagged, not failed: it is
//! either a win or a broken measurement, and a person should know which.
//! Fewer than `MIN_HISTORY` prior rows: no band, and the gate says so.
//!
//! One thing stands between that rule and a verdict: the machine. A row
//! is compared to rows of its class, and the class is the CPU model, the
//! core count, the memory and virtualisation -- which does not pin the
//! host a guest lands on. So every row measures the machine with no
//! engine in the way, in the floors, and the floors are this gate's
//! control: when a floor is below every row in the window, the quantities
//! that depend on the machine get no verdict rather than a regression,
//! and the ones that do not -- the byte ratios, which are arithmetic on
//! what was stored -- are judged as always. A quick row here failed with
//! 121 of 1,106 quantities regressed while LMDB's own scan, code no
//! engine change can touch, ran a third of its window: that row cost a
//! day to read and the reading is what this control is.
//!
//! That is the whole rule. The window is the only parameter and it is
//! stated once, in DESIGN.md; this is the code for it.

use crate::row::Row;
use crate::stats::{Samples, CI_CONF, CI_RESAMPLES};
use std::collections::HashMap;
use std::io;
use std::path::Path;

pub const WINDOW: usize = 10;
pub const MIN_HISTORY: usize = 3;

/// Which way is worse. Every quantity a workload records is named here;
/// one that is not is an error, never a guess. The threaded read and scan
/// quantities are one per count in `run::THREADS`, and a count added
/// there is named here or `every_threaded_quantity_is_named` fails --
/// before a run has paid for the measurement the gate would refuse.
pub fn higher_is_better(quantity: &str) -> Option<bool> {
    Some(match quantity {
        "ops_per_s" | "reads_per_s" | "entries_per_s" | "bytes_per_s" => true,
        "reads_per_s_2t" | "reads_per_s_4t" | "entries_per_s_2t" | "entries_per_s_4t" => true,
        "entries_per_s_lag0pct" | "entries_per_s_lag1pct" => true,
        "entries_per_s_lag10pct" | "entries_per_s_lag100pct" => true,
        "p99_us" | "device_bytes_per_byte" | "bytes_on_disk_per_byte" => false,
        _ => return None,
    })
}

/// The workloads that measure the machine rather than an engine: the
/// device's sync rate, the mapped sequential read, and the random-access
/// chase. Named here, never guessed, so a floor added in `run` is added
/// here or it is not a control.
pub fn is_floor(workload: &str) -> bool {
    matches!(workload, "wal-floor" | "scan-floor" | "mem-floor")
}

/// The arms an engine change cannot touch: the comparators a user would
/// otherwise pick. They are measured in the same process as the arms,
/// interleaved within a rep, over the same data -- so a comparator below
/// its window is this row's own statement that the machine moved, exact
/// and needing no statistic. Named, never inferred from the name, and
/// `every_arm_is_named_as_engine_or_comparator` fails if an arm is added
/// to `engines::ARMS` without a side.
pub fn is_comparator(arm: &str) -> bool {
    matches!(
        arm,
        "lmdb" | "lmdb-nosync" | "rocksdb-tuned" | "rocksdb-nosync"
    )
}

/// Whether a quantity moves with the machine. The byte ratios are
/// arithmetic on what the engine stored -- they came back identical to
/// three decimals across hosts that moved every rate by half -- so they
/// are judged whatever the floors say. Everything else is a rate or a
/// latency.
pub fn machine_dependent(quantity: &str) -> bool {
    !matches!(quantity, "device_bytes_per_byte" | "bytes_on_disk_per_byte")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Entirely on the worse side of every prior CI.
    Regressed,
    /// Worse than every prior CI, on a machine whose floors are too: the
    /// row cannot say whether the change or the host did it.
    NoVerdict,
    /// Entirely on the better side of every prior CI.
    Flagged,
    Within,
    /// Fewer than `MIN_HISTORY` prior rows carry this quantity.
    InsufficientHistory(usize),
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub workload: String,
    pub arm: String,
    pub size: Option<u64>,
    pub quantity: String,
    pub unit: String,
    pub verdict: Verdict,
    /// The new row's CI of the median.
    pub ci: (f64, f64),
    /// The new row's median, and the envelope of the prior rows' medians.
    /// A floor is judged on these rather than on the CIs: see `gate`.
    pub median: f64,
    pub prior_medians: Option<(f64, f64)>,
    /// The envelope of the prior CIs: (lowest lo, highest hi).
    pub prior: Option<(f64, f64)>,
    pub prior_rows: usize,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub class: String,
    pub scale: &'static str,
    /// Prior rows found for this class and scale, after the window.
    pub prior_rows: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    /// A floor or a comparator below its window is the machine, never the
    /// engine, so both are reported and neither is failed on.
    pub fn regressed(&self) -> bool {
        self.findings.iter().any(|f| {
            f.verdict == Verdict::Regressed && !is_floor(&f.workload) && !is_comparator(&f.arm)
        })
    }

    /// The floors that came in below every row in the window.
    pub fn host_out_of_band(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| is_floor(&f.workload) && f.verdict == Verdict::Regressed)
            .collect()
    }

    /// The comparators that came in below their window: what an engine
    /// change cannot have done, so the machine did it.
    pub fn comparators_low(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| is_comparator(&f.arm) && f.verdict == Verdict::Regressed)
            .collect()
    }

    fn count(&self, pred: impl Fn(&Verdict) -> bool) -> usize {
        self.findings.iter().filter(|f| pred(&f.verdict)).count()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "gate: class {} scale {} -- {} prior row{} (window {WINDOW})\n",
            self.class,
            self.scale,
            self.prior_rows,
            if self.prior_rows == 1 { "" } else { "s" },
        ));
        let floors: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| is_floor(&f.workload))
            .collect();
        let banded = floors
            .iter()
            .filter(|f| !matches!(f.verdict, Verdict::InsufficientHistory(_)))
            .count();
        let low = self.host_out_of_band();
        if banded == 0 {
            out.push_str(&format!(
                "host: no floor in band yet ({} floor quantities); the verdict stands on the engine's alone\n",
                floors.len()
            ));
        } else if low.is_empty() {
            let above = floors
                .iter()
                .filter(|f| f.verdict == Verdict::Flagged)
                .count();
            out.push_str(&format!(
                "host: floors within the window ({banded} of {}){}\n",
                floors.len(),
                if above > 0 {
                    format!(", {above} above it -- a quantity flagged better may be the machine")
                } else {
                    String::new()
                }
            ));
        } else {
            let which: Vec<String> = low
                .iter()
                .map(|f| {
                    let (lo, hi) = f.prior_medians.unwrap_or((f64::NAN, f64::NAN));
                    format!(
                        "{} {} median {} against the window's medians [{}, {}]",
                        f.workload,
                        f.quantity,
                        fmt(f.median),
                        fmt(lo),
                        fmt(hi)
                    )
                })
                .collect();
            out.push_str(&format!(
                "host: OUT OF BAND -- {}. Every quantity that moves with the machine gets no verdict.\n",
                which.join("; ")
            ));
        }
        let comparators = self.comparators_low();
        if comparators.is_empty() {
            out.push_str("comparators: none below their window\n");
        } else {
            let mut where_: Vec<String> = comparators
                .iter()
                .map(|f| format!("{} {} {}", f.arm, f.workload, f.quantity))
                .collect();
            where_.sort();
            where_.dedup();
            let shown = where_.len().min(6);
            out.push_str(&format!(
                "comparators: {} below their window ({}{}); an arm's regression in those workloads gets no verdict\n",
                comparators.len(),
                where_[..shown].join(", "),
                if where_.len() > shown { ", ..." } else { "" }
            ));
        }
        for f in &self.findings {
            let (tag, note) = match &f.verdict {
                Verdict::Regressed => ("REGRESSED", String::new()),
                Verdict::NoVerdict => (
                    "no verdict",
                    " (worse than the window, and so are the floors)".into(),
                ),
                Verdict::Flagged => (
                    "flagged",
                    " (better than every prior row -- a win or a broken measurement)".into(),
                ),
                Verdict::Within | Verdict::InsufficientHistory(_) => continue,
            };
            let prior = f
                .prior
                .map(|(lo, hi)| format!("prior CIs span [{}, {}]", fmt(lo), fmt(hi)))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {tag:<9} {:<14} {:<15} {:>9}  {:<22} [{}, {}] {}; {prior}{note}\n",
                f.workload,
                f.arm,
                f.size.map(|s| s.to_string()).unwrap_or_default(),
                f.quantity,
                fmt(f.ci.0),
                fmt(f.ci.1),
                f.unit,
            ));
        }
        let total = self.findings.len();
        let insufficient = self.count(|v| matches!(v, Verdict::InsufficientHistory(_)));
        // The engine's own, which is what a verdict is about: a floor or a
        // comparator below its window is the machine's and is reported
        // above.
        let regressed = self
            .findings
            .iter()
            .filter(|f| {
                f.verdict == Verdict::Regressed && !is_floor(&f.workload) && !is_comparator(&f.arm)
            })
            .count();
        let no_verdict = self.count(|v| *v == Verdict::NoVerdict);
        let flagged = self.count(|v| *v == Verdict::Flagged);
        if insufficient == total {
            out.push_str(&format!(
                "  no band yet: fewer than {MIN_HISTORY} prior rows for every quantity ({total} quantities)\n"
            ));
        } else if insufficient > 0 {
            out.push_str(&format!(
                "  no band yet for {insufficient} of {total} quantities (fewer than {MIN_HISTORY} prior rows)\n"
            ));
        }
        let withheld = if no_verdict > 0 {
            format!("; {no_verdict} withheld from the machine")
        } else {
            String::new()
        };
        out.push_str(&if regressed > 0 {
            format!("REGRESSED: {regressed} of {total} quantities are worse than every row in the window{withheld}\n")
        } else if no_verdict > 0 {
            format!(
                "no verdict: {no_verdict} of {total} quantities are worse than the window and so is the machine under them; nothing the engine's own ({flagged} flagged)\n"
            )
        } else {
            format!("ok: nothing worse than the window ({flagged} flagged{withheld})\n")
        });
        out
    }
}

fn fmt(v: f64) -> String {
    if v.abs() >= 1000.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Every row under `runs/<scale>/` in the same class as `row`, except `row`
/// itself, newest last. A missing directory is zero rows, not an error: the
/// first run on a machine has no history and should say so.
pub fn history(row: &Row, runs: &Path) -> io::Result<Vec<Row>> {
    let dir = runs.join(row.scale.as_str());
    let mut rows = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(rows);
    };
    let class = row.class();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let r = Row::read(&p)?;
        if r.class() == class && !(r.utc == row.utc && r.sha == row.sha) {
            rows.push(r);
        }
    }
    rows.sort_by(|a, b| a.utc.cmp(&b.utc));
    Ok(rows)
}

pub fn gate(row: &Row, runs: &Path) -> io::Result<Report> {
    let all = history(row, runs)?;
    let window: Vec<&Row> = all.iter().rev().take(WINDOW).collect();

    // Prior CIs and medians by key.
    type Key = (String, String, Option<u64>, String);
    let mut prior: HashMap<Key, Vec<(f64, f64)>> = HashMap::new();
    let mut prior_med: HashMap<Key, Vec<f64>> = HashMap::new();
    for r in &window {
        for m in &r.measurements {
            let k = (
                m.workload.clone(),
                m.arm.clone(),
                m.size,
                m.quantity.clone(),
            );
            let s = Samples::new(m.samples.clone());
            prior
                .entry(k.clone())
                .or_default()
                .push(s.median_ci(CI_CONF, CI_RESAMPLES));
            prior_med.entry(k).or_default().push(s.median());
        }
    }

    let mut findings = Vec::with_capacity(row.measurements.len());
    for m in &row.measurements {
        let Some(up) = higher_is_better(&m.quantity) else {
            return Err(io::Error::other(format!(
                "quantity {:?} has no recorded direction; add it to gate::higher_is_better",
                m.quantity
            )));
        };
        let samples = Samples::new(m.samples.clone());
        let ci = samples.median_ci(CI_CONF, CI_RESAMPLES);
        let med = samples.median();
        let k = (
            m.workload.clone(),
            m.arm.clone(),
            m.size,
            m.quantity.clone(),
        );
        let priors = prior.get(&k).map(Vec::as_slice).unwrap_or(&[]);
        let meds = prior_med.get(&k).map(Vec::as_slice).unwrap_or(&[]);
        let med_envelope = (meds.len() >= MIN_HISTORY).then(|| {
            (
                meds.iter().copied().fold(f64::INFINITY, f64::min),
                meds.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            )
        });
        let (verdict, envelope) = if priors.len() < MIN_HISTORY {
            (Verdict::InsufficientHistory(priors.len()), None)
        } else if is_floor(&m.workload) {
            // A floor is the control, and a control is judged on its
            // median against every prior median rather than on disjoint
            // CIs. The two rules answer different questions and the
            // asymmetry is deliberate: a regression claim costs a day of
            // investigation when it is wrong, so it must survive the
            // noise; a control costs a rerun, so it must survive a bad
            // machine. The CI rule proved useless here -- a floor's
            // samples range over a factor of two within one run, so its
            // CI overlaps the window even on a host where LMDB's own scan
            // ran at a third of its band.
            let (lo, hi) = med_envelope.expect("history counted above");
            let worse = if up { med < lo } else { med > hi };
            let better = if up { med > hi } else { med < lo };
            let v = if worse {
                Verdict::Regressed
            } else if better {
                Verdict::Flagged
            } else {
                Verdict::Within
            };
            (v, Some((lo, hi)))
        } else {
            let lo_min = priors.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
            let hi_max = priors.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max);
            // Worse than every prior CI: no overlap with any of them, on the
            // worse side. Better than every prior CI: the mirror.
            let worse = if up { ci.1 < lo_min } else { ci.0 > hi_max };
            let better = if up { ci.0 > hi_max } else { ci.1 < lo_min };
            let v = if worse {
                Verdict::Regressed
            } else if better {
                Verdict::Flagged
            } else {
                Verdict::Within
            };
            (v, Some((lo_min, hi_max)))
        };
        findings.push(Finding {
            workload: m.workload.clone(),
            arm: m.arm.clone(),
            size: m.size,
            quantity: m.quantity.clone(),
            unit: m.unit.clone(),
            verdict,
            ci,
            median: med,
            prior_medians: med_envelope,
            prior: envelope,
            prior_rows: priors.len(),
        });
    }

    // The machine's verdict, taken before the engine's, from two
    // controls the row carries. The comparators are the exact one: a
    // quantity where an arm no engine change can touch came in below its
    // own window is a quantity this machine is slower at, so an arm's
    // regression there says nothing about the arm. The floors are the
    // coarse one, for a machine slow in a way no comparator happened to
    // show. Either way the byte ratios keep their verdict: they are
    // arithmetic on what was stored.
    let shadowed: std::collections::HashSet<(String, String)> = findings
        .iter()
        .filter(|f| is_comparator(&f.arm) && f.verdict == Verdict::Regressed)
        .map(|f| (f.workload.clone(), f.quantity.clone()))
        .collect();
    let host_low = findings
        .iter()
        .any(|f| is_floor(&f.workload) && f.verdict == Verdict::Regressed);
    for f in &mut findings {
        if f.verdict != Verdict::Regressed
            || is_floor(&f.workload)
            || is_comparator(&f.arm)
            || !machine_dependent(&f.quantity)
        {
            continue;
        }
        if host_low || shadowed.contains(&(f.workload.clone(), f.quantity.clone())) {
            f.verdict = Verdict::NoVerdict;
        }
    }

    Ok(Report {
        class: row.class(),
        scale: row.scale.as_str(),
        prior_rows: window.len(),
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row::{Guarantee, MachineInfo, Measurement};
    use crate::Scale;

    fn machine(cpus: usize) -> MachineInfo {
        MachineInfo {
            arch: "x86_64".into(),
            cpu_model: "Test CPU".into(),
            cpus,
            mem_total_kb: 16_000_000,
            page_size: 4096,
            cache_line: 64,
            cache_line_detected: true,
            l1d: 0,
            l2: 0,
            l3: 0,
            kernel: "k".into(),
            governor: "unknown".into(),
            thp: "never".into(),
            smt_on: false,
            pmu_available: false,
            aslr_disabled: false,
            virtualised: "none".into(),
        }
    }

    fn row(utc: &str, cpus: usize, reads: [f64; 5], p99: [f64; 5]) -> Row {
        let m = |q: &str, unit: &str, s: [f64; 5]| Measurement {
            workload: "read".into(),
            arm: "supdb".into(),
            guarantee: Guarantee::Durable,
            size: Some(10_000),
            quantity: q.into(),
            unit: unit.into(),
            samples: s.to_vec(),
        };
        Row {
            utc: utc.into(),
            sha: format!("sha-{utc}"),
            rustc: "r".into(),
            scale: Scale::Quick,
            machine: machine(cpus),
            measurements: vec![m("reads_per_s", "reads/s", reads), m("p99_us", "µs", p99)],
        }
    }

    fn fixture(n_prior: usize) -> (std::path::PathBuf, Vec<Row>) {
        // One directory per call. Keying it on the prior count and the
        // second was not enough: two tests with the same count ran in the
        // same second on an arm runner, and one removed the directory the
        // other was reading rows from.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "supdb-bench-gate-{}-{n_prior}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut priors = Vec::new();
        for i in 0..n_prior {
            let jitter = (i % 3) as f64;
            let r = row(
                &format!("20260101T{:02}0000Z", i),
                4,
                [100.0 + jitter, 101.0, 99.0 + jitter, 100.5, 100.0],
                [5.0, 5.2, 4.9 + jitter * 0.1, 5.1, 5.0],
            );
            r.write(&dir).unwrap();
            priors.push(r);
        }
        (dir, priors)
    }

    fn floor(workload: &str, quantity: &str, unit: &str, v: f64) -> Measurement {
        Measurement {
            workload: workload.into(),
            arm: crate::run::FLOOR_ARM.into(),
            guarantee: Guarantee::Buffered,
            size: None,
            quantity: quantity.into(),
            unit: unit.into(),
            samples: vec![v, v * 1.01, v * 0.99, v, v * 1.005],
        }
    }

    /// The three floors and a byte ratio on a row: what the gate's control
    /// reads, and one quantity the machine cannot move.
    fn with_control(mut r: Row, wal: f64, scan: f64, mem: f64, ratio: f64) -> Row {
        r.measurements
            .push(floor("wal-floor", "ops_per_s", "ops/s", wal));
        r.measurements
            .push(floor("scan-floor", "bytes_per_s", "B/s", scan));
        r.measurements
            .push(floor("mem-floor", "ops_per_s", "chases/s", mem));
        r.measurements.push(Measurement {
            workload: "load".into(),
            arm: "supdb".into(),
            guarantee: Guarantee::Durable,
            size: Some(10_000),
            quantity: "bytes_on_disk_per_byte".into(),
            unit: "B/B".into(),
            samples: vec![ratio; 5],
        });
        r
    }

    fn fixture_with_control(n_prior: usize) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "supdb-bench-gate-ctl-{}-{n_prior}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for i in 0..n_prior {
            let jitter = (i % 3) as f64 * 0.5;
            let r = row(
                &format!("2026010{}T000000Z", i + 1),
                4,
                [100.0 + jitter, 101.0, 99.0 + jitter, 100.5, 100.0],
                [5.0, 5.2, 4.9 + jitter * 0.1, 5.1, 5.0],
            );
            with_control(r, 1_000_000.0, 9.0e9, 3_000_000.0, 1.44)
                .write(&dir)
                .unwrap();
        }
        dir
    }

    /// The row that cost a day: every rate below the window, and the
    /// machine's own floors below it too.
    /// The exact control: a comparator below its window in the same
    /// workload and quantity is this machine, so an arm's regression
    /// there says nothing about the arm -- while an arm's regression
    /// where no comparator moved is still the arm's.
    #[test]
    fn a_comparator_below_its_window_withholds_the_arms_verdict_there() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "supdb-bench-gate-cmp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Two workloads, an arm and a comparator in each, and floors that
        // stay in band so this test is about the comparators alone.
        let build = |utc: &str, scan_supdb: f64, scan_lmdb: f64, read_supdb: f64| {
            let q = |wl: &str, arm: &str, quantity: &str, unit: &str, v: f64| Measurement {
                workload: wl.into(),
                arm: arm.into(),
                guarantee: Guarantee::Durable,
                size: Some(10_000),
                quantity: quantity.into(),
                unit: unit.into(),
                samples: vec![v, v * 1.01, v * 0.99, v, v * 1.005],
            };
            let mut r = Row {
                utc: utc.into(),
                sha: format!("sha-{utc}"),
                rustc: "r".into(),
                scale: Scale::Quick,
                machine: machine(4),
                measurements: vec![
                    q("scan", "supdb", "entries_per_s", "entries/s", scan_supdb),
                    q("scan", "lmdb", "entries_per_s", "entries/s", scan_lmdb),
                    q("read", "supdb", "reads_per_s", "reads/s", read_supdb),
                    q("read", "lmdb", "reads_per_s", "reads/s", 50.0),
                ],
            };
            r.measurements
                .push(floor("wal-floor", "ops_per_s", "ops/s", 1_000_000.0));
            r.measurements
                .push(floor("scan-floor", "bytes_per_s", "B/s", 9.0e9));
            r.measurements
                .push(floor("mem-floor", "ops_per_s", "chases/s", 3_000_000.0));
            r
        };
        for i in 0..6 {
            build(&format!("2026010{}T000000Z", i + 1), 100.0, 80.0, 40.0)
                .write(&dir)
                .unwrap();
        }
        // The scans fall for both, the reads for the arm alone.
        let new = build("20260201T000000Z", 60.0, 50.0, 25.0);
        let rep = gate(&new, &dir).unwrap();
        let text = rep.render();
        let find = |wl: &str, arm: &str| {
            rep.findings
                .iter()
                .find(|f| f.workload == wl && f.arm == arm)
                .unwrap_or_else(|| panic!("{wl}/{arm} is a finding:\n{text}"))
        };
        assert_eq!(find("scan", "supdb").verdict, Verdict::NoVerdict, "{text}");
        assert_eq!(find("scan", "lmdb").verdict, Verdict::Regressed, "{text}");
        assert_eq!(find("read", "supdb").verdict, Verdict::Regressed, "{text}");
        assert!(
            rep.regressed(),
            "the arm's own regression still fails\n{text}"
        );
        assert_eq!(rep.comparators_low().len(), 1, "{text}");
        assert!(text.contains("comparators: 1 below their window"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A comparator below its window never fails the gate by itself: the
    /// run did not choose its machine.
    #[test]
    fn a_comparator_alone_never_fails_the_gate() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "supdb-bench-gate-cmp2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let build = |utc: &str, lmdb: f64| {
            let q = |arm: &str, v: f64| Measurement {
                workload: "read".into(),
                arm: arm.into(),
                guarantee: Guarantee::Durable,
                size: Some(10_000),
                quantity: "reads_per_s".into(),
                unit: "reads/s".into(),
                samples: vec![v, v * 1.01, v * 0.99, v, v * 1.005],
            };
            Row {
                utc: utc.into(),
                sha: format!("sha-{utc}"),
                rustc: "r".into(),
                scale: Scale::Quick,
                machine: machine(4),
                measurements: vec![q("supdb", 100.0), q("lmdb", lmdb)],
            }
        };
        for i in 0..6 {
            build(&format!("2026010{}T000000Z", i + 1), 80.0)
                .write(&dir)
                .unwrap();
        }
        let rep = gate(&build("20260201T000000Z", 40.0), &dir).unwrap();
        let text = rep.render();
        assert!(!rep.regressed(), "{text}");
        assert_eq!(rep.comparators_low().len(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every arm the suite runs is on one side of the control or the
    /// other: an arm added without a side would be a control nobody
    /// declared, or an engine nobody judges.
    #[test]
    fn every_arm_is_named_as_engine_or_comparator() {
        for arm in crate::engines::ARMS {
            let engine = arm.starts_with("supdb");
            assert_ne!(
                engine,
                is_comparator(arm),
                "arm {arm:?} is neither an engine nor a comparator, or both"
            );
        }
    }

    #[test]
    fn a_floor_below_the_window_withholds_every_machine_verdict() {
        let dir = fixture_with_control(6);
        let new = with_control(
            row(
                "20260201T000000Z",
                4,
                [70.0, 71.0, 69.5, 70.2, 70.8],
                [9.0, 9.1, 9.0, 9.2, 8.9],
            ),
            600_000.0,
            6.0e9,
            1_800_000.0,
            1.44,
        );
        let rep = gate(&new, &dir).unwrap();
        let text = rep.render();
        assert!(!rep.regressed(), "{text}");
        assert_eq!(rep.host_out_of_band().len(), 3, "{text}");
        for f in &rep.findings {
            if f.workload == "read" {
                assert_eq!(f.verdict, Verdict::NoVerdict, "{text}");
            }
        }
        assert!(text.contains("host: OUT OF BAND"), "{text}");
        assert!(text.contains("no verdict:"), "{text}");
    }

    /// A ratio of bytes is arithmetic on what was stored, so the machine
    /// cannot excuse it.
    #[test]
    fn a_byte_ratio_is_judged_whatever_the_floors_say() {
        let dir = fixture_with_control(6);
        let new = with_control(
            row(
                "20260201T000000Z",
                4,
                [70.0, 71.0, 69.5, 70.2, 70.8],
                [9.0, 9.1, 9.0, 9.2, 8.9],
            ),
            600_000.0,
            6.0e9,
            1_800_000.0,
            1.90,
        );
        let rep = gate(&new, &dir).unwrap();
        let text = rep.render();
        assert!(rep.regressed(), "{text}");
        let ratio = rep
            .findings
            .iter()
            .find(|f| f.quantity == "bytes_on_disk_per_byte")
            .expect("the ratio is a finding");
        assert_eq!(ratio.verdict, Verdict::Regressed, "{text}");
    }

    /// The control in band is the case the gate exists for.
    #[test]
    fn a_regression_on_a_machine_in_band_still_fails() {
        let dir = fixture_with_control(6);
        let new = with_control(
            row(
                "20260201T000000Z",
                4,
                [70.0, 71.0, 69.5, 70.2, 70.8],
                [9.0, 9.1, 9.0, 9.2, 8.9],
            ),
            1_000_000.0,
            9.0e9,
            3_000_000.0,
            1.44,
        );
        let rep = gate(&new, &dir).unwrap();
        let text = rep.render();
        assert!(rep.regressed(), "{text}");
        assert!(rep.host_out_of_band().is_empty(), "{text}");
        assert!(text.contains("host: floors within the window"), "{text}");
    }

    /// A floor is the machine's, so it is reported and never failed on.
    #[test]
    fn a_floor_alone_never_fails_the_gate() {
        let dir = fixture_with_control(6);
        let new = with_control(
            row(
                "20260201T000000Z",
                4,
                [100.0, 101.0, 99.5, 100.2, 100.8],
                [5.0, 5.1, 5.0, 5.2, 4.9],
            ),
            600_000.0,
            6.0e9,
            1_800_000.0,
            1.44,
        );
        let rep = gate(&new, &dir).unwrap();
        let text = rep.render();
        assert!(!rep.regressed(), "{text}");
        assert_eq!(rep.host_out_of_band().len(), 3, "{text}");
    }

    /// A window from before a floor existed cannot control for it, and the
    /// gate says so rather than passing everything.
    #[test]
    fn a_window_without_floors_says_it_has_no_control() {
        let (dir, _) = fixture(6);
        let new = row(
            "20260201T000000Z",
            4,
            [70.0, 71.0, 69.5, 70.2, 70.8],
            [9.0, 9.1, 9.0, 9.2, 8.9],
        );
        let rep = gate(&new, &dir).unwrap();
        let text = rep.render();
        assert!(rep.regressed(), "{text}");
        assert!(text.contains("host: no floor in band yet"), "{text}");
    }

    #[test]
    fn a_row_within_the_band_passes() {
        let (dir, _) = fixture(6);
        let new = row(
            "20260201T000000Z",
            4,
            [100.0, 101.0, 99.5, 100.2, 100.8],
            [5.0, 5.1, 5.0, 5.2, 4.9],
        );
        let rep = gate(&new, &dir).unwrap();
        assert!(!rep.regressed(), "{}", rep.render());
        assert!(
            rep.findings.iter().all(|f| f.verdict == Verdict::Within),
            "{}",
            rep.render()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn slower_reads_regress_and_higher_p99_regresses() {
        let (dir, _) = fixture(6);
        let new = row(
            "20260201T000000Z",
            4,
            [80.0, 81.0, 79.0, 80.5, 80.0],
            [7.0, 7.1, 7.0, 7.2, 6.9],
        );
        let rep = gate(&new, &dir).unwrap();
        assert!(rep.regressed());
        assert!(
            rep.findings.iter().all(|f| f.verdict == Verdict::Regressed),
            "{}",
            rep.render()
        );
        assert!(rep.render().contains("REGRESSED"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn better_than_every_prior_is_flagged_not_failed() {
        let (dir, _) = fixture(6);
        let new = row(
            "20260201T000000Z",
            4,
            [130.0, 131.0, 129.0, 130.5, 130.0],
            [3.0, 3.1, 3.0, 3.2, 2.9],
        );
        let rep = gate(&new, &dir).unwrap();
        assert!(!rep.regressed());
        assert!(
            rep.findings.iter().all(|f| f.verdict == Verdict::Flagged),
            "{}",
            rep.render()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn two_prior_rows_is_not_a_band() {
        let (dir, _) = fixture(2);
        let new = row(
            "20260201T000000Z",
            4,
            [10.0, 10.0, 10.0, 10.0, 10.0],
            [50.0, 50.0, 50.0, 50.0, 50.0],
        );
        let rep = gate(&new, &dir).unwrap();
        assert!(!rep.regressed(), "a wild row with no band must not fail");
        assert!(rep
            .findings
            .iter()
            .all(|f| f.verdict == Verdict::InsufficientHistory(2)));
        assert!(rep.render().contains("no band yet"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn another_class_is_not_history() {
        let (dir, _) = fixture(6);
        // Same numbers, eight cores: a different class, so no history.
        let new = row(
            "20260201T000000Z",
            8,
            [10.0, 10.0, 10.0, 10.0, 10.0],
            [50.0, 50.0, 50.0, 50.0, 50.0],
        );
        let rep = gate(&new, &dir).unwrap();
        assert_eq!(rep.prior_rows, 0);
        assert!(!rep.regressed());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_window_is_the_last_ten() {
        let (dir, _) = fixture(14);
        let new = row(
            "20260201T000000Z",
            4,
            [100.0, 101.0, 99.5, 100.2, 100.8],
            [5.0, 5.1, 5.0, 5.2, 4.9],
        );
        let rep = gate(&new, &dir).unwrap();
        assert_eq!(rep.prior_rows, WINDOW);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_runs_directory_is_no_history() {
        let dir =
            std::env::temp_dir().join(format!("supdb-bench-gate-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let new = row("20260201T000000Z", 4, [1.0; 5], [1.0; 5]);
        let rep = gate(&new, &dir).unwrap();
        assert_eq!(rep.prior_rows, 0);
        assert!(!rep.regressed());
    }

    /// One name per thread count the runner measures at. Kept in step
    /// here rather than derived, since a derived direction is a guess
    /// about a quantity nobody has looked at.
    #[test]
    fn every_threaded_quantity_is_named() {
        for t in crate::run::THREADS {
            for base in ["reads_per_s", "entries_per_s"] {
                let q = crate::run::threaded_quantity(base, t);
                assert_eq!(higher_is_better(&q), Some(true), "{q} is not named");
            }
        }
    }

    #[test]
    fn an_unknown_quantity_is_an_error_not_a_guess() {
        let (dir, _) = fixture(3);
        let mut new = row("20260201T000000Z", 4, [1.0; 5], [1.0; 5]);
        new.measurements[0].quantity = "frobs_per_fortnight".into();
        assert!(gate(&new, &dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
