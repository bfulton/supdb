//! The measurement substrate.
//!
//! This module is Tier 0 of the benchmark program: the machinery that has to
//! exist before any number produced by this repository can be trusted. It is
//! deliberately separate from the experiments, because the experiments are
//! where opinions live and this is where the rules live.
//!
//! Four rules, each closing a specific hole in the original harness:
//!
//! 1. **Nothing is measured once.** Configurations are run interleaved and
//!    reported as a median with an interquartile range and a bootstrap
//!    interval, never as a single number.
//! 2. **A difference is not a difference until it clears the gate.** See
//!    `stats::compare` -- a Mann-Whitney U test *and* a minimum effect size.
//! 3. **Throughput is never reported alone.** Every record carries the
//!    latency distribution, peak RSS, and bytes actually written to the
//!    device, so a win bought with memory or write amplification shows the
//!    price next to it.
//! 4. **Every record carries its machine.** A number without the machine that
//!    produced it cannot be reproduced and so cannot be falsified.

pub mod env;
pub mod hist;
pub mod jparse;
pub mod json;
pub mod plot;
pub mod stats;
pub mod workload;

pub use env::{peak_rss_bytes, Env, IoCounters};
pub use jparse::parse as parse_json;
pub use hist::Hist;
pub use json::J;
pub use stats::{compare, Comparison, Samples, Trial, Verdict, DEFAULT_REPS, MIN_EFFECT};
pub use workload::{db_key_into, KeyDist, KeyGen, Payload, Rng};

use crate::jobj;
use std::io::Write;
use std::path::Path;

/// How large an experiment runs.
///
/// The same code has to produce a result fast enough to gate a pull request
/// and a result large enough to mean something. Making that an explicit axis
/// keeps CI honest: a `Ci` result is labelled `Ci` in the output and is never
/// mistaken for evidence about a real dataset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Seconds. Proves the experiment runs and the invariants hold. Too small
    /// to say anything about performance.
    Ci,
    /// Minutes. Useful during development.
    Dev,
    /// Tens of minutes to hours. The profile any published claim must cite.
    Full,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Profile> {
        match s {
            "ci" => Some(Profile::Ci),
            "dev" => Some(Profile::Dev),
            "full" => Some(Profile::Full),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Profile::Ci => "ci",
            Profile::Dev => "dev",
            Profile::Full => "full",
        }
    }

    /// Pick the value for this profile.
    pub fn pick<T>(&self, ci: T, dev: T, full: T) -> T {
        match self {
            Profile::Ci => ci,
            Profile::Dev => dev,
            Profile::Full => full,
        }
    }

    /// Repetitions. CI still repeats -- fewer, but never once, because a
    /// single run is the habit this whole module exists to break.
    pub fn reps(&self) -> usize {
        self.pick(5, 5, DEFAULT_REPS)
    }

    /// Whether a result at this profile may be cited as evidence.
    pub fn is_citable(&self) -> bool {
        matches!(self, Profile::Full)
    }
}

/// One experiment's result, as committed to `results/`.
pub struct Record {
    pub experiment: String,
    pub profile: Profile,
    /// Free-form parameters of the run, echoed so the result is self-describing.
    pub params: Vec<(String, J)>,
    /// The measured series, keyed by name.
    pub series: Vec<(String, J)>,
    /// Comparisons that went through the significance gate.
    pub comparisons: Vec<(String, Comparison)>,
    /// Statements the experiment checked and their outcome. This is what makes
    /// the repository self-policing: a finding is recorded as a claim with a
    /// verdict, not as prose someone has to re-derive.
    pub findings: Vec<Finding>,
    pub env: Env,
    pub notes: Vec<String>,
}

/// The outcome of a checked statement.
///
/// Three states, not two. A finding whose preconditions were never met must
/// not report the same thing as one that was tested and passed: at the `ci`
/// profile the multi-process experiment runs 8 readers for 2 seconds, which
/// exercises neither the 64-slot reader table nor the 30-second stale window,
/// and reporting that as "holds" would turn an untested condition into a green
/// build. That is the precise failure mode a benchmark suite exists to
/// prevent, so it is represented in the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Holds,
    Fails,
    /// The conditions required to test this were not met by this run.
    NotExercised,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Holds => "holds",
            Status::Fails => "fails",
            Status::NotExercised => "not_exercised",
        }
    }
    pub fn from_str(s: &str) -> Option<Status> {
        match s {
            "holds" => Some(Status::Holds),
            "fails" => Some(Status::Fails),
            "not_exercised" => Some(Status::NotExercised),
            _ => None,
        }
    }
}

/// A checked statement about the engine.
#[derive(Clone, Debug)]
pub struct Finding {
    pub id: String,
    pub statement: String,
    pub status: Status,
    pub detail: String,
}

impl Finding {
    pub fn new(id: &str, statement: &str, holds: bool, detail: impl Into<String>) -> Finding {
        Finding {
            id: id.to_string(),
            statement: statement.to_string(),
            status: if holds { Status::Holds } else { Status::Fails },
            detail: detail.into(),
        }
    }

