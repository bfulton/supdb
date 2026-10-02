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
    // The window the segment file name exists for: the seal's segment is
    // in place and the manifest names it, then the process died before
    // the WAL reset. The WAL still holds every sealed record. (Before the
    // manifest from birth this staged the state one step earlier, the
    // segment renamed and named by nothing, and the open found it by
    // scanning the directory; that state is now the swept one, and
    // `a_segment_the_manifest_does_not_name_is_swept_and_its_wal_replayed`
    // has it.)
    let d = dir("renamewin");
    let mut db = Db::create(&d, Options::default()).unwrap();
    for i in 0u32..40 {
        db.append(b"dup-window", &i.to_le_bytes());
    }
    db.commit().unwrap();

    // Emulate: copy the WAL aside, seal and land it (which resets the
    // WAL), then put the pre-seal WAL back. Disk state is now exactly
    // manifest-done, reset-lost.
    let wal = d.join("wal-00000000");
    let saved = std::fs::read(&wal).unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
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

fn oracle(cursors: bool, upkeep: supdb::Upkeep) {
    let _ = oracle_in(
        &format!(
            "oracle-{}-{upkeep:?}",
            if cursors { "cursors" } else { "probes" }
        ),
        Options {
            cursor_merge: cursors,
            upkeep,
            ..Options::default()
        },
    );
}

fn oracle_in(name: &str, opts: Options) -> supdb::db::SealWaits {
    // The differential model oracle: random appends, commits, seals, and
    // crash-reopens, checked against a HashMap after every reopen.
    // Uncommitted writes are trimmed from the model at a crash, which is the
    // durability contract.
    let d = dir(name);
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // What the seals did, summed over the reopens, for a wrapper to hold
    // its option to having been taken.
    let mut total = supdb::db::SealWaits::default();
    let add = |t: &mut supdb::db::SealWaits, w: supdb::db::SealWaits| {
        t.wal_rotations += w.wal_rotations;
        t.rotated_unsynced += w.rotated_unsynced;
        t.deferred += w.deferred;
        t.publishes += w.publishes;
    };
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
                add(&mut total, db.seal_waits());
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
    add(&mut total, db.seal_waits());
    total
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
    oracle(true, supdb::Upkeep::Inline);
}

/// The same oracle with the writer's upkeep on a thread: every reopen is
/// a drop, which stops the thread with the upkeep lent or home.
#[test]
fn the_oracle_holds_with_the_upkeep_on_a_thread() {
    oracle(true, supdb::Upkeep::Background(3));
}

/// The probe merge stays behind `cursor_merge` as the comparison arm -- and
/// a path only one arm exercises is a path nothing tests.
#[test]
fn the_probe_merge_arm_passes_the_same_oracle() {
    oracle(false, supdb::Upkeep::Inline);
}

/// An unpinned writer holds no state it reads, so a store refuses it
/// beside any other thread that publishes: the segment work on its own
/// thread replaced and freed a state under the writer's read, and the
/// suite's unpinned arm faulted in a third of its runs. Inline, where the
/// writer is the one publisher, it is the shape the pins were priced
/// against, and the model holds there.
#[test]
fn an_unpinned_writer_is_refused_beside_another_publisher() {
    let refused = |name: &str, opts: Options| {
        let d = dir(name);
        let e = Db::create(&d, opts.clone())
            .err()
            .expect("create refuses it");
        assert!(e.to_string().contains("writer_pins"), "{e}");
        assert!(Db::open(&d, opts).is_err(), "and so does open");
    };
    refused(
        "nopin-background",
        Options {
            writer_pins: false,
            ..Options::default()
        },
    );
    refused(
        "nopin-keeper",
        Options {
            writer_pins: false,
            publish_in_background: false,
            snapshot_keeper: true,
            ..Options::default()
        },
    );
    oracle_in(
        "oracle-nopin-inline",
        Options {
            writer_pins: false,
            publish_in_background: false,
            ..Options::default()
        },
    );
}

/// The oracle with seals that take their end from the live WAL
/// (`seal_rotates_wal` off) and a log small enough to rotate by size
/// every few commits: files that span landed seals, files closed and
/// retired by the next seal's landing, and crash-reopens between every
/// one of those, each replaying from the manifest's covered sequence.
#[test]
fn the_oracle_holds_with_seals_that_keep_the_wal() {
    let w = oracle_in(
        "oracle-walseq",
        Options {
            seal_rotates_wal: false,
            seal_bytes: 2 << 10,
            ..Options::default()
        },
    );
    assert!(w.wal_rotations > 0, "the log rotated by size: {w:?}");
    assert_eq!(w.rotated_unsynced, 0, "{w:?}");
}

