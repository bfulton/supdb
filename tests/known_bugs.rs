//! Reduced reproducers for bugs the suites found.
//!
//! Each was a real defect in the engine, found by a randomized or adversarial
//! experiment and then reduced to the smallest deterministic form that still
//! showed it. A reproducer stays here after its bug is fixed: it is the
//! regression test, and it is worth more than a test written from the fix,
//! because it is written from the failure.
//!
//! A reproducer for a bug that is still open carries `#[ignore]` with the
//! reason, so `cargo test` stays green on a known-broken engine while
//! `cargo test --test known_bugs -- --ignored` still shows it. When a bug is
//! fixed, remove the `#[ignore]` and flip its `claims.json` entry from `fails`
//! to `holds` in the same commit.

use supdb::{Options, Reader, Reclaim, Store};

/// A delete is lost if it lands between `seal_shard` and `flush_builder`.
///
/// `append` calls `seal_shard` inline once the shard's pending buffer fills.
/// That stages the extent in the block builder and records it in
/// `Shard::members`, but the block has no id yet, so the key's `extents` are
/// not updated until `flush_builder` runs -- which may be many operations
/// later, or not until the next checkpoint.
///
/// `delete` clears `entry.extents`. It knows nothing about the staged member,
/// so when `flush_builder` finally runs it does `entry.extents.push(ext)` and
/// the deleted key comes back with every value it had.
///
/// Found by the differential oracle (`correctness c2-oracle`), which reported
/// 3,048 divergences over 60 rounds; this is the minimal deterministic form.
///
/// The fix has to make a staged member cancellable: either `delete` and `put`
/// drop matching entries from `Shard::members`, or each entry carries a
/// version that `flush_builder` re-checks before pushing.
/// FIXED: `delete` and `put` now drop matching entries from `Shard::members`.
#[test]
fn delete_is_not_undone_by_a_staged_extent() {
    let dir = std::env::temp_dir().join("supdb-test-delete-resurrection");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.dat");

    let store = Store::create(
        &path,
        Options {
            // One shard, and a buffer small enough that a few appends force a
            // seal without filling the block builder.
            shards: 1,
            buffer_bytes: 2048,
            block_size: 1 << 20,
            reclaim: Reclaim::Never,
            ..Default::default()
        },
    )
    .unwrap();

    let key = b"the-key";
    let value = vec![b'v'; 256];
    for _ in 0..16 {
        store.append(key, &value).unwrap();
    }
    // The extent is now staged in the builder with no block id yet.
    store.delete(key).unwrap();
    // checkpoint -> flush -> flush_builder, which pushes the staged extent.
    store.checkpoint().unwrap();
    store.close().unwrap();

    let reader = Reader::open(&path).unwrap();
    let mut seen = 0usize;
    reader.read_all(key, |_| seen += 1).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(seen, 0, "deleted key came back with {seen} value(s)");
}
