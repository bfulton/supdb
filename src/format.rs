//! The on-disk format's fixed quantities.
//!
//! These are properties of a supdb file, not of whatever wrote it. They live
//! apart from any writer because two of them exist -- a segment writer and,
//! historically, a store -- and three readers parse what either produced.
//! `blob.rs` keeps its own copies and asserts they match these, which is why
//! a drift between a writer and a reader has never shipped.

/// The superblock magic, and the version gate.
///
/// Every scalar in a supdb file goes to disk little-endian, but the two
/// structures that make the format fast are addressed in place regardless:
/// `flatindex` hands back `&[Ext]` borrowed out of the mapping, and a block
/// table's records are reinterpreted rather than decoded. So a file is
/// self-consistent only on the byte order that wrote it.
///
/// Two things refuse, in different directions, and it is worth naming both
/// because each looks incomplete alone. The magic goes to disk `to_ne_bytes`
/// while `Blob` reads it back `from_le_bytes`, and that asymmetry is the
/// byte-order mark: a file written big-endian reads back byte-swapped on a
/// little-endian machine and fails the comparison. The other direction never
/// reaches the comparison, because `Blob::open` refuses a big-endian target
/// outright -- the index is addressed in place, so a big-endian reader would
/// misread a valid little-endian file rather than reject it.
///
/// The low bytes are the format version, and a reader from before a version
/// refuses a file rather than misreading it. 0004 when redo-log frames grew a
/// kind byte; 0005 when an extent's byte length became a count and the top bit
/// became the tombstone flag, which also brought inline runs; 0006 when bit 30
/// of that word became `FIXED` and a run of one width lost its per-value
/// length prefixes.
pub(crate) const MAGIC: u64 = 0x5355_5044_4200_0006;

/// The superblock page: two slots in the first sector-pair of the file, which
/// a writer alternates between.
///
/// Alternating slots make publishing a new state atomic in the way that
/// matters -- a torn write can damage at most the slot being written, and the
/// other still describes a complete, older state. Recovery picks the valid
/// slot with the higher generation.
pub(crate) const SUPER: u64 = 4096;

/// The stride between the two superblock slots.
pub(crate) const SLOT: u64 = 512;

/// Encoded size of a superblock: the fields, then the magic, then the
/// checksum.
///
/// Named because eight call sites used to slice the literal, and adding two
/// fields left every one of them reading a prefix that no longer contained
/// the checksum -- a format change that presented itself as "no valid supdb
/// checkpoint" on a healthy file.
pub(crate) const SUPER_BYTES: usize = 144;

/// Sub-phases of `flatindex::encode`, printed under `SUPDB_CKPT_PHASES`.
pub(crate) fn enc_phase(what: &str, t: std::time::Instant) {
    if std::env::var_os("SUPDB_CKPT_PHASES").is_some() {
        eprintln!("      enc:{what} {:.4}s", t.elapsed().as_secs_f64());
    }
}
