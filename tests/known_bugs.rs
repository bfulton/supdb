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

use supdb::{Options, ReadOptions, Reader, Reclaim, Store};

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
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
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
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
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
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
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
/// Serialises tests that depend on `Options::checksums`.
///
/// That option is process-global, not per-store: `Store::create` writes a
/// static atomic that every reader in the process then consults. Two stores
/// with different settings cannot coexist, and a test that creates one flips
/// the flag under any other test running concurrently. This is a real
/// limitation of the engine rather than of the tests -- an embedded library
/// whose configuration is process-wide is surprising -- and it is recorded in
/// docs/architecture-review.md. The lock is what keeps the suite honest until
/// the option becomes per-store.
static CHECKSUM_FLAG: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn an_uncompressed_block_is_not_rechecksummed_per_read() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
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

/// A checkpoint-heavy workload must reach a steady file size, not grow forever.
///
/// `repeated_checkpoints_stop_growing_the_file` holds no reader open, which
/// lets the reuse floor advance freely. A real client does the opposite: the
/// external YCSB adapter checkpoints and then keeps its `Reader` alive, which
/// pins the floor. This is that shape.
///
/// It also records the price of the mapped index in the one place it is
/// highest. The steady state is roughly six times the file, because every
/// checkpoint writes a whole uncompressed index and only the sections below
/// the floor come back. f11 measures +73% on a minimal file; this is what the
/// same trade costs when checkpoints are frequent, and it is why a YCSB run
/// grew a 22 GB store and filled the disk.
#[test]
fn a_held_reader_does_not_stop_reclaim() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let plateau = |flat: bool| -> u64 {
        let dir = std::env::temp_dir().join(format!("supdb-kb-held-{flat}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let s = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: 64 << 20,
                reclaim: supdb::Reclaim::AfterReads,
                flat_index: flat,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..100_000u64 {
            s.append(format!("k{i:012}").as_bytes(), &[7u8; 100])
                .unwrap();
        }
        let mut held = None;
        let mut sizes = Vec::new();
        for _ in 0..8 {
            s.checkpoint().unwrap();
            held = Some(supdb::Reader::open(&file).unwrap());
            sizes.push(std::fs::metadata(&file).unwrap().len());
        }
        drop(held);
        let tail: Vec<i64> = sizes[5..]
            .windows(2)
            .map(|w| w[1] as i64 - w[0] as i64)
            .collect();
        assert!(
            tail.iter().all(|d| *d == 0),
            "flat={flat}: still growing with a reader held open: {tail:?}"
        );
        let last = *sizes.last().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        last
    };
    let off = plateau(false);
    let on = plateau(true);
    // The mapped index is dearer here than anywhere else. Bounded, and the
    // bound is what matters -- but it is a bound worth knowing about.
    assert!(
        on < off * 12,
        "the mapped index costs {:.1}x the file under frequent checkpoints ({on} vs {off}); \
         it was about 6x when this was written",
        on as f64 / off as f64
    );
}

/// A checkpoint must cost what changed, not what is stored.
///
/// checkpoint() rewrote the whole key index every time, so publishing a
/// hundred updates against a two-million-key store cost the same as
/// publishing the store. That is the floor under every read-your-writes
/// workload: the external YCSB adapter checkpoints after each write batch,
/// which is why its mixed workloads run at a hundredth of LMDB's rate while
/// its read-only workload is competitive.
///
/// Two changes were needed and neither sufficed alone. The key index is
/// published in place -- new records into reserved slack, one aligned store
/// per slot -- so nothing is rewritten. And fsync is separated from
/// publishing, because it dominated whatever was left: 38ms against 0.32ms at
/// two million keys.
///
/// Asserts the shape rather than a wall-clock number: cost must not scale
/// with the key count. A twenty-fold increase in keys may not cost anything
/// like twenty times more.
#[test]
fn a_checkpoint_costs_what_changed_not_what_is_stored() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    use std::time::Instant;
    let cost = |keys: u64| -> f64 {
        let dir = std::env::temp_dir().join(format!("supdb-kb-inc-{keys}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let s = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: 512 << 20,
                reclaim: supdb::Reclaim::AfterReads,
                sync: supdb::Sync::Never,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..keys {
            s.append(format!("k{i:012}").as_bytes(), &[7u8; 100])
                .unwrap();
        }
        s.checkpoint().unwrap();
        let mut best = f64::MAX;
        for r in 0..6u64 {
            for i in 0..100u64 {
                s.append(
                    format!("k{:012}", (i * 7 + r) % keys).as_bytes(),
                    &[9u8; 100],
                )
                .unwrap();
            }
            let t = Instant::now();
            s.checkpoint().unwrap();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        // The values must survive all of it.
        let rd = supdb::Reader::open(&file).unwrap();
        assert_eq!(rd.keys(), keys as usize);
        let mut bytes = 0u64;
        for i in 0..keys.min(2000) {
            bytes += rd.read_all(format!("k{i:012}").as_bytes(), |_| {}).unwrap();
        }
        assert!(
            bytes >= keys.min(2000) * 100,
            "keys={keys}: lost values, {bytes} bytes for {} keys",
            keys.min(2000)
        );
        drop(rd);
        let _ = std::fs::remove_dir_all(&dir);
        best
    };
    let small = cost(100_000);
    let large = cost(2_000_000);
    // 20x the keys. A full rewrite showed roughly 20x the cost; publishing
    // only what changed showed about 4x, most of which is the block table.
    // 8x is loose enough for a noisy machine and tight enough to catch a
    // return to rewriting.
    assert!(
        large < small * 8.0,
        "checkpoint cost is tracking the key count again: {small:.2}ms at 100k against \
         {large:.2}ms at 2M ({:.1}x for 20x the keys)",
        large / small.max(1e-9)
    );
}

/// Two mechanisms both released a superseded key index section.
///
/// A full checkpoint recorded its key section in `index_history`. When a later
/// checkpoint replaced it, one path released it directly; the pruning loop
/// released it again once `live_key_off` had moved on. The free list then held
/// one range twice and handed it to two different blocks -- and in the
/// reproducing run a data block landed exactly on the live key index.
///
/// It only bit with the mapped index because that section is about 57 bytes
/// per key against the varint format's 9, so the space it frees is large
/// enough for a data block to want. The defect was in the release bookkeeping,
/// not in the format.
///
/// c2-oracle is the general version of this; this is the reduction, and it
/// asserts the property that actually matters: a range handed out by the free
/// list is never a range something else is still using.
#[test]
fn a_superseded_index_section_is_released_exactly_once() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    for flat in [false, true] {
        let dir = std::env::temp_dir().join(format!("supdb-kb-dbl-{flat}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: 4 << 20,
                // The only policy that hands freed space back out, and the
                // default. Never and OnClose never reproduced this.
                reclaim: supdb::Reclaim::AfterReads,
                flat_index: flat,
                ..Default::default()
            },
        )
        .unwrap();
        let mut model: std::collections::BTreeMap<String, Vec<Vec<u8>>> =
            std::collections::BTreeMap::new();
        // Enough rounds for the reuse floor to pass a section that is still
        // live: the original failure needed nine.
        for round in 0..14u64 {
            for i in 0..800u64 {
                let k = format!("k{:06}", i % 200);
                let v = vec![(round as u8).wrapping_mul(7).wrapping_add(i as u8); 48];
                if i % 5 == 0 {
                    store.put(k.as_bytes(), &v).unwrap();
                    model.insert(k, vec![v]);
                } else {
                    store.append(k.as_bytes(), &v).unwrap();
                    model.entry(k).or_default().push(v);
                }
            }
            store.checkpoint().unwrap();
            let r = supdb::Reader::open(&file).unwrap();
            for (k, want) in &model {
                let mut got: Vec<Vec<u8>> = Vec::new();
                r.read_all(k.as_bytes(), |v| got.push(v.to_vec()))
                    .unwrap_or_else(|e| panic!("flat={flat} round {round} key {k}: {e}"));
                assert_eq!(&got, want, "flat={flat} round {round} key {k}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `close` is durable whatever the sync policy says.
///
/// `Sync::Never` means "durable when I say so", and closing the store is
/// saying so. A clean shutdown that leaves acknowledged writes on the wrong
/// side of a power cut is not a policy, it is a bug -- and it is the obvious
/// way to get this wrong, because the policy is consulted in one place and
/// close goes through the same path.
#[test]
fn close_is_durable_under_every_sync_policy() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    for (name, policy) in [
        ("Always", supdb::Sync::Always),
        ("Never", supdb::Sync::Never),
        ("EveryN", supdb::Sync::EveryN(1000)),
        (
            "Interval",
            supdb::Sync::Interval(std::time::Duration::from_secs(3600)),
        ),
    ] {
        let dir = std::env::temp_dir().join(format!("supdb-kb-sync-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                buffer_bytes: 8 << 20,
                sync: policy,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..5_000u64 {
            store
                .append(format!("k{i:08}").as_bytes(), &[3u8; 40])
                .unwrap();
        }
        // Publish without persisting, then close. Everything must survive.
        store.publish().unwrap();
        store.close().unwrap();

        let r = supdb::Reader::open(&file).unwrap();
        assert_eq!(r.keys(), 5_000, "{name}: keys lost across close");
        for i in (0..5_000u64).step_by(97) {
            let mut n = 0usize;
            r.read_all(format!("k{i:08}").as_bytes(), |v| n += v.len())
                .unwrap();
            assert_eq!(n, 40, "{name}: key {i} lost across close");
        }
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `publish` makes writes visible without flushing; `sync` is what flushes.
///
/// The two are separate calls precisely because they are separate things, and
/// the failure this guards is a `publish` that quietly syncs anyway -- which
/// would be correct but would cost 31x and look like nothing was wrong.
#[test]
fn publish_makes_writes_visible_to_a_new_reader() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("supdb-kb-publish");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("s.dat");
    let store = supdb::Store::create(
        &file,
        supdb::Options {
            buffer_bytes: 8 << 20,
            sync: supdb::Sync::Never,
            ..Default::default()
        },
    )
    .unwrap();
    for round in 0..5u64 {
        for i in 0..500u64 {
            store
                .append(format!("k{i:08}").as_bytes(), &[round as u8; 16])
                .unwrap();
        }
        store.publish().unwrap();
        let r = supdb::Reader::open(&file).unwrap();
        let mut n = 0usize;
        r.read_all(b"k00000007", |v| n += v.len()).unwrap();
        assert_eq!(
            n,
            16 * (round as usize + 1),
            "round {round}: publish did not make the writes visible"
        );
    }
    // sync after the fact is a no-op for visibility and must not error.
    store.sync().unwrap();
    store.sync().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The writer can read its own writes without publishing them.
///
/// `Store` had no read method, so seeing your own write meant `checkpoint()`
/// plus a fresh `Reader`. LMDB needs neither, which is the whole of EXT.3 and
/// the reason the mixed YCSB workloads sit two orders of magnitude behind
/// while read-only sits ahead.
///
/// The hazard is ordering. A key's values live in up to three places at once
/// -- sealed extents, bytes staged in the block builder, and bytes still
/// pending against the key -- and a `put` marks its pending value as replacing
/// without clearing what it supersedes until the seal happens. Reading those
/// in the wrong order resurrects deleted values, which this repository has
/// already done once.
///
/// So this compares against a model at every step, with a small buffer so
/// seals and builder flushes happen mid-run rather than never.
#[test]
fn a_writer_reads_its_own_writes() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    for reclaim in [
        supdb::Reclaim::AfterReads,
        supdb::Reclaim::Never,
        supdb::Reclaim::OnClose,
    ] {
        let dir = std::env::temp_dir().join(format!("supdb-kb-ryow-{reclaim:?}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.dat");
        let store = supdb::Store::create(
            &file,
            supdb::Options {
                // Small, so sealing and flushing happen during the run and the
                // three-places-at-once case is actually exercised.
                buffer_bytes: 64 << 10,
                reclaim,
                ..Default::default()
            },
        )
        .unwrap();

        let mut model: std::collections::BTreeMap<String, Vec<Vec<u8>>> =
            std::collections::BTreeMap::new();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for step in 0..12_000u64 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = format!("k{:05}", x % 400);
            let v = vec![(step % 251) as u8; 20 + (step % 40) as usize];
            match x % 8 {
                0 => {
                    store.put(k.as_bytes(), &v).unwrap();
                    model.insert(k.clone(), vec![v]);
                }
                1 => {
                    store.delete(k.as_bytes()).unwrap();
                    model.remove(&k);
                }
                _ => {
                    store.append(k.as_bytes(), &v).unwrap();
                    model.entry(k.clone()).or_default().push(v);
                }
            }
            // Check the key just touched every time, and sweep the whole model
            // periodically -- the cheap check catches ordering, the sweep
            // catches a key that some other key's seal disturbed.
            let mut got: Vec<Vec<u8>> = Vec::new();
            store
                .read_all(k.as_bytes(), |b| got.push(b.to_vec()))
                .unwrap();
            assert_eq!(
                &got,
                model.get(&k).unwrap_or(&Vec::new()),
                "{reclaim:?} step {step}: writer's own view of {k} is wrong"
            );
            if step % 1000 == 0 {
                for (mk, want) in &model {
                    let mut mg: Vec<Vec<u8>> = Vec::new();
                    store
                        .read_all(mk.as_bytes(), |b| mg.push(b.to_vec()))
                        .unwrap();
                    assert_eq!(&mg, want, "{reclaim:?} step {step}: sweep found {mk} wrong");
                }
            }
        }

        // And what the writer saw must be what a reader sees after publishing.
        store.checkpoint().unwrap();
        let r = supdb::Reader::open(&file).unwrap();
        for (mk, want) in &model {
            let mut rg: Vec<Vec<u8>> = Vec::new();
            r.read_all(mk.as_bytes(), |b| rg.push(b.to_vec())).unwrap();
            assert_eq!(
                &rg, want,
                "{reclaim:?}: reader disagrees with the model on {mk}"
            );
        }
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A reader that would not map the block table read the wrong one.
///
/// The block table moved from a varint encoding to a flat one that a reader
/// borrows out of the mapping. The reader chose between them by trying the
/// flat parser and, on `None`, decoding the bytes as varints -- so "I cannot
/// map this" and "this is the older format" were the same branch. Any reason
/// to decline the mapping fed a flat section to the varint decoder, which read
/// it as a shorter table of plausible-looking blocks and answered a scan with
/// "extent names block 91 but the table has 83" on a store with nothing wrong
/// with it. A misparse that presents as corruption is the worst shape for one:
/// the error accuses the file.
///
/// FIXED: the format is decided by the section's magic, and the two decisions
/// -- which format, and whether to map it -- are separate. Both arms of
/// `ReadOptions::mapped_blocks` must now see the same store.
#[test]
fn a_reader_that_declines_the_mapping_still_reads_the_block_table() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("supdb-test-blocktable-arms");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bt.dat");
    let n = 50_000u64;
    let val = vec![b'v'; 100];
    {
        let s = Store::create(&path, Options::default()).unwrap();
        for i in 0..n {
            s.put(format!("{i:016}").as_bytes(), &val).unwrap();
        }
        s.flush().unwrap();
        s.close().unwrap();
    }
    let mut seen = Vec::new();
    for mapped_blocks in [true, false] {
        let r = Reader::open_with(
            &path,
            ReadOptions {
                mapped_blocks,
                ..Default::default()
            },
        )
        .unwrap();
        let mut bytes = 0u64;
        let got = r
            .scan(None, usize::MAX, |_k, v| bytes += v.len() as u64)
            .unwrap_or_else(|e| panic!("scan with mapped_blocks={mapped_blocks}: {e}"));
        assert_eq!(got, n, "mapped_blocks={mapped_blocks} walked {got} keys");
        seen.push((got, bytes));
    }
    assert_eq!(
        seen[0], seen[1],
        "the two block-table representations disagree about the same file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Per-chunk checksums must still catch damage the whole-block one caught.
///
/// A plain block used to be hashed in full on first touch, which is correct
/// and, on a cold scan, 0.715x (`f19-coldscan`). Verifying only the chunks an
/// extent touches makes the cost proportional to the read -- and would be
/// worthless if it let damage through. Every byte of live payload is damaged
/// in turn and the read must fail, exactly as it did before.
#[test]
fn per_chunk_checksums_still_catch_damaged_payload() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("supdb-test-chunk-crc");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c.dat");
    let n = 4_000u64;
    let val = vec![b'v'; 200];
    {
        let s = Store::create(
            &path,
            Options {
                checksums: true,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..n {
            s.put(format!("{i:016}").as_bytes(), &val).unwrap();
        }
        s.flush().unwrap();
        s.close().unwrap();
    }
    let clean = std::fs::read(&path).unwrap();
    let mut caught = 0usize;
    let mut served = 0usize;
    // Step across the data region; the first page is the superblock and the
    // index sections sit at the end, so this walks payload.
    for at in (4096..clean.len()).step_by(1021) {
        let mut bytes = clean.clone();
        bytes[at] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        let Ok(r) = Reader::open(&path) else { continue };
        let mut wrong = false;
        let mut err = false;
        for i in 0..n {
            match r.read_all(format!("{i:016}").as_bytes(), |v| {
                if v != val.as_slice() {
                    wrong = true;
                }
            }) {
                Ok(_) => {}
                Err(_) => err = true,
            }
        }
        if wrong && !err {
            served += 1;
        } else if err {
            caught += 1;
        }
    }
    std::fs::write(&path, &clean).unwrap();
    assert_eq!(
        served, 0,
        "{served} damaged files returned wrong bytes without an error"
    );
    assert!(caught > 0, "no damage was detected at all");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Both read paths must reject damage the other rejects.
///
/// `Reader::read_all` has always verified a block's checksum. `Store::read_all`
/// -- added so a writer could read its own writes without a checkpoint, which
/// took the mixed YCSB workloads from 0.07x of LMDB to 18x -- went from the
/// mapping to the caller with nothing in between, so the same store answered
/// with two different guarantees depending on which handle you held. C1.2
/// claimed "a read returns the bytes that were written, or an error" and every
/// trial behind it opened a `Reader`.
///
/// This test was written to assert the *broken* behaviour so it would turn red
/// the day the writer started verifying. It did, on the commit that made it
/// verify, and this is that assertion inverted. RocksDB verifies every block
/// it loads by default; `Options::verify_reads` is the knob for callers who
/// want LMDB's trade instead, and `f21-writerverify` prices it.
#[test]
fn both_read_paths_reject_damaged_payload() {
    use std::io::Write;
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("supdb-test-writer-unverified");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("w.dat");
    let val = vec![b'v'; 400];
    let s = Store::create(
        &path,
        Options {
            checksums: true,
            ..Default::default()
        },
    )
    .unwrap();
    for i in 0..2_000u64 {
        s.put(format!("{i:016}").as_bytes(), &val).unwrap();
    }
    s.flush().unwrap();
    s.checkpoint().unwrap();

    // Find a byte of live payload and flip it, through a separate handle. The
    // appender maps the file shared, so this is what the writer now sees.
    let clean = std::fs::read(&path).unwrap();
    let at = clean
        .windows(val.len())
        .position(|w| w == val.as_slice())
        .expect("payload present")
        + 8;
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        use std::io::Seek;
        f.seek(std::io::SeekFrom::Start(at as u64)).unwrap();
        f.write_all(&[clean[at] ^ 0xff]).unwrap();
        f.sync_all().unwrap();
    }

    let mut writer_saw_damage = false;
    let mut writer_err = false;
    for i in 0..2_000u64 {
        match s.read_all(format!("{i:016}").as_bytes(), |v| {
            if v != val.as_slice() {
                writer_saw_damage = true;
            }
        }) {
            Ok(_) => {}
            Err(_) => writer_err = true,
        }
    }

    let r = Reader::open(&path).unwrap();
    let mut reader_err = false;
    for i in 0..2_000u64 {
        if r.read_all(format!("{i:016}").as_bytes(), |_| {}).is_err() {
            reader_err = true;
        }
    }

    assert!(
        !writer_saw_damage,
        "the writer served bytes that differed from what was written"
    );
    assert!(writer_err, "the writer path did not reject the damage");
    assert!(reader_err, "the reader path did not reject the damage");
    drop(r);
    let _ = s.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// `Store::scan` must agree with `Reader::scan`, key for key.
///
/// It exists so a scan does not open a cold `Reader` over a store this
/// process just wrote -- duplicating the mapping and starting a fresh
/// verified bitset -- but a faster ordered walk that disagreed with the
/// authoritative one would be worthless. Checked with writes outstanding
/// (so the scan has to publish first), across replaces and deletes, from
/// every kind of start position, under every reclaim policy.
#[test]
fn a_writer_scans_in_the_same_order_a_reader_does() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    for policy in [Reclaim::Now, Reclaim::AfterReads, Reclaim::Never] {
        let dir = std::env::temp_dir().join(format!("supdb-test-storescan-{policy:?}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.dat");
        let n = 3_000u64;
        let s = Store::create(
            &path,
            Options {
                reclaim: policy,
                buffer_bytes: 64 * 1024,
                ..Default::default()
            },
        )
        .unwrap();
        for i in 0..n {
            s.put(format!("{i:016}").as_bytes(), format!("v{i}").as_bytes())
                .unwrap();
        }
        // Left outstanding on purpose: the scan has to publish to see them.
        for i in (0..n).step_by(7) {
            s.put(format!("{i:016}").as_bytes(), b"replaced").unwrap();
        }
        for i in (0..n).step_by(11) {
            s.delete(format!("{i:016}").as_bytes()).unwrap();
        }

        let starts: Vec<Option<Vec<u8>>> = vec![
            None,
            Some(b"".to_vec()),
            Some(format!("{0:016}", 0).into_bytes()),
            Some(format!("{0:016}", 1500).into_bytes()),
            Some(b"00000000000001500x".to_vec()),
            Some(format!("{n:016}").into_bytes()),
            Some(vec![0xff; 4]),
        ];
        for limit in [0usize, 1, 5, 200, 100_000] {
            for st in &starts {
                let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let a = s
                    .scan(st.as_deref(), limit, |k, v| {
                        got.push((k.to_vec(), v.to_vec()))
                    })
                    .unwrap();
                // The reader is opened after the scan published, so both see
                // the same state.
                let r = Reader::open(&path).unwrap();
                let b = r
                    .scan(st.as_deref(), limit, |k, v| {
                        want.push((k.to_vec(), v.to_vec()))
                    })
                    .unwrap();
                assert_eq!(a, b, "{policy:?} limit={limit} start={st:?}: counts differ");
                assert_eq!(got, want, "{policy:?} limit={limit} start={st:?}");
            }
        }
        let _ = s.close();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `Readahead::Auto` must resolve to something, and to the same data.
///
/// It picks between telling the kernel a mapping is random and leaving its
/// readahead alone, from the file's size against the memory allowed to cache
/// it -- a decision worth up to 30x out of core (f24) and about 15x the other
/// way when a store fits many times over (f23). Advice is advisory, so the
/// only thing that can go wrong silently is the *reads*, and that is what this
/// checks: every advice must return identical bytes for every key.
#[test]
fn readahead_advice_never_changes_what_is_read() {
    use supdb::Readahead;
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("supdb-test-readahead");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r.dat");
    let n = 5_000u64;
    {
        let s = Store::create(&path, Options::default()).unwrap();
        for i in 0..n {
            s.put(
                format!("{i:016}").as_bytes(),
                format!("value-{i}").as_bytes(),
            )
            .unwrap();
        }
        s.flush().unwrap();
        s.close().unwrap();
    }
    let mut seen: Vec<Vec<(Vec<u8>, Vec<u8>)>> = Vec::new();
    for advice in [
        Readahead::Auto,
        Readahead::Default,
        Readahead::Random,
        Readahead::Sequential,
    ] {
        let r = Reader::open_with(
            &path,
            ReadOptions {
                readahead: advice,
                ..Default::default()
            },
        )
        .unwrap();
        // Auto must resolve to a concrete advice, never report itself.
        assert_ne!(
            r.advice(),
            Readahead::Auto,
            "Auto was not resolved at open for {advice:?}"
        );
        let mut got = Vec::new();
        r.scan(None, usize::MAX, |k, v| got.push((k.to_vec(), v.to_vec())))
            .unwrap();
        assert_eq!(
            got.len(),
            n as usize,
            "{advice:?} walked {} keys",
            got.len()
        );
        seen.push(got);
    }
    for w in seen.windows(2) {
        assert_eq!(w[0], w[1], "two readahead advices disagreed about the data");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A store can be reopened for writing, and survives being reopened repeatedly.
///
/// `Store::create` truncates, so until `Store::open` existed a store could be
/// written once and thereafter only read. The architecture review lists that
/// first and calls it critical; the comparison suite docks a feature point for
/// it; and it had no test, because a limitation that prevents the second
/// session also prevents the test of the second session.
///
/// Three sessions, because one reopen can pass by accident. Every key written
/// in every session must survive, replaced values must stay replaced, deleted
/// keys must stay deleted, and a `Reader` must agree with the writer at the
/// end.
#[test]
fn a_store_can_be_reopened_and_written_again() {
    let _flag = CHECKSUM_FLAG.lock().unwrap_or_else(|e| e.into_inner());
    for reclaim in [Reclaim::Now, Reclaim::AfterReads, Reclaim::Never] {
        let dir = std::env::temp_dir().join(format!("supdb-test-reopen-{reclaim:?}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.dat");
        let opts = || Options {
            reclaim,
            buffer_bytes: 64 * 1024,
            ..Default::default()
        };
        let mut model: std::collections::BTreeMap<Vec<u8>, Vec<u8>> =
            std::collections::BTreeMap::new();

        for session in 0..3u64 {
            let s = if session == 0 {
                Store::create(&path, opts()).unwrap()
            } else {
                Store::open(&path, opts()).expect("reopen")
            };
            // Everything written in an earlier session is still here.
            for (k, v) in &model {
                let mut got = Vec::new();
                s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
                assert_eq!(&got, v, "{reclaim:?} session {session}: lost {k:?}");
            }
            for i in 0..500u64 {
                let k = format!("s{session}-k{i:08}").into_bytes();
                let v = format!("session-{session}-value-{i}").into_bytes();
                s.put(&k, &v).unwrap();
                model.insert(k, v);
            }
            // Replace and delete across the session boundary, so the reopened
            // state is mutated rather than only appended to.
            let old: Vec<Vec<u8>> = model.keys().take(80).cloned().collect();
            for (n, k) in old.iter().enumerate() {
                if n % 2 == 0 {
                    let v = format!("rewritten-in-{session}").into_bytes();
                    s.put(k, &v).unwrap();
                    model.insert(k.clone(), v);
                } else {
                    s.delete(k).unwrap();
                    model.remove(k);
                }
            }
            s.flush().unwrap();
            s.close().unwrap();
        }

        let s = Store::open(&path, opts()).expect("final reopen");
        for (k, v) in &model {
            let mut got = Vec::new();
            s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(&got, v, "{reclaim:?}: final writer disagrees for {k:?}");
        }
        let mut walked = 0usize;
        s.scan(None, usize::MAX, |k, v| {
            walked += 1;
            assert_eq!(model.get(k).map(|x| x.as_slice()), Some(v), "scan {k:?}");
        })
        .unwrap();
        assert_eq!(walked, model.len(), "{reclaim:?}: scan saw {walked} keys");
        s.close().unwrap();

        let r = Reader::open(&path).unwrap();
        for (k, v) in &model {
            let mut got = Vec::new();
            r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(&got, v, "{reclaim:?}: reader disagrees for {k:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The arena must be invisible: both buffering paths answer identically.
///
/// `Options::pending_arena` changes where a buffered value lives -- one arena
/// per shard rather than one `Vec` per key -- and nothing else. It was worth
/// doing because a load scattered its writes across one malloc block per key
/// and paid 21x LMDB's last-level misses for it (docs/profiling.md), but a
/// buffering change reaches every read path in the engine: `put` replaces a
/// run, `append` extends one and has to keep it contiguous when another key
/// has appended in between, `delete` abandons one, and the writer, the scan
/// and a fresh reader all have to see the same bytes either way.
///
/// So this drives all four operations with a shape that forces relocation --
/// appends interleaved across keys that share a shard -- and compares the two
/// arms against each other rather than against a hand-written expectation.
#[test]
fn the_pending_arena_changes_no_answer() {
    type Seen = (Vec<(Vec<u8>, Vec<u8>)>, Vec<(Vec<u8>, Vec<u8>)>);
    fn run(arena: bool, dir: &std::path::Path) -> Seen {
        let path = dir.join("s.dat");
        let opts = Options {
            pending_arena: arena,
            buffer_bytes: 1 << 16,
            reclaim: Reclaim::AfterReads,
            ..Default::default()
        };
        let s = Store::create(&path, opts).unwrap();
        // Interleaved appends: every key's run is broken by another key's, so
        // the arena has to relocate rather than extend in place.
        for round in 0..40u32 {
            for k in 0..25u32 {
                let key = format!("key-{k:04}");
                s.append(key.as_bytes(), format!("v{k}-{round}").as_bytes())
                    .unwrap();
            }
        }
        // Replacements abandon a run mid-arena; deletes drop one entirely.
        for k in (0..25u32).step_by(3) {
            s.put(format!("key-{k:04}").as_bytes(), b"replaced").unwrap();
        }
        for k in (0..25u32).step_by(7) {
            s.delete(format!("key-{k:04}").as_bytes()).unwrap();
        }
        // What the writer sees before anything is sealed. `scan` yields one
        // entry per value, so it is kept as-is rather than folded into the
        // read_all view -- they are different questions and both must match.
        let mut scanned = Vec::new();
        s.scan(None, usize::MAX, |k, v| {
            scanned.push((k.to_vec(), v.to_vec()));
        })
        .unwrap();
        let mut keys: Vec<Vec<u8>> = scanned.iter().map(|(k, _)| k.clone()).collect();
        keys.dedup();
        let mut whole = Vec::new();
        for k in &keys {
            let mut got = Vec::new();
            s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            whole.push((k.clone(), got));
        }
        s.flush().unwrap();
        s.checkpoint().unwrap();
        s.close().unwrap();
        // And the same questions through a fresh reader after sealing.
        let r = Reader::open(&path).unwrap();
        let mut reread = Vec::new();
        for k in &keys {
            let mut got = Vec::new();
            r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            reread.push((k.clone(), got));
        }
        assert_eq!(
            whole, reread,
            "arena={arena}: the writer and a fresh reader disagree"
        );
        (scanned, whole)
    }

    let base = std::env::temp_dir().join(format!("supdb-arena-{}", std::process::id()));
    let (a, b) = (base.join("on"), base.join("off"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let off = run(false, &b);
    let on = run(true, &a);
    assert!(
        on.0.len() > 100 && on.1.len() > 5,
        "the shape stopped exercising anything: {} scanned, {} keys",
        on.0.len(),
        on.1.len()
    );
    assert_eq!(on.0, off.0, "the arena changed what a scan yields");
    assert_eq!(on.1, off.1, "the arena changed what read_all returns");
    let _ = std::fs::remove_dir_all(&base);
}

/// A logged checkpoint is durable: reopening finds everything it acknowledged.
///
/// `Options::redo_log` splits what `checkpoint` had conflated. f27 measured
/// the reason: inserting under `Sync::Always` ran at 42,079 ops/s against
/// 173,446 for updating the same keys with the same checkpoint count, because
/// any insertion sends `checkpoint_in_place` down the full-rewrite path. A
/// logged checkpoint writes what changed and fsyncs that, and rewrites the
/// index only when the arena fills.
///
/// The arena is deliberately tiny and the store is reopened repeatedly,
/// because both bugs this found live at those boundaries rather than away
/// from them. The first was the log arena missing from the free-list
/// reconstruction in `Store::open` -- it is a region the superblock points at
/// and is not one of the three that loop knew about, so a reopened store
/// handed its own live log to the next allocation and read back a block
/// checksum mismatch. The comment above that loop describes the same symptom
/// arriving by a different route. The second was quieter: replayed records are
/// durable and *unpublished*, so `read_all` found them from the shards while
/// `scan` walked the index and did not, and a test that checked only
/// `read_all` would have passed.
#[test]
fn a_logged_checkpoint_survives_reopen() {
    let dir = std::env::temp_dir().join(format!("supdb-redolog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.dat");
    let opts = || Options {
        redo_log: true,
        // Small enough that the arena fills, is abandoned and reallocated
        // several times inside one session.
        log_bytes: 4 * 1024,
        buffer_bytes: 1 << 16,
        ..Default::default()
    };

    let mut model: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = Default::default();
    {
        let s = Store::create(&path, opts()).unwrap();
        s.put(b"seed", b"0").unwrap();
        model.insert(b"seed".to_vec(), b"0".to_vec());
        s.checkpoint().unwrap();
        s.close().unwrap();
    }

    // Several sessions, each ending in a close, each reopening what the last
    // one logged. A single round would not have found the free-list bug.
    for session in 0..6u32 {
        let s = Store::open(&path, opts()).expect("reopen");
        for round in 0..12u32 {
            for k in 0..15u32 {
                let key = format!("s{session}-k{k:03}");
                let val = format!("v-{session}-{k}-{round}");
                s.put(key.as_bytes(), val.as_bytes()).unwrap();
                model.insert(key.into_bytes(), val.into_bytes());
            }
            // Replace a key from an earlier session, so replay has to apply
            // records in order rather than merge them, and delete another so
            // the log carries an empty extent list.
            if session > 0 {
                let hit = format!("s{}-k000", session - 1);
                s.put(hit.as_bytes(), b"overwritten").unwrap();
                model.insert(hit.into_bytes(), b"overwritten".to_vec());
                let gone = format!("s{}-k001", session - 1);
                s.delete(gone.as_bytes()).unwrap();
                model.remove(gone.as_bytes());
            }
            s.checkpoint().unwrap();
        }
        // Everything acknowledged is readable by both paths before close.
        for (k, v) in &model {
            let mut got = Vec::new();
            s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(&got, v, "session {session}: read_all lost {k:?}");
        }
        let mut walked = 0usize;
        s.scan(None, usize::MAX, |k, v| {
            walked += 1;
            assert_eq!(model.get(k).map(|x| x.as_slice()), Some(v), "scan {k:?}");
        })
        .unwrap();
        assert_eq!(walked, model.len(), "session {session}: scan saw {walked}");
        s.close().unwrap();

        // And by a fresh reader, which only ever sees the published index.
        let r = Reader::open(&path).unwrap();
        for (k, v) in &model {
            let mut got = Vec::new();
            r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(&got, v, "session {session}: reader disagrees for {k:?}");
        }
    }
    assert!(model.len() > 80, "the shape stopped exercising anything");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scan sees what an in-place checkpoint wrote.
///
/// The flat index carries two routes to a record: the hash, which a point
/// lookup takes, and a rank-ordered directory of record offsets, which a scan
/// takes. `checkpoint_in_place` republished an updated record by storing its
/// new offset into the key's hash slot -- and not into its directory entry. So
/// after any in-place checkpoint, `read_all` returned the new value and `scan`
/// returned the old one, for every key that checkpoint touched, with no error
/// anywhere.
///
/// It needs a reopen to show, which is why nothing caught it: a store opened
/// fresh takes the full-rewrite path for a while, and the in-place path only
/// engages once an index section exists with the same key count. Found while
/// building the redo log, on a test that had the log turned off.
#[test]
fn a_scan_sees_what_an_in_place_checkpoint_wrote() {
    let dir = std::env::temp_dir().join(format!("supdb-inplace-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.dat");
    let opts = || Options {
        buffer_bytes: 1 << 16,
        ..Default::default()
    };

    let mut model: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = Default::default();
    {
        let s = Store::create(&path, opts()).unwrap();
        s.put(b"seed", b"0").unwrap();
        model.insert(b"seed".to_vec(), b"0".to_vec());
        s.checkpoint().unwrap();
        s.close().unwrap();
    }
    // Reopened, so an index section exists and the in-place path engages.
    let s = Store::open(&path, opts()).unwrap();
    for round in 0..12u32 {
        for k in 0..15u32 {
            let key = format!("k-{k:03}");
            let val = format!("v-{k}-{round}");
            s.put(key.as_bytes(), val.as_bytes()).unwrap();
            model.insert(key.into_bytes(), val.into_bytes());
        }
        s.checkpoint().unwrap();
    }
    // The two routes must agree. Before the fix, every key here came back one
    // checkpoint stale through `scan` and current through `read_all`.
    let mut walked = 0usize;
    s.scan(None, usize::MAX, |k, v| {
        walked += 1;
        assert_eq!(
            model.get(k).map(|x| x.as_slice()),
            Some(v),
            "scan is stale for {:?}",
            String::from_utf8_lossy(k)
        );
    })
    .unwrap();
    assert_eq!(walked, model.len(), "scan saw {walked} of {}", model.len());
    for (k, v) in &model {
        let mut got = Vec::new();
        s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
        assert_eq!(&got, v, "read_all disagrees for {k:?}");
    }
    s.close().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file written on the other byte order is refused, and says so.
///
/// Supdb writes every scalar little-endian and then addresses two structures
/// in place regardless: `flatindex` hands back `&[Ext]` borrowed straight out
/// of the mapping, and a block table's records are reinterpreted rather than
/// decoded. So a file is self-consistent only on the byte order that wrote it
/// -- and nothing recorded which that was, so a big-endian-written store would
/// have been *read*, with extents whose fields were all byte-swapped, rather
/// than refused.
///
/// The fix costs nothing and breaks no existing file: the three magics are
/// written `to_ne_bytes` instead of `to_le_bytes`, which is the same bytes on
/// a little-endian machine and a byte-order mark everywhere else.
///
/// This machine is little-endian, so the other order is simulated by swapping
/// the mark in a file that is otherwise perfectly intact. That is exactly what
/// a big-endian writer would have produced for that field.
#[test]
fn a_file_from_the_other_byte_order_is_refused_by_name() {
    let dir = std::env::temp_dir().join(format!("supdb-endian-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.dat");
    {
        let s = Store::create(&path, Options::default()).unwrap();
        for k in 0..8u32 {
            s.put(format!("k{k}").as_bytes(), b"v").unwrap();
        }
        s.checkpoint().unwrap();
        s.close().unwrap();
    }
    // Intact as written: both readers open it.
    Reader::open(&path).expect("a same-endian file opens");
    Store::open(&path, Options::default())
        .expect("a same-endian file opens for writing")
        .close()
        .unwrap();

    // Swap the byte-order mark in both superblock slots, and nothing else.
    // The checksum is computed over the field *values*, so it still matches:
    // this file is damaged in exactly one way, and it is the way that says
    // "another machine wrote me".
    let mut bytes = std::fs::read(&path).unwrap();
    for slot in [0usize, 512] {
        let at = slot + 120;
        bytes[at..at + 8].reverse();
    }
    std::fs::write(&path, &bytes).unwrap();

    let msg = match Reader::open(&path) {
        Ok(_) => panic!("a foreign-endian file must not open"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("endian"),
        "the error must name the byte order, not read as damage: {msg}"
    );
    let msg = match Store::open(&path, Options::default()) {
        Ok(_) => panic!("a foreign-endian file must not open for writing"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("byte order"),
        "Store::open must name it too: {msg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Inserting a key does not have to rewrite the index section.
///
/// `checkpoint_in_place` used to decline every insertion outright: records
/// carry half again in slack and the hash runs at half load, so both could
/// take a new key where they lie, but the directory is a sorted array of
/// record offsets and growing it shifts everything after -- not a change a
/// reader may catch half-done. f27 priced that refusal at 4.122x on a workload
/// that only inserts, and it is the reason EXT.9 reads 0.010x.
///
/// With `Options::index_inserts` the directory is double-buffered: the spliced
/// copy goes into the buffer nobody is reading and one aligned store of
/// `dir_state` publishes which buffer is live and how many keys it holds.
///
/// What this asserts is that it is invisible. Both arms must answer every
/// lookup and every scan identically, through the writer and through a fresh
/// reader, with keys inserted in an order that forces splices at the front,
/// the middle and the end of the directory.
#[test]
fn inserting_in_place_changes_no_answer() {
    type Walked = (Vec<(Vec<u8>, Vec<u8>)>, u64);
    fn run(inserts: bool, dir: &std::path::Path) -> Walked {
        let path = dir.join("s.dat");
        let opts = || Options {
            index_inserts: inserts,
            buffer_bytes: 1 << 16,
            ..Default::default()
        };
        let mut model: std::collections::BTreeMap<Vec<u8>, Vec<u8>> = Default::default();
        {
            let s = Store::create(&path, opts()).unwrap();
            for k in (0..40u32).step_by(2) {
                let key = format!("k-{k:04}");
                s.put(key.as_bytes(), b"first").unwrap();
                model.insert(key.into_bytes(), b"first".to_vec());
            }
            s.checkpoint().unwrap();
            s.close().unwrap();
        }
        // Reopened, so an index section exists and the in-place path engages.
        let s = Store::open(&path, opts()).unwrap();
        for round in 0..6u32 {
            // Odd keys interleave with the even ones already indexed, so each
            // batch splices at ranks scattered through the directory. The
            // 9000s land after everything and the 0000s before it.
            for k in (1..40u32).step_by(2) {
                let key = format!("k-{k:04}");
                let val = format!("odd-{k}-{round}");
                s.put(key.as_bytes(), val.as_bytes()).unwrap();
                model.insert(key.into_bytes(), val.into_bytes());
            }
            let head = format!("a-{round:04}");
            let tail = format!("z-{round:04}");
            s.put(head.as_bytes(), b"head").unwrap();
            model.insert(head.into_bytes(), b"head".to_vec());
            s.put(tail.as_bytes(), b"tail").unwrap();
            model.insert(tail.into_bytes(), b"tail".to_vec());
            s.checkpoint().unwrap();
        }
        let mut scanned = Vec::new();
        let n = s
            .scan(None, usize::MAX, |k, v| {
                scanned.push((k.to_vec(), v.to_vec()));
            })
            .unwrap();
        for (k, v) in &model {
            let mut got = Vec::new();
            s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(&got, v, "inserts={inserts}: read_all disagrees for {k:?}");
        }
        s.close().unwrap();
        let r = Reader::open(&path).unwrap();
        for (k, v) in &model {
            let mut got = Vec::new();
            r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(&got, v, "inserts={inserts}: reader disagrees for {k:?}");
        }
        let want: Vec<(Vec<u8>, Vec<u8>)> = model.into_iter().collect();
        assert_eq!(scanned, want, "inserts={inserts}: scan is not the model");
        (scanned, n)
    }

    let base = std::env::temp_dir().join(format!("supdb-ins-{}", std::process::id()));
    let (a, b) = (base.join("on"), base.join("off"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let on = run(true, &a);
    let off = run(false, &b);
    assert!(on.0.len() > 50, "the shape stopped exercising anything");
    assert_eq!(on, off, "double-buffering the directory changed an answer");
    let _ = std::fs::remove_dir_all(&base);
}
