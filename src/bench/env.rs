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
            // Provenance for the two counters whose platform analogues are not
            // the same quantity. A write-amp figure is only comparable to
            // another one when both records name the same counter here.
            "rss_counter" => J::s(rss_counter_source()),
            "device_write_counter" => J::s(device_write_counter_source()),
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
#[cfg(not(target_os = "macos"))]
pub fn peak_rss_bytes() -> u64 {
    first_line_after("/proc/self/status", "VmHWM")
        .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

/// macOS: `resident_size_max` from mach `task_info`, the analogue of VmHWM.
/// See the `darwin` module for what "analogue" does and does not promise.
#[cfg(target_os = "macos")]
pub fn peak_rss_bytes() -> u64 {
    darwin::task_basic_info()
        .map(|i| i.resident_size_max)
        .unwrap_or(0)
}

/// Current resident set size in bytes.
///
/// Distinct from `peak_rss_bytes`, which reports VmHWM -- a high-water mark.
/// For measuring what a structure costs, the peak is the wrong statistic: if
/// building the inputs spiked resident memory above what the structure itself
/// occupies, the delta between two peak readings is zero. That produced
/// "2.1 bytes per key" for a structure that plainly costs seventeen.
#[cfg(not(target_os = "macos"))]
pub fn rss_bytes() -> u64 {
    first_line_after("/proc/self/status", "VmRSS")
        .and_then(|v| v.split_whitespace().next().map(|s| s.to_string()))
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

/// macOS: `resident_size` from mach `task_info`, the analogue of VmRSS.
#[cfg(target_os = "macos")]
pub fn rss_bytes() -> u64 {
    darwin::task_basic_info()
        .map(|i| i.resident_size)
        .unwrap_or(0)
}

/// Names the counter behind `rss_bytes` / `peak_rss_bytes` on this platform.
/// Recorded in every env block so a record taken on macOS cannot be silently
/// read as a Linux /proc figure.
pub fn rss_counter_source() -> &'static str {
    if cfg!(target_os = "macos") {
        "mach task_info MACH_TASK_BASIC_INFO resident_size (process-level analogue of \
         /proc/self/status VmRSS/VmHWM, not the identical quantity)"
    } else {
        "/proc/self/status"
    }
}

/// Names the counter behind `IoCounters::write_bytes` on this platform.
///
/// The Linux quantity is specific -- bytes this process caused to be sent to
/// the block layer -- and rule 4 says write amplification is measured from it,
/// never inferred from file size. The macOS counter is the closest
/// process-level analogue xnu keeps, not the same quantity, so the env block
/// of every record names which one produced the number: a Mac record must not
/// be read as a Linux-comparable write-amp figure.
pub fn device_write_counter_source() -> &'static str {
    if cfg!(target_os = "macos") {
        "proc_pid_rusage RUSAGE_INFO_V2 ri_diskio_byteswritten (process-level analogue of \
         /proc/self/io write_bytes, not the identical quantity)"
    } else {
        "/proc/self/io"
    }
}

