//! Environment capture and device-level I/O accounting.
//!
//! Two gaps in the original harness are closed here.
//!
//! The first is provenance. Every number in the design document came from one
//! machine and the write-up says so, but the results themselves do not carry
//! that fact -- so a figure copied into a table becomes unfalsifiable the
//! moment it leaves the page. Every record emitted by this harness carries the
//! machine it was taken on.
//!
//! The second is write amplification. The document compares its own
//! file-size-derived 1.15x against an LSM's device-level 10-30x. Those are not
//! the same quantity: file size counts what survived, not what was written,
//! and misses every byte that was written and later reused or truncated away.
//! `IoCounters` reads what the process actually sent to storage.

use super::J;
use crate::jobj;
use std::fs;

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn first_line_after(path: &str, prefix: &str) -> Option<String> {
    read(path)?
        .lines()
        .find(|l| l.starts_with(prefix))
        .map(|l| {
            l[prefix.len()..]
                .trim()
                .trim_start_matches(':')
                .trim()
                .to_string()
        })
}

/// Everything about the machine that could plausibly move a number.
#[derive(Clone, Debug)]
pub struct Env {
    pub kernel: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpus: usize,
    pub mem_total_kb: u64,
    pub page_size: u64,
    pub thp: String,
    pub governor: String,
    pub swap_total_kb: u64,
    pub rustc: String,
    pub git_sha: String,
    pub profile: String,
    /// Whether hardware performance counters can be read at all. False inside
    /// a Firecracker guest, which exposes no PMU.
    pub pmu_available: bool,
    pub smt_on: bool,
    pub aslr_disabled: bool,
}

