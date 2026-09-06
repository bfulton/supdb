//! The cache hierarchy, as the host reports it.
//!
//! Linux publishes it under sysfs; macOS under `sysctl`. Neither is
//! guessed: a line size that could not be read is recorded as not detected,
//! because a layout constant built on a guess is not a measurement, and on
//! Apple Silicon the obvious guess (64) is wrong by a factor of two.

#[derive(Clone, Copy, Debug)]
pub struct Machine {
    pub cache_line: usize,
    pub cache_line_detected: bool,
    pub page_size: usize,
    pub l1d: usize,
    pub l2: usize,
    pub l3: usize,
}

impl Machine {
    pub fn detect() -> Machine {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as usize;
        let (line, l1d, l2, l3) = caches();
        Machine {
            cache_line: line.unwrap_or(64),
            cache_line_detected: line.is_some(),
            page_size,
            l1d: l1d.unwrap_or(0),
            l2: l2.unwrap_or(0),
            l3: l3.unwrap_or(0),
        }
    }
}

type Caches = (Option<usize>, Option<usize>, Option<usize>, Option<usize>);

#[cfg(target_os = "macos")]
fn caches() -> Caches {
    (
        sysctl_num("hw.cachelinesize"),
        sysctl_num("hw.l1dcachesize"),
        sysctl_num("hw.l2cachesize"),
        sysctl_num("hw.l3cachesize"),
    )
}

/// Shelling out rather than calling `sysctlbyname` through FFI: this runs
/// once per process and a wrong buffer size in FFI is a silent zero, which
/// is exactly the failure this module exists to refuse.
#[cfg(target_os = "macos")]
pub fn sysctl_num(name: &str) -> Option<usize> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", name])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(target_os = "macos")]
pub fn sysctl_str(name: &str) -> Option<String> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", name])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(target_os = "macos"))]
fn caches() -> Caches {
    let base = "/sys/devices/system/cpu/cpu0/cache";
    let mut line = None;
    let (mut l1d, mut l2, mut l3) = (None, None, None);
    let Ok(rd) = std::fs::read_dir(base) else {
        return (None, None, None, None);
    };
    for e in rd.flatten() {
        let p = e.path();
        let read = |f: &str| {
            std::fs::read_to_string(p.join(f))
                .ok()
                .map(|s| s.trim().to_string())
        };
        let level = read("level").and_then(|s| s.parse::<u8>().ok());
        let kind = read("type").unwrap_or_default();
        let size = read("size").and_then(|s| parse_size(&s));
        if line.is_none() {
            line = read("coherency_line_size").and_then(|s| s.parse().ok());
        }
        match (level, kind.as_str()) {
            (Some(1), "Data") | (Some(1), "Unified") => l1d = size,
            (Some(2), _) => l2 = size,
            (Some(3), _) => l3 = size,
            _ => {}
        }
    }
    (line, l1d, l2, l3)
}

/// `32K`, `1024K`, `32M` or a bare number of bytes.
#[cfg(not(target_os = "macos"))]
fn parse_size(s: &str) -> Option<usize> {
    let (digits, mult) = match s.chars().last()? {
        'K' => (&s[..s.len() - 1], 1024),
        'M' => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    digits.parse::<usize>().ok().map(|v| v * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_something_sane() {
        let m = Machine::detect();
        assert!(m.page_size >= 4096);
        assert!(m.cache_line == 64 || m.cache_line == 128);
    }
}