/// The macOS analogues of the /proc counters, kept in one place.
///
/// These are process-level analogues, not the Linux quantities under other
/// names: `ri_diskio_byteswritten` is xnu's per-process disk-I/O ledger where
/// /proc/self/io `write_bytes` counts bytes sent to the block layer, and mach
/// `resident_size` is the task's resident footprint where VmRSS is the
/// process's. Close enough to make a Mac record legible, not close enough to
/// compare against a Linux one -- which is why `rss_counter_source` and
/// `device_write_counter_source` go into every env block.
///
/// Every call here degrades to "counter absent" on a syscall failure, and the
/// callers above turn that into 0 -- exactly what the /proc reads used to
/// return on this platform -- rather than panicking. A wrong answer from the
/// kernel loses a column, not a run.
//
// `libc` is deprecating its mach bindings in favour of the `mach2` crate, and
// this module is the only thing that calls them -- which is why the
// deprecation went unnoticed until macOS joined the test matrix and `-D
// warnings` turned it into an error. Allowed here rather than depended away:
// `mach2` would be a whole crate for one symbol, and `task_info` would then
// take a port from one crate's mach bindings and its flavour constants from
// another's. If libc removes these rather than deprecating them, that is the
// moment to take the dependency.
#[allow(deprecated)]
#[cfg(target_os = "macos")]
mod darwin {
    /// `MACH_TASK_BASIC_INFO` for this task, or `None` if the kernel refused.
    pub fn task_basic_info() -> Option<libc::mach_task_basic_info> {
        let mut info: libc::mach_task_basic_info = unsafe { std::mem::zeroed() };
        let mut count: libc::mach_msg_type_number_t = libc::MACH_TASK_BASIC_INFO_COUNT;
        // SAFETY: `info` is a zero-initialized mach_task_basic_info, `count`
        // holds its size in natural_t units as the call requires, and
        // mach_task_self() names the calling task.
        let kr = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                &mut info as *mut libc::mach_task_basic_info as libc::task_info_t,
                &mut count,
            )
        };
        (kr == libc::KERN_SUCCESS).then_some(info)
    }

    /// `RUSAGE_INFO_V2` for this process, or `None` if the kernel refused.
    pub fn rusage_v2() -> Option<libc::rusage_info_v2> {
        let mut ru: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: the RUSAGE_INFO_V2 flavor tells the kernel the buffer is a
        // rusage_info_v2, which it is, and the pid names this process. The
        // double-pointer-looking cast matches Apple's own declaration --
        // `rusage_info_t *buffer` where rusage_info_t is `void *` -- and the
        // convention every caller of it uses: a pointer to the struct itself,
        // cast to that parameter type.
        let rc = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                &mut ru as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
            )
        };
        (rc == 0).then_some(ru)
    }
}

/// Bytes this process has actually caused to be sent to storage.
#[derive(Clone, Copy, Debug, Default)]
pub struct IoCounters {
    /// From /proc/self/io `write_bytes`: bytes sent to the block layer.
    /// Unlike file size, this counts data that was later reused or truncated.
    /// On macOS this is `ri_diskio_byteswritten` instead -- a process-level
    /// analogue, not the identical quantity; `device_write_counter_source`
    /// names which one a record was taken with.
    pub write_bytes: u64,
    pub read_bytes: u64,
    /// Logical bytes passed to write syscalls, for comparison. The gap between
    /// this and `write_bytes` is page-cache absorption.
    pub wchar: u64,
}

impl IoCounters {
    /// macOS: the disk-I/O ledger from `proc_pid_rusage`. `write_bytes` and
    /// `read_bytes` are process-level analogues of the Linux counters, not the
    /// identical quantities (`device_write_counter_source` says so in every
    /// record). There is no macOS counter for `wchar`, so it stays 0 and
    /// page-cache absorption reads as unmeasured rather than as zero pages
    /// absorbed.
    #[cfg(target_os = "macos")]
    pub fn read_now() -> IoCounters {
        match darwin::rusage_v2() {
            Some(ru) => IoCounters {
                write_bytes: ru.ri_diskio_byteswritten,
                read_bytes: ru.ri_diskio_bytesread,
                wchar: 0,
            },
            None => IoCounters::default(),
        }
    }

