//! The block cache's settle: a write is filed under the key's identity,
//! into a block of the partition whose range holds the key.
//!
//! The settle reads the memtable's log and files each key's writes into
//! the block the key falls in, and a key created since the scan snapshot
//! into its block's list. A write it never files is a key or a value a
//! scan through the block cache answers without, while the point read
//! beside it, through the memtable, is right.

use std::path::PathBuf;
use supdb::{Db, Options, Upkeep};

mod common;
use common::{kv, partition_fences, vf};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-settle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A scan through the writer, as the model it answers.
fn scan_model(db: &Db) -> std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut m = std::collections::BTreeMap::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            m.entry(k.to_vec())
                .or_insert_with(Vec::new)
                .push(v.to_vec());
        }),
    )
    .unwrap();
    m
}

/// A scan through the writer as it comes: every key with its values, in
/// the order the scan hands them over. `scan_model` gathers into a map,
/// which answers the same for a scan that emits a key out of its place.
fn scan_list(db: &Db) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut l: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| match l.last_mut() {
            Some((last, vals)) if last.as_slice() == k => vals.push(v.to_vec()),
            _ => l.push((k.to_vec(), vec![v.to_vec()])),
        }),
    )
    .unwrap();
    l
}

/// The empty key takes no bytes of the arena, so the key created after it
/// starts at the same arena offset; folded by offset, that key's creating
/// write vanished into the empty key's entry and was never filed, and a
/// value appended to a key at that offset was never patched in.
#[test]
fn a_key_at_the_empty_keys_arena_offset_is_filed_and_patched() {
    let dir = scratch("empty-key-offset");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: true,
            publish_in_background: false,
            upkeep: Upkeep::Inline,
            scan_block_cache: true,
            ..common::options()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // The block path stands once a scan has built its tables.
    let before = scan_model(&db);
    assert_eq!(before.len(), 300);
    // The empty key, then a key created at its offset, then a second
    // value for a sealed key written right after another empty-key write.
    db.append(b"", b"e");
    db.append(b"k00150x", b"created");
    db.commit().unwrap();
    db.append(b"", b"e2");
    db.append(&keys[200], b"again");
    db.commit().unwrap();
    let got = scan_model(&db);
    assert_eq!(
        got.get(&b""[..]).map(Vec::len),
        Some(2),
        "the empty key's values"
    );
    assert_eq!(
        got.get(&b"k00150x"[..]),
        Some(&vec![b"created".to_vec()]),
        "the key created at the empty key's offset, in a scan"
    );
    assert_eq!(
        got.get(&keys[200]),
        Some(&vec![value.to_vec(), b"again".to_vec()]),
        "the value appended after the empty key's write, in a scan"
    );
    let mut n = 0;
    db.read_all(b"k00150x", vf(&mut |_| n += 1)).unwrap();
    assert_eq!(n, 1, "the point read");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A partition emptied by a merge keeps its range, and the block cache
/// counted its blocks from its records: none, so the settle dropped every
/// write resolved into it, and the block scan walked past the partition
/// without a block to walk. A key written into the range was in no scan
/// through the cache, then or after its seal into a piece over the range,
/// while the point read beside it was right. An empty partition has one
/// block, over no record.
#[test]
fn a_key_written_into_an_empty_partitions_range_is_in_a_scan_through_the_cache() {
    let dir = scratch("empty-partition");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: true,
            publish_in_background: false,
            upkeep: Upkeep::Inline,
            scan_block_cache: true,
            l0_trigger: 2,
            promote: true,
            ..common::options()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // A partition in the middle, closed at both ends, emptied: every key
    // of its range deleted, in enough pieces to have the range rewritten,
    // which writes the partition back with no key.
    let fences = partition_fences(&dir);
    assert!(fences.len() >= 3, "partitions: {}", fences.len());
    let (lo, hi) = {
        let (lo, hi) = &fences[fences.len() / 2];
        (lo.clone(), hi.clone().expect("a closed partition"))
    };
    let inside: Vec<Vec<u8>> = keys
        .iter()
        .filter(|k| **k >= lo && **k < hi)
        .cloned()
        .collect();
    assert!(inside.len() >= 2, "keys inside the range: {}", inside.len());
    for _ in 0..2 {
        for k in &inside {
            db.delete(k);
        }
        db.commit().unwrap();
        db.seal().unwrap();
    }
    db.settle().unwrap();
    assert!(
        partition_fences(&dir)
            .iter()
            .any(|(l, h)| *l == lo && *h == Some(hi.clone())),
        "the emptied range is still a partition: {:?}",
        partition_fences(&dir)
    );
    // The block path stands once a scan has built its tables.
    let before = scan_model(&db);
    assert_eq!(before.len(), 300 - inside.len());
    assert!(!before.contains_key(&inside[0]));
    // Two keys written into the range: one of the deleted ones again, and
    // one that never was. Filed at the commit, under the inline upkeep.
    let fresh = {
        let mut k = inside[0].clone();
        k.push(b'x');
        k
    };
    db.put(&inside[0], b"back");
    db.put(&fresh, b"new");
    db.commit().unwrap();
    let want_back = Some(&vec![b"back".to_vec()]);
    let want_new = Some(&vec![b"new".to_vec()]);
    let got = scan_model(&db);
    assert_eq!(
        got.get(&inside[0]),
        want_back,
        "the key written back, in the writer's scan"
    );
    assert_eq!(
        got.get(&fresh),
        want_new,
        "the new key, in the writer's scan"
    );
    assert_eq!(got.len(), before.len() + 2);
    // A scan from inside the range, and one across its lower fence.
    let mut from_inside: Vec<Vec<u8>> = Vec::new();
    db.scan(&inside[0], 2, kv(&mut |k, _| from_inside.push(k.to_vec())))
        .unwrap();
    assert_eq!(from_inside, vec![inside[0].clone(), fresh.clone()]);
    let below: Vec<&Vec<u8>> = keys.iter().filter(|k| **k < lo).collect();
    let mut across: Vec<Vec<u8>> = Vec::new();
    db.scan(
        below[below.len() - 1],
        3,
        kv(&mut |k, _| across.push(k.to_vec())),
    )
    .unwrap();
    assert_eq!(
        across,
        vec![
            below[below.len() - 1].clone(),
            inside[0].clone(),
            fresh.clone()
        ],
        "a scan across the empty partition's lower fence"
    );
    // A handle's scan, through the published forms.
    let r = db.reader().unwrap();
    let mut via_handle = std::collections::BTreeMap::new();
    r.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            via_handle
                .entry(k.to_vec())
                .or_insert_with(Vec::new)
                .push(v.to_vec());
        }),
    )
    .unwrap();
    assert_eq!(via_handle.get(&inside[0]), want_back, "the handle's scan");
    assert_eq!(via_handle.get(&fresh), want_new, "the handle's scan");
    drop(r);
    // Sealed into a piece over the empty partition, the two keys are the
    // piece's over its one block.
    db.seal().unwrap();
    db.settle().unwrap();
    let got = scan_model(&db);
    assert_eq!(
        got.get(&inside[0]),
        want_back,
        "the key written back, sealed"
    );
    assert_eq!(got.get(&fresh), want_new, "the new key, sealed");
    assert_eq!(got.len(), before.len() + 2);
    let mut n = 0;
    db.read_all(&fresh, vf(&mut |_| n += 1)).unwrap();
    assert_eq!(n, 1, "the point read");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A table is carried across the merge that rewrites its partition over
