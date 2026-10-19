//! What the integration tests share.
//!
//! Every test binary that names this module compiles the whole of it and
//! uses a part, so each item is dead in some binary; the allowance is for
//! that and nothing else.
#![allow(dead_code)]

/// A segment file without the part of it that is a function of when it was
/// written.
///
/// The superblock records `SystemTime::now()` in seconds as its third field,
/// and the FNV-1a over the fields that follows it covers that second, in each
/// of the two slots. So two writes of identical input differ in eighteen-odd
/// bytes whenever they straddle a tick, and comparing whole files is a test
/// that passes on a fast machine and fails on a slow one -- which is what it
/// did: green here, red on the macOS runner, on the arm of a loop where
/// nothing about the setting under test touches a byte.
///
/// Everything else is compared, including the superblock's offsets, which are
/// what a mis-applied setting would actually move. Only a segment has the
/// superblock; the caller hands this nothing else.
pub fn without_the_clock(bytes: &[u8]) -> Vec<u8> {
    const SLOT: usize = 512;
    const TS: usize = 16;
    const FNV: usize = 136;
    let mut out = bytes.to_vec();
    for slot in [0, SLOT] {
        for at in [slot + TS, slot + FNV] {
            out[at..at + 8].fill(0);
        }
    }
    out
}

/// The store's segment files by name, in name order.
pub fn seg_names(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    v.sort();
    v
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// The live partitions' fences, from their names: `(lo, hi)`, an open
/// fence empty, in key order.
pub fn partition_fences(d: &std::path::Path) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let mut v: Vec<(Vec<u8>, Option<Vec<u8>>)> = seg_names(d)
        .iter()
        .filter(|n| n.starts_with("par-"))
        .map(|n| {
            let stem = n.trim_end_matches(".sup");
            let f: Vec<&str> = stem.split('-').collect();
            let hi = if f[4].is_empty() {
                None
            } else {
                Some(unhex(f[4]))
            };
            (unhex(f[3]), hi)
        })
        .collect();
    v.sort();
    v
}

/// The mappings this process holds of sidecar files under `dir`, as
/// `/proc/self/maps` names them: a renamed file shows under its new name,
/// an unlinked one with ` (deleted)`. One line a mapping, deduplicated by
/// the file the line names, since a mapping may be split into several.
#[cfg(target_os = "linux")]
pub fn zpg_mappings(dir: &std::path::Path) -> Vec<String> {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let prefix = dir.join("zpg-");
    let prefix = prefix.to_string_lossy();
    let mut v: Vec<String> = maps
        .lines()
        .filter_map(|l| l.split_whitespace().nth(5))
        .filter(|p| p.starts_with(&*prefix))
        .map(str::to_owned)
        .collect();
    v.sort();
    v.dedup();
    v
}
