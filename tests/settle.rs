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
use common::partition_fences;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-settle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A scan through the writer, as the model it answers.
fn scan_model(db: &Db) -> std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut m = std::collections::BTreeMap::new();
    db.scan(b"", usize::MAX, |k, v| {
        m.entry(k.to_vec())
            .or_insert_with(Vec::new)
            .push(v.to_vec());
    })
    .unwrap();
    m
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
    db.read_all(b"k00150x", |_| n += 1).unwrap();
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
    db.scan(&inside[0], 2, |k, _| from_inside.push(k.to_vec()))
        .unwrap();
    assert_eq!(from_inside, vec![inside[0].clone(), fresh.clone()]);
    let below: Vec<&Vec<u8>> = keys.iter().filter(|k| **k < lo).collect();
    let mut across: Vec<Vec<u8>> = Vec::new();
    db.scan(below[below.len() - 1], 3, |k, _| across.push(k.to_vec()))
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
    r.scan(b"", usize::MAX, |k, v| {
        via_handle
            .entry(k.to_vec())
            .or_insert_with(Vec::new)
            .push(v.to_vec());
    })
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
    db.read_all(&fresh, |_| n += 1).unwrap();
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
        db.scan(b"", usize::MAX, |k, v| {
            if order.last().map(|l| l.as_slice()) != Some(k) {
                order.push(k.to_vec());
            }
            m.entry(k.to_vec())
                .or_insert_with(Vec::new)
                .push(v.to_vec());
        })
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
    r.scan(b"", usize::MAX, |k, v| {
        via_handle
            .entry(k.to_vec())
            .or_insert_with(Vec::new)
            .push(v.to_vec());
    })
    .unwrap();
    assert_eq!(via_handle, got, "the handle's scan");
    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