/// the same block boundaries, and the keys filed since the snapshot were
/// carried with the cut each had resolved to: a rank in the partition
/// before the rewrite. A key the merge folded in or dropped inside the
/// block moves every rank after it, so a form built from the kept cut
/// put the filed key at the wrong record: a scan emitted the record the
/// key was to mask and the key itself after its successors, with the
/// point read beside it right.
/// A copy whose entries are its partition's records, kept across a merge
/// that rewrites the partition over the same blocks, holds the old
/// partition's records: a key dropped and a key folded in one block keep
/// the boundaries and the count, and every record between them moves a
/// rank. The writes that did it reach the copy only after the rebase --
/// lent to the upkeep thread, held before its pass, and settled through
/// the tables the freezes replaced -- so the folded key's patch is made
/// against the new partition's cut. A copy that still claimed its
/// identity took that cut for its own entry: in the checked profile the
/// assertion beside the patch, in a release build the folded key's value
/// written over another key's entry.
#[test]
fn a_copys_identity_is_its_partitions_and_not_the_one_a_rewrite_left() {
    let dir = scratch("copy-identity-rewrite");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(4096),
            partition_on_flush: true,
            publish_in_background: true,
            upkeep: Upkeep::Background(1),
            upkeep_pace_syncs_bp: 0,
            scan_block_cache: true,
            l0_trigger: 2,
            ..common::options()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let fences = partition_fences(&dir);
    assert!(fences.len() >= 3, "partitions: {}", fences.len());
    let (lo, hi) = {
        let (lo, hi) = &fences[fences.len() / 2];
        (lo.clone(), hi.clone().expect("a closed partition"))
    };
    let inside: Vec<Vec<u8>> = keys
        .iter()
        .filter(|k| **k >= lo && **k < hi)
        .cloned()
        .collect();
    let n = inside.len();
    assert!((26..=64).contains(&n), "keys inside the range: {n}");
    // The block's copy, built over updates alone: every entry a record.
    let _ = scan_model(&db);
    for k in &inside[10..] {
        db.put(k, b"updated");
    }
    db.commit().unwrap();
    db.settle().unwrap();
    let _ = scan_model(&db);
    let (claimed, holds) = db.block_cache_identity();
    assert!(
        claimed >= 1 && holds == claimed,
        "a copy of the partition's records: {claimed} claimed, {holds} of its partition"
    );
    // A key dropped below a key folded in, in the copy's block: the
    // records between them move down a rank and the block's first key and
    // count stay, so the merge's rewrite keeps the layout and the copy.
    let dropped = inside[2].clone();
    let folded = {
        let mut k = inside[5].clone();
        k.push(b'x');
        k
    };
    let rebased_before = db.forms_rebased();
    let merges_before = db.seal_waits().merges;
    let kept_before = db.seal_waits().keyseqs_kept;
    db.hold_upkeep_pass(true);
    db.hold_seal_durable(true);
    for _ in 0..2 {
        db.delete(&dropped);
        db.put(&folded, b"folded");
        db.commit().unwrap();
        db.seal().unwrap();
    }
    db.hold_seal_durable(false);
    let t = std::time::Instant::now();
    while db.seal_waits().merges == merges_before {
        assert!(t.elapsed().as_secs() < 60, "the merge never landed");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    db.hold_upkeep_pass(false);
    db.settle().unwrap();
    db.settle().unwrap();
    assert!(
        db.forms_rebased() > rebased_before,
        "the rewrite kept the partition's layout: {} before, {} after",
        rebased_before,
        db.forms_rebased()
    );
    // The output holds a key its partition did not, as many keys as it
    // and between the same fences: not that partition's key sequence
    // (`Options::identity_by_keys`).
    assert_eq!(
        db.seal_waits().keyseqs_kept,
        kept_before,
        "an output with a key folded in took the key sequence of the partition it rewrote"
    );
    let mut want = std::collections::BTreeMap::new();
    for k in &keys {
        let v = if inside[10..].contains(k) {
            b"updated".to_vec()
        } else {
            value.to_vec()
        };
        want.insert(k.clone(), vec![v]);
    }
    want.remove(&dropped);
    want.insert(folded.clone(), vec![b"folded".to_vec()]);
    assert_eq!(scan_model(&db), want, "the writer's scan");
    let r = db.reader().unwrap();
    let mut via_handle = std::collections::BTreeMap::new();
    r.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            via_handle
                .entry(k.to_vec())
                .or_insert_with(Vec::new)
                .push(v.to_vec());
        }),
    )
    .unwrap();
    assert_eq!(via_handle, want, "the handle's scan");
    drop(r);
    // The writes after the rewrite patch the copy where its entries are,
    // found by search now that the identity is of the old partition.
    db.put(&folded, b"again");
    db.put(&inside[6], b"again");
    db.commit().unwrap();
    db.settle().unwrap();
    want.insert(folded.clone(), vec![b"again".to_vec()]);
    want.insert(inside[6].clone(), vec![b"again".to_vec()]);
    assert_eq!(scan_model(&db), want, "the writer's scan after the patches");
    for (k, v) in &want {
        let mut got = Vec::new();
        db.read_all(k, vf(&mut |x| got.push(x.to_vec()))).unwrap();
        assert_eq!(
            &got,
            v,
            "the point read of {:?}",
            String::from_utf8_lossy(k)
        );
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_filed_keys_cut_is_resolved_again_across_its_partitions_rewrite() {
    let dir = scratch("rebase-filed-cuts");
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1024),
            partition_on_flush: true,
            publish_in_background: true,
            upkeep: Upkeep::Inline,
            scan_block_cache: true,
            l0_trigger: 2,
            ..common::options()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    for k in &keys {
        db.put(k, &value);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // A partition in the middle, and its keys: the first and the last are
    // its block's boundaries and stay, one inside is dropped and one is
    // folded in, so the rewrite keeps the boundaries and the count and
    // moves the ranks between.
    let fences = partition_fences(&dir);
    assert!(fences.len() >= 3, "partitions: {}", fences.len());
    let (lo, hi) = {
        let (lo, hi) = &fences[fences.len() / 2];
        (lo.clone(), hi.clone().expect("a closed partition"))
    };
    let inside: Vec<Vec<u8>> = keys
        .iter()
        .filter(|k| **k >= lo && **k < hi)
        .cloned()
        .collect();
    let n = inside.len();
    assert!(n >= 5, "keys inside the range: {n}");
    let dropped = inside[1].clone();
    let folded = {
        let mut k = inside[n - 2].clone();
        k.push(b'x');
        k
    };
    // The block path stands once a scan has built its tables.
    let before = scan_model(&db);
    assert_eq!(before.len(), 300);
    let rebased_before = db.forms_rebased();
    // Two pieces over the range, each dropping and folding the same keys,
    // held between their phases so the merge they trigger lands only
    // once the hold lifts.
    db.hold_seal_durable(true);
    for _ in 0..2 {
        db.delete(&dropped);
        db.put(&folded, b"folded");
        db.commit().unwrap();
        db.seal().unwrap();
    }
    // The writer's look across the two freezes, and a snapshot built over
    // what the seals left -- a few keys written elsewhere, so that the
    // one key written after the snapshot does not outnumber it, which
    // would have the next scan build the snapshot again and empty the
    // lists -- so that the key written next is one created since the
    // snapshot: filed into its block's list with its cut.
    for i in 0..8u8 {
        db.put(format!("zz-elsewhere-{i}").as_bytes(), b"elsewhere");
    }
    db.commit().unwrap();
    let _ = scan_model(&db);
    // A key filed against the partition as it stands, between the
    // dropped key and the folded one: its cut is a rank the rewrite will
    // move down by one.
    let filed = {
        let mut k = inside[n / 2].clone();
        k.push(b'y');
        k
    };
    db.put(&filed, b"filed");
    db.commit().unwrap();
    db.hold_seal_durable(false);
    db.settle().unwrap();
    db.settle().unwrap();
    assert!(
        db.forms_rebased() > rebased_before,
        "the rewrite kept the partition's layout: {} before, {} after",
        rebased_before,
        db.forms_rebased()
    );
    let mut order: Vec<Vec<u8>> = Vec::new();
    let got = {
        let mut m = std::collections::BTreeMap::new();
        db.scan(
            b"",
            usize::MAX,
            kv(&mut |k, v| {
                if order.last().map(|l| l.as_slice()) != Some(k) {
                    order.push(k.to_vec());
                }
                m.entry(k.to_vec())
                    .or_insert_with(Vec::new)
                    .push(v.to_vec());
            }),
        )
        .unwrap();
        m
    };
    let mut sorted = order.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(order, sorted, "the scan's keys, in order and once each");
    assert_eq!(
        got.get(&filed),
        Some(&vec![b"filed".to_vec()]),
        "the filed key"
    );
    assert_eq!(
        got.get(&folded),
        Some(&vec![b"folded".to_vec()]),
        "the folded key"
    );
    assert!(!got.contains_key(&dropped), "the dropped key");
    assert_eq!(got.len(), 300 + 1 + 8, "the keys");
    let r = db.reader().unwrap();
    let mut via_handle = std::collections::BTreeMap::new();
    r.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            via_handle
                .entry(k.to_vec())
                .or_insert_with(Vec::new)
                .push(v.to_vec());
        }),
    )
    .unwrap();
    assert_eq!(via_handle, got, "the handle's scan");
    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sparse form names each delta's place by a rank in its partition's
