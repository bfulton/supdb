//! The engine's contract: multivalue order across seals, a
//! model-checked read path, and the crash windows the module doc enumerates.
//! Every crash here is emulated the way `tests/known_bugs.rs` emulates them:
//! by constructing the exact on-disk state the window leaves behind, because
//! a clean result on a path a test never took proves nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use supdb::{Db, Isolation, Options, ReadAdvice, Reader};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read_vec(db: &Reader, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    db.read_all(key, |v| out.push(v.to_vec())).unwrap();
    out
}

#[test]
fn values_come_back_in_append_order_across_seals() {
    let d = dir("order");
    let mut db = Db::create(&d, Options::default()).unwrap();
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
        assert_eq!(
            &read_vec(&db, key),
            want,
            "key {}",
            String::from_utf8_lossy(key)
        );
    }
    db.close().unwrap();
}

#[test]
fn reads_see_uncommitted_and_unsealed_state() {
    let d = dir("ryw");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.append(b"k", b"one");
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"one".to_vec()],
        "pre-commit read"
    );
    db.commit().unwrap();
    db.append(b"k", b"two");
    assert_eq!(read_vec(&db, b"k"), vec![b"one".to_vec(), b"two".to_vec()]);
}

#[test]
fn killed_before_first_seal_opens_from_the_wal_alone() {
    // P-E: no segment exists, only a WAL.
    let d = dir("preseal");
    let mut db = Db::create(&d, Options::default()).unwrap();
    for i in 0u32..500 {
        db.append(format!("k{i}").as_bytes(), &i.to_le_bytes());
    }
    db.commit().unwrap();
    drop(db); // no close, no seal: the crash

    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(db.segments(), 0, "nothing was sealed");
    for i in 0u32..500 {
        assert_eq!(
            read_vec(&db, format!("k{i}").as_bytes()),
            vec![i.to_le_bytes().to_vec()]
        );
    }
}

#[test]
fn uncommitted_tail_is_lost_whole_and_committed_state_survives() {
    let d = dir("tail");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.append(b"durable", b"yes");
    db.commit().unwrap();
    db.append(b"volatile", b"never-synced");
    drop(db); // pending buffer never reached the file

    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(read_vec(&db, b"durable"), vec![b"yes".to_vec()]);
    assert_eq!(read_vec(&db, b"volatile"), Vec::<Vec<u8>>::new());
}

#[test]
fn a_torn_tail_loses_its_batch_whole_and_earlier_batches_survive() {
    let d = dir("torn");
    // The WAL's own window: these keys arrive in order and would go
    // straight into a segment, so the WAL path is held on its arm.
    let mut db = Db::create(&d, wal_arm()).unwrap();
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

    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(
        read_vec(&db, b"a"),
        vec![b"1".to_vec()],
        "a committed batch survives"
    );
    assert_eq!(
        read_vec(&db, b"b"),
        Vec::<Vec<u8>>::new(),
        "the torn batch is gone whole"
    );
    assert_eq!(
        read_vec(&db, b"c"),
        Vec::<Vec<u8>>::new(),
        "the torn batch is gone whole"
    );
}

#[test]
fn a_transaction_is_all_or_nothing_and_sees_its_own_writes() {
    let d = dir("txn");
    let mut db = Db::create(&d, Options::default()).unwrap();
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
        assert_eq!(
            got,
            vec![b"1".to_vec(), b"2".to_vec()],
            "read-your-writes inside"
        );
        let mut got = Vec::new();
        tx.read_all(b"z", |v| got.push(v.to_vec())).unwrap();
        assert_eq!(
            got,
            vec![b"new".to_vec()],
            "a staged delete masks the store's values"
        );
        assert_eq!(tx.count(b"z").unwrap(), 1);
        assert_eq!(tx.count(b"x").unwrap(), 2);
        tx.abort();
    }
    assert!(
        read_vec(&db, b"x").is_empty(),
        "an aborted transaction leaves nothing"
    );
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
    assert!(
        read_vec(&db, b"y").is_empty(),
        "dropped without commit is abort"
    );
    drop(db);
    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(
        read_vec(&db, b"x"),
        vec![b"1".to_vec(), b"2".to_vec()],
        "committed, durably"
    );
    assert_eq!(read_vec(&db, b"z"), vec![b"new".to_vec()]);
    assert!(read_vec(&db, b"y").is_empty());
}

#[test]
fn crash_between_rename_and_wal_reset_does_not_duplicate() {
    // The window the segment file name exists for: the seal renamed its
    // segment into place and synced the directory, then the process died
    // before the WAL reset. The WAL still holds every sealed record.
    let d = dir("renamewin");
    let mut db = Db::create(&d, Options::default()).unwrap();
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

    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(db.segments(), 1);
    let got = read_vec(&db, b"dup-window");
    assert_eq!(
        got.len(),
        40,
        "sealed records must not replay into duplicates"
    );
    for (i, v) in got.iter().enumerate() {
        assert_eq!(v, &(i as u32).to_le_bytes().to_vec());
    }
}

#[test]
fn reopen_after_seal_serves_both_old_and_new_writes() {
    let d = dir("reopen");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.append(b"k", b"sealed");
    db.commit().unwrap();
    db.seal().unwrap();
    db.append(b"k", b"walled");
    db.commit().unwrap();
    drop(db); // crash with one segment and one live WAL record

    let mut db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"sealed".to_vec(), b"walled".to_vec()]
    );
    db.append(b"k", b"after");
    db.commit().unwrap();
    db.seal().unwrap();
    drop(db);

    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"sealed".to_vec(), b"walled".to_vec(), b"after".to_vec()],
        "order survives a second generation of seals"
    );
}

fn oracle(cursors: bool) {
    // The differential model oracle: random appends, commits, seals, and
    // crash-reopens, checked against a HashMap after every reopen.
    // Uncommitted writes are trimmed from the model at a crash, which is the
    // durability contract.
    let d = dir(&format!(
        "oracle-{}",
        if cursors { "cursors" } else { "probes" }
    ));
    let opts = Options {
        cursor_merge: cursors,
        ..Options::default()
    };
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
        // One op in twenty is a delete.
        if rng() % 20 == 0 {
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
    db.scan(b"", usize::MAX, |k, v| {
        scanned.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    let live: HashMap<Vec<u8>, Vec<Vec<u8>>> = model
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
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
    oracle(true)
}

/// The probe merge stays behind `cursor_merge` as the comparison arm -- and
/// a path only one arm exercises is a path nothing tests.
#[test]
fn the_probe_merge_arm_passes_the_same_oracle() {
    oracle(false)
}

/// The names of the live segment files, sorted. A promoted piece keeps the
/// id the seal gave it; a merge writes a fresh one, so the id in the name is
/// what tells the two apart on disk.
fn seg_names(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    v.sort();
    v
}

/// A flush that leaves one full-range piece promotes it instead of merging.
///
/// The merge it used to fall through to read that piece back and wrote it out
/// again as one partition over the same range -- a quarter of the load window
/// at 100k keys, and half the device bytes the store wrote. The store this
/// leaves has to be the one the merge would have left, so this checks the
/// shape and every value, not just that it was fast.
#[test]
fn a_lone_piece_is_promoted_rather_than_rewritten() {
    let d = dir("promote-one");
    let mut db = Db::create(&d, Options::default()).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for k in 0u32..2_000 {
        let key = format!("key-{k:05}").into_bytes();
        let val = format!("v-{k}").into_bytes();
        db.append(&key, &val);
        model.insert(key, vec![val]);
    }
    db.commit().unwrap();
    db.flush().unwrap();

    let names = seg_names(&d);
    assert_eq!(names.len(), 1, "one piece in, one partition out: {names:?}");
    assert!(names[0].starts_with("par-"), "left unrouted: {names:?}");
    // Id 0 is the one the seal allocated. A merge would have taken the next.
    assert!(
        names[0].starts_with("par-00000000-"),
        "rewritten by a merge rather than promoted: {names:?}"
    );
    assert_eq!(db.segments(), 1);
    for (key, want) in &model {
        assert_eq!(
            &read_vec(&db, key),
            want,
            "key {}",
            String::from_utf8_lossy(key)
        );
    }
    let mut seen = Vec::new();
    db.scan(b"", model.len(), |k, _| seen.push(k.to_vec()))
        .unwrap();
    let mut want: Vec<Vec<u8>> = model.keys().cloned().collect();
    want.sort();
    assert_eq!(
        seen, want,
        "the promoted partition does not scan in key order"
    );
    db.close().unwrap();
}

/// The same flush, with a tombstone in the piece: promotion keeps the file as
/// it is and a tombstone has to be collected, so this one must still merge.
#[test]
fn a_lone_piece_holding_a_tombstone_still_merges() {
    let d = dir("promote-tomb");
    let mut db = Db::create(&d, Options::default()).unwrap();
    for k in 0u32..2_000 {
        db.append(
            format!("key-{k:05}").as_bytes(),
            format!("v-{k}").as_bytes(),
        );
    }
    db.commit().unwrap();
    db.delete(b"key-00042");
    db.commit().unwrap();
    db.flush().unwrap();

    let names = seg_names(&d);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        !names[0].starts_with("par-00000000-"),
        "promoted a piece carrying a tombstone: {names:?}"
    );
    assert!(
        read_vec(&db, b"key-00042").is_empty(),
        "the delete came back"
    );
    assert_eq!(read_vec(&db, b"key-00041"), vec![b"v-41".to_vec()]);
    assert_eq!(read_vec(&db, b"key-00043"), vec![b"v-43".to_vec()]);
    db.close().unwrap();
}

fn ord_names(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("ord-"))
        .collect();
    v.sort();
    v
}

/// Every live segment has an ordered index, and the store refuses to open
/// without it.
///
/// There is deliberately no fallback to `Blob::seek`. A reader that quietly
/// took the slow path when the index was missing would answer correctly
/// forever and never say so -- the shape of every gate this repository has
/// broken -- so a missing or damaged index is damage, like a torn key
/// section, and the open fails.
#[test]
fn a_segment_without_its_ordered_index_refuses_to_open() {
    let d = dir("ord-required");
    let mut db = Db::create(&d, Options::default()).unwrap();
    for k in 0u32..2_000 {
        db.append(
            format!("key-{k:05}").as_bytes(),
            format!("v-{k}").as_bytes(),
        );
    }
    db.commit().unwrap();
    db.flush().unwrap();
    let segs = seg_names(&d);
    let ords = ord_names(&d);
    assert_eq!(
        ords.len(),
        segs.len(),
        "one index a segment: {segs:?} {ords:?}"
    );
    assert_eq!(read_vec(&db, b"key-00500"), vec![b"v-500".to_vec()]);
    db.close().unwrap();

    // Reopens cleanly as it stands.
    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(read_vec(&db, b"key-00500"), vec![b"v-500".to_vec()]);
    db.close().unwrap();

    // Damaged: a flipped byte fails the open rather than falling back.
    let victim = d.join(&ords[0]);
    let mut bytes = std::fs::read(&victim).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 0x40;
    std::fs::write(&victim, &bytes).unwrap();
    assert!(
        Db::open(&d, Options::default()).is_err(),
        "opened over a damaged ordered index"
    );

    // Absent: the same answer, not a silent slow path.
    std::fs::remove_file(&victim).unwrap();
    assert!(
        Db::open(&d, Options::default()).is_err(),
        "opened with an ordered index missing"
    );
}

/// The index has to survive a promotion, which renames the segment. It is
/// named by the id and covered end-sequence, which a promotion keeps, so
/// there is nothing to rename -- and this is what says so.
#[test]
fn an_ordered_index_survives_promotion_and_reopen() {
    let d = dir("ord-promote");
    let mut db = Db::create(&d, small_opts(3)).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    for k in 0u32..4_000 {
        let key = format!("key-{k:06}").into_bytes();
        let val = format!("v-{k}").into_bytes();
        db.append(&key, &val);
        model.insert(key, vec![val]);
        if k % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.close().unwrap();

    let db = Db::open(&d, small_opts(3)).unwrap();
    assert_eq!(ord_names(&d).len(), seg_names(&d).len());
    for (key, want) in &model {
        assert_eq!(
            &read_vec(&db, key),
            want,
            "key {}",
            String::from_utf8_lossy(key)
        );
    }
    let mut seen = Vec::new();
    db.scan(b"", model.len(), |k, _| seen.push(k.to_vec()))
        .unwrap();
    let mut want: Vec<Vec<u8>> = model.keys().cloned().collect();
    want.sort();
    assert_eq!(seen, want, "ordered scan after promotion and reopen");
    db.close().unwrap();
}

/// The write path before direct ingest: every batch through the WAL and
/// the seal. What a test of the WAL's own crash windows, or of the shape a
/// seal leaves, has to ask for, since ordered writes otherwise never reach
/// either.
fn wal_arm() -> Options {
    Options {
        direct_ingest: false,
        ..Options::default()
    }
}

fn small_opts(l0_trigger: usize) -> Options {
    // Small enough that a few hundred records seal and compact, so the
    // level machinery is exercised at test scale rather than described.
    // Partitions follow the seal here (`partition_bytes: None`): these tests
    // want many small partitions, where the shipping default holds them at
    // 64 MB whatever the seal size.
    Options {
        seal_bytes: 4 << 10,
        l0_trigger,
        partition_bytes: None,
        ..Options::default()
    }
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
    assert!(
        partitioned > 1,
        "the merge should have split the key space, got {partitioned}"
    );
    assert!(l0 <= 3, "the tail stays bounded by l0_trigger, got {l0}");
    for (key, want) in &model {
        assert_eq!(
            &read_vec(&db, key),
            want,
            "key {}",
            String::from_utf8_lossy(key)
        );
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
        assert_eq!(
            &read_vec(&db, key),
            want,
            "after reopen: {}",
            String::from_utf8_lossy(key)
        );
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
    assert!(
        !post.is_empty(),
        "the copy produced no new segments; the window was not staged"
    );
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
    scale(true)
}

/// The original flush -- re-partition everything from every key -- stays
/// behind `flush_ranges: false` as the comparison arm, and a path only one
/// arm exercises is a path nothing tests.
#[test]
fn every_key_survives_the_full_flush_too() {
    scale(false)
}

fn scale(flush_ranges: bool) {
    // The contract tests above use stores too small to make a partition
    // boundary interesting. This one loads enough to force the initial
    // partitioning AND several per-range merges, then demands every key
    // back. A fence that does not tile the key space loses keys silently
    // here and nowhere smaller.
    let d = dir(if flush_ranges {
        "scale"
    } else {
        "scale-fullflush"
    });
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 512 << 10,
            l0_trigger: 3,
            partition_bytes: None,
            flush_ranges,
            ..Options::default()
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
    assert!(
        par > 1,
        "expected several partitions, got {par} with {l0} in the tail"
    );

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
        assert_eq!(
            db.count(key).unwrap(),
            *want,
            "key {}",
            String::from_utf8_lossy(key)
        );
        // And it agrees with actually reading them, which is the only
        // definition of correct that matters.
        assert_eq!(read_vec(&db, key).len() as u64, *want);
    }
    drop(db);

    let db = Db::open(&d, small_opts(2)).unwrap();
    for (key, want) in &model {
        assert_eq!(
            db.count(key).unwrap(),
            *want,
            "after reopen: {}",
            String::from_utf8_lossy(key)
        );
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
    use supdb::SyncPolicy;
    let d = dir("everyn");
    // The WAL's own window, on its arm; see `wal_arm`.
    let opts = Options {
        sync: SyncPolicy::EveryN(16),
        ..wal_arm()
    };
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
    std::fs::OpenOptions::new()
        .write(true)
        .open(&wal)
        .unwrap()
        .set_len(len - 5)
        .unwrap();

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
    assert_eq!(
        read_vec(&db, b"k022"),
        Vec::<Vec<u8>>::new(),
        "the torn frame is the crash point"
    );
    for c in 16u32..22 {
        assert_eq!(read_vec(&db, format!("k{c:03}").as_bytes()).len(), 1);
    }
}

fn dir_bytes(d: &std::path::Path) -> u64 {
    std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum()
}

#[test]
fn a_delete_ends_older_values_and_later_appends_start_fresh() {
    let d = dir("delete");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.append(b"k", b"v1");
    db.append(b"k", b"v2");
    db.commit().unwrap();
    db.seal().unwrap();
    db.append(b"k", b"v3");
    db.commit().unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"v1".to_vec(), b"v2".to_vec(), b"v3".to_vec()]
    );
    db.delete(b"k");
    assert_eq!(
        read_vec(&db, b"k"),
        Vec::<Vec<u8>>::new(),
        "a delete ends everything before it, sealed or not"
    );
    assert_eq!(db.count(b"k").unwrap(), 0);
    db.append(b"k", b"v4");
    db.commit().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()]);
    assert_eq!(db.count(b"k").unwrap(), 1);
    db.seal().unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"v4".to_vec()],
        "through a sealed tombstone"
    );
    drop(db);
    let mut db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()], "after reopen");
    db.flush().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"v4".to_vec()], "after the merge");
    assert_eq!(db.count(b"k").unwrap(), 1);
    let mut scanned = Vec::new();
    db.scan(b"", usize::MAX, |k, v| {
        scanned.push((k.to_vec(), v.to_vec()))
    })
    .unwrap();
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
    db.scan(b"", usize::MAX, |k, v| {
        scanned.push((k.to_vec(), v.to_vec()))
    })
    .unwrap();
    assert!(
        scanned.is_empty(),
        "a merged-away key does not scan: {scanned:?}"
    );
}

