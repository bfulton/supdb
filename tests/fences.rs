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

mod common;
use common::{kv, partition_fences, seg_names, vf, without_the_clock};

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
        db.read_all(k, vf(&mut |_| n += 1))
            .unwrap_or_else(|e| panic!("read of {:?}: {e}", String::from_utf8_lossy(k)));
        assert_eq!(n, 1, "key {:?}", String::from_utf8_lossy(k));
    }
    let mut seen: Vec<Vec<u8>> = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, _| {
            if seen.last().map(|l| l.as_slice()) != Some(k) {
                seen.push(k.to_vec());
            }
        }),
    )
    .unwrap();
    let mut want = keys.to_vec();
    want.sort();
    assert_eq!(seen, want, "the scan's keys");
}

/// A merge planned with fences a landing old. The planner takes the
/// ranges due from the partitions standing, and the merge it starts
/// joins a finished merge first, whose landing replaces the partitions
/// that merge rewrote: a fence of a partition that was gone named a range
/// two of the new partitions tiled, the merge cut its one key list at
/// both, and it refused its own key ("merge slices leave a key
/// unassigned") -- one run in a few hundred of the reader-thread tests,
/// once in an out-of-core sitting. Built here by holding the first merge
/// before its landing and making the planner join it before it plans:
/// the fences resolve to the partitions standing after the landing, the
/// segment work goes on, the partitions tile, and every key reads back.
#[test]
fn a_merge_planned_with_fences_a_landing_old_takes_the_partitions_standing_after_it() {
    let d = scratch("stale-fences");
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 32 << 10,
            partition_bytes: Some(32 << 10),
            l0_trigger: 2,
            ..Options::default()
        },
    )
    .unwrap();
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    let mut keys: Vec<Vec<u8>> = (0..3000).map(key).collect();
    for k in &keys {
        db.append(k, b"0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    let fences = partition_fences(&d);
    assert!(fences.len() >= 2, "partitions: {fences:?}");
    let first_hi = fences[0].1.clone().expect("the first partition closes");
    // Three pieces over the first partition, each of four hundred keys new
    // to its range (between its first two keys), so that the merge of the
    // range writes more keys than a piece may hold and splits it. The
    // second piece's landing starts the merge, which is held before its
    // landing; the third's landing finds the range due again, its pieces
    // the merge's inputs, and plans a merge it must wait for.
    db.hold_merge_landing(true);
    for round in 0..3u32 {
        for i in 0..400u32 {
            let k = format!("key-00000-{round}{i:04}").into_bytes();
            assert!(k.as_slice() < first_hi.as_slice());
            db.append(&k, b"n");
            keys.push(k);
        }
        db.commit().unwrap();
        if round == 1 {
            db.plan_joins_merge(true);
        }
        db.seal().unwrap();
    }
    let t = std::time::Instant::now();
    while !db.merge_plan_joining() {
        assert!(
            t.elapsed().as_secs() < 30,
            "the planner never waited to join the merge in flight"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    db.hold_merge_landing(false);
    db.settle().unwrap();
    db.plan_joins_merge(false);
    db.settle().unwrap();
    let after = partition_fences(&d);
    assert!(after.len() > fences.len(), "the range split: {after:?}");
    for w in after.windows(2) {
        assert_eq!(
            w[0].1.as_ref(),
            Some(&w[1].0),
            "the partitions tile: {after:?}"
        );
    }
    hold_to(&db, &keys);
    let _ = std::fs::remove_dir_all(&d);
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

/// A promotion closes the partition at the first piece's first key. An
/// empty partition -- one whose every key was deleted and merged away --
/// has no last key to hold that bound above, and a piece whose first key
/// is the partition's own lower fence closed it at that fence: a partition
/// of no width, which shares its lower fence with the one after it, and a
/// scan across the boundary walked the next range's pieces once for each
/// and listed their values twice.
#[test]
fn a_promotion_never_closes_a_partition_at_its_own_lower_fence() {
    let dir = scratch("promotion-empty-partition");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: true,
            publish_in_background: false,
            scan_block_cache: false,
            l0_trigger: 2,
            promote: true,
            ..Options::default()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let mut keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // A partition in the middle, closed at both ends.
    let fences = partition_fences(&dir);
    assert!(fences.len() >= 3, "partitions: {}", fences.len());
    let (lo, hi) = {
        let (lo, hi) = &fences[fences.len() / 2];
        (lo.clone(), hi.clone().expect("a closed partition"))
    };
    // Empty it: every key of its range deleted, in enough pieces to have
    // the range rewritten, which writes the partition back with no key.
    let inside: Vec<Vec<u8>> = keys
        .iter()
        .filter(|k| **k >= lo && **k < hi)
        .cloned()
        .collect();
    assert!(!inside.is_empty());
    for _ in 0..2 {
        for k in &inside {
            db.delete(k);
        }
        db.commit().unwrap();
        db.seal().unwrap();
    }
    db.settle().unwrap();
    keys.retain(|k| !inside.contains(k));
    let empty = partition_fences(&dir)
        .iter()
        .any(|(l, h)| *l == lo && *h == Some(hi.clone()));
    assert!(
        empty,
        "the emptied range is still a partition: {:?}",
        partition_fences(&dir)
    );
    // Pieces over the range, disjoint and ascending as a promotion needs
    // them, the first piece's first key the range's own lower fence.
    let mut fresh: Vec<Vec<u8>> = Vec::new();
    for round in 0..2u8 {
        let mut batch: Vec<Vec<u8>> = Vec::new();
        for i in 0..2u8 {
            let mut k = lo.clone();
            if round + i > 0 {
                k.push(2 * round + i);
            }
            batch.push(k);
        }
        for k in &batch {
            db.put(k, &[round; 100]);
        }
        db.commit().unwrap();
        db.seal().unwrap();
        fresh.extend(batch);
    }
    db.settle().unwrap();
    for (l, h) in partition_fences(&dir) {
        assert!(
            h.as_ref().is_none_or(|h| *h > l),
            "a partition of no width: [{}, {:?})",
            String::from_utf8_lossy(&l),
            h.as_deref().map(String::from_utf8_lossy)
        );
    }
    // One more piece over the range, and a scan across the boundary.
    db.put(&lo, b"last");
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    let mut got: Vec<Vec<u8>> = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            if k == lo.as_slice() {
                got.push(v.to_vec());
            }
        }),
    )
    .unwrap();
    assert_eq!(
        got,
        vec![b"last".to_vec()],
        "the boundary key's values in a scan across it"
    );
    keys.extend(fresh);
    hold_to(&db, &keys);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A merge over a fence no input key lies in wrote nothing for it, and the
/// partition that names the fence is an input, so the set that landed had
/// a hole where it was. The shape: a partition emptied by a merge (every
/// key of its range deleted), then a wide piece -- one sealed while the
/// first partitioning was in flight, so named with no fence -- which
/// selects every range for the next merge, that one included, with no key
/// of its own inside it.
#[test]
fn a_merge_writes_an_empty_partition_for_a_fence_no_input_key_lies_in() {
    let dir = scratch("merge-fence-without-keys");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: false,
            publish_in_background: true,
            scan_block_cache: false,
            l0_trigger: 2,
            promote: true,
            compact: true,
            ..Options::default()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.seal().unwrap();
    // The second piece: the middle hundred deleted, the first hundred
    // written again so the two pieces overlap and are merged, not
    // promoted. Its landing starts the first partitioning, which writes
    // the slices the deleted run covers as empty partitions. Its durable
    // phase is held: no manifest is written while a seal is between its
    // phases, so the partitioning cannot land before the next seal is
    // named, and that one is named with no fence, a wide piece.
    db.hold_seal_durable(true);
    for k in &keys[100..200] {
        db.delete(k);
    }
    for k in &keys[..100] {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.seal().unwrap();
    // The wide piece, holding one key below the last partition's last so
    // that no promotion takes it: every range is merged, that of an empty
    // partition included, with no key of the piece inside it.
    db.put(&keys[250], b"again");
    db.commit().unwrap();
    db.seal().unwrap();
    // The three seals' files, written off the writer's thread; no
    // partition among them, since none can land under the hold.
    let t = std::time::Instant::now();
    while seg_names(&dir).len() < 3 {
        assert!(
            t.elapsed().as_secs() < 60,
            "the seals' files: {:?}",
            seg_names(&dir)
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let before: Vec<String> = seg_names(&dir);
    assert!(
        before.iter().all(|n| n.starts_with("seg-")),
        "no partition before the hold is released: {before:?}"
    );
    db.hold_seal_durable(false);
    db.settle().unwrap();
    db.settle().unwrap();
    let seq_of = |n: &str| n.split('-').nth(2).unwrap().parse::<u64>().unwrap();
    let first: Vec<u64> = seg_names(&dir)
        .iter()
        .filter(|n| n.starts_with("par-"))
        .map(|n| seq_of(n))
        .collect();
    assert!(
        first.len() > 3 && first.iter().all(|s| *s == first[0]),
        "the first partitioning: {first:?}"
    );
    assert!(
        seg_names(&dir).iter().any(|n| n.starts_with("seg-")),
        "the wide piece beside the partitions: {:?}",
        seg_names(&dir)
    );
    // A merge's landing decides nothing; the next seal's does, and its
    // piece is cut at the partitions' fences, so the wide piece is what
    // selects every range.
    db.put(&keys[0], b"later");
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    db.settle().unwrap();
    // The partitions tile: open below, each opening where the last
    // closed, open above.
    let fences = partition_fences(&dir);
    assert!(fences.len() > 3, "partitions: {}", fences.len());
    assert!(fences[0].0.is_empty(), "open below: {:?}", fences[0]);
    for w in fences.windows(2) {
        assert_eq!(
            w[0].1.as_ref(),
            Some(&w[1].0),
            "a hole or an overlap between {:?} and {:?}",
            w[0],
            w[1]
        );
    }
    assert!(fences.last().unwrap().1.is_none(), "open above");
    // Every partition is the second merge's, the one the wide piece
    // selected every range for: one covered sequence over all of them,
    // past the first partitioning's, and the wide piece retired.
    let names = seg_names(&dir);
    assert!(
        !names.iter().any(|n| n.starts_with("seg-")),
        "the wide piece was merged away: {names:?}"
    );
    let seqs: Vec<u64> = names
        .iter()
        .filter(|n| n.starts_with("par-"))
        .map(|n| seq_of(n))
        .collect();
    assert!(
        seqs.iter().all(|s| *s == seqs[0]) && seqs[0] > first[0],
        "partitions not all from the merge over the wide piece: first {} now {seqs:?}",
        first[0]
    );
    let mut live: Vec<Vec<u8>> = keys[..100].to_vec();
    live.extend(keys[200..].iter().cloned());
    hold_to(&db, &live);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A merge's first pass reads its inputs' keys off their ordered indexes'
/// heads where a head is a whole key (`Options::merge_keys_by_heads`), and
/// off the records otherwise. Two stores fed the same operations, one each
/// way, merge to the same files byte for byte, the superblock's clock
/// aside; a store whose keys are of two lengths reads every input's keys
/// off the records under either, and merges to the same files as its
/// twin too.
#[test]
fn a_merges_first_pass_over_the_heads_writes_what_the_records_write() {
    for mixed in [false, true] {
        let mut dirs = Vec::new();
        for heads in [true, false] {
            let d = scratch(&format!("merge-heads-{mixed}-{heads}"));
            let mut db = Db::create(
                &d,
                Options {
                    seal_bytes: 32 << 10,
                    partition_bytes: Some(32 << 10),
                    l0_trigger: 2,
                    merge_keys_by_heads: heads,
                    ..Options::default()
                },
            )
            .unwrap();
            // One length of key, so a head is a whole key; or two, with a
            // third of them a suffix longer, so the heads cannot say.
            let key = |k: u32| {
                if mixed && k.is_multiple_of(3) {
                    format!("key-{k:05}-x").into_bytes()
                } else {
                    format!("key-{k:05}").into_bytes()
                }
            };
            for k in 0..3000u32 {
                db.append(&key(k), b"0");
            }
            db.commit().unwrap();
            db.flush().unwrap();
            assert!(partition_fences(&d).len() >= 2, "partitions");
            // Two pieces of updates and of new keys over every partition:
            // the second's landing starts the merge of every range.
            for round in 0..2u32 {
                for k in (round..3000u32).step_by(7) {
                    db.append(&key(k), b"n");
                }
                for k in 0..300u32 {
                    db.append(&key(90000 + k * 10 + round), b"m");
                }
                db.commit().unwrap();
                db.seal().unwrap();
            }
            db.settle().unwrap();
            let (by_heads, by_records) = db.merge_key_sources();
            assert!(by_heads + by_records > 0, "a merge ran");
            if heads && !mixed {
                assert_eq!(
                    by_records, 0,
                    "every input's keys off its heads: {by_records} inputs off the records"
                );
            } else {
                assert_eq!(
                    by_heads, 0,
                    "the records' pass: {by_heads} inputs off their heads"
                );
            }
            db.close().unwrap();
            dirs.push(d);
        }
        let names: Vec<Vec<String>> = dirs.iter().map(|d| seg_names(d)).collect();
        assert_eq!(names[0], names[1], "the same segments (mixed {mixed})");
        for n in &names[0] {
            let a = std::fs::read(dirs[0].join(n)).unwrap();
            let b = std::fs::read(dirs[1].join(n)).unwrap();
            assert_eq!(
                without_the_clock(&a),
                without_the_clock(&b),
                "{n} differs between the heads' pass and the records' (mixed {mixed})"
            );
        }
        for d in &dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// A merge drops its inputs' record pages from the page cache behind its
/// second pass (`Options::merge_drop_behind`), and changes no byte doing
/// so: two stores fed the same operations, one dropping and one not, merge
/// to the same files, the superblock's clock aside, and the one that drops
/// counts what it dropped.
#[test]
fn a_merge_drops_its_inputs_pages_behind_its_second_pass() {
    let mut dirs = Vec::new();
    for drop in [true, false] {
        let d = scratch(&format!("merge-drop-{drop}"));
        let mut db = Db::create(
            &d,
            Options {
                seal_bytes: 32 << 10,
                partition_bytes: Some(32 << 10),
                l0_trigger: 2,
                merge_drop_behind: drop,
                // A drop every sixteen kilobytes, so a store of kilobytes
                // reaches one.
                merge_drop_chunk: 16 << 10,
                ..Options::default()
            },
        )
        .unwrap();
        let key = |k: u32| format!("key-{k:05}").into_bytes();
        for k in 0..3000u32 {
            db.append(&key(k), b"0");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        for round in 0..2u32 {
            for k in (round..3000u32).step_by(7) {
                db.append(&key(k), b"n");
            }
            for k in 0..300u32 {
                db.append(&key(90000 + k * 10 + round), b"m");
            }
            db.commit().unwrap();
            db.seal().unwrap();
        }
        db.settle().unwrap();
        let (by_heads, by_records) = db.merge_key_sources();
        assert!(by_heads + by_records > 0, "a merge ran");
        let dropped = db.merge_dropped_bytes();
        if cfg!(target_os = "linux") {
            assert_eq!(
                dropped > 0,
                drop,
                "the merge dropped its inputs' pages only with the option: {dropped}"
            );
        }
        db.close().unwrap();
        dirs.push(d);
    }
    let names: Vec<Vec<String>> = dirs.iter().map(|d| seg_names(d)).collect();
    assert_eq!(names[0], names[1], "the same segments");
    for n in &names[0] {
        let a = std::fs::read(dirs[0].join(n)).unwrap();
        let b = std::fs::read(dirs[1].join(n)).unwrap();
        assert_eq!(
            without_the_clock(&a),
            without_the_clock(&b),
            "{n} differs between dropping and keeping"
        );
    }
    for d in &dirs {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// The segment work's loop plans a merge when none is in flight and a scan
/// has come since its last look (`Options::merge_plan_idle`): the chain
/// plans only at a landing, so a range left due when the last merge landed
/// with no scan before it -- the shape a burst's end leaves when no merge is
/// in flight -- stood for as long as the store was only read. Here the first
/// merge is held while two ranges become due, released with no scan, and
/// lands alone; then one scan, and with the option the loop merges the
/// range left, where without it the pieces stay.
#[test]
fn a_store_read_with_no_merge_in_flight_plans_one_in_its_loop() {
    for idle in [true, false] {
        let d = scratch(&format!("merge-idle-{idle}"));
        let mut db = Db::create(
            &d,
            Options {
                seal_bytes: 32 << 10,
                partition_bytes: Some(32 << 10),
                l0_trigger: 2,
                merge_chains: true,
                merge_bytes_cap: 0,
                merge_plan_idle: idle,
                ..Options::default()
            },
        )
        .unwrap();
        let key = |k: u32| format!("key-{k:05}").into_bytes();
        let mut keys: Vec<Vec<u8>> = (0..3000).map(key).collect();
        for k in &keys {
            db.append(k, b"0");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let fences = partition_fences(&d);
        assert!(fences.len() >= 3, "partitions: {fences:?}");
        let (merges0, plans0) = (db.seal_waits().merges, db.seal_waits().plans);
        let in_range = |r: usize, round: u32, i: u32| -> Vec<u8> {
            let (lo, _) = &fences[r];
            [lo.as_slice(), format!("-{round}{i:03}").as_bytes()].concat()
        };
        db.hold_merge_landing(true);
        let rounds = [1usize, 1, 2, 2];
        for (round, &r) in rounds.iter().enumerate() {
            for i in 0..200u32 {
                let k = in_range(r, round as u32, i);
                db.append(&k, b"n");
                keys.push(k);
            }
            db.commit().unwrap();
            db.seal().unwrap();
        }
        let t = std::time::Instant::now();
        while db.seal_waits().plans < plans0 + rounds.len() as u64 {
            assert!(
                t.elapsed().as_secs() < 30,
                "the seals' landings never planned"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Released with no scan: the held merge lands and plans nothing.
        db.hold_merge_landing(false);
        let t = std::time::Instant::now();
        while db.seal_waits().merges < merges0 + 1 {
            assert!(t.elapsed().as_secs() < 30, "the held merge never landed");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            db.seal_waits().merges,
            merges0 + 1,
            "idle {idle}: nothing more merged before a scan"
        );
        let mut seen = 0usize;
        db.scan(b"", 5, kv(&mut |_, _| seen += 1)).unwrap();
        assert_eq!(seen, 5);
        let want = if idle { 2 } else { 1 };
        let t = std::time::Instant::now();
        while db.seal_waits().merges < merges0 + want {
            assert!(
                t.elapsed().as_secs() < 30,
                "idle {idle}: {} merges landed, {want} expected",
                db.seal_waits().merges - merges0
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let pieces = seg_names(&d)
            .iter()
            .filter(|n| !n.starts_with("par-"))
            .count();
        assert_eq!(
            (db.seal_waits().merges - merges0, pieces),
            (want, if idle { 0 } else { 2 }),
            "idle {idle}: merges landed and pieces left"
        );
        hold_to(&db, &keys);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A merge's landing plans the next (`Options::merge_chains`). The planner
/// ran at a seal's durable landing alone, so a store that was only read
/// after a burst kept the burst's pieces for as long as it was read: under
/// a bound the full rewrite's pass met seventeen pieces a range with one
/// merge in flight and nothing to start the next. A range made due while a
/// held merge runs is merged by the landing's plan with no write after the
/// hold is lifted, and stays as it is with the chain off; and the cap on a
/// merge's bytes (`Options::merge_bytes_cap`) takes two ranges due at once
/// as two merges, one after the other, where no cap takes them as one. The
/// chain waits for reads -- a scan since the last landing -- and a store
/// that has only been written plans nothing at the landing, unless the
/// store is out of core under `Options::merge_chains_writes`, where the
/// chain runs through the write-only stretch too; in core that option
/// changes nothing.
#[test]
fn a_merges_landing_plans_the_next_and_a_cap_lands_a_range_at_a_time() {
    // (chain, cap, both ranges due at once, a scan before the landing,
    // chained through writes, the controller's gate) -> (pieces left,
    // merges landed).
    for (chain, cap, at_once, scan, writes, gate, left, merges) in [
        (true, 0u64, false, true, false, 0u8, 0usize, 2u64),
        (false, 0, false, true, false, 0, 2, 1),
        (true, 1, true, true, false, 0, 0, 2),
        (true, 0, true, true, false, 0, 0, 1),
        (true, 0, false, false, false, 0, 2, 1),
        (true, 0, false, false, true, 1, 0, 2),
        (true, 0, false, false, true, 0, 2, 1),
        (true, 0, false, false, false, 1, 2, 1),
    ] {
        let d = scratch(&format!(
            "merge-chain-{chain}-{cap}-{at_once}-{scan}-{writes}-{gate}"
        ));
        let mut db = Db::create(
            &d,
            Options {
                seal_bytes: 32 << 10,
                partition_bytes: Some(32 << 10),
                l0_trigger: 2,
                merge_chains: chain,
                merge_bytes_cap: cap,
                merge_chains_writes: writes,
                // The gate is the test's to set where it is not raw.
                zpages: if gate == 0 {
                    supdb::ZPages::default()
                } else {
                    supdb::ZPages::Off
                },
                ..Options::default()
            },
        )
        .unwrap();
        if gate != 0 {
            db.zgate_set(gate);
        }
        let key = |k: u32| format!("key-{k:05}").into_bytes();
        let mut keys: Vec<Vec<u8>> = (0..3000).map(key).collect();
        for k in &keys {
            db.append(k, b"0");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let fences = partition_fences(&d);
        assert!(fences.len() >= 3, "partitions: {fences:?}");
        let (merges0, plans0) = (db.seal_waits().merges, db.seal_waits().plans);
        // Keys new to a range, between its first two keys: its lower fence
        // and a suffix sorts above the fence and below every other key.
        let in_range = |r: usize, round: u32, i: u32| -> Vec<u8> {
            let (lo, hi) = &fences[r];
            let k = [lo.as_slice(), format!("-{round}{i:03}").as_bytes()].concat();
            assert!(k.as_slice() > lo.as_slice(), "{k:?} above {lo:?}");
            if let Some(h) = hi {
                assert!(k.as_slice() < h.as_slice(), "{k:?} below {h:?}");
            }
            k
        };
        // The merge's thread waits before returning while held, so the merge
        // the second seal's landing plans is in flight through the seals
        // after it, whose landings find it running and plan nothing.
        db.hold_merge_landing(true);
        let rounds: Vec<Vec<usize>> = if at_once {
            vec![vec![1, 2], vec![1, 2]]
        } else {
            vec![vec![1], vec![1], vec![2], vec![2]]
        };
        for (round, ranges) in rounds.iter().enumerate() {
            for &r in ranges {
                for i in 0..200u32 {
                    let k = in_range(r, round as u32, i);
                    db.append(&k, b"n");
                    keys.push(k);
                }
            }
            db.commit().unwrap();
            db.seal().unwrap();
        }
        // Every seal landed durable and made its plan: the planner's count,
        // never the clock.
        let t = std::time::Instant::now();
        while db.seal_waits().plans < plans0 + rounds.len() as u64 {
            assert!(
                t.elapsed().as_secs() < 30,
                "the seals' landings never planned"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(db.seal_waits().merges, merges0, "nothing landed while held");
        // Reads around: a landing plans the next merge only where the
        // store's scan life moved since the last landing.
        if scan {
            let mut seen = 0usize;
            db.scan(b"", 5, kv(&mut |_, _| seen += 1)).unwrap();
            assert_eq!(seen, 5);
        }
        db.hold_merge_landing(false);
        // The segment work's own loop lands the merge and, with the chain,
        // plans and lands the next; a settle would join the merge itself and
        // plan nothing, so the loop is waited for by the merges it lands.
        let t = std::time::Instant::now();
        while db.seal_waits().merges < merges0 + merges {
            assert!(
                t.elapsed().as_secs() < 30,
                "chain {chain} cap {cap} at once {at_once} scan {scan} writes {writes} gate {gate}: {} merges landed, {merges} expected",
                db.seal_waits().merges - merges0
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        db.settle().unwrap();
        let pieces = seg_names(&d)
            .iter()
            .filter(|n| !n.starts_with("par-"))
            .count();
        assert_eq!(
            pieces, left,
            "chain {chain} cap {cap} at once {at_once} scan {scan} writes {writes} gate {gate}: pieces left after the landings"
        );
        assert_eq!(
            db.seal_waits().merges - merges0,
            merges,
            "chain {chain} cap {cap} at once {at_once} scan {scan} writes {writes} gate {gate}: merges landed"
        );
        hold_to(&db, &keys);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}
