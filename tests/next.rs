//! Milestone-1 contract of the next engine: multivalue order across seals, a
//! model-checked read path, and the crash windows the module doc enumerates.
//! Every crash here is emulated the way `tests/known_bugs.rs` emulates them:
//! by constructing the exact on-disk state the window leaves behind, because
//! a clean result on a path a test never took proves nothing.

use std::collections::HashMap;
use std::path::PathBuf;

use supdb::next::{Db, NextOptions};

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

#[test]
fn values_come_back_in_append_order_across_seals() {
    let d = dir("order");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for round in 0u32..3 {
        for k in 0u32..50 {
            let key = format!("key-{k:04}").into_bytes();
            let val = format!("v{round}-{k}").into_bytes();
            db.append(&key, &val);
            model.entry(key).or_default().push(val);
        }
        db.commit().unwrap();
        db.seal().unwrap();
    }
    assert_eq!(db.segments(), 3);
    for (key, want) in &model {
        assert_eq!(&read_vec(&db, key), want, "key {}", String::from_utf8_lossy(key));
    }
    db.close().unwrap();
}

#[test]
fn reads_see_uncommitted_and_unsealed_state() {
    let d = dir("ryw");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"k", b"one");
    assert_eq!(read_vec(&db, b"k"), vec![b"one".to_vec()], "pre-commit read");
    db.commit().unwrap();
    db.append(b"k", b"two");
    assert_eq!(read_vec(&db, b"k"), vec![b"one".to_vec(), b"two".to_vec()]);
}

#[test]
fn killed_before_first_seal_opens_from_the_wal_alone() {
    // P-E, the flip of C3.4: no segment exists, only a WAL.
    let d = dir("preseal");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    for i in 0u32..500 {
        db.append(format!("k{i}").as_bytes(), &i.to_le_bytes());
    }
    db.commit().unwrap();
    drop(db); // no close, no seal: the crash

    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(db.segments(), 0, "nothing was sealed");
    for i in 0u32..500 {
        assert_eq!(read_vec(&db, format!("k{i}").as_bytes()), vec![i.to_le_bytes().to_vec()]);
    }
}

#[test]
fn uncommitted_tail_is_lost_whole_and_committed_state_survives() {
    let d = dir("tail");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"durable", b"yes");
    db.commit().unwrap();
    db.append(b"volatile", b"never-synced");
    drop(db); // pending buffer never reached the file

    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(read_vec(&db, b"durable"), vec![b"yes".to_vec()]);
    assert_eq!(read_vec(&db, b"volatile"), Vec::<Vec<u8>>::new());
}

#[test]
fn a_torn_tail_loses_its_batch_whole_and_earlier_batches_survive() {
    let d = dir("torn");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"a", b"1");
    db.commit().unwrap();
    db.append(b"b", b"2");
    db.append(b"c", b"3");
    db.commit().unwrap();
    drop(db);

    // Tear the tail: chop bytes off the WAL, the state a crash mid-write
    // leaves. The cut lands in the second batch's commit frame, so `b` is an
    // intact frame -- and it must NOT be served, because its batch never
    // committed. Before the commit frame existed this test expected the
    // intact frame back; that was a partial batch replayed as whole.
    let wal = d.join("wal-00000000");
    let len = std::fs::metadata(&wal).unwrap().len();
    let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
    f.set_len(len - 3).unwrap();
    drop(f);

    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(read_vec(&db, b"a"), vec![b"1".to_vec()], "a committed batch survives");
    assert_eq!(read_vec(&db, b"b"), Vec::<Vec<u8>>::new(), "the torn batch is gone whole");
    assert_eq!(read_vec(&db, b"c"), Vec::<Vec<u8>>::new(), "the torn batch is gone whole");
}