#[test]
fn deleted_values_do_not_survive_the_merge() {
    let d = dir("delete-merge");
    let opts = Options {
        seal_bytes: 256 << 10,
        l0_trigger: 3,
        ..Options::default()
    };
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
    // The WAL's own window, on its arm; see `wal_arm`.
    let mut db = Db::create(&d, wal_arm()).unwrap();
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
        let mut db = Db::open(&dd, wal_arm()).unwrap();
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
        let db = Db::open(&dd, wal_arm()).unwrap();
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
    // These two knobs move where the seal's and merge's bytes go and when;
    // neither may change what a reader sees, through seals, merges and a
    // reopen. The idle class may be ignored by the host's scheduler and
    // the syscall may fail -- both are silent by design, and the store
    // must be identical either way.
    use supdb::BackgroundIo;
    let d = dir("ioprio");
    let opts = Options {
        seal_bytes: 256 << 10,
        l0_trigger: 3,
        background_io: BackgroundIo::Idle,
        seal_sync_every: 64 << 10,
        partition_bytes: Some(512 << 10),
        ..Options::default()
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

#[test]
fn ordered_pieces_are_promoted_to_partitions_without_a_merge() {
    // A log's shape: every seal's keys lie above everything sealed before,
    // so nothing overlaps and nothing needs merging. With promotion each
    // piece becomes a partition by rename; the merge phase stays near
    // zero and every key reads back through the fences.
    let d = dir("promote");
    let opts = Options {
        seal_bytes: 256 << 10,
        l0_trigger: 3,
        partition_bytes: None,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let filler = vec![b'p'; 100];
    for i in 0..12_000u32 {
        let k = format!("log-{i:08}");
        db.append(k.as_bytes(), &filler);
        db.append(k.as_bytes(), &i.to_le_bytes());
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    let (parts, l0) = db.levels();
    assert!(
        parts >= 4,
        "ordered pieces should have become partitions: {parts} partitions, {l0} pieces"
    );
    assert_eq!(l0, 0);
    let (_, _, merge_ns) = db.phase_ns();
    assert!(
        merge_ns < 50_000_000,
        "promotion should not spend a merge's time: {merge_ns} ns"
    );
    for i in (0..12_000u32).step_by(997) {
        let got = read_vec(&db, format!("log-{i:08}").as_bytes());
        assert_eq!(got.len(), 2, "log-{i:08}");
        assert_eq!(got[1], i.to_le_bytes().to_vec());
    }
    let mut n = 0usize;
    let mut last: Vec<u8> = Vec::new();
    db.scan(b"", usize::MAX, |k, _| {
        assert!(k >= last.as_slice(), "scan order");
        last = k.to_vec();
        n += 1;
    })
    .unwrap();
    assert_eq!(n, 24_000);
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    assert_eq!(read_vec(&db, b"log-00011999").len(), 2, "after reopen");
    assert_eq!(read_vec(&db, b"log-00000000").len(), 2);
    let mut names: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    names.sort();
    assert!(
        names.iter().all(|n| n.starts_with("par-")),
        "every segment is a partition: {names:?}"
    );
}

#[test]
fn a_wal_header_torn_by_power_loss_opens_and_is_rewritten() {
    // A seal rotates to a fresh WAL whose eight-byte header has been
    // written and not synced -- nothing in it has, until the first commit
    // into it. A power loss there leaves a prefix of the header, and the
    // store must open on its segments alone, then write a whole header
    // before appending. The crash experiment tears exactly this in a third
    // of its trials; this is the one-shot version.
    let d = dir("torn-header");
    let opts = Options {
        seal_bytes: 1 << 10,
        ..Options::default()
    };
    let newest_wal = |d: &std::path::Path| -> std::path::PathBuf {
        let mut wals: Vec<std::path::PathBuf> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("wal-"))
            .collect();
        wals.sort();
        wals.last().unwrap().clone()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for i in 0u32..64 {
        db.append(format!("k{i:03}").as_bytes(), &[7u8; 64]);
        db.commit().unwrap();
    }
    db.flush().unwrap();
    drop(db);
    for (round, cut) in [0u64, 1, 3, 7].into_iter().enumerate() {
        let live = newest_wal(&d);
        assert_eq!(
            std::fs::metadata(&live).unwrap().len(),
            8,
            "a rotated WAL holds only its header"
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(&live)
            .unwrap()
            .set_len(cut)
            .unwrap();
        let mut db = Db::open(&d, opts.clone()).unwrap();
        for i in 0u32..64 {
            assert_eq!(
                read_vec(&db, format!("k{i:03}").as_bytes()),
                vec![vec![7u8; 64]]
            );
        }
        let key = format!("after{round}");
        db.append(key.as_bytes(), b"x");
        db.commit().unwrap();
        drop(db);
        let mut db = Db::open(&d, opts.clone()).unwrap();
        assert_eq!(
            read_vec(&db, key.as_bytes()),
            vec![b"x".to_vec()],
            "cut {cut}: the header was rewritten whole"
        );
        // Seal so the next round starts from a header-only live WAL again.
        db.flush().unwrap();
        drop(db);
    }
}

#[test]
fn a_recycled_wal_never_adopts_a_frame_from_its_previous_life() {
    // With `recycle_wal`, a rotation renames a retired WAL into place and
    // writes over its old frames. If the new life commits less than the
    // old one did, the old life's frames sit past the new tail -- intact,
    // CRC-correct under the old id -- and replay must stop at the new tail
    // rather than read them. Their keys are already in a segment, so
    // adopting one would show as a duplicated value.
    let d = dir("recycle-stale");
    let opts = Options {
        recycle_wal: true,
        seal_bytes: 64 << 10,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // Life 1 of wal-00000000: about 3 seals' worth, so the file is long.
    for i in 0u32..3000 {
        db.append(format!("k{i:05}").as_bytes(), &[1u8; 64]);
        if i % 50 == 49 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    let (live, _, _) = db.wal_durable();
    // After the flush the live WAL is a recycled file with a long stale
    // tail behind an eight-byte header.
    assert!(
        std::fs::metadata(&live).unwrap().len() > 8,
        "the live WAL should be a recycled file with old frames behind the header"
    );
    // Life 2: two small batches, then a crash.
    db.append(b"new-a", b"A");
    db.commit().unwrap();
    db.append(b"new-b", b"B");
    db.commit().unwrap();
    drop(db);

    let db = Db::open(&d, opts.clone()).unwrap();
    assert_eq!(read_vec(&db, b"new-a"), vec![b"A".to_vec()]);
    assert_eq!(read_vec(&db, b"new-b"), vec![b"B".to_vec()]);
    for i in 0u32..3000 {
        let got = read_vec(&db, format!("k{i:05}").as_bytes());
        assert_eq!(
            got.len(),
            1,
            "k{i:05} came back {} times: a stale frame was adopted",
            got.len()
        );
    }
    drop(db);
    // And again after another commit and reopen: the truncation at open
    // must have cut the stale tail so nothing behind the new frames is
    // ever read.
    let mut db = Db::open(&d, opts.clone()).unwrap();
    db.append(b"new-c", b"C");
    db.commit().unwrap();
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    assert_eq!(read_vec(&db, b"new-c"), vec![b"C".to_vec()]);
    for i in (0u32..3000).step_by(97) {
        assert_eq!(read_vec(&db, format!("k{i:05}").as_bytes()).len(), 1);
    }
}

#[test]
fn recycling_survives_crashes_and_leaves_no_spare_after_close() {
    let d = dir("recycle-crash");
    let opts = Options {
        recycle_wal: true,
        seal_bytes: 32 << 10,
        l0_trigger: 2,
        ..Options::default()
    };
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for round in 0u32..12 {
        for i in 0u32..400 {
            let k = format!("k{:04}", (i * 7 + round) % 500).into_bytes();
            let v = format!("r{round}i{i}").into_bytes();
            db.append(&k, &v);
            model.entry(k).or_default().push(v);
        }
        db.commit().unwrap();
        if round % 4 == 3 {
            drop(db); // crash
            db = Db::open(&d, opts.clone()).unwrap();
            for (k, want) in &model {
                assert_eq!(&read_vec(&db, k), want, "after the crash in round {round}");
            }
        }
    }
    db.close().unwrap();
    let spares = std::fs::read_dir(&d)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("spare-")
        })
        .count();
    assert_eq!(spares, 0, "close leaves no spare behind");
    let db = Db::open(&d, opts).unwrap();
    for (k, want) in &model {
        assert_eq!(&read_vec(&db, k), want);
    }
}

#[test]
fn a_flipped_byte_anywhere_in_a_batch_loses_that_batch_and_the_ones_after() {
    // The CRC is per batch. The contract it must keep is the one a
    // CRC per frame gave: damage anywhere inside a batch -- a record
    // frame's header, its key, its value, the commit frame -- loses that
    // batch and every batch after it, and nothing before it. Every byte
    // offset of a three-batch WAL is flipped in turn.
    let d = dir("flip");
    let mut db = Db::create(&d, Options::default()).unwrap();
    let mut ends = Vec::new();
    for b in 0u32..3 {
        for i in 0..4u32 {
            db.append(
                format!("b{b}k{i}").as_bytes(),
                format!("v{b}{i}").as_bytes(),
            );
        }
        if b == 1 {
            db.delete(b"b0k0");
        }
        db.commit().unwrap();
        ends.push(db.wal_durable().2 as usize);
    }
    drop(db);
    let wal = d.join("wal-00000000");
    let full = std::fs::read(&wal).unwrap();
    assert_eq!(full.len(), *ends.last().unwrap());
    for off in 8..full.len() {
        let dd = dir(&format!("flip-{off}"));
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_name() != "wal-00000000" {
                std::fs::copy(e.path(), dd.join(e.file_name())).unwrap();
            }
        }
        let mut damaged = full.clone();
        damaged[off] ^= 0x5a;
        std::fs::write(dd.join("wal-00000000"), &damaged).unwrap();
        // The batch that holds this byte, and so the number that survive.
        let survive = ends.iter().position(|&e| off < e).unwrap();
        let db = match Db::open(&dd, Options::default()) {
            Ok(db) => db,
            Err(e) => panic!("offset {off}: open refused the store: {e}"),
        };
        for b in 0u32..3 {
            for i in 0..4u32 {
                let key = format!("b{b}k{i}");
                let got = read_vec(&db, key.as_bytes());
                let deleted = b == 0 && i == 0 && survive >= 2;
                let want: Vec<Vec<u8>> = if (b as usize) < survive && !deleted {
                    vec![format!("v{b}{i}").into_bytes()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    got, want,
                    "offset {off} (batch {survive} damaged): key {key}"
                );
            }
        }
    }
}

#[test]
fn put_replaces_and_append_accumulates() {
    // The two write verbs. YCSB's update is a put; the load is appends; the
    // external harness confused them once and every Zipfian rewrite piled
    // onto its key until reads walked the pile.
    let d = dir("put");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.append(b"k", b"1");
    db.append(b"k", b"2");
    db.commit().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"1".to_vec(), b"2".to_vec()]);
    db.put(b"k", b"3");
    db.commit().unwrap();
    assert_eq!(read_vec(&db, b"k"), vec![b"3".to_vec()], "a put replaces");
    assert_eq!(db.count(b"k").unwrap(), 1);
    db.append(b"k", b"4");
    db.commit().unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"3".to_vec(), b"4".to_vec()],
        "appends after a put accumulate again"
    );
    drop(db);
    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(
        read_vec(&db, b"k"),
        vec![b"3".to_vec(), b"4".to_vec()],
        "and the WAL replays the same"
    );
}

/// A store that merges does not accumulate ordered indexes whose segments
/// are gone. The sweep at open would collect them, but a process that
/// merges for hours never reaches it: the leak was found as 53,596 files in
/// one store directory on a run at a hundred million keys, and it is
/// counted here without reopening, because reopening is what hid it.
#[test]
fn a_merge_takes_the_ordered_index_with_the_segment_it_retires() {
    let d = dir("ord-leak");
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 64 << 10,
            partition_bytes: Some(128 << 10),
            ..Default::default()
        },
    )
    .unwrap();
    for round in 0u32..12 {
        for k in 0u32..2_000 {
            let key = format!("key-{k:06}").into_bytes();
            db.append(
                &key,
                format!("value-{round}-{k}-{}", "z".repeat(40)).as_bytes(),
            );
        }
        db.commit().unwrap();
        db.flush().unwrap();
    }
    db.settle().unwrap();
    let count = |suffix: &str| {
        std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
            .count()
    };
    let (segs, ords) = (count(".sup"), count(".oidx"));
    assert!(segs > 0, "the store sealed nothing, so this proves nothing");
    assert_eq!(
        ords, segs,
        "{ords} ordered indexes for {segs} segments: the retired ones leaked"
    );
}