/// records, and the look that carries a table across the merge that
/// rewrites the partition over the same blocks dropped every sparse form
/// for it, leaving the block to be built again. The form is kept with each
/// cut resolved again (`Options::rebase_recuts_sparse`): here the rewrite
/// drops a key below the form's deltas and folds one in above them, so a
/// delta that updated a record finds it a rank lower, one that deleted a
/// record finds it gone, and one that inserted a key finds it a record. A
/// cut left as it was masks the wrong record and emits a key twice.
#[test]
fn a_sparse_forms_cuts_are_resolved_again_across_its_partitions_rewrite() {
    for recut in [true, false] {
        let dir = scratch(&format!("rebase-sparse-cuts-{recut}"));
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 1 << 20,
                // Some sixty keys a partition, a block of its own: the
                // overlay below is a few of them, so the form a scan
                // builds over it is sparse, where over a block of eight
                // records it is most of the block and the form is wide.
                partition_bytes: Some(8 << 10),
                partition_on_flush: true,
                publish_in_background: true,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                l0_trigger: 2,
                rebase_recuts_sparse: recut,
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
        for k in &keys {
            db.put(k, &value);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let fences = partition_fences(&dir);
        assert!(fences.len() >= 3, "partitions: {}", fences.len());
        let (lo, hi) = {
            let (lo, hi) = &fences[fences.len() / 2];
            (lo.clone(), hi.clone().expect("a closed partition"))
        };
        let inside: Vec<Vec<u8>> = keys
            .iter()
            .filter(|k| **k >= lo && **k < hi)
            .cloned()
            .collect();
        let n = inside.len();
        assert!(n >= 16, "keys inside the range: {n}");
        // The rewrite drops the partition's second key and folds one in
        // after its ninth, so it keeps every block's first key, the last
        // key and the count, and moves each rank between the two down by
        // one; the two keys written into the block sit there.
        let dropped = inside[1].clone();
        let folded = {
            let mut k = inside[8].clone();
            k.push(b'x');
            k
        };
        let updated = inside[4].clone();
        let inserted = {
            let mut k = updated.clone();
            k.push(b'y');
            k
        };
        assert_eq!(scan_model(&db).len(), 300);
        // Two pieces over the range, held between their phases so the
        // merge they trigger lands only once the hold lifts.
        db.hold_seal_durable(true);
        for _ in 0..2 {
            db.delete(&dropped);
            db.put(&folded, b"folded");
            db.commit().unwrap();
            db.seal().unwrap();
        }
        // Two more keys of the block, standing in the memtable across the
        // rewrite, and the scan that builds the block's form over all
        // four: sparse, each delta at a rank of the partition as it
        // stands.
        db.put(&updated, b"updated");
        db.put(&inserted, b"inserted");
        db.commit().unwrap();
        let mid = scan_model(&db);
        assert_eq!(mid.get(&updated), Some(&vec![b"updated".to_vec()]));
        assert_eq!(mid.get(&inserted), Some(&vec![b"inserted".to_vec()]));
        assert_eq!(mid.get(&folded), Some(&vec![b"folded".to_vec()]));
        assert!(!mid.contains_key(&dropped));
        let rebased0 = db.forms_rebased();
        let (recut0, _) = db.forms_recut();
        let (kinds0, _, _) = db.build_kinds();
        db.hold_seal_durable(false);
        db.settle().unwrap();
        db.settle().unwrap();
        let mut order: Vec<Vec<u8>> = Vec::new();
        let got = {
            let mut m = std::collections::BTreeMap::new();
            db.scan(
                b"",
                usize::MAX,
                kv(&mut |k, v| {
                    if order.last().map(|l| l.as_slice()) != Some(k) {
                        order.push(k.to_vec());
                    }
                    m.entry(k.to_vec())
                        .or_insert_with(Vec::new)
                        .push(v.to_vec());
                }),
            )
            .unwrap();
            m
        };
        assert!(
            db.forms_rebased() > rebased0,
            "the rewrite kept the partition's layout ({recut})"
        );
        let (forms, deltas) = db.forms_recut();
        let (kinds1, _, _) = db.build_kinds();
        if recut {
            assert!(forms > recut0, "the block's sparse form was kept");
            assert!(deltas >= 4, "its four deltas resolved again: {deltas}");
            assert_eq!(
                kinds1[1], kinds0[1],
                "no sparse form built again across the rewrite"
            );
        } else {
            assert_eq!(forms, recut0, "the arm drops the form");
            assert!(
                kinds1[1] > kinds0[1],
                "the dropped form's block built again: {kinds0:?} then {kinds1:?}"
            );
        }
        let mut sorted = order.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            order, sorted,
            "the scan's keys, in order and once each ({recut})"
        );
        assert_eq!(got.get(&updated), Some(&vec![b"updated".to_vec()]));
        assert_eq!(got.get(&inserted), Some(&vec![b"inserted".to_vec()]));
        assert_eq!(got.get(&folded), Some(&vec![b"folded".to_vec()]));
        assert!(!got.contains_key(&dropped), "the dropped key ({recut})");
        for k in &inside {
            if *k != dropped && *k != updated {
                assert_eq!(got.get(k), Some(&vec![value.to_vec()]), "an untouched key");
            }
        }
        assert_eq!(got.len(), 300 + 1, "the keys ({recut})");
        // A handle's scan, through the forms the writer published after
        // the look.
        let r = db.reader().unwrap();
        let mut via_handle = std::collections::BTreeMap::new();
        r.scan(
            b"",
            usize::MAX,
            kv(&mut |k, v| {
                via_handle
                    .entry(k.to_vec())
                    .or_insert_with(Vec::new)
                    .push(v.to_vec());
            }),
        )
        .unwrap();
        assert_eq!(via_handle, got, "the handle's scan ({recut})");
        drop(r);
        // The form stands for the writes after it: a patch by the cut the
        // settle resolves against the rewritten partition finds its delta.
        db.put(&updated, b"again");
        db.delete(&inserted);
        db.commit().unwrap();
        let after = scan_model(&db);
        assert_eq!(after.get(&updated), Some(&vec![b"again".to_vec()]));
        assert!(!after.contains_key(&inserted), "the key deleted since");
        assert_eq!(after.len(), 300, "the keys after ({recut})");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The scan snapshot's arena answered a reservation of no bytes -- the
/// empty key's -- with its tail and no block when the keys were reserved
/// in one batch, and the read of those no bytes asked for the block: a
/// snapshot built over the empty key alone, which is what the first write
/// after a flush leaves, panicked the thread that sought the next
/// partition's fence in it, the upkeep's or the scan's own. A read of no
/// bytes touches no block now, and the scan answers the key.
#[test]
fn a_snapshot_over_the_empty_key_alone_is_sought_at_a_partitions_fence() {
    for runs in [false, true] {
        let dir = scratch(&format!("empty-key-alone-{runs}"));
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 1 << 20,
                partition_bytes: Some(1024),
                partition_on_flush: true,
                publish_in_background: false,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                snapshot_runs: runs,
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        for i in 0..300 {
            db.put(format!("k{i:05}").as_bytes(), &value);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        assert!(
            partition_fences(&dir).len() > 1,
            "the shape needs a fence to seek in the snapshot (runs {runs})"
        );
        // The empty key alone in the live table: the snapshot over it
        // holds one key of no bytes.
        db.append(b"", b"e");
        db.commit().unwrap();
        let got = scan_model(&db);
        assert_eq!(got.len(), 301, "runs {runs}: every key and the empty one");
        assert_eq!(got.get(&b""[..]), Some(&vec![b"e".to_vec()]), "runs {runs}");
        let r = db.reader().unwrap();
        let mut seen = Vec::new();
        r.scan(b"", 3, kv(&mut |k, _| seen.push(k.to_vec())))
            .unwrap();
        assert_eq!(
            seen[0], b"",
            "runs {runs}: the handle's scan starts at the empty key"
        );
        let mut n = 0;
        db.read_all(b"", vf(&mut |_| n += 1)).unwrap();
        assert_eq!(n, 1, "runs {runs}: the point read");
        drop(r);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Under `Options::forms_settle_every` a commit near reads files its own
/// batch: after a scan, a stretch of commits with no scan between them
/// leaves the first scan after it nothing to settle, where the default
/// files the first commit's batch and leaves that scan the rest. Both
/// scans answer every write of the stretch.
#[test]
fn a_commit_near_reads_files_its_batch_under_settle_every() {
    for every in [false, true] {
        let dir = scratch(&format!("settle-every-{every}"));
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 1 << 20,
                partition_bytes: Some(4096),
                partition_on_flush: true,
                publish_in_background: false,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                forms_settle_every: every,
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        let keys: Vec<Vec<u8>> = (0..2000).map(|i| format!("k{i:05}").into_bytes()).collect();
        for k in &keys {
            db.put(k, &value);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        // A scan: reads are around from here.
        let mut want = scan_model(&db);
        assert_eq!(want.len(), 2000);
        // Ten commits of three updates each, thirty writes, under the
        // default bound's two percent of the store, and no scan between.
        for round in 0..10usize {
            for i in 0..3usize {
                let k = &keys[(round * 191 + i * 613) % keys.len()];
                db.append(k, b"u");
                want.get_mut(k).unwrap().push(b"u".to_vec());
            }
            db.commit().unwrap();
        }
        let before = db.settle_density();
        let got = scan_model(&db);
        let after = db.settle_density();
        assert!(got == want, "every={every}: the scan answers every write");
        if every {
            assert_eq!(
                after[1], before[1],
                "the commits filed their batches; the scan settled nothing"
            );
        } else {
            assert!(
                after[1] > before[1],
                "by default the scan files what the commits left: {before:?} {after:?}"
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Under `Options::forms_settle_builds` the commit's fill builds a form for
/// every block its settle filed into that held none, where under a cache
/// budget it builds no form at all: after a scan that
/// formed a few blocks, a stretch of commits over the rest leaves the next
/// full scan no sparse block to build, where without it that scan builds
/// one for every block the stretch wrote into. Both scans answer every
/// write.
#[test]
fn a_commits_fill_builds_the_blocks_its_settle_filed_into() {
    for builds in [false, true] {
        let dir = scratch(&format!("settle-builds-{builds}"));
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 1 << 20,
                partition_bytes: Some(64 << 10),
                partition_on_flush: true,
                publish_in_background: false,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                forms_settle_every: true,
                forms_settle_builds: builds,
                // A budget, as the suite's arms run under, and one large
                // enough to shed nothing: without one the fill completes
                // every overlaid block's form whatever this option says.
                scan_cache_bytes: 1 << 30,
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        let keys: Vec<Vec<u8>> = (0..2000).map(|i| format!("k{i:05}").into_bytes()).collect();
        let mut want = std::collections::BTreeMap::new();
        for k in &keys {
            db.put(k, &value);
            want.insert(k.clone(), vec![value.to_vec()]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        // A scan of one key at each partition's start: reads are around,
        // every partition has its table, as the suite's passes leave them,
        // and only each partition's first block holds a form.
        let fences = partition_fences(&dir);
        assert!(
            fences.len() >= 3,
            "partitions of several blocks: {}",
            fences.len()
        );
        for (lo, _) in &fences {
            db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
        }
        for round in 0..10usize {
            for i in 0..3usize {
                let k = &keys[(round * 191 + i * 613 + 300) % keys.len()];
                db.append(k, b"u");
                want.get_mut(k).unwrap().push(b"u".to_vec());
            }
            db.commit().unwrap();
        }
        let sparse_before = db.build_kinds().0[1];
        let filled = db.settle_built();
        let got = scan_model(&db);
        let sparse_after = db.build_kinds().0[1];
        assert!(got == want, "builds={builds}: the scan answers every write");
        if builds {
            assert!(filled > 0, "the fills built the stretch's blocks");
            assert_eq!(
                sparse_after, sparse_before,
                "the scan found the stretch's blocks formed: {filled} built by the fills"
            );
        } else {
            assert_eq!(filled, 0);
            assert!(
                sparse_after > sparse_before,
                "without, the scan builds them: {sparse_before} {sparse_after}"
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Under `Options::forms_settle_builds` the fill builds only while the
/// memory group's file refaults hold still and the bound leaves room past
/// what the reads keep resident (`SettleRoom`), here the rate and the
/// budget a test sets through the seam: none built while the rate reads
/// past zero or the budget leaves nothing, and
/// when it rises after some were built, the ones no scan has walked since
/// are shed at the next fill. Every scan answers every write throughout.
#[test]
fn the_fills_build_only_while_the_refaults_hold_still() {
    let dir = scratch("settle-room");
    let inject = std::sync::Arc::new(supdb::ZInject::new());
    inject.set_refault_rate(1000.0);
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(64 << 10),
            partition_on_flush: true,
            publish_in_background: false,
            upkeep: Upkeep::Inline,
            scan_block_cache: true,
            forms_settle_every: true,
            forms_settle_builds: true,
            scan_cache_bytes: 1 << 30,
            zpages_inject: Some(inject.clone()),
            ..common::options()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..2000).map(|i| format!("k{i:05}").into_bytes()).collect();
    let mut want = std::collections::BTreeMap::new();
    for k in &keys {
        db.put(k, &value);
        want.insert(k.clone(), vec![value.to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    for (lo, _) in &partition_fences(&dir) {
        db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
    }
    let stretch =
        |db: &mut Db, want: &mut std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>, at: usize| {
            for round in 0..10usize {
                for i in 0..3usize {
                    let k = &keys[(round * 191 + i * 613 + at) % keys.len()];
                    db.append(k, b"u");
                    want.get_mut(k).unwrap().push(b"u".to_vec());
                }
                db.commit().unwrap();
            }
        };
    // Refaulting: nothing built.
    stretch(&mut db, &mut want, 300);
    assert_eq!(db.settle_built(), 0, "no room, no build");
    // Quiet, but no room left in bytes: the budget the bound leaves is
    // the form's own reads' already, and nothing is built.
    inject.set_refault_rate(0.0);
    inject.set_budget(1);
    stretch(&mut db, &mut want, 600);
    assert_eq!(db.settle_built(), 0, "no room in bytes, no build");
    // And no filing at every commit either: the scan after the stretch
    // files what the commits left, as without the option. A scan of one
    // key, so that the blocks the next phase builds are unwalked.
    let filed = db.settle_density()[1];
    db.scan(&keys[0], 1, kv(&mut |_, _| {})).unwrap();
    assert!(
        db.settle_density()[1] > filed,
        "no room: the commits leave the filing to the scan"
    );
    // Quiet and room: the stretch's blocks built.
    inject.set_budget(1 << 50);
    stretch(&mut db, &mut want, 900);
    let built = db.settle_built();
    assert!(built > 0, "room, builds");
    assert_eq!(db.settle_shed(), 0);
    // Refaulting again: the next fill sheds what was built and not walked.
    inject.set_refault_rate(1000.0);
    stretch(&mut db, &mut want, 1500);
    assert_eq!(db.settle_built(), built, "no room, no build");
    assert!(db.settle_shed() > 0, "the unwalked forms shed");
    assert!(scan_model(&db) == want, "the scan answers every write");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What closes the fills' room is the group refaulting past the line the
/// controller's pressure term is drawn at, not a refault
/// (`Options::forms_settle_room_pressure`): at a rate under the line the
/// fills build, and past it they build nothing; with the option off any
/// rate past zero closes the room, the shape before. The rate is the
/// seam's; the platform's readings go through `refaults_moved`, which the
/// engine's own test holds.
#[test]
fn the_fills_room_is_closed_by_pressure_and_not_by_a_refault() {
    for (case, pressure) in [("pressure", true), ("still", false)] {
        let dir = scratch(&format!("settle-room-{case}"));
        let inject = std::sync::Arc::new(supdb::ZInject::new());
        inject.set_refault_rate(100.0);
        inject.set_budget(1 << 50);
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 1 << 20,
                partition_bytes: Some(64 << 10),
                partition_on_flush: true,
                publish_in_background: false,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                forms_settle_every: true,
                forms_settle_builds: true,
                forms_settle_room_pressure: pressure,
                scan_cache_bytes: 1 << 30,
                zpages_inject: Some(inject.clone()),
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        let keys: Vec<Vec<u8>> = (0..2000).map(|i| format!("k{i:05}").into_bytes()).collect();
        let mut want = std::collections::BTreeMap::new();
        for k in &keys {
            db.put(k, &value);
            want.insert(k.clone(), vec![value.to_vec()]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        for (lo, _) in &partition_fences(&dir) {
            db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
        }
        let stretch = |db: &mut Db,
                       want: &mut std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
                       at: usize| {
            for round in 0..10usize {
                for i in 0..3usize {
                    let k = &keys[(round * 191 + i * 613 + at) % keys.len()];
                    db.append(k, b"u");
                    want.get_mut(k).unwrap().push(b"u".to_vec());
                }
                db.commit().unwrap();
            }
        };
        // A hundred refaults a second, under the line.
        stretch(&mut db, &mut want, 300);
        let built = db.settle_built();
        assert_eq!(
            built > 0,
            pressure,
            "{case}: a rate under the pressure line, {built} built"
        );
        let refused = db.settle_refusals();
        assert_eq!(
            refused[2] > 0,
            !pressure,
            "{case}: commits refused for want of room: {refused:?}"
        );
        // Past the line: nothing more under either.
        inject.set_refault_rate(300.0);
        stretch(&mut db, &mut want, 900);
        assert_eq!(db.settle_built(), built, "{case}: past the line, no build");
        assert!(
            db.settle_refusals()[2] > refused[2],
            "{case}: and the commits counted as refused for room"
        );
        assert!(
            scan_model(&db) == want,
            "{case}: the scan answers every write"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The fills list every form they build, for the shed that takes the
/// unwalked ones back when their room goes, and only a shed emptied the
/// list: in a store whose room never goes, which is any store in memory,
/// it grew by an entry a build for the store's life. It is pruned to the
/// forms a shed could still take once it holds twice the store's blocks.
#[test]
fn the_list_of_the_fills_forms_is_bounded_by_the_stores_blocks() {
    let dir = scratch("settle-built-list");
    let inject = std::sync::Arc::new(supdb::ZInject::new());
    inject.set_refault_rate(0.0);
    inject.set_budget(1 << 50);
    let mut db = Db::create(
        &dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(64 << 10),
            partition_on_flush: true,
            publish_in_background: false,
            upkeep: Upkeep::Inline,
            scan_block_cache: true,
            forms_settle_every: true,
            forms_settle_builds: true,
            // A rewrite drops the partition's sparse forms, so that every
            // round below builds its blocks' forms again.
            rebase_recuts_sparse: false,
            zpages_inject: Some(inject.clone()),
            ..common::options()
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..2000).map(|i| format!("k{i:05}").into_bytes()).collect();
    let mut want = std::collections::BTreeMap::new();
    for k in &keys {
        db.put(k, &value);
        want.insert(k.clone(), vec![value.to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let fences = partition_fences(&dir);
    for round in 0..48usize {
        // Near reads, then a write into every block or so, filed and built
        // by the commit, and the flush whose rewrite drops the forms.
        for (lo, _) in &fences {
            db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
        }
        for i in (0..keys.len()).step_by(40) {
            let k = &keys[(i + round) % keys.len()];
            db.append(k, b"u");
            want.get_mut(k).unwrap().push(b"u".to_vec());
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
    }
    let built = db.settle_built();
    assert!(built >= 600, "the rounds' fills built: {built}");
    assert_eq!(db.settle_shed(), 0, "and nothing shed them");
    let listed = db.settle_built_listed();
    assert!(
        listed <= 200,
        "the list holds what a shed could take, not every build: {listed} of {built}"
    );
    assert!(scan_model(&db) == want, "the scan answers every write");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The forms the fills built are shed where their room goes, for the page
/// cache to have back what they took. While the cache's budget is not
/// bound the commit's own fill builds a form for every overlaid block that
/// holds none, so the pass that shed them built each again, from the
/// sources: a shed the same pass undoes, and a second build of every form
/// it took. There is no shed there (`Options::forms_settle_sheds_unbound`
/// off); the arm sheds, and its fill builds what it shed again.
#[test]
fn the_fills_forms_are_not_shed_for_a_fill_that_builds_them_again() {
    for sheds in [false, true] {
        let dir = scratch(&format!("settle-shed-unbound-{sheds}"));
        let inject = std::sync::Arc::new(supdb::ZInject::new());
        inject.set_refault_rate(0.0);
        inject.set_budget(1 << 50);
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 1 << 20,
                partition_bytes: Some(64 << 10),
                partition_on_flush: true,
                publish_in_background: false,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                forms_settle_every: true,
                forms_settle_builds: true,
                forms_settle_sheds_unbound: sheds,
                zpages_inject: Some(inject.clone()),
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        let keys: Vec<Vec<u8>> = (0..2000).map(|i| format!("k{i:05}").into_bytes()).collect();
        let mut want = std::collections::BTreeMap::new();
        for k in &keys {
            db.put(k, &value);
            want.insert(k.clone(), vec![value.to_vec()]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        for (lo, _) in &partition_fences(&dir) {
            db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
        }
        let stretch = |db: &mut Db,
                       want: &mut std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
                       at: usize| {
            for round in 0..10usize {
                for i in 0..3usize {
                    let k = &keys[(round * 191 + i * 613 + at) % keys.len()];
                    db.append(k, b"u");
                    want.get_mut(k).unwrap().push(b"u".to_vec());
                }
                db.commit().unwrap();
            }
        };
        // Quiet and room: the stretch's blocks built by the fills, unwalked.
        stretch(&mut db, &mut want, 900);
        let built = db.settle_built();
        assert!(built > 0, "room, builds ({sheds})");
        assert_eq!(db.settle_shed(), 0);
        // Refaulting: the room goes at the next fill.
        inject.set_refault_rate(1000.0);
        stretch(&mut db, &mut want, 1500);
        let shed = db.settle_shed();
        if sheds {
            assert!(shed > 0, "the arm sheds the unwalked forms");
        } else {
            assert_eq!(shed, 0, "no shed for a fill that builds the forms again");
            assert!(
                db.settle_sheds_declined() > 0,
                "the room went and was seen to"
            );
        }
        // Quiet again, and the first stretch's keys written once more: the
        // forms kept are patched, and the forms shed are built a second
        // time, block for block.
        inject.set_refault_rate(0.0);
        stretch(&mut db, &mut want, 900);
        let again = db.settle_built() - built;
        if sheds {
            assert_eq!(shed, built, "every form of the first stretch was shed");
            assert!(
                again >= shed,
                "the shed forms built again: {again} since for {shed} shed"
            );
        } else {
            // The blocks the refaulting stretch wrote into and nothing
            // formed are built now; the first stretch's are not.
            assert!(
                again < built,
                "the forms kept, none of them built twice: {again} since, {built} before"
            );
        }
        assert!(
            scan_model(&db) == want,
            "the scan answers every write ({sheds})"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A settle resolves each of a batch's keys to its block by a seek over
/// its partition's heads, the batch in key order. The stride each seek
/// searches is named a few keys ahead from the ordered index's top level
/// and asked of the core, and the key is then sought in its stride alone
/// (`Options::settle_seek_ahead`); zero is the gallop from the rank the
/// key before it reached. Either way a batch is filed where it belongs:
/// keys updated, inserted between records, below every key and past every
/// key, written twice in a batch, dense and scattered, over keys whose
/// heads are whole keys and over keys whose heads tie, all of them in the
/// scan through the cache.
#[test]
fn a_settles_seeks_planned_ahead_file_a_batch_as_the_gallop_does() {
    for shape in ["whole", "tied"] {
        for ahead in [0usize, 1, 8, 64] {
            let dir = scratch(&format!("settle-seek-ahead-{shape}-{ahead}"));
            let mut db = Db::create(
                &dir,
                Options {
                    seal_bytes: 1 << 20,
                    partition_bytes: Some(64 << 10),
                    partition_on_flush: true,
                    publish_in_background: false,
                    upkeep: Upkeep::Inline,
                    scan_block_cache: true,
                    forms_settle_every: true,
                    settle_seek_ahead: ahead,
                    ..common::options()
                },
            )
            .unwrap();
            let value = [b'v'; 100];
            let keys: Vec<Vec<u8>> = (0..4000usize)
                .map(|i| match shape {
                    "whole" => format!("k{:05}", i * 2).into_bytes(),
                    // Fifty keys to a head: the eight bytes past the common
                    // prefix are the group's, and the records break the tie.
                    _ => format!("{:04}-tail-{:04}", i / 50, (i % 50) * 2).into_bytes(),
                })
                .collect();
            let mut want = std::collections::BTreeMap::new();
            for k in &keys {
                db.put(k, &value);
                want.insert(k.clone(), vec![value.to_vec()]);
            }
            db.commit().unwrap();
            db.flush().unwrap();
            db.settle().unwrap();
            let fences = partition_fences(&dir);
            assert!(fences.len() >= 3, "several partitions: {}", fences.len());
            let mut under = 0u64;
            for (round, step) in [3usize, 40, 331, 7, 1].into_iter().enumerate() {
                // Near reads, so the commit files its batch.
                for (lo, _) in &fences {
                    db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
                }
                for i in (round..keys.len()).step_by(step) {
                    let k = &keys[i];
                    db.append(k, b"u");
                    want.get_mut(k).unwrap().push(b"u".to_vec());
                    under += 1;
                    if i % 3 == 0 {
                        // Between this record and the next.
                        let mut ins = k.clone();
                        ins.push(b'm');
                        db.append(&ins, b"i");
                        want.entry(ins).or_insert_with(Vec::new).push(b"i".to_vec());
                        under += 1;
                    }
                    if i % 5 == 0 {
                        // The key a second time in the batch.
                        db.append(k, b"w");
                        want.get_mut(k).unwrap().push(b"w".to_vec());
                    }
                }
                // Outside the keys' common prefix, below and above them all.
                for (k, v) in [(&b"!"[..], &b"lo"[..]), (&b"~"[..], &b"hi"[..])] {
                    db.append(k, v);
                    want.entry(k.to_vec())
                        .or_insert_with(Vec::new)
                        .push(v.to_vec());
                }
                db.commit().unwrap();
                assert!(
                    scan_model(&db) == want,
                    "{shape}, {ahead} ahead, round {round}: the scan answers every write"
                );
            }
            let planned = db.seeks_planned();
            if ahead == 0 {
                assert_eq!(planned, 0, "{shape}: the gallop plans nothing");
            } else {
                // Every round but a batch the scan rebuilt its tables for
                // is resolved key by key, each key under the prefix planned.
                assert!(
                    planned >= under / 4,
                    "{shape}, {ahead} ahead: the resolves were planned: {planned} of {under}"
                );
            }
            db.flush().unwrap();
            db.settle().unwrap();
            assert!(
                scan_model(&db) == want,
                "{shape}, {ahead} ahead: and after the flush"
            );
            drop(db);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// A settle's patch into a copy whose entries are its block's records
/// writes the key's run at the entry its rank names and searches nothing,
/// so the patch loop asks for that entry's line and the run it names by
/// the cut, a key or three ahead, where it asked for the whole form
/// (`Options::settle_patch_at_cut`). The ask is a hint and the patch is
/// the same patch: the blocks rewritten until their forms are copies and
/// then written again, key by key and several keys a block, read back as
/// they were written under either, and only the option's patches are
/// counted as asked for by their cut.
#[test]
fn a_patch_by_its_rank_asks_for_its_lines_by_the_cut() {
    for at_cut in [true, false] {
        // The asks for the form's header and for the block table's own
        // lines ride along, on with the one and off with the other
        // (`Options::settle_form_ahead`, `Options::settle_table_ahead`):
        // hints too, counted where they are made.
        let (form_ahead, table_ahead) = if at_cut { (6, 12) } else { (0, 0) };
        let dir = scratch(&format!("settle-patch-at-cut-{at_cut}"));
        let inject = std::sync::Arc::new(supdb::ZInject::new());
        inject.set_refault_rate(0.0);
        inject.set_budget(1 << 50);
        let mut db = Db::create(
            &dir,
            Options {
                seal_bytes: 8 << 20,
                partition_bytes: Some(64 << 10),
                partition_on_flush: true,
                publish_in_background: false,
                upkeep: Upkeep::Inline,
                scan_block_cache: true,
                forms_settle_every: true,
                forms_settle_builds: true,
                settle_patch_at_cut: at_cut,
                settle_form_ahead: form_ahead,
                settle_table_ahead: table_ahead,
                zpages_inject: Some(inject.clone()),
                ..common::options()
            },
        )
        .unwrap();
        let value = [b'v'; 100];
        let keys: Vec<Vec<u8>> = (0..4000).map(|i| format!("k{i:05}").into_bytes()).collect();
        let mut want = std::collections::BTreeMap::new();
        for k in &keys {
            db.put(k, &value);
            want.insert(k.clone(), vec![value.to_vec()]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let fences = partition_fences(&dir);
        let round = |db: &mut Db,
                     want: &mut std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
                     step: usize,
                     tag: u8| {
            // A few hundred keys a commit, each commit near reads, so the
            // commit files its batch and its fill builds and converts.
            for chunk in (0..keys.len())
                .step_by(step)
                .collect::<Vec<_>>()
                .chunks(250)
            {
                for (lo, _) in &fences {
                    db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
                }
                for &i in chunk {
                    let v = [tag; 100];
                    db.put(&keys[i], &v);
                    want.insert(keys[i].clone(), vec![v.to_vec()]);
                }
                db.commit().unwrap();
            }
        };
        // Every other key, twice: thirty-two keys a block, past the count
        // a sparse form is converted to a copy at.
        round(&mut db, &mut want, 2, b'a');
        round(&mut db, &mut want, 2, b'b');
        assert!(scan_model(&db) == want, "{at_cut}: after the rewrites");
        let before = db.patch_asks();
        // The copies written again: one key a block or so, then several.
        round(&mut db, &mut want, 61, b'c');
        round(&mut db, &mut want, 3, b'd');
        let asked = db.patch_asks() - before;
        if at_cut {
            assert!(
                asked >= 500,
                "the patches into the copies were asked for by their cut: {asked}"
            );
        } else {
            assert_eq!(db.patch_asks(), 0, "every form asked for whole");
        }
        let [headers, lines] = db.settle_ahead_asks();
        if at_cut {
            assert!(
                headers >= 500 && lines >= 500,
                "the forms' headers and the table's lines were asked for ahead: {headers}, {lines}"
            );
        } else {
            assert_eq!(
                (headers, lines),
                (0, 0),
                "neither asked for with its option at zero"
            );
        }
        assert!(
            scan_model(&db) == want,
            "{at_cut}: the scan answers every write"
        );
        db.flush().unwrap();
        db.settle().unwrap();
        assert!(scan_model(&db) == want, "{at_cut}: and after the flush");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A store of `n` keys in key order, appended and flushed into partitions,
/// with its writer's upkeep inline: the stores of the two tests below.
fn appended(dir: &std::path::Path, n: usize, background: bool, o: Options) -> (Db, Vec<Vec<u8>>) {
    let inject = std::sync::Arc::new(supdb::ZInject::new());
    inject.set_refault_rate(0.0);
    inject.set_budget(1 << 50);
    let mut db = Db::create(
        dir,
        Options {
            seal_bytes: 8 << 20,
            partition_bytes: Some(64 << 10),
            partition_on_flush: true,
            publish_in_background: background,
            upkeep: Upkeep::Inline,
            scan_block_cache: true,
            forms_settle_every: true,
            forms_settle_builds: true,
            l0_trigger: 64,
            zpages_inject: Some(inject),
            ..o
        },
    )
    .unwrap();
    let keys: Vec<Vec<u8>> = (0..n).map(|i| format!("k{i:05}").into_bytes()).collect();
    // Out of key order, so the load goes through the log and the table
    // and not into a run of ordered ingest.
    for i in (0..n).rev() {
        db.append(&keys[i], &[b'v'; 100]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    (db, keys)
}

/// A settle's patch of a copy appends a key above the copy's last entry on
/// one compare with that entry (`Options::settle_append_last`), where it
/// searched the copy's keys for the place: every key inserted past a
/// store's end lands so, in the copy its last block is held as. A key
/// below the last entry is searched for as before, and lands in its
/// place.
#[test]
fn a_key_past_a_copys_last_entry_is_appended_without_a_search() {
    for on in [true, false] {
        let dir = scratch(&format!("append-last-{on}"));
        let (mut db, keys) = appended(
            &dir,
            1000,
            false,
            Options {
                settle_append_last: on,
                ..common::options()
            },
        );
        let mut want: std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> = keys
            .iter()
            .map(|k| (k.clone(), vec![vec![b'v'; 100]]))
            .collect();
        let fences = partition_fences(&dir);
        let near = |db: &Db| {
            for (lo, _) in &fences {
                db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
            }
        };
        // The store's last block as a copy: its keys rewritten, twice,
        // each commit near reads.
        for tag in *b"ab" {
            near(&db);
            for k in &keys[keys.len() - 64..] {
                db.put(k, &[tag; 100]);
                want.insert(k.clone(), vec![vec![tag; 100]]);
            }
            db.commit().unwrap();
        }
        assert!(scan_model(&db) == want, "{on}: after the rewrites");
        let (_, _, copies, _) = db.block_cache_kinds();
        assert!(copies >= 1, "{on}: the last block is held as a copy");
        // Keys past the store's end, each above the one before. The last
        // block is a sparse form for the first few and a copy from there:
        // every insert that finds no sparse form left is a copy's.
        let mut into_a_copy = 0;
        for i in 0..16 {
            near(&db);
            let copy = db.block_cache_kinds().1 == 0;
            let before = db.settle_short()[1];
            let k = format!("k{:05}", 1000 + i).into_bytes();
            db.append(&k, &[b'n'; 100]);
            want.insert(k, vec![vec![b'n'; 100]]);
            db.commit().unwrap();
            assert!(
                scan_list(&db) == want.clone().into_iter().collect::<Vec<_>>(),
                "{on}: after insert {i} past the end, every key in its place"
            );
            let appended = db.settle_short()[1] - before;
            if !on {
                assert_eq!(appended, 0, "insert {i} searched for with the option off");
            } else if copy {
                assert_eq!(appended, 1, "insert {i} past a copy's end, on one compare");
            }
            into_a_copy += usize::from(copy);
        }
        assert!(
            into_a_copy >= 8,
            "{on}: the last block was a copy for most of the inserts: {into_a_copy}"
        );
        // A key inside the copy, below its last entry: searched for, and
        // in its place.
        let before = db.settle_short()[1];
        near(&db);
        let inside = b"k00990x".to_vec();
        db.append(&inside, b"inside");
        want.insert(inside, vec![b"inside".to_vec()]);
        db.commit().unwrap();
        assert!(
            scan_list(&db) == want.clone().into_iter().collect::<Vec<_>>(),
            "{on}: a key inside the copy, in its place"
        );
        assert_eq!(
            db.settle_short()[1],
            before,
            "{on}: a key below the last entry is not appended"
        );
        db.flush().unwrap();
        db.settle().unwrap();
        assert!(scan_model(&db) == want, "{on}: and after the flush");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A settle's patch takes a key's run from the key's own chain where no
/// older source can hold the key (`Options::settle_lone_chain`): a plain
/// insert of a key the partition has no record of, with no piece over the
/// partition and no table frozen. A key the partition holds, a key under
/// a piece and a key under a frozen table keep the walk of the sources,
/// which is what finds their older values.
#[test]
fn a_key_only_its_chain_holds_takes_its_run_from_the_chain() {
    for on in [true, false] {
        // No piece and no frozen table.
        let dir = scratch(&format!("lone-chain-{on}"));
        let (mut db, keys) = appended(
            &dir,
            1000,
            false,
            Options {
                settle_lone_chain: on,
                ..common::options()
            },
        );
        let mut want: std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> = keys
            .iter()
            .map(|k| (k.clone(), vec![vec![b'v'; 100]]))
            .collect();
        let fences = partition_fences(&dir);
        let near = |db: &Db| {
            for (lo, _) in &fences {
                db.scan(lo, 1, kv(&mut |_, _| {})).unwrap();
            }
        };
        assert!(scan_model(&db) == want, "{on}: the store as loaded");
        let lone = |db: &Db| db.settle_short()[0];
        // A form for the block the keys below fall in, which a patch
        // needs to have something to patch: one of its keys given a second
        // value. By an append and not a put, so that the store holds no
        // tombstone when the fresh key's run is taken: the chain is walked
        // for the lone key's sake there, and for nothing else.
        near(&db);
        db.append(&keys[505], b"again");
        want.get_mut(&keys[505]).unwrap().push(b"again".to_vec());
        db.commit().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: a key of the block written again"
        );
        assert_eq!(lone(&db), 0, "{on}: a key the partition holds is not lone");
        // A key the store has never held, and a second value for it.
        let fresh = b"k00500x".to_vec();
        for (i, v) in [b"one".to_vec(), b"two".to_vec()].into_iter().enumerate() {
            let before = lone(&db);
            near(&db);
            db.append(&fresh, &v);
            want.entry(fresh.clone()).or_default().push(v);
            db.commit().unwrap();
            assert!(scan_model(&db) == want, "{on}: value {i} of a fresh key");
            let took = lone(&db) - before;
            if on {
                assert_eq!(took, 1, "value {i} of a fresh key came from its chain");
            } else {
                assert_eq!(took, 0, "the sources walked with the option off");
            }
        }
        // A second value for a key the partition holds: the partition's
        // value is the run's first, and the chain alone would lose it.
        let before = lone(&db);
        near(&db);
        db.append(&keys[510], b"more");
        want.get_mut(&keys[510]).unwrap().push(b"more".to_vec());
        db.commit().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: a second value for a sealed key"
        );
        assert_eq!(
            lone(&db),
            before,
            "{on}: a key the partition holds is not lone"
        );
        // A fresh key written by a put: its tombstone masks, as before.
        let before = lone(&db);
        near(&db);
        db.put(b"k00507x", b"put");
        want.insert(b"k00507x".to_vec(), vec![b"put".to_vec()]);
        db.commit().unwrap();
        assert!(scan_model(&db) == want, "{on}: a fresh key put");
        assert_eq!(lone(&db), before, "{on}: a masked key is the masked path's");
        db.flush().unwrap();
        db.settle().unwrap();
        assert!(scan_model(&db) == want, "{on}: and after the flush");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);

        // Under a piece: a key sealed into a level-0 piece and written
        // again holds its first value in the piece.
        let dir = scratch(&format!("lone-chain-piece-{on}"));
        let (mut db, keys) = appended(
            &dir,
            1000,
            false,
            Options {
                settle_lone_chain: on,
                ..common::options()
            },
        );
        let mut want: std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> = keys
            .iter()
            .map(|k| (k.clone(), vec![vec![b'v'; 100]]))
            .collect();
        let under = b"k00700x".to_vec();
        db.append(&under, b"in the piece");
        want.insert(under.clone(), vec![b"in the piece".to_vec()]);
        db.commit().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
        assert!(
            db.segments() > partition_fences(&dir).len(),
            "a piece stands"
        );
        assert!(scan_model(&db) == want, "{on}: with the piece standing");
        db.scan(b"k00690", 20, kv(&mut |_, _| {})).unwrap();
        db.put(&keys[705], b"rewritten");
        want.insert(keys[705].clone(), vec![b"rewritten".to_vec()]);
        db.commit().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: a key of the block rewritten"
        );
        let before = lone(&db);
        db.scan(b"k00690", 20, kv(&mut |_, _| {})).unwrap();
        db.append(&under, b"in the table");
        want.get_mut(&under).unwrap().push(b"in the table".to_vec());
        db.commit().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: a key a piece holds, written again"
        );
        assert_eq!(lone(&db), before, "{on}: a key under a piece is not lone");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);

        // Under a frozen table: a key in a table a held seal keeps frozen,
        // written again into the live one.
        let dir = scratch(&format!("lone-chain-frozen-{on}"));
        let (mut db, keys) = appended(
            &dir,
            1000,
            true,
            Options {
                settle_lone_chain: on,
                ..common::options()
            },
        );
        let mut want: std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> = keys
            .iter()
            .map(|k| (k.clone(), vec![vec![b'v'; 100]]))
            .collect();
        assert!(scan_model(&db) == want, "{on}: the store as loaded");
        let held = b"k00800x".to_vec();
        db.hold_seal_landing(true);
        db.append(&held, b"frozen");
        want.insert(held.clone(), vec![b"frozen".to_vec()]);
        db.commit().unwrap();
        db.seal().unwrap();
        assert_eq!(db.frozen_tables(), 1, "the seal's table is frozen");
        assert!(scan_model(&db) == want, "{on}: with the table frozen");
        db.scan(b"k00790", 20, kv(&mut |_, _| {})).unwrap();
        db.put(&keys[805], b"rewritten");
        want.insert(keys[805].clone(), vec![b"rewritten".to_vec()]);
        db.commit().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: a key of the block rewritten"
        );
        let before = lone(&db);
        db.scan(b"k00790", 20, kv(&mut |_, _| {})).unwrap();
        db.append(&held, b"live");
        want.get_mut(&held).unwrap().push(b"live".to_vec());
        db.commit().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: a key a frozen table holds, written again"
        );
        assert_eq!(
            lone(&db),
            before,
            "{on}: a key under a frozen table is not lone"
        );
        db.hold_seal_landing(false);
        db.settle().unwrap();
        assert!(
            scan_model(&db) == want,
            "{on}: and once the seal has landed"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A store of partitions a block each, every block held as a copy built
/// over updates alone, so each copy's entries are its partition's records
/// (`CachedBlock::identity`): the keys, what a scan answers, and the keys
/// inside the middle partition.
#[allow(clippy::type_complexity)]
fn copies_of_records(
    dir: &std::path::Path,
    o: Options,
) -> (
    Db,
    Vec<Vec<u8>>,
    std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
    Vec<Vec<u8>>,
) {
    let mut db = Db::create(
        dir,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(4096),
            partition_on_flush: true,
            publish_in_background: true,
            upkeep: Upkeep::Background(1),
            upkeep_pace_syncs_bp: 0,
            scan_block_cache: true,
            l0_trigger: 2,
            ..o
        },
    )
    .unwrap();
    let value = [b'v'; 100];
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i:05}").into_bytes()).collect();
    let mut want = std::collections::BTreeMap::new();
    for k in &keys {
        db.put(k, &value);
        want.insert(k.clone(), vec![value.to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let fences = partition_fences(dir);
    assert!(fences.len() >= 4, "partitions: {}", fences.len());
    let (lo, hi) = {
        let (lo, hi) = &fences[fences.len() / 2];
        (lo.clone(), hi.clone().expect("a closed partition"))
    };
    let inside: Vec<Vec<u8>> = keys
        .iter()
        .filter(|k| **k >= lo && **k < hi)
        .cloned()
        .collect();
    assert!(
        (26..=64).contains(&inside.len()),
        "keys inside the range: {}",
        inside.len()
    );
    let _ = scan_model(&db);
    for (i, k) in keys.iter().enumerate() {
        if i % 4 != 0 {
            db.put(k, b"updated");
            want.insert(k.clone(), vec![b"updated".to_vec()]);
        }
    }
    db.commit().unwrap();
    db.settle().unwrap();
    assert_eq!(scan_model(&db), want);
    let (claimed, holds) = db.block_cache_identity();
    assert!(
        claimed >= 4 && holds == claimed,
        "copies of their partitions' records: {claimed} claimed, {holds} of their partitions"
    );
    (db, keys, want, inside)
}

/// The store's merges landed past `before`, and the writer's look across
/// them made.
fn merged_past(db: &mut Db, before: u64) {
    let t = std::time::Instant::now();
    while db.seal_waits().merges == before {
        assert!(t.elapsed().as_secs() < 60, "the merge never landed");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    db.settle().unwrap();
    db.settle().unwrap();
}

/// A copy's claim that its entries are its partition's records is of the
/// partition's keys, and a merge that folds updates alone into a
/// partition writes those keys at the ranks they had: the copy carried
/// across it starts a scan and takes a patch at a rank as before. Held to
/// the file its build read, the claim was lost at every merge of the
/// partition for the copy's life, and every scan's start in the block and
/// every patch of it searched from then on (`Options::identity_by_keys`;
/// off is that shape, and reads the same answers by the search). Under
/// either way a merge finds its keys (`Options::cursor_merge`).
#[test]
fn a_copys_identity_outlives_a_merge_that_kept_its_partitions_keys() {
    for (by_keys, cursors) in [(true, true), (true, false), (false, true)] {
        let dir = scratch(&format!("copy-identity-kept-{by_keys}-{cursors}"));
        let (mut db, keys, mut want, inside) = copies_of_records(
            &dir,
            Options {
                identity_by_keys: by_keys,
                cursor_merge: cursors,
                ..common::options()
            },
        );
        let partitions = partition_fences(&dir).len();
        // Updates alone in every partition, sealed twice: every range is
        // due, and each merge writes its partition's keys and no others.
        let rebased_before = db.forms_rebased();
        let merges_before = db.seal_waits().merges;
        let kept_before = db.seal_waits().keyseqs_kept;
        db.hold_seal_durable(true);
        for round in 0..2u8 {
            let v = [b'a' + round; 100];
            for k in keys.iter().step_by(5) {
                db.put(k, &v);
                want.insert(k.clone(), vec![v.to_vec()]);
            }
            db.commit().unwrap();
            db.seal().unwrap();
        }
        db.hold_seal_durable(false);
        merged_past(&mut db, merges_before);
        // The writer's look across the landings, which carries its tables
        // to the partitions as they stand, is its next read of the log: a
        // scan.
        assert_eq!(scan_model(&db), want, "the writer's scan across the merges");
        assert!(
            db.forms_rebased() > rebased_before,
            "the rewrites kept the partitions' layouts: {} before, {} after",
            rebased_before,
            db.forms_rebased()
        );
        let kept = db.seal_waits().keyseqs_kept - kept_before;
        let (claimed, holds) = db.block_cache_identity();
        if by_keys {
            assert_eq!(
                kept, partitions as u64,
                "every output is of its partition's keys"
            );
            assert!(
                claimed >= 4 && holds == claimed,
                "the copies carried across: {claimed} claimed, {holds} of their partitions"
            );
        } else {
            assert_eq!(kept, 0, "a claim by the file carries no key sequence");
            assert!(
                claimed >= 4 && holds == 0,
                "the copies carried across: {claimed} claimed, {holds} of their partitions"
            );
        }
        // A write after the rewrites, patched into the carried copy, and
        // a scan that starts inside it: at the key's rank where the claim
        // stands, by a search of the copy where it does not, and the same
        // answer either way.
        let patches_before = db.copy_patches();
        let asks_before = db.patch_asks();
        for k in [&inside[7], &inside[9]] {
            db.put(k, b"after");
            want.insert(k.clone(), vec![b"after".to_vec()]);
        }
        db.commit().unwrap();
        db.settle().unwrap();
        let before = db.copy_starts();
        let mut got = Vec::new();
        db.scan(
            &inside[4],
            5,
            kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))),
        )
        .unwrap();
        let after = db.copy_starts();
        let patches = db.copy_patches();
        let from_model: Vec<(Vec<u8>, Vec<u8>)> = want
            .range(inside[4].clone()..)
            .take(5)
            .map(|(k, v)| (k.clone(), v[0].clone()))
            .collect();
        assert_eq!(got, from_model, "the scan from inside the copy");
        assert_eq!(
            [after[0] - before[0], after[1] - before[1]],
            if by_keys { [1, 0] } else { [0, 1] },
            "the scan's start in the copy, at its rank and by a search"
        );
        assert_eq!(
            [
                patches[0] - patches_before[0],
                patches[1] - patches_before[1]
            ],
            if by_keys { [2, 0] } else { [0, 2] },
            "the patches of the copy, at their ranks and by a search"
        );
        // And the lines a patch by its rank asks for ahead of it, the
        // second key's while the first is patched.
        assert_eq!(
            db.patch_asks() - asks_before,
            u64::from(by_keys),
            "the asks ahead of a patch by its rank"
        );
        let listed: Vec<(Vec<u8>, Vec<Vec<u8>>)> =
            want.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(scan_list(&db), listed, "the writer's scan");
        let r = db.reader().unwrap();
        let mut via_handle = std::collections::BTreeMap::new();
        r.scan(
            b"",
            usize::MAX,
            kv(&mut |k, v| {
                via_handle
                    .entry(k.to_vec())
                    .or_insert_with(Vec::new)
                    .push(v.to_vec());
            }),
        )
        .unwrap();
        assert_eq!(via_handle, want, "the handle's scan");
        drop(r);
        // The tables made again over the partitions as the merges left
        // them, and their copies built again: a table and the copies it
        // holds name one key sequence, the partition's, inherited or not.
        db.drop_forms();
        let _ = scan_model(&db);
        for (i, k) in keys.iter().enumerate() {
            if i % 4 != 0 {
                db.put(k, b"again");
                want.insert(k.clone(), vec![b"again".to_vec()]);
            }
        }
        db.commit().unwrap();
        db.settle().unwrap();
        assert_eq!(
            scan_model(&db),
            want,
            "the writer's scan over the forms made again"
        );
        let (claimed, holds) = db.block_cache_identity();
        assert!(
            claimed >= 4 && holds == claimed,
            "the copies built again: {claimed} claimed, {holds} of their partitions"
        );
        let before = db.copy_starts();
        let mut n = 0;
        db.scan(&inside[4], 5, kv(&mut |_, _| n += 1)).unwrap();
        let after = db.copy_starts();
        assert_eq!(n, 5);
        assert_eq!(
            [after[0] - before[0], after[1] - before[1]],
            [1, 0],
            "the scan's start in a copy built over the partition as it stands"
        );
        for (k, v) in &want {
            let mut got = Vec::new();
            db.read_all(k, vf(&mut |x| got.push(x.to_vec()))).unwrap();
            assert_eq!(
                &got,
                v,
                "the point read of {:?}",
                String::from_utf8_lossy(k)
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// An output a key short of the partition it rewrote holds only keys that
/// partition held, and is not its key sequence: every rank past the key
/// the merge dropped is another key's. The count is what says so, where
/// the keys folded in say nothing; and the partition's is the count that
/// is asked, not that of a piece over its range, which here holds as many
/// keys as the output does.
#[test]
fn a_merge_that_drops_a_key_writes_a_key_sequence_of_its_own() {
    let dir = scratch("copy-identity-dropped-key");
    let (mut db, _, mut want, inside) = copies_of_records(&dir, common::options());
    let merges_before = db.seal_waits().merges;
    let kept_before = db.seal_waits().keyseqs_kept;
    db.hold_seal_durable(true);
    // The first piece drops the key; the second holds every other key of
    // the range, as many records as the output will.
    db.delete(&inside[2]);
    db.put(&inside[3], &[b'a'; 100]);
    want.remove(&inside[2]);
    want.insert(inside[3].clone(), vec![vec![b'a'; 100]]);
    db.commit().unwrap();
    db.seal().unwrap();
    for (i, k) in inside.iter().enumerate() {
        if i != 2 {
            db.put(k, &[b'b'; 100]);
            want.insert(k.clone(), vec![vec![b'b'; 100]]);
        }
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.hold_seal_durable(false);
    merged_past(&mut db, merges_before);
    assert_eq!(
        db.seal_waits().keyseqs_kept,
        kept_before,
        "an output a key short took a key sequence that is not its own"
    );
    db.put(&inside[7], b"after");
    want.insert(inside[7].clone(), vec![b"after".to_vec()]);
    db.commit().unwrap();
    db.settle().unwrap();
    let listed: Vec<(Vec<u8>, Vec<Vec<u8>>)> =
        want.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(scan_list(&db), listed, "the writer's scan");
    let mut got = Vec::new();
    db.scan(
        &inside[4],
        5,
        kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))),
    )
    .unwrap();
    let from_model: Vec<(Vec<u8>, Vec<u8>)> = want
        .range(inside[4].clone()..)
        .take(5)
        .map(|(k, v)| (k.clone(), v[0].clone()))
        .collect();
    assert_eq!(got, from_model, "the scan from inside the block");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key no partition held is counted against the output it is written
/// to and no other: one merge over every range, with a key dropped and a
/// key folded in -- as many keys as before -- in the first partition and
/// in the last, the last's folded key the store's greatest, the last the
/// merge writes. Those two outputs are key sequences of their own and
/// every output between them is its partition's. Under either way a merge
/// finds its keys (`Options::cursor_merge`).
#[test]
fn a_key_folded_in_is_its_own_outputs_and_no_other_outputs() {
    for cursors in [true, false] {
        let dir = scratch(&format!("copy-identity-folded-{cursors}"));
        let (mut db, keys, mut want, _) = copies_of_records(
            &dir,
            Options {
                cursor_merge: cursors,
                ..common::options()
            },
        );
        let partitions = partition_fences(&dir).len() as u64;
        let merges_before = db.seal_waits().merges;
        let kept_before = db.seal_waits().keyseqs_kept;
        let folded_low = {
            let mut k = keys[1].clone();
            k.push(b'x');
            k
        };
        let folded_high = {
            let mut k = keys[keys.len() - 1].clone();
            k.push(b'x');
            k
        };
        let (dropped_low, dropped_high) = (keys[3].clone(), keys[keys.len() - 4].clone());
        db.hold_seal_durable(true);
        for round in 0..2u8 {
            let v = [b'a' + round; 100];
            for k in keys.iter().step_by(5) {
                db.put(k, &v);
                want.insert(k.clone(), vec![v.to_vec()]);
            }
            db.delete(&dropped_low);
            db.delete(&dropped_high);
            db.put(&folded_low, b"folded");
            db.put(&folded_high, b"folded");
            want.remove(&dropped_low);
            want.remove(&dropped_high);
            want.insert(folded_low.clone(), vec![b"folded".to_vec()]);
            want.insert(folded_high.clone(), vec![b"folded".to_vec()]);
            db.commit().unwrap();
            db.seal().unwrap();
        }
        db.hold_seal_durable(false);
        merged_past(&mut db, merges_before);
        assert_eq!(
            db.seal_waits().merges,
            merges_before + 1,
            "one merge over every range"
        );
        assert_eq!(
            db.seal_waits().keyseqs_kept - kept_before,
            partitions - 2,
            "the outputs of their partitions' keys, of {partitions}"
        );
        let listed: Vec<(Vec<u8>, Vec<Vec<u8>>)> =
            want.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(scan_list(&db), listed, "the writer's scan across the merge");
        // Patches into the first and the last blocks, between the key each
        // lost and the key each gained, where every rank moved.
        db.put(&keys[2], b"after");
        db.put(&keys[keys.len() - 2], b"after");
        want.insert(keys[2].clone(), vec![b"after".to_vec()]);
        want.insert(keys[keys.len() - 2].clone(), vec![b"after".to_vec()]);
        db.commit().unwrap();
        db.settle().unwrap();
        let listed: Vec<(Vec<u8>, Vec<Vec<u8>>)> =
            want.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(
            scan_list(&db),
            listed,
            "the writer's scan after the patches"
        );
        for (k, v) in &want {
            let mut got = Vec::new();
            db.read_all(k, vf(&mut |x| got.push(x.to_vec()))).unwrap();
            assert_eq!(
                &got,
                v,
                "the point read of {:?}",
                String::from_utf8_lossy(k)
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