#[test]
fn a_transaction_is_all_or_nothing_and_sees_its_own_writes() {
    let d = dir("txn");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"z", b"old");
    db.commit().unwrap();
    {
        let mut tx = db.begin();
        tx.append(b"x", b"1");
        tx.append(b"x", b"2");
        tx.delete(b"z");
        tx.append(b"z", b"new");
        let mut got = Vec::new();
        tx.read_all(b"x", |v| got.push(v.to_vec())).unwrap();
        assert_eq!(got, vec![b"1".to_vec(), b"2".to_vec()], "read-your-writes inside");
        let mut got = Vec::new();
        tx.read_all(b"z", |v| got.push(v.to_vec())).unwrap();
        assert_eq!(got, vec![b"new".to_vec()], "a staged delete masks the store's values");
        assert_eq!(tx.count(b"z").unwrap(), 1);
        assert_eq!(tx.count(b"x").unwrap(), 2);
        tx.abort();
    }
    assert!(read_vec(&db, b"x").is_empty(), "an aborted transaction leaves nothing");
    assert_eq!(read_vec(&db, b"z"), vec![b"old".to_vec()]);
    {
        let mut tx = db.begin();
        tx.append(b"x", b"1");
        tx.append(b"x", b"2");
        tx.delete(b"z");
        tx.append(b"z", b"new");
        tx.commit().unwrap();
    }
    assert_eq!(read_vec(&db, b"x"), vec![b"1".to_vec(), b"2".to_vec()]);
    assert_eq!(read_vec(&db, b"z"), vec![b"new".to_vec()]);
    {
        let mut tx = db.begin();
        tx.append(b"y", b"gone");
        drop(tx);
    }
    assert!(read_vec(&db, b"y").is_empty(), "dropped without commit is abort");
    drop(db);
    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(read_vec(&db, b"x"), vec![b"1".to_vec(), b"2".to_vec()], "committed, durably");
    assert_eq!(read_vec(&db, b"z"), vec![b"new".to_vec()]);
    assert!(read_vec(&db, b"y").is_empty());
}

#[test]
fn crash_between_rename_and_wal_reset_does_not_duplicate() {
    // The window the segment file name exists for: the seal renamed its
    // segment into place and synced the directory, then the process died
    // before the WAL reset. The WAL still holds every sealed record.
    let d = dir("renamewin");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    for i in 0u32..40 {
        db.append(b"dup-window", &i.to_le_bytes());
    }
    db.commit().unwrap();

    // Emulate: copy the WAL aside, seal (which resets it), then put the
    // pre-seal WAL back. Disk state is now exactly rename-done, reset-lost.
    let wal = d.join("wal-00000000");
    let saved = std::fs::read(&wal).unwrap();
    db.seal().unwrap();
    drop(db);
    std::fs::write(&wal, &saved).unwrap();

    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(db.segments(), 1);
    let got = read_vec(&db, b"dup-window");
    assert_eq!(got.len(), 40, "sealed records must not replay into duplicates");
    for (i, v) in got.iter().enumerate() {
        assert_eq!(v, &(i as u32).to_le_bytes().to_vec());
    }
}

#[test]
fn reopen_after_seal_serves_both_old_and_new_writes() {
    let d = dir("reopen");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"k", b"sealed");
    db.commit().unwrap();
    db.seal().unwrap();
    db.append(b"k", b"walled");
    db.commit().unwrap();
    drop(db); // crash with one segment and one live WAL record

    let mut db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"sealed".to_vec(), b"walled".to_vec()]);
    db.append(b"k", b"after");
    db.commit().unwrap();
    db.seal().unwrap();
    drop(db);

    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"sealed".to_vec(), b"walled".to_vec(), b"after".to_vec()],
        "order survives a second generation of seals"
    );
}