#[test]
fn an_advising_store_advises_the_ordered_companions_too() {
    // `Db::advise` walks the segments, and a segment's ordered companion is
    // not one of them, so the companion was left on the kernel's default
    // readahead under every policy. It is reached only by `seek`'s binary
    // search -- the most random access the engine makes -- and out of core
    // the default's readahead fetched a window per probe to consume eight
    // bytes, costing 12% of scan throughput on a store past the page cache.
    // Nothing failed and no check could see it.
    let build = |name: &str, advice: ReadAdvice| {
        let d = dir(name);
        let mut db = Db::create(
            &d,
            Options {
                seal_bytes: 64 << 10,
                partition_bytes: Some(128 << 10),
                read_advice: advice,
                ..Default::default()
            },
        )
        .unwrap();
        for round in 0u32..6 {
            for k in 0u32..2_000 {
                let key = format!("key-{k:06}").into_bytes();
                db.append(&key, format!("v-{round}-{k}-{}", "z".repeat(40)).as_bytes());
            }
            db.commit().unwrap();
            db.flush().unwrap();
        }
        db.settle().unwrap();
        db
    };

    let db = build("ord-advice-adaptive", ReadAdvice::Adaptive);
    let (advised, total) = db.ords_advised();
    assert!(total > 1, "{total} segments: this proves nothing");
    assert_eq!(advised, total, "{advised} of {total} companions advised");

    // A scan long enough to want readahead puts the segments on the kernel's
    // default -- the companion must not follow them there, because a binary
    // search is random in either phase. It has to be a long one: a scan is
    // advised by the span it will walk, and a short scan now keeps
    // MADV_RANDOM for the segments too.
    db.scan(b"key-000000", 100_000, |_, _| {}).unwrap();
    assert!(!db.advice_random(), "the scan did not switch the segments");
    let (advised, total) = db.ords_advised();
    assert_eq!(
        advised,
        total,
        "a scan un-advised {} companions",
        total - advised
    );

    // And the arm that prices the advice still gets none of it, or it is
    // measuring the same thing twice.
    let db = build("ord-advice-normal", ReadAdvice::Normal);
    let (advised, total) = db.ords_advised();
    assert!(total > 1, "{total} segments: this proves nothing");
    assert_eq!(advised, 0, "{advised} of {total} advised under Normal");
}

#[test]
fn a_short_scan_keeps_the_random_advice_and_a_long_one_gives_it_up() {
    // `Adaptive` put the segments on the kernel's default for every scan,
    // because a scan walks values in order and wants the pages ahead of it.
    // That is right for a long scan and wrong for a short one: out of core a
    // hundred-entry scan ran 9.4x faster under MADV_RANDOM, each of its seeks
    // paying for a readahead it never read, while a hundred-thousand-entry
    // scan ran 3.6x slower under it. The limit says which is which before a
    // page is touched, so the store does not have to guess.
    let d = dir("scan-span-advice");
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 64 << 10,
            partition_bytes: Some(128 << 10),
            scan_readahead_bytes: 256 << 10,
            ..Default::default()
        },
    )
    .unwrap();
    for round in 0u32..6 {
        for k in 0u32..3_000 {
            let key = format!("key-{k:06}").into_bytes();
            db.append(&key, format!("v-{round}-{k}-{}", "z".repeat(60)).as_bytes());
        }
        db.commit().unwrap();
        db.flush().unwrap();
    }
    db.settle().unwrap();

    // A point read leaves the store advised random; that much was already so.
    let _ = db.read_all(b"key-000001", |_| {}).unwrap();
    assert!(db.advice_random(), "a point read should advise random");

    // A scan of one entry cannot span the threshold, so it must NOT give the
    // advice up. This is the case that was 9.4x slow.
    db.scan(b"key-000000", 1, |_, _| {}).unwrap();
    assert!(
        db.advice_random(),
        "a one-entry scan dropped MADV_RANDOM: it cannot span {} bytes",
        256 << 10
    );

    // A scan long enough to cover the threshold several times over wants the
    // readahead, and has to be able to ask for it.
    db.scan(b"key-000000", 100_000, |_, _| {}).unwrap();
    assert!(
        !db.advice_random(),
        "a 100,000-entry scan kept MADV_RANDOM: readahead is what it is for"
    );

    // And back, because the span is asked per scan rather than latched once.
    db.scan(b"key-000000", 1, |_, _| {}).unwrap();
    assert!(
        db.advice_random(),
        "the advice did not come back for a short scan"
    );

    // Reopened, not created. `open` sorts its own segments instead of going
    // through `sort_segs`, so the mean a scan compares against was seeded to
    // zero there and every scan took the short path however long it was. The
    // create path above cannot see that, which is the whole reason this is
    // here.
    drop(db);
    let db = Db::open(
        &d,
        Options {
            seal_bytes: 64 << 10,
            partition_bytes: Some(128 << 10),
            scan_readahead_bytes: 256 << 10,
            ..Default::default()
        },
    )
    .unwrap();
    db.scan(b"key-000000", 100_000, |_, _| {}).unwrap();
    assert!(
        !db.advice_random(),
        "after reopen a 100,000-entry scan kept MADV_RANDOM: the mean was never seeded"
    );
    db.scan(b"key-000000", 1, |_, _| {}).unwrap();
    assert!(
        db.advice_random(),
        "after reopen the advice did not come back"
    );
}

/// What a scan over the store must answer, kept beside it: each key's live
/// values in append order, and the keys any unsealed source has touched
/// since the last flush. A touched key is visited by the scan and counts
/// toward its limit whether or not it has a value left -- that is what the
/// merge over unrouted sources does with a tombstone-only key, and the bulk
/// walk has to agree with it.
#[derive(Default)]
struct ScanModel {
    vals: BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
    touched: BTreeSet<Vec<u8>>,
}

impl ScanModel {
    fn append(&mut self, db: &mut Db, key: &str, val: &str) {
        db.append(key.as_bytes(), val.as_bytes());
        self.vals
            .entry(key.as_bytes().to_vec())
            .or_default()
            .push(val.as_bytes().to_vec());
        self.touched.insert(key.as_bytes().to_vec());
    }
    fn delete(&mut self, db: &mut Db, key: &str) {
        db.delete(key.as_bytes());
        self.vals
            .entry(key.as_bytes().to_vec())
            .or_default()
            .clear();
        self.touched.insert(key.as_bytes().to_vec());
    }
    /// A flush merges every unsealed source into the partitions and drops
    /// the keys with nothing left.
    fn flushed(&mut self) {
        self.vals.retain(|_, v| !v.is_empty());
        self.touched.clear();
    }
    /// The keys a scan visits, in order.
    fn visited(&self) -> Vec<&[u8]> {
        self.vals
            .iter()
            .filter(|(k, v)| !v.is_empty() || self.touched.contains(*k))
            .map(|(k, _)| k.as_slice())
            .collect()
    }
    /// Every scan the store can be asked for, against the model: from every
    /// visited key, from between them, from below and from past the end,
    /// at limits from one to unbounded. Both the stream and the count.
    fn check(&self, db: &Reader, state: &str) {
        // Point reads and counts of a sample of every key ever written,
        // deleted ones included: a scan and a read take different paths
        // into the memtables, and a lookup that answered nothing while the
        // scans stayed right went unseen until this was added.
        for k in self.touched.iter().step_by(7) {
            let want = self.vals.get(k).cloned().unwrap_or_default();
            assert_eq!(
                read_vec(db, k),
                want,
                "{state}: read {:?}",
                String::from_utf8_lossy(k)
            );
            assert_eq!(
                db.count(k).unwrap(),
                want.len() as u64,
                "{state}: count {:?}",
                String::from_utf8_lossy(k)
            );
        }
        let visited = self.visited();
        // Every visited key is a start while there are few; past a few
        // hundred, every scan still streams every key but the starts are
        // sampled, or a check over a burst of thousands takes minutes in
        // the debug build.
        let step = visited.len().div_ceil(400).max(1);
        let sample = visited.iter().step_by(step);
        let mut starts: Vec<Vec<u8>> = sample.clone().map(|k| k.to_vec()).collect();
        starts.extend(sample.map(|k| [k, &b"+"[..]].concat()));
        starts.push(Vec::new());
        starts.push(b"a".to_vec());
        starts.push(b"zzz".to_vec());
        for from in &starts {
            for &limit in &[1usize, 2, 3, 5, 17, usize::MAX] {
                let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let mut want_n = 0usize;
                for k in visited
                    .iter()
                    .filter(|k| **k >= from.as_slice())
                    .take(limit)
                {
                    want_n += 1;
                    for v in &self.vals[*k] {
                        want.push((k.to_vec(), v.clone()));
                    }
                }
                let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let n = db
                    .scan(from, limit, |k, v| got.push((k.to_vec(), v.to_vec())))
                    .unwrap();
                assert_eq!(
                    got,
                    want,
                    "{state}: scan from {:?} limit {limit}",
                    String::from_utf8_lossy(from)
                );
                assert_eq!(
                    n,
                    want_n,
                    "{state}: count from {:?} limit {limit}",
                    String::from_utf8_lossy(from)
                );
            }
        }
    }
}

/// The bulk walk over partitions with unsealed keys laid over it, held to
/// the merge it stands in for, in every state the store passes through.
///
/// The walk used to run only when no unsealed key was at or after the
/// scan's start. YCSB's inserts land past the end of the loaded range, so
/// one insert sent every later scan through the merge for keys it never
/// reached. Now the walk runs whenever there is no level-0 piece, and this
/// holds it to a model through every source an unsealed key can come from:
/// the live memtable alone (the YCSB shape, inserts past the end), then the
/// frozen memtable under a seal that has not been joined -- deterministic,
/// because only a `&mut` call joins one -- with the live table written over
/// it: a key in all three sources, tombstones in each memtable cutting the
/// older ones, a delete followed by an append in one table and across the
/// two, keys below the first partition key, between existing keys, and past
/// the last. The same model then checks the merge after `settle` publishes
/// the level-0 pieces, and the walk alone after a flush.
#[test]
fn the_bulk_walk_lays_unsealed_keys_over_the_partitions_exactly_as_the_merge_does() {
    overlay_model("overlay", false, 0);
}

/// The same model against the block cache: a scan walks cached copies of
/// the blocks it crosses, and every write between the checks must drop
/// the copy of the block it lands in.
#[test]
fn the_block_cache_answers_the_same_model() {
    overlay_model("overlay-cache", true, 0);
}

/// Under a budget of a couple of blocks, every build sheds another, and
/// a scan finds blocks dropped since its last visit; the answers must not
/// change, and the bytes the cache counts must be the bytes it holds.
#[test]
fn the_block_cache_answers_the_same_model_under_a_budget() {
    overlay_model("overlay-budget", true, 2048);
}

