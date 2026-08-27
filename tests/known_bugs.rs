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
                    .unwrap_or_else(|e| {
                        panic!("flat={flat} round {round} key {k}: {e}")
                    });
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
                    store.read_all(mk.as_bytes(), |b| mg.push(b.to_vec())).unwrap();
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
            assert_eq!(&rg, want, "{reclaim:?}: reader disagrees with the model on {mk}");
        }
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