fn oracle(bulk: bool, cursors: bool) {
    // The c2-oracle discipline at milestone-1 scale: random appends,
    // commits, seals, and crash-reopens, checked against a HashMap after
    // every reopen. Uncommitted writes are trimmed from the model at a
    // crash, which is the durability contract.
    let d = dir(&format!("oracle-{}-{}", if bulk { "bulk" } else { "general" }, if cursors { "cursors" } else { "probes" }));
    let opts = NextOptions { bulk_writer: bulk, cursor_merge: cursors, ..NextOptions::default() };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    let mut uncommitted: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
    let mut state = 0x5eedu64;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for step in 0..2_000u32 {
        let key = format!("k{}", rng() % 97).into_bytes();
        // One op in twenty is a delete, on the bulk writer only: the general
        // writer is a measurement arm and cannot express one.
        if bulk && rng() % 20 == 0 {
            db.delete(&key);
            uncommitted.push((key, None));
        } else {
            let val = format!("v{step}").into_bytes();
            db.append(&key, &val);
            uncommitted.push((key, Some(val)));
        }
        match rng() % 100 {
            0..=9 => {
                db.commit().unwrap();
                apply(&mut model, &mut uncommitted);
            }
            10..=12 => {
                db.commit().unwrap();
                apply(&mut model, &mut uncommitted);
                db.seal().unwrap();
            }
            13 => {
                drop(db); // crash: uncommitted appends vanish
                uncommitted.clear();
                db = match Db::open(&d, opts.clone()) {
                    Ok(db) => db,
                    Err(e) => {
                        let mut files: Vec<String> = std::fs::read_dir(&d)
                            .unwrap()
                            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                            .collect();
                        files.sort();
                        panic!("open failed at step {step}: {e}; dir = {files:?}");
                    }
                };
                for (k, want) in &model {
                    assert_eq!(&read_vec(&db, k), want, "after crash at step {step}");
                }
            }
            _ => {}
        }
    }
    db.commit().unwrap();
    apply(&mut model, &mut uncommitted);
    for (k, want) in &model {
        assert_eq!(&read_vec(&db, k), want);
    }
    // The scan agrees with the point reads: every live key, every live
    // value, and no key whose values were all deleted.
    let mut scanned: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    db.scan(b"", usize::MAX, |k, v| scanned.entry(k.to_vec()).or_default().push(v.to_vec()))
        .unwrap();
    let live: HashMap<Vec<u8>, Vec<Vec<u8>>> =
        model.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(scanned, live, "the scan must agree with the model");
}

/// Apply a committed batch to the model: an append pushes, a delete clears.
fn apply(model: &mut HashMap<Vec<u8>, Vec<Vec<u8>>>, batch: &mut Vec<(Vec<u8>, Option<Vec<u8>>)>) {
    for (k, v) in batch.drain(..) {
        let e = model.entry(k).or_default();
        match v {
            Some(v) => e.push(v),
            None => e.clear(),
        }
    }
}

#[test]
fn model_oracle_over_random_ops_and_crashes() {
    oracle(true, true)
}

/// The general `Store` writer stays behind `bulk_writer` and the probe merge
/// behind `cursor_merge`, as f49's comparison arms -- and a path only one arm
/// exercises is a path nothing tests.
#[test]
fn the_general_writer_arm_passes_the_same_oracle() {
    oracle(false, true)
}

#[test]
fn the_probe_merge_arm_passes_the_same_oracle() {
    oracle(true, false)
}

fn small_opts(l0_trigger: usize) -> NextOptions {
    // Small enough that a few hundred records seal and compact, so the
    // level machinery is exercised at test scale rather than described.
    // Partitions follow the seal here (`partition_bytes: None`): these tests
    // want many small partitions, where the shipping default holds them at
    // 64 MB whatever the seal size (f52).
    NextOptions { seal_bytes: 4 << 10, l0_trigger, partition_bytes: None, ..NextOptions::default() }
}

