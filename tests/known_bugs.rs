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

/// A mapped index found nothing, while reporting the right number of keys.
///
/// `flatindex` hands back `&[Ext]` borrowed out of the mapping, so those
/// extents have to be aligned at their absolute address. The records were laid
/// out 4-aligned *within the section*, and the section was written wherever
/// the appender happened to be -- so whether the layout was actually aligned
/// depended on how many bytes preceded it. `record()` refused to build an
/// unaligned borrow and returned `None`, which meant every lookup missed and
/// every scan came back empty while `keys()`, which reads only the header,
/// stayed correct.
///
/// It presented as a scale bug and was not one: at a fixed key count it came
/// and went with the number of checkpoints. The fix aligns the section in the
/// file. This test varies both axes because either alone would have passed.
///
/// It compares the two index arms against each other rather than against an
/// expected count. That is the stronger assertion -- the arms must be
/// indistinguishable through the public API -- and it does not bake in a
/// belief about what `read_all` returns, which is bytes rather than values.
#[test]
fn a_mapped_index_agrees_with_the_decoded_one_at_every_alignment() {
    fn harvest(file: &std::path::Path, n: u64) -> (usize, Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let r = supdb::Reader::open(file).unwrap();
        let mut values = Vec::new();
        for i in 0..n {
            r.read_all(format!("k{i:012}").as_bytes(), |v| values.push(v.to_vec()))
                .unwrap();
        }
        let mut scanned = Vec::new();
        r.scan(None, n as usize, |k, _| scanned.push(k.to_vec()))
            .unwrap();
        (r.keys(), values, scanned)
    }

    for &n in &[1_000u64, 5_000, 20_000] {
        for extra_checkpoints in 0..4 {
            let mut arms = Vec::new();
            for flat in [false, true] {
                let dir = std::env::temp_dir()
                    .join(format!("supdb-kb-flatalign-{n}-{extra_checkpoints}-{flat}"));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                let file = dir.join("s.dat");
                {
                    let store = supdb::Store::create(
                        &file,
                        supdb::Options {
                            buffer_bytes: 64 << 20,
                            flat_index: flat,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    for i in 0..n {
                        store
                            .append(format!("k{i:012}").as_bytes(), b"a value worth finding")
                            .unwrap();
                    }
                    for _ in 0..extra_checkpoints {
                        store.checkpoint().unwrap();
                    }
                    store.close().unwrap();
                }
                arms.push((dir, harvest(&file, n)));
            }
            let (heap_keys, heap_vals, heap_scan) = &arms[0].1;
            let (flat_keys, flat_vals, flat_scan) = &arms[1].1;
            let at = format!("n={n} checkpoints={extra_checkpoints}");
            assert_eq!(heap_keys, flat_keys, "{at}: key counts differ");
            assert_eq!(*heap_keys as u64, n, "{at}: keys lost before the read");
            assert_eq!(
                heap_vals.len() as u64,
                n,
                "{at}: the decoded arm lost values"
            );
            assert_eq!(heap_vals, flat_vals, "{at}: the mapped arm lost values");
            assert_eq!(
                heap_scan, flat_scan,
                "{at}: the mapped arm lost an ordered scan"
            );
            for (d, _) in arms {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
}

/// Every checkpoint appended three index sections and freed none of them.
///
/// The keys, blocks and reuse-log sections were written at the append cursor
/// and never handed back, so a store that checkpointed often grew without
/// bound in *checkpoint count* rather than in data. It went unnoticed because
/// the varint key section is small and compresses well; the flat index made
/// the same leak seven times more expensive and thereby visible.
///
/// The fix reuses freed section space and releases superseded sections once no
/// reader can still reach them, which is the same reuse floor blocks already
/// use. Growth must therefore stop, not merely slow.
#[test]
fn repeated_checkpoints_stop_growing_the_file() {
    for flat in [false, true] {
        let dir = std::env::temp_dir().join(format!("supdb-kb-ckptleak-{flat}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: 128 << 20,
                reclaim: supdb::Reclaim::AfterReads,
                flat_index: flat,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..20_000u64 {
            store
                .append(format!("k{i:012}").as_bytes(), &[7u8; 64])
                .unwrap();
        }
        let mut sizes = Vec::new();
        for _ in 0..8 {
            store.checkpoint().unwrap();
            sizes.push(std::fs::metadata(&file).unwrap().len());
        }
        // The first few may still grow while the free list warms; the tail
        // must be flat. A leak shows up as a constant positive step forever.
        let tail: Vec<i64> = sizes[4..]
            .windows(2)
            .map(|w| w[1] as i64 - w[0] as i64)
            .collect();
        assert!(
            tail.iter().all(|d| *d == 0),
            "flat={flat}: file still grows per checkpoint: {tail:?} (sizes {sizes:?})"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// An uncompressed block re-verified its whole checksum on every read.
///
/// The checksum covers the entire block, so a plain block read straight out of
/// the mapping paid O(block size) of CRC to hand back one value -- 64 KiB of
/// work for 100 bytes of data. An ordered scan measured 7985 ns per entry
/// against 26 with checking off: 307x.
///
/// It hid because the default configuration compresses, and a compressed block
/// is chunked, so that path verifies a kilobyte at a time. The visible effect
/// was that turning compression *off* appeared to make reads 87% slower, which
/// reads as "compression is free" rather than "the other path is broken".
///
/// A block a reader can see cannot be rewritten underneath it, so verifying
/// once per reader is sound. This asserts the cost is bounded rather than
/// asserting a wall-clock number: what must never come back is the shape,
/// where scan cost tracks block size instead of value size.
#[test]
fn an_uncompressed_block_is_not_rechecksummed_per_read() {
    use std::time::Instant;
    let scan_ns = |compress: bool, checksums: bool| -> f64 {
        let dir = std::env::temp_dir().join(format!("supdb-kb-crc-{compress}-{checksums}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: 128 << 20,
                reclaim: supdb::Reclaim::AfterReads,
                compress,
                checksums,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..60_000u64 {
            store
                .append(format!("k{i:012}").as_bytes(), &[7u8; 100])
                .unwrap();
        }
        store.close().unwrap();
        let r = supdb::Reader::open(&file).unwrap();
        let t = Instant::now();
        let n = r
            .scan(None, 20_000, |_, v| {
                std::hint::black_box(v);
            })
            .unwrap();
        let ns = t.elapsed().as_secs_f64() * 1e9 / n as f64;
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
        ns
    };
    let with = scan_ns(false, true);
    let without = scan_ns(false, false);
    // Generous: the defect was 307x. Anything near that is the bug returning,
    // and a loose bound survives a noisy machine where a tight one would not.
    assert!(
        with < without * 20.0,
        "checksums cost {:.0}x on an uncompressed scan ({with:.0} ns/entry against \
         {without:.0}); the per-read whole-block CRC is back",
        with / without.max(1e-9)
    );
}
