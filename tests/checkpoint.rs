//! `Db::checkpoint`: a store as of the call made a store of its own.
//!
//! What a checkpoint is held to: it reads back exactly what the store read
//! at the call -- every commit before it, what was staged committed with
//! it, nothing after -- whatever the store held unsealed, in a direct run or
//! in flight at the time; it is opened beside the live store, under a lock
//! of its own, and neither sees what the other writes after; it outlives
//! the merges that retire the segments the two share; and its path is
//! absent or a whole store at every instant, so a path that exists is
//! refused and a half-made one is never left under the name.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use supdb::{Db, Options};

mod common;
use common::{kv, seg_names, vf};

/// A fresh directory for a store, and beside it the path a checkpoint of
/// it goes to, which does not exist.
fn dirs(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "supdb-next-checkpoint-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("store")).unwrap();
    (root.join("store"), root.join("checkpoint"))
}

fn root_of(store: &Path) -> &Path {
    store.parent().unwrap()
}

/// Every key and value a scan of the whole store answers, in order.
fn dump(db: &Db) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| out.push((k.to_vec(), v.to_vec()))),
    )
    .unwrap();
    out
}

fn values(db: &Db, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    db.read_all(key, vf(&mut |v| out.push(v.to_vec()))).unwrap();
    out
}

fn key(i: u32) -> Vec<u8> {
    format!("key-{i:06}").into_bytes()
}

/// Options that seal and merge at a few tens of kilobytes, so a store of a
/// few thousand keys has partitions, pieces and merges; the segment work on
/// a thread of its own or inline.
fn small(background: bool) -> Options {
    Options {
        seal_bytes: 32 << 10,
        partition_bytes: Some(32 << 10),
        l0_trigger: 2,
        publish_in_background: background,
        ..Options::default()
    }
}

