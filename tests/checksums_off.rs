//! One test, alone in its process on purpose.
//!
//! `block::CHECKSUMS` is process-wide and write-time: `Db::create` stores it
//! from the store's options, and every segment written in the process after
//! that carries checksums or not accordingly. Cargo runs the tests in a file
//! as threads of one process, so while the store below is being written with
//! checksums OFF, any other test flushing at the same moment writes blocks
//! with zero checksums -- and then reads them back with verification on,
//! because its own options say so. That is a compaction failing with
//! "compact read: block checksum mismatch" in a test that never touched the
//! switch, and it happened twice in about twenty gate runs, never twice in a
//! row, never alone.
//!
//! The test itself is unchanged from `tests/db.rs`. What moved is the
//! process boundary.

use std::path::PathBuf;
use supdb::{Db, Options};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read_vec(db: &Db, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    db.read_all(key, |v| out.push(v.to_vec())).unwrap();
    out
}

/// A store written with checksums off must read back in a process that did
/// not write it. The switch is process-wide and only the segment writer set
/// it, so a reader that had written nothing -- or had last written some
/// other store -- verified the zeroes this one stored on purpose and
/// refused every run whose values reached a block. A run under
/// `inline_bytes` lives in the index record and reads no block at all,
/// which is why short values hid it: with the suite's value sizes about one
/// run in a hundred spilled, and it read as scattered damage rather than as
/// a wrong setting. The runs here all spill. The second store is what puts
/// the writer's switch back the way a fresh process has it; without it this
/// test passes for the wrong reason.
#[test]
fn a_store_written_without_checksums_reads_back_under_another_store_s_setting() {
    let off = Options {
        segment: supdb::SegmentOptions {
            checksums: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let quiet = dir("ck-off");
    let mut db = Db::create(&quiet, off.clone()).unwrap();
    for k in 0u32..4_000 {
        let key = format!("key-{k:08}").into_bytes();
        for v in 0u32..8 {
            // Over `inline_bytes` for the run, so the values live in a block
            // and the read has a checksum to disagree about.
            db.append(
                &key,
                format!("value-{k:08}-{v}-{}", "p".repeat(64)).as_bytes(),
            );
        }
        if k % 500 == 0 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    drop(db);

    let loud = dir("ck-on");
    let mut other = Db::create(&loud, Options::default()).unwrap();
    other.append(b"k", b"v");
    other.commit().unwrap();
    other.flush().unwrap();
    drop(other);

    let db = Db::open(&quiet, off).unwrap();
    for k in 0u32..4_000 {
        let key = format!("key-{k:08}").into_bytes();
        assert_eq!(read_vec(&db, &key).len(), 8, "values for {k}");
    }
    let mut seen = 0usize;
    db.scan(b"", usize::MAX, |_k, _v| seen += 1).unwrap();
    assert_eq!(seen, 4_000 * 8, "every value scans");
}
