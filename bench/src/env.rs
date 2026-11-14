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
    // The device the stores are built on: the temp directory's, which is
    // where `run` puts every store.
    let dev = block_device_of(&std::env::temp_dir());
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
        mem_limit_bytes: None,
        read_ahead_kb: dev.as_deref().and_then(|d| queue_num(d, "read_ahead_kb")),
        rotational: dev
            .as_deref()
            .and_then(|d| queue_num(d, "rotational"))
            .map(|n| n != 0),
        write_cache: dev
            .as_deref()
            .and_then(|d| trimmed(&format!("/sys/block/{d}/queue/write_cache"))),
        device: dev,
    }
}

/// The RAM-backed block device under `path`, by name, or none: zram, a
/// loop device, brd, anything the kernel files under
/// `/sys/devices/virtual/block/`, where a disk's entry names its bus,
/// that is not a mapping over a disk (`virtual_block_device`).
/// Such a device answers a fault in microseconds where the machine's
/// disk takes tens, with the kernel's reclaim and the engine's page
/// touches the same, so a pairing on one reads the disk's counts at a
/// price no user pays: `scripts/zram.sh` runs `ab` on one for the
/// counts, `run` refuses it, and `ab` names it on its table. One
/// definition for every platform: off Linux `block_device_of` finds no
/// device, so nothing is RAM-backed there, and the helper below is used
/// on every target rather than dead code on all but one.
pub fn ram_backed_device(path: &std::path::Path) -> Option<String> {
    let dev = block_device_of(path)?;
    virtual_block_device(std::path::Path::new("/sys/block"), &dev).then_some(dev)
}

/// Whether `sys_block/<dev>` is a device with no disk under it: one the
/// kernel files under its virtual devices that maps no other device, or
/// maps one that is such a device itself.
///
/// The kernel files a device-mapper volume and an md array as virtual
/// too, and those are the devices under them, which their `slaves`
/// directory names. The first version asked for the virtual link alone,
/// and a hosted runner whose work disk was an LVM volume (`dm-0`) was
/// refused as RAM-backed: the test that runs a pass in a process of its
/// own failed on it with the engine and the suite unchanged, where the
/// same job on the runner beside it passed.
fn virtual_block_device(sys_block: &std::path::Path, dev: &str) -> bool {
    no_disk_under(&sys_block.join(dev), 0)
}