#[test]
fn compaction_partitions_the_key_space_and_keeps_every_value() {
    let d = dir("compact");
    let mut db = Db::create(&d, small_opts(3)).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for round in 0u32..12 {
        for k in 0u32..200 {
            let key = format!("key-{k:05}").into_bytes();
            let val = format!("r{round}-{k}").into_bytes();
            db.append(&key, &val);
            model.entry(key).or_default().push(val);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    let (partitioned, l0) = db.levels();
    assert!(partitioned > 1, "the merge should have split the key space, got {partitioned}");
    assert!(l0 <= 3, "the tail stays bounded by l0_trigger, got {l0}");
    for (key, want) in &model {
        assert_eq!(&read_vec(&db, key), want, "key {}", String::from_utf8_lossy(key));
    }
    // Ordered scan over a compacted store: keys ascending, no duplicates.
    let mut seen: Vec<Vec<u8>> = Vec::new();
    db.scan(b"", 1000, |k, _| {
        if seen.last().map(|l| l.as_slice()) != Some(k) {
            seen.push(k.to_vec());
        }
    })
    .unwrap();
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(seen, sorted, "scan must be ordered and duplicate-free");
    assert_eq!(seen.len(), 200);
}

#[test]
fn a_compacted_store_reopens_with_the_same_answers() {
    let d = dir("compactreopen");
    let mut db = Db::create(&d, small_opts(2)).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for round in 0u32..10 {
        for k in 0u32..150 {
            let key = format!("k{k:04}").into_bytes();
            let val = format!("v{round}-{k}").into_bytes();
            db.append(&key, &val);
            model.entry(key).or_default().push(val);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    drop(db);

    let db = Db::open(&d, small_opts(2)).unwrap();
    for (key, want) in &model {
        assert_eq!(&read_vec(&db, key), want, "after reopen: {}", String::from_utf8_lossy(key));
    }
}

#[test]
fn a_crash_before_the_manifest_lands_keeps_the_pre_merge_store() {
    // The window the manifest exists for: a merge wrote its outputs and
    // renamed them into place, then the process died before the manifest
    // named them. Those files are unreachable and open must sweep them
    // rather than read them alongside the inputs they duplicate.
    //
    // Staged by building the post-merge files in a COPY and moving them
    // into the pre-merge store, because a merge legitimately deletes its
    // inputs -- an earlier version of this test restored an old manifest
    // over a merged store and was really testing whether open survives a
    // manifest naming deleted files, which is a different question (it
    // now answers it with a diagnosis rather than an ENOENT).
    let d = dir("mergewin");
    let mut db = Db::create(&d, small_opts(2)).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for round in 0u32..6 {
        for k in 0u32..100 {
            let key = format!("k{k:04}").into_bytes();
            let val = format!("v{round}-{k}").into_bytes();
            db.append(&key, &val);
            model.entry(key).or_default().push(val);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    drop(db);

    let pre: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup"))
        .collect();

    // A copy of the store carries on and merges; its outputs are the
    // files a crash would have left behind unnamed.
    let d2 = dir("mergewin2");
    for name in std::fs::read_dir(&d).unwrap() {
        let name = name.unwrap().file_name();
        std::fs::copy(d.join(&name), d2.join(&name)).unwrap();
    }
    let mut db2 = Db::open(&d2, small_opts(2)).unwrap();
    for k in 0u32..100 {
        db2.append(format!("k{k:04}").as_bytes(), b"extra");
    }
    db2.commit().unwrap();
    db2.flush().unwrap();
    drop(db2);
    let post: Vec<String> = std::fs::read_dir(&d2)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup") && !pre.contains(&n.to_string()))
        .collect();
    assert!(!post.is_empty(), "the copy produced no new segments; the window was not staged");
    for name in &post {
        std::fs::copy(d2.join(name), d.join(name)).unwrap();
    }

    let db = Db::open(&d, small_opts(2)).unwrap();
    let after: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    assert_eq!(
        after.len(),
        pre.len(),
        "every unnamed segment must be swept: kept {after:?} against a manifest naming {pre:?}"
    );
    for (key, want) in &model {
        assert_eq!(
            &read_vec(&db, key),
            want,
            "the manifest's store, with no trace of the unnamed merge: {}",
            String::from_utf8_lossy(key)
        );
    }
}

#[test]
fn every_key_survives_partitioning_and_range_merges_at_scale() {
    // The contract tests above use stores too small to make a partition
    // boundary interesting. This one loads enough to force the initial
    // partitioning AND several per-range merges, then demands every key
    // back. A fence that does not tile the key space loses keys silently
    // here and nowhere smaller.
    let d = dir("scale");
    let mut db = Db::create(
        &d,
        NextOptions {
            seal_bytes: 512 << 10,
            l0_trigger: 3,
            partition_bytes: None,
            ..NextOptions::default()
        },
    )
    .unwrap();
    let n = 60_000u32;
    let filler = vec![b'x'; 100];
    for i in 0..n {
        let key = format!("k{i:08}").into_bytes();
        let mut val = i.to_le_bytes().to_vec();
        val.extend_from_slice(&filler);
        db.append(&key, &val);
        if i % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    let (par, l0) = db.levels();
    assert!(par > 1, "expected several partitions, got {par} with {l0} in the tail");

    let mut missing = Vec::new();
    for i in 0..n {
        let key = format!("k{i:08}").into_bytes();
        let mut want = i.to_le_bytes().to_vec();
        want.extend_from_slice(&filler);
        let got = read_vec(&db, &key);
        if got != vec![want] {
            missing.push((i, got.len()));
        }
    }
    assert!(
        missing.is_empty(),
        "{} keys wrong, first ten {:?} ({par} partitions, {l0} tail)",
        missing.len(),
        &missing[..missing.len().min(10)]
    );
}

#[test]
fn count_is_exact_for_variable_width_values_through_seals_and_merges() {
    // The case `count_fixed` cannot serve: every key holds a different
    // number of values and every value is a different length, so the only
    // other way to answer is to walk the length prefixes.
    let d = dir("counts");
    let mut db = Db::create(&d, small_opts(2)).unwrap();
    let mut model: HashMap<Vec<u8>, u64> = HashMap::new();
    for round in 0u32..8 {
        for k in 0u32..120 {
            let key = format!("k{k:04}").into_bytes();
            // A varying number of values per key per round, each of a
            // varying length.
            for j in 0..(k % 5) + 1 {
                let val = vec![b'v'; ((k + j + round) % 37 + 1) as usize];
                db.append(&key, &val);
                *model.entry(key.clone()).or_default() += 1;
            }
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    let (par, l0) = db.levels();
    assert!(par + l0 > 1, "expected several segments, got {par}+{l0}");

    for (key, want) in &model {
        assert_eq!(db.count(key).unwrap(), *want, "key {}", String::from_utf8_lossy(key));
        // And it agrees with actually reading them, which is the only
        // definition of correct that matters.
        assert_eq!(read_vec(&db, key).len() as u64, *want);
    }
    drop(db);

    let db = Db::open(&d, small_opts(2)).unwrap();
    for (key, want) in &model {
        assert_eq!(db.count(key).unwrap(), *want, "after reopen: {}", String::from_utf8_lossy(key));
    }
}

#[test]
fn every_n_loses_the_unsynced_tail_whole_and_never_in_part() {
    // SyncPolicy::EveryN's contract: the WAL is written every commit and
    // synced every nth, so a crash loses at most n batches -- and what it
    // loses it loses WHOLE. Emulated the only honest way in one process:
    // tear the file inside the unsynced tail, since a same-process reopen
    // would otherwise find the page cache still holding what the device
    // never got.
    use supdb::next::SyncPolicy;
    let d = dir("everyn");
    let opts = NextOptions { sync: SyncPolicy::EveryN(16), ..NextOptions::default() };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // 16 commits reach a barrier; the next 7 do not.
    for c in 0u32..23 {
        db.append(format!("k{c:03}").as_bytes(), &c.to_le_bytes());
        db.commit().unwrap();
    }
    drop(db);
    let wal = d.join("wal-00000000");
    let len = std::fs::metadata(&wal).unwrap().len();
    // Tear a few bytes off the end: the last unsynced frame is torn, the
    // ones before it are intact-but-unsynced, and the sixteen before those
    // were behind a barrier.
    std::fs::OpenOptions::new().write(true).open(&wal).unwrap().set_len(len - 5).unwrap();

    let db = Db::open(&d, opts).unwrap();
    for c in 0u32..16 {
        assert_eq!(
            read_vec(&db, format!("k{c:03}").as_bytes()),
            vec![c.to_le_bytes().to_vec()],
            "a synced record must survive"
        );
    }
    // Everything after the tear is gone; everything intact before it is
    // served (this emulation tore only the last frame), and no record is
    // ever duplicated or served out of order.
    assert_eq!(read_vec(&db, b"k022"), Vec::<Vec<u8>>::new(), "the torn frame is the crash point");
    for c in 16u32..22 {
        assert_eq!(read_vec(&db, format!("k{c:03}").as_bytes()).len(), 1);
    }
}

fn dir_bytes(d: &std::path::Path) -> u64 {
    std::fs::read_dir(d).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum()
}

#[test]
fn a_delete_ends_older_values_and_later_appends_start_fresh() {
    let d = dir("delete");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"k", b"v1");
    db.append(b"k", b"v2");
    db.commit().unwrap();
    db.seal().unwrap();
    db.append(b"k", b"v3");
    db.commit().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v1".to_vec(), b"v2".to_vec(), b"v3".to_vec()]);
    db.delete(b"k");
    assert_eq!(read_vec(&db, b"k"), Vec::<Vec<u8>>::new(), "a delete ends everything before it, sealed or not");
    assert_eq!(db.count(b"k").unwrap(), 0);
    db.append(b"k", b"v4");
    db.commit().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()]);
    assert_eq!(db.count(b"k").unwrap(), 1);
    db.seal().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()], "through a sealed tombstone");
    drop(db);
    let mut db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()], "after reopen");
    db.flush().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()], "after the merge");
    assert_eq!(db.count(b"k").unwrap(), 1);
    let mut scanned = Vec::new();
    db.scan(b"", usize::MAX, |k, v| scanned.push((k.to_vec(), v.to_vec()))).unwrap();
    assert_eq!(scanned, vec![(b"k".to_vec(), b"v4".to_vec())]);
    // A delete of a key never written is a tombstone too; it masks nothing.
    db.delete(b"never");
    db.commit().unwrap();
    assert_eq!(read_vec(&db, b"never"), Vec::<Vec<u8>>::new());
    // A delete with no later append: empty through seal and merge, and the
    // key leaves the scan once the merge has dropped it.
    db.delete(b"k");
    db.commit().unwrap();
    db.flush().unwrap();
    assert_eq!(read_vec(&db, b"k"), Vec::<Vec<u8>>::new());
    assert_eq!(db.count(b"k").unwrap(), 0);
    let mut scanned = Vec::new();
    db.scan(b"", usize::MAX, |k, v| scanned.push((k.to_vec(), v.to_vec()))).unwrap();
    assert!(scanned.is_empty(), "a merged-away key does not scan: {scanned:?}");
}

