//! A row: one run's measurements and the machine they were taken on.
//!
//! Nothing in a row is derived. `samples` is the raw per-rep values;
//! median, CI and spread are computed when the series is read, so a change
//! to the statistic recomputes history rather than stranding it. The machine
//! fields are what was read; the class that decides which rows are
//! comparable is derived from them by `Row::class` at read time.

use crate::Scale;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// What an arm promises about a committed batch. Comparisons are made
/// within a guarantee, never across one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Guarantee {
    /// Every batch is on the device before the call returns.
    Durable,
    /// Written to the OS; the OS gets to it.
    Buffered,
}

/// The machine, as read. Nothing here is classified.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineInfo {
    pub arch: String,
    pub cpu_model: String,
    pub cpus: usize,
    pub mem_total_kb: u64,
    pub page_size: u64,
    pub cache_line: usize,
    /// False when the line size was defaulted rather than read. A layout
    /// constant built on a guessed line size is not a measurement, and on
    /// Apple Silicon the guess is wrong by a factor of two.
    pub cache_line_detected: bool,
    pub l1d: usize,
    pub l2: usize,
    pub l3: usize,
    pub kernel: String,
    pub governor: String,
    pub thp: String,
    pub smt_on: bool,
    pub pmu_available: bool,
    pub aslr_disabled: bool,
    /// `none` on bare metal; otherwise the hypervisor as the host names it
    /// -- `kvm`, `firecracker`, `vmware` -- or `hypervisor` when it says only
    /// that there is one. A noisy VM is a class like any other.
    pub virtualised: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub workload: String,
    pub arm: String,
    pub guarantee: Guarantee,
    /// The ladder rung in keys. The floors carry no size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub quantity: String,
    pub unit: String,
    /// One value per repetition, in the order they were taken.
    pub samples: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    /// When the run started, `YYYYMMDDThhmmssZ`.
    pub utc: String,
    pub sha: String,
    pub rustc: String,
    pub scale: Scale,
    pub machine: MachineInfo,
    pub measurements: Vec<Measurement>,
}

impl Row {
    /// `<utc>-<sha7>.json`.
    pub fn file_name(&self) -> String {
        let sha7: String = self.sha.chars().take(7).collect();
        format!("{}-{sha7}.json", self.utc)
    }

    /// Write under `runs/<scale>/` and return the path.
    pub fn write(&self, runs: &Path) -> io::Result<PathBuf> {
        let dir = runs.join(self.scale.as_str());
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(self.file_name());
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&path, text + "\n")?;
        Ok(path)
    }

    pub fn read(path: &Path) -> io::Result<Row> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(io::Error::other)
    }

    /// Which rows are comparable. Derived here, at read time, from the
    /// fields as read -- change this function and history re-buckets.
    ///
    /// Architecture, the CPU model, the core count, memory to the nearest
    /// power of two, and whether the host is virtualised. Two GitHub
    /// runners of the same instance type land in the same class, which is
    /// what a band needs.
    pub fn class(&self) -> String {
        let m = &self.machine;
        let cpu = slug(&m.cpu_model);
        let mem_gb = (m.mem_total_kb as f64 / 1048576.0).log2().round().exp2() as u64;
        format!("{}/{cpu}/{}c/{mem_gb}g/{}", m.arch, m.cpus, m.virtualised)
    }
}

/// Lowercase, alphanumerics and dots kept, runs of anything else collapsed
/// to one hyphen, trimmed. `Intel(R) Xeon(R) Processor @ 2.80GHz` becomes
/// `intel-r-xeon-r-processor-2.80ghz`.
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Now, as `YYYYMMDDThhmmssZ`, from the system clock and nothing else.
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}{mo:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_702), (2026, 9, 6));
    }

    #[test]
    fn slugs() {
        assert_eq!(
            slug("Intel(R) Xeon(R) Processor @ 2.80GHz"),
            "intel-r-xeon-r-processor-2.80ghz"
        );
        assert_eq!(slug("Apple M2"), "apple-m2");
    }

    #[test]
    fn a_row_round_trips() {
        let row = Row {
            utc: "20260906T000000Z".into(),
            sha: "0123456789abcdef".into(),
            rustc: "rustc 1.90".into(),
            scale: Scale::Quick,
            machine: MachineInfo {
                arch: "x86_64".into(),
                cpu_model: "Intel(R) Xeon(R) Processor @ 2.80GHz".into(),
                cpus: 4,
                mem_total_kb: 16_461_000,
                page_size: 4096,
                cache_line: 64,
                cache_line_detected: true,
                l1d: 32768,
                l2: 1_048_576,
                l3: 34_603_008,
                kernel: "6.18".into(),
                governor: "unknown".into(),
                thp: "always [madvise] never".into(),
                smt_on: false,
                pmu_available: false,
                aslr_disabled: false,
                virtualised: "firecracker".into(),
            },
            measurements: vec![Measurement {
                workload: "read".into(),
                arm: "supdb".into(),
                guarantee: Guarantee::Durable,
                size: Some(10_000),
                quantity: "reads_per_s".into(),
                unit: "reads/s".into(),
                samples: vec![1.0, 2.0, 3.0],
            }],
        };
        let dir = std::env::temp_dir().join(format!("supdb-bench-row-{}", std::process::id()));
        let path = row.write(&dir).unwrap();
        assert!(path.ends_with("quick/20260906T000000Z-0123456.json"));
        let back = Row::read(&path).unwrap();
        assert_eq!(back.measurements[0].samples, vec![1.0, 2.0, 3.0]);
        assert_eq!(
            back.class(),
            "x86_64/intel-r-xeon-r-processor-2.80ghz/4c/16g/firecracker"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