impl Env {
    pub fn capture() -> Env {
        let cpuinfo = read("/proc/cpuinfo").unwrap_or_default();
        let cpu_model = cpuinfo
            .lines()
            .find(|l| l.starts_with("model name"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".into());
        let cpus = cpuinfo
            .lines()
            .filter(|l| l.starts_with("processor"))
            .count()
            .max(1);

        let kb = |k: &str| -> u64 {
            first_line_after("/proc/meminfo", k)
                .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };

        Env {
            kernel: read("/proc/sys/kernel/osrelease")
                .map(|s| s.trim().into())
                .unwrap_or_default(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_model,
            cpus,
            mem_total_kb: kb("MemTotal"),
            swap_total_kb: kb("SwapTotal"),
            page_size: page_size(),
            thp: read("/sys/kernel/mm/transparent_hugepage/enabled")
                .map(|s| s.trim().into())
                .unwrap_or_else(|| "unknown".into()),
            governor: read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
                .map(|s| s.trim().into())
                .unwrap_or_else(|| "unknown".into()),
            // A PMU that exists reports a nonzero count for a trivial program;
            // one that does not reports "<not supported>".
            pmu_available: read("/proc/sys/kernel/perf_event_paranoid").is_some()
                && std::path::Path::new("/sys/bus/event_source/devices/cpu").exists(),
            smt_on: read("/sys/devices/system/cpu/smt/active")
                .map(|s| s.trim() == "1")
                .unwrap_or(false),
            aslr_disabled: read("/proc/sys/kernel/randomize_va_space")
                .map(|s| s.trim() == "0")
                .unwrap_or(false),
            rustc: option_env!("SUPDB_RUSTC").unwrap_or("unknown").to_string(),
            git_sha: option_env!("SUPDB_GIT_SHA")
                .unwrap_or("unknown")
                .to_string(),
            profile: if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
        }
    }

    /// True when the machine is configured in a way that makes timings less
    /// trustworthy. Recorded rather than enforced -- CI runners are never
    /// clean, and refusing to run there would mean never running.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.governor != "performance" && self.governor != "unknown" {
            w.push(format!(
                "cpu governor is '{}', not 'performance'",
                self.governor
            ));
        }
        if self.thp.contains("[always]") {
            w.push("transparent hugepages are 'always'; page-fault costs will vary".into());
        }
        if self.swap_total_kb > 0 {
            w.push(
                "swap is enabled; an out-of-core result may measure swap, not the engine".into(),
            );
        }
        if cfg!(debug_assertions) {
            w.push("built without --release; timings are meaningless".into());
        }
        w
    }

    pub fn to_json(&self) -> J {
        jobj! {
            "kernel" => J::s(&self.kernel),
            "arch" => J::s(&self.arch),
            "cpu_model" => J::s(&self.cpu_model),
            "cpus" => J::u(self.cpus as u64),
            "mem_total_mb" => J::fp(self.mem_total_kb as f64 / 1024.0, 0),
            "swap_total_mb" => J::fp(self.swap_total_kb as f64 / 1024.0, 0),
            "page_size" => J::u(self.page_size),
            "thp" => J::s(&self.thp),
            "governor" => J::s(&self.governor),
            "rustc" => J::s(&self.rustc),
            "git_sha" => J::s(&self.git_sha),
            "profile" => J::s(&self.profile),
            "pmu_available" => J::Bool(self.pmu_available),
            "smt_on" => J::Bool(self.smt_on),
            "aslr_disabled" => J::Bool(self.aslr_disabled),
            "machine" => super::machine::Machine::detect().to_json(),
            "warnings" => J::arr(self.warnings().iter().map(J::s).collect()),
        }
    }
}

pub fn page_size() -> u64 {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
}

pub fn mem_total_bytes() -> u64 {
    first_line_after("/proc/meminfo", "MemTotal")
        .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

/// Peak resident set size in bytes, for the memory axis of the RUM trade.
///
/// A fully resident key index is memory spent to buy read speed. Reporting it
/// alongside throughput is what turns an unqualified win into a stated trade.
pub fn peak_rss_bytes() -> u64 {
    first_line_after("/proc/self/status", "VmHWM")
        .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

/// Current resident set size in bytes.
///
/// Distinct from `peak_rss_bytes`, which reports VmHWM -- a high-water mark.
/// For measuring what a structure costs, the peak is the wrong statistic: if
/// building the inputs spiked resident memory above what the structure itself
/// occupies, the delta between two peak readings is zero. That produced
/// "2.1 bytes per key" for a structure that plainly costs seventeen.
pub fn rss_bytes() -> u64 {
    first_line_after("/proc/self/status", "VmRSS")
        .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

/// Bytes this process has actually caused to be sent to storage.
#[derive(Clone, Copy, Debug, Default)]
pub struct IoCounters {
    /// From /proc/self/io `write_bytes`: bytes sent to the block layer.
    /// Unlike file size, this counts data that was later reused or truncated.
    pub write_bytes: u64,
    pub read_bytes: u64,
    /// Logical bytes passed to write syscalls, for comparison. The gap between
    /// this and `write_bytes` is page-cache absorption.
    pub wchar: u64,
}

impl IoCounters {
    pub fn read_now() -> IoCounters {
        let mut c = IoCounters::default();
        if let Some(s) = read("/proc/self/io") {
            for line in s.lines() {
                let mut it = line.split(':');
                let (Some(k), Some(v)) = (it.next(), it.next()) else {
                    continue;
                };
                let v: u64 = v.trim().parse().unwrap_or(0);
                match k {
                    "write_bytes" => c.write_bytes = v,
                    "read_bytes" => c.read_bytes = v,
                    "wchar" => c.wchar = v,
                    _ => {}
                }
            }
        }
        c
    }

    pub fn since(&self, start: &IoCounters) -> IoCounters {
        IoCounters {
            write_bytes: self.write_bytes.saturating_sub(start.write_bytes),
            read_bytes: self.read_bytes.saturating_sub(start.read_bytes),
            wchar: self.wchar.saturating_sub(start.wchar),
        }
    }
}

/// Device-level write amplification, measured rather than inferred.
///
/// `logical_bytes` is the user data handed to the store. The ratio against
/// bytes actually written to the device is the number an LSM's "10-30x" is
/// quoted in; the ratio against final file size is a different and more
/// flattering quantity, so both are reported side by side.
pub fn write_amp_json(io: &IoCounters, logical_bytes: u64, file_bytes: u64) -> J {
    let l = logical_bytes.max(1) as f64;
    jobj! {
        "logical_mb" => J::fp(logical_bytes as f64 / 1048576.0, 2),
        "device_write_mb" => J::fp(io.write_bytes as f64 / 1048576.0, 2),
        "syscall_write_mb" => J::fp(io.wchar as f64 / 1048576.0, 2),
        "device_read_mb" => J::fp(io.read_bytes as f64 / 1048576.0, 2),
        "file_mb" => J::fp(file_bytes as f64 / 1048576.0, 2),
        "write_amp_device" => J::fp(io.write_bytes as f64 / l, 3),
        "write_amp_syscall" => J::fp(io.wchar as f64 / l, 3),
        "space_amp_file" => J::fp(file_bytes as f64 / l, 3),
    }
}

/// Ask the kernel to drop the page cache. Requires root; reports whether it
/// worked so a "cold" result that was never cold cannot be reported as cold.
///
/// This is the exact failure the design document confesses to: "Java unmaps
/// only when a cleaner runs, so the mmap engine pinned its own pages across
/// drop_caches. It invalidated every earlier cold-read number." A cold
/// measurement that cannot prove it was cold is not a cold measurement.
pub fn drop_caches() -> bool {
    use std::io::Write;
    let synced = std::process::Command::new("sync")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !synced {
        return false;
    }
    fs::OpenOptions::new()
        .write(true)
        .open("/proc/sys/vm/drop_caches")
        .and_then(|mut f| f.write_all(b"3"))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_capture_is_populated() {
        let e = Env::capture();
        assert!(e.cpus >= 1);
        assert!(e.page_size >= 4096);
        assert!(!e.to_json().render().is_empty());
    }

    #[test]
    fn io_counters_are_monotonic() {
        let a = IoCounters::read_now();
        let b = IoCounters::read_now();
        assert!(b.wchar >= a.wchar);
        assert_eq!(b.since(&b).write_bytes, 0);
    }

    #[test]
    fn write_amp_reports_both_quantities() {
        let io = IoCounters {
            write_bytes: 300,
            read_bytes: 0,
            wchar: 250,
        };
        let j = write_amp_json(&io, 100, 150).render();
        assert!(j.contains("\"write_amp_device\":3.000"), "{j}");
        assert!(j.contains("\"space_amp_file\":1.500"), "{j}");
    }

    #[test]
    fn debug_build_is_flagged_as_untrustworthy() {
        let e = Env::capture();
        if cfg!(debug_assertions) {
            assert!(e.warnings().iter().any(|w| w.contains("--release")));
        }
    }
}