/// `virtual_block_device` for a device by its link: under `/sys/block`,
/// or in the `slaves` of the device mapped over it. A stack of mappings
/// is a few deep; past eight it is taken for what its link says.
fn no_disk_under(link: &std::path::Path, depth: usize) -> bool {
    // Where the link leads, through every link on the way: a slave's
    // link to a device beside it is `../../zram0`, which names no
    // directory it passes.
    let filed_virtual = std::fs::canonicalize(link)
        .or_else(|_| std::fs::read_link(link))
        .map(|target| target.components().any(|c| c.as_os_str() == "virtual"))
        .unwrap_or(false);
    if !filed_virtual {
        return false;
    }
    let under: Vec<std::path::PathBuf> = std::fs::read_dir(link.join("slaves"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    if under.is_empty() || depth >= 8 {
        return true;
    }
    under.iter().any(|s| no_disk_under(s, depth + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device under the kernel's virtual devices is RAM-backed by this
    /// test, a disk under its bus is not, and a name with no entry is not.
    #[test]
    fn a_virtual_block_device_is_told_from_a_disk_by_its_link() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-sysblock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink("../devices/virtual/block/zram0", root.join("zram0")).unwrap();
        std::os::unix::fs::symlink("../devices/virtual/block/loop3", root.join("loop3")).unwrap();
        std::os::unix::fs::symlink(
            "../devices/pci0000:00/0000:00:02.0/virtio1/block/vda",
            root.join("vda"),
        )
        .unwrap();
        assert!(virtual_block_device(&root, "zram0"));
        assert!(virtual_block_device(&root, "loop3"));
        assert!(!virtual_block_device(&root, "vda"));
        assert!(!virtual_block_device(&root, "nvme0n1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A mapped volume is what it maps. The tree is sysfs's shape: the
    /// devices' directories, each mapped one's `slaves` linking to the
    /// devices under it, and `block/` linking to every whole device. A
    /// volume over a disk's partition is a disk's, one over zram is not,
    /// one stacked over another follows it down, and one over a disk and
    /// zram both is refused with the zram under it.
    #[test]
    fn a_mapped_volume_is_ram_backed_only_where_what_it_maps_is() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("supdb-bench-sysdm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let block = root.join("block");
        let virt = root.join("devices/virtual/block");
        let disk = root.join("devices/pci0000:00/0000:00:02.0/virtio1/block/vda");
        std::fs::create_dir_all(&block).unwrap();
        std::fs::create_dir_all(disk.join("vda2")).unwrap();
        symlink(
            "../devices/pci0000:00/0000:00:02.0/virtio1/block/vda",
            block.join("vda"),
        )
        .unwrap();
        for dev in ["zram0", "dm-0", "dm-1", "dm-2", "dm-3"] {
            std::fs::create_dir_all(virt.join(dev).join("slaves")).unwrap();
            symlink(format!("../devices/virtual/block/{dev}"), block.join(dev)).unwrap();
        }
        // A slave's link as sysfs writes it, relative to the `slaves`
        // directory it is in.
        let partition = "../../../../pci0000:00/0000:00:02.0/virtio1/block/vda/vda2";
        symlink(partition, virt.join("dm-0/slaves/vda2")).unwrap();
        symlink("../../zram0", virt.join("dm-1/slaves/zram0")).unwrap();
        symlink("../../dm-0", virt.join("dm-2/slaves/dm-0")).unwrap();
        symlink(partition, virt.join("dm-3/slaves/vda2")).unwrap();
        symlink("../../zram0", virt.join("dm-3/slaves/zram0")).unwrap();
        assert!(!virtual_block_device(&block, "vda"));
        assert!(virtual_block_device(&block, "zram0"));
        assert!(!virtual_block_device(&block, "dm-0"), "a volume over a disk");
        assert!(virtual_block_device(&block, "dm-1"), "a volume over zram");
        assert!(!virtual_block_device(&block, "dm-2"), "a volume over a volume");
        assert!(virtual_block_device(&block, "dm-3"), "a volume over both");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// A number from the device's queue directory, or none where the file is
/// not there or does not parse.
fn queue_num(dev: &str, name: &str) -> Option<u64> {
    trimmed(&format!("/sys/block/{dev}/queue/{name}")).and_then(|s| s.parse().ok())
}

/// The block device holding `path`, by name under `/sys/block` (`vda`):
/// the device number the file sits on, resolved through `/sys/dev/block`,
/// and a partition resolved to its disk, since the queue and the stat are
/// the disk's. None where the device has no entry there -- an overlay or
/// a tmpfs, whose numbers are anonymous -- or on a host without `/sys`.
#[cfg(target_os = "linux")]
pub fn block_device_of(path: &std::path::Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(path).ok()?.dev();
    // The kernel's encoding of a device number (`include/linux/kdev_t.h`,
    // the `new_encode_dev` form glibc's `major` and `minor` undo): twelve
    // bits of major split around the low byte of minor.
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    let link = std::fs::read_link(format!("/sys/dev/block/{major}:{minor}")).ok()?;
    let sys = std::path::Path::new("/sys/dev/block").join(&link);
    let node = if sys.join("partition").exists() {
        sys.parent()?.to_path_buf()
    } else {
        sys
    };
    let name = node.file_name()?.to_str()?.to_string();
    std::path::Path::new("/sys/block")
        .join(&name)
        .exists()
        .then_some(name)
}

#[cfg(not(target_os = "linux"))]
pub fn block_device_of(_path: &std::path::Path) -> Option<String> {
    None
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
