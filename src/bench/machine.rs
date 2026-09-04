//! The machine's shape, detected rather than assumed.
//!
//! Every tuning constant in a layout is really a formula over two or three
//! numbers: how big a cache line is, how big a page is, and how much cache
//! there is. Compiling those in as literals is what makes a structure fast on
//! the machine it was tuned on and mediocre elsewhere.
//!
//! The distinction that matters, and which the measurements so far support:
//!
//! * **Structural choices appear to be machine-invariant.** Flat records beat
//!   pointer-chasing because they cost one fewer dependent load and no per-key
//!   allocation; fixed-width beats varint on a hot path because a branchy
//!   serial decode is compute, not memory. Neither mechanism depends on the
//!   cache geometry, and both would be expected to hold anywhere -- the varint
//!   one arguably more strongly on a machine with a weaker branch predictor.
//! * **Granularity choices are not.** How many records share a prefix, how
//!   many fit in a page, how large a compression chunk should be -- these are
//!   all sized against a cache line or a memory page, and those differ by a
//!   factor of two and four respectively between x86, Graviton and Apple
//!   Silicon.
//!
//! So the hope of one implementation that is near-optimal everywhere is
//! reasonable, provided the granularity constants are derived at runtime from
//! the numbers below rather than written down. Whether a single derivation
//! actually lands near the empirical optimum on every shape is a question for
//! measurement, not for argument: `indexlab sweep` exists to answer it.

use super::J;
use crate::jobj;

/// Read one integer from `sysctl`, for platforms without `/sys`.
///
/// Shelling out rather than calling `sysctlbyname` through FFI: this cannot be
/// compiled or tested from the Linux machine it was written on, and a wrong
/// FFI signature fails in ways a wrong command does not.
#[cfg(target_os = "macos")]
fn sysctl_num(name: &str) -> Option<usize> {
    let out = std::process::Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(target_os = "macos"))]
fn sysctl_num(_name: &str) -> Option<usize> {
    None
}

/// Override detection. The escape hatch for a platform whose values are not
/// read correctly -- which is better than deriving a tuning constant from a
/// default that happens to be wrong.
fn env_num(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}

#[derive(Clone, Copy, Debug)]
pub struct Machine {
    /// Bytes per cache line. 64 on x86-64 and Graviton, 128 on Apple Silicon.
    pub cache_line: usize,
    /// Bytes per page. 4 KiB on x86-64 and Graviton, 16 KiB on Apple Silicon.
    pub page_size: usize,
    pub l1d: usize,
    pub l2: usize,
    pub l3: usize,
    /// False when the cache line size was defaulted rather than read. A
    /// derived constant built on a guessed line size is not a measurement, and
    /// on Apple Silicon the guess is wrong by a factor of two.
    pub cache_line_detected: bool,
}

fn sysfs_num(path: &str) -> Option<usize> {
    let s = std::fs::read_to_string(path).ok()?;
    let t = s.trim();
    let (digits, mult) = match t.chars().last() {
        Some('K') => (&t[..t.len() - 1], 1024),
        Some('M') => (&t[..t.len() - 1], 1024 * 1024),
        _ => (t, 1),
    };
    digits.parse::<usize>().ok().map(|v| v * mult)
}