/// A store with every kind of thing a checkpoint has to carry: an ordered
/// stretch (a direct run), a shuffled one (the log and the memtable),
/// keys with several values, deletes, replaced values, sealed segments
/// behind them and a last batch still unsealed.
fn a_store_with_everything(d: &Path, opts: Options) -> Db {
    let mut db = Db::create(d, opts).unwrap();
    for i in 0..3000u32 {
        db.append(&key(i), format!("v{i}").as_bytes());
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    // Shuffled, below and among the ordered keys, with second values.
    for j in 0..2000u32 {
        let i = (j * 7919) % 3000;
        db.append(&key(i), format!("w{j}").as_bytes());
        if j % 300 == 299 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    for i in (0..3000u32).step_by(7) {
        db.delete(&key(i));
    }
    for i in (3..3000u32).step_by(11) {
        db.put(&key(i), b"replaced");
    }
    db.commit().unwrap();
    // Unsealed at the call: a batch in the memtable, and keys above the
    // store's greatest in a run of ordered ingest.
    for i in 3000..3200u32 {
        db.append(&key(i), b"tail");
    }
    db.commit().unwrap();
    db
}

#[test]
fn a_checkpoint_reads_what_the_store_read_at_the_call_and_is_a_store_of_its_own() {
    for background in [true, false] {
        let (d, c) = dirs(&format!("whole-{background}"));
        let mut db = a_store_with_everything(&d, small(background));
        // Staged and not committed: a checkpoint commits it, as a flush
        // does, so it is in both stores.
        db.append(b"staged", b"at the call");
        db.checkpoint(&c).unwrap();
        let at_call = dump(&db);
        assert!(
            at_call.iter().any(|(k, _)| k == b"staged"),
            "what was staged is committed by the checkpoint"
        );
        assert_eq!(values(&db, &key(0)), Vec::<Vec<u8>>::new(), "a deleted key");
        assert_eq!(values(&db, &key(3)), vec![b"replaced".to_vec()]);
        assert_eq!(values(&db, &key(3100)), vec![b"tail".to_vec()]);

        // The store runs on: new keys, a delete and a replacement of keys
        // the checkpoint holds, and a drain that seals them.
        for i in 5000..5400u32 {
            db.append(&key(i), b"after");
        }
        db.delete(&key(1));
        db.put(&key(2), b"after");
        db.commit().unwrap();
        db.flush().unwrap();

        // Opened beside the live store, under its own lock.
        let mut ck = Db::open(&c, small(background)).unwrap();
        assert_eq!(dump(&ck), at_call, "background {background}");
        assert_eq!(values(&ck, &key(5000)), Vec::<Vec<u8>>::new());
        assert_eq!(
            ck.count(&key(1)).unwrap(),
            values(&ck, &key(1)).len() as u64
        );
        assert!(!values(&ck, &key(1)).is_empty(), "deleted after the call");
        assert_ne!(values(&ck, &key(2)), vec![b"after".to_vec()]);
        assert_eq!(ck.unsealed_keys(), 0, "a checkpoint has no log to replay");

        // And written on its own: the live store sees none of it.
        ck.append(b"only-in-the-checkpoint", b"x");
        ck.delete(&key(4));
        ck.commit().unwrap();
        ck.close().unwrap();
        let ck = Db::open(&c, small(background)).unwrap();
        assert_eq!(values(&ck, b"only-in-the-checkpoint"), vec![b"x".to_vec()]);
        assert!(values(&ck, &key(4)).is_empty());
        assert!(values(&db, b"only-in-the-checkpoint").is_empty());
        assert!(!values(&db, &key(4)).is_empty());
        assert_eq!(values(&db, &key(5000)), vec![b"after".to_vec()]);
        assert_eq!(values(&db, &key(2)), vec![b"after".to_vec()]);
        drop(ck);
        db.close().unwrap();
        let _ = std::fs::remove_dir_all(root_of(&d));
    }
}

#[test]
fn a_checkpoint_shares_its_segments_and_outlives_the_merges_that_retire_them() {
    let (d, c) = dirs("outlives");
    let mut db = a_store_with_everything(&d, small(true));
    db.checkpoint(&c).unwrap();
    let at_call = dump(&db);
    let shared = seg_names(&c);
    assert!(shared.len() >= 2, "a store of several segments: {shared:?}");
    // On one filesystem a segment is linked, not copied: one inode under
    // both names, and the store's files all there under the checkpoint's.
    for name in &shared {
        let (a, b) = (
            std::fs::metadata(d.join(name)).unwrap(),
            std::fs::metadata(c.join(name)).unwrap(),
        );
        assert_eq!((a.dev(), a.ino()), (b.dev(), b.ino()), "{name} is one file");
        assert!(a.nlink() >= 2, "{name} has both names");
    }
    // The store rewrites itself: every key replaced, round after round,
    // drained and settled, until none of the segments the checkpoint
    // shares is the store's any more.
    let t = std::time::Instant::now();
    let mut round = 0u32;
    while shared.iter().any(|n| d.join(n).exists()) {
        assert!(
            t.elapsed().as_secs() < 60,
            "the store's merges never retired {:?}",
            shared
                .iter()
                .filter(|n| d.join(n).exists())
                .collect::<Vec<_>>()
        );
        for i in 0..3200u32 {
            db.put(&key(i), format!("round-{round}").as_bytes());
            if i % 400 == 399 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        round += 1;
    }
    db.close().unwrap();
    // The checkpoint's names are the last on those files, and it reads
    // what the store read at the call.
    for name in &shared {
        assert_eq!(
            std::fs::metadata(c.join(name)).unwrap().nlink(),
            1,
            "{name}"
        );
    }
    let ck = Db::open(&c, small(true)).unwrap();
    assert_eq!(dump(&ck), at_call);
    drop(ck);
    // And the other way: the checkpoint removed, the store is whole.
    std::fs::remove_dir_all(&c).unwrap();
    let db = Db::open(&d, small(true)).unwrap();
    assert_eq!(
        values(&db, &key(10)),
        vec![format!("round-{}", round - 1).into_bytes()]
    );
    drop(db);
    let _ = std::fs::remove_dir_all(root_of(&d));
}

#[test]
fn a_checkpoint_at_a_path_that_exists_is_refused_and_nothing_changes() {
    let (d, c) = dirs("exists");
    let mut db = a_store_with_everything(&d, small(true));
    db.flush().unwrap();
    db.settle().unwrap();
    let before = dump(&db);
    let files = |p: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let store_files = files(&d);
    // An empty directory, a file, a dangling link and a store are each a
    // path that exists.
    std::fs::create_dir(&c).unwrap();
    let file = root_of(&d).join("a-file");
    std::fs::write(&file, b"x").unwrap();
    let dangling = root_of(&d).join("dangling");
    std::os::unix::fs::symlink(root_of(&d).join("nowhere"), &dangling).unwrap();
    for dest in [&c, &file, &dangling, &d] {
        let e = db.checkpoint(dest).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists, "{dest:?}: {e}");
    }
    assert_eq!(
        files(&c),
        Vec::<String>::new(),
        "the directory was left as it was"
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"x");
    assert_eq!(
        files(&d),
        store_files,
        "a refused checkpoint did not drain the store"
    );
    assert_eq!(dump(&db), before);
    // A path with no parent to be made in fails, and leaves nothing.
    let orphan = root_of(&d).join("no-such-parent").join("checkpoint");
    assert!(db.checkpoint(&orphan).is_err());
    assert!(!root_of(&d).join("no-such-parent").exists());
    assert_eq!(dump(&db), before);
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(root_of(&d));
}

#[test]
fn a_checkpoint_left_half_made_is_never_under_its_name_and_the_next_removes_it() {
    let (d, c) = dirs("half");
    let mut db = a_store_with_everything(&d, small(true));
    // What a crash inside a checkpoint leaves: the directory it was being
    // built in, under its temp name, with some of a store's segments and
    // no manifest -- which under the checkpoint's own name an open would
    // take for a store of exactly those segments.
    let half = root_of(&d).join("checkpoint.checkpoint-tmp");
    std::fs::create_dir(&half).unwrap();
    std::fs::write(
        half.join("seg-00000099-0000000000000001.sup"),
        b"half a segment",
    )
    .unwrap();
    assert!(!c.exists(), "nothing is under the checkpoint's name");
    assert!(Db::open(&c, small(true)).is_err());
    db.checkpoint(&c).unwrap();
    assert!(!half.exists(), "the half-made directory is gone");
    assert!(!c.join("seg-00000099-0000000000000001.sup").exists());
    let ck = Db::open(&c, small(true)).unwrap();
    assert_eq!(dump(&ck), dump(&db));
    drop(ck);
    // A second checkpoint to another name, of the same store.
    let c2 = root_of(&d).join("checkpoint-2");
    db.append(b"between", b"the two");
    db.commit().unwrap();
    db.checkpoint(&c2).unwrap();
    let ck2 = Db::open(&c2, small(true)).unwrap();
    assert_eq!(values(&ck2, b"between"), vec![b"the two".to_vec()]);
    let ck = Db::open(&c, small(true)).unwrap();
    assert!(values(&ck, b"between").is_empty());
    drop((ck, ck2));
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(root_of(&d));
}

/// A checkpoint carries on from the store's sequence, which its manifest
/// records as the store's does. Two things read it. The pieces over one
/// range order by the sequence their names carry, so a piece the
/// checkpoint seals has to be named above the ones it was made with, or a
/// read takes the older piece's value for the newest. And a replay skips
/// the records a manifest covers, so the checkpoint's log has to number
/// its records above what its segments' names cover, or a commit made in
/// the checkpoint is skipped at the open after a crash. The store here
/// keeps its pieces unmerged, so the checkpoint is made of pieces.
#[test]
fn a_checkpoint_carries_on_from_the_stores_sequence() {
    let (d, c) = dirs("sequence");
    let opts = || Options {
        seal_bytes: 16 << 10,
        compact: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts()).unwrap();
    for round in 0..4u32 {
        for j in 0..600u32 {
            // Shuffled, so the batches go through the log and every seal
            // leaves a piece over the whole range.
            let i = (j * 7919 + round) % 600;
            db.put(&key(i), format!("round-{round}").as_bytes());
        }
        db.commit().unwrap();
        db.flush().unwrap();
    }
    db.checkpoint(&c).unwrap();
    assert!(
        seg_names(&c).len() >= 4,
        "a checkpoint of unmerged pieces: {:?}",
        seg_names(&c)
    );
    db.close().unwrap();

    let mut ck = Db::open(&c, opts()).unwrap();
    for i in 0..600u32 {
        ck.put(&key(i), b"in the checkpoint");
    }
    ck.commit().unwrap();
    ck.flush().unwrap();
    for i in 0..600u32 {
        assert_eq!(
            values(&ck, &key(i)),
            vec![b"in the checkpoint".to_vec()],
            "key {i} after the checkpoint's own seal"
        );
    }
    // Committed and not sealed, then the process gone: the log replays it.
    ck.append(b"after-the-seal", b"replayed");
    ck.put(&key(7), b"replayed");
    ck.commit().unwrap();
    ck.crash();
    let ck = Db::open(&c, opts()).unwrap();
    assert_eq!(values(&ck, b"after-the-seal"), vec![b"replayed".to_vec()]);
    assert_eq!(values(&ck, &key(7)), vec![b"replayed".to_vec()]);
    for i in (0..600u32).filter(|&i| i != 7) {
        assert_eq!(
            values(&ck, &key(i)),
            vec![b"in the checkpoint".to_vec()],
            "key {i} after the reopen"
        );
    }
    drop(ck);
    let _ = std::fs::remove_dir_all(root_of(&d));
}

#[test]
fn a_checkpoint_of_an_empty_store_is_an_empty_store() {
    let (d, c) = dirs("empty");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.checkpoint(&c).unwrap();
    let mut ck = Db::open(&c, Options::default()).unwrap();
    assert_eq!(dump(&ck), Vec::new());
    ck.append(b"k", b"v");
    ck.commit().unwrap();
    assert_eq!(values(&ck, b"k"), vec![b"v".to_vec()]);
    assert!(values(&db, b"k").is_empty());
    drop(ck);
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(root_of(&d));
}

/// Where the checkpoint's path is on another filesystem a link is refused
/// and every file is copied: `/dev/shm` is one wherever it is a mount of
/// its own, which is every Linux host the checks run on. A host with none
/// says so and tests nothing here.
#[test]
fn a_checkpoint_on_another_filesystem_is_a_copy() {
    let (d, _) = dirs("copy");
    let shm = Path::new("/dev/shm");
    let other = shm.is_dir()
        && std::fs::metadata(shm).unwrap().dev() != std::fs::metadata(&d).unwrap().dev();
    if !other {
        eprintln!("no second filesystem at /dev/shm: the copy is not exercised on this host");
        let _ = std::fs::remove_dir_all(root_of(&d));
        return;
    }
    let c = shm.join(format!("supdb-next-checkpoint-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&c);
    let mut db = a_store_with_everything(&d, small(true));
    db.checkpoint(&c).unwrap();
    let at_call = dump(&db);
    for name in seg_names(&c) {
        let (a, b) = (
            std::fs::metadata(d.join(&name)).unwrap(),
            std::fs::metadata(c.join(&name)).unwrap(),
        );
        assert_ne!(a.dev(), b.dev(), "{name} is on the other filesystem");
        assert_eq!(a.len(), b.len(), "{name}");
        assert_eq!(a.nlink(), 1, "{name} was copied, not linked");
    }
    db.close().unwrap();
    std::fs::remove_dir_all(&d).unwrap();
    let ck = Db::open(&c, small(true)).unwrap();
    assert_eq!(dump(&ck), at_call);
    drop(ck);
    let _ = std::fs::remove_dir_all(&c);
    let _ = std::fs::remove_dir_all(root_of(&d));
}
