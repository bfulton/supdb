//! The gate: is this row worse than its own history?
//!
//! For each (class, workload, arm, size, quantity), take the last `WINDOW`
//! rows at the same scale in `runs/` for that class. The new row regresses
//! if its CI lies entirely on the worse side of every one of those rows'
//! CIs. A row better than every prior CI is flagged, not failed: it is
//! either a win or a broken measurement, and a person should know which.
//! Fewer than `MIN_HISTORY` prior rows: no band, and the gate says so.
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
/// one that is not is an error, never a guess.
pub fn higher_is_better(quantity: &str) -> Option<bool> {
    Some(match quantity {
        "ops_per_s" | "reads_per_s" | "entries_per_s" | "bytes_per_s" => true,
        "p99_us" | "device_bytes_per_byte" => false,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Entirely on the worse side of every prior CI.
    Regressed,
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
    pub fn regressed(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.verdict == Verdict::Regressed)
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
        for f in &self.findings {
            let (tag, note) = match &f.verdict {
                Verdict::Regressed => ("REGRESSED", String::new()),
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
        let regressed = self.count(|v| *v == Verdict::Regressed);
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
        out.push_str(&if regressed > 0 {
            format!("REGRESSED: {regressed} of {total} quantities are worse than every row in the window\n")
        } else {
            format!("ok: nothing worse than the window ({flagged} flagged)\n")
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

    // Prior CIs by key.
    type Key = (String, String, Option<u64>, String);
    let mut prior: HashMap<Key, Vec<(f64, f64)>> = HashMap::new();
    for r in &window {
        for m in &r.measurements {
            let k = (
                m.workload.clone(),
                m.arm.clone(),
                m.size,
                m.quantity.clone(),
            );
            prior
                .entry(k)
                .or_default()
                .push(Samples::new(m.samples.clone()).median_ci(CI_CONF, CI_RESAMPLES));
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
        let ci = Samples::new(m.samples.clone()).median_ci(CI_CONF, CI_RESAMPLES);
        let k = (
            m.workload.clone(),
            m.arm.clone(),
            m.size,
            m.quantity.clone(),
        );
        let priors = prior.get(&k).map(Vec::as_slice).unwrap_or(&[]);
        let (verdict, envelope) = if priors.len() < MIN_HISTORY {
            (Verdict::InsufficientHistory(priors.len()), None)
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
            prior: envelope,
            prior_rows: priors.len(),
        });
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

    #[test]
    fn an_unknown_quantity_is_an_error_not_a_guess() {
        let (dir, _) = fixture(3);
        let mut new = row("20260201T000000Z", 4, [1.0; 5], [1.0; 5]);
        new.measurements[0].quantity = "frobs_per_fortnight".into();
        assert!(gate(&new, &dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
