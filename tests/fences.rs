//! The merge's fences hold what it writes.
//!
//! A merge cuts its output into pieces by fence, and refuses a key that
//! would land outside the fence of the piece it is written to, since the
//! read path routes by the fence and would deny the key without raising.
//! The refusal stops the store's segment work for good, so a cut that puts
//! a key outside its own fence is a store that stops. These tests build
//! the key shapes that make such a cut and hold the merge to its fences.

use std::path::PathBuf;
use supdb::{Db, Options};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-fences-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Every key reads back its one value through the writer, and a scan of
/// the whole store lists the keys in order, once each.
fn hold_to(db: &Db, keys: &[Vec<u8>]) {
    for k in keys {
        let mut n = 0;
        db.read_all(k, |_| n += 1)
            .unwrap_or_else(|e| panic!("read of {:?}: {e}", String::from_utf8_lossy(k)));
        assert_eq!(n, 1, "key {:?}", String::from_utf8_lossy(k));
    }
    let mut seen: Vec<Vec<u8>> = Vec::new();
    db.scan(b"", usize::MAX, |k, _| {
        if seen.last().map(|l| l.as_slice()) != Some(k) {
            seen.push(k.to_vec());
        }
    })
    .unwrap();
    let mut want = keys.to_vec();
    want.sort();
    assert_eq!(seen, want, "the scan's keys");
}

/// The first partitioning derives its bounds from the keys at even ranks,
/// cut to the fence's length, so a run of keys longer than a fence that
/// share their first sixteen bytes gives a bound at or below the run's
/// first key, which cuts nothing. The pieces that follow must still carry
/// the fences of the slices they hold.
#[test]
fn the_first_partitioning_holds_its_fences_when_a_bound_cuts_nothing() {
    let dir = scratch("first-bound-cuts-nothing");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: true,
            publish_in_background: false,
            ..Options::default()
        },
    )
    .unwrap();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for i in 0..300 {
        keys.push(format!("a-very-long-common-prefix/{i:05}").into_bytes());
    }
    for i in 0..300 {
        keys.push(format!("z{i:05}").into_bytes());
    }
    let value = [b'v'; 100];
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    hold_to(&db, &keys);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A partition merge splits a fence that holds more keys than a piece
/// should into sub-pieces, and names each join by a key cut to the
/// fence's length. Keys longer than a fence that share their first
/// sixteen bytes cannot be told apart by such a join, so the join must
/// fall where the cut still separates its two sides, or not at all.
#[test]
fn a_partition_rewrite_joins_its_sub_pieces_where_a_fence_can_separate_them() {
    let dir = scratch("sub-pieces-long-prefix");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: true,
            publish_in_background: false,
            l0_trigger: 2,
            ..Options::default()
        },
    )
    .unwrap();
    // More keys than a piece holds (a thousand at least), all sharing
    // sixteen bytes, beside a few that do not.
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for i in 0..1200 {
        keys.push(format!("a-very-long-common-prefix/{i:05}").into_bytes());
    }
    for i in 0..20 {
        keys.push(format!("z{i:05}").into_bytes());
    }
    let value = [b'v'; 100];
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    hold_to(&db, &keys);
    // Enough pieces over the range to have it rewritten.
    for round in 0..2u8 {
        for k in &keys[..1200] {
            db.put(k, &[round; 100]);
        }
        db.commit().unwrap();
        db.seal().unwrap();
    }
    db.settle().unwrap();
    hold_to(&db, &keys);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