/// And under the shape (`adaptive_shape`), whose landings promote and
/// shape: seals that keep the WAL beside the shaping's merges, under
/// random writes and crashes. Deferral is not here: at this cadence the
/// seals are too small to hold the slot at a threshold commit, and an
/// oracle that sets the option without reaching it would claim a path
/// it never took; `a_commit_past_the_threshold_keeps_writing_while_a_seal_holds_the_slot`
/// holds a seal to take it.
#[test]
fn the_oracle_holds_under_the_shape_with_seals_that_keep_the_wal() {
    let w = oracle_in(
        "oracle-shape-walseq",
        Options {
            seal_rotates_wal: false,
            adaptive_shape: true,
            seal_bytes: 2 << 10,
            ..Options::default()
        },
    );
    assert!(w.wal_rotations > 0 && w.publishes > 0, "{w:?}");
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

/// Promotion without a merge is for a store with no partitions yet: every
/// piece becomes one, the first open below. The full flush asked for it on
/// a partitioned store too, and pieces that happened to be disjoint became
/// partitions over ranges partitions already held -- nothing raised, and
/// the reads routed past the keys. Pieces piled up over a partitioned store
/// are what a flush meets after a merge in flight held them back; here the
/// trigger holds them back.
#[test]
fn a_full_flush_over_partitions_does_not_promote_its_pieces() {
    let d = dir("fullflush-promote");
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 64 << 10,
            l0_trigger: 1000,
            partition_bytes: None,
            flush_ranges: false,
            ..Options::default()
        },
    )
    .unwrap();
    let n = 20_000u32;
    let val = |i: u32| {
        let mut v = i.to_le_bytes().to_vec();
        v.extend_from_slice(&[b'x'; 100]);
        v
    };
    for i in 0..n {
        db.append(format!("k{i:08}").as_bytes(), &val(i));
        if i % 500 == 499 {
            db.commit().unwrap();
        }
        if i == n / 2 - 1 {
            db.flush().unwrap();
            assert!(db.levels().0 > 1, "the first flush partitions");
        }
    }
    db.settle().unwrap();
    assert!(db.levels().1 > 1, "pieces held back over the partitions");
    db.flush().unwrap();
    let wrong: Vec<u32> = (0..n)
        .filter(|&i| read_vec(&db, format!("k{i:08}").as_bytes()) != vec![val(i)])
        .collect();
    assert!(
        wrong.is_empty(),
        "{} keys wrong, first {:?} ({:?})",
        wrong.len(),
        &wrong[..wrong.len().min(5)],
        db.levels()
    );
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
        // The seal's rotation; a rotation by size has its own test.
        seal_rotates_wal: true,
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
    /// One scan against the model, stream and count.
    fn check_one(&self, db: &Reader, from: &[u8], limit: usize, state: &str) {
        let mut want: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut want_n = 0usize;
        for k in self.visited().iter().filter(|k| **k >= from).take(limit) {
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
/// frozen memtable under a seal held short of its landing
/// (`hold_seal_landing`, since the segment work lands one on its own) with
/// the live table written over it: a key in all three sources, tombstones in each memtable cutting the
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

/// The same model with the writer's upkeep on a thread: every check reads
/// through the writer, which takes the upkeep back from wherever the
/// thread has brought it.
#[test]
fn the_block_cache_answers_the_same_model_with_the_upkeep_on_a_thread() {
    overlay_model_with("overlay-upkeep", true, 0, supdb::Upkeep::Background(3));
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
        // The premise is sixteen pieces over every range, so none merge.
        tier_pieces: 0,
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

/// A seal's piece landed readable, with its keys above the last
/// partition's last key, before the writer looks at its log again, and
/// then a key above the store's greatest opens a direct run: the writer's
/// switch to the ordered table is its own publish and carries its tables,
/// and the landing between is not its own. Carried across both, the
/// tables had no bounds for the piece and the snapshot named a frozen
/// table the landing had retired, so every scan through the writer read
/// the last block without a key of the piece, while a point read and a
/// handle found them all. The seal is held between its phases so the
/// piece stays unpromoted for the scans.
#[test]
fn a_direct_run_opened_after_a_landing_the_writer_has_not_seen_keeps_the_piece() {
    for staged in [true, false] {
        let d = dir(&format!("switch-after-landing-{staged}"));
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
        // Written high to low, so the seal is the memtable's and not a
        // direct run's.
        for k in (0..2100).rev() {
            m.append(&mut db, &format!("live-{k:05}"), "l");
        }
        db.commit().unwrap();
        m.check(&db, "a snapshot over thousands of live keys");
        db.hold_seal_durable(true);
        db.seal().unwrap();
        // The landing, with nothing through the writer that reads its log.
        wait_for("the piece's publish", || db.levels().1 == 1);
        assert!(db.in_flight().0, "held: in flight until durable");
        // Ascending and above everything: the first opens a direct run.
        for k in 0..600 {
            m.append(&mut db, &format!("new-{k:05}"), "n");
        }
        if !staged {
            db.commit().unwrap();
        }
        m.check(&db, "a direct run opened after the landing");
        if !staged {
            let r = db.reader().unwrap();
            m.check(&r, "a handle beside it");
        }
        db.hold_seal_durable(false);
        db.commit().unwrap();
        db.settle().unwrap();
        m.check(&db, "the seal durable");
    }
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
            // The segments as the threshold left them, none merged.
            tier_pieces: 0,
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
    overlay_model_with(name, block_cache, budget, supdb::Upkeep::Inline)
}

fn overlay_model_with(name: &str, block_cache: bool, budget: usize, upkeep: supdb::Upkeep) {
    let d = dir(name);
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        scan_block_cache: block_cache,
        scan_cache_bytes: budget,
        upkeep,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    // On a thread, a commit of a few writes wakes nothing and the next
    // scan takes the upkeep back untouched; `settle` waits for the
    // thread's pass instead, so the check after it reads what the thread
    // filed.
    let pass = |db: &mut Db| {
        if upkeep != supdb::Upkeep::Inline {
            db.settle().unwrap();
        }
    };
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
    pass(&mut db);
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
    // Held before its landing: the segment work lands a seal on its own
    // thread as soon as the seal thread names its segments, which is
    // before the checks below could run, and the frozen table is what
    // they are about.
    db.hold_seal_landing(true);
    db.seal().unwrap();
    assert!(db.in_flight().0, "the seal is in flight, held");
    assert_eq!(db.levels(), (parts, 0), "a held seal publishes no piece");

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

    db.hold_seal_landing(false);
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
    pass(&mut db);
    assert_eq!(db.levels().1, 0);
    m.check(&db, "live inserts past the end, after the merge");
    held(&db, budget);
    if upkeep != supdb::Upkeep::Inline {
        assert!(
            db.upkeep_counts()[0] > 0,
            "the thread made no pass, so nothing above tested it"
        );
    }
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
    // A store that does not partition on flush seals its piece a piece --
    // the seal names only what a partitioning flush would have promoted --
    // and with `flush_schedules` off the flush leaves it one.
    let d = dir("first-partition-unpartitioned");
    let mut db = Db::create(
        &d,
        Options {
            partition_on_flush: false,
            flush_schedules: false,
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
    // With it on, the default, the flush promotes the piece it sealed:
    // a partition by link, under the partition's name, nothing rewritten.
    let d = dir("first-partition-scheduled");
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
    assert_eq!(db.levels(), (1, 0), "the lone piece promoted");
    let names = files(&d);
    assert!(
        names.len() == 1 && names[0].starts_with("par-") && names[0].ends_with("--.sup"),
        "the promoted file carries the partition's name: {names:?}"
    );
    assert_eq!(read_vec(&db, &key(7)), vec![b"v".to_vec()]);
    let mut n = 0usize;
    db.scan(&key(0), 5000, |_k, _v| n += 1).unwrap();
    assert_eq!(n, 3000);
}

/// An ordered load a `sync` only makes durable is left in its direct run,
/// unpartitioned; with `Options::adaptive_shape` the sync hands the run to
/// a seal as well, and the seal, over a store with no segment, names the
/// first partition itself, so no promotion follows and the first read
/// after the landing routes by fence.
#[test]
fn a_durable_only_sync_seals_an_ordered_load_as_its_first_partition() {
    let d = dir("adaptive-shape-sync");
    let mut db = Db::create(
        &d,
        Options {
            adaptive_shape: true,
            ..Options::default()
        },
    )
    .unwrap();
    let n = 30_000u32;
    let val = |i: u32| {
        let mut v = i.to_le_bytes().to_vec();
        v.extend_from_slice(&[b'x'; 100]);
        v
    };
    for i in 0..n {
        db.append(format!("k{i:08}").as_bytes(), &val(i));
        if i % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.sync().unwrap();
    db.settle().unwrap();
    assert_eq!(
        db.levels(),
        (1, 0),
        "the sync's seal is the first partition"
    );
    let r = db.reader().unwrap();
    for i in (0..n).step_by(97) {
        assert_eq!(read_vec(&r, format!("k{i:08}").as_bytes()), vec![val(i)]);
    }
    drop(r);
    db.close().unwrap();
}

/// A store reopened with one piece and no partition is promoted by the
/// segment work at once, with nothing reading it: a promotion rewrites
/// nothing, so it waits for nobody.
#[test]
fn a_lone_piece_is_promoted_at_open_without_a_read() {
    let d = dir("adaptive-shape-open");
    let val = |i: u32| {
        let mut v = i.to_le_bytes().to_vec();
        v.extend_from_slice(&[b'x'; 100]);
        v
    };
    let n = 20_000u32;
    {
        // Without the shaping, a seal that is not a flush's leaves a piece.
        let mut db = Db::create(&d, Options::default()).unwrap();
        for i in 0..n {
            db.append(format!("k{i:08}").as_bytes(), &val(i));
            if i % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        db.seal().unwrap();
        db.settle().unwrap();
        assert_eq!(db.levels(), (0, 1), "a seal leaves a piece");
        // Dropped, not closed: a close flushes, and a flush partitions.
    }
    let db = Db::open(
        &d,
        Options {
            adaptive_shape: true,
            ..Options::default()
        },
    )
    .unwrap();
    let t = std::time::Instant::now();
    while db.levels().0 == 0 && t.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(db.levels(), (1, 0), "the open promoted the piece");
    for i in (0..n).step_by(97) {
        assert_eq!(read_vec(&db, format!("k{i:08}").as_bytes()), vec![val(i)]);
    }
}

/// Overlapping pieces a promotion cannot partition are merged once the
/// store is read, and not before: left unread, the store stays as it is.
#[test]
fn overlapping_pieces_are_merged_once_the_store_is_read() {
    let d = dir("adaptive-shape-merge");
    let n = 20_000u64;
    let val = |i: u64| {
        let mut v = i.to_le_bytes().to_vec();
        v.extend_from_slice(&[b'x'; 100]);
        v
    };
    // Shuffled, so every piece spans the key space; seals small enough
    // to leave several, and a trigger that never fires.
    let key = |i: u64| format!("k{:08}", (i * 7919) % n);
    let pieces = {
        let mut db = Db::create(
            &d,
            Options {
                seal_bytes: 512 << 10,
                l0_trigger: 100,
                ..Options::default()
            },
        )
        .unwrap();
        for i in 0..n {
            db.append(key(i).as_bytes(), &val((i * 7919) % n));
            if i % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        db.seal().unwrap();
        db.settle().unwrap();
        let (parts, pieces) = db.levels();
        assert_eq!(parts, 0);
        assert!(
            pieces >= 2,
            "the load leaves {pieces} pieces; the test wants several"
        );
        pieces
    };
    let db = Db::open(
        &d,
        Options {
            adaptive_shape: true,
            l0_trigger: 100,
            ..Options::default()
        },
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        db.levels(),
        (0, pieces),
        "nothing was read, and the store was merged anyway"
    );
    let r = db.reader().unwrap();
    let t = std::time::Instant::now();
    let mut scans = 0u64;
    while db.levels().0 == 0 && t.elapsed() < std::time::Duration::from_secs(20) {
        let from = format!("k{:08}", (scans * 131) % n);
        r.scan(from.as_bytes(), 100, |_, _| {}).unwrap();
        scans += 1;
    }
    assert!(db.levels().0 > 0, "{scans} scans and nothing was merged");
    for i in (0..n).step_by(97) {
        assert_eq!(read_vec(&r, format!("k{i:08}").as_bytes()), vec![val(i)]);
    }
    drop(r);
    db.close().unwrap();
}

/// A seal lands without the writer: with the segment work on a thread of
/// its own, the frozen table's segment is published, and a handle reads
/// it, while the writer makes no call at all. Before it, a finished seal
/// waited for the writer's next commit, seal or flush to be published.
#[test]
fn a_seal_lands_while_the_writer_is_away() {
    let d = dir("seal-lands-away");
    let opts = Options {
        publish_in_background: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let key = |k: u32| format!("key-{k:06}");
    for k in 0..2000u32 {
        db.append(key(k).as_bytes(), format!("v{k}").as_bytes());
    }
    db.commit().unwrap();
    db.seal().unwrap();
    let r = db.reader().unwrap();
    // No call on the writer from here on.
    let t = std::time::Instant::now();
    while r.unsealed_keys() > 0 {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "the seal did not land without the writer"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(r.levels().1 > 0, "a piece landed: {:?}", r.levels());
    for k in (0..2000u32).step_by(37) {
        assert_eq!(
            read_vec(&r, key(k).as_bytes()),
            vec![format!("v{k}").into_bytes()],
            "key {k}"
        );
    }
    drop(r);
    db.close().unwrap();
}

/// A flush that does not partition leaves what promotion cannot make
/// partitions -- pieces that overlap -- to a merge it starts in the
/// background, and the writer publishes that merge at its first commit
/// after it finishes. Before `flush_schedules` nothing ever did: the
/// background waits for `l0_trigger` pieces, and a store of fewer stayed
/// pieces, every scan on the merge path. Held against the same store with
/// the option off, which is the check that the test reaches the path.
#[test]
fn a_flush_that_does_not_partition_merges_the_rest_in_the_background() {
    let key = |k: u32| format!("key-{k:06}").into_bytes();
    // A hundred-byte value, the suite's, so the load seals several times.
    let val = |k: u32| format!("v{k:06}-{}", "x".repeat(92)).into_bytes();
    let keys = 4000u32;
    let mut order: Vec<u32> = (0..keys).collect();
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for i in (1..order.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        order.swap(i, (x % (i as u64 + 1)) as usize);
    }
    for schedules in [false, true] {
        let d = dir(&format!("flush-schedules-{schedules}"));
        let mut db = Db::create(
            &d,
            Options {
                partition_on_flush: false,
                flush_schedules: schedules,
                seal_bytes: 64 << 10,
                // No merge of the seals' own: the pieces the flush leaves
                // are the whole of what happens to the store.
                l0_trigger: 100,
                ..Options::default()
            },
        )
        .unwrap();
        for (i, &k) in order.iter().enumerate() {
            db.append(&key(k), &val(k));
            if i % 100 == 99 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        let (parts, pieces) = db.levels();
        assert_eq!(
            parts, 0,
            "schedules {schedules}: nothing partitioned in the flush"
        );
        assert!(
            pieces >= 2,
            "schedules {schedules}: overlapping pieces, which no promotion tiles: {pieces}"
        );
        for k in [0, 1234, keys - 1] {
            assert_eq!(
                read_vec(&db, &key(k)),
                vec![val(k)],
                "schedules {schedules}"
            );
        }
        let t = std::time::Instant::now();
        loop {
            db.commit().unwrap();
            if db.levels().1 == 0
                || t.elapsed() > std::time::Duration::from_secs(if schedules { 30 } else { 1 })
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (parts, pieces) = db.levels();
        if schedules {
            assert!(
                parts >= 1 && pieces == 0,
                "the merge the flush started was published at a commit: {parts} partitions, {pieces} pieces"
            );
        } else {
            assert_eq!(parts, 0, "without the option the store stays pieces");
        }
        let mut n = 0usize;
        let mut last: Option<Vec<u8>> = None;
        db.scan(&key(0), keys as usize + 10, |k, v| {
            if let Some(l) = &last {
                assert!(l.as_slice() < k, "a scan out of key order");
            }
            let kn: u32 = std::str::from_utf8(&k[4..]).unwrap().parse().unwrap();
            assert_eq!(v, val(kn).as_slice());
            last = Some(k.to_vec());
            n += 1;
        })
        .unwrap();
        assert_eq!(n, keys as usize, "schedules {schedules}: every key, once");
    }
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
        // This test is about the scan-gated regime -- nothing maintained before a
        // scan -- so the backlog bound, which maintains past a share of the
        // store's writes with no scan at all, is off here.
        forms_settle_backlog_pct: 0,
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
/// no scan comes back out of order or older than a read before it, and
/// no key holds two values.
#[test]
fn reader_threads_over_maintained_forms_keep_answering() {
    reader_threads_over_blocks(true, supdb::Upkeep::Inline);
}

/// The same threads over blocks each handle builds and settles itself.
#[test]
fn reader_threads_over_their_own_blocks_keep_answering() {
    reader_threads_over_blocks(false, supdb::Upkeep::Inline);
}

/// The same threads with the writer's upkeep on a thread of its own: the
/// forms they walk are the ones the upkeep thread published, at the
/// commits the writer named for it.
#[test]
fn reader_threads_over_forms_the_upkeep_thread_maintains() {
    reader_threads_over_blocks(true, supdb::Upkeep::Background(3));
}

/// The same at level 2, which files a batch of fifty itself and holds a
/// commit of four hundred for the thread's pass.
#[test]
fn reader_threads_over_forms_the_adaptive_upkeep_maintains() {
    reader_threads_over_blocks(true, supdb::Upkeep::Background(2));
}

/// The same at level 1, where no commit holds: the thread's passes run
/// while the writer commits past them, so the forms a pass publishes
/// must carry the commit it was named and not the writer's latest.
#[test]
fn reader_threads_over_forms_a_lagging_upkeep_maintains() {
    reader_threads_over_blocks(true, supdb::Upkeep::Background(1));
}

fn reader_threads_over_blocks(commit_forms: bool, upkeep: supdb::Upkeep) {
    let d = dir(&format!("commit-forms-threads-{commit_forms}-{upkeep:?}"));
    let opts = Options {
        upkeep,
        seal_bytes: 32 << 10,
        partition_bytes: Some(64 << 10),
        l0_trigger: 2,
        scan_block_cache: true,
        commit_forms,
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
                    // A point read and then a scan from the same key: the
                    // scan walks cached blocks -- the writer's forms or
                    // the handle's own -- and the point read the
                    // memtable, so a block settled short of a commit it
                    // claims shows the key older than the read before it.
                    if let Some(v) = read_vec(&r, &key(k)).first() {
                        let ver: u64 = std::str::from_utf8(v).unwrap().parse().unwrap();
                        let prev = seen.entry(k).or_insert(0);
                        *prev = (*prev).max(ver);
                    }
                    let mut last: Option<Vec<u8>> = None;
                    r.scan(&key(k), 30, |kk, v| {
                        if let Some(l) = &last {
                            assert!(l.as_slice() < kk, "a scan out of key order");
                        }
                        last = Some(kk.to_vec());
                        let s = std::str::from_utf8(v).unwrap();
                        let ver: u64 = s
                            .parse()
                            .unwrap_or_else(|_| panic!("a scanned value that is no version: {s}"));
                        let kn: u32 = std::str::from_utf8(&kk[4..]).unwrap().parse().unwrap();
                        let prev = seen.entry(kn).or_insert(0);
                        assert!(
                            ver >= *prev,
                            "a scan went backwards: key {kn} scanned {ver} after {prev}"
                        );
                        *prev = ver;
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
        // Every tenth batch large enough to wake an upkeep thread, so its
        // passes run while the commits after them land.
        let batch = if round % 10 == 0 { 400 } else { 50 };
        for _ in 0..batch {
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
    if !commit_forms {
        return;
    }
    // The forms are current to a commit, so a reader only walks them
    // while the writer is between commits: under the writer above, which
    // commits every fifty puts, a reader is behind the table and builds
    // its own, which is what the invariants held. With the writer quiet
    // the same handle takes them.
    // On a thread, the forms of a commit are published once its pass has
    // run: `settle` waits for that, as a quiet writer would.
    let quiet = |db: &mut Db| {
        if upkeep != supdb::Upkeep::Inline {
            db.settle().unwrap();
        }
    };
    for k in (0..keys).step_by(9) {
        db.put(&key(k), b"901");
    }
    db.commit().unwrap();
    quiet(&mut db);
    let r = db.reader().unwrap();
    let mut sink = 0usize;
    r.scan(&key(0), 500, |_k, v| sink += v.len()).unwrap();
    for k in (1..keys).step_by(11) {
        db.put(&key(k), b"902");
    }
    db.commit().unwrap();
    quiet(&mut db);
    let before = db.canonical_forms().2;
    r.scan(&key(0), 500, |_k, v| sink += v.len()).unwrap();
    let (forms, _, takes, _) = db.canonical_forms();
    assert!(forms > 0, "the quiet writer maintained forms: {forms}");
    assert!(
        takes > before,
        "a reader over a quiet store walks them: {takes} against {before}"
    );
    // At level 2 every commit above followed a scan, which the commit's
    // own rules file, so the thread is not asked to.
    if matches!(upkeep, supdb::Upkeep::Background(l) if l != 2) {
        assert!(
            db.upkeep_counts()[0] > 0,
            "the thread made no pass, so nothing above tested it"
        );
    }
    std::hint::black_box(sink);
}

/// Level 2 hands the thread only what a commit's own rules would leave
/// to the next read: a commit a scan preceded files its batch itself, as
/// does a batch too small to wake the thread for, and a batch under the
/// settle bound with no scan since holds for the thread's pass. Every
/// write reads back through the scan after.
#[test]
fn the_adaptive_upkeep_holds_only_for_what_a_commit_leaves_to_a_read() {
    let d = dir("upkeep-adaptive");
    let opts = Options {
        upkeep: supdb::Upkeep::Background(2),
        partition_bytes: Some(256 << 10),
        // The plain seal, so the few hundred writes below seal nothing.
        seal_max_pct: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    // The settle bound is 2% of the partitions' keys: 400.
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:06}").into_bytes();
    for k in 0..keys {
        db.append(&key(k), b"0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    assert!(db.levels().0 > 0, "partitions for the forms to hang on");
    let mut want: HashMap<u32, String> = HashMap::new();
    let check = |db: &Db, want: &HashMap<u32, String>, when: &str| {
        let mut seen = 0u32;
        db.scan(&key(0), keys as usize, |k, v| {
            let kn: u32 = std::str::from_utf8(&k[4..]).unwrap().parse().unwrap();
            let got = std::str::from_utf8(v).unwrap();
            let expect = want.get(&kn).map_or("0", |s| s.as_str());
            assert_eq!(got, expect, "key {kn} {when}");
            seen += 1;
        })
        .unwrap();
        assert_eq!(seen, keys, "every key {when}");
    };
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut write = |db: &mut Db, want: &mut HashMap<u32, String>, n: usize, v: &str| {
        for _ in 0..n {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = (x % keys as u64) as u32;
            db.put(&key(k), v.as_bytes());
            want.insert(k, v.to_string());
        }
        db.commit().unwrap();
    };
    let holds = |db: &Db| db.upkeep_counts()[4];
    check(&db, &want, "before any write");
    let h = holds(&db);
    write(&mut db, &mut want, 300, "a");
    assert_eq!(
        holds(&db),
        h,
        "a commit a scan preceded files its own batch"
    );
    write(&mut db, &mut want, 300, "b");
    assert_eq!(holds(&db), h + 1, "a batch left to the next read holds");
    write(&mut db, &mut want, 100, "c");
    assert_eq!(holds(&db), h + 1, "a batch too small to wake for does not");
    check(&db, &want, "after the three commits");
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
        // Read-driven promotion is what this test is about, and it is a
        // property of a handle that builds its own forms: with the
        // writer maintaining them a reader walks what it is given, pays
        // for no block and promotes nothing, which is the point of the
        // maintenance rather than a fault in it.
        commit_forms: false,
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
    // Its seal waits for the second piece's publish, whose landing then
    // starts the merge on the segment work's thread -- a few fsyncs after
    // the publish the writer waited for, so the start is polled for -- and
    // that thread lands this piece after it, so the merge is running when
    // this piece lands.
    for i in 0..10 {
        put(&mut db, &mut model, &key(3000 + i), "c");
        put(&mut db, &mut model, &between(3000, i), "c");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    let t = std::time::Instant::now();
    while !db.in_flight().1 && t.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        db.in_flight().1,
        "the merge did not start at the second piece's landing"
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
        // The count of builds below assumes no commit refreshes the snapshot on
        // its own; the backlog bound would, so it is off here.
        forms_settle_backlog_pct: 0,
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

/// The extension orders its batch by the keys' prefix words and finds
/// each key's place from the last one's, by a gallop where the batch is
/// sparse in the run and a walk where it is dense (`Snapshot::extend`).
/// Neither raises anything when wrong: a key put in the wrong gap of
/// the run is a scan out of order, and a tie the prefix words cannot
/// settle is a batch in insertion order. So both shapes are scanned
/// against the model: a batch of a dozen keys spread a hundred and fifty
/// run keys apart, extended onto the seal's snapshot of two thousand --
/// the gallop, doubling several times and searching inside its last
/// span -- then a batch of thousands, half of them alike through sixteen
/// bytes and written out of order, half between the run's keys, carried
/// forward by the walk. Keys of every batch are checked at every start.
fn extend_order_model(block_cache: bool) {
    let d = dir(&format!("extend-order-{block_cache}"));
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
    // The run: the seal's snapshot over a frozen table of two thousand
    // keys, the seal held short of its landing so the run stands.
    for k in (0..6000u32).step_by(3) {
        m.append(&mut db, &key(k), "f");
    }
    db.commit().unwrap();
    db.hold_seal_landing(true);
    db.seal().unwrap();
    assert!(db.in_flight().0, "a seal in flight");
    wait_for("the seal's snapshot", || {
        db.seal_snapshots() > 0 || !db.in_flight().0
    });
    let (b0, e0) = (db.snapshot_builds(), db.snapshot_extends());
    // Sparse: a key just above every four hundred and fiftieth run key.
    for k in (0..6000u32).step_by(450) {
        m.append(&mut db, &format!("{}x", key(k)), "s");
    }
    db.commit().unwrap();
    let mut sink = 0usize;
    db.scan(key(0).as_bytes(), 100, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(
        (db.snapshot_builds() - b0) + (db.snapshot_extends() - e0),
        1,
        "the sparse batch is one extension of the seal's snapshot, or one sort"
    );
    m.check(&db, "a sparse batch galloped into the run");
    // Dense, and more than a snapshot may lack before a scan carries it
    // forward: keys between the run's, and keys alike through sixteen
    // bytes in an order that is not theirs.
    let (b1, e1) = (db.snapshot_builds(), db.snapshot_extends());
    for k in (1..6000u32).step_by(3) {
        m.append(&mut db, &key(k), "d");
    }
    let tie = |i: u32| format!("tiekey-000000000{i:05}");
    let mut x = 7u32;
    for _ in 0..2600 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        m.append(&mut db, &tie((x >> 8) % 100_000), "t");
    }
    db.commit().unwrap();
    db.scan(key(0).as_bytes(), 100, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(
        db.snapshot_builds(),
        b1,
        "carried forward, not sorted again"
    );
    assert_eq!(db.snapshot_extends(), e1 + 1, "by one walk");
    m.check(
        &db,
        "a dense batch alike through sixteen bytes walked into the run",
    );
    db.hold_seal_landing(false);
}

#[test]
fn the_extension_gallops_a_sparse_batch_and_walks_a_dense_one_into_the_run() {
    extend_order_model(true);
}

#[test]
fn the_extension_orders_a_batch_alike_through_sixteen_bytes_on_the_merge_path() {
    extend_order_model(false);
}

/// The seal cap and the merge trigger follow the reads
/// (`Options::adaptive_cap`, `Options::adaptive_trigger`): a scan over
/// unsealed keys, or a point read that consults a piece, raises the lag
/// level to its full at the next commit, and the cap's share and the
/// trigger stand at their reading values; a scan over a drained store
/// raises nothing; and commits with no read over lag between decay the
/// level by their writes over the partitions' keys, back to the idle
/// values. A write-only stretch of the store's size relaxes the cadence
/// and one read over lag tightens it again.
#[test]
fn the_cap_and_the_trigger_follow_the_reads_and_relax_over_a_write_only_stretch() {
    let d = dir("adaptive-lag");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        seal_max_pct: 10,
        adaptive_cap: true,
        cap_reading_pct: 2,
        l0_trigger: 4,
        adaptive_trigger: true,
        trigger_reading: 2,
        lag_relax_pct: 100,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let key = |k: u32| format!("key-{k:06}");
    for k in 0..6000u32 {
        db.append(key(k).as_bytes(), b"p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    assert!(db.levels().0 > 1, "several partitions");
    assert_eq!(db.lag_level(), 0, "nothing has read over lag");
    assert_eq!(
        (db.cap_pct(), db.l0_trigger_now()),
        (10, 4),
        "the idle values"
    );
    // A scan over a drained store costs no lag, and counts for none.
    let mut sink = 0usize;
    db.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
        .unwrap();
    db.put(key(6000).as_bytes(), b"w");
    db.commit().unwrap();
    assert_eq!(db.lag_level(), 0, "a drained scan is not a read over lag");
    assert_eq!(db.lag_reads(), 0);
    // A scan over the key just written is; the next commit takes it.
    db.scan(key(5990).as_bytes(), 20, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(db.lag_reads(), 1);
    assert_eq!(
        db.lag_level(),
        0,
        "the level moves at the commit, not the read"
    );
    db.put(key(6001).as_bytes(), b"w");
    db.commit().unwrap();
    assert_eq!(db.lag_level(), 1000);
    assert_eq!(
        (db.cap_pct(), db.l0_trigger_now()),
        (2, 2),
        "the reading values"
    );
    // Writes with no read over lag between: the relax span is the
    // partitions' six thousand keys, so a thousand writes a commit take
    // a sixth of the level each, and seven commits take it to zero.
    let mut k = 7000u32;
    for round in 1..=7u32 {
        for _ in 0..1000 {
            db.put(key(k).as_bytes(), b"w");
            k += 1;
        }
        db.commit().unwrap();
        let level = db.lag_level();
        assert!(
            level <= 1000u32.saturating_sub(round * 166),
            "round {round}: level {level} has not decayed by the writes"
        );
    }
    assert_eq!(db.lag_level(), 0, "a store's worth of writes relaxes it");
    assert_eq!(
        (db.cap_pct(), db.l0_trigger_now()),
        (10, 4),
        "back to the idle values"
    );
    // A point read over a piece counts as a scan does. A seal alone
    // leaves its piece a piece -- the drain's promotion and the shaping
    // are a flush's and `adaptive_shape`'s -- so the read below meets it.
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.levels().1 > 0, "the seal left a piece");
    let mut got = 0usize;
    db.read_all(key(7100).as_bytes(), |v| got += v.len())
        .unwrap();
    assert_eq!(got, 1);
    assert_eq!(db.lag_reads(), 2, "a point read over a piece");
    db.put(key(k).as_bytes(), b"w");
    db.commit().unwrap();
    assert_eq!(db.lag_level(), 1000, "one read over lag tightens it again");
    db.close().unwrap();
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
    // A seal that is not landed, so the snapshot is built over a frozen
    // memtable and a live one both: held before its segments are written,
    // since a landing between the two scans below replaces the frozen
    // table and the snapshot with it, and the landing waits for no fsync
    // now, so it came inside the scans on a loaded machine.
    for k in (0..6000u32).step_by(6) {
        m.append(&mut db, &key(k), "f");
    }
    db.commit().unwrap();
    db.hold_seal_landing(true);
    db.seal().unwrap();
    assert!(db.in_flight().0, "a seal in flight");
    // The seal's own snapshot published before anything here makes one
    // (`Options::seal_snapshot`), so the first snapshot below is that
    // one extended over the live keys, or sorted from nothing where the
    // seal has landed already -- one either way. Left to race, a
    // snapshot sorted before the seal's publish was switched for the
    // seal's at the scan after, and counted twice.
    let t = std::time::Instant::now();
    while db.seal_snapshots() == 0 && db.in_flight().0 {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(30),
            "the seal published no snapshot"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let (b0, e0) = (db.snapshot_builds(), db.snapshot_extends());
    for k in (0..6000u32).step_by(12) {
        m.append(&mut db, &key(k), "l0");
    }
    db.commit().unwrap();
    let mut sink = 0usize;
    db.scan(key(0).as_bytes(), 6000, |_k, v| sink += v.len())
        .unwrap();
    let built = db.snapshot_builds();
    let extended = db.snapshot_extends();
    assert_eq!(
        (built - b0) + (extended - e0),
        1,
        "the first one is sorted, or extends the seal's"
    );
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
    assert_eq!(db.snapshot_extends(), extended + 1, "by one merge");
    m.check(&db, "a snapshot carried forward over a frozen memtable");
    db.hold_seal_landing(false);
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
        // The regime this test is about, which is the arm rather than the
        // default: the default maintains wherever anything has scanned,
        // since with the snapshot shared the forms cost the writer's own
        // reads nothing.
        forms_from_reader_scans: 1,
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

/// EXPERIMENT: the writer's own handle reads the canonical forms it
/// maintains, and declines them where it has staged writes they cannot
/// carry. Without `forms_to_writer` the maintenance is pure cost on a
/// store whose reads are the writer's own, which is the suite's ycsb-E.
#[test]
fn the_writers_own_handle_takes_the_forms_it_maintains() {
    for to_writer in [false, true] {
        let d = dir(if to_writer { "wforms-on" } else { "wforms-off" });
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(2 << 10),
            l0_trigger: 64,
            scan_block_cache: true,
            scan_cache_ahead: false,
            // Maintained for the writer alone, which is the case this is
            // about: with the default the writer's own scans start it too,
            // but only once a handle the caller made has scanned.
            forms_from_reader_scans: 0,
            forms_to_writer: to_writer,
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
        // A handle the forms are published for, a scan over the state so
        // the commit after it has something to maintain for, then writes,
        // then the commit that maintains.
        let _handle = db.reader().unwrap();
        db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        for k in (0..1500u32).step_by(5) {
            m.append(&mut db, &key(k), "vw");
        }
        db.commit().unwrap();
        assert!(db.canonical_forms().0 > 0, "the commit maintained forms");
        let before = db.form_takes();
        m.check(&db, "the writer over the forms it maintains");
        let took = db.form_takes() - before;
        if to_writer {
            assert!(took > 0, "the writer takes the forms it maintains");
        } else {
            assert_eq!(took, 0, "without the arm the writer builds its own");
        }
        // Staged past that commit: the forms are settled at it and cannot
        // carry this, and the writer's reads honour no watermark, so the
        // scan must build its own and must answer the staged value.
        m.append(&mut db, &key(7), "staged");
        let before = db.form_takes();
        m.check(&db, "the writer with a write staged past the forms");
        assert_eq!(
            db.form_takes() - before,
            0,
            "a form settled at the last commit cannot carry a staged write"
        );
        db.commit().unwrap();
        m.check(&db, "committed");
    }
}

/// EXPERIMENT: the seal capped by a share of the store, so a store
/// smaller than `seal_bytes` cannot hold the whole of itself unsealed.
/// The cap engages only once something has been sealed, since a store of
/// no bytes has no share to take, which is why a first load runs
/// uncapped.
#[test]
fn a_store_below_the_seal_floor_still_seals_once_it_has_bytes() {
    // Values big enough that a few thousand keys clear `SEAL_CAP_FLOOR`
    // without the test writing for a second.
    let val = "v".repeat(400);
    let key = |k: u32| format!("key-{k:06}");
    let run = |cap: usize| -> (usize, usize) {
        let d = dir(&format!("sealcap-{cap}"));
        let opts = Options {
            // Far above anything this test writes, so the cap is the only
            // thing that can seal it.
            seal_bytes: 32 << 20,
            seal_max_pct: cap,
            partition_bytes: Some(2 << 20),
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        for k in 0..9000u32 {
            db.append(key(k).as_bytes(), val.as_bytes());
            if k % 500 == 499 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let after_load = db.levels().0;
        // Updates to a store that now has bytes: the capped arm seals
        // through them, the uncapped one holds them all.
        for k in (0..9000u32).step_by(2) {
            db.append(key(k).as_bytes(), val.as_bytes());
            if k % 500 == 498 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        // The seal runs on its own thread and is published when a commit
        // finds it finished: counted before it is joined, the piece was
        // there or not by the disk's timing, one run in three.
        db.settle().unwrap();
        let sealed = db.levels().1;
        assert!(after_load > 0, "cap {cap}: the load left partitions");
        (after_load, sealed)
    };
    let (_, uncapped) = run(0);
    let (_, capped) = run(10);
    assert_eq!(
        uncapped, 0,
        "uncapped, the memtable holds every update: it is far below `seal_bytes`"
    );
    assert!(
        capped > 0,
        "capped, the updates seal once they pass a share of the store; got {capped}"
    );
}

/// The seal is sized by the key and value bytes the store holds, which
/// every segment records in its superblock, and not by the partitions'
/// file bytes, which move with the record format: the same writes in
/// compact records and in full ones seal at one threshold, and a reopened
/// store reads the recorded payload back rather than estimating it. The
/// load goes through ordered ingest's segment and the updates through the
/// WAL, a seal and a merge, so each writer of a partition is asked.
#[test]
fn the_seal_is_sized_by_the_data_and_not_by_the_file() {
    let val = "v".repeat(100);
    let key = |k: u32| format!("{k:016}");
    // Enough that a tenth of the store clears `SEAL_CAP_FLOOR`, so the
    // threshold is the cap's and moves with the size it is taken from.
    let n = 80_000u32;
    let run = |compact: bool, on_file: bool| -> (u64, u64, usize) {
        let d = dir(&format!("sealdata-{compact}-{on_file}"));
        let opts = Options {
            segment: supdb::SegmentOptions {
                compact_records: compact,
                ..Default::default()
            },
            seal_on_file: on_file,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts.clone()).unwrap();
        for k in 0..n {
            db.append(key(k).as_bytes(), val.as_bytes());
            if k % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        for k in (0..n).step_by(4) {
            db.append(key(k).as_bytes(), val.as_bytes());
            if k % 1000 == 996 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let (file, data) = db.sized_bytes();
        let threshold = db.seal_threshold();
        drop(db);
        let db = Db::open(&d, opts).unwrap();
        assert_eq!(
            db.sized_bytes(),
            (file, data),
            "compact {compact}, file rule {on_file}: a reopened store reads the sizes back"
        );
        assert_eq!(db.seal_threshold(), threshold);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        (file, data, threshold)
    };
    let (full_file, full_data, full_t) = run(false, false);
    let (compact_file, compact_data, compact_t) = run(true, false);
    let (_, _, full_on_file) = run(false, true);
    let (_, _, compact_on_file) = run(true, true);
    let per = (16 + val.len()) as u64;
    let want = n as u64 * per + (n as u64).div_ceil(4) * per;
    assert_eq!(full_data, want, "the payload is every key and value put");
    assert_eq!(compact_data, want);
    assert!(
        compact_file < full_file,
        "compact records are the smaller file: {compact_file} against {full_file}"
    );
    assert_eq!(
        compact_t, full_t,
        "sized by the data, the format does not move the seal"
    );
    assert!(
        compact_on_file < full_on_file,
        "sized by the file, the denser format seals sooner: {compact_on_file} against {full_on_file}"
    );
}

/// EXPERIMENT: a write burst with no read between its commits is settled
/// into the forms once its unfiled backlog passes `forms_settle_backlog_pct`
/// of the store's keys, so the first read after the burst does not file
/// the whole of it. `forms_position` is the log position the forms are
/// settled to: a commit that maintained them moves it, one that did not
/// leaves it where it was.
#[test]
fn a_write_burst_is_settled_once_its_backlog_passes_the_bound() {
    let run = |pct: usize| -> (bool, ScanModel, Db) {
        let d = dir(&format!("settle-backlog-{pct}"));
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(2 << 10),
            l0_trigger: 64,
            scan_block_cache: true,
            scan_cache_ahead: false,
            forms_from_reader_scans: 0,
            forms_settle_backlog_pct: pct,
            // The bound alone: the recency window is the next test's.
            forms_settle_recent_pct: 0,
            // The commit's own rules, whose deferred burst the upkeep
            // thread would file ahead of the read.
            upkeep: supdb::Upkeep::Inline,
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
        // A handle the forms are published for, and one scan and a
        // commit, so the forms exist and are current.
        let _handle = db.reader().unwrap();
        let mut sink = 0usize;
        db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        db.commit().unwrap();
        let before = db.forms_position();
        assert_ne!(before, usize::MAX, "maintained once");
        // The burst: three batches of 400 writes, committed, with no read
        // between them. Five percent of 1,500 keys is 75, so every batch
        // is past the bound.
        for batch in 0..3u32 {
            for k in (batch..1500u32).step_by(4).take(400) {
                m.append(&mut db, &key(k), "burst");
            }
            db.commit().unwrap();
        }
        (db.forms_position() != before, m, db)
    };
    let (moved, m0, db0) = run(0);
    assert!(
        !moved,
        "with no bound, the forms stay where the last read left them"
    );
    m0.check(&db0, "unbounded, after the burst");
    let (moved, m1, db1) = run(5);
    assert!(
        moved,
        "past the bound, the burst's commits settle as they go"
    );
    m1.check(&db1, "bounded, after the burst");
}

/// The settle bound is a share of the partitions' keys. A level-0 piece
/// sealed from updates holds keys a partition holds already, and summed
/// with them the bound grew with every seal, until a batch past the share
/// the bound meant no longer crossed it and was left for the first read
/// to file. `forms_settle_keys_all` is the old sum.
#[test]
fn the_settle_bound_is_a_share_of_the_partitions_keys() {
    let run = |all: bool| -> (bool, ScanModel, Db) {
        let d = dir(&format!("settle-keys-{all}"));
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(2 << 10),
            l0_trigger: 64,
            scan_block_cache: true,
            scan_cache_ahead: false,
            forms_from_reader_scans: 0,
            forms_settle_backlog_pct: 5,
            forms_settle_recent_pct: 0,
            forms_settle_keys_all: all,
            // The commit's own rules; see the test above.
            upkeep: supdb::Upkeep::Inline,
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
        // Every key written again and sealed into pieces the partitions
        // keep beside them: the store still holds 1,500 keys, and its
        // segments 3,000.
        for k in 0..1500u32 {
            m.append(&mut db, &key(k), "v1");
        }
        db.commit().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
        let (parts, pieces) = db.levels();
        assert!(
            parts > 1 && pieces > 0,
            "partitions and pieces: {parts}, {pieces}"
        );
        let _handle = db.reader().unwrap();
        let mut sink = 0usize;
        db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        db.commit().unwrap();
        let before = db.forms_position();
        assert_ne!(before, usize::MAX, "maintained once");
        // One batch of 100 writes and no read: past five percent of the
        // partitions' 1,500 keys, short of five percent of 3,000.
        for k in (0..1500u32).step_by(15) {
            m.append(&mut db, &key(k), "burst");
        }
        db.commit().unwrap();
        (db.forms_position() != before, m, db)
    };
    let (moved, m, db) = run(false);
    assert!(
        moved,
        "a batch past the share of the store's keys settles at its commit"
    );
    m.check(&db, "the bound on the partitions' keys");
    let (moved, m, db) = run(true);
    assert!(
        !moved,
        "summed over the pieces too, the same batch falls short of the bound"
    );
    m.check(&db, "the bound on every segment's keys");
}

/// A block whose overlay outgrows every form but the wide one -- the
/// last block of the last partition, which collects every key inserted
/// past the end -- read through the canonical forms by a handle the
/// caller made. The writer drops its own form for such a block and
/// builds it wide, a form it never publishes, so what the reader finds
/// in the table for that block is whatever was published before or
/// nothing; either must send it to build for itself, never to walk the
/// block as clean.
#[test]
fn a_reader_meets_a_block_gone_wide_through_the_forms() {
    let d = dir("forms-wide");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_settle_backlog_pct: 0,
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
    let mut sink = 0usize;
    // A handle's scan, so the writer maintains the forms from here on.
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    // A few keys in the last block first, so it has a published form to
    // go stale, then keys past the end until the block is wide, a
    // handle scanning between the commits so each one maintains.
    for k in (1400..1500u32).step_by(10) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(1400).as_bytes(), 100, |_k, v| sink += v.len())
        .unwrap();
    for batch in 0..5u32 {
        for k in 0..100u32 {
            m.append(&mut db, &key(2000 + batch * 100 + k), "past");
        }
        db.commit().unwrap();
        let r = db.reader().unwrap();
        r.scan(key(1400).as_bytes(), 50, |_k, v| sink += v.len())
            .unwrap();
    }
    db.commit().unwrap();
    std::hint::black_box(sink);
    let (forms, _, _, _) = db.canonical_forms();
    assert!(forms > 0, "the forms are maintained");
    let r = db.reader().unwrap();
    m.check(&r, "a reader through the forms, the last block wide");
    m.check(&db, "the writer over its own forms");
}

/// EXPERIMENT: the canonical forms survive a seal. A form's content is
/// the merged block, which a seal does not change; what the seal changes
/// is what the form was resolved against, and that is remade. Held to
/// the model with a burst the forms were not settled to before the seal,
/// a delete, a block gone wide, a handle taking the carried forms, the
/// writer over its carried tables, writes settled into them after the
/// seal, and the merge that finally drops them.
/// A merge that folds updates into a partition rewrites it over the same
/// keys, and the copies built over the old partition are the new one's
/// blocks as they were the old one's: carried rather than dropped, which
/// was a pass that rebuilt every block after the fully-unmerged burst.
/// The sparse forms splice deltas at the old partition's ranks and are
/// dropped. Every answer is held to the model, through a reader handle
/// and through the writer, before and after writes into the carried
/// copies; and the path is asserted taken.
#[test]
fn the_copies_survive_a_merge_that_keeps_every_key() {
    // Over both record forms: the merge rewrites the partition, and a
    // compact record's extent is rebuilt at every read of it.
    for compact in [false, true] {
        the_copies_survive_a_merge_with(compact);
    }
}

fn the_copies_survive_a_merge_with(compact: bool) {
    let d = dir(if compact {
        "forms-rebase-compact"
    } else {
        "forms-rebase"
    });
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(4 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_settle_backlog_pct: 0,
        commit_forms: true,
        forms_carry: true,
        forms_rebase: true,
        segment: supdb::SegmentOptions {
            compact_records: compact,
            ..Default::default()
        },
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..3000u32 {
        m.append(&mut db, &key(k), "v0");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    let parts = db.levels().0;
    assert!(parts > 1, "several partitions");
    let mut sink = 0usize;
    // Every key of the first half updated, so its blocks are copies, and
    // a few of the second half, so those are sparse; a handle's scan so
    // the forms are maintained, and the pieces the seal leaves.
    for k in 0..1500u32 {
        m.append(&mut db, &key(k), "v1");
    }
    for k in (1500..3000u32).step_by(40) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 3000, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    let (held, _, _, _) = db.canonical_forms();
    assert!(held > 0, "forms held before the merge: {held}");
    // The merge: the flush folds the pieces into their partitions, and
    // no key comes or goes.
    let rebased0 = db.forms_rebased();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert_eq!(db.levels(), (parts, 0), "the pieces merged");
    let carried0 = db.forms_rebased() - rebased0;
    assert!(carried0 > 0, "the forms were carried across the merge");
    let (after, _, _, _) = db.canonical_forms();
    assert!(after > 0, "copies survived the merge: {after}");
    let r = db.reader().unwrap();
    m.check(&r, "a reader over copies carried across a merge");
    m.check(&db, "the writer over them");
    // Writes into the carried copies, a scan so they are settled, and a
    // delete, which a copy must drop.
    for k in (0..3000u32).step_by(9) {
        m.append(&mut db, &key(k), "after");
    }
    m.delete(&mut db, &key(33));
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    let r = db.reader().unwrap();
    m.check(&r, "a reader after writes into the carried copies");
    m.check(&db, "the writer after them");
    // A merge that loses a key cuts different blocks, and that
    // partition's forms start afresh; the others, rewritten over the same
    // keys by the same merge, are carried each on its own.
    let rebased1 = db.forms_rebased();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert_eq!(
        db.forms_rebased() - rebased1,
        carried0 - 1,
        "the partition that deleted a key is not carried, and only it"
    );
    m.check(&db, "after a merge that deleted a key");
    std::hint::black_box(sink);
}

#[test]
fn the_forms_survive_a_seal() {
    // And with `snapshot_carry`, which carries the scan snapshot across
    // the same publishes rather than dropping it: the merge's snapshot
    // whole and the freeze's live entries made frozen ones. A snapshot
    // carried wrong answers a scan with keys the seal has taken, or
    // loses the ones its lists held, so the model check below is what
    // holds it.
    for snapshot_carry in [false, true] {
        the_forms_survive_a_seal_with(snapshot_carry);
    }
}

fn the_forms_survive_a_seal_with(snapshot_carry: bool) {
    let d = dir(if snapshot_carry {
        "forms-carry-snap"
    } else {
        "forms-carry"
    });
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_settle_backlog_pct: 0,
        commit_forms: true,
        forms_carry: true,
        snapshot_carry,
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
    // Overlay in most blocks and keys past the end enough to make the
    // last block wide, a handle's scan so the forms are maintained.
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    for k in 0..300u32 {
        m.append(&mut db, &key(2000 + k), "past");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 2000, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    let (forms, _, _, _) = db.canonical_forms();
    assert!(forms > 0, "maintained");
    // A burst the forms are not settled to -- no scan between these
    // commits and the bound is off -- then the seal, which files it.
    for k in (2..1500u32).step_by(7) {
        m.append(&mut db, &key(k), "burst");
    }
    db.commit().unwrap();
    m.delete(&mut db, &key(10));
    db.commit().unwrap();
    assert_ne!(
        db.forms_position(),
        usize::MAX,
        "maintained before the seal"
    );
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.levels().1 > 0, "the seal left a piece");
    let (after, _, _, _) = db.canonical_forms();
    assert!(after > 0, "the forms survived the seal: {after}");
    // The publish carried them and vouches for no commit: the writer
    // does, when it next publishes into the new state -- here for the
    // handle it is asked for.
    assert_eq!(
        db.forms_position(),
        usize::MAX,
        "carried, and current to no commit until the writer says so"
    );
    // A handle takes them, and finds the burst and the delete in them.
    let r = db.reader().unwrap();
    assert_eq!(
        db.forms_position(),
        0,
        "current to the new log, which holds nothing yet"
    );
    let (_, hit0) = db.canonical_tries();
    m.check(&r, "a reader over the carried forms");
    let (_, hit1) = db.canonical_tries();
    assert!(hit1 > hit0, "the reader took the carried forms");
    m.check(&db, "the writer over its carried tables");
    // Writes after the seal land in the new memtable and are settled
    // into the carried forms at the commit a scan precedes.
    for k in (3..1500u32).step_by(11) {
        m.append(&mut db, &key(k), "after");
    }
    m.delete(&mut db, &key(2005));
    for k in 300..320u32 {
        m.append(&mut db, &key(2000 + k), "past");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    let r = db.reader().unwrap();
    m.check(&r, "a reader after writes into the carried forms");
    m.check(&db, "the writer after them");
    // A second seal, from a store with a piece already.
    for k in (4..1500u32).step_by(13) {
        m.append(&mut db, &key(k), "again");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.levels().1 > 1, "two pieces");
    let (twice, _, _, _) = db.canonical_forms();
    assert!(twice > 0, "carried again: {twice}");
    let r = db.reader().unwrap();
    m.check(&r, "a reader after the second seal");
    m.check(&db, "the writer after the second seal");
    std::hint::black_box(sink);
    // A merge changes the partitions: the table starts afresh.
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert_eq!(
        db.forms_position(),
        usize::MAX,
        "a merge starts the table afresh"
    );
    m.check(&db, "after the merge");
}

/// A seal that empties the live table, then a run of keys above the
/// store's greatest, which the writer takes into an ordered table for
/// direct ingest: the writer's forms survive the switch as they survive
/// the seal, so the scans after the piece lands build nothing. Left to
/// the next look at the log, the rebase found a live table it did not
/// know and dropped every form, and the scans built every block.
#[test]
fn the_forms_survive_a_direct_runs_start() {
    let d = dir("forms-carry-direct");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_settle_backlog_pct: 0,
        commit_forms: true,
        forms_carry: true,
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
    // Overlay in most blocks, a handle's scan so the forms are
    // maintained, then the seal, which files the overlay, carries the
    // forms across and leaves the live table empty.
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 2000, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    let (forms, _, _, _) = db.canonical_forms();
    assert!(forms > 0, "maintained");
    db.seal().unwrap();
    // Keys above the store's greatest into the empty table: a direct
    // run, which replaces the live table with an ordered one.
    for k in 0..200u32 {
        m.append(&mut db, &key(2000 + k), "run");
    }
    db.commit().unwrap();
    // The seal's piece lands on the segment work's thread; `settle`
    // would join it, but a flush would close the run too.
    let t = std::time::Instant::now();
    while db.levels().1 == 0 && t.elapsed() < std::time::Duration::from_secs(30) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(db.levels().1 > 0, "the seal landed a piece");
    let built0 = db.blocks_built().1;
    m.check(&db, "the writer after the switch and the landing");
    let built = db.blocks_built().1 - built0;
    assert!(
        built <= 2,
        "the writer's tables survived the switch: {built} blocks built by the scans after it"
    );
    let (after, _, _, _) = db.canonical_forms();
    assert!(after > 0, "the forms survived: {after}");
    m.check(&r, "a reader after the run");
    db.settle().unwrap();
    m.check(&db, "the writer after the settle");
    db.close().unwrap();
    std::hint::black_box(sink);
}

/// EXPERIMENT: a publish starts the builder and the next commit installs
/// what it posted, with no scan asking: after a seal the table is empty,
/// and with `build_ahead_on_publish` a commit with no scan before it
/// fills it. Held to the model through a handle and the writer.
#[test]
fn a_publish_starts_the_builder_and_a_commit_installs_its_forms() {
    let d = dir("ahead-publish");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        // The builder on a store this small.
        scan_cache_ahead_min_blocks: 0,
        forms_settle_backlog_pct: 0,
        commit_forms: true,
        build_ahead_on_publish: true,
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
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    // A handle the forms are published for. The seal: a publish, and
    // `settle` joins the builder it started.
    let _handle = db.reader().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert_eq!(db.canonical_forms().0, 0, "the seal emptied the table");
    // A commit with no scan before it: the builder's forms installed and
    // published, the batch settled into them.
    for k in (1..1500u32).step_by(11) {
        m.append(&mut db, &key(k), "v2");
    }
    db.commit().unwrap();
    let (forms, _, _, _) = db.canonical_forms();
    assert!(
        forms > 0,
        "the commit installed the builder's forms: {forms}"
    );
    assert_ne!(db.forms_position(), usize::MAX, "and published them");
    let r = db.reader().unwrap();
    m.check(&r, "a reader over the installed forms");
    m.check(&db, "the writer over its own");
}

/// EXPERIMENT: the forms are published for a handle and not before.
/// Commits with no handle live leave the table where it was, so a
/// handle at that commit still finds its forms; a claim files the
/// backlog and publishes everything dirty, at once when nothing is
/// staged and at the next commit otherwise. Held to the model through
/// the handle and the writer at each step.
#[test]
fn the_forms_are_published_when_a_handle_asks() {
    let d = dir("publish-lazily");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_settle_backlog_pct: 0,
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
    let mut sink = 0usize;
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    // A handle's scan and a commit: maintained and, with the handle
    // live, published.
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    let at_first = db.forms_position();
    assert_ne!(at_first, usize::MAX, "published for the handle");
    assert!(db.canonical_forms().0 > 0);
    drop(r);
    // Writes and commits with no handle live: settled at the commits a
    // scan precedes, published for nobody, the table left where it was.
    for round in 0..3u32 {
        for k in (round..1500u32).step_by(7) {
            m.append(&mut db, &key(k), "quiet");
        }
        db.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
            .unwrap();
        db.commit().unwrap();
    }
    assert_eq!(
        db.forms_position(),
        at_first,
        "nothing published with no handle to publish for"
    );
    m.check(&db, "the writer over its own forms, unpublished");
    // A claim: the backlog filed and the forms published at the log's
    // length, which the handle reads at.
    let r = db.reader().unwrap();
    let published = db.forms_position();
    assert!(
        published != usize::MAX && published > at_first,
        "published at the claim: {published} after {at_first}"
    );
    let (_, hit0) = db.canonical_tries();
    m.check(&r, "a handle over the forms published at its claim");
    let (_, hit1) = db.canonical_tries();
    assert!(hit1 > hit0, "the handle took them");
    m.check(&db, "the writer beside it");
    drop(r);
    // Staged writes when a handle is claimed: nothing published until
    // the commit, which then publishes whether or not a scan preceded.
    for k in (3..1500u32).step_by(11) {
        m.append(&mut db, &key(k), "staged");
    }
    db.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
        .unwrap();
    let r = db.reader().unwrap();
    assert_eq!(
        db.forms_position(),
        published,
        "staged writes: not published at the claim"
    );
    db.commit().unwrap();
    assert!(
        db.forms_position() > published,
        "published at the commit after the claim"
    );
    let r2 = db.reader().unwrap();
    m.check(&r2, "a handle after the commit, over the forms");
    m.check(&r, "the earlier handle, after the commit");
    m.check(&db, "the writer");
    std::hint::black_box(sink);
}

/// A table the commit's fill made is one the writes after it are filed
/// into. The writer's first table over a state was made by its own scan
/// until the forms were maintained at commit; made by the commit's fill
/// instead -- a handle's scan asked for the maintenance, not the
/// writer's -- the flag that files writes into the tables stayed off,
/// and every write after went unfiled for the rest of the state: the
/// writer's own scan answered key 0 short of the value it had just
/// written, while the point read beside it answered it, and the forms
/// it published carried the same hole.
#[test]
fn a_table_the_commit_filled_takes_the_writes_after_it() {
    let d = dir("commit-filled-table");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_settle_backlog_pct: 0,
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
    let mut sink = 0usize;
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    // A handle's scan, not the writer's, and the commit that maintains:
    // the writer's tables are the commit's fill.
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    db.commit().unwrap();
    assert!(db.canonical_forms().0 > 0, "maintained");
    // A write staged, the writer's own scan, and the commit: the scan
    // and the read agree at each step, and a handle after the commit.
    m.append(&mut db, &key(0), "after");
    m.check(&db, "the writer with a write staged past the fill");
    db.commit().unwrap();
    m.check(&db, "the writer after the commit");
    let r = db.reader().unwrap();
    m.check(&r, "a handle over the forms after the commit");
    std::hint::black_box(sink);
}

/// A handle claimed after a write burst adopts the writer's snapshot
/// rather than building one, and its first read files nothing from the
/// log: the claim brings the writer's snapshot current and publishes
/// it. Before this the published snapshot was whatever the writer last
/// built -- at ten thousand keys the empty one the drained scan pass
/// made -- and every handle claimed after the mixes sorted the unsealed
/// keys itself, 70 µs of a first scan whose steady successors take two.
#[test]
fn a_handle_claimed_after_a_burst_adopts_the_writers_snapshot() {
    let d = dir("adopt-at-claim");
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
    let mut sink = 0usize;
    // The writer scans the drained store, then a burst: its snapshot is
    // the empty one and the burst's keys are filed by block.
    db.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
        .unwrap();
    for k in (0..1500u32).step_by(3) {
        m.append(&mut db, &key(k), "burst");
    }
    db.commit().unwrap();
    let builds = db.snapshot_builds();
    // The claim brings the writer's snapshot current; the handle's scans
    // build none of their own.
    let r = db.reader().unwrap();
    let at_claim = db.snapshot_builds() + db.snapshot_extends();
    assert!(at_claim > builds, "the claim brought the snapshot current");
    r.scan(key(0).as_bytes(), 200, |_k, v| sink += v.len())
        .unwrap();
    r.scan(key(700).as_bytes(), 200, |_k, v| sink += v.len())
        .unwrap();
    assert_eq!(
        db.snapshot_builds() + db.snapshot_extends(),
        at_claim,
        "the handle adopted the writer's snapshot"
    );
    m.check(&r, "a handle over the adopted snapshot");
    m.check(&db, "the writer");
    // Staged writes at a claim: the snapshot is not moved past them, and
    // the handle before the commit sees none of them.
    for k in (1..1500u32).step_by(7) {
        m.append(&mut db, &key(k), "staged");
    }
    let r2 = db.reader().unwrap();
    let mut got = Vec::new();
    r2.scan(key(1).as_bytes(), 1, |k, v| {
        if k == key(1).as_bytes() {
            got.push(v.to_vec());
        }
    })
    .unwrap();
    assert_eq!(got, vec![b"v0".to_vec()], "a handle sees nothing staged");
    db.commit().unwrap();
    let r3 = db.reader().unwrap();
    m.check(&r3, "a handle after the commit");
    m.check(&db, "the writer after the commit");
    std::hint::black_box(sink);
}

/// EXPERIMENT: the backlog bound settles a burst only while the store
/// was scanned within `forms_settle_recent_pct` of its keys written
/// since. Filing costs the same at the commit and at the next read, and
/// the forms are discarded at the next seal, so a burst far from any
/// scan is left for the read that wants it and a burst near one is filed
/// as it goes. `forms_position` is the log position the forms are
/// settled to: a commit that maintained them moves it.
#[test]
fn a_burst_is_settled_by_its_backlog_only_near_a_scan() {
    let d = dir("settle-recent");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_from_reader_scans: 0,
        // Five percent of 1,500 keys is 75 writes, a tenth is 150.
        forms_settle_backlog_pct: 5,
        forms_settle_recent_pct: 10,
        // The commit's own rules, whose deferred burst the upkeep thread
        // would file ahead of the read.
        upkeep: supdb::Upkeep::Inline,
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
    let scan = |db: &Db| {
        let mut sink = 0usize;
        db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        std::hint::black_box(sink);
    };
    // A handle the forms are published for, and one scan and a commit,
    // so the forms exist and are current.
    let _handle = db.reader().unwrap();
    scan(&db);
    db.commit().unwrap();
    let at_scan = db.forms_position();
    assert_ne!(at_scan, usize::MAX, "maintained once");
    // A hundred writes, within a tenth of the store of that scan and past
    // the bound: filed at their commit.
    for k in (0..1500u32).step_by(15).take(100) {
        m.append(&mut db, &key(k), "near");
    }
    db.commit().unwrap();
    let near = db.forms_position();
    assert!(
        near > at_scan,
        "near a scan, a batch past the bound is filed at its commit: {near} <= {at_scan}"
    );
    // A hundred more, two hundred since the scan: past the window, left
    // where they are.
    for k in (1..1500u32).step_by(15).take(100) {
        m.append(&mut db, &key(k), "far");
    }
    db.commit().unwrap();
    let far = db.forms_position();
    assert_eq!(
        far, near,
        "far from a scan, a batch past the bound is left for the next read"
    );
    // The next read files it, as it always did.
    scan(&db);
    db.commit().unwrap();
    assert!(
        db.forms_position() > far,
        "the read after the burst files it"
    );
    m.check(&db, "after the burst");
}

/// Pieces over one partition's range are merged into one piece beside the
/// partition merge, carrying their tombstones: a store that tiers and one
/// that does not, driven identically -- a load, then rounds of appends and
/// deletes each sealed with no partition merge due -- answer every key and
/// every scan alike, the tiering store holds fewer pieces, and a reopen of
/// it answers the same.
#[test]
fn pieces_merge_into_a_piece_and_carry_their_tombstones() {
    // Once with the partition merge out of the way, so every seal's piece
    // waits for the piece merge, and once with both merges due every few
    // seals, since a partition merge that took a piece sealed after a
    // piece merge's inputs folded newer values under older ones.
    for (tier, l0) in [(2usize, 1000usize), (2, 3)] {
        pieces_merge_case(tier, l0);
    }
}

fn pieces_merge_case(tier_pieces: usize, l0_trigger: usize) {
    let opts = |tier: usize, l0: usize| Options {
        seal_bytes: 1 << 20,
        partition_on_flush: true,
        l0_trigger: l0,
        tier_pieces: tier,
        scan_block_cache: true,
        commit_forms: true,
        ..Options::default()
    };
    let key = |i: u64| format!("{i:016}");
    let (da, dbd) = (
        dir(&format!("tier-a-{l0_trigger}")),
        dir(&format!("tier-b-{l0_trigger}")),
    );
    let mut a = Db::create(&da, opts(tier_pieces, l0_trigger)).unwrap();
    let mut b = Db::create(&dbd, opts(0, 1000)).unwrap();
    let n = 3000u64;
    for db in [&mut a, &mut b] {
        for i in 0..n {
            db.append(key(i).as_bytes(), format!("v0-{i}").as_bytes());
        }
        db.flush().unwrap();
        assert_eq!(db.levels(), (1, 0));
    }
    // The rounds' operations decided once, applied to both.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for round in 1..=6 {
        let ops: Vec<(u64, bool)> = (0..400).map(|j| (next() % n, j % 5 == 0)).collect();
        for db in [&mut a, &mut b] {
            for &(k, delete) in &ops {
                if delete {
                    db.delete(key(k).as_bytes());
                } else {
                    db.append(key(k).as_bytes(), format!("v{round}-{k}").as_bytes());
                }
            }
            db.seal().unwrap();
        }
    }
    a.settle().unwrap();
    b.settle().unwrap();
    assert_eq!(b.levels(), (1, 6), "every seal left a piece");
    assert!(a.levels().1 < 6, "the pieces were merged: {:?}", a.levels());
    let opts = |tier: usize| opts(tier, l0_trigger);
    let scan_all = |db: &Db| {
        let mut out = Vec::new();
        db.scan(b"", usize::MAX, |k, v| out.push((k.to_vec(), v.to_vec())))
            .unwrap();
        out
    };
    let want = scan_all(&b);
    assert!(want.len() > n as usize, "appends kept: {}", want.len());
    assert_eq!(scan_all(&a), want);
    for i in 0..n {
        assert_eq!(
            read_vec(&a, key(i).as_bytes()),
            read_vec(&b, key(i).as_bytes()),
            "key {i}"
        );
    }
    drop(a);
    let a = Db::open(&da, opts(2)).unwrap();
    assert_eq!(scan_all(&a), want, "after a reopen");
    for i in 0..n {
        assert_eq!(
            read_vec(&a, key(i).as_bytes()),
            read_vec(&b, key(i).as_bytes()),
            "key {i} after a reopen"
        );
    }
}

/// The scan snapshot carries each unsealed key's chain as a run in key
/// order, and a read of a run answers as a read of the chain does: a
/// tombstone in it cuts every older source, a chunk past a handle's
/// commit is not there, a frozen table's run is whole, and a key written
/// again after the copy is read from its chain. Held on the block path
/// and on the merge path, through the writer and through a handle under
/// `Latest`, across a seal left in flight.
#[test]
fn the_snapshots_runs_answer_as_the_chains_do() {
    for (block_cache, keeper) in [(true, false), (false, false), (true, true), (false, true)] {
        let d = dir(&format!("snap-runs-{block_cache}-{keeper}"));
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(2 << 10),
            scan_block_cache: block_cache,
            scan_cache_ahead: false,
            share_snapshot: true,
            snapshot_runs: true,
            // With the keeper extending the published snapshot beside
            // every step below, so the reads meet its versions too.
            snapshot_keeper: keeper,
            snapshot_keeper_recent_pct: 0,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let mut m = ScanModel::default();
        let key = |k: u32| format!("key-{k:06}");
        for k in 0..3000u32 {
            m.append(&mut db, &key(k), "p");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        m.flushed();
        assert!(db.levels().0 > 1, "several partitions");
        // Updates as the suite makes them, a delete and an append; then a
        // seal left in flight, so the runs are copied from a frozen table
        // and a live one both.
        for k in (0..3000u32).step_by(2) {
            m.delete(&mut db, &key(k));
            m.append(&mut db, &key(k), "u1");
        }
        db.commit().unwrap();
        db.seal().unwrap();
        for k in (0..3000u32).step_by(4) {
            m.delete(&mut db, &key(k));
            m.append(&mut db, &key(k), "u2");
        }
        for k in (1..3000u32).step_by(8) {
            m.append(&mut db, &key(k), "a2");
        }
        db.commit().unwrap();
        let reader = db.reader().unwrap();
        let mut sink = 0usize;
        db.scan(key(0).as_bytes(), 3000, |_k, v| sink += v.len())
            .unwrap();
        m.check(
            &db,
            &format!("cache {block_cache}: the writer over the runs"),
        );
        m.check(
            &reader,
            &format!("cache {block_cache}: a handle over the runs"),
        );
        // Writes after the copy, over keys the runs hold and over new
        // ones, staged: the handle at its commit sees none of them and
        // the writer sees them all, from the chains.
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
        let before_12 = scan_one(&reader, 12);
        let before_17 = scan_one(&reader, 17);
        assert_eq!(before_12, vec![b"u2".to_vec()]);
        assert_eq!(before_17, vec![b"p".to_vec(), b"a2".to_vec()]);
        for k in (0..3000u32).step_by(6) {
            m.delete(&mut db, &key(k));
            m.append(&mut db, &key(k), "u3");
        }
        for k in (1..3000u32).step_by(16) {
            m.append(&mut db, &key(k), "a3");
        }
        assert_eq!(
            scan_one(&db, 12),
            vec![b"u3".to_vec()],
            "the writer sees its staged delete"
        );
        assert_eq!(
            scan_one(&db, 17),
            vec![b"p".to_vec(), b"a2".to_vec(), b"a3".to_vec()],
            "the writer sees its staged append"
        );
        assert_eq!(
            scan_one(&reader, 12),
            before_12,
            "staged, so unseen under Latest"
        );
        assert_eq!(
            scan_one(&reader, 17),
            before_17,
            "staged, so unseen under Latest"
        );
        db.commit().unwrap();
        m.check(
            &db,
            &format!("cache {block_cache}: the writer after writes over copied runs"),
        );
        m.check(
            &reader,
            &format!("cache {block_cache}: a handle after the commit of writes over copied runs"),
        );
        // A handle claimed now adopts the writer's snapshot, runs and all.
        let later = db.reader().unwrap();
        m.check(
            &later,
            &format!("cache {block_cache}: a handle claimed after"),
        );
        db.settle().unwrap();
        m.check(&db, &format!("cache {block_cache}: after the seal lands"));
        m.check(
            &reader,
            &format!("cache {block_cache}: a handle after the seal lands"),
        );
        drop(reader);
        drop(later);
        drop(db);
        let db = Db::open(&d, Options::default()).unwrap();
        m.check(&db, &format!("cache {block_cache}: reopened"));
    }
}

/// The run keeper: dormant until a scan, then every commit's batch is
/// appended to the published snapshot and its runs, so the scans after a
/// burst adopt a current snapshot and sort nothing; carried across a
/// seal's freeze and its landing; and past its bound of writes since the
/// last scan, dormant again, the scan bringing the rest itself.
#[test]
fn the_run_keeper_keeps_the_published_snapshot_current() {
    for (block_cache, pct) in [(true, 0), (false, 0), (true, 100), (false, 100)] {
        let d = dir(&format!("snap-keeper-{block_cache}-{pct}"));
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(2 << 10),
            scan_block_cache: block_cache,
            scan_cache_ahead: false,
            share_snapshot: true,
            snapshot_runs: true,
            snapshot_keeper: true,
            snapshot_keeper_recent_pct: pct,
            // The seal below is held in flight across a burst of writes,
            // which only the writer driving its own landings can promise:
            // on a thread of its own the landing may fall between the
            // keeper's settle and the scan after it, and the landing's
            // carry is checked apart, after the settle that lands it.
            publish_in_background: false,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let mut m = ScanModel::default();
        let key = |k: u32| format!("key-{k:06}");
        for k in 0..3000u32 {
            m.append(&mut db, &key(k), "p");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        m.flushed();
        assert!(db.levels().0 > 1, "several partitions");
        // Before any scan over the store the keeper is dormant: a
        // commit does not wake it, and nothing is kept.
        for k in (0..3000u32).step_by(3) {
            m.delete(&mut db, &key(k));
            m.append(&mut db, &key(k), "u0");
        }
        db.commit().unwrap();
        assert_eq!(
            db.snapshot_kept().0,
            0,
            "{block_cache} {pct}: dormant before a scan"
        );
        m.check(
            &db,
            &format!("{block_cache} {pct}: the writer before the keeper"),
        );
        let builds = db.snapshot_builds();
        // A burst after the scan: updates over the run's keys and new
        // keys, in rounds the keeper follows commit by commit.
        for round in 0..10u32 {
            for k in (round..3000).step_by(10) {
                m.delete(&mut db, &key(k));
                m.append(&mut db, &key(k), &format!("u{round}"));
            }
            for k in 3000 + round * 50..3000 + round * 50 + 50 {
                m.append(&mut db, &key(k), "n");
            }
            db.commit().unwrap();
        }
        db.settle_keeper();
        let (kept, _) = db.snapshot_kept();
        assert!(
            kept > 0,
            "{block_cache} {pct}: the keeper extended the snapshot"
        );
        // The scans after adopt what it published: nothing is sorted
        // again, through the writer or a handle claimed now.
        m.check(
            &db,
            &format!("{block_cache} {pct}: the writer over the kept snapshot"),
        );
        assert_eq!(
            db.snapshot_builds(),
            builds,
            "{block_cache} {pct}: the writer adopted the keeper's snapshot"
        );
        let reader = db.reader().unwrap();
        m.check(
            &reader,
            &format!("{block_cache} {pct}: a handle over the kept snapshot"),
        );
        assert_eq!(
            db.snapshot_builds(),
            builds,
            "{block_cache} {pct}: the handle adopted it"
        );
        // A seal left in flight: the keeper carries the snapshot across
        // the freeze, the live entries frozen ones now, and the writes
        // after go into the new table over the same keys.
        db.seal().unwrap();
        db.settle_keeper();
        assert_eq!(
            db.snapshot_kept().1,
            1,
            "{block_cache} {pct}: carried across the freeze"
        );
        let builds = db.snapshot_builds();
        for k in (0..3000u32).step_by(7) {
            m.delete(&mut db, &key(k));
            m.append(&mut db, &key(k), "f1");
        }
        for k in (1..3000u32).step_by(11) {
            m.append(&mut db, &key(k), "f2");
        }
        db.commit().unwrap();
        db.settle_keeper();
        m.check(
            &db,
            &format!("{block_cache} {pct}: the writer across the freeze"),
        );
        m.check(
            &reader,
            &format!("{block_cache} {pct}: a handle across the freeze"),
        );
        assert_eq!(
            db.snapshot_builds(),
            builds,
            "{block_cache} {pct}: nothing sorted again across the freeze"
        );
        // The seal lands: the frozen entries leave the snapshot. At least
        // one more carry: the landing's publish, and a merge it starts
        // is one more when the keeper meets the two apart.
        db.settle().unwrap();
        assert!(
            db.snapshot_kept().1 >= 2,
            "{block_cache} {pct}: carried across the landing: {}",
            db.snapshot_kept().1
        );
        m.check(
            &db,
            &format!("{block_cache} {pct}: the writer after the seal lands"),
        );
        m.check(
            &reader,
            &format!("{block_cache} {pct}: a handle after the seal lands"),
        );
        assert_eq!(
            db.snapshot_builds(),
            builds,
            "{block_cache} {pct}: nothing sorted again across the landing"
        );
        // Past the bound of writes since the last scan the keeper goes
        // dormant, and the scan after brings the rest itself: right
        // either way, and the runs it holds are the chains' answers.
        for round in 0..4u32 {
            for k in (0..3000u32).step_by(2) {
                m.delete(&mut db, &key(k));
                m.append(&mut db, &key(k), &format!("d{round}"));
            }
            db.commit().unwrap();
        }
        m.check(
            &db,
            &format!("{block_cache} {pct}: the writer past the bound"),
        );
        m.check(
            &reader,
            &format!("{block_cache} {pct}: a handle past the bound"),
        );
        let later = db.reader().unwrap();
        m.check(
            &later,
            &format!("{block_cache} {pct}: a handle claimed after"),
        );
        drop(reader);
        drop(later);
        drop(db);
        let db = Db::open(&d, Options::default()).unwrap();
        m.check(&db, &format!("{block_cache} {pct}: reopened"));
    }
}

/// A run no longer than the one it replaces is written over it in the
/// writer's copy of a form, so a burst that rewrites every key leaves the
/// copies the size a build makes them. Appended instead, half of every
/// copy's bytes were pointed at by nothing after such a burst, and the
/// blocks past twice their live bytes were unlisted and built again in
/// the scans that followed.
#[test]
fn a_replacing_write_patches_a_copy_in_place() {
    let d = dir("inplace");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        forms_from_reader_scans: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..1500u32 {
        m.append(&mut db, &key(k), "v0000");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    db.settle().unwrap();
    assert!(db.levels().0 > 1, "several partitions");
    let _handle = db.reader().unwrap();
    let mut sink = 0usize;
    // Every key replaced once: each block's deltas grow past the dense
    // bound and the commit's fill builds the block as a copy.
    let mut replace = |db: &mut Db, m: &mut ScanModel, val: &str| {
        db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
            .unwrap();
        for k in 0..1500u32 {
            m.delete(db, &key(k));
            m.append(db, &key(k), val);
        }
        db.commit().unwrap();
    };
    replace(&mut db, &mut m, "v0001");
    let (_, _, copies, _) = db.block_cache_kinds();
    assert!(copies > 0, "the rewritten blocks are copies");
    let (blocks, bytes) = db.block_cache_size();
    m.check(&db, "copies built from the first rewrite");
    // The same again, at the same length and then shorter: nothing grows.
    replace(&mut db, &mut m, "v0002");
    assert_eq!(
        db.block_cache_size(),
        (blocks, bytes),
        "a run of the same length is written over the one it replaces"
    );
    m.check(&db, "copies patched in place at the same length");
    replace(&mut db, &mut m, "v3");
    assert_eq!(
        db.block_cache_size(),
        (blocks, bytes),
        "a shorter run is written over the one it replaces"
    );
    m.check(&db, "copies patched in place with a shorter run");
    // Longer: appended, as every run was before.
    replace(&mut db, &mut m, "v0004-longer");
    let (blocks2, bytes2) = db.block_cache_size();
    assert!(
        blocks2 == blocks && bytes2 > bytes,
        "a longer run is appended: {blocks2} blocks {bytes2} bytes from {blocks} {bytes}"
    );
    m.check(&db, "copies patched with a longer run");
}

/// A scan that finds no snapshot of its state -- dropped at the publish
/// before it -- walks the forms it holds without building one, and at the
/// first block that needs one builds it and goes on from that block's
/// first key. Both halves against the model, and each held to having taken
/// the path it is about: a scan over held forms alone, which builds
/// nothing, and one that runs from held blocks into blocks with no form,
/// which builds once and must answer what one uninterrupted walk would.
#[test]
fn a_scan_builds_the_snapshot_only_where_a_block_needs_it() {
    for lazy in [true, false] {
        let d = dir(if lazy { "lazysnap" } else { "lazysnap-off" });
        let opts = Options {
            seal_bytes: 1 << 20,
            partition_bytes: Some(4 << 10),
            l0_trigger: 64,
            scan_block_cache: true,
            scan_cache_ahead: false,
            forms_settle_backlog_pct: 0,
            forms_from_reader_scans: 0,
            commit_forms: true,
            forms_carry: true,
            scan_lazy_snapshot: lazy,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let mut m = ScanModel::default();
        let key = |k: u32| format!("key-{k:05}");
        for k in 0..3000u32 {
            m.append(&mut db, &key(k), "v0");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        m.flushed();
        db.settle().unwrap();
        assert!(db.levels().0 > 1, "several partitions");
        let mut sink = 0usize;
        // The first half overlaid, so its blocks get forms from the fill;
        // the second half's blocks stay without one.
        db.scan(key(0).as_bytes(), 3000, |_k, v| sink += v.len())
            .unwrap();
        for k in (0..1500u32).step_by(3) {
            m.append(&mut db, &key(k), "v1");
        }
        db.commit().unwrap();
        db.scan(key(0).as_bytes(), 10, |_k, v| sink += v.len())
            .unwrap();
        db.commit().unwrap();
        // More overlay, then the seal, whose landing drops the snapshot.
        for k in (1..1500u32).step_by(7) {
            m.append(&mut db, &key(k), "v2");
        }
        db.commit().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
        assert!(db.levels().1 > 0, "the seal left a piece");
        // A write inside the store and past every scan below, so
        // something is unsealed: with nothing unsealed the snapshot is
        // empty and the scan builds it. Above the store's greatest key
        // it would open an ordered run instead.
        m.append(&mut db, &key(2500), "v9");
        db.commit().unwrap();

        let (done0, resumed0) = db.lazy_scans();
        let built0 = db.snapshot_builds();
        m.check_one(&db, key(100).as_bytes(), 200, "held forms only");
        let (done1, resumed1) = db.lazy_scans();
        if lazy {
            assert_eq!(
                (done1 - done0, resumed1 - resumed0),
                (1, 0),
                "the first scan walked held forms without a snapshot"
            );
            assert_eq!(db.snapshot_builds(), built0, "and built none");
        }
        let built1 = db.snapshot_builds();
        m.check_one(
            &db,
            key(1400).as_bytes(),
            500,
            "held forms into bare blocks",
        );
        let (done2, resumed2) = db.lazy_scans();
        if lazy {
            assert_eq!(
                (done2 - done1, resumed2 - resumed1),
                (0, 1),
                "the second scan stopped at a block with no form"
            );
            assert_eq!(
                db.snapshot_builds(),
                built1 + 1,
                "and built the snapshot once"
            );
        }
        // A second landing, and a scan whose very first block has no form:
        // the walk stops before emitting anything and goes on from the
        // scan's own start.
        for k in (2..1500u32).step_by(11) {
            m.append(&mut db, &key(k), "v3");
        }
        db.commit().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
        m.append(&mut db, &key(2600), "v9");
        db.commit().unwrap();
        let (done3, resumed3) = db.lazy_scans();
        m.check_one(&db, key(2100).as_bytes(), 50, "a bare first block");
        let (done4, resumed4) = db.lazy_scans();
        if lazy {
            assert_eq!(
                (done4 - done3, resumed4 - resumed3),
                (0, 1),
                "the scan stopped at its first block"
            );
        }
        // Everything, now that the snapshot stands.
        m.check(&db, if lazy { "lazy, after" } else { "eager, after" });
    }
}

/// Whether `needle` occurs in `hay`: a segment's name in the manifest's
/// bytes.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Poll `done` until it holds, and fail rather than hang when it does not
/// within two minutes: a test that waits for a thread's state says so when
/// the state is never reached.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let t = std::time::Instant::now();
    while !done() {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(120),
            "{what}: the case was not reached"
        );
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

/// The `.sup` and `ord-` files of a store, sorted.
fn segment_files(d: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup") || n.starts_with("ord-"))
        .collect();
    out.sort();
    out
}

/// The window a seal opens by renaming its segment into place before its
/// fsyncs: the segment and its index stand under their final names, the
/// manifest -- a store has one from birth -- does not name them, and the
/// WAL still holds every write they carry. Open sweeps the segment and
/// replays the WAL. Trusting the segment would read a file whose sync
/// never happened, and reading both would answer every value twice; before
/// the manifest from birth, an open with no manifest took every `seg-`
/// file as live and skipped the WAL behind it, which is the second of
/// those.
#[test]
fn a_segment_the_manifest_does_not_name_is_swept_and_its_wal_replayed() {
    let d = dir("unnamed-seg");
    let mut db = Db::create(&d, wal_arm()).unwrap();
    assert!(d.join("manifest").exists(), "a manifest from birth");
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for round in 0u32..3 {
        for k in 0u32..200 {
            let key = format!("k{k:04}").into_bytes();
            let val = format!("v{round}-{k}").into_bytes();
            db.append(&key, &val);
            model.entry(key).or_default().push(val);
        }
        db.commit().unwrap();
    }
    drop(db); // the writes are in the WAL alone

    // A copy of the store seals them; its segment and index, moved into
    // the original, are the files phase R leaves before phase D and the
    // manifest that would name them.
    let d2 = dir("unnamed-seg2");
    for e in std::fs::read_dir(&d).unwrap() {
        let name = e.unwrap().file_name();
        std::fs::copy(d.join(&name), d2.join(&name)).unwrap();
    }
    let mut db2 = Db::open(&d2, wal_arm()).unwrap();
    db2.seal().unwrap();
    db2.settle().unwrap();
    assert_eq!(db2.levels(), (0, 1), "the copy sealed one piece");
    drop(db2);
    let moved = segment_files(&d2);
    assert!(
        moved.iter().any(|n| n.ends_with(".sup")) && moved.iter().any(|n| n.starts_with("ord-")),
        "the copy left no segment and index to stage the window with: {moved:?}"
    );
    for name in &moved {
        std::fs::copy(d2.join(name), d.join(name)).unwrap();
    }
    let manifest = std::fs::read(d.join("manifest")).unwrap();
    for name in &moved {
        assert!(
            !contains(&manifest, name.as_bytes()),
            "the window is a segment the manifest does not name: {name}"
        );
    }

    let db = Db::open(&d, wal_arm()).unwrap();
    for name in &moved {
        assert!(!d.join(name).exists(), "{name} is swept at open");
    }
    assert_eq!(db.segments(), 0, "nothing the manifest names");
    let r = db.reader().unwrap();
    for (key, want) in &model {
        assert_eq!(
            &read_vec(&db, key),
            want,
            "every value once, in order, from the WAL: {}",
            String::from_utf8_lossy(key)
        );
        assert_eq!(
            &read_vec(&r, key),
            want,
            "and through a handle: {}",
            String::from_utf8_lossy(key)
        );
    }
}

/// A seal whose readable landing fails is counted out, and the seals
/// queued behind it do not land: a manifest a later landing wrote would
/// cover the failed seal's sequence with its segments unnamed, and a
/// reopen would skip the WAL that holds its writes. The first version of
/// the queue let the next seal land, and a frozen table whose piece could
/// not be opened was gone at the reopen after. Here the first seal is a
/// frozen table and the second a table `sync` handed beside it; the
/// first's landing fails, the settle reports it, no manifest moves, every
/// value still reads through the tables, and the reopen replays both
/// WALs, each value once.
#[test]
fn a_failed_landing_stops_the_seals_behind_it_and_a_reopen_replays_them() {
    let d = dir("wedged-landing");
    let opts = Options {
        seal_bytes: 1 << 30,
        adaptive_shape: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    // Descending, so the batch is a hashed table and not an ordered run.
    for k in (0..3000u32).rev() {
        db.append(&key(k), b"a");
        model.entry(key(k)).or_default().push(b"a".to_vec());
    }
    db.commit().unwrap();
    db.hold_seal_landing(true);
    db.seal().unwrap();
    for k in (0..3000u32).rev().step_by(3) {
        db.append(&key(k), b"b");
        model.entry(key(k)).or_default().push(b"b".to_vec());
    }
    db.commit().unwrap();
    db.sync().unwrap();
    assert!(db.in_flight().0, "two seals in flight, both held");
    let manifest_before = std::fs::read(d.join("manifest")).unwrap();
    db.fail_next_landing();
    db.hold_seal_landing(false);
    assert!(
        db.settle().is_err(),
        "the failed landing is reported to the settle"
    );
    assert!(db.seals_wedged(), "the seals behind the failed one stopped");
    assert_eq!(
        std::fs::read(d.join("manifest")).unwrap(),
        manifest_before,
        "no manifest covers the failed seal's sequence"
    );
    for (k, want) in &model {
        assert_eq!(
            &read_vec(&db, k),
            want,
            "still readable through the tables: {}",
            String::from_utf8_lossy(k)
        );
    }
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    for (k, want) in &model {
        assert_eq!(
            &read_vec(&db, k),
            want,
            "replayed from the WALs, once: {}",
            String::from_utf8_lossy(k)
        );
    }
}

/// A store written before manifests has none, and its `seg-` files are
/// what it has: open takes them as live and the replay bound from their
/// names, as it always did. The path is kept for those stores and taken by
/// no store `create` made, which has a manifest from birth.
#[test]
fn a_store_from_before_manifests_opens_from_its_segment_names() {
    let d = dir("premanifest");
    let mut db = Db::create(&d, wal_arm()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for round in 0u32..2 {
        for k in 0u32..200 {
            let key = format!("k{k:04}").into_bytes();
            let val = format!("v{round}-{k}").into_bytes();
            db.append(&key, &val);
            model.entry(key).or_default().push(val);
        }
        db.commit().unwrap();
    }
    db.seal().unwrap();
    db.settle().unwrap();
    assert_eq!(db.levels(), (0, 1));
    // A batch after the seal, in the WAL the segment does not cover.
    db.append(b"after", b"walled");
    db.commit().unwrap();
    model.insert(b"after".to_vec(), vec![b"walled".to_vec()]);
    drop(db);
    std::fs::remove_file(d.join("manifest")).unwrap();
    let segs: Vec<String> = segment_files(&d)
        .into_iter()
        .filter(|n| n.ends_with(".sup"))
        .collect();
    assert!(
        !segs.is_empty() && segs.iter().all(|n| n.starts_with("seg-")),
        "the fallback reads seg- names: {segs:?}"
    );

    let db = Db::open(&d, wal_arm()).unwrap();
    assert_eq!(db.segments(), 1, "the segment taken as live from its name");
    for (key, want) in &model {
        assert_eq!(
            &read_vec(&db, key),
            want,
            "key {}",
            String::from_utf8_lossy(key)
        );
    }
}

/// A seal's segments are published to readers before their fsyncs, and no
/// manifest may be written until those are paid: a merge that finishes in
/// that window waits for it. Here the window is held open
/// (`hold_seal_durable`) with a merge of the partition running beside the
/// seal, so the merge finishes inside it every time; the segment work must
/// leave the merge unlanded and the manifest untouched until the seal is
/// durable, and land both after. Under the checked profile `publish`
/// asserts the same rule.
#[test]
fn a_merge_that_finishes_while_a_seal_is_between_its_phases_lands_after_it() {
    let d = dir("held-landing");
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 20),
        l0_trigger: 2,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
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
    // Two pieces over keys the partition holds: the second's durable
    // landing starts the merge of the range.
    for (tag, base) in [("a", 1000u32), ("b", 2000u32)] {
        for i in 0..10 {
            put(&mut db, &mut model, &key(base + i), tag);
            put(&mut db, &mut model, &between(base, i), tag);
        }
        db.commit().unwrap();
        db.seal().unwrap();
        wait_for("the seal's landing", || !db.in_flight().0);
    }
    wait_for("the merge's start", || db.in_flight().1);
    assert_eq!(db.levels(), (1, 2));
    let manifest_before = std::fs::read(d.join("manifest")).unwrap();
    // The third piece, held between its phases: published, not durable.
    db.hold_seal_durable(true);
    for i in 0..10 {
        put(&mut db, &mut model, &key(3000 + i), "c");
        put(&mut db, &mut model, &between(3000, i), "c");
    }
    db.commit().unwrap();
    db.seal().unwrap();
    wait_for("the third piece's publish", || db.levels() == (1, 3));
    assert!(
        db.in_flight().0,
        "a seal is in flight until its segments are durable"
    );
    assert!(
        db.in_flight().1,
        "the merge landed before the third piece was published; the case was not reached"
    );
    // The merge finishes under the hold, and the segment work finds it
    // finished and leaves it: that look is what the counter counts.
    wait_for("a merge held for the seal", || {
        db.seal_waits().held_landings > 0
    });
    assert!(db.in_flight().1, "the merge is not landed in the window");
    assert_eq!(
        std::fs::read(d.join("manifest")).unwrap(),
        manifest_before,
        "no manifest is written while the seal is between its phases"
    );
    assert_eq!(db.levels(), (1, 3));
    db.hold_seal_durable(false);
    db.settle().unwrap();
    assert_eq!(db.in_flight(), (false, false));
    assert_eq!(
        db.levels(),
        (1, 1),
        "the merge landed after the seal: a new partition, the third piece kept"
    );
    let manifest_after = std::fs::read(d.join("manifest")).unwrap();
    assert_ne!(manifest_after, manifest_before, "the landings wrote it");
    let check = |db: &Reader, state: &str| {
        let mut seen: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        db.scan(b"", usize::MAX, |k, v| {
            seen.entry(k.to_vec()).or_default().push(v.to_vec())
        })
        .unwrap();
        assert_eq!(seen.len(), model.len(), "{state}: the scan's key count");
        assert!(seen == model, "{state}: the scan disagrees with the model");
    };
    check(&db, "after the landings");
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    assert_eq!(db.levels(), (1, 1), "the manifest names both landings");
    check(&db, "reopened");
}

/// A handle under `Latest` or `Snapshot` reads the live table to the entry
/// count of the commit it holds, as it reads the log to that commit's
/// length and the values to its watermark. A handle held across a direct
/// run's exit is the sharp case: the run's uncommitted tail is truncated
/// from the ordered table and re-appended to the next one, so a snapshot
/// over the table's raw length names an entry the table no longer has.
#[test]
fn a_held_handle_never_names_an_entry_a_direct_run_left_uncommitted() {
    let d = dir("held-direct-exit");
    let mut db = Db::create(&d, Options::default()).unwrap();
    let r = db.reader().unwrap();
    // The first key of an empty store opens an ordered run, left
    // uncommitted.
    db.append(b"key-5", b"staged");
    r.snapshot();
    let mut first = Vec::new();
    let n = r
        .scan(b"", 10, |k, v| first.push((k.to_vec(), v.to_vec())))
        .unwrap();
    assert!(first.is_empty(), "the held commit has nothing: {first:?}");
    assert_eq!(n, 0, "a scan counted a key its commit does not hold");
    // A key below the run's greatest leaves the run: its uncommitted tail
    // is truncated from the ordered table.
    db.append(b"key-1", b"staged");
    let mut again = Vec::new();
    let n = r
        .scan(b"", 10, |k, v| again.push((k.to_vec(), v.to_vec())))
        .unwrap();
    assert!(again.is_empty(), "the held commit has nothing: {again:?}");
    assert_eq!(n, 0);
    r.release();
    db.commit().unwrap();
    let mut after = Vec::new();
    r.scan(b"", 10, |k, _| after.push(k.to_vec())).unwrap();
    assert_eq!(after, vec![b"key-1".to_vec(), b"key-5".to_vec()]);
}

/// A scan's limit counts the keys its commit holds: a key the writer has
/// staged and not committed is not one, and a handle that counted it
/// answered short of the limit with every key it skipped still unsealed.
#[test]
fn a_handles_scan_limit_counts_only_committed_keys() {
    let d = dir("scan-limit-committed");
    let opts = Options {
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    for k in 0..20u32 {
        db.append(format!("key-{k:02}").as_bytes(), b"v");
    }
    db.commit().unwrap();
    let r = db.reader().unwrap();
    // New keys between the committed ones, staged and not committed.
    for k in 0..10u32 {
        db.append(format!("key-{k:02}a").as_bytes(), b"staged");
    }
    let mut got = Vec::new();
    let n = r.scan(b"", 10, |k, _| got.push(k.to_vec())).unwrap();
    let want: Vec<Vec<u8>> = (0..10u32)
        .map(|k| format!("key-{k:02}").into_bytes())
        .collect();
    assert_eq!(
        got, want,
        "a handle's scan answered keys of no commit or fell short"
    );
    assert_eq!(n, 10);
    db.commit().unwrap();
}

/// A published snapshot is adopted only by a handle whose commit holds
/// every entry it names: one published at a later commit, or by the
/// writer over what it has staged, names keys the adopting handle's
/// commit does not hold, and a scan that walked it counted them.
#[test]
fn a_handle_adopts_no_snapshot_past_its_commit() {
    let d = dir("adopt-past-commit");
    let opts = Options {
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    for k in 0..20u32 {
        db.append(format!("key-{k:02}").as_bytes(), b"v");
    }
    db.commit().unwrap();
    let want: Vec<Vec<u8>> = (0..10u32)
        .map(|k| format!("key-{k:02}").into_bytes())
        .collect();
    let scan10 = |r: &Reader| {
        let mut got = Vec::new();
        let n = r.scan(b"", 10, |k, _| got.push(k.to_vec())).unwrap();
        (got, n)
    };
    // A handle pinned at this commit, and a later commit's snapshot
    // published by another handle over the same state.
    let pinned = db.reader().unwrap();
    pinned.snapshot();
    for k in 0..10u32 {
        db.append(format!("key-{k:02}a").as_bytes(), b"later");
    }
    db.commit().unwrap();
    let latest = db.reader().unwrap();
    let (got, n) = scan10(&latest);
    assert_eq!(n, 10);
    assert_eq!(got.len(), 10);
    let (got, n) = scan10(&pinned);
    assert_eq!(got, want, "a pinned handle walked a later commit's keys");
    assert_eq!(n, 10);
    pinned.release();
    // The writer's own scan over keys it has staged, and a handle at the
    // last commit after it.
    for k in 0..10u32 {
        db.append(format!("key-{k:02}b").as_bytes(), b"staged");
    }
    let mut mine = 0usize;
    db.scan(b"", 40, |_, _| mine += 1).unwrap();
    assert_eq!(mine, 40, "the writer reads what it has staged");
    let fresh = db.reader().unwrap();
    let (got, n) = scan10(&fresh);
    let want_latest: Vec<Vec<u8>> = (0..5u32)
        .flat_map(|k| [format!("key-{k:02}"), format!("key-{k:02}a")])
        .map(String::into_bytes)
        .collect();
    assert_eq!(
        got, want_latest,
        "a handle walked keys the writer has only staged"
    );
    assert_eq!(n, 10);
    db.commit().unwrap();
}

/// Reader handles across seals whose segments land before their fsyncs:
/// every value of a key once and in order, through point reads and scans,
/// before the landing, in the window between the phases -- held open at
/// each seal -- and after it, and a handle never sees fewer values of a key
/// than it saw before.
#[test]
fn reader_handles_across_seals_see_every_value_once_and_in_order() {
    let d = dir("readers-across-seals");
    let opts = Options {
        // Seals are the test's own.
        seal_bytes: 1 << 30,
        partition_bytes: Some(1 << 30),
        l0_trigger: 1 << 20,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let keys = 2000u32;
    let key = |k: u32| format!("key-{k:05}").into_bytes();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut threads = Vec::new();
    for t in 0..3u64 {
        let r = db.reader().unwrap();
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            // A value is the round it was appended in, so a key's values
            // are ascending and distinct, and a handle's view of a key
            // only grows.
            let mut seen: HashMap<u32, usize> = HashMap::new();
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ (t + 1);
            let mut ops = 0usize;
            let check = |seen: &mut HashMap<u32, usize>, k: u32, vals: &[Vec<u8>], how: &str| {
                let mut last = 0u64;
                for v in vals {
                    let ver: u64 = std::str::from_utf8(v).unwrap().parse().unwrap();
                    assert!(
                        ver > last,
                        "{how}: key {k} has value {ver} after {last}: repeated or out of order"
                    );
                    last = ver;
                }
                let prev = seen.entry(k).or_insert(0);
                assert!(
                    vals.len() >= *prev,
                    "{how}: key {k} lost values: {} after {prev}",
                    vals.len()
                );
                *prev = vals.len();
            };
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let k = (x % keys as u64) as u32;
                if x.is_multiple_of(4) {
                    let mut by_key: BTreeMap<u32, Vec<Vec<u8>>> = BTreeMap::new();
                    r.scan(&key(k), 20, |kk, v| {
                        let kn: u32 = std::str::from_utf8(&kk[4..]).unwrap().parse().unwrap();
                        by_key.entry(kn).or_default().push(v.to_vec());
                    })
                    .unwrap();
                    for (kn, vals) in &by_key {
                        check(&mut seen, *kn, vals, "scan");
                    }
                } else {
                    let got = read_vec(&r, &key(k));
                    check(&mut seen, k, &got, "read");
                }
                ops += 1;
            }
            ops
        }));
    }
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let mut x = 42u64;
    let mut pieces = 0usize;
    for round in 1..=40u64 {
        // Each key at most once a round, so a key's values are distinct,
        // in the order drawn: an ascending batch would be an ordered run
        // and go straight to a segment of its own.
        let mut drawn = BTreeSet::new();
        let mut batch = Vec::new();
        for _ in 0..300 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = (x % keys as u64) as u32;
            if drawn.insert(k) {
                batch.push(k);
            }
        }
        for k in batch {
            let v = round.to_string().into_bytes();
            db.append(&key(k), &v);
            model.entry(key(k)).or_default().push(v);
        }
        db.commit().unwrap();
        if round % 5 == 0 {
            // The seal's segments published and held short of durable, so
            // the readers see the window; then released. The last seal is
            // durable before the hold is set again: a seal thread released
            // and not yet past its check would take the new hold, and the
            // seal after it would wait for a durable end nobody ends.
            wait_for("the last seal's durable end", || !db.in_flight().0);
            db.hold_seal_durable(true);
            db.seal().unwrap();
            pieces += 1;
            wait_for("the seal's publish", || db.levels().1 >= pieces);
            assert!(db.in_flight().0, "held: in flight until durable");
            std::thread::sleep(std::time::Duration::from_millis(3));
            db.hold_seal_durable(false);
        }
    }
    db.settle().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut total = 0usize;
    for t in threads {
        total += t.join().unwrap();
    }
    assert!(total > 100, "the threads did some work: {total}");
    assert_eq!(db.levels(), (0, pieces));
    let check = |db: &Reader, state: &str| {
        for (k, want) in &model {
            assert_eq!(
                &read_vec(db, k),
                want,
                "{state}: key {}",
                String::from_utf8_lossy(k)
            );
        }
    };
    let r = db.reader().unwrap();
    check(&r, "a handle after the seals");
    drop(r);
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    assert_eq!(db.levels(), (0, pieces), "every seal's manifest landed");
    check(&db, "reopened");
}

/// A store for the seal-snapshot tests: keys with several values each,
/// deletes that end a chain, deletes followed by values again, and a key
/// created only to be deleted, all committed and none sealed. The seal
/// is landed by nothing but the settle, since the segment work is
/// inline, so the reads below are made while it runs; and the table has
/// a few megabytes of values and the seal syncs at every block, so the
/// seal outlasts the writer's commits below by a wide margin -- only a
/// writer's commit could land a finished seal under them. A table of a
/// few hundred kilobytes did not: this disk's syncs are fast, and the
/// seal was landed by a commit two milliseconds after its publish.
fn seal_snapshot_store(name: &str) -> (Db, ScanModel) {
    seal_snapshot_store_with(name, Options::default())
}

fn seal_snapshot_store_with(name: &str, base: Options) -> (Db, ScanModel) {
    let d = dir(name);
    let opts = Options {
        seal_bytes: 1 << 30,
        publish_in_background: false,
        seal_sync_every: 1024,
        ..base
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    let val = |k: u32, v: &str| format!("{v}-{k}-{}", "x".repeat(1000));
    for k in 0..3000u32 {
        m.append(&mut db, &key(k), &val(k, "v0"));
        if k % 3 == 0 {
            m.append(&mut db, &key(k), &val(k, "v1"));
        }
        if k % 5 == 0 {
            m.append(&mut db, &key(k), &val(k, "v2"));
        }
    }
    for k in (0..3000u32).step_by(7) {
        m.delete(&mut db, &key(k));
    }
    for k in (0..3000u32).step_by(14) {
        m.append(&mut db, &key(k), &val(k, "v3"));
    }
    m.append(&mut db, "key-01500x", "gone");
    m.delete(&mut db, "key-01500x");
    db.commit().unwrap();
    (db, m)
}

/// Wait for the seal thread to publish its snapshot; the seal itself
/// stays in flight until the settle.
fn await_seal_snapshot(db: &Db) {
    let t = std::time::Instant::now();
    while db.seal_snapshots() == 0 {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(30),
            "the seal did not publish its snapshot"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(db.in_flight().0, "the seal is still in flight");
}

/// The seal thread publishes the frozen table's snapshot, every chain
/// copied beside its key in key order, into the state the freeze made,
/// and a handle that scans while the seal runs adopts it instead of
/// sorting the table itself: every value in order, the tombstones
/// cutting what they cut, through the snapshot's runs first and then
/// through the piece the seal wrote from the same runs. The counters say
/// the arm was reached, since a handle that built its own would answer
/// the same.
#[test]
fn a_handle_reads_the_seals_snapshot_while_the_seal_runs() {
    let (mut db, m) = seal_snapshot_store("seal-snapshot-reads");
    db.seal().unwrap();
    await_seal_snapshot(&db);
    let r = db.reader().unwrap();
    m.check(&r, "a handle during the seal");
    assert_eq!(
        (db.snapshot_builds(), db.snapshot_extends()),
        (0, 0),
        "the handle adopted the seal's snapshot as it stood"
    );
    m.check(&db, "the writer during the seal");
    assert_eq!(db.snapshot_builds(), 0, "and so did the writer");
    assert!(db.in_flight().0, "nothing landed the seal");
    db.settle().unwrap();
    assert_eq!(db.levels(), (0, 1), "the piece landed");
    m.check(&r, "a handle after the landing");
    m.check(&db, "the writer after the landing");
    assert_eq!(db.seal_snapshots(), 1);
    drop(r);
    db.close().unwrap();
}

/// A frozen table's snapshot lives on the table, not in the state it was
/// published into: a state the writer publishes while the seal runs --
/// here the switch to an ordered table a key above the store's greatest
/// makes -- holds no published snapshot, and the first scan over it, a
/// handle's or the writer's, carries the table's own forward instead of
/// sorting the table again. The seal sorted it once and nobody else does.
#[test]
fn a_frozen_tables_snapshot_outlives_the_state_it_was_published_in() {
    for frozen_snaps in [true, false] {
        let (mut db, mut m) = seal_snapshot_store_with(
            &format!("frozen-snap-on-table-{frozen_snaps}"),
            Options {
                frozen_snaps,
                ..Options::default()
            },
        );
        db.seal().unwrap();
        await_seal_snapshot(&db);
        // Above every key, over the empty live table: a direct run opens,
        // and its switch publishes a state with no snapshot in it.
        m.append(&mut db, "zz-00001", "after");
        m.append(&mut db, "zz-00002", "after");
        db.commit().unwrap();
        let r = db.reader().unwrap();
        m.check(&r, "a handle over the switched state");
        m.check(&db, "the writer over the switched state");
        if frozen_snaps {
            assert_eq!(
                db.snapshot_builds(),
                0,
                "the handle and the writer carried the table's snapshot forward"
            );
        } else {
            assert!(
                db.snapshot_builds() > 0,
                "off, a reader of the switched state sorts the table itself"
            );
        }
        assert_eq!(db.frozen_sorts(), 0, "nobody sorted the frozen table alone");
        assert!(db.in_flight().0, "nothing landed the seal");
        db.settle().unwrap();
        m.check(&r, "a handle after the landing");
        m.check(&db, "the writer after the landing");
        assert_eq!(db.seal_waits().seal_sorts, 1, "the seal sorted its table");
        drop(r);
        db.close().unwrap();
    }
}

/// The writer's snapshot carried across its freeze (`snapshot_carry`) is
/// the frozen table's whole order, set on the table at the freeze, so the
/// seal takes it rather than sorting the table again, and the reads
/// through the seal and after its landing answer as the model does.
#[test]
fn the_seal_takes_the_order_the_writers_freeze_carried() {
    let d = dir("seal-takes-carried-order");
    let opts = Options {
        seal_bytes: 1 << 30,
        partition_bytes: Some(64 << 10),
        snapshot_carry: true,
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let mut m = ScanModel::default();
    for k in 0..3000u32 {
        m.append(&mut db, &format!("key-{k:05}"), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    assert!(db.levels().0 > 1, "partitions for the forms to carry over");
    for k in (0..3000u32).step_by(3) {
        m.append(&mut db, &format!("key-{k:05}"), "u");
    }
    for k in 0..500u32 {
        m.append(&mut db, &format!("key-{k:05}a"), "n");
    }
    for k in (0..3000u32).step_by(11) {
        m.delete(&mut db, &format!("key-{k:05}"));
    }
    db.commit().unwrap();
    // A scan through the writer before the freeze, so its tables are in
    // use and the freeze carries them and the snapshot.
    m.check(&db, "the writer before the freeze");
    // The flush's seal sorted its own table; this seal is counted alone.
    let sorts_before = db.seal_waits().seal_sorts;
    db.seal().unwrap();
    m.check(&db, "the writer during the seal");
    let r = db.reader().unwrap();
    m.check(&r, "a handle during the seal");
    db.settle().unwrap();
    m.check(&db, "the writer after the landing");
    m.check(&r, "a handle after the landing");
    assert_eq!(
        db.seal_waits().seal_sorts,
        sorts_before,
        "the seal took the order the freeze set on its table"
    );
    assert_eq!(db.frozen_sorts(), 0, "and so did every reader");
    drop(r);
    db.close().unwrap();
}

/// A key the frozen table holds and the live table writes again is one
/// key on every path a scan takes, read from the snapshot's base -- the
/// frozen table's own run -- and its live runs together: a live
/// tombstone cuts the frozen values, a live append comes after them, a
/// frozen tombstone cuts what was older and nothing written live. The
/// store has partitions and a piece under the frozen table, and the seal
/// is held before it writes, so every scan meets all four sources;
/// through the writer and a handle, before and after the landing, on
/// every scan arm.
#[test]
fn a_live_write_over_the_frozen_table_is_one_key_on_every_scan_path() {
    let arms = scan_path_arms();
    let key = |k: u32| format!("key-{k:05}");
    for (name, base) in arms {
        let d = dir(&format!("live-over-frozen-{name}"));
        let mut db = Db::create(
            &d,
            Options {
                seal_bytes: 1 << 30,
                partition_bytes: Some(64 << 10),
                direct_ingest: false,
                ..base
            },
        )
        .unwrap();
        let mut m = ScanModel::default();
        for k in 0..3000u32 {
            m.append(&mut db, &key(k), "p");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        m.flushed();
        assert!(db.levels().0 > 1, "{name}: partitions under the rest");
        // A piece: updates and new keys sealed and landed over them.
        for k in (0..3000u32).step_by(5) {
            m.append(&mut db, &key(k), "q");
        }
        for k in (0..3000u32).step_by(13) {
            m.delete(&mut db, &key(k));
        }
        db.commit().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
        assert!(db.levels().1 >= 1, "{name}: a piece over the partitions");
        // The frozen table: values, tombstones, and keys of its own.
        for k in (0..3000u32).step_by(3) {
            m.append(&mut db, &key(k), "f");
        }
        for k in (0..3000u32).step_by(11) {
            m.delete(&mut db, &key(k));
        }
        for k in 0..400u32 {
            m.append(&mut db, &format!("key-{k:05}f"), "f");
        }
        db.commit().unwrap();
        m.check(&db, &format!("{name}: the writer before the freeze"));
        db.hold_seal_landing(true);
        db.seal().unwrap();
        assert!(db.in_flight().0, "{name}: the seal is held");
        // The live table over it: appends after frozen values and after
        // frozen tombstones, tombstones over frozen values, keys of its
        // own, and a key the frozen table made and the live one deletes.
        for k in (0..3000u32).step_by(4) {
            m.append(&mut db, &key(k), "l");
        }
        for k in (0..3000u32).step_by(9) {
            m.delete(&mut db, &key(k));
        }
        for k in (0..400u32).step_by(2) {
            m.delete(&mut db, &format!("key-{k:05}f"));
        }
        for k in 0..300u32 {
            m.append(&mut db, &format!("key-{k:05}l"), "l");
        }
        db.commit().unwrap();
        let r = db.reader().unwrap();
        m.check(&r, &format!("{name}: a handle during the seal"));
        m.check(&db, &format!("{name}: the writer during the seal"));
        db.hold_seal_landing(false);
        db.settle().unwrap();
        m.check(&r, &format!("{name}: a handle after the landing"));
        m.check(&db, &format!("{name}: the writer after the landing"));
        drop(r);
        db.close().unwrap();
    }
}

/// The scan paths a frozen table's keys reach a scan through, each the
/// arm that keeps it: the block cache's, the cursor merge's, the merge
/// over unrouted sources, the lazy snapshot, the snapshot's runs, a
/// snapshot each handle builds alone, and frozen tables with no snapshot
/// of their own.
fn scan_path_arms() -> [(&'static str, Options); 7] {
    [
        ("default", Options::default()),
        (
            "cursormerge",
            Options {
                scan_merge: false,
                scan_block_cache: false,
                ..Options::default()
            },
        ),
        (
            "nocache",
            Options {
                scan_block_cache: false,
                ..Options::default()
            },
        ),
        (
            "lazy",
            Options {
                scan_lazy_snapshot: true,
                ..Options::default()
            },
        ),
        (
            "runs",
            Options {
                snapshot_runs: true,
                ..Options::default()
            },
        ),
        (
            "unshared",
            Options {
                share_snapshot: false,
                ..Options::default()
            },
        ),
        (
            "nofrozen",
            Options {
                frozen_snaps: false,
                ..Options::default()
            },
        ),
    ]
}

/// The writer's snapshot holds the live table's keys alone over the
/// frozen table's own as its base, so the landing that seals the frozen
/// table keeps it with the base dropped: the writer's first scan after
/// the landing builds and extends nothing, where a snapshot of both
/// tables went with the frozen keys it held and was sorted again.
#[test]
fn the_writer_keeps_its_live_snapshot_across_a_landing() {
    let d = dir("live-snap-across-landing");
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 1 << 30,
            partition_bytes: Some(64 << 10),
            direct_ingest: false,
            ..Options::default()
        },
    )
    .unwrap();
    let mut m = ScanModel::default();
    let key = |k: u32| format!("key-{k:05}");
    for k in 0..3000u32 {
        m.append(&mut db, &key(k), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    for k in (0..3000u32).step_by(3) {
        m.append(&mut db, &key(k), "f");
    }
    db.commit().unwrap();
    m.check(&db, "the writer before the freeze");
    db.hold_seal_landing(true);
    db.seal().unwrap();
    for k in (0..3000u32).step_by(7) {
        m.append(&mut db, &key(k), "l");
    }
    db.commit().unwrap();
    m.check(&db, "the writer during the seal");
    let before = (db.snapshot_builds(), db.snapshot_extends());
    db.hold_seal_landing(false);
    db.settle().unwrap();
    assert!(!db.in_flight().0, "the seal landed");
    m.check(&db, "the writer after the landing");
    assert_eq!(
        (db.snapshot_builds(), db.snapshot_extends()),
        before,
        "the writer's live snapshot stood across the landing"
    );
    db.close().unwrap();
}

// ------------------------------------------------------------------------
// The tail a `sync` hands to a seal without freezing it
// (`Options::adaptive_shape`): replaced by whichever side publishes first.

fn tail_key(i: u32) -> Vec<u8> {
    format!("k{i:08}").into_bytes()
}

/// A hundred-byte value carrying `ver`, the suite's shape, so a load of a
/// few thousand keys crosses a small seal.
fn tail_val(ver: u32) -> Vec<u8> {
    format!("{ver:08}{}", "x".repeat(92)).into_bytes()
}

fn tail_ver(v: &[u8]) -> u32 {
    std::str::from_utf8(&v[..8]).unwrap().parse().unwrap()
}

fn sup_files(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    v.sort();
    v
}

/// Reader threads over keys `0..n`, each holding one value whose version
/// never goes backwards: point reads, and scans that check the keys come
/// in order and every one is present. Returns the operations made.
fn tail_readers(
    db: &Db,
    n: u32,
    stop: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Vec<std::thread::JoinHandle<usize>> {
    (0..3u64)
        .map(|t| {
            let r = db.reader().unwrap();
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut seen: HashMap<u32, u32> = HashMap::new();
                let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ (t + 1);
                let mut ops = 0usize;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    let k = (x % n as u64) as u32;
                    if x.is_multiple_of(8) {
                        let mut expect = k;
                        r.scan(&tail_key(k), 50, |kk, v| {
                            assert_eq!(
                                kk,
                                tail_key(expect).as_slice(),
                                "scan from {k}: key {expect} missing or out of order"
                            );
                            let ver = tail_ver(v);
                            let prev = seen.entry(expect).or_insert(0);
                            assert!(ver >= *prev, "key {expect} scanned {ver} after {prev}");
                            *prev = ver;
                            expect += 1;
                        })
                        .unwrap();
                        assert_eq!(expect, (k + 50).min(n), "scan from {k} came up short");
                    } else {
                        let got = read_vec(&r, &tail_key(k));
                        assert_eq!(got.len(), 1, "key {k}: {} values", got.len());
                        let ver = tail_ver(&got[0]);
                        let prev = seen.entry(k).or_insert(0);
                        assert!(ver >= *prev, "key {k} read {ver} after {prev}");
                        *prev = ver;
                    }
                    ops += 1;
                }
                ops
            })
        })
        .collect()
}

/// `n` keys loaded in order under a seal small enough that the run closes
/// once mid-load, leaving a tail live beside a seal in flight or a
/// partition: the shape a durable-only `sync` used to leave for good.
fn load_ordered_with_tail(d: &std::path::Path, n: u32) -> Db {
    let mut db = Db::create(
        d,
        Options {
            adaptive_shape: true,
            seal_bytes: 1 << 20,
            partition_bytes: Some(8 << 20),
            ..Options::default()
        },
    )
    .unwrap();
    for i in 0..n {
        db.append(&tail_key(i), &tail_val(1));
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    // The run closed once mid-load -- its seal in flight or landed -- and
    // the keys since are the live tail. The unsealed count takes in the
    // frozen table too, so the tail's size is not read here; a sync over
    // an empty live table would hand nothing, which the tests then see.
    assert!(
        db.in_flight().0 || db.levels() == (1, 0),
        "the run closed once mid-load: {:?}, {:?}",
        db.in_flight(),
        db.levels()
    );
    assert!(db.unsealed_keys() > 0, "a tail beside the run that closed");
    db
}

/// An ordered load whose run closed once mid-load leaves a tail; a `sync`
/// hands the tail to a seal without freezing it, the seal lands beside the
/// first partition as a piece over its range, and the landing promotes
/// the piece by link: two partitions, no unsealed key, no merge. Reader
/// threads read and scan throughout, and every key reads once with its
/// value after. Before this the tail stayed live for good -- the sync
/// sealed nothing while a seal was in flight or a partition existed -- and
/// every read of another key searched it.
#[test]
fn an_ordered_tail_handed_at_sync_is_promoted_beside_the_partition() {
    let d = dir("handed-tail-ordered");
    let n = 14_000u32;
    let db = load_ordered_with_tail(&d, n);
    let merges_before = db.phase_ns().2;
    let mut db = db;
    db.sync().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers = tail_readers(&db, n, &stop);
    let t = std::time::Instant::now();
    while !(db.levels() == (2, 0) && db.unsealed_keys() == 0) {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "the tail did not land and promote: levels {:?}, unsealed {}",
            db.levels(),
            db.unsealed_keys()
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let ops: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(ops > 100, "the readers read: {ops}");
    assert_eq!(
        db.phase_ns().2,
        merges_before,
        "promoted by link, not merged"
    );
    let names = sup_files(&d);
    assert!(
        names.len() == 2 && names.iter().all(|nm| nm.starts_with("par-")),
        "two partitions and nothing else: {names:?}"
    );
    assert_eq!(db.tail_swaps(), (0, 1), "the landing replaced the table");
    let r = db.reader().unwrap();
    for i in 0..n {
        assert_eq!(read_vec(&r, &tail_key(i)), vec![tail_val(1)], "key {i}");
    }
    drop(r);
    db.close().unwrap();
    let db = Db::open(
        &d,
        Options {
            adaptive_shape: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(db.levels(), (2, 0));
    for i in (0..n).step_by(37) {
        assert_eq!(
            read_vec(&db, &tail_key(i)),
            vec![tail_val(1)],
            "key {i} reopened"
        );
    }
}

/// The writer writes again before the handed seal lands, in both orders.
/// The landing first: the writer's next write finds the table replaced by
/// an empty one. The writer first: its write freezes the handed table
/// under a fresh one, forced by a seal slow enough -- a large hashed table
/// -- that the write comes before it. Which side swapped is asserted from
/// the store's own count, and every value reads exactly once before,
/// during and after, through reader threads and a handle.
#[test]
fn a_write_after_a_handed_seal_takes_a_fresh_table_from_whichever_side_swapped() {
    // The landing first.
    {
        let d = dir("handed-tail-landing-first");
        let n = 14_000u32;
        let mut db = load_ordered_with_tail(&d, n);
        db.sync().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readers = tail_readers(&db, n, &stop);
        let t = std::time::Instant::now();
        while db.unsealed_keys() > 0 {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "the tail did not land"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(db.tail_swaps(), (0, 1), "the landing replaced the table");
        // Writes of every kind after: updates of loaded keys under the
        // readers, then -- the readers' model ends at `n` -- keys above
        // the store's greatest, which go direct again over the empty
        // table the landing installed.
        for k in (0..n).step_by(7) {
            db.put(&tail_key(k), &tail_val(2));
        }
        db.commit().unwrap();
        assert_eq!(db.tail_swaps(), (0, 1), "nothing for the writer to replace");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let ops: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();
        assert!(ops > 100, "the readers read: {ops}");
        for k in n..n + 2000 {
            db.append(&tail_key(k), &tail_val(1));
        }
        db.commit().unwrap();
        db.flush().unwrap();
        let r = db.reader().unwrap();
        for k in 0..n + 2000 {
            let want = if k < n && k % 7 == 0 { 2 } else { 1 };
            assert_eq!(read_vec(&r, &tail_key(k)), vec![tail_val(want)], "key {k}");
        }
    }
    // The writer first.
    {
        let d = dir("handed-tail-writer-first");
        let n = 60_000u32;
        let mut db = Db::create(
            &d,
            Options {
                adaptive_shape: true,
                // Never on its own: the whole table is the sync's to hand.
                seal_bytes: 1 << 40,
                partition_bytes: Some(64 << 20),
                ..Options::default()
            },
        )
        .unwrap();
        // Shuffled, so the table is hashed and its seal sorts and writes
        // sixty thousand keys: tens of milliseconds the write below beats.
        for i in 0..n {
            let k = (i as u64 * 7919 % n as u64) as u32;
            db.append(&tail_key(k), &tail_val(1));
            if i % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readers = tail_readers(&db, n, &stop);
        db.sync().unwrap();
        assert!(db.in_flight().0, "the tail's seal is in flight");
        db.put(&tail_key(3), &tail_val(2));
        assert_eq!(
            db.tail_swaps(),
            (1, 0),
            "the writer froze the handed table under a fresh one before the landing"
        );
        assert_eq!(read_vec(&db, &tail_key(3)), vec![tail_val(2)]);
        assert_eq!(read_vec(&db, &tail_key(4)), vec![tail_val(1)]);
        for k in (0..n).step_by(11) {
            db.put(&tail_key(k), &tail_val(2));
        }
        db.commit().unwrap();
        // Until the landing, the handed table is the frozen one and the
        // reads merge both; after it, the piece and the live table.
        let t = std::time::Instant::now();
        while db.in_flight().0 {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(30),
                "the handed table did not land: unsealed {}",
                db.unsealed_keys()
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            db.unsealed_keys() < n as usize / 2,
            "the handed table went with its landing: unsealed {}",
            db.unsealed_keys()
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let ops: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();
        assert!(ops > 100, "the readers read: {ops}");
        assert_eq!(db.tail_swaps(), (1, 0));
        db.flush().unwrap();
        let r = db.reader().unwrap();
        for k in 0..n {
            let want = if k % 11 == 0 || k == 3 { 2 } else { 1 };
            assert_eq!(read_vec(&r, &tail_key(k)), vec![tail_val(want)], "key {k}");
        }
    }
}

/// Exits the process unless dropped within a minute: for a test whose
/// failure is a wait that never returns, so it reports itself rather
/// than hanging. Dropped as the waits end, or by the unwind of a failure
/// in them, so an assertion reports itself and not the watchdog.
struct Watchdog {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    watch: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn arm(what: &'static str) -> Watchdog {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = {
            let done = done.clone();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    if t.elapsed() > std::time::Duration::from_secs(60) {
                        eprintln!("{what}");
                        std::process::exit(101);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })
        };
        Watchdog {
            done,
            watch: Some(watch),
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(w) = self.watch.take() {
            let _ = w.join();
        }
    }
}

/// Partitions under a piece, the store each held-seal test below starts
/// from: three thousand keys flushed, then updates and tombstones over a
/// fifth and a thirteenth of them sealed and landed as a piece.
fn partitions_under_a_piece(name: &str, base: Options) -> (Db, ScanModel, PathBuf) {
    let d = dir(name);
    let mut db = Db::create(
        &d,
        Options {
            seal_bytes: 1 << 30,
            partition_bytes: Some(64 << 10),
            direct_ingest: false,
            // No merge of the pieces the landings leave: a merge drops
            // the tombstones the model's scans still visit.
            l0_trigger: 64,
            ..base
        },
    )
    .unwrap();
    let key = |k: u32| format!("key-{k:05}");
    let mut m = ScanModel::default();
    for k in 0..3000u32 {
        m.append(&mut db, &key(k), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    m.flushed();
    assert!(db.levels().0 > 1, "{name}: partitions under the rest");
    for k in (0..3000u32).step_by(5) {
        m.append(&mut db, &key(k), "q");
    }
    for k in (0..3000u32).step_by(13) {
        m.delete(&mut db, &key(k));
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.levels().1 >= 1, "{name}: a piece over the partitions");
    (db, m, d)
}

/// Frozen table `f` of three: values, tombstones and keys of its own,
/// each table over a different stride so every pair of them shares keys,
/// and two keys whose versions cross all three -- a value, a tombstone
/// and a value again, and two values under a tombstone -- committed and
/// sealed.
fn freeze_one_of_three(db: &mut Db, m: &mut ScanModel, f: u32) {
    let key = |k: u32| format!("key-{k:05}");
    for k in (0..3000u32).step_by(3 + f as usize) {
        m.append(db, &key(k), &format!("f{f}"));
    }
    for k in (0..3000u32).step_by(11 + 2 * f as usize) {
        m.delete(db, &key(k));
    }
    for k in 0..200u32 {
        m.append(db, &format!("key-{k:05}f{f}"), "f");
    }
    match f {
        0 => {
            m.append(db, "key-cross-a", "v0");
            m.append(db, "key-cross-b", "a");
        }
        1 => {
            m.delete(db, "key-cross-a");
            m.append(db, "key-cross-b", "b");
        }
        _ => {
            m.append(db, "key-cross-a", "v2");
            m.delete(db, "key-cross-b");
        }
    }
    db.commit().unwrap();
    db.seal().unwrap();
}

/// Three seals in flight, each held before it writes
/// (`hold_seal_landing`): the writer freezes into the room the list has
/// and waits for none of them, and every read path folds the three
/// frozen tables and the live one by age -- a tombstone in a newer frozen
/// table cuts the values of the older ones and of the piece and the
/// partition under them, and the values after it stand. Released, the
/// seals land in the order they were handed and every read holds after.
#[test]
fn a_writer_freezes_into_room_while_three_seals_are_held() {
    let key = |k: u32| format!("key-{k:05}");
    for (name, base) in scan_path_arms() {
        let (mut db, mut m, _) = partitions_under_a_piece(&format!("three-held-{name}"), base);
        let watch = Watchdog::arm("a freeze waited for a held seal: the list had no room");
        let w0 = db.seal_waits();
        db.hold_seal_landing(true);
        for f in 0..3u32 {
            freeze_one_of_three(&mut db, &mut m, f);
            assert_eq!(
                db.frozen_tables(),
                f as usize + 1,
                "{name}: frozen into room, the seals held"
            );
            m.check(&db, &format!("{name}: the writer over {} frozen", f + 1));
        }
        drop(watch);
        let w = db.seal_waits();
        assert_eq!(
            (
                w.join_wait_ns - w0.join_wait_ns,
                w.blocked_joins - w0.blocked_joins
            ),
            (0, 0),
            "{name}: no freeze waited: {w:?}"
        );
        // The live table over the three: values after frozen values and
        // after frozen tombstones, tombstones over frozen values, and a
        // key the oldest frozen table made that the live one deletes.
        for k in (0..3000u32).step_by(4) {
            m.append(&mut db, &key(k), "l");
        }
        for k in (0..3000u32).step_by(9) {
            m.delete(&mut db, &key(k));
        }
        for k in (0..200u32).step_by(2) {
            m.delete(&mut db, &format!("key-{k:05}f0"));
        }
        m.append(&mut db, "key-cross-a", "v3");
        db.commit().unwrap();
        let r = db.reader().unwrap();
        m.check(&r, &format!("{name}: a handle beside three held seals"));
        m.check(&db, &format!("{name}: the writer beside three held seals"));
        // The two crossing keys, spelled out: the middle table's tombstone
        // cut the oldest's value, and the newest's cut both of theirs.
        assert_eq!(
            read_vec(&db, b"key-cross-a"),
            vec![b"v2".to_vec(), b"v3".to_vec()],
            "{name}"
        );
        assert_eq!(db.count(b"key-cross-a").unwrap(), 2, "{name}");
        assert!(read_vec(&db, b"key-cross-b").is_empty(), "{name}");
        assert_eq!(db.count(b"key-cross-b").unwrap(), 0, "{name}");
        db.hold_seal_landing(false);
        db.settle().unwrap();
        assert_eq!(db.frozen_tables(), 0, "{name}: all three landed");
        m.check(&r, &format!("{name}: a handle after the landings"));
        m.check(&db, &format!("{name}: the writer after the landings"));
        drop(r);
        db.close().unwrap();
    }
}

/// A fourth freeze beside three held seals waits, and only for the front:
/// once the hold lifts the oldest lands, the freeze takes its room, and
/// the wait is counted once. The seals land in the order they were
/// handed, so a newer table's tombstone keeps cutting an older table's
/// values at every landing.
#[test]
fn a_fourth_freeze_waits_for_the_front_to_land() {
    let key = |k: u32| format!("key-{k:05}");
    let (mut db, mut m, _) = partitions_under_a_piece("fourth-freeze", Options::default());
    let watch = Watchdog::arm("a freeze waited for a held seal, or the fourth past the release");
    db.hold_seal_landing(true);
    for f in 0..3u32 {
        freeze_one_of_three(&mut db, &mut m, f);
    }
    assert_eq!(db.frozen_tables(), 3);
    for k in (0..3000u32).step_by(7) {
        m.append(&mut db, &key(k), "f3");
    }
    m.append(&mut db, "key-cross-b", "c");
    db.commit().unwrap();
    let w0 = db.seal_waits();
    let release = db.release_seal_landing_after(std::time::Duration::from_millis(500));
    db.seal().unwrap();
    release.join().unwrap();
    drop(watch);
    let w = db.seal_waits();
    assert_eq!(
        w.blocked_joins - w0.blocked_joins,
        1,
        "the fourth freeze waited once: {w:?}"
    );
    assert!(w.join_wait_ns > w0.join_wait_ns, "{w:?}");
    assert!(db.frozen_tables() >= 1, "the fourth table frozen");
    m.check(&db, "the writer over the fourth frozen table");
    let r = db.reader().unwrap();
    m.check(&r, "a handle over the fourth frozen table");
    db.settle().unwrap();
    assert_eq!(db.frozen_tables(), 0, "every seal landed");
    m.check(&db, "the writer after the landings");
    m.check(&r, "a handle after the landings");
    assert_eq!(
        read_vec(&db, b"key-cross-b"),
        vec![b"c".to_vec()],
        "the fourth table's value over the third's tombstone"
    );
}

/// A table `sync` hands to a seal beside two frozen tables whose seals
/// are held: the writer's next write freezes it into the third room and
/// waits for neither, where a list of one parked the write until the
/// frozen slot emptied. Its seal lands behind the other two.
#[test]
fn a_handed_tail_freezes_into_room_beside_two_seals() {
    let (mut db, mut m, _) = partitions_under_a_piece(
        "handed-into-room",
        Options {
            adaptive_shape: true,
            ..Options::default()
        },
    );
    let watch = Watchdog::arm("a freeze or the write after the hand-off waited for a held seal");
    let w0 = db.seal_waits();
    db.hold_seal_landing(true);
    for f in 0..2u32 {
        freeze_one_of_three(&mut db, &mut m, f);
    }
    assert_eq!(db.frozen_tables(), 2);
    // Shuffled, so the tail is hashed and goes to a seal.
    for i in 0..500u32 {
        m.append(&mut db, &format!("key-{:05}t", i * 7919 % 500), "t");
    }
    db.commit().unwrap();
    db.sync().unwrap();
    assert_eq!(db.tail_swaps(), (0, 0), "handed, not yet replaced");
    m.append(&mut db, "key-00001", "w");
    db.commit().unwrap();
    drop(watch);
    assert_eq!(db.tail_swaps(), (1, 0), "the write froze the handed table");
    assert_eq!(db.frozen_tables(), 3, "into the third room");
    let w = db.seal_waits();
    assert_eq!(
        (
            w.join_wait_ns - w0.join_wait_ns,
            w.blocked_joins - w0.blocked_joins
        ),
        (0, 0),
        "{w:?}"
    );
    let r = db.reader().unwrap();
    m.check(&r, "a handle over three frozen, the newest handed");
    m.check(&db, "the writer over three frozen, the newest handed");
    db.hold_seal_landing(false);
    db.settle().unwrap();
    assert_eq!(db.frozen_tables(), 0);
    m.check(&r, "a handle after the landings");
    m.check(&db, "the writer after the landings");
}

/// Every file of a store copied as it stands, as a crash at this instant
/// would leave it on a device that kept every write. A file gone between
/// the listing and its copy -- a merge's temp name renamed or removed --
/// is left out, as a crash before its creation would leave it: no
/// manifest names a file of work that has not landed.
fn copy_store(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if !e.file_type().unwrap().is_file() {
            continue;
        }
        match std::fs::copy(e.path(), to.join(e.file_name())) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!("copying {:?}: {err}", e.path()),
        }
    }
}

/// A crash with three seals in flight, the store copied as it stood with
/// nothing writing a file: all three held before they write, and the
/// oldest between its phases -- its segment renamed into place, unsynced
/// and named in no manifest -- with the two behind it held before they
/// write. Each copy opens to every committed key: the manifest covers
/// nothing the three sealed, open sweeps the segments it does not name,
/// and the WAL replays the three tables' records by sequence, refusing
/// no gap.
#[test]
fn a_crash_with_three_seals_in_flight_replays_them_all() {
    for between in [false, true] {
        let name = format!("crash-three-{between}");
        let (mut db, mut m, src) = partitions_under_a_piece(&name, Options::default());
        let watch = Watchdog::arm("a freeze waited for a held seal");
        if between {
            db.hold_seal_durable(true);
            freeze_one_of_three(&mut db, &mut m, 0);
            wait_for("the oldest seal's readable landing", || {
                db.frozen_tables() == 0
            });
            db.hold_seal_landing(true);
            for f in 1..3u32 {
                freeze_one_of_three(&mut db, &mut m, f);
            }
            assert_eq!(db.frozen_tables(), 2);
        } else {
            db.hold_seal_landing(true);
            for f in 0..3u32 {
                freeze_one_of_three(&mut db, &mut m, f);
            }
            assert_eq!(db.frozen_tables(), 3);
        }
        drop(watch);
        assert!(db.in_flight().0);
        m.append(&mut db, "key-00002", "after");
        db.commit().unwrap();
        let copy = dir(&format!("{name}-copy"));
        copy_store(&src, &copy);
        // Opened as the store was made: no merge of the pieces at open.
        let opts = Options {
            l0_trigger: 64,
            ..Options::default()
        };
        let opened = Db::open(&copy, opts.clone()).unwrap();
        m.check(&opened, &format!("{name}: the crash copy reopened"));
        drop(opened);
        db.hold_seal_durable(false);
        db.hold_seal_landing(false);
        db.settle().unwrap();
        m.check(&db, &format!("{name}: the store itself, landed"));
        // The close is a flush.
        db.close().unwrap();
        m.flushed();
        let reopened = Db::open(&src, opts).unwrap();
        m.check(&reopened, &format!("{name}: the store reopened"));
    }
}

/// Reader handles on their own threads keep answering while the writer
/// freezes three tables at a time under held seals and releases them to
/// land under the readers, round after round: every value read is one
/// the writer wrote for that key, a key's version never goes backwards
/// for one reader, and a scan comes back in key order.
#[test]
fn readers_on_threads_keep_answering_beside_three_frozen_tables() {
    let d = dir("readers-three-frozen");
    let opts = Options {
        seal_bytes: 1 << 30,
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
    db.flush().unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut threads = Vec::new();
    for t in 0..3u64 {
        let r = db.reader().unwrap();
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            let mut seen: HashMap<u32, u64> = HashMap::new();
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ t;
            let (mut reads, mut scans) = (0usize, 0usize);
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
    let mut ver = 0u64;
    let mut deepest = 0usize;
    for _ in 0..12 {
        let watch = Watchdog::arm("a freeze waited for a held seal: the list had no room");
        db.hold_seal_landing(true);
        for _ in 0..3 {
            for _ in 0..4 {
                ver += 1;
                for _ in 0..50 {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    let k = (x % keys as u64) as u32;
                    db.put(&key(k), ver.to_string().as_bytes());
                }
                db.commit().unwrap();
            }
            db.seal().unwrap();
            deepest = deepest.max(db.frozen_tables());
        }
        drop(watch);
        // Writes over the three while they are held, then the landings
        // under the readers and under the writes that follow.
        for _ in 0..4 {
            ver += 1;
            for _ in 0..50 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let k = (x % keys as u64) as u32;
                db.put(&key(k), ver.to_string().as_bytes());
            }
            db.commit().unwrap();
        }
        db.hold_seal_landing(false);
        // The landings under the readers and under writes, all of them
        // before the next round holds again: a seal released for less
        // than its poll would be held once more, and the round's first
        // freeze would wait on it for good.
        let watch = Watchdog::arm("the released seals did not land");
        while db.frozen_tables() > 0 {
            ver += 1;
            for _ in 0..50 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let k = (x % keys as u64) as u32;
                db.put(&key(k), ver.to_string().as_bytes());
            }
            db.commit().unwrap();
        }
        drop(watch);
    }
    db.flush().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut total = (0usize, 0usize);
    for t in threads {
        let (r, s) = t.join().unwrap();
        total.0 += r;
        total.1 += s;
    }
    assert_eq!(deepest, 3, "the list reached three");
    assert!(
        total.0 > 1000 && total.1 > 100,
        "the readers read: {total:?}"
    );
    let r = db.reader().unwrap();
    for k in (0..keys).step_by(97) {
        assert_eq!(read_vec(&db, &key(k)), read_vec(&r, &key(k)), "key {k}");
    }
}

/// A table a `sync` handed to a seal is final, and the seal sets its
/// snapshot on the table while it is still live (`FrozenSnaps`); the
/// writer's next write freezes it, and the first scan over that state, a
/// handle's, carries the seal's copy forward with the live key alone. The
/// seal publishes into a state only a snapshot of that state's own frozen
/// table, which a handed table was not when the seal made it, so off,
/// the handle sorts the handed table again. The seal is held before it
/// writes, so the write freezes the table on every run.
#[test]
fn a_handed_tables_seal_snapshot_serves_the_state_that_freezes_it() {
    for frozen_snaps in [true, false] {
        let d = dir(&format!("handed-snap-{frozen_snaps}"));
        let mut db = Db::create(
            &d,
            Options {
                adaptive_shape: true,
                // Never on its own: the whole table is the sync's to hand.
                seal_bytes: 1 << 40,
                partition_bytes: Some(64 << 20),
                frozen_snaps,
                ..Options::default()
            },
        )
        .unwrap();
        let n = 4000u64;
        let mut m = ScanModel::default();
        // Shuffled, so the table is hashed and the seal sorts it.
        for i in 0..n {
            m.append(&mut db, &format!("key-{:05}", i * 7919 % n), "a");
        }
        db.commit().unwrap();
        db.hold_seal_landing(true);
        db.sync().unwrap();
        assert!(db.in_flight().0, "the handed table's seal is in flight");
        // The copy made, its publish refused -- the seal's time is counted
        // after both -- and, on, set on the table.
        wait_for("the seal's snapshot", || {
            db.seal_waits().seal_snap_ns > 0 && db.seal_table_snapshots() == u64::from(frozen_snaps)
        });
        assert_eq!(
            db.seal_snapshots(),
            0,
            "the handed table is live: no publish"
        );
        m.append(&mut db, "key-00007", "b");
        db.commit().unwrap();
        assert_eq!(db.tail_swaps(), (1, 0), "the write froze the handed table");
        let builds = db.snapshot_builds();
        let r = db.reader().unwrap();
        m.check(&r, "a handle over the frozen handed table");
        if frozen_snaps {
            assert_eq!(
                db.snapshot_builds(),
                builds,
                "the handle carried the seal's copy forward"
            );
        } else {
            assert!(
                db.snapshot_builds() > builds,
                "off, the handle sorts the handed table itself"
            );
        }
        m.check(&db, "the writer over the frozen handed table");
        assert_eq!(db.frozen_sorts(), 0, "nobody sorted the handed table alone");
        db.hold_seal_landing(false);
        db.settle().unwrap();
        m.check(&r, "a handle after the landing");
        m.check(&db, "the writer after the landing");
        drop(r);
        db.close().unwrap();
    }
}

/// A hashed tail -- shuffled writes -- handed while a seal is in flight:
/// the frozen slot is that seal's, so the tail goes without a freeze, and
/// lands after it as a piece over the partition, which shuffled keys
/// cannot promote. Every key reads right through the landings, and both
/// WALs -- the frozen table's and the handed one's -- are retired by
/// their own landings. The seal in flight is held before it writes
/// (`hold_seal_landing`), so it is in flight at the sync on every run and
/// not only where the machine's timing puts it there.
#[test]
fn a_hashed_tail_handed_beside_a_seal_in_flight_lands_as_a_piece() {
    let d = dir("handed-tail-hashed");
    let n = 40_000u32;
    let mut db = Db::create(
        &d,
        Options {
            adaptive_shape: true,
            seal_bytes: 4 << 20,
            partition_bytes: Some(64 << 20),
            // Buffered commits, the arm's shape, so the tail's few commits
            // do not wait out the seal in flight.
            sync: supdb::SyncPolicy::EveryN(u32::MAX),
            // Each seal's own WAL, retired by its own landing.
            seal_rotates_wal: true,
            ..Options::default()
        },
    )
    .unwrap();
    db.hold_seal_landing(true);
    for i in 0..n {
        let k = (i as u64 * 7919 % n as u64) as u32;
        db.append(&tail_key(k), &tail_val(1));
        if i % 200 == 199 {
            db.commit().unwrap();
        }
    }
    // The run's seal is in flight, held, and nothing has landed: every key
    // is in the frozen table or the live tail, which is the case the test
    // is about.
    assert!(
        db.in_flight().0 && db.levels() == (0, 0),
        "a seal in flight and nothing landed: {:?}, {:?}",
        db.in_flight(),
        db.levels()
    );
    assert_eq!(db.unsealed_keys(), n as usize);
    db.sync().unwrap();
    assert!(
        db.in_flight().0 && db.unsealed_keys() == n as usize,
        "the tail handed, and still live beside the frozen table"
    );
    assert_eq!(db.segments(), 2, "two tables whose seals are in flight");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers = tail_readers(&db, n, &stop);
    db.hold_seal_landing(false);
    let t = std::time::Instant::now();
    while db.unsealed_keys() > 0 || db.in_flight().0 {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(30),
            "the seals did not land: levels {:?}, unsealed {}",
            db.levels(),
            db.unsealed_keys()
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let ops: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(ops > 100, "the readers read: {ops}");
    assert_eq!(
        db.levels(),
        (1, 1),
        "the partition, and the tail as a piece"
    );
    assert_eq!(db.tail_swaps(), (0, 1));
    // Every seal is durable, its WAL retired behind its own manifest.
    db.settle().unwrap();
    // The first seal rotated to wal-1, the hand-off to wal-2; each landing
    // retired its own.
    assert!(
        !d.join("wal-00000000").exists(),
        "the frozen table's WAL retired"
    );
    assert!(
        !d.join("wal-00000001").exists(),
        "the handed table's WAL retired"
    );
    assert!(d.join("wal-00000002").exists(), "the live WAL");
    let r = db.reader().unwrap();
    for k in 0..n {
        assert_eq!(read_vec(&r, &tail_key(k)), vec![tail_val(1)], "key {k}");
    }
    drop(r);
    db.close().unwrap();
}

/// Keys written into the fresh live table while the seal runs: past the
/// frozen keys, between them, and over them -- an append, a delete, a
/// delete and then an append. A handle's scan extends the published
/// snapshot with them rather than sorting everything, and the frozen
/// runs come along; a second batch is carried onto that extension. Only
/// a writer operation lands the seal inline, so the commits come within
/// milliseconds of the publish, before the seal thread can have
/// finished, and the model checks, which take long, come after them.
#[test]
fn a_handle_extends_the_seals_snapshot_with_the_keys_written_after_it() {
    let (mut db, mut m) = seal_snapshot_store("seal-snapshot-extends");
    let key = |k: u32| format!("key-{k:05}");
    db.seal().unwrap();
    for k in 3000..3200u32 {
        m.append(&mut db, &key(k), "live");
    }
    for k in (1..3000u32).step_by(11) {
        m.append(&mut db, &key(k), "live-over-frozen");
    }
    for k in (2..3000u32).step_by(13) {
        m.delete(&mut db, &key(k));
    }
    for k in (4..3000u32).step_by(26) {
        m.append(&mut db, &key(k), "live-after-delete");
    }
    m.append(&mut db, "key-01000x", "between");
    db.commit().unwrap();
    assert!(
        db.in_flight().0,
        "the seal is still in flight after the first commit"
    );
    await_seal_snapshot(&db);
    let r = db.reader().unwrap();
    // One scan brings the handle's snapshot current: an extension of the
    // seal's, not a build.
    m.check_one(
        &r,
        key(0).as_bytes(),
        5,
        "a handle's first scan during the seal",
    );
    assert_eq!(db.snapshot_builds(), 0, "the handle built no snapshot");
    assert!(
        db.snapshot_extends() >= 1,
        "the handle extended the seal's snapshot"
    );
    // A second batch, committed at once: the handle files it into the
    // extension it holds, or carries that forward, and builds nothing.
    for k in (5..3000u32).step_by(17) {
        m.append(&mut db, &key(k), "live-again");
    }
    m.delete(&mut db, &key(3100));
    db.commit().unwrap();
    assert!(
        db.in_flight().0,
        "the seal is still in flight after the second commit"
    );
    m.check_one(
        &r,
        key(0).as_bytes(),
        5,
        "a handle's scan after the second batch",
    );
    assert_eq!(db.snapshot_builds(), 0, "still no build");
    // The full checks, on handles only: nothing lands the seal under them.
    m.check(
        &r,
        "a handle during the seal, over the keys written after it",
    );
    m.check(&db, "the writer during the seal");
    assert_eq!(db.snapshot_builds(), 0, "the writer extended too");
    assert!(db.in_flight().0, "the seal is still in flight");
    db.settle().unwrap();
    assert_eq!(db.levels(), (0, 1), "the piece landed");
    m.check(&r, "a handle after the landing");
    m.check(&db, "the writer after the landing");
    drop(r);
    db.close().unwrap();
}

/// The crash windows of a handed table, emulated on the files: the WAL
/// the hand-off rotated out is retired only at the landing, so a crash
/// after the manifest and before the retirement replays nothing twice, and
/// a crash before the manifest sweeps the piece and replays the WAL. The
/// ordered tail's log is its direct segment's temp file, with the same
/// two windows.
#[test]
fn a_handed_tables_log_is_retired_at_its_landing_and_replays_before_it() {
    // The windows of a hand-off that rotates the WAL; a seal that keeps
    // it has its own, below.
    let opts = || Options {
        adaptive_shape: true,
        seal_rotates_wal: true,
        ..Options::default()
    };
    // The hashed tail: updates over a partition, through the WAL.
    {
        let d = dir("handed-tail-wal");
        let n = 3000u32;
        let mut db = Db::create(&d, opts()).unwrap();
        for i in 0..n {
            db.append(&tail_key(i), &tail_val(1));
        }
        db.commit().unwrap();
        db.flush().unwrap();
        assert_eq!(db.levels(), (1, 0));
        for i in 0..2000u32 {
            let k = (i as u64 * 7919 % 2000) as u32;
            db.put(&tail_key(k), &tail_val(2));
            if i % 500 == 499 {
                db.commit().unwrap();
            }
        }
        // The live WAL, the one the tail's frames are in: the flush of an
        // ordered run rotates nothing, so its id is not fixed here.
        let wal1 = db.wal_durable().0;
        let saved_wal = std::fs::read(&wal1).unwrap();
        let saved_manifest = std::fs::read(d.join("manifest")).unwrap();
        db.sync().unwrap();
        let wal2 = db.wal_durable().0;
        assert_ne!(wal1, wal2, "the hand-off rotated the WAL");
        db.settle().unwrap();
        assert_eq!(db.levels(), (1, 1), "the tail landed as a piece");
        assert_eq!(db.unsealed_keys(), 0);
        assert!(
            !wal1.exists(),
            "the handed table's WAL retired at its landing"
        );
        assert!(wal2.exists());
        drop(db);
        let check = |db: &Db, what: &str| {
            for k in 0..n {
                let want = if k < 2000 { 2 } else { 1 };
                assert_eq!(
                    read_vec(db, &tail_key(k)),
                    vec![tail_val(want)],
                    "{what}: key {k}"
                );
            }
        };
        // The manifest names the piece, the WAL was not yet retired: its
        // records are covered by sequence and replay nothing.
        std::fs::write(&wal1, &saved_wal).unwrap();
        let db = Db::open(&d, opts()).unwrap();
        assert_eq!(db.levels(), (1, 1));
        assert_eq!(db.unsealed_keys(), 0, "the covered WAL replayed nothing");
        check(&db, "after the manifest, before the retirement");
        drop(db);
        // Before the manifest: the piece is an orphan and the WAL replays.
        std::fs::write(&wal1, &saved_wal).unwrap();
        std::fs::write(d.join("manifest"), &saved_manifest).unwrap();
        let db = Db::open(&d, opts()).unwrap();
        assert_eq!(db.levels(), (1, 0), "the piece swept");
        assert_eq!(db.unsealed_keys(), 2000, "the WAL replayed the tail");
        check(&db, "before the manifest");
    }
    // The ordered tail: its direct segment's temp file.
    {
        let d = dir("handed-tail-tmp");
        let n = 14_000u32;
        let mut db = load_ordered_with_tail(&d, n);
        db.settle().unwrap();
        assert_eq!(db.levels(), (1, 0));
        let tmp: Vec<PathBuf> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("direct-") && s.ends_with(".tmp"))
            })
            .collect();
        assert_eq!(tmp.len(), 1, "the tail's open run: {tmp:?}");
        let saved_tmp = std::fs::read(&tmp[0]).unwrap();
        let saved_manifest = std::fs::read(d.join("manifest")).unwrap();
        // The partition as the old manifest names it: a promotion links it
        // under a name with its fence closed and unlinks this one after
        // the manifest, so the window before the manifest still has it.
        let parts = sup_files(&d);
        assert_eq!(
            parts.len(),
            1,
            "one partition before the hand-off: {parts:?}"
        );
        let old_part = d.join(&parts[0]);
        let saved_part = std::fs::read(&old_part).unwrap();
        db.sync().unwrap();
        db.settle().unwrap();
        assert_eq!(db.levels(), (2, 0), "landed and promoted");
        assert!(!tmp[0].exists(), "the temp name retired at the landing");
        assert!(!old_part.exists(), "the partition's old name unlinked");
        drop(db);
        let check = |db: &Db, what: &str| {
            for k in (0..n).step_by(3) {
                assert_eq!(
                    read_vec(db, &tail_key(k)),
                    vec![tail_val(1)],
                    "{what}: key {k}"
                );
            }
        };
        // After the manifest: a temp name whose id the manifest already
        // names is the window's leftover, removed rather than recovered.
        std::fs::write(&tmp[0], &saved_tmp).unwrap();
        let db = Db::open(&d, opts()).unwrap();
        assert_eq!(db.levels(), (2, 0));
        assert_eq!(db.unsealed_keys(), 0);
        assert!(!tmp[0].exists());
        check(&db, "after the manifest");
        drop(db);
        // Before the manifest: the promoted names are orphans, swept, the
        // partition stands under its old name, and the temp file recovers
        // as a piece over the last range -- which the segment work's
        // thread may already have promoted again.
        std::fs::write(&tmp[0], &saved_tmp).unwrap();
        std::fs::write(&old_part, &saved_part).unwrap();
        std::fs::write(d.join("manifest"), &saved_manifest).unwrap();
        let db = Db::open(&d, opts()).unwrap();
        let lv = db.levels();
        assert!(
            lv == (1, 1) || lv == (2, 0),
            "the orphan swept and the temp file recovered: {lv:?}"
        );
        assert_eq!(db.unsealed_keys(), 0, "nothing replayed into memory");
        check(&db, "before the manifest");
    }
}

/// A landing that installs an empty live table for a handed one carries no
/// form into the new state. A freeze files the writer's backlog first, so
/// the forms of the state it replaces are current to the frozen table's
/// whole log and a copy of one is the block; a hand-off files nothing, so
/// a form copied out of that state is current to a position short of the
/// table's end while the piece the landing publishes holds the rest.
/// Carried, such a form stood as the block once the writer's next
/// maintained commit stamped the new state's position over it, and a
/// handle's scan at that commit read a key short of the handed table's
/// last batch while the point read beside it answered right.
///
/// The shape: the writer scans, which builds the forms its next commit
/// publishes; a handle scans, so that commit maintains; a batch is then
/// committed with no scan since the last maintained commit, so the
/// published forms are behind the log by that batch; the sync hands the
/// table and the landing wins the swap, the writer writing nothing until
/// it has; the handle scans once more, so the writer's next commit
/// maintains; one write and its commit; then the handle's scans against
/// the model. Under a cache budget (`scan_cache_bytes`), the arm where the
/// commit's maintenance stamps the position and publishes only what the
/// writer settled: the arm that builds a form for every overlaid block at
/// the commit rebuilt every carried one first and hid the stale copies,
/// and the settle-only arm publishes none at all.
#[test]
fn a_landing_that_replaces_the_live_table_carries_no_form() {
    let d = dir("handed-tail-forms");
    let opts = Options {
        adaptive_shape: true,
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_bytes: 64 << 20,
        scan_cache_ahead: false,
        commit_forms: true,
        forms_carry: true,
        // Filed only where a scan preceded the commit, so a batch committed
        // with none since is left to the next scan, as the shape needs.
        forms_settle_backlog_pct: 0,
        // The writer files at its own commit: the position the forms are
        // current to is the writer's, not a thread's.
        upkeep: supdb::Upkeep::Inline,
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
    // Overlay keys in most blocks; the writer's scan builds their forms,
    // and a handle's scan makes the next commit publish them.
    for k in (0..1500u32).step_by(5) {
        m.append(&mut db, &key(k), "v1");
    }
    db.commit().unwrap();
    db.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    let r = db.reader().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    for k in (1..1500u32).step_by(13) {
        m.append(&mut db, &key(k), "v1b");
    }
    db.commit().unwrap();
    r.scan(key(0).as_bytes(), 1500, |_k, v| sink += v.len())
        .unwrap();
    let (forms, _, takes, _) = db.canonical_forms();
    assert!(forms > 0, "forms published before the hand-off: {forms}");
    assert!(takes > 0, "the handle walked them: {takes}");
    m.check(&r, "a handle over the maintained forms");
    // A commit after the handle's scans, maintained, so the one after it
    // has no scan since; then the last batch, left to the next scan, so
    // every published form is behind the log by it.
    m.append(&mut db, &key(1499), "v1x");
    db.commit().unwrap();
    for k in (2..1500u32).step_by(17) {
        m.append(&mut db, &key(k), "v1c");
    }
    db.commit().unwrap();
    assert_eq!(db.tail_swaps(), (0, 0));
    db.sync().unwrap();
    // The landing wins the swap: the writer writes nothing until it has.
    wait_for("the landing", || db.tail_swaps() == (0, 1));
    assert_eq!(db.unsealed_keys(), 0, "the table went with its landing");
    assert_eq!(
        db.canonical_forms().0,
        0,
        "the state the landing published starts with no form"
    );
    // The handle's scans make the writer's next commit maintain; through
    // them the handle reads the piece, since the state has no form yet.
    m.check(&r, "a handle after the landing");
    m.append(&mut db, &key(0), "after");
    db.commit().unwrap();
    // At the new commit's position: whatever form the state holds now is
    // the block to this handle.
    m.check(&r, "a handle at the commit after the landing");
    m.check(&db, "the writer at it");
    db.settle().unwrap();
    m.check(&r, "a handle after the settle");
    std::hint::black_box(sink);
}

/// An ordered load's tail handed while a threshold seal is still in
/// flight, the case a promotion at every landing makes common: the seal
/// in flight is a piece cut open above against the last partition, and
/// the tail is cut the same way; the piece's landing promotes it and
/// closes the partition the tail was cut against, so the tail lands as a
/// piece open above and cut lower than the last range's low fence -- an
/// unaligned piece, which used to go to a merge of every range it
/// overlapped. It is promoted on its keys now (`promote_ranges`), which
/// the counter says, and the store settles as partitions and no merge.
/// The seal in flight is held before it writes (`hold_seal_landing`), so
/// the case is reached on every run rather than when the machine's
/// timing puts a seal inside the sync.
#[test]
fn an_open_cut_tail_handed_beside_a_seal_in_flight_is_promoted_on_its_keys() {
    let d = dir("handed-tail-open-cut");
    let mut db = Db::create(
        &d,
        Options {
            adaptive_shape: true,
            seal_bytes: 512 << 10,
            partition_bytes: Some(8 << 20),
            ..Options::default()
        },
    )
    .unwrap();
    // Load until two partitions stand and nothing is in flight, then hold
    // every seal before it writes and load on until the next threshold
    // seal starts, held.
    let mut i = 0u32;
    let mut parts = 0usize;
    let mut held = false;
    while !(held && db.in_flight().0) {
        assert!(i < 80_000, "the shape was not reached: {:?}", db.levels());
        for _ in 0..500 {
            db.append(&tail_key(i), &tail_val(1));
            i += 1;
        }
        db.commit().unwrap();
        // Counted out at its manifest, a seal is not in flight while its
        // landing's promotion still runs: the piece must be gone too.
        let lv = db.levels();
        if !held && lv.0 >= 2 && lv.1 == 0 && !db.in_flight().0 {
            parts = lv.0;
            db.hold_seal_landing(true);
            held = true;
        }
    }
    assert_eq!(
        db.levels(),
        (parts, 0),
        "the held seal's piece has not landed"
    );
    // The tail: a run above the held seal's keys, small enough that no
    // threshold seal waits on the held one.
    for _ in 0..1500 {
        db.append(&tail_key(i), &tail_val(1));
        i += 1;
    }
    db.commit().unwrap();
    let n = i;
    let merges_before = db.phase_ns().2;
    let promoted_before = db.promotions();
    db.sync().unwrap();
    assert!(db.in_flight().0, "the held seal and the handed tail");
    assert_eq!(db.tail_swaps(), (0, 0));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers = tail_readers(&db, n, &stop);
    db.hold_seal_landing(false);
    wait_for("the piece and the tail landed and promoted", || {
        db.levels() == (parts + 2, 0) && db.unsealed_keys() == 0 && !db.in_flight().0
    });
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let ops: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(ops > 100, "the readers read: {ops}");
    assert_eq!(db.tail_swaps(), (0, 1), "the landing replaced the table");
    // The promotion unlinks the old names after its manifest, on the
    // segment work's thread: joined before the directory is read.
    db.settle().unwrap();
    assert_eq!(
        db.phase_ns().2,
        merges_before,
        "promoted by link, not merged"
    );
    let (promoted, open) = db.promotions();
    assert_eq!(
        promoted - promoted_before.0,
        2,
        "the held seal's piece and the tail, each promoted at its landing"
    );
    assert_eq!(
        open - promoted_before.1,
        1,
        "the tail promoted as a piece cut against fences a landing closed"
    );
    let names = sup_files(&d);
    assert!(
        names.len() == parts + 2 && names.iter().all(|nm| nm.starts_with("par-")),
        "partitions and nothing else: {names:?}"
    );
    let r = db.reader().unwrap();
    for k in 0..n {
        assert_eq!(read_vec(&r, &tail_key(k)), vec![tail_val(1)], "key {k}");
    }
    drop(r);
    db.close().unwrap();
    let db = Db::open(
        &d,
        Options {
            adaptive_shape: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(db.levels(), (parts + 2, 0));
    for k in (0..n).step_by(41) {
        assert_eq!(
            read_vec(&db, &tail_key(k)),
            vec![tail_val(1)],
            "key {k} reopened"
        );
    }
}

/// The WAL files in a store's directory, sorted by id.
fn wal_files(d: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut wals: Vec<std::path::PathBuf> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("wal-"))
        .collect();
    wals.sort();
    wals
}

/// Every key the store holds, read back by point read and by one scan,
/// against the values it should hold.
fn holds_all(db: &Reader, want: &BTreeMap<Vec<u8>, Vec<Vec<u8>>>, state: &str) {
    for (k, v) in want {
        let mut got = Vec::new();
        db.read_all(k, |x| got.push(x.to_vec())).unwrap();
        assert_eq!(&got, v, "{state}: key {:?}", String::from_utf8_lossy(k));
    }
    let mut scanned: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    db.scan(b"", usize::MAX, |k, v| {
        scanned.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    assert_eq!(&scanned, want, "{state}: the scan");
}

/// A seal that keeps the WAL (`seal_rotates_wal` off) rotates nothing:
/// the seal ends at the live file's sequence, a reopen replays the file
/// from the manifest's covered sequence and finds nothing to replay, and
/// the file rotates only once it passes `seal_bytes`, synced whole first,
/// retiring at the next seal's landing.
#[test]
fn a_seal_that_keeps_the_wal_retires_files_by_sequence() {
    let d = dir("walseq-retire");
    let opts = Options {
        seal_rotates_wal: false,
        seal_bytes: 1 << 20,
        seal_max_pct: 0,
        // Buffered, the shape whose commits never sync: a rotation must
        // sync the file itself before it starts the next.
        sync: supdb::SyncPolicy::EveryN(u32::MAX),
        // Ascending keys would go to ordered ingest, which writes a
        // segment and not the log.
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let put = |db: &mut Db, want: &mut BTreeMap<Vec<u8>, Vec<Vec<u8>>>, i: u32, ver: u32| {
        db.append(&tail_key(i), &tail_val(ver));
        want.entry(tail_key(i)).or_default().push(tail_val(ver));
    };
    for i in 0..3_000u32 {
        put(&mut db, &mut want, i, 1);
        if i % 100 == 99 {
            db.commit().unwrap();
        }
    }
    db.seal().unwrap();
    db.settle().unwrap();
    let w = db.seal_waits();
    assert_eq!(w.wal_rotations, 0, "the seal rotated nothing");
    assert_eq!(wal_files(&d).len(), 1, "one WAL, spanning the landed seal");
    assert!(db.levels().1 > 0 || db.levels().0 > 0, "the seal landed");
    holds_all(&db, &want, "after the seal");
    drop(db);
    let mut db = Db::open(&d, opts.clone()).unwrap();
    assert_eq!(
        db.unsealed_keys(),
        0,
        "the covered records are not replayed"
    );
    holds_all(&db, &want, "reopened over a WAL that spans the seal");

    // Past `seal_bytes` of log -- the first phase's bytes are in the file
    // still -- the commit rotates, synced first, while the memtable is
    // well under the seal.
    let levels = db.levels();
    let mut i = 3_000u32;
    while db.seal_waits().wal_rotations == 0 {
        put(&mut db, &mut want, i, 1);
        i += 1;
        if i.is_multiple_of(100) {
            db.commit().unwrap();
        }
        assert!(i < 20_000, "the log never rotated");
    }
    db.commit().unwrap();
    let w = db.seal_waits();
    assert_eq!(w.rotated_unsynced, 0, "every rotated file was synced whole");
    assert_eq!(db.levels(), levels, "nothing sealed since the reopen");
    assert_eq!(db.unsealed_keys(), (i - 3_000) as usize);
    assert_eq!(wal_files(&d).len(), 2, "the closed file and the live one");
    // The next seal covers every record in the closed file, and its
    // landing retires it.
    db.seal().unwrap();
    db.settle().unwrap();
    let wals = wal_files(&d);
    assert_eq!(wals.len(), 1, "the closed files retired: {wals:?}");
    holds_all(&db, &want, "after the rotation's seal");

    // A crash with committed writes past the last seal: the reopen
    // replays them from the live file behind the covered sequence.
    for i in 0..500u32 {
        put(&mut db, &mut want, i, 2);
        if i % 100 == 99 {
            db.commit().unwrap();
        }
    }
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    holds_all(&db, &want, "reopened after a crash past the seal");
}

/// The crash the rotation's sync exists for: the closed file whole and
/// the new file torn anywhere, including before its header. Every record
/// of the closed file comes back, and of the new file the batches before
/// the tear, each whole -- and the store opens, where a closed file with
/// an unsynced tail could leave a sequence gap replay refuses.
#[test]
fn a_rotated_wal_torn_in_its_new_file_reopens_to_a_prefix() {
    let d = dir("walseq-torn");
    let opts = Options {
        seal_rotates_wal: false,
        seal_bytes: 1 << 20,
        seal_max_pct: 0,
        sync: supdb::SyncPolicy::EveryN(u32::MAX),
        // Ascending keys would go to ordered ingest, which writes a
        // segment and not the log.
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // Records until the log has rotated once and before the memtable
    // reaches the seal: a log frame carries its header beside the key and
    // value, so the log passes `seal_bytes` first.
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        db.append(&tail_key(i), &tail_val(1));
        i += 1;
        if i.is_multiple_of(100) {
            db.commit().unwrap();
        }
        assert!(i < 20_000, "the log never rotated");
    }
    let closed = i;
    assert_eq!(
        db.levels(),
        (0, 0),
        "nothing sealed: the closed file is not covered"
    );
    assert_eq!(db.seal_waits().rotated_unsynced, 0);
    // Five batches of a hundred into the new file.
    for b in 0..5u32 {
        for j in 0..100u32 {
            db.append(&tail_key(closed + b * 100 + j), &tail_val(1));
        }
        db.commit().unwrap();
    }
    assert_eq!(db.levels(), (0, 0), "still nothing sealed");
    drop(db);
    let wals = wal_files(&d);
    assert_eq!(wals.len(), 2, "the closed file and the new one: {wals:?}");
    let full = std::fs::read(&wals[1]).unwrap();
    for cut in [
        0usize,
        3,
        full.len() / 3,
        full.len() / 2,
        full.len() - 1,
        full.len(),
    ] {
        std::fs::write(&wals[1], &full[..cut]).unwrap();
        let db = Db::open(&d, opts.clone())
            .unwrap_or_else(|e| panic!("the store must open with the new file cut at {cut}: {e}"));
        let mut got = 0u32;
        db.scan(b"", usize::MAX, |_, _| got += 1).unwrap();
        assert!(
            got >= closed,
            "cut {cut}: every record of the closed file ({closed}), got {got}"
        );
        assert_eq!(
            (got - closed) % 100,
            0,
            "cut {cut}: the new file's batches whole, got {got}"
        );
        if cut == full.len() {
            assert_eq!(got, closed + 500, "uncut, every batch");
        }
        drop(db);
        // The open truncated the file to its last commit; put it back.
        std::fs::write(&wals[1], &full).unwrap();
    }
}

/// A commit past the threshold while the frozen tables fill the list
/// keeps writing (`seal_defers`): the seals in flight are held before
/// they write, so a commit that waited for room would wait for good, and
/// the deferred count says the commits took the other path. The first
/// three crossings freeze into room and defer nothing. Released, a later
/// commit seals what grew, and every key reads right.
#[test]
fn a_commit_past_the_threshold_keeps_writing_while_the_frozen_list_is_full() {
    let d = dir("seal-defers");
    let opts = Options {
        seal_rotates_wal: false,
        seal_defers: true,
        seal_bytes: 256 << 10,
        seal_max_pct: 0,
        sync: supdb::SyncPolicy::EveryN(u32::MAX),
        // Ascending keys would go to ordered ingest, which writes a
        // segment and not the log.
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    // A commit that waited would never return; say so rather than hang.
    // Disarmed as the commits end, or by the unwind of a failure in them,
    // so an assertion below reports itself and not the watchdog.
    struct Disarm(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Disarm {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let disarm = Disarm(done.clone());
    let watch = {
        let done = done.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                if t.elapsed() > std::time::Duration::from_secs(60) {
                    eprintln!("a commit waited for a held seal: seal_defers did not defer");
                    std::process::exit(101);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        })
    };
    db.hold_seal_landing(true);
    let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    // Past the threshold three times, each sealing into room and held,
    // then past it again and under the ceiling of twice the threshold.
    let per = tail_key(0).len() + tail_val(1).len();
    let crossing = (256 << 10) / per as u32 + 100;
    let full = 3 * crossing;
    let n = full + (350 << 10) / per as u32;
    for i in 0..n {
        db.append(&tail_key(i), &tail_val(1));
        want.entry(tail_key(i)).or_default().push(tail_val(1));
        if i % 100 == 99 {
            db.commit().unwrap();
        }
        if i + 1 == full {
            db.commit().unwrap();
            assert_eq!(db.frozen_tables(), 3, "three crossings froze into room");
            assert_eq!(db.seal_waits().deferred, 0, "and deferred nothing");
        }
    }
    db.commit().unwrap();
    drop(disarm);
    watch.join().unwrap();
    let w = db.seal_waits();
    assert!(db.in_flight().0, "the seals are in flight, held");
    assert!(
        w.deferred > 0,
        "the commits past the threshold deferred: {w:?}"
    );
    assert_eq!(w.join_wait_ns, 0, "and none waited for the slot");
    holds_all(&db, &want, "beside the held seal");
    db.hold_seal_landing(false);
    // The seals land; the table that grew meanwhile seals at the first
    // commit that finds room. A writer that goes quiet
    // first leaves it live until it commits again -- this stage moves no
    // idle tail.
    db.settle().unwrap();
    let before = db.seal_waits().publishes;
    for i in n..n + 100 {
        db.append(&tail_key(i), &tail_val(1));
        want.entry(tail_key(i)).or_default().push(tail_val(1));
    }
    db.commit().unwrap();
    db.settle().unwrap();
    assert!(
        db.seal_waits().publishes > before,
        "the deferred seal happened once the list had room"
    );
    assert!(
        db.unsealed_keys() < 200,
        "and took what grew: {} unsealed",
        db.unsealed_keys()
    );
    holds_all(&db, &want, "after the deferred seal");
}

/// A commit under the seal threshold seals whenever the sealer is idle
/// (`seal_idle`) in a stretch of writes nobody reads: no table frozen or
/// handed, the live table past the floor, and no read since the commit
/// before. The seal it starts is held before it writes, so the sealer is
/// busy for as long as the test says: commits past the floor meanwhile
/// seal nothing more, and the first quiet commit after the landing seals
/// what grew. Writes with a read before every commit, as a mix makes
/// them, keep their writes in the table and seal nothing. Off, nothing
/// seals below the threshold at all. Every key reads right throughout.
#[test]
fn a_commit_seals_whenever_the_sealer_is_idle() {
    for idle in [false, true] {
        let d = dir(&format!("seal-idle-{idle}"));
        let opts = Options {
            seal_idle: idle,
            // Never by the threshold: every seal here is the idle rule's.
            seal_bytes: 1 << 30,
            seal_max_pct: 0,
            sync: supdb::SyncPolicy::EveryN(u32::MAX),
            direct_ingest: false,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        let per = tail_key(0).len() + tail_val(1).len();
        // A little past the floor's worth of keys, shuffled so the table
        // is hashed.
        let batch = (1u32 << 20) / per as u32 + 200;
        let mut next = 0u32;
        let mut write = |db: &mut Db, want: &mut BTreeMap<Vec<u8>, Vec<Vec<u8>>>, n: u32| {
            for i in 0..n {
                let k = tail_key((next + i).wrapping_mul(7919) % 1_000_003);
                db.append(&k, &tail_val(1));
                want.entry(k).or_default().push(tail_val(1));
                if i % 100 == 99 {
                    db.commit().unwrap();
                }
            }
            db.commit().unwrap();
            next += n;
        };
        let scan_once = |db: &Db| {
            db.scan(b"", 1, |_, _| {}).unwrap();
        };
        db.hold_seal_landing(true);
        let watch = Watchdog::arm("a commit waited for a held seal");
        write(&mut db, &mut want, batch);
        let first = db.seal_waits().idle_seals;
        assert_eq!(
            (first, db.frozen_tables()),
            if idle { (1, 1) } else { (0, 0) },
            "idle {idle}: one seal past the floor"
        );
        // Busy: past the floor again, and nothing more seals.
        write(&mut db, &mut want, batch);
        assert_eq!(db.seal_waits().idle_seals, first, "idle {idle}: busy");
        drop(watch);
        holds_all(&db, &want, &format!("idle {idle}: beside the held seal"));
        db.hold_seal_landing(false);
        db.settle().unwrap();
        // Idle again: the check's reads are one commit's window, and the
        // next quiet commit seals what grew.
        db.commit().unwrap();
        write(&mut db, &mut want, 100);
        db.settle().unwrap();
        assert_eq!(
            db.seal_waits().idle_seals,
            if idle { 2 } else { 0 },
            "idle {idle}: the next commit after the landing"
        );
        if idle {
            assert!(
                db.unsealed_keys() < 200,
                "idle {idle}: what grew went to the seal: {} unsealed",
                db.unsealed_keys()
            );
        } else {
            assert_eq!(db.segments(), 0, "off, nothing sealed under the threshold");
        }
        // A read before every commit, by key and by range in turn: past
        // the floor, and nothing seals.
        let probe: Vec<Vec<u8>> = want.keys().take(64).cloned().collect();
        for i in 0..batch {
            let k = tail_key((next + i).wrapping_mul(7919) % 1_000_003);
            db.append(&k, &tail_val(2));
            want.entry(k).or_default().push(tail_val(2));
            if i % 100 == 99 {
                if (i / 100) % 2 == 0 {
                    read_vec(&db, &probe[(i as usize / 100) % probe.len()]);
                } else {
                    scan_once(&db);
                }
                db.commit().unwrap();
            }
        }
        read_vec(&db, &probe[0]);
        db.commit().unwrap();
        db.settle().unwrap();
        assert_eq!(
            db.seal_waits().idle_seals,
            if idle { 2 } else { 0 },
            "idle {idle}: reads around keep the writes in the table"
        );
        assert!(
            db.unsealed_keys() >= batch as usize,
            "idle {idle}: unsealed"
        );
        holds_all(&db, &want, &format!("idle {idle}: after the landings"));
    }
}

/// A fresh store under the shape (`adaptive_shape`) seals its first load
/// at the floor (`seal_first_floor`), so the load leaves a partition
/// behind, where without it a store under `seal_bytes` stays wholly in
/// the memtable until something reads it.
#[test]
fn a_fresh_store_under_the_shape_seals_its_first_load_at_the_floor() {
    for floor in [false, true] {
        let d = dir(if floor {
            "first-floor"
        } else {
            "first-floor-off"
        });
        let opts = Options {
            adaptive_shape: true,
            seal_rotates_wal: false,
            seal_first_floor: floor,
            sync: supdb::SyncPolicy::EveryN(u32::MAX),
            ..Options::default()
        };
        let mut db = Db::create(&d, opts).unwrap();
        let n = 30_000u32;
        let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        for i in 0..n {
            let k = (i as u64 * 7919 % n as u64) as u32;
            db.append(&tail_key(k), &tail_val(1));
            want.entry(tail_key(k)).or_default().push(tail_val(1));
            if i % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        db.settle().unwrap();
        if floor {
            assert!(
                db.levels().0 >= 1,
                "the load left a partition: {:?}, unsealed {}",
                db.levels(),
                db.unsealed_keys()
            );
            assert!(
                db.unsealed_keys() < n as usize / 2,
                "most of the load sealed: {} unsealed",
                db.unsealed_keys()
            );
        } else {
            assert_eq!(db.levels(), (0, 0), "without the floor nothing sealed");
            assert_eq!(db.unsealed_keys(), n as usize);
        }
        holds_all(&db, &want, if floor { "the floor" } else { "no floor" });
    }
}

/// The window a closed file spends between a seal's phases: the seal that
/// takes it published and not durable, held there. The closed file is
/// kept and the manifest untouched for as long as the seal is between its
/// phases, and a crash there -- the drop joins the held seal and lands
/// nothing durable -- reopens on the manifest before it, replaying both
/// files: every committed record comes back.
#[test]
fn a_closed_wal_outlives_a_seal_between_its_phases_and_a_crash_there() {
    let d = dir("walseq-between");
    let opts = Options {
        seal_rotates_wal: false,
        seal_bytes: 1 << 20,
        seal_max_pct: 0,
        sync: supdb::SyncPolicy::EveryN(u32::MAX),
        direct_ingest: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        db.append(&tail_key(i), &tail_val(1));
        want.entry(tail_key(i)).or_default().push(tail_val(1));
        i += 1;
        if i.is_multiple_of(100) {
            db.commit().unwrap();
        }
        assert!(i < 20_000, "the log never rotated");
    }
    for _ in 0..3 {
        for _ in 0..100 {
            db.append(&tail_key(i), &tail_val(1));
            want.entry(tail_key(i)).or_default().push(tail_val(1));
            i += 1;
        }
        db.commit().unwrap();
    }
    assert_eq!(db.levels(), (0, 0), "nothing sealed yet");
    let wals = wal_files(&d);
    assert_eq!(wals.len(), 2, "the closed file and the live one: {wals:?}");
    let closed = wals[0].clone();
    let manifest = std::fs::read(d.join("manifest")).unwrap();
    db.hold_seal_durable(true);
    db.seal().unwrap();
    wait_for("the seal's publish", || db.levels() != (0, 0));
    assert!(db.in_flight().0, "the seal is between its phases");
    assert!(
        closed.exists(),
        "the closed file outlives the undurable seal"
    );
    assert_eq!(
        std::fs::read(d.join("manifest")).unwrap(),
        manifest,
        "no manifest while the seal is between its phases"
    );
    holds_all(&db, &want, "between the seal's phases");
    drop(db);
    let db = Db::open(&d, opts.clone()).unwrap();
    holds_all(&db, &want, "reopened after a crash between the phases");
    drop(db);
    // And once a seal lands after the reopen, the open's older file is
    // retired by it, as the closed one was to be.
    let mut db = Db::open(&d, opts).unwrap();
    db.append(&tail_key(i), &tail_val(1));
    want.entry(tail_key(i)).or_default().push(tail_val(1));
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert_eq!(
        wal_files(&d).len(),
        1,
        "the open's older files retired: {:?}",
        wal_files(&d)
    );
    holds_all(&db, &want, "after the reopen's seal");
}

/// A flush lands its seal readable before the seal's fsyncs, as the
/// poll does, rather than joining the seal's thread first: held between
/// its phases, the seal's table is already retired and its segment
/// published to a handle while the flush still waits for the fsyncs.
/// Joined whole, the flush published nothing until the hold lifted. On
/// both arms of the segment work, and with every key read back after.
#[test]
fn a_flush_publishes_its_seal_before_the_seal_is_durable() {
    for background in [true, false] {
        let d = dir(&format!("flush-readable-{background}"));
        let mut db = Db::create(
            &d,
            Options {
                publish_in_background: background,
                direct_ingest: false,
                ..Options::default()
            },
        )
        .unwrap();
        let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        for i in 0..2_000u32 {
            let k = tail_key(i.wrapping_mul(7919) % 100_003);
            db.append(&k, &tail_val(1));
            want.entry(k).or_default().push(tail_val(1));
        }
        db.commit().unwrap();
        let handle = db.reader().unwrap();
        let hold = std::time::Duration::from_millis(400);
        db.hold_seal_durable(true);
        let release = db.release_seal_durable_after(hold);
        let t0 = std::time::Instant::now();
        let watcher = std::thread::spawn(move || {
            while handle.unsealed_keys() > 0 {
                if t0.elapsed() > std::time::Duration::from_secs(10) {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Some(t0.elapsed())
        });
        db.flush().unwrap();
        let waited = t0.elapsed();
        release.join().unwrap();
        let seen = watcher
            .join()
            .unwrap()
            .expect("the seal's table was never retired");
        assert!(
            waited >= hold,
            "background {background}: the flush waited for the held fsyncs ({waited:?})"
        );
        assert!(
            seen < hold / 2,
            "background {background}: the segment was published at {seen:?}, \
             with the seal held until {hold:?}"
        );
        holds_all(
            &db,
            &want,
            &format!("background {background}: after the flush"),
        );
    }
}

/// A direct run under a policy that does not sync its commits hands its
/// pieces to writeback as they fill (`direct_writeback`), which no store
/// under the default policy reaches: a run of several pieces, flushed and
/// reopened, reads back every key, and so does one with the writeback off.
#[test]
fn a_buffered_direct_run_written_back_as_it_grows_reads_back_whole() {
    for writeback in [true, false] {
        let d = dir(&format!("direct-writeback-{writeback}"));
        let opts = Options {
            sync: supdb::SyncPolicy::EveryN(u32::MAX),
            direct_writeback: writeback,
            ..Options::default()
        };
        let mut want: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        {
            let mut db = Db::create(&d, opts.clone()).unwrap();
            // Ascending keys, a value of a few hundred bytes each: about
            // seven megabytes, three and more whole pieces of the run.
            for i in 0..24_000u32 {
                let k = tail_key(i);
                let v = vec![(i % 251) as u8; 300];
                db.append(&k, &v);
                want.entry(k).or_default().push(v);
                if i % 500 == 499 {
                    db.commit().unwrap();
                }
            }
            db.commit().unwrap();
            db.flush().unwrap();
            holds_all(
                &db,
                &want,
                &format!("writeback {writeback}: after the flush"),
            );
        }
        let db = Db::open(&d, opts).unwrap();
        holds_all(&db, &want, &format!("writeback {writeback}: reopened"));
    }
}
