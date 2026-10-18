//! What the integration tests share.

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