    #[cfg(not(target_os = "macos"))]
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

/// Where a phase's wall-clock time went: on a CPU, or waiting.
///
/// This closes the gap `f13-sync` and `F13.2` are about. Callgrind counts
/// instructions, so it can say the block table decode is 34% of them -- which
/// was true, and mapping it changed throughput by nothing, because the
/// workload was waiting on `fsync`. An instruction profile answers where the
/// CPU goes, and the question it cannot answer is why a workload is slow when
/// the CPU is not where it is going.
///
/// There is no PMU on this hypervisor, so `perf` reports `<not supported>`.
/// None is needed for this: the kernel already tracks per-thread CPU time, and
/// wall minus CPU is time the thread was not running -- blocked on I/O, on a
/// page fault that reached the disk, or on a lock. `getrusage` supplies the
/// reason: a major fault went to disk, a voluntary context switch is a thread
/// that chose to block, an involuntary one is a thread that was preempted.
///
/// Thread-scoped, so it measures the caller and not whatever else the process
/// is doing. Every timing benchmark here is single-threaded by rule.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wait {
    pub wall_ns: u64,
    pub cpu_ns: u64,
    /// Faults that had to read the backing store.
    pub major_faults: u64,
    pub minor_faults: u64,
    /// Blocked on purpose: a syscall that slept.
    pub voluntary_switches: u64,
    /// Preempted: the scheduler took the CPU away.
    pub involuntary_switches: u64,
}