#[test]
fn deleted_values_do_not_survive_the_merge() {
    let d = dir("delete-merge");
    let opts = NextOptions { seal_bytes: 256 << 10, l0_trigger: 3, ..NextOptions::default() };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let filler = vec![b'x'; 200];
    for i in 0..4_000u32 {
        let k = format!("key-{i:06}");
        db.append(k.as_bytes(), &filler);
        db.append(k.as_bytes(), &filler);
        if i % 100 == 99 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    let before = dir_bytes(&d);
    for i in (0..4_000u32).step_by(2) {
        db.delete(format!("key-{i:06}").as_bytes());
        if i % 200 == 198 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    let after = dir_bytes(&d);
    assert!(
        (after as f64) <= (before as f64) * 0.7,
        "the merge must reclaim the deleted half: {before} -> {after} bytes"
    );
    for i in 0..4_000u32 {
        let k = format!("key-{i:06}");
        let got = read_vec(&db, k.as_bytes());
        if i % 2 == 0 {
            assert!(got.is_empty(), "{k} was deleted");
            assert_eq!(db.count(k.as_bytes()).unwrap(), 0);
        } else {
            assert_eq!(got.len(), 2, "{k} must keep both values");
            assert_eq!(db.count(k.as_bytes()).unwrap(), 2);
        }
    }
    let mut keys: Vec<Vec<u8>> = Vec::new();
    db.scan(b"", usize::MAX, |k, _| {
        if keys.last().map(|l| l.as_slice()) != Some(k) {
            keys.push(k.to_vec());
        }
    })
    .unwrap();
    assert_eq!(keys.len(), 2_000, "only the live half scans");
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    assert!(read_vec(&db, b"key-000000").is_empty());
    assert_eq!(read_vec(&db, b"key-000001").len(), 2);
}

#[test]
fn a_batch_without_its_commit_frame_is_lost_whole() {
    // A batch is the frames between commit frames. If the crash lands
    // anywhere inside the second batch -- inside its commit frame, exactly
    // at it, or inside its last record -- the whole batch is gone, and it
    // stays gone after the next commit rather than being adopted by it.
    let d = dir("torn-batch");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    for i in 0..3u32 {
        db.append(format!("a{i}").as_bytes(), b"A");
    }
    db.commit().unwrap();
    for i in 0..3u32 {
        db.append(format!("b{i}").as_bytes(), b"B");
    }
    db.commit().unwrap();
    drop(db);
    let wal = d.join("wal-00000000");
    let full = std::fs::read(&wal).unwrap();
    for cut in [3usize, 17, 17 + 12] {
        let dd = dir(&format!("torn-batch-{cut}"));
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_name() != "wal-00000000" {
                std::fs::copy(e.path(), dd.join(e.file_name())).unwrap();
            }
        }
        std::fs::write(dd.join("wal-00000000"), &full[..full.len() - cut]).unwrap();
        let mut db = Db::open(&dd, NextOptions::default()).unwrap();
        for i in 0..3u32 {
            assert_eq!(
                read_vec(&db, format!("a{i}").as_bytes()),
                vec![b"A".to_vec()],
                "cut {cut}: the committed batch survives"
            );
            assert!(
                read_vec(&db, format!("b{i}").as_bytes()).is_empty(),
                "cut {cut}: a batch without its commit frame is gone whole"
            );
        }
        db.append(b"c0", b"C");
        db.commit().unwrap();
        drop(db);
        let db = Db::open(&dd, NextOptions::default()).unwrap();
        assert_eq!(read_vec(&db, b"c0"), vec![b"C".to_vec()]);
        for i in 0..3u32 {
            assert!(
                read_vec(&db, format!("b{i}").as_bytes()).is_empty(),
                "cut {cut}: still gone after another commit"
            );
        }
    }
}

#[test]
fn idle_io_priority_and_sync_spreading_change_nothing_observable() {
    // f51's two knobs move where the seal's and merge's bytes go and when;
    // neither may change what a reader sees, through seals, merges and a
    // reopen. The idle class may be ignored by the host's scheduler and
    // the syscall may fail -- both are silent by design, and the store
    // must be identical either way.
    use supdb::next::BackgroundIo;
    let d = dir("ioprio");
    let opts = NextOptions {
        seal_bytes: 256 << 10,
        l0_trigger: 3,
        background_io: BackgroundIo::Idle,
        seal_sync_every: 64 << 10,
        partition_bytes: Some(512 << 10),
        ..NextOptions::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let filler = vec![b'y'; 120];
    for i in 0..20_000u32 {
        db.append(format!("k{i:06}").as_bytes(), &filler);
        db.append(format!("k{i:06}").as_bytes(), &i.to_le_bytes());
        if i % 250 == 249 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    for i in (0..20_000u32).step_by(997) {
        let got = read_vec(&db, format!("k{i:06}").as_bytes());
        assert_eq!(got.len(), 2, "k{i:06}");
        assert_eq!(got[1], i.to_le_bytes().to_vec());
    }
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    let mut n = 0usize;
    db.scan(b"", usize::MAX, |_, _| n += 1).unwrap();
    assert_eq!(n, 40_000, "every value survives the knobs and a reopen");
}