    /// Record that this run could not test the statement, and why.
    pub fn not_exercised(id: &str, statement: &str, why: impl Into<String>) -> Finding {
        Finding {
            id: id.to_string(),
            statement: statement.to_string(),
            status: Status::NotExercised,
            detail: why.into(),
        }
    }

    /// `holds` only when the statement was actually tested and passed.
    pub fn holds(&self) -> bool {
        self.status == Status::Holds
    }

    pub fn failed(&self) -> bool {
        self.status == Status::Fails
    }

    pub fn to_json(&self) -> J {
        jobj! {
            "id" => J::s(&self.id),
            "statement" => J::s(&self.statement),
            "status" => J::s(self.status.as_str()),
            "holds" => J::Bool(self.status == Status::Holds),
            "detail" => J::s(&self.detail),
        }
    }
}

impl Record {
    pub fn new(experiment: &str, profile: Profile) -> Record {
        Record {
            experiment: experiment.to_string(),
            profile,
            params: Vec::new(),
            series: Vec::new(),
            comparisons: Vec::new(),
            findings: Vec::new(),
            env: Env::capture(),
            notes: Vec::new(),
        }
    }

    pub fn param(&mut self, k: &str, v: J) -> &mut Self {
        self.params.push((k.to_string(), v));
        self
    }

    pub fn series(&mut self, k: &str, v: J) -> &mut Self {
        self.series.push((k.to_string(), v));
        self
    }

    pub fn compare(&mut self, name: &str, c: Comparison) -> &mut Self {
        self.comparisons.push((name.to_string(), c));
        self
    }

    pub fn finding(&mut self, f: Finding) -> &mut Self {
        self.findings.push(f);
        self
    }

    pub fn note(&mut self, n: impl Into<String>) -> &mut Self {
        self.notes.push(n.into());
        self
    }

    /// True when no finding in this record failed. A finding that was not
    /// exercised does not fail the record, but it is reported loudly and
    /// `verify` treats it as a regression when a claim expected it to run.
    pub fn all_findings_hold(&self) -> bool {
        self.findings.iter().all(|f| !f.failed())
    }

    pub fn unexercised(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.status == Status::NotExercised).collect()
    }

    pub fn to_json(&self) -> J {
        jobj! {
            "experiment" => J::s(&self.experiment),
            "profile" => J::s(self.profile.as_str()),
            "citable" => J::Bool(self.profile.is_citable()),
            "params" => J::O(self.params.clone()),
            "series" => J::O(self.series.clone()),
            "comparisons" => J::O(
                self.comparisons.iter().map(|(k, c)| (k.clone(), c.to_json())).collect()
            ),
            "findings" => J::arr(self.findings.iter().map(|f| f.to_json()).collect()),
            "env" => self.env.to_json(),
            "notes" => J::arr(self.notes.iter().map(J::s).collect()),
        }
    }

    /// Write to `results/<experiment>.<profile>.json` and echo a human summary.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.{}.json", self.experiment, self.profile.as_str()));
        let mut f = std::fs::File::create(&path)?;
        writeln!(f, "{}", self.to_json().render())?;
        eprintln!("# wrote {}", path.display());
        Ok(())
    }

    /// Print the summary a human reads in the terminal.
    pub fn print_summary(&self) {
        println!("\n=== {} [{}] ===", self.experiment, self.profile.as_str());
        for w in self.env.warnings() {
            println!("  ! {w}");
        }
        for (name, c) in &self.comparisons {
            println!("  {}", c.summary(name, "baseline"));
        }
        for f in &self.findings {
            let tag = match f.status {
                Status::Holds => "HOLDS",
                Status::Fails => "FAILS",
                Status::NotExercised => "NOT EXERCISED",
            };
            println!("  [{tag}] {}: {}", f.id, f.statement);
            if !f.detail.is_empty() {
                println!("        {}", f.detail);
            }
        }
        for n in &self.notes {
            println!("  note: {n}");
        }
        if !self.profile.is_citable() {
            println!("  (profile '{}' is not citable evidence)", self.profile.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_scale_and_only_full_is_citable() {
        assert_eq!(Profile::parse("ci"), Some(Profile::Ci));
        assert_eq!(Profile::parse("nonsense"), None);
        assert!(!Profile::Ci.is_citable());
        assert!(!Profile::Dev.is_citable());
        assert!(Profile::Full.is_citable());
        assert!(Profile::Ci.reps() >= 5, "even CI must repeat");
    }

    #[test]
    fn record_round_trips_to_parseable_json() {
        let mut r = Record::new("t", Profile::Ci);
        r.param("n", J::u(10));
        r.finding(Finding::new("F", "a thing holds", true, "because"));
        let s = r.to_json().render();
        assert!(s.starts_with('{') && s.ends_with('}'));
        assert!(s.contains("\"experiment\":\"t\""));
        assert!(s.contains("\"citable\":false"));
    }

    #[test]
    fn a_failing_finding_fails_the_record() {
        let mut r = Record::new("t", Profile::Ci);
        r.finding(Finding::new("A", "holds", true, ""));
        assert!(r.all_findings_hold());
        r.finding(Finding::new("B", "does not", false, ""));
        assert!(!r.all_findings_hold());
    }
}
