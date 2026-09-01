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
fn a_torn_frame_stops_replay_at_the_crash_point() {
    let d = dir("torn");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    db.append(b"a", b"1");
    db.append(b"b", b"2");
    db.commit().unwrap();
    drop(db);

    // Tear the last frame: chop bytes off the WAL tail, the state a crash
    // mid-write leaves.
    let wal = d.join("wal-00000000");
    let len = std::fs::metadata(&wal).unwrap().len();
    let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
    f.set_len(len - 3).unwrap();
    drop(f);

    let db = Db::open(&d, NextOptions::default()).unwrap();
    assert_eq!(read_vec(&db, b"a"), vec![b"1".to_vec()], "intact frame survives");
    assert_eq!(read_vec(&db, b"b"), Vec::<Vec<u8>>::new(), "torn frame is the crash point");
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

#[test]
fn model_oracle_over_random_ops_and_crashes() {
    // The c2-oracle discipline at milestone-1 scale: random appends,
    // commits, seals, and crash-reopens, checked against a HashMap after
    // every reopen. Uncommitted writes are trimmed from the model at a
    // crash, which is the durability contract.
    let d = dir("oracle");
    let mut db = Db::create(&d, NextOptions::default()).unwrap();
    let mut model: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
    let mut uncommitted: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut state = 0x5eedu64;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for step in 0..2_000u32 {
        let key = format!("k{}", rng() % 97).into_bytes();
        let val = format!("v{step}").into_bytes();
        db.append(&key, &val);
        uncommitted.push((key, val));
        match rng() % 100 {
            0..=9 => {
                db.commit().unwrap();
                for (k, v) in uncommitted.drain(..) {
                    model.entry(k).or_default().push(v);
                }
            }
            10..=12 => {
                db.commit().unwrap();
                for (k, v) in uncommitted.drain(..) {
                    model.entry(k).or_default().push(v);
                }
                db.seal().unwrap();
            }
            13 => {
                drop(db); // crash: uncommitted appends vanish
                uncommitted.clear();
                db = match Db::open(&d, NextOptions::default()) {
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
    for (k, v) in uncommitted.drain(..) {
        model.entry(k).or_default().push(v);
    }
    for (k, want) in &model {
        assert_eq!(&read_vec(&db, k), want);
    }
}

fn small_opts(l0_trigger: usize) -> NextOptions {
    // Small enough that a few hundred records seal and compact, so the
    // level machinery is exercised at test scale rather than described.
    NextOptions { seal_bytes: 4 << 10, l0_trigger, ..NextOptions::default() }
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
        NextOptions { seal_bytes: 512 << 10, l0_trigger: 3, ..NextOptions::default() },
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
