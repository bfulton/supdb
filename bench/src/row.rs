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
    /// The memory bound the run measured under (`bound::Bound`), as the
    /// kernel read it back; none for an unbounded run. It is part of the
    /// class, since a store past the bound reads from the device where
    /// the same store unbounded reads from memory. Defaulted on read so
    /// that every row banked before the field parses, as the three device
    /// fields below are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_limit_bytes: Option<u64>,
    /// The block device under the run's stores, as its queue reports it:
    /// its readahead window, whether it calls itself rotational, and its
    /// write cache's mode. None where the device cannot be found from the
    /// temp directory or the file is unreadable, which a macOS row is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_ahead_kb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotational: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_cache: Option<String>,
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
    /// what a band needs. A bounded run appends its bound in exact
    /// mebibytes (`/b1536m`): the bound decides what the store reads
    /// from, so two bounds are two classes, and an unbounded row's class
    /// is the string it always was, which `every_banked_row_keeps_its_class`
    /// holds over every row under `runs/`.
    pub fn class(&self) -> String {
        let m = &self.machine;
        let cpu = slug(&m.cpu_model);
        let mem_gb = (m.mem_total_kb as f64 / 1048576.0).log2().round().exp2() as u64;
        let class = format!("{}/{cpu}/{}c/{mem_gb}g/{}", m.arch, m.cpus, m.virtualised);
        match m.mem_limit_bytes {
            Some(b) => format!("{class}/b{}m", b >> 20),
            None => class,
        }
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
                mem_limit_bytes: None,
                read_ahead_kb: Some(128),
                rotational: Some(true),
                write_cache: Some("write back".into()),
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
        assert_eq!(back.machine.read_ahead_kb, Some(128));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A bound is a class of its own, in exact mebibytes, and two bounds
    /// are two classes; without one the class is the unbounded string.
    #[test]
    fn a_bound_is_its_own_class() {
        let mut row = Row::read(&first_banked_row()).unwrap();
        let base = row.class();
        assert!(!base.contains("/b"), "{base}");
        row.machine.mem_limit_bytes = Some(1536 << 20);
        let b1536 = row.class();
        assert_eq!(b1536, format!("{base}/b1536m"));
        row.machine.mem_limit_bytes = Some(2048 << 20);
        let b2048 = row.class();
        assert_eq!(b2048, format!("{base}/b2048m"));
        assert_ne!(b1536, b2048);
        // The JSON of a bounded row carries the bound; an unbounded row's
        // carries no field for it, as the rows before the field had none.
        let text = serde_json::to_string(&row).unwrap();
        assert!(text.contains("\"mem_limit_bytes\":2147483648"), "{text}");
        row.machine.mem_limit_bytes = None;
        let text = serde_json::to_string(&row).unwrap();
        assert!(!text.contains("mem_limit_bytes"), "{text}");
    }

    fn runs_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runs")
    }

    fn banked_rows() -> Vec<PathBuf> {
        let mut v = Vec::new();
        for scale in ["quick", "full", "ooc"] {
            let Ok(rd) = std::fs::read_dir(runs_dir().join(scale)) else {
                continue;
            };
            for e in rd.flatten() {
                if e.path().extension().and_then(|s| s.to_str()) == Some("json") {
                    v.push(e.path());
                }
            }
        }
        v.sort();
        v
    }

    fn first_banked_row() -> PathBuf {
        banked_rows()
            .into_iter()
            .find(|p| !p.to_string_lossy().contains("/ooc/"))
            .expect("a banked row under runs/")
    }

    /// Every row banked before the machine gained its bound and device
    /// fields parses with none of them, and its class is the string it
    /// was: a field added to the machine re-buckets nothing. The classes
    /// are a snapshot of the files present when the fields were added,
    /// so a later change to `class` that moves one of them is caught
    /// here, and a row banked after the snapshot is held only to parsing
    /// and, outside `runs/ooc/`, to carrying no bound.
    #[test]
    fn every_banked_row_keeps_its_class() {
        const XEON_280: &str = "x86_64/intel-r-xeon-r-processor-2.80ghz/4c/16g/hypervisor";
        const XEON_210: &str = "x86_64/intel-r-xeon-r-processor-2.10ghz/4c/16g/hypervisor";
        const XEON: &str = "x86_64/intel-r-xeon-r-processor/4c/16g/hypervisor";
        let snapshot: &[(&str, &str)] = &[
            ("full/20260927T214755Z-f87b394.json", XEON_280),
            ("full/20260927T231042Z-260c09d.json", XEON_280),
            ("full/20260928T091313Z-260c09d.json", XEON_280),
            ("full/20260928T180204Z-f19abe5.json", XEON_280),
            ("full/20260929T025048Z-5eabd8d.json", XEON_210),
            ("quick/20260907T184735Z-4f0797c.json", XEON_280),
            ("quick/20260914T045949Z-7344833.json", XEON_280),
            ("quick/20260914T052409Z-9ef908a.json", XEON_280),
            ("quick/20260914T194945Z-13ebfcd.json", XEON_280),
            ("quick/20260915T010025Z-499c9b4.json", XEON_280),
            ("quick/20260915T161912Z-5fda5d8.json", XEON_210),
            ("quick/20260915T210115Z-400512b.json", XEON_280),
            ("quick/20260916T000227Z-9e78797.json", XEON_280),
            ("quick/20260916T005118Z-94a93d3.json", XEON_210),
            ("quick/20260916T005857Z-d6b1e4d.json", XEON_280),
            ("quick/20260916T022124Z-7a9391b.json", XEON_280),
            ("quick/20260916T033905Z-c6af669.json", XEON_280),
            ("quick/20260916T034414Z-c6af669.json", XEON_210),
            ("quick/20260916T102539Z-6b75f98.json", XEON_280),
            ("quick/20260916T112531Z-86940c3.json", XEON_280),
            ("quick/20260916T113005Z-9d09fd7.json", XEON_280),
            ("quick/20260916T131311Z-cb21c7e.json", XEON_280),
            ("quick/20260916T133021Z-d0a8440.json", XEON_280),
            ("quick/20260916T134450Z-33b864e.json", XEON_280),
            ("quick/20260916T140750Z-9b4bae8.json", XEON_280),
            ("quick/20260916T143244Z-1521f41.json", XEON_280),
            ("quick/20260916T150101Z-628ecb6.json", XEON_280),
            ("quick/20260916T154615Z-3a1235a.json", XEON_280),
            ("quick/20260916T180836Z-069eecb.json", XEON_280),
            ("quick/20260916T193913Z-820e03e.json", XEON_280),
            ("quick/20260916T212033Z-1583179.json", XEON_280),
            ("quick/20260916T232510Z-e2f4a83.json", XEON_280),
            ("quick/20260917T094308Z-47e0f2b.json", XEON_210),
            ("quick/20260923T084105Z-91870c8.json", XEON_280),
            ("quick/20260927T213848Z-f87b394.json", XEON_280),
            ("quick/20260927T230050Z-260c09d.json", XEON_280),
            ("quick/20260928T175054Z-f19abe5.json", XEON_280),
            ("quick/20261004T071938Z-b247414.json", XEON_280),
            ("quick/20261005T002546Z-7476b89.json", XEON_210),
            ("quick/20261005T021448Z-0cac4db.json", XEON_210),
            ("quick/20261005T030257Z-a466e59.json", XEON_210),
            ("quick/20261011T212304Z-db6fe5c.json", XEON),
            ("quick/20261012T085748Z-3c42fff.json", XEON),
            ("quick/20261012T112705Z-0968643.json", XEON),
            ("quick/20261012T165604Z-81a21d7.json", XEON),
            ("quick/20261012T183615Z-a0e1122.json", XEON),
            ("quick/20261013T000540Z-f63607a.json", XEON),
            ("quick/20261013T004708Z-570ce26.json", XEON),
            ("quick/20261013T051945Z-8cbe619.json", XEON),
            ("quick/20261013T103938Z-2b893ff.json", XEON),
            ("quick/20261013T123455Z-4cff9e9.json", XEON),
            ("quick/20261013T130539Z-503c1cf.json", XEON),
        ];
        let rows = banked_rows();
        assert!(!rows.is_empty(), "no rows under {}", runs_dir().display());
        let mut seen = 0;
        for path in &rows {
            let row = Row::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let rel = path
                .strip_prefix(runs_dir())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if row.scale != Scale::Ooc {
                assert_eq!(row.machine.mem_limit_bytes, None, "{rel}");
                assert!(!row.class().contains("/b"), "{rel}: {}", row.class());
            }
            if let Some((_, class)) = snapshot.iter().find(|(f, _)| *f == rel) {
                assert_eq!(row.class(), *class, "{rel}");
                assert_eq!(row.machine.read_ahead_kb, None, "{rel}");
                assert_eq!(row.machine.rotational, None, "{rel}");
                assert_eq!(row.machine.write_cache, None, "{rel}");
                seen += 1;
            }
        }
        assert_eq!(
            seen,
            snapshot.len(),
            "a snapshotted row is missing from runs/"
        );
    }
}