impl Machine {
    pub fn detect() -> Machine {
        let page_size = env_num("SUPDB_PAGE_SIZE")
            .or_else(|| Some(unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }))
            .unwrap_or(4096)
            .max(4096);
        // Linux exposes this under /sys; macOS under sysctl. Neither is
        // present on the other, and defaulting silently is how Apple Silicon
        // gets tuned as though it had 64-byte lines.
        let line = env_num("SUPDB_CACHE_LINE")
            .or_else(|| sysfs_num("/sys/devices/system/cpu/cpu0/cache/index0/coherency_line_size"))
            .or_else(|| sysctl_num("hw.cachelinesize"));
        let mut m = Machine {
            cache_line: line.unwrap_or(64),
            cache_line_detected: line.is_some(),
            page_size,
            l1d: 0,
            l2: 0,
            l3: 0,
        };
        // Apple Silicon reports per-performance-level caches; the P-core
        // figures are the relevant ones for a latency-sensitive path.
        m.l1d = sysctl_num("hw.perflevel0.l1dcachesize")
            .or_else(|| sysctl_num("hw.l1dcachesize"))
            .unwrap_or(0);
        m.l2 = sysctl_num("hw.perflevel0.l2cachesize")
            .or_else(|| sysctl_num("hw.l2cachesize"))
            .unwrap_or(0);
        m.l3 = sysctl_num("hw.l3cachesize").unwrap_or(0);
        for i in 0..8 {
            let base = format!("/sys/devices/system/cpu/cpu0/cache/index{i}");
            let Some(size) = sysfs_num(&format!("{base}/size")) else {
                continue;
            };
            let level = sysfs_num(&format!("{base}/level")).unwrap_or(0);
            let kind = std::fs::read_to_string(format!("{base}/type")).unwrap_or_default();
            match (level, kind.trim()) {
                (1, "Data") | (1, "Unified") => m.l1d = m.l1d.max(size),
                (2, _) => m.l2 = m.l2.max(size),
                (3, _) => m.l3 = m.l3.max(size),
                _ => {}
            }
        }
        // A guest may expose no cache topology at all; keep plausible defaults
        // so a derived constant stays sane rather than collapsing to zero.
        if m.l1d == 0 {
            m.l1d = 32 * 1024;
        }
        if m.l2 == 0 {
            m.l2 = 1024 * 1024;
        }
        if m.l3 == 0 {
            m.l3 = 8 * 1024 * 1024;
        }
        m
    }

    /// Records per page in a paged index layout.
    ///
    /// Derived so the page's slot directory occupies one cache line: a lookup
    /// reads the header, one slot, and one record, and the slot it wants is on
    /// the line it already fetched. Two bytes per slot, so the count scales
    /// directly with the line size -- 32 at 64 bytes, 64 at 128.
    ///
    /// This is a hypothesis with a mechanism, not a measured optimum. See
    /// `indexlab sweep`, which searches the parameter directly, and the
    /// `derived_is_near_optimal` finding that compares the two.
    pub fn records_per_page(&self) -> usize {
        (self.cache_line / 2).clamp(16, 256)
    }

    /// Records sharing a prefix between restart points, where records are
    /// decoded sequentially from the restart. One cache line's worth, since
    /// the scan is what the group costs.
    pub fn restart_group(&self) -> usize {
        (self.cache_line / 4).clamp(4, 64)
    }

    pub fn to_json(&self) -> J {
        jobj! {
            "cache_line" => J::u(self.cache_line as u64),
            "page_size" => J::u(self.page_size as u64),
            "l1d" => J::u(self.l1d as u64),
            "l2" => J::u(self.l2 as u64),
            "l3" => J::u(self.l3 as u64),
            "derived_records_per_page" => J::u(self.records_per_page() as u64),
            "derived_restart_group" => J::u(self.restart_group() as u64),
            "cache_line_detected" => J::Bool(self.cache_line_detected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_plausible() {
        let m = Machine::detect();
        assert!(m.cache_line.is_power_of_two(), "line {}", m.cache_line);
        assert!((32..=256).contains(&m.cache_line));
        assert!(m.page_size >= 4096 && m.page_size.is_power_of_two());
        assert!(m.l1d >= 8 * 1024);
    }

    /// The derived constants must stay sane on machines this was not written
    /// on -- including ones that expose no cache topology at all.
    #[test]
    fn derived_constants_are_bounded_for_every_plausible_shape() {
        for line in [32usize, 64, 128, 256] {
            for page in [4096usize, 16384, 65536] {
                let m = Machine {
                    cache_line: line,
                    page_size: page,
                    l1d: 32 << 10,
                    l2: 1 << 20,
                    l3: 8 << 20,
                    cache_line_detected: true,
                };
                let rpp = m.records_per_page();
                let rg = m.restart_group();
                assert!((16..=256).contains(&rpp), "line {line}: rpp {rpp}");
                assert!((4..=64).contains(&rg), "line {line}: rg {rg}");
                // A slot directory must never exceed the line it is meant to fit in
                // by more than a factor of two, or the derivation has lost its point.
                assert!(rpp * 2 <= line.max(64) * 2);
            }
        }
    }

    #[test]
    fn apple_silicon_shape_derives_larger_groups_than_x86() {
        let x86 = Machine {
            cache_line: 64,
            page_size: 4096,
            l1d: 48 << 10,
            l2: 1 << 20,
            l3: 32 << 20,
            cache_line_detected: true,
        };
        let m1 = Machine {
            cache_line: 128,
            page_size: 16384,
            l1d: 128 << 10,
            l2: 4 << 20,
            l3: 8 << 20,
            cache_line_detected: true,
        };
        assert!(m1.records_per_page() > x86.records_per_page());
        assert!(m1.restart_group() > x86.restart_group());
    }
}
