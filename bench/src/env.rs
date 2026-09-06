//! The host, as read. Feeds `MachineInfo`; nothing here classifies.

use crate::machine::Machine;
use crate::row::MachineInfo;

fn read(p: &str) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

fn trimmed(p: &str) -> Option<String> {
    read(p)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Everything a row records about the machine.
pub fn capture() -> MachineInfo {
    let m = Machine::detect();
    let (cpu_model, cpus, mem_total_kb) = cpu_and_memory();
    MachineInfo {
        arch: std::env::consts::ARCH.to_string(),
        cpu_model,
        cpus,
        mem_total_kb,
        page_size: m.page_size as u64,
        cache_line: m.cache_line,
        cache_line_detected: m.cache_line_detected,
        l1d: m.l1d,
        l2: m.l2,
        l3: m.l3,
        kernel: kernel(),
        governor: trimmed("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
            .unwrap_or_else(|| "unknown".into()),
        thp: trimmed("/sys/kernel/mm/transparent_hugepage/enabled")
            .unwrap_or_else(|| "unknown".into()),
        smt_on: trimmed("/sys/devices/system/cpu/smt/active").as_deref() == Some("1"),
        pmu_available: read("/proc/sys/kernel/perf_event_paranoid").is_some()
            && std::path::Path::new("/sys/bus/event_source/devices/cpu").exists(),
        aslr_disabled: trimmed("/proc/sys/kernel/randomize_va_space").as_deref() == Some("0"),
        virtualised: virtualised(),
    }
}

#[cfg(not(target_os = "macos"))]
fn cpu_and_memory() -> (String, usize, u64) {
    let cpuinfo = read("/proc/cpuinfo").unwrap_or_default();
    let model = cpuinfo
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
    let mem = read("/proc/meminfo")
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal"))
                .and_then(|l| l.split_whitespace().nth(1).map(|v| v.to_string()))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    (model, cpus, mem)
}

#[cfg(target_os = "macos")]
fn cpu_and_memory() -> (String, usize, u64) {
    use crate::machine::{sysctl_num, sysctl_str};
    (
        sysctl_str("machdep.cpu.brand_string").unwrap_or_else(|| "unknown".into()),
        sysctl_num("hw.ncpu").unwrap_or(1),
        (sysctl_num("hw.memsize").unwrap_or(0) / 1024) as u64,
    )
}

#[cfg(not(target_os = "macos"))]
fn kernel() -> String {
    trimmed("/proc/sys/kernel/osrelease").unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn kernel() -> String {
    crate::machine::sysctl_str("kern.osrelease").unwrap_or_default()
}

/// `none` on bare metal, the hypervisor's name where the host gives one,
/// `hypervisor` where it only says there is one.
#[cfg(not(target_os = "macos"))]
fn virtualised() -> String {
    // DMI names the product on most clouds and VMMs.
    if let Some(p) = trimmed("/sys/class/dmi/id/product_name") {
        let l = p.to_ascii_lowercase();
        for (needle, name) in [
            ("firecracker", "firecracker"),
            ("kvm", "kvm"),
            ("qemu", "qemu"),
            ("vmware", "vmware"),
            ("virtualbox", "virtualbox"),
            ("hyper-v", "hyper-v"),
            ("virtual machine", "hyper-v"),
            ("xen", "xen"),
            ("google compute engine", "gce"),
            ("amazon ec2", "ec2"),
        ] {
            if l.contains(needle) {
                return name.into();
            }
        }
    }
    if let Some(t) = trimmed("/sys/hypervisor/type") {
        return t.to_ascii_lowercase();
    }
    let flags = read("/proc/cpuinfo").unwrap_or_default();
    if flags
        .lines()
        .any(|l| l.starts_with("flags") && l.split_whitespace().any(|f| f == "hypervisor"))
    {
        return "hypervisor".into();
    }
    "none".into()
}

#[cfg(target_os = "macos")]
fn virtualised() -> String {
    match crate::machine::sysctl_num("kern.hv_vmm_present") {
        Some(1) => "hypervisor".into(),
        _ => "none".into(),
    }
}

/// Bytes this process has moved to and from the device, cumulative.
///
/// `load` reports device bytes written per byte stored from the delta
/// across the load: a different quantity from file size, and the one that
/// says what a durable commit actually costs the device.
#[derive(Clone, Copy, Debug, Default)]
pub struct IoCounters {
    pub write_bytes: u64,
    pub read_bytes: u64,
}

impl IoCounters {
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
                    _ => {}
                }
            }
        }
        c
    }

    #[cfg(target_os = "macos")]
    pub fn read_now() -> IoCounters {
        let mut ru: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                &mut ru as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
            )
        };
        if rc == 0 {
            IoCounters {
                write_bytes: ru.ri_diskio_byteswritten,
                read_bytes: ru.ri_diskio_bytesread,
            }
        } else {
            IoCounters::default()
        }
    }

    pub fn since(&self, earlier: &IoCounters) -> IoCounters {
        IoCounters {
            write_bytes: self.write_bytes.saturating_sub(earlier.write_bytes),
            read_bytes: self.read_bytes.saturating_sub(earlier.read_bytes),
        }
    }
}