/// The seal grows with the partitions: past the floor, a memtable seals
/// only once it holds a sixteenth of what a merge would rewrite. On a
/// store whose partitions are far larger than the floor, a commit past
/// the floor and under the grown threshold seals nothing, and with the
/// growth off the same commit seals.
#[test]
fn the_seal_grows_with_the_store() {
    for grows in [true, false] {
        let d = dir(if grows { "seal-grows" } else { "seal-fixed" });
        let opts = Options {
            seal_bytes: 64 << 10,
            partition_bytes: Some(256 << 10),
            seal_grows: grows,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let val = vec![7u8; 200];
        for k in 0..20_000u32 {
            db.append(format!("key-{k:06}").as_bytes(), &val);
            if k % 500 == 499 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        let (parts, _) = db.levels();
        assert!(parts > 1, "the load did not partition: {parts}");
        let floor = 64usize << 10;
        let threshold = db.seal_threshold();
        if grows {
            assert!(
                threshold > 4 * floor,
                "the threshold did not grow past the floor: {threshold}"
            );
        } else {
            assert_eq!(threshold, floor);
        }
        // Twice the floor, under the grown threshold. On the direct path a
        // seal of these ordered keys is a partition more, not a piece.
        let parts = db.levels().0;
        for k in 0..600u32 {
            db.append(format!("late-{k:06}").as_bytes(), &val);
        }
        db.commit().unwrap();
        let sealed = db.in_flight().0 || db.levels().1 > 0 || db.levels().0 > parts;
        assert_eq!(
            sealed, !grows,
            "seal_grows={grows}: a commit of twice the floor sealed={sealed}"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A key the frozen memtable holds and the live one then touches, with a
/// scan between the seal and the touch so the snapshot of unsealed keys
/// was built with the frozen entry and the live one arrives through the
/// side list: the two must fold into one key, a live tombstone cutting
/// the frozen values, a live append following them.
#[test]
fn a_live_write_over_a_frozen_key_folds_into_it_after_the_snapshot() {
    fold_model(true);
}

/// The same phases on the merge path, where the keys written since the
/// snapshot are filed into its side runs instead of by block: a burst
/// under the rebuild threshold is filed and folded, one over it rebuilds,
/// and a rehash under either renumbers the runs' slots.
#[test]
fn the_merge_path_files_the_keys_written_since_its_snapshot() {
    fold_model(false);
}

fn fold_model(block_cache: bool) {
    let d = dir(&format!("overlay-fold-{block_cache}"));
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        scan_block_cache: block_cache,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in (0..600).step_by(3) {
        m.append(&mut db, &key(k), &format!("p{k}"));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    assert!(db.levels().0 > 1);
    for k in [300, 303, 306, 600, 603] {
        m.append(&mut db, &key(k), "f");
    }
    // Two keys alone in a block far from the others: their block stays
    // sparse, so a later tombstone over them exercises the sparse form's
    // shadowing, not the dense copy's.
    m.append(&mut db, &key(30), "f");
    m.append(&mut db, &key(33), "f");
    db.commit().unwrap();
    db.seal().unwrap();
    assert!(db.in_flight().0);
    m.check(&db, "frozen over the partitions, snapshot built");
    held(&db, 0);
    // Live writes to frozen keys, each creating a live entry.
    m.delete(&mut db, &key(300));
    m.append(&mut db, &key(303), "l");
    m.delete(&mut db, &key(306));
    m.append(&mut db, &key(306), "l-after-delete");
    m.delete(&mut db, &key(600));
    m.append(&mut db, &key(603), "l");
    m.check(&db, "live over frozen, through the side list");
    held(&db, 0);
    // More keys than the side list may hold before a scan rebuilds the
    // snapshot: the rebuild happens with the tables standing, and the
    // writes after are filed under blocks whose bounds were walked again.
    // The blocks these land in hold hundreds of keys above the partition
    // from here on, and are walked as a merge rather than built.
    let burst = |k: u32| format!("key-00{:03}x{:02}", 100 + k % 500, k / 500);
    for k in 0..4200 {
        m.append(&mut db, &burst(k), "burst");
    }
    m.check(&db, "a burst that rebuilds the snapshot under the tables");
    held(&db, 0);
    assert!(
        !block_cache || db.block_cache_wide() > 0,
        "the burst's blocks are walked wide"
    );
    // Fewer than the snapshot holds, so no rebuild: the wide blocks take
    // these as filed keys and keep them in order.
    for k in 4200..6200 {
        m.append(&mut db, &burst(k), "burst");
    }
    m.check(&db, "a burst the wide blocks file");
    held(&db, 0);
    // Still under the rebuild threshold, and past the count the memtable
    // holds before it rehashes: every filed key's slot is renumbered,
    // in the wide blocks' orders and in the snapshot's side runs, and
    // the check reads through them.
    for k in 6200..8200 {
        m.append(&mut db, &burst(k), "burst");
    }
    m.check(&db, "a burst that rehashes under the filed keys");
    held(&db, 0);
    // Past the snapshot's count: the rebuild happens with wide blocks
    // standing that hold filed keys, whose order names slots the new
    // snapshot holds.
    for k in 8200..10600 {
        m.append(&mut db, &burst(k), "burst");
    }
    m.check(&db, "a burst that rebuilds the snapshot under wide blocks");
    held(&db, 0);
    // Keys filed into wide blocks after that rebuild: a wide block kept
    // across it would count them against the order it made before.
    for k in 10600..10640 {
        m.append(&mut db, &burst(k), "after-rebuild");
    }
    m.delete(&mut db, &burst(7));
    m.check(&db, "keys filed into wide blocks after the rebuild");
    held(&db, 0);
    assert!(
        !block_cache || db.block_cache_wide() > 0,
        "the wide blocks stand after the rebuild"
    );
    m.delete(&mut db, &key(303));
    m.append(&mut db, &key(309), "l-after-rebuild");
    m.delete(&mut db, "key-00150x");
    m.append(&mut db, "key-00151y", "between-after-rebuild");
    m.check(&db, "writes filed after the rebuild");
    held(&db, 0);
    db.settle().unwrap();
    m.check(&db, "pieces and live");
    held(&db, 0);
    m.delete(&mut db, &key(309));
    m.delete(&mut db, &key(30));
    m.append(&mut db, &key(33), "l-over-piece");
    m.append(&mut db, "key-00152y", "over pieces");
    m.check(&db, "writes filed over the pieces");
    held(&db, 0);
    db.flush().unwrap();
    m.flushed();
    // Keys created while their partition has no table yet: the flush
    // dropped every table, a scan that stays in the first partition makes
    // only that one's, the writes below land in the last partition's
    // range, and the check makes the last partition's table with those
    // keys already written.
    let mut n = 0usize;
    db.scan(key(0).as_bytes(), 1, |_, _| n += 1).unwrap();
    assert_eq!(n, 1);
    m.append(&mut db, "key-00590a", "before-the-table");
    m.delete(&mut db, &key(594));
    m.append(&mut db, &key(597), "before-the-table");
    m.append(&mut db, "key-00599z", "before-the-table");
    m.check(
        &db,
        "merged, with keys created before their partition's table",
    );
}

/// A write is settled into the built block it lands in, in place, as a
/// rebuild would have it: a clean block becomes sparse with the key's
/// merged run; a sparse block takes a new key at its place and an update
/// of a partition key as one entry standing for the partition's record;
/// a copy takes a new key at its place, an update in the key's run, and a
/// delete as an empty run. One partition of several blocks, since a
/// partition with nothing unsealed anywhere is walked in bulk and its
/// blocks never built: the first write makes the rest materialize clean,
/// and every later write finds a built block to patch. The model's scans
/// and reads hold each result to the merge.
#[test]
fn a_write_settles_into_the_block_it_lands_in() {
    let d = dir("settle-in-place");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 10),
        scan_block_cache: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in (0..1200).step_by(3) {
        m.append(&mut db, &key(k), &format!("p{k}"));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    assert_eq!(db.levels(), (1, 0), "one partition of several blocks");
    m.check(&db, "the partition, walked in bulk");
    // The first write: the partition is no longer clean throughout, and
    // the scans after it build every block, this one sparse from sources.
    m.append(&mut db, &key(301), "new");
    db.commit().unwrap();
    m.check(&db, "a new key in a partition that was clean throughout");
    // A clean block becomes sparse: a new key between two partition keys.
    m.append(&mut db, &key(1001), "new in a clean block");
    db.commit().unwrap();
    m.check(&db, "a new key settled into a clean block");
    // The same block, sparse now: an update of a partition key beside it,
    // one entry standing for the partition's record and the new value.
    m.delete(&mut db, &key(303));
    m.append(&mut db, &key(303), "updated");
    db.commit().unwrap();
    m.check(&db, "a partition key updated in a sparse block");
    // A clean block elsewhere, its first write an update of a partition
    // key: sparse now with one entry standing for the partition's record.
    m.delete(&mut db, &key(900));
    m.append(&mut db, &key(900), "updated clean");
    db.commit().unwrap();
    m.check(&db, "a partition key updated in a clean block");
    // A block made dense enough to be a copy: twenty new keys in it, then
    // the copy patched with a new key, an update and a delete.
    for k in (600..660).step_by(3) {
        m.append(&mut db, &key(k + 1), "dense");
    }
    db.commit().unwrap();
    let mut sink = 0usize;
    db.scan(key(600).as_bytes(), 40, |_k, v| sink += v.len())
        .unwrap();
    assert!(sink > 0);
    m.append(&mut db, &key(632), "inserted");
    m.delete(&mut db, &key(612));
    m.append(&mut db, &key(612), "updated");
    m.delete(&mut db, &key(615));
    db.commit().unwrap();
    m.check(
        &db,
        "a copy patched: a key inserted, one updated, one deleted",
    );
    // Reads and scans after a reopen see the same store: the cache held
    // nothing the WAL did not.
    drop(db);
    let db = Db::open(
        &d,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(64 << 10),
            scan_block_cache: true,
            ..Options::default()
        },
    )
    .unwrap();
    m.check(&db, "reopened");
}

/// A builder ahead of the reader: at the writer's first scan over a
/// state, and again at a commit once the memtable has grown by an eighth
/// since, the blocks a piece or an unsealed key overlays are built on a
/// thread from the partitions, the pieces and the memtables as of the
/// last commit, and installed at the store's next scan with the keys
/// written since that commit spliced in. A scan starts it, `settle`
/// waits for it, and the scan after finds every block of every partition
/// built; the model holds the installed forms to the merge, with keys
/// written after the builder's commit that only the store knew, and with
/// the store reopened between, so the second builder's forms are the
/// ones installed and the memtable it built from is the one the log
/// replayed and then grew.
#[test]
fn a_builder_ahead_of_the_reader_fills_the_cache() {
    let d = dir("build-ahead");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: true,
        scan_cache_ahead_min_blocks: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in (0..1200).step_by(3) {
        m.append(&mut db, &key(k), &format!("p{k}"));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    let parts = db.levels().0;
    assert!(parts > 1);
    // A piece over every range: every loaded key updated once and sealed;
    // `settle` waits for the builder the join started.
    for k in (0..1200).step_by(3) {
        m.append(&mut db, &key(k), "r1");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert_eq!(db.levels().1, parts, "a piece over every range");
    // The first scan over the published state starts the builder, over
    // the memtable as of the last commit; the keys written after it are
    // the builder's to lack and the install's to splice.
    let mut sink = 0usize;
    db.scan(key(0).as_bytes(), 3, |_k, v| sink += v.len())
        .unwrap();
    db.settle().unwrap();
    for k in (1..1200).step_by(50) {
        m.append(&mut db, &key(k), "live");
    }
    db.commit().unwrap();
    db.scan(key(0).as_bytes(), 3, |_k, v| sink += v.len())
        .unwrap();
    let (blocks, _) = db.block_cache_size();
    assert!(
        blocks >= parts,
        "every partition's blocks built ahead: {blocks} blocks over {parts} partitions after one scan of three"
    );
    m.check(
        &db,
        "installed forms with the keys since the builder's commit spliced in",
    );
    // More writes settle into the installed forms as into any built block.
    for k in (2..1200).step_by(70) {
        m.append(&mut db, &key(k), "later");
    }
    m.delete(&mut db, &key(600));
    db.commit().unwrap();
    m.check(&db, "writes settled into installed forms");
    // Reopened: the cache is empty, the memtable is the log's replay, and
    // the builder is gone. A scan starts one; a commit of more than a
    // thousand writes, an eighth of the memtable and more, starts another
    // over the memtable as of that commit, dropping the first's forms;
    // `settle` waits for it, with its forms held for the next scan.
    drop(db);
    let mut db = Db::open(
        &d,
        Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(2 << 10),
            l0_trigger: 64,
            scan_block_cache: true,
            scan_cache_ahead: true,
            scan_cache_ahead_min_blocks: 0,
            ..Options::default()
        },
    )
    .unwrap();
    db.scan(key(0).as_bytes(), 3, |_k, v| sink += v.len())
        .unwrap();
    let between = |k: u32| format!("key-{k:05}x");
    for k in 0..1100 {
        m.append(&mut db, &between(k), "m");
    }
    for k in (0..1200).step_by(9) {
        m.append(&mut db, &key(k), "m2");
    }
    m.delete(&mut db, &key(300));
    db.commit().unwrap();
    db.settle().unwrap();
    // Written after the builder's commit: keys its forms lack, that the
    // install splices in, among them a key it built and one it never saw.
    for k in (5..1200).step_by(40) {
        m.append(&mut db, &key(k), "since");
    }
    m.append(&mut db, &between(1100), "since");
    m.delete(&mut db, &key(9));
    m.delete(&mut db, &between(7));
    db.commit().unwrap();
    db.scan(key(0).as_bytes(), 3, |_k, v| sink += v.len())
        .unwrap();
    let (blocks, _) = db.block_cache_size();
    assert!(
        blocks >= parts,
        "every partition's blocks built ahead from the memtable: {blocks} blocks over {parts} partitions after one scan of three"
    );
    m.check(
        &db,
        "forms built from the memtable, with the keys since the builder's commit spliced in",
    );
    for k in (3..1200).step_by(90) {
        m.append(&mut db, &key(k), "later2");
    }
    db.commit().unwrap();
    m.check(&db, "writes settled into the second builder's forms");
    std::hint::black_box(sink);
}

/// A key updated in every seal is in every piece over its range, and it
/// is one key above the partition: a block whose every key sits in
/// sixteen pieces holds a block's worth, not sixteen, and is built rather
/// than walked as a merge of seventeen sources on every scan through it.
/// A block with three hundred keys past the end in one source is wide
/// still.
#[test]
fn a_key_held_by_many_pieces_counts_once_toward_a_wide_block() {
    let d = dir("overlay-pieces");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in (0..600).step_by(3) {
        m.append(&mut db, &key(k), &format!("p{k}"));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    let parts = db.levels().0;
    assert!(parts > 1);
    // Every loaded key again in each of sixteen seals, each joined:
    // sixteen pieces over every range, each holding every key of every
    // block, and the blocks here hold about twenty keys, so summed once
    // per piece every block would be over the wide threshold.
    for round in 0..16 {
        for k in (0..600).step_by(3) {
            m.append(&mut db, &key(k), &format!("r{round}"));
        }
        db.commit().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
    }
    assert_eq!(db.levels().1, 16 * parts, "sixteen pieces over every range");
    m.check(&db, "every key in eight pieces");
    held(&db, 0);
    assert_eq!(
        db.block_cache_wide(),
        0,
        "a key in sixteen pieces counted once per piece"
    );
    // Three hundred keys past the end, filed under the last block from
    // the live memtable alone: one source, and the block is wide.
    for k in 0..300 {
        m.append(&mut db, &format!("key-00600x{k:03}"), "tail");
    }
    m.check(&db, "three hundred keys past the end in one source");
    held(&db, 0);
    assert_eq!(db.block_cache_wide(), 1, "the tail's block is wide");
}

/// A read consults the level-0 pieces over its key, found by their
/// fences: every piece while one spans the ranges, and once every piece
/// is aligned to a partition, the run over the key's range alone, oldest
/// first after the partition and before the memtables. Keys in the first
/// range, middle ones and the last, through the values and the count,
/// with a tombstone sealed into a piece cutting the sources below it.
#[test]
fn a_read_consults_the_pieces_over_its_range() {
    let d = dir("read-route");
    // The spanning piece needs the first partitioning to happen under a
    // seal; on the direct path the first seal is already a partition.
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(1 << 10),
        l0_trigger: 5,
        ..wal_arm()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let key = |k: u32| format!("key-{k:05}");
    let val = |v: &str| format!("{v:<16}");
    let write = |db: &mut Db, v: &str| {
        for k in (0..600).step_by(3) {
            db.append(key(k).as_bytes(), val(v).as_bytes());
        }
        db.commit().unwrap();
    };
    let read = |db: &Db, k: u32| -> (Vec<String>, u64, u64) {
        let mut got = Vec::new();
        let n = db
            .read_all(key(k).as_bytes(), |v| {
                got.push(String::from_utf8(v.to_vec()).unwrap())
            })
            .unwrap();
        (got, n, db.count(key(k).as_bytes()).unwrap())
    };
    let want = |tags: &[&str]| tags.iter().map(|t| val(t)).collect::<Vec<_>>();
    let probe = [0, 3, 297, 300, 303, 594, 597];
    // Five seals reach the trigger when the sixth joins them, which starts
    // the first partitioning; the sixth is cut with no fence to split at,
    // so it spans every range the partitioning makes, and a read anywhere
    // must consult it.
    for round in 0..6 {
        write(&mut db, &format!("s{round}"));
        db.seal().unwrap();
    }
    db.settle().unwrap();
    let (parts, l0) = db.levels();
    assert!(
        parts >= 3 && l0 == 1,
        "partitions with one piece spanning them, got {parts} and {l0}"
    );
    assert!(!db.pieces_aligned());
    for k in probe {
        let (got, n, c) = read(&db, k);
        assert_eq!(got, want(&["s0", "s1", "s2", "s3", "s4", "s5"]), "key {k}");
        assert_eq!((n, c), (6, 6), "key {k}");
    }
    // A scan across the same store: the merge over the partitions and
    // the spanning piece, on both scan paths, must carry the piece's
    // values for keys in every range.
    for from in [0, 297, 591] {
        let mut got: Vec<(String, String)> = Vec::new();
        let n = db
            .scan(key(from).as_bytes(), 3, |k, v| {
                got.push((
                    String::from_utf8(k.to_vec()).unwrap(),
                    String::from_utf8(v.to_vec()).unwrap(),
                ))
            })
            .unwrap();
        assert_eq!(n, 3, "scan from {from}");
        let mut want_all: Vec<(String, String)> = Vec::new();
        for k in [from, from + 3, from + 6] {
            for t in ["s0", "s1", "s2", "s3", "s4", "s5"] {
                want_all.push((key(k), val(t)));
            }
        }
        assert_eq!(got, want_all, "scan from {from} over the spanning piece");
    }
    // Merged, then two seals split at the fences and joined: two aligned
    // pieces over every range, a frozen memtable under a seal not joined,
    // and live writes over all of it.
    db.flush().unwrap();
    let parts = db.levels().0;
    for round in 0..2 {
        write(&mut db, &format!("a{round}"));
        db.seal().unwrap();
        db.settle().unwrap();
    }
    assert_eq!(db.levels(), (parts, 2 * parts));
    assert!(db.pieces_aligned());
    write(&mut db, "f");
    db.seal().unwrap();
    assert!(db.in_flight().0);
    for k in (0..600).step_by(3) {
        db.append(key(k).as_bytes(), val("l").as_bytes());
    }
    let all = ["s0", "s1", "s2", "s3", "s4", "s5", "a0", "a1", "f", "l"];
    for k in probe {
        let (got, n, c) = read(&db, k);
        assert_eq!(got, want(&all), "key {k} over aligned pieces");
        assert_eq!((n, c), (10, 10), "key {k}");
    }
    // Tombstones: live first, then sealed with the live values into a
    // third piece over every range, where a read of a deleted key finds
    // the tombstone and skips every source below it, and a read of its
    // neighbour finds everything, the live value now in that piece.
    db.delete(key(3).as_bytes());
    db.delete(key(300).as_bytes());
    for k in [3, 300] {
        let (got, n, c) = read(&db, k);
        assert!(got.is_empty() && n == 0 && c == 0, "key {k} deleted live");
    }
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.pieces_aligned());
    assert!(
        db.levels().1 > 3 * parts,
        "the tombstones' seal made pieces"
    );
    for k in [3, 300] {
        let (got, n, c) = read(&db, k);
        assert!(
            got.is_empty() && n == 0 && c == 0,
            "key {k} deleted in a piece"
        );
    }
    for k in [0, 297, 303, 597] {
        let (got, n, c) = read(&db, k);
        assert_eq!(got, want(&all), "key {k} beside a deleted one");
        assert_eq!((n, c), (10, 10), "key {k}");
    }
}

/// A scan builds its snapshot of the unsealed keys naming the live
/// memtable's slots; a seal replaces that memtable with an empty one,
/// and the inserts after it rehash the new table at a few hundred keys,
/// renumbering every slot the snapshot names through a map the size of
/// the new table. The snapshot must not outlive the memtable it names:
/// thousands of keys in the old one, so their slots run past the new
/// map, then a seal, then enough inserts to rehash before any scan.
#[test]
fn a_seal_drops_the_scan_snapshot_before_the_next_rehash() {
    let d = dir("seal-snapshot");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 10),
        scan_block_cache: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    for k in 0..2100 {
        m.append(&mut db, &format!("key-{k:05}"), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    assert!(db.levels().0 > 1);
    // Written high to low: an ordered batch over an empty memtable would
    // go straight to a segment, and this test is about the memtable's seal.
    for k in (0..2100).rev() {
        m.append(&mut db, &format!("live-{k:05}"), "l");
    }
    db.commit().unwrap();
    m.check(&db, "a snapshot over thousands of live keys");
    db.seal().unwrap();
    assert!(db.in_flight().0);
    for k in 0..600 {
        m.append(&mut db, &format!("new-{k:05}"), "n");
    }
    m.check(&db, "inserts past a rehash of the memtable the seal made");
    db.settle().unwrap();
    m.check(&db, "the seal joined");
}

/// A scan starts at the first partition that may reach its start, found
/// by a gallop from the front: over enough partitions for the gallop to
/// double past them several times, every scan from every sampled start,
/// on both scan paths, against the model, with unsealed keys in the
/// first range and the last so the start's partition is not always the
/// one the keys are in.
#[test]
fn a_scan_starts_at_the_first_partition_that_reaches_it() {
    for block_cache in [false, true] {
        let d = dir(&format!("scan-start-{block_cache}"));
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1 << 10),
            scan_block_cache: block_cache,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let mut m = ScanModel::default();
        for k in 0..3000 {
            m.append(&mut db, &format!("key-{k:05}"), "p");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        m.flushed();
        assert!(db.levels().0 > 40, "partitions: {}", db.levels().0);
        m.append(&mut db, "key-00001x", "l");
        m.append(&mut db, "key-02999x", "l");
        m.check(
            &db,
            &format!("scans over many partitions, cache {block_cache}"),
        );
    }
}

/// The ordered index's seek brackets its search with a top level of every
/// sixty-fourth head: over a partition of tens of thousands of keys the
/// scans from sampled starts, at keys and between them, must land where
/// the model says on both scan paths, across hundreds of brackets and
/// their edges. Live keys between and past the loaded ones so the seek's
/// answer is laid over, not only read.
#[test]
fn a_seek_brackets_its_search_in_the_top_level() {
    for block_cache in [false, true] {
        let d = dir(&format!("seek-top-{block_cache}"));
        let opts = Options {
            seal_bytes: 8 << 20,
            partition_bytes: Some(64 << 20),
            scan_block_cache: block_cache,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let mut m = ScanModel::default();
        for k in 0..50_000u32 {
            m.append(&mut db, &format!("key-{:06}", k * 2), "p");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        m.flushed();
        assert_eq!(db.levels().0, 1, "one partition of fifty thousand keys");
        for k in (1..50_000u32).step_by(997) {
            m.append(&mut db, &format!("key-{:06}", k * 2 - 1), "l");
        }
        m.append(&mut db, "key-100001", "l");
        m.check(
            &db,
            &format!("seeks over a large partition, cache {block_cache}"),
        );
        // Starts exactly on a sample's rank, just below one and just past
        // one, which the check's sampling never lands on: rank r is the
        // loaded key 2r.
        let visited = m.visited();
        for r in (64..50_000usize).step_by(64 * 3) {
            for from in [
                format!("key-{:06}", r * 2),
                format!("key-{:06}", r * 2 - 1),
                format!("key-{:06}", r * 2 + 2),
            ] {
                let want: Vec<(Vec<u8>, Vec<u8>)> = visited
                    .iter()
                    .filter(|k| **k >= from.as_bytes())
                    .take(3)
                    .flat_map(|k| m.vals[*k].iter().map(move |v| (k.to_vec(), v.clone())))
                    .collect();
                let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                db.scan(from.as_bytes(), 3, |k, v| {
                    got.push((k.to_vec(), v.to_vec()))
                })
                .unwrap();
                assert_eq!(got, want, "scan from {from} at a sample's edge");
            }
        }
    }
}

/// Ordered ingest goes straight into a segment: a load of keys in order,
/// batch by batch, leaves the WAL holding nothing but its header, the
/// store readable and scannable throughout against the model, and the
/// segments closing at the partition size as partitions whose fences
/// tile the space. A batch that is not in order under an open segment
/// closes it and goes through the WAL whole, and the store reopens to
/// the same model, so the batch's frames replay and the segment's do not.
#[test]
fn an_ordered_load_goes_straight_into_segments() {
    for block_cache in [false, true] {
        let d = dir(&format!("direct-{block_cache}"));
        // A direct segment closes at the seal threshold and joins whole,
        // so the seal is the small size here and the partition the larger.
        let opts = Options {
            seal_bytes: 32 << 10,
            partition_bytes: Some(256 << 10),
            scan_block_cache: block_cache,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts.clone()).unwrap();
        let mut m = ScanModel::default();
        let key = |k: u32| format!("key-{k:06}");
        let val = |k: u32| format!("value-{k:06}-{:>20}", "");
        for batch in 0..60u32 {
            for k in batch * 100..(batch + 1) * 100 {
                m.append(&mut db, &key(k), &val(k));
            }
            db.commit().unwrap();
            if batch % 30 == 29 {
                m.check(&db, &format!("under the direct segment, batch {batch}"));
            }
        }
        // Nothing but the WAL's eight-byte header.
        let (_, _, written) = db.wal_durable();
        assert!(
            written <= 8,
            "the WAL took {written} bytes under an ordered load"
        );
        // The segments closed at the seal threshold, and joined as a
        // seal's pieces do: promoted to partitions by rename once their
        // range held enough of them, the rest still pieces over it.
        db.settle().unwrap();
        let (parts, l0) = db.levels();
        assert!(
            parts >= 4 && parts + l0 >= 6,
            "segments as the threshold was crossed: {parts} partitions, {l0} pieces"
        );
        assert!(db.pieces_aligned() || l0 == 0);
        // One more ordered batch, so a segment is open for what follows.
        for k in 10000..10100u32 {
            m.append(&mut db, &key(k), &val(k));
        }
        db.commit().unwrap();
        assert!(db.wal_durable().2 <= 8);
        let (parts, l0) = db.levels();
        // Not in order: inserts past the end, then an update of a loaded
        // key. The inserts went into the run; at the update the run's
        // committed batches close as a segment and the inserts move to
        // the memtable with the WAL frames they never had, in order, and
        // the batch goes whole through the WAL -- as do the ordered
        // batches after it, since the memtable is no longer empty.
        for k in 10100..10150u32 {
            m.append(&mut db, &key(k), &val(k));
        }
        m.check(&db, "ordered inserts staged in the run");
        m.delete(&mut db, &key(7));
        m.append(&mut db, &key(7), "updated");
        m.check(&db, "a mixed batch staged, the segment closing under it");
        db.commit().unwrap();
        m.check(&db, "a mixed batch under the segment");
        db.settle().unwrap();
        let (parts_after, l0_after) = db.levels();
        assert!(
            parts_after > parts || l0_after > l0,
            "the segment closed and joined: {parts}+{l0} -> {parts_after}+{l0_after}"
        );
        m.check(&db, "the closed segment joined");
        assert!(
            db.wal_durable().2 > 8,
            "the mixed batch went through the WAL"
        );
        let before = db.wal_durable().2;
        for k in 10150..10300u32 {
            m.append(&mut db, &key(k), &val(k));
        }
        db.commit().unwrap();
        assert!(
            db.wal_durable().2 > before,
            "through the WAL, the memtable not empty"
        );
        m.check(&db, "ordered batches after, through the WAL");
        // Reopened: the partitions from the manifest, the mixed batch and
        // what followed from the WAL, the same model.
        drop(db);
        let mut db = Db::open(&d, opts.clone()).unwrap();
        m.check(&db, "reopened");
        db.flush().unwrap();
        m.flushed();
        m.check(&db, "flushed");
        // Empty again after the flush: the next ordered batch goes direct.
        for k in 11000..11100u32 {
            m.append(&mut db, &key(k), &val(k));
        }
        let before = db.wal_durable().2;
        db.commit().unwrap();
        assert_eq!(
            db.wal_durable().2,
            before,
            "direct again over an empty memtable"
        );
        m.check(&db, "direct again");
        db.flush().unwrap();
        m.flushed();
        // A run forming with nothing committed yet, left the same way: the
        // inserts move to the memtable, and there is no segment to close.
        for k in 12000..12050u32 {
            m.append(&mut db, &key(k), &val(k));
        }
        m.delete(&mut db, &key(9));
        db.commit().unwrap();
        m.check(&db, "a forming run left mid-batch");
        drop(db);
        let db = Db::open(&d, opts.clone()).unwrap();
        m.check(&db, "reopened: the moved inserts from the WAL");
        // A close flushes, and the flush's merge reclaims the tombstone.
        db.close().unwrap();
        m.flushed();
        let db = Db::open(&d, opts).unwrap();
        m.check(&db, "reopened after close");
    }
}

/// A direct segment's crash windows, emulated on the file a dropped store
/// leaves: batches after the last commit marker are lost whole, whatever
/// bytes follow it; a marker torn in half loses its batch; and a temp file
/// whose id the manifest already names is a close that published and never
/// unlinked, removed rather than recovered twice.
#[test]
fn a_direct_segment_recovers_to_its_last_commit_marker() {
    let key = |k: u32| format!("key-{k:06}");
    let load = |d: &std::path::Path| -> (Db, ScanModel) {
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1 << 20),
            ..Options::default()
        };
        let mut db = Db::create(d, opts).unwrap();
        let mut m = ScanModel::default();
        for batch in 0..3u32 {
            for k in batch * 100..(batch + 1) * 100 {
                m.append(&mut db, &key(k), "v");
            }
            db.commit().unwrap();
        }
        (db, m)
    };
    let tmp_of = |d: &std::path::Path| d.join("direct-00000000.tmp");
    // Bytes after the last marker: a batch that never committed.
    {
        let d = dir("direct-torn-tail");
        let (db, m) = load(&d);
        std::mem::forget(db);
        let tmp = tmp_of(&d);
        assert!(tmp.exists(), "the open segment's temp file");
        let mut f = std::fs::OpenOptions::new().append(true).open(&tmp).unwrap();
        std::io::Write::write_all(&mut f, &[0x2a; 777]).unwrap();
        drop(f);
        let db = Db::open(&d, Options::default()).unwrap();
        assert!(!tmp.exists(), "recovered and removed");
        m.check(
            &db,
            "three batches, the garbage after the last marker ignored",
        );
    }
    // The last marker torn: its batch is gone whole, the two before stand.
    {
        let d = dir("direct-torn-marker");
        let (db, mut m) = load(&d);
        std::mem::forget(db);
        let tmp = tmp_of(&d);
        let len = std::fs::metadata(&tmp).unwrap().len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&tmp)
            .unwrap()
            .set_len(len - 8)
            .unwrap();
        for k in 200..300u32 {
            m.vals.remove(key(k).as_bytes());
            m.touched.remove(key(k).as_bytes());
        }
        let mut db = Db::open(&d, Options::default()).unwrap();
        m.check(&db, "two batches");
        for k in 200..300u32 {
            assert!(
                read_vec(&db, key(k).as_bytes()).is_empty(),
                "key {k} lost whole"
            );
        }
        // The recovered piece merges like any other.
        db.flush().unwrap();
        m.flushed();
        m.check(&db, "two batches, merged");
    }
    // The marker's page on disk and not a page of its batch: one byte of
    // the last batch's last key flipped, so the record still parses and
    // the marker after it is whole and counts the batch correctly. The
    // batch is gone whole; a marker vouches for its bytes, not for its
    // own presence.
    {
        let d = dir("direct-lost-page");
        let (db, mut m) = load(&d);
        std::mem::forget(db);
        let tmp = tmp_of(&d);
        let mut bytes = std::fs::read(&tmp).unwrap();
        let last = key(299);
        let at = bytes
            .windows(last.len())
            .rposition(|w| w == last.as_bytes())
            .expect("the last key in the stream");
        bytes[at + last.len() - 1] ^= 0x40;
        std::fs::write(&tmp, &bytes).unwrap();
        for k in 200..300u32 {
            m.vals.remove(key(k).as_bytes());
            m.touched.remove(key(k).as_bytes());
        }
        let db = Db::open(&d, Options::default()).unwrap();
        m.check(
            &db,
            "two batches, the third's record damaged under its marker",
        );
        assert!(
            read_vec(&db, key(299).as_bytes()).is_empty(),
            "the damaged key"
        );
        assert!(
            read_vec(&db, key(200).as_bytes()).is_empty(),
            "its batch, whole"
        );
    }
    // A close that published and never unlinked its temp file.
    {
        let d = dir("direct-stale-tmp");
        let (mut db, mut m) = load(&d);
        db.flush().unwrap();
        m.flushed();
        let parts = db.levels().0;
        let par = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("par-00000000-"))
            .expect("the closed segment as a partition");
        drop(db);
        std::fs::copy(d.join(&par), tmp_of(&d)).unwrap();
        let db = Db::open(&d, Options::default()).unwrap();
        assert!(!tmp_of(&d).exists());
        assert_eq!(db.levels(), (parts, 0), "not recovered twice");
        m.check(&db, "the stale temp file removed");
    }
}

/// A value above the inline size goes to a block, which a direct segment
/// holds in memory until it closes, so a batch carrying one goes through
/// the WAL: durable at its commit and back after a crash. Beside it the
/// same keys with values one byte shorter, which do fit, to show which
/// path each takes.
#[test]
fn a_value_the_record_cannot_hold_sends_its_batch_through_the_wal() {
    let key = |k: u32| format!("key-{k:06}");
    for (name, big) in [("direct-fits", false), ("direct-block", true)] {
        let d = dir(name);
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(1 << 20),
            ..Options::default()
        };
        let val = "x".repeat(opts.inline_bytes + usize::from(big));
        let mut db = Db::create(&d, opts).unwrap();
        let mut m = ScanModel::default();
        for batch in 0..3u32 {
            for k in batch * 100..(batch + 1) * 100 {
                m.append(&mut db, &key(k), &val);
            }
            db.commit().unwrap();
        }
        let (_, _, written) = db.wal_durable();
        let tmp = d.join("direct-00000000.tmp");
        if big {
            assert!(written > 8, "{name}: through the WAL");
            assert!(!tmp.exists(), "{name}: no segment opened");
        } else {
            assert!(written <= 8, "{name}: the WAL untouched");
            assert!(tmp.exists(), "{name}: a segment open");
        }
        std::mem::forget(db);
        let db = Db::open(&d, Options::default()).unwrap();
        m.check(&db, &format!("{name}: every batch back after a crash"));
    }
}

/// After a check has filled the cache: the bytes it counts against a
/// walk of what it holds, and against the budget with one block's slack,
/// since the block a scan is about to walk is never shed.
fn held(db: &Db, budget: usize) {
    let (_, walked) = db.block_cache_size();
    assert_eq!(
        walked,
        db.block_cache_bytes(),
        "the cache miscounts what it holds"
    );
    if budget > 0 {
        let largest = db.block_cache_largest();
        assert!(
            walked <= budget + largest,
            "the cache holds {walked} B against a budget of {budget} and a largest block of {largest}"
        );
    }
}

fn overlay_model(name: &str, block_cache: bool, budget: usize) {
    let d = dir(name);
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        scan_block_cache: block_cache,
        scan_cache_bytes: budget,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in (0..600).step_by(3) {
        m.append(&mut db, &key(k), &format!("p{k}"));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    let (parts, l0) = db.levels();
    assert!(
        parts > 1 && l0 == 0,
        "want several partitions and no piece, got {parts}/{l0}"
    );
    m.check(&db, "partitions only");
    held(&db, budget);

    // A few writes in one partition's range: an update, a delete, a new
    // key between two, so a block holds unsealed keys without being dense
    // with them.
    m.append(&mut db, &key(300), "s");
    m.delete(&mut db, &key(306));
    m.append(&mut db, &key(310), "s-between");
    db.commit().unwrap();
    m.check(&db, "a few unsealed keys in one partition");
    held(&db, budget);

    // The YCSB shape: inserts past the end, committed, nothing sealed.
    for k in (600..660).step_by(3) {
        m.append(&mut db, &key(k), &format!("e{k}"));
    }
    db.commit().unwrap();
    assert_eq!(db.levels(), (parts, 0));
    m.check(&db, "live inserts past the end");
    held(&db, budget);

    // What the frozen memtable will hold.
    for k in (0..600).step_by(3) {
        match k % 10 {
            1 => m.append(&mut db, &key(k), "f"),
            2 => m.append(&mut db, &key(k + 1), "f-between"),
            4 => m.delete(&mut db, &key(k)),
            7 => {
                m.delete(&mut db, &key(k));
                m.append(&mut db, &key(k), "f-after-delete");
            }
            _ => {}
        }
    }
    m.delete(&mut db, &key(1)); // a key no source holds
    m.append(&mut db, "kex-below", "f-below");
    db.commit().unwrap();
    db.seal().unwrap();
    assert!(
        db.in_flight().0,
        "the seal is not joined until a &mut call joins it"
    );
    assert_eq!(
        db.levels(),
        (parts, 0),
        "an unjoined seal publishes no piece"
    );

    // The live memtable over it, uncommitted so nothing joins the seal.
    for k in (0..600).step_by(3) {
        match k % 10 {
            0 => {
                m.delete(&mut db, &key(k));
                m.append(&mut db, &key(k), "l-after-delete");
            }
            1 => m.append(&mut db, &key(k), "l"),
            4 => m.append(&mut db, &key(k), "l-after-frozen-delete"),
            5 => m.append(&mut db, &key(k + 2), "l-between"),
            7 => m.delete(&mut db, &key(k)),
            _ => {}
        }
    }
    m.append(&mut db, &key(600), "l-on-frozen-insert");
    m.append(&mut db, "kex-below", "l-below");
    m.append(&mut db, "kez-above", "l-above");
    assert!(db.in_flight().0);
    assert_eq!(db.levels(), (parts, 0));
    m.check(&db, "frozen and live over the partitions");
    held(&db, budget);

    db.settle().unwrap();
    assert!(db.levels().1 > 0, "settle publishes the sealed pieces");
    m.check(&db, "level-0 pieces and live over the partitions");
    held(&db, budget);

    db.flush().unwrap();
    m.flushed();
    assert_eq!(db.levels().1, 0);
    m.check(&db, "partitions only, after the merge");
    held(&db, budget);

    // And the YCSB shape once more on the merged store.
    for k in (660..700).step_by(3) {
        m.append(&mut db, &key(k), &format!("e{k}"));
    }
    db.commit().unwrap();
    assert_eq!(db.levels().1, 0);
    m.check(&db, "live inserts past the end, after the merge");
    held(&db, budget);
    db.close().unwrap();
}

/// What a reader handle on another thread asks of the store, and what it
/// answers with.
enum Ask {
    Read(Vec<u8>),
    Scan(Vec<u8>, usize),
    Isolation(Isolation),
    Snapshot,
    Release,
    Stop,
}

fn serve(
    r: Reader,
    asks: std::sync::mpsc::Receiver<Ask>,
    answers: std::sync::mpsc::Sender<Vec<Vec<u8>>>,
) {
    for ask in asks {
        match ask {
            Ask::Read(k) => answers.send(read_vec(&r, &k)).unwrap(),
            Ask::Scan(from, n) => {
                let mut out = Vec::new();
                r.scan(&from, n, |k, v| out.push([k, b":", v].concat()))
                    .unwrap();
                answers.send(out).unwrap();
            }
            Ask::Isolation(i) => {
                r.set_isolation(i);
                answers.send(Vec::new()).unwrap();
            }
            Ask::Snapshot => {
                r.snapshot();
                answers.send(Vec::new()).unwrap();
            }
            Ask::Release => {
                r.release();
                answers.send(Vec::new()).unwrap();
            }
            Ask::Stop => {
                answers.send(Vec::new()).unwrap();
                break;
            }
        }
    }
}

/// A reader handle on another thread sees each commit as the writer makes
/// it and nothing of the batch the writer is still staging; a dirty one
/// sees the batch; a snapshot holds its view across commits, a seal and a
/// merge until it is released, and the states it held are freed once it
/// lets go. The writer never waits for any of them.
#[test]
fn a_reader_handle_sees_what_its_isolation_says() {
    let d = dir("reader-isolation");
    let opts = Options {
        seal_bytes: 64 << 10,
        partition_bytes: Some(128 << 10),
        l0_trigger: 2,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    for k in 0..2000u32 {
        db.append(&key(k), b"v0");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    let (ask, asks) = std::sync::mpsc::channel();
    let (answer, answers) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || serve(r, asks, answer));
    let read = |k: u32| -> Vec<Vec<u8>> {
        ask.send(Ask::Read(key(k))).unwrap();
        answers.recv().unwrap()
    };
    let tell = |a: Ask| {
        ask.send(a).unwrap();
        answers.recv().unwrap();
    };
    assert_eq!(
        read(7),
        vec![b"v0".to_vec()],
        "a committed value, from the other thread"
    );
    // A batch staged and not committed: latest reads stop at the commit,
    // dirty reads see the batch.
    db.put(&key(7), b"v1");
    db.append(&key(2500), b"new");
    assert_eq!(
        read(7),
        vec![b"v0".to_vec()],
        "latest: the staged put is not there"
    );
    assert_eq!(
        read(2500),
        Vec::<Vec<u8>>::new(),
        "latest: the staged key is not there"
    );
    tell(Ask::Isolation(Isolation::Dirty));
    assert_eq!(
        read(7),
        vec![b"v1".to_vec()],
        "dirty: the staged put is there"
    );
    assert_eq!(
        read(2500),
        vec![b"new".to_vec()],
        "dirty: the staged key is there"
    );
    tell(Ask::Isolation(Isolation::Latest));
    db.commit().unwrap();
    assert_eq!(read(7), vec![b"v1".to_vec()], "latest, after the commit");
    // A snapshot holds through commits, a seal that moves the memtable
    // into a segment, and the merge that follows.
    tell(Ask::Snapshot);
    for k in 0..2000u32 {
        db.put(&key(k), b"v2");
    }
    db.commit().unwrap();
    assert_eq!(
        read(7),
        vec![b"v1".to_vec()],
        "snapshot: the commit after it is invisible"
    );
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(
        db.retired_states() >= 1,
        "the state the snapshot holds is kept"
    );
    assert_eq!(
        read(7),
        vec![b"v1".to_vec()],
        "snapshot: the seal after it is invisible"
    );
    assert_eq!(
        read(1999),
        vec![b"v0".to_vec()],
        "snapshot: a key the puts after it changed"
    );
    ask.send(Ask::Scan(key(5), 3)).unwrap();
    let got = answers.recv().unwrap();
    assert_eq!(
        got,
        vec![
            [key(5), b":v0".to_vec()].concat(),
            [key(6), b":v0".to_vec()].concat(),
            [key(7), b":v1".to_vec()].concat()
        ],
        "snapshot: a scan sees the view it took"
    );
    tell(Ask::Release);
    assert_eq!(read(7), vec![b"v2".to_vec()], "released: the latest commit");
    // Another publish, and the states the snapshot held are freed: nothing
    // is pinned before them.
    db.append(&key(3000), b"x");
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert_eq!(db.retired_states(), 0, "no reader pins the retired states");
    tell(Ask::Stop);
    t.join().unwrap();
}

/// Reader handles on their own threads keep answering while the writer
/// puts, seals, promotes and merges under them: every value read is one
/// the writer wrote for that key, a key's version never goes backwards
/// for one reader, and a scan comes back in key order with each value
/// under its key.
#[test]
fn readers_on_threads_keep_answering_through_seals_and_merges() {
    let d = dir("readers-threads");
    let opts = Options {
        seal_bytes: 32 << 10,
        partition_bytes: Some(64 << 10),
        l0_trigger: 2,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 3000u32;
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    for k in 0..keys {
        db.append(&key(k), b"0");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut threads = Vec::new();
    for t in 0..3u64 {
        let r = db.reader().unwrap();
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            let mut seen: HashMap<u32, u64> = HashMap::new();
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ t;
            let mut reads = 0usize;
            let mut scans = 0usize;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let k = (x % keys as u64) as u32;
                if x.is_multiple_of(8) {
                    let mut last: Option<Vec<u8>> = None;
                    r.scan(&key(k), 20, |kk, v| {
                        if let Some(l) = &last {
                            assert!(l.as_slice() < kk, "a scan out of key order");
                        }
                        last = Some(kk.to_vec());
                        let s = std::str::from_utf8(v).unwrap();
                        assert!(
                            s.parse::<u64>().is_ok(),
                            "a scanned value that is no version: {s}"
                        );
                    })
                    .unwrap();
                    scans += 1;
                } else {
                    let got = read_vec(&r, &key(k));
                    assert!(got.len() <= 1, "a put key with two values");
                    if let Some(v) = got.first() {
                        let ver: u64 = std::str::from_utf8(v).unwrap().parse().unwrap();
                        let prev = seen.entry(k).or_insert(0);
                        assert!(
                            ver >= *prev,
                            "a version that went backwards: key {k} read {ver} after {prev}"
                        );
                        *prev = ver;
                    }
                    reads += 1;
                }
            }
            (reads, scans)
        }));
    }
    let mut x = 42u64;
    for round in 1..=300u64 {
        for _ in 0..50 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = (x % keys as u64) as u32;
            db.put(&key(k), round.to_string().as_bytes());
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut total = (0usize, 0usize);
    for t in threads {
        let (r, s) = t.join().unwrap();
        total.0 += r;
        total.1 += s;
    }
    assert!(
        total.0 > 1000 && total.1 > 100,
        "the readers read: {total:?}"
    );
    assert!(
        db.levels().0 > 1,
        "the writer partitioned under them: {:?}",
        db.levels()
    );
    // Every key's last version, from the writer's own handle and from a
    // fresh reader, agree.
    let r = db.reader().unwrap();
    for k in (0..keys).step_by(97) {
        assert_eq!(read_vec(&db, &key(k)), read_vec(&r, &key(k)), "key {k}");
    }
}

/// The reader table has a slot for each handle and no more: the handle
/// past the last slot is refused, and a dropped handle's slot is free
/// again.
#[test]
fn the_reader_table_has_a_slot_for_each_handle() {
    let d = dir("reader-slots");
    let db = Db::create(&d, Options::default()).unwrap();
    let mut held = Vec::new();
    while let Ok(r) = db.reader() {
        held.push(r);
    }
    assert_eq!(held.len(), 256, "the table's slots, all claimed");
    assert!(db.reader().is_err(), "one more is refused");
    held.pop();
    assert!(db.reader().is_ok(), "a dropped handle's slot is free");
}

/// What a reopen replays from the WAL was committed before the crash, and
/// a reader handle under `Latest` sees it from the open on: the replayed
/// memtable carries the commit watermark, not zero until the writer's
/// first commit after. Found by the builder ahead of the reader, which
/// reads a reopened store at that watermark and built as if the memtable
/// were empty.
#[test]
fn a_reader_sees_the_replayed_writes_from_the_open_on() {
    let d = dir("reopen-watermark");
    let mut db = Db::create(&d, Options::default()).unwrap();
    for k in 0..300u32 {
        db.append(format!("key-{k:05}").as_bytes(), b"v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    for k in (0..300u32).step_by(7) {
        db.append(format!("key-{k:05}").as_bytes(), b"v1");
    }
    db.delete(b"key-00001");
    db.commit().unwrap();
    drop(db);
    let db = Db::open(&d, Options::default()).unwrap();
    let reader = db.reader().unwrap();
    assert_eq!(reader.isolation(), Isolation::Latest);
    assert_eq!(
        read_vec(&reader, b"key-00007"),
        vec![b"v0".to_vec(), b"v1".to_vec()]
    );
    assert_eq!(read_vec(&reader, b"key-00001"), Vec::<Vec<u8>>::new());
    assert_eq!(reader.count(b"key-00014").unwrap(), 2);
    let mut n = 0;
    reader
        .scan(b"key-00006", 3, |k, v| {
            if k == b"key-00007" && v == b"v1" {
                n += 1;
            }
        })
        .unwrap();
    assert_eq!(n, 1, "the replayed append in a scan");
    reader.snapshot();
    assert_eq!(
        read_vec(&reader, b"key-00021"),
        vec![b"v0".to_vec(), b"v1".to_vec()]
    );
    reader.release();
}

/// A handle under `Latest` reads the write log as far as it goes, past the
/// last commit, and settles a key it finds there into the block the key
/// falls in under the committed watermark: the uncommitted value is left
/// out, as it must be. When the commit lands, the log has not moved, so
/// the handle's next scan must still find the value the commit made
/// visible, in the block it settled and in the run of a block it builds
/// after.
#[test]
fn a_reader_under_latest_sees_the_commit_of_a_key_it_settled_uncommitted() {
    let d = dir("latest-commit-after-settle");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..2000u32 {
        db.append(key(k).as_bytes(), b"v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(db.levels().0 > 1);
    let reader = db.reader().unwrap();
    let scan_one = |r: &supdb::Reader, k: u32| -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let want = key(k);
        r.scan(want.as_bytes(), 1, |kk, v| {
            if kk == want.as_bytes() {
                out.push(v.to_vec());
            }
        })
        .unwrap();
        out
    };
    // Built clean through the handle, then a write the handle settles
    // while it is uncommitted.
    assert_eq!(scan_one(&reader, 100), vec![b"v0".to_vec()]);
    assert_eq!(scan_one(&reader, 1500), vec![b"v0".to_vec()]);
    db.append(key(100).as_bytes(), b"v1");
    db.append(key(1500).as_bytes(), b"v1");
    assert_eq!(
        scan_one(&reader, 100),
        vec![b"v0".to_vec()],
        "uncommitted, so unseen under Latest"
    );
    db.commit().unwrap();
    assert_eq!(
        scan_one(&reader, 100),
        vec![b"v0".to_vec(), b"v1".to_vec()],
        "the commit of a key the handle settled while uncommitted"
    );
    assert_eq!(
        scan_one(&reader, 1500),
        vec![b"v0".to_vec(), b"v1".to_vec()],
        "the commit of a key the handle settled while uncommitted, in a block it had not walked since"
    );
    assert_eq!(
        read_vec(&reader, key(100).as_bytes()),
        vec![b"v0".to_vec(), b"v1".to_vec()]
    );
    // The same through a block the handle builds after the commit.
    db.append(key(700).as_bytes(), b"v1");
    db.commit().unwrap();
    assert_eq!(
        scan_one(&reader, 700),
        vec![b"v0".to_vec(), b"v1".to_vec()],
        "a block built after the commit"
    );
}

/// A store's first flush seals its partition directly: one publish, the
/// file under the partition's name, no piece to promote -- through the
/// direct segment an ordered run takes and through the memtable a
/// shuffled one takes. A piece with a tombstone cannot be a partition as
/// it is, so that flush publishes twice and still leaves one partition.
#[test]
fn a_first_flush_seals_the_partition_in_one_publish() {
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    let files = |d: &std::path::Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".sup"))
            .collect();
        v.sort();
        v
    };
    for (name, shuffled) in [
        ("first-partition-ordered", false),
        ("first-partition-shuffled", true),
    ] {
        let d = dir(name);
        let mut db = Db::create(&d, Options::default()).unwrap();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for i in 0..3000u32 {
            let k = if shuffled {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x % 3000) as u32
            } else {
                i
            };
            db.append(&key(k), b"v");
            if i % 100 == 99 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        let before = db.seal_waits().publishes;
        db.flush().unwrap();
        assert_eq!(db.levels(), (1, 0), "{name}: one partition, no piece");
        assert_eq!(
            db.seal_waits().publishes - before,
            1,
            "{name}: the flush published once"
        );
        let names = files(&d);
        assert_eq!(names.len(), 1, "{name}: {names:?}");
        assert!(
            names[0].starts_with("par-") && names[0].ends_with("--.sup"),
            "{name}: the file carries the partition's name with empty fences: {names:?}"
        );
        assert_eq!(read_vec(&db, &key(7)), vec![b"v".to_vec()]);
        let mut n = 0usize;
        db.scan(&key(0), 5000, |_k, _v| n += 1).unwrap();
        assert_eq!(n, 3000, "{name}");
    }
    let d = dir("first-partition-tombstone");
    let mut db = Db::create(&d, Options::default()).unwrap();
    for i in 0..3000u32 {
        db.append(&key(i), b"v");
    }
    db.delete(&key(5));
    db.commit().unwrap();
    let before = db.seal_waits().publishes;
    db.flush().unwrap();
    assert_eq!(db.levels(), (1, 0));
    assert!(
        db.seal_waits().publishes - before >= 2,
        "a piece with a tombstone is not a partition as it is"
    );
    assert_eq!(read_vec(&db, &key(5)), Vec::<Vec<u8>>::new());
    // A store that does not partition on flush keeps its piece a piece:
    // the seal names only what the flush would have promoted.
    let d = dir("first-partition-unpartitioned");
    let mut db = Db::create(
        &d,
        Options {
            partition_on_flush: false,
            ..Options::default()
        },
    )
    .unwrap();
    for i in 0..3000u32 {
        db.append(&key(i), b"v");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    assert_eq!(
        db.levels(),
        (0, 1),
        "no partition where the flush makes none"
    );
    let names = files(&d);
    assert!(
        names.len() == 1 && names[0].starts_with("seg-"),
        "the piece keeps its name: {names:?}"
    );
    assert_eq!(read_vec(&db, &key(7)), vec![b"v".to_vec()]);
}

/// The range-read structure written at ingest: with `commit_forms` the
/// writer keeps a canonical form of every overlaid block current at each
/// commit, and a reader handle under `Latest` walks those forms instead
/// of building its own. Held to the model through a commit, a staged
/// batch, a seal, a merge and a delete, with a reader taking the forms,
/// a second reader pinned behind them, and the writer reading its own.
#[test]
fn a_reader_walks_the_forms_the_writer_maintains_at_commit() {
    let d = dir("commit-forms");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        commit_forms: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..1500u32 {
        m.append(&mut db, &key(k), "v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert!(db.levels().0 > 1, "several partitions");
    // Overlay keys in most blocks, and the commit that maintains them.
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    m.delete(&mut db, &key(77));
    db.commit().unwrap();
    let r = db.reader().unwrap();
    let mut sink = 0usize;
    // The first scan is what tells the store it is range-read; nothing
    // was maintained before it, so this one builds for itself.
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(
        db.canonical_forms().0,
        0,
        "nothing maintained before a scan"
    );
    for k in (1..1500u32).step_by(13) {
        m.append(&mut db, &key(k), "v1b");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    let (forms, bytes, takes, complete) = db.canonical_forms();
    assert!(forms > 0 && bytes > 0, "{forms} forms, {bytes} bytes");
    assert!(complete, "the table is complete after a maintained commit");
    assert!(takes > 0, "the reader walked the forms: {takes}");
    let (built, _) = r.block_cache_size();
    assert_eq!(built, 0, "the reader built nothing of its own");
    m.check(&r, "a reader over the maintained forms");
    m.check(&db, "the writer over its own forms");
    // A batch staged and not committed: the reader must not see it, and
    // the forms it walks must not hold it.
    for k in (1..1500u32).step_by(11) {
        m.append(&mut db, &key(k), "staged");
    }
    // Key 1 took "v1b" at the commit before, and "staged" after it.
    let mut got = Vec::new();
    r.scan(key(1).as_bytes(), 1, |k, v| {
        if k == key(1).as_bytes() {
            got.push(v.to_vec());
        }
    })
    .unwrap();
    assert_eq!(
        got,
        vec![b"v0".to_vec(), b"v1b".to_vec()],
        "a staged write is not in what a reader walks"
    );
    let mut staged = 0usize;
    r.scan(key(0).as_bytes(), 1500, |_k, v| {
        if v == b"staged" {
            staged += 1;
        }
    })
    .unwrap();
    assert_eq!(staged, 0, "no staged value anywhere in a reader's scan");
    db.commit().unwrap();
    m.check(&r, "after the staged batch committed");
    // A second reader pinned before the commit keeps its own answer.
    for k in (2..1500u32).step_by(23) {
        m.append(&mut db, &key(k), "v2");
    }
    db.commit().unwrap();
    let pinned = db.reader().unwrap();
    pinned.snapshot();
    let before: Vec<Vec<u8>> = {
        let mut v = Vec::new();
        pinned
            .scan(key(4).as_bytes(), 1, |k, val| {
                if k == key(4).as_bytes() {
                    v.push(val.to_vec());
                }
            })
            .unwrap();
        v
    };
    for k in (4..1500u32).step_by(37) {
        m.append(&mut db, &key(k), "v3");
    }
    db.commit().unwrap();
    let after: Vec<Vec<u8>> = {
        let mut v = Vec::new();
        pinned
            .scan(key(4).as_bytes(), 1, |k, val| {
                if k == key(4).as_bytes() {
                    v.push(val.to_vec());
                }
            })
            .unwrap();
        v
    };
    assert_eq!(before, after, "a pinned reader is not moved by a commit");
    m.check(&r, "a reader at the latest commit");
    pinned.release();
    m.check(&pinned, "the released reader");
    // A seal and a merge: a new state, an empty table, and the forms
    // maintained again from the commits after it.
    // A seal, which keeps every tombstone in its piece, so a deleted key
    // is still visited with no values; only the merge below drops one.
    db.seal().unwrap();
    db.settle().unwrap();
    for k in (3..1500u32).step_by(17) {
        m.append(&mut db, &key(k), "v4");
    }
    m.delete(&mut db, &key(300));
    db.commit().unwrap();
    m.check(&r, "after a seal, through the reader");
    db.flush().unwrap();
    m.flushed();
    for k in (7..1500u32).step_by(29) {
        m.append(&mut db, &key(k), "v5");
    }
    db.commit().unwrap();
    m.check(&r, "after a merge, through the reader");
    m.check(&db, "after a merge, through the writer");
    // Keys above the store's greatest, which fall in the last block
    // alone, and a reader over them.
    for k in 1500..1700u32 {
        m.append(&mut db, &key(k), "top");
    }
    db.commit().unwrap();
    m.check(&r, "keys past the top");
    std::hint::black_box(sink);
}

/// Three reader threads and a writer that seals and merges under
/// `commit_forms`: every version a thread reads is one the writer wrote,
/// no scan comes back out of order, and no key holds two values.
#[test]
fn reader_threads_over_maintained_forms_keep_answering() {
    let d = dir("commit-forms-threads");
    let opts = Options {
        seal_bytes: 32 << 10,
        partition_bytes: Some(64 << 10),
        l0_trigger: 2,
        scan_block_cache: true,
        commit_forms: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 3000u32;
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    for k in 0..keys {
        db.append(&key(k), b"0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut threads = Vec::new();
    for t in 0..3u64 {
        let r = db.reader().unwrap();
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            let mut seen: HashMap<u32, u64> = HashMap::new();
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ t;
            let mut ops = 0usize;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let k = (x % keys as u64) as u32;
                if x.is_multiple_of(4) {
                    let mut last: Option<Vec<u8>> = None;
                    r.scan(&key(k), 30, |kk, v| {
                        if let Some(l) = &last {
                            assert!(l.as_slice() < kk, "a scan out of key order");
                        }
                        last = Some(kk.to_vec());
                        let s = std::str::from_utf8(v).unwrap();
                        assert!(
                            s.parse::<u64>().is_ok(),
                            "a scanned value that is no version: {s}"
                        );
                    })
                    .unwrap();
                } else {
                    let got = read_vec(&r, &key(k));
                    assert!(got.len() <= 1, "a put key with two values");
                    if let Some(v) = got.first() {
                        let ver: u64 = std::str::from_utf8(v).unwrap().parse().unwrap();
                        let prev = seen.entry(k).or_insert(0);
                        assert!(
                            ver >= *prev,
                            "a version that went backwards: key {k} read {ver} after {prev}"
                        );
                        *prev = ver;
                    }
                }
                ops += 1;
            }
            ops
        }));
    }
    let mut x = 42u64;
    for round in 1..=200u64 {
        for _ in 0..50 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = (x % keys as u64) as u32;
            db.put(&key(k), round.to_string().as_bytes());
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut total = 0usize;
    for t in threads {
        total += t.join().unwrap();
    }
    assert!(total > 100, "the threads did some work: {total}");
    // The forms are current to a commit, so a reader only walks them
    // while the writer is between commits: under the writer above, which
    // commits every fifty puts, a reader is behind the table and builds
    // its own, which is what the invariants held. With the writer quiet
    // the same handle takes them.
    for k in (0..keys).step_by(9) {
        db.put(&key(k), b"901");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    let mut sink = 0usize;
    r.scan(&key(0), 500, |_k, v| sink += v.len()).unwrap();
    for k in (1..keys).step_by(11) {
        db.put(&key(k), b"902");
    }
    db.commit().unwrap();
    let before = db.canonical_forms().2;
    r.scan(&key(0), 500, |_k, v| sink += v.len()).unwrap();
    let (forms, _, takes, _) = db.canonical_forms();
    assert!(forms > 0, "the quiet writer maintained forms: {forms}");
    assert!(
        takes > before,
        "a reader over a quiet store walks them: {takes} against {before}"
    );
    std::hint::black_box(sink);
}

/// A block held two ways at once, and the read choosing: with
/// `promote_entries` the store keeps a merged copy of a block beside its
/// cheap form once reads have taken enough entries from it, and every
/// read after that walks the copy. A write to the block drops the copy
/// and halves the count, so a block the writes own is never held twice.
/// The answers are the model's throughout, whichever form a read picked.
#[test]
fn a_block_the_reads_pay_for_is_held_as_a_copy_too() {
    let d = dir("promote-forms");
    fn opts_promote() -> usize {
        300
    }
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(8 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        promote_entries: opts_promote(),
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..4000u32 {
        m.append(&mut db, &key(k), "v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert!(db.levels().0 > 1, "several partitions");
    // A few overlay keys in every block, which is the shape a block is
    // walked as deltas in: below `CACHE_DENSE`, so nothing is a copy yet.
    for k in (0..4000u32).step_by(23) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    let mut sink = 0usize;
    db.scan(key(0).as_bytes(), 64, |_k, v| sink += v.len())
        .unwrap();
    let [promoted, dropped, on_copy, on_cheap, bytes] = db.form_choices();
    assert_eq!(promoted, 0, "nothing promoted by one scan of a block");
    assert_eq!(on_copy, 0, "no copy to walk yet");
    assert!(on_cheap > 0 && dropped == 0 && bytes == 0);
    // The hot range, read until the reads have paid for a copy of it.
    for _ in 0..12 {
        db.scan(key(0).as_bytes(), 64, |_k, v| sink += v.len())
            .unwrap();
    }
    let [promoted, _, on_copy, _, bytes] = db.form_choices();
    assert!(promoted > 0, "the reads paid for a copy: {promoted}");
    assert!(on_copy > 0, "and the reads after walk it: {on_copy}");
    assert!(bytes > 0, "the copy holds bytes: {bytes}");
    m.check(&db, "a block held as a copy too");
    // A write into the hot block drops its copy; the reads that follow
    // walk the cheap form until they have paid for it again, which the
    // halved count makes half the entries.
    let hot = key(0);
    let before = db.form_choices();
    m.append(&mut db, &key(7), "v2");
    db.commit().unwrap();
    db.scan(hot.as_bytes(), 64, |_k, v| sink += v.len())
        .unwrap();
    let after = db.form_choices();
    assert!(
        after[1] > before[1],
        "the write dropped the copy: {after:?} against {before:?}"
    );
    let (_, _, dense, reads, kind) = db.block_state(hot.as_bytes()).expect("the hot block");
    assert!(!dense, "and the block is held one way again");
    assert_eq!(kind, "sparse", "as deltas over the partition");
    assert!(
        reads > 0 && (reads as usize) < opts_promote(),
        "the count is halved, not cleared: {reads}"
    );
    // Nothing but scans of the hot range from here, so the promotion
    // that follows is the reads' and no model check is between.
    let mut promoted_again = 0u64;
    for _ in 0..4 {
        db.scan(hot.as_bytes(), 64, |_k, v| sink += v.len())
            .unwrap();
        let now = db.form_choices();
        if now[0] > after[0] {
            promoted_again = now[0] - after[0];
            break;
        }
    }
    assert_eq!(
        promoted_again,
        1,
        "the copy is back for a block the reads still own: {:?} against {after:?}",
        db.form_choices()
    );
    let (_, _, dense, _, _) = db.block_state(hot.as_bytes()).expect("the hot block");
    assert!(dense, "held both ways again");
    m.check(&db, "after the copy came back");
    // A cold range the writes own: every scan of it is one entry, and
    // its blocks are never held twice.
    let cold = db.form_choices()[0];
    for r in 0..40u32 {
        m.append(&mut db, &key(3000 + r), "w");
        db.commit().unwrap();
        db.scan(key(3000).as_bytes(), 1, |_k, v| sink += v.len())
            .unwrap();
    }
    assert_eq!(
        db.form_choices()[0],
        cold,
        "a block the writes own is not promoted"
    );
    m.check(&db, "a range the writes own");
    // Through a reader handle, which holds its own forms and makes its
    // own choices, over the same store.
    let r = db.reader().unwrap();
    for _ in 0..12 {
        r.scan(key(0).as_bytes(), 64, |_k, v| sink += v.len())
            .unwrap();
    }
    assert!(
        r.form_choices()[0] > 0,
        "a reader handle promotes for itself: {:?}",
        r.form_choices()
    );
    m.check(&r, "through a reader handle");
    std::hint::black_box(sink);
}

/// A piece sealed while a merge of its range runs is kept across the
/// merge's publish, under a new partition over the same range. The ranks
/// it was given at its own publish -- each key's cut in the partition it
/// was aligned to -- were taken against the partition the merge replaced,
/// and against the new one every cut below a key the merge folded in is
/// behind by that key. A block built through a stale cut emits the
/// piece's key where the old partition had it. The oracle test reached
/// this in about one run in twenty as an assertion under the checked
/// profile, once the seal's timing put a piece inside a merge; here the
/// merge is long and the seal short, so the piece is published while the
/// merge runs every time, and the pieces the merge folds in carry new
/// keys below the kept piece's, so the old ranks are wrong by ten.
#[test]
fn a_piece_kept_across_a_merge_is_ranked_against_the_partition_it_meets() {
    let d = dir("kept-piece-ranks");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 20),
        l0_trigger: 2,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let key = |k: u32| format!("key-{k:06}").into_bytes();
    let between = |k: u32, i: u32| format!("key-{k:06}x{i}").into_bytes();
    let put = |db: &mut Db, model: &mut BTreeMap<Vec<u8>, Vec<Vec<u8>>>, k: &[u8], v: &str| {
        db.append(k, v.as_bytes());
        model
            .entry(k.to_vec())
            .or_default()
            .push(v.as_bytes().to_vec());
    };
    for k in 0..400_000 {
        put(&mut db, &mut model, &key(k), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    assert_eq!(db.levels(), (1, 0), "one partition over every key");
    // Two pieces over keys the partition holds, so the range is merged
    // rather than promoted, each with new keys below the third piece's.
    for i in 0..10 {
        put(&mut db, &mut model, &key(1000 + i), "a");
        put(&mut db, &mut model, &between(1000, i), "a");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    for i in 0..10 {
        put(&mut db, &mut model, &key(2000 + i), "b");
        put(&mut db, &mut model, &between(2000, i), "b");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    // The third piece: keys the partition holds and keys between them.
    // Its seal joins the second piece's first, which publishes it and
    // starts the merge, so the merge is running when this piece seals.
    for i in 0..10 {
        put(&mut db, &mut model, &key(3000 + i), "c");
        put(&mut db, &mut model, &between(3000, i), "c");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    assert!(
        db.in_flight().1,
        "the merge did not start at the second piece's publish"
    );
    // A commit joins a finished seal and leaves a running merge alone:
    // the third piece is published, and ranked, under the old partition.
    while db.in_flight().0 {
        db.commit().unwrap();
        std::thread::yield_now();
    }
    assert!(
        db.in_flight().1,
        "the merge finished before the third piece was published; the case was not reached"
    );
    assert_eq!(db.levels(), (1, 3));
    db.settle().unwrap();
    assert_eq!(
        db.levels(),
        (1, 1),
        "a new partition, and the third piece kept over it"
    );
    let mut seen: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    db.scan(b"", usize::MAX, |k, v| {
        seen.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    assert_eq!(seen.len(), model.len(), "the scan's key count");
    assert!(seen == model, "the scan disagrees with the model");
    // Short scans from inside the blocks the kept piece's keys land in,
    // which are built from those keys' cuts.
    for k in [2990u32, 2999, 3000, 3004, 3009, 3010] {
        let from = key(k);
        let mut got: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
        db.scan(&from, 20, |k, v| match got.last_mut() {
            Some((last, vals)) if last.as_slice() == k => vals.push(v.to_vec()),
            _ => got.push((k.to_vec(), vec![v.to_vec()])),
        })
        .unwrap();
        let want: Vec<(Vec<u8>, Vec<Vec<u8>>)> = model
            .range(from.clone()..)
            .take(20)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert!(
            got == want,
            "a scan of twenty from {} disagrees with the model",
            String::from_utf8_lossy(&from)
        );
    }
}

/// The scan snapshot is the sorted keys of everything unsealed, and a
/// handle that builds its own sorts all of them: at a hundred thousand
/// keys each updated once, 9 ms, paid again by every handle that reads
/// the same state. Published in the state, the first handle to want one
/// builds it and the rest adopt it.
#[test]
fn every_handle_after_the_first_adopts_the_snapshot_it_published() {
    let d = dir("share-snap");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        share_snapshot: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..1500u32 {
        m.append(&mut db, &key(k), "v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    // Unsealed keys, which are what a snapshot holds.
    for k in (0..1500u32).step_by(3) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    let before = db.snapshot_builds();
    let mut sink = 0usize;
    let first = db.reader().unwrap();
    first
        .scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(
        db.snapshot_builds(),
        before + 1,
        "the handle that wants one first builds it"
    );
    let mut others = Vec::new();
    for _ in 0..4 {
        let r = db.reader().unwrap();
        r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        others.push(r);
    }
    assert_eq!(
        db.snapshot_builds(),
        before + 1,
        "the handles after it adopt that one"
    );
    for (i, r) in others.iter().enumerate() {
        m.check(r, &format!("handle {i} over a snapshot it adopted"));
    }
}

/// A published snapshot stops where the memtable was when it was built,
/// and a handle that adopts it is typically past that point: the slots
/// created since stay in the handle's added list, keyed by the length
/// the snapshot it took covers rather than by the length it would have
/// built at. Getting that wrong loses exactly the newest keys, which no
/// scan of a store written and read in one go would show.
#[test]
fn a_handle_that_adopts_a_snapshot_sees_the_keys_written_past_it() {
    let d = dir("share-snap-past");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        share_snapshot: true,
        // Wide enough that the writes below leave the published snapshot
        // adoptable, which is what this test is about.
        snapshot_adopt_behind: 4096,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..1500u32 {
        m.append(&mut db, &key(k), "v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    for k in (0..1500u32).step_by(3) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    let mut sink = 0usize;
    let first = db.reader().unwrap();
    first
        .scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    let published = db.snapshot_builds();
    // Keys the published snapshot cannot hold: some beyond its greatest,
    // some between the keys it has, and one it must stop reporting.
    for k in 1500..1700u32 {
        m.append(&mut db, &key(k), "v2");
    }
    for k in (1..1500u32).step_by(7) {
        m.append(&mut db, &key(k), "v2");
    }
    m.delete(&mut db, &key(9));
    db.commit().unwrap();
    let next = db.reader().unwrap();
    m.check(
        &next,
        "a handle over a snapshot from before the last writes",
    );
    assert_eq!(
        db.snapshot_builds(),
        published,
        "and it read them without building one of its own"
    );
}

/// A snapshot carried forward rather than sorted again: the batch written
/// since it was built is sorted on its own and merged into the run, which
/// is what an immutable batch costs once. The merge has to fold as the
/// build's sort does: a key the frozen memtable holds and a live write
/// then touches is one key with two slots, and left as two entries the
/// merge path emits the frozen values and drops the live ones.
#[test]
fn a_snapshot_carried_forward_folds_a_live_write_onto_a_frozen_key() {
    carry_model(true);
}

/// The same on the merge path, where the snapshot's runs are what a scan
/// walks rather than the bounds of a block it builds: a key left in the
/// run twice is emitted twice there, which the block path does not show.
#[test]
fn a_snapshot_carried_forward_on_the_merge_path_folds_it_too() {
    carry_model(false);
}

fn carry_model(block_cache: bool) {
    let d = dir(&format!("extend-snap-{block_cache}"));
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        scan_block_cache: block_cache,
        scan_cache_ahead: false,
        share_snapshot: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:06}");
    for k in (0..6000u32).step_by(3) {
        m.append(&mut db, &key(k), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    assert!(db.levels().0 > 1, "several partitions");
    // A seal that is not joined, so the snapshot is built over a frozen
    // memtable and a live one both.
    for k in (0..6000u32).step_by(6) {
        m.append(&mut db, &key(k), "f");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    assert!(db.in_flight().0, "a seal in flight");
    for k in (0..6000u32).step_by(12) {
        m.append(&mut db, &key(k), "l0");
    }
    db.commit().unwrap();
    let mut sink = 0usize;
    db.scan(key(0).as_bytes(), 6000, |_k, v| sink += v.len())
        .unwrap();
    let built = db.snapshot_builds();
    assert_eq!(db.snapshot_extends(), 0, "the first one is sorted");
    // More new slots than a snapshot may lack before a scan renews it.
    // Half of these are live writes onto keys only the frozen table
    // holds, which is the fold; the rest are keys nothing holds yet.
    for k in (0..6000u32).step_by(6) {
        m.append(&mut db, &key(k), "l1");
    }
    m.delete(&mut db, &key(18));
    for k in 6000..13000u32 {
        m.append(&mut db, &key(k), "l1");
    }
    db.commit().unwrap();
    db.scan(key(0).as_bytes(), 6000, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(
        db.snapshot_builds(),
        built,
        "the second is carried forward, not sorted again"
    );
    assert_eq!(db.snapshot_extends(), 1, "by one merge");
    m.check(&db, "a snapshot carried forward over a frozen memtable");
}

/// The maintenance regime is the store's behaviour, not a setting: the
/// canonical forms cost the writer at every commit and are read by
/// whoever holds a handle, so they pay where several read and lose where
/// the writer reads its own store -- 1.280x and 1.441x of the arm without
/// them on the threaded scan mix, 0.849x and 0.879x on ycsb-E. The writer
/// maintains them once a handle its caller made is live, and not before.
#[test]
fn the_writer_maintains_the_forms_once_a_reader_handle_is_live() {
    let d = dir("regime-readers");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..1500u32 {
        m.append(&mut db, &key(k), "v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert!(db.levels().0 > 1, "several partitions");
    let mut sink = 0usize;
    // The writer reading its own store: it scans, it writes, it commits,
    // and nothing is maintained, because there is nobody to read it.
    for round in 0..3 {
        for k in (0..1500u32).step_by(5) {
            m.append(&mut db, &key(k), "vw");
        }
        db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        db.commit().unwrap();
        assert_eq!(
            db.canonical_forms().0,
            0,
            "round {round}: nothing maintained for the writer alone"
        );
    }
    // A handle the caller holds, scanning: from the next commit the
    // writer keeps the forms current for it.
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    for k in (1..1500u32).step_by(7) {
        m.append(&mut db, &key(k), "vr");
    }
    db.commit().unwrap();
    assert!(
        db.canonical_forms().0 > 0,
        "maintained once a handle is live"
    );
    m.check(&r, "a reader over the forms the writer maintains for it");
    // The evidence is the store's rather than the state's, so a seal
    // carries it: a store being read through handles goes on being read,
    // and kept per state the threaded scan mix at three hundred thousand
    // keys measured 1.49x for the arm that maintains regardless, because
    // a seal lands there often enough that the writer commits several
    // times before a handle reads again. Nothing has to forget, since a
    // commit with no scan since the last one maintains nothing anyway.
    let r2 = db.reader().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    for k in (2..1500u32).step_by(11) {
        m.append(&mut db, &key(k), "vg");
    }
    r2.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    assert!(
        db.canonical_forms().0 > 0,
        "the state after the seal keeps what the store learned"
    );
    m.check(&r2, "a reader over the state a seal published");
}