fn clock_ns(id: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, fully initialized timespec and the clock ids
    // used here are the POSIX constants.
    if unsafe { libc::clock_gettime(id, &mut ts) } != 0 {
        return 0;
    }
    (ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64
}

impl Wait {
    pub fn read_now() -> Wait {
        // Thread-scoped where the platform has it. `RUSAGE_THREAD` is a Linux
        // extension and does not exist on macOS, where this asks about the
        // process instead -- which is the same answer here, because no timing
        // benchmark in this repository is allowed to run beside another one.
        #[cfg(target_os = "linux")]
        let who = libc::RUSAGE_THREAD;
        #[cfg(not(target_os = "linux"))]
        let who = libc::RUSAGE_SELF;
        // SAFETY: `ru` is fully initialized before the call reads it, and
        // `who` is one of the POSIX constants.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::getrusage(who, &mut ru) } == 0;
        Wait {
            wall_ns: clock_ns(libc::CLOCK_MONOTONIC),
            cpu_ns: clock_ns(libc::CLOCK_THREAD_CPUTIME_ID),
            major_faults: if ok { ru.ru_majflt as u64 } else { 0 },
            minor_faults: if ok { ru.ru_minflt as u64 } else { 0 },
            voluntary_switches: if ok { ru.ru_nvcsw as u64 } else { 0 },
            involuntary_switches: if ok { ru.ru_nivcsw as u64 } else { 0 },
        }
    }

    pub fn since(&self, start: &Wait) -> Wait {
        Wait {
            wall_ns: self.wall_ns.saturating_sub(start.wall_ns),
            cpu_ns: self.cpu_ns.saturating_sub(start.cpu_ns),
            major_faults: self.major_faults.saturating_sub(start.major_faults),
            minor_faults: self.minor_faults.saturating_sub(start.minor_faults),
            voluntary_switches: self
                .voluntary_switches
                .saturating_sub(start.voluntary_switches),
            involuntary_switches: self
                .involuntary_switches
                .saturating_sub(start.involuntary_switches),
        }
    }

    /// Wall time the thread spent not running. The half an instruction profile
    /// cannot see.
    pub fn off_cpu_ns(&self) -> u64 {
        self.wall_ns.saturating_sub(self.cpu_ns)
    }

    /// Fraction of wall time spent off CPU, in [0, 1].
    pub fn off_cpu_fraction(&self) -> f64 {
        if self.wall_ns == 0 {
            return 0.0;
        }
        self.off_cpu_ns() as f64 / self.wall_ns as f64
    }

    pub fn to_json(&self) -> J {
        jobj! {
            "wall_ms" => J::fp(self.wall_ns as f64 / 1e6, 3),
            "cpu_ms" => J::fp(self.cpu_ns as f64 / 1e6, 3),
            "off_cpu_ms" => J::fp(self.off_cpu_ns() as f64 / 1e6, 3),
            "off_cpu_fraction" => J::fp(self.off_cpu_fraction(), 4),
            "major_faults" => J::u(self.major_faults),
            "minor_faults" => J::u(self.minor_faults),
            "voluntary_switches" => J::u(self.voluntary_switches),
            "involuntary_switches" => J::u(self.involuntary_switches)
        }
    }
}

/// Cap the memory this process may use, page cache included.
///
/// The out-of-core hazard (`F1.2`, a 916x collapse) needs a store larger than
/// the memory available to cache it, and the experiment that measures it
/// therefore builds 23GB and is `not_exercised` anywhere smaller. A machine
/// with 15GB of RAM and 20GB of free disk cannot run it at all.
///
/// A memory cgroup makes the ratio a parameter instead of a property of the
/// host. Page cache is charged to the cgroup that faults it in, so capping the
/// cgroup caps the cache and reclaim starts at the limit -- which is the
/// pressure the hazard is about. A 6GB store under a 2GB cap is out-of-core by
/// the same 3:1 ratio as 45GB on this host would be, and fits.
///
/// Join *after* building, since anonymous memory counts against the same
/// limit. Returns false when there is no cgroup filesystem to write to, which
/// is not an error -- it means the experiment cannot claim to have exercised
/// the condition, and `Finding::not_exercised` is what that is for.
pub fn cap_memory(bytes: u64) -> bool {
    let root = "/sys/fs/cgroup/memory";
    if !std::path::Path::new(root).is_dir() {
        return false;
    }
    let dir = format!("{root}/supdb-{}", std::process::id());
    if fs::create_dir_all(&dir).is_err() {
        return false;
    }
    if fs::write(format!("{dir}/memory.limit_in_bytes"), bytes.to_string()).is_err() {
        return false;
    }
    // Best effort: without this the cap applies to page cache only once the
    // process is inside, so a failure here means the cap does nothing.
    fs::write(
        format!("{dir}/cgroup.procs"),
        std::process::id().to_string(),
    )
    .is_ok()
}

/// Lift the cap this process set, and leave the cgroup it set it on.
///
/// A cap is a property of the *process*, not of the experiment that asked for
/// it: `cap_memory` writes this pid into the cgroup and it stays there. Every
/// experiment that runs afterwards inherits the limit, and the first one to
/// allocate past it is killed by the OOM killer. That is not hypothetical --
/// `internal all` died at f24 on every host with a writable v1 memory
/// controller, because f23 capped at 16MB and nothing put it back. On a host
/// without one the cap silently fails and the suite runs to the end, which is
/// why the committed results exist and the bug stayed invisible.
///
/// Idempotent, and false when there was nothing to lift.
pub fn uncap_memory() -> bool {
    let root = "/sys/fs/cgroup/memory";
    let dir = format!("{root}/supdb-{}", std::process::id());
    if !std::path::Path::new(&dir).is_dir() {
        return false;
    }
    // The limit first: if moving the process out fails, the cap is still
    // gone, which is the half that matters.
    let lifted = fs::write(format!("{dir}/memory.limit_in_bytes"), "-1").is_ok();
    let _ = fs::write(
        format!("{root}/cgroup.procs"),
        std::process::id().to_string(),
    );
    // Only removable once empty, so this fails harmlessly if the move did.
    let _ = fs::remove_dir(&dir);
    lifted
}

/// Lifts whatever cap this process set, when it is dropped.
///
/// Hold one for the length of an experiment that caps memory, so the cap
/// belongs to the experiment rather than to the rest of the run -- including
/// when the experiment returns early or panics.
pub struct CapGuard;

impl Drop for CapGuard {
    fn drop(&mut self) {
        uncap_memory();
    }
}

/// A `CapGuard`. Bind it before capping: `let _cap = env::cap_guard();`
pub fn cap_guard() -> CapGuard {
    CapGuard
}

/// The cap actually in force, if any.
pub fn memory_cap() -> Option<u64> {
    let dir = format!("/sys/fs/cgroup/memory/supdb-{}", std::process::id());
    read(&format!("{dir}/memory.limit_in_bytes"))?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|v| *v < u64::MAX / 2)
}
