//! The engine's durability contract held against crash states built by
//! hand: a write the store acknowledged reads back after any crash the
//! acknowledgement promised to survive, and under a buffered policy what a
//! crash leaves is a state the store was in. Each test drives the store
//! into its window with the engine's own holds (and, where a failure has
//! to be made, the seams that inject it), copies its files at that instant
//! as a crash would leave them, and opens the copy. Nothing here asserts
//! what the engine happens to do; each test asserts the promise, so a test
//! that fails names a broken promise.
//!
//! One binary for every defect of the plain store's durability that was
//! reproduced and fixed together, a section each.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use supdb::{Db, Encryption, Options, Reader, StoreCipher};

mod common;
use common::{vf, Gcm};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read_vec(db: &Reader, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let n = db.read_all(key, vf(&mut |v| out.push(v.to_vec()))).unwrap();
    assert_eq!(n, out.len() as u64, "read_all's count against its values");
    out
}

/// Every file of a store copied as it stands: a crash at this instant on a
/// device that kept every completed write, which is what a process kill
/// leaves. A file gone between the listing and its copy is left out, as a
/// crash before its creation would leave it. A store under a cipher loses
/// its memory files here, as it does at a crash.
fn copy_store(from: &Path, to: &Path) {
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

fn open_run(d: &Path) -> bool {
    std::fs::read_dir(d).unwrap().any(|e| {
        let n = e.unwrap().file_name().to_string_lossy().into_owned();
        n.starts_with("direct-") && n.ends_with(".tmp")
    })
}

fn key(k: u32) -> String {
    format!("key-{k:06}")
}

fn under_cipher() -> Options {
    let c = StoreCipher::new(Gcm::new(&[7; 32]), Gcm::new).key_id(7);
    Options {
        encryption: Encryption::Cipher(c),
        ..Options::default()
    }
}

// ---------------------------------------------------------------------
// delete-above-max: a delete raises the store's greatest key, so a key a
// tombstone in the log names never opens a direct run, whose piece a
// recovery orders below the log's replayed frames.
// ---------------------------------------------------------------------

/// Which window the seal carrying the tombstone is held in when the store
/// crashes: before it writes anything, or with its segment renamed into
/// place and published to readers but unsynced and named by no manifest.
#[derive(Clone, Copy, Debug)]
enum SealHeld {
    BeforeWrite,
    BetweenPhases,
}

/// How the key is deleted: first in its batch, second in it (behind a
/// staged write), alone in its batch, in a transaction, or staged with no
/// commit, for the seal to commit.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Del {
    Delete,
    DeleteSecond,
    Alone,
    Txn,
    SealCommits,
}

/// The write that follows the delete.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    Append,
    AppendBatch,
    Put,
}

/// A store whose greatest key is key(999), loaded by a direct run and
/// flushed, its run's temp name retired.
fn loaded(d: &Path, opts: Options) -> Db {
    let mut db = Db::create(d, opts).unwrap();
    for k in 0..1000 {
        db.append(key(k).as_bytes(), b"base");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    // The flush is answered before the manifest's slot is synced and the
    // closed run's temp name retired; the settle joins that.
    db.settle().unwrap();
    assert!(!open_run(d), "the load's run closed at the flush");
    db
}

/// The key deleted, beside a write of key(10) so the batch has a record a
/// replay shows whatever a seal does with a lone tombstone (but for
/// `Del::Alone`), and committed (but for `Del::SealCommits`).
fn delete_and_commit(db: &mut Db, k: &str, del: Del) {
    match del {
        Del::Delete => {
            db.delete(k.as_bytes());
            db.put(key(10).as_bytes(), b"rewritten");
            db.commit().unwrap();
        }
        Del::DeleteSecond => {
            db.put(key(10).as_bytes(), b"rewritten");
            db.delete(k.as_bytes());
            db.commit().unwrap();
        }
        Del::Alone => {
            db.delete(k.as_bytes());
            db.commit().unwrap();
        }
        Del::Txn => {
            let mut t = db.begin();
            t.delete(k.as_bytes());
            t.delete(key(10).as_bytes());
            t.append(key(10).as_bytes(), b"rewritten");
            t.commit().unwrap();
        }
        Del::SealCommits => {
            db.delete(k.as_bytes());
            db.put(key(10).as_bytes(), b"rewritten");
        }
    }
}

fn hold(db: &Db, held: SealHeld, on: bool) {
    match held {
        SealHeld::BeforeWrite => db.hold_seal_landing(on),
        SealHeld::BetweenPhases => db.hold_seal_durable(on),
    }
}

/// The live table, holding the tombstone, frozen and handed to a seal held
/// in `held`'s window.
fn seal_held(db: &mut Db, held: SealHeld) {
    hold(db, held, true);
    let before = db.levels().1;
    db.seal().unwrap();
    if let SealHeld::BetweenPhases = held {
        wait_for("the seal's readable landing", || db.levels().1 > before);
    }
    assert!(db.in_flight().0, "the seal is in flight, held");
}

/// The key and two above it written after the delete, and committed.
fn write_after(db: &mut Db, k: &str, then: Then) {
    let ks = [k.to_string(), key(500_001), key(500_002)];
    match then {
        Then::Append => {
            for x in &ks {
                db.append(x.as_bytes(), b"after-delete");
            }
        }
        Then::AppendBatch => {
            let items: Vec<(&[u8], &[u8])> = ks
                .iter()
                .map(|x| (x.as_bytes(), &b"after-delete"[..]))
                .collect();
            db.append_batch(&items);
        }
        Then::Put => {
            for x in &ks {
                db.put(x.as_bytes(), b"after-delete");
            }
        }
    }
    db.commit().unwrap();
}

/// What the copy must read: the batch after the delete whole, the delete's
/// batch replayed, and the key with the value written after its delete --
/// at the first open and at the one after it, since the order a recovery
/// gives the sources is the store's for good.
fn holds_the_write(c: &Path, opts: &Options, k: &str, del: Del, tag: &str) {
    for open in ["the first open", "the next open"] {
        let r = Db::open(c, opts.clone()).unwrap();
        assert_eq!(
            read_vec(&r, key(500_002).as_bytes()),
            vec![b"after-delete".to_vec()],
            "{tag}, {open}: the write's last key was recovered (the premise)"
        );
        // A lone tombstone's batch has nothing else to show it replayed;
        // the key's own read below is then the whole check.
        let k10: Vec<Vec<u8>> = if del == Del::Alone {
            vec![b"base".to_vec()]
        } else {
            vec![b"rewritten".to_vec()]
        };
        assert_eq!(
            read_vec(&r, key(10).as_bytes()),
            k10,
            "{tag}, {open}: the delete's batch replayed (the premise)"
        );
        assert_eq!(
            read_vec(&r, k.as_bytes()),
            vec![b"after-delete".to_vec()],
            "{tag}, {open}: a write acknowledged after the key's delete reads back after the crash"
        );
    }
}

/// A delete of a key above the store's greatest, committed and frozen into
/// a seal that has not landed durably, then that key written again and
/// committed: both acknowledged under `Always`, in that order. After a
/// crash in either window of the seal the key reads its value written
/// after the delete. The delete's frames are in the log past what the
/// manifest covers and replay as newer than every segment; an append that
/// opened a direct run would be recovered as a piece below them.
fn delete_then_write_crash_while_sealing(
    opts: Options,
    held: SealHeld,
    del: Del,
    then: Then,
    tag: &str,
) {
    let tag = format!("{tag}-{held:?}-{del:?}-{then:?}");
    let d = dir(&format!("dur-del-above-max-{tag}"));
    let c = dir(&format!("dur-del-above-max-{tag}-crash"));
    assert!(matches!(opts.sync, supdb::SyncPolicy::Always));
    assert!(opts.direct_ingest, "the default ingests ordered keys");
    let mut db = loaded(&d, opts.clone());
    let k = key(500_000);
    delete_and_commit(&mut db, &k, del);
    seal_held(&mut db, held);
    write_after(&mut db, &k, then);
    if then == Then::Put {
        assert!(!open_run(&d), "a put opens no direct run");
    }
    // Control: the live store reads the write.
    assert_eq!(
        read_vec(&db, k.as_bytes()),
        vec![b"after-delete".to_vec()],
        "{tag}: the live store reads the acknowledged write"
    );

    // The crash.
    copy_store(&d, &c);
    hold(&db, held, false);
    db.settle().unwrap();
    assert_eq!(
        read_vec(&db, k.as_bytes()),
        vec![b"after-delete".to_vec()],
        "{tag}: the live store reads the write once the seal has landed"
    );
    drop(db);

    holds_the_write(&c, &opts, &k, del, &tag);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

#[test]
fn an_append_after_a_delete_survives_a_crash_before_the_deletes_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::Delete,
        Then::Append,
        "plain",
    );
}

#[test]
fn an_append_after_a_delete_survives_a_crash_between_the_deletes_seal_phases() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BetweenPhases,
        Del::Delete,
        Then::Append,
        "plain",
    );
}

#[test]
fn an_append_batch_after_a_delete_survives_a_crash_before_the_deletes_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::Delete,
        Then::AppendBatch,
        "plain",
    );
}

/// The delete made in a transaction, which applies it through the store's
/// own `delete`.
#[test]
fn an_append_after_a_transactions_delete_survives_a_crash_before_its_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::Txn,
        Then::Append,
        "plain",
    );
}

/// The delete second in its batch, behind a staged write: the raise does not
/// depend on the batch or the table being empty when the delete is made.
#[test]
fn an_append_after_a_delete_second_in_its_batch_survives_a_crash_before_its_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::DeleteSecond,
        Then::Append,
        "plain",
    );
}

/// The delete staged and committed by the seal that freezes it, which
/// commits what is staged without `commit`: the raise is the delete's own,
/// not a step of whichever call makes the batch durable.
#[test]
fn an_append_after_a_delete_its_seal_commits_survives_a_crash_before_the_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::SealCommits,
        Then::Append,
        "plain",
    );
}

/// The delete alone in its batch, into an empty table: the raise does not
/// depend on the table holding anything when the delete is made.
#[test]
fn an_append_after_a_lone_delete_survives_a_crash_before_its_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::Alone,
        Then::Append,
        "plain",
    );
}

/// Control: a put leaves no direct run and writes the log behind the
/// tombstone, so replay keeps the order.
#[test]
fn a_put_after_a_delete_survives_a_crash_before_the_deletes_seal_writes() {
    delete_then_write_crash_while_sealing(
        Options::default(),
        SealHeld::BeforeWrite,
        Del::Delete,
        Then::Put,
        "plain",
    );
}

/// The same order under a cipher. A run's commits there go to the log as
/// well, behind the tombstone, so the order holds by the log whatever the
/// greatest key says; the test holds that it stays so.
#[test]
fn an_append_after_a_delete_survives_a_crash_under_a_cipher() {
    for held in [SealHeld::BeforeWrite, SealHeld::BetweenPhases] {
        delete_then_write_crash_while_sealing(
            under_cipher(),
            held,
            Del::Delete,
            Then::Append,
            "cipher",
        );
    }
}

/// The delete made while the load's run is still open: the delete ends the
/// run (`leave_direct`) and goes to the log behind it, and the key it names
/// is above the run's greatest, so the rule holds on that path too.
#[test]
fn an_append_after_a_delete_that_ends_a_run_survives_a_crash_before_its_seal_writes() {
    let tag = "ends-a-run";
    let d = dir("dur-del-above-max-ends-a-run");
    let c = dir("dur-del-above-max-ends-a-run-crash");
    let opts = Options::default();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..1000 {
        db.append(key(k).as_bytes(), b"base");
    }
    db.commit().unwrap();
    assert!(open_run(&d), "the load's run is open (the premise)");
    // An uncommitted tail in the run: the delete that ends the run moves
    // it to the log ahead of itself, and the delete's commit makes it
    // durable there.
    let tail: Vec<String> = (1000..1010).map(key).collect();
    for t in &tail {
        db.append(t.as_bytes(), b"tail");
    }
    let k = key(500_000);
    delete_and_commit(&mut db, &k, Del::Delete);
    seal_held(&mut db, SealHeld::BeforeWrite);
    write_after(&mut db, &k, Then::Append);
    copy_store(&d, &c);
    hold(&db, SealHeld::BeforeWrite, false);
    db.settle().unwrap();
    assert_eq!(
        read_vec(&db, k.as_bytes()),
        vec![b"after-delete".to_vec()],
        "{tag}: the live store reads the write once the seal has landed"
    );
    for t in &tail {
        assert_eq!(
            read_vec(&db, t.as_bytes()),
            vec![b"tail".to_vec()],
            "{tag}: the live store reads the run's tail"
        );
    }
    drop(db);
    holds_the_write(&c, &opts, &k, Del::Delete, tag);
    for open in ["the first open", "the next open"] {
        let r = Db::open(&c, opts.clone()).unwrap();
        for t in &tail {
            assert_eq!(
                read_vec(&r, t.as_bytes()),
                vec![b"tail".to_vec()],
                "{tag}, {open}: the run's tail, committed with the delete that ended the run, was recovered"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

/// A tombstone the open replays out of the log into the memtable counts
/// toward the store's greatest key as a delete made live does: the store
/// reopened after a crash with the delete in its log, the table sealed and
/// held, the key appended and the store crashed again -- the key reads its
/// append.
#[test]
fn an_append_after_a_replayed_delete_survives_a_crash_before_its_seal_writes() {
    let tag = "replayed";
    let d = dir("dur-del-above-max-replayed");
    let c1 = dir("dur-del-above-max-replayed-crash1");
    let c2 = dir("dur-del-above-max-replayed-crash2");
    let opts = Options::default();
    let mut db = loaded(&d, opts.clone());
    let k = key(500_000);
    delete_and_commit(&mut db, &k, Del::Delete);
    copy_store(&d, &c1);
    drop(db);

    let mut db = Db::open(&c1, opts.clone()).unwrap();
    assert!(
        read_vec(&db, k.as_bytes()).is_empty(),
        "the delete replayed (the premise)"
    );
    assert_eq!(
        read_vec(&db, key(10).as_bytes()),
        vec![b"rewritten".to_vec()],
        "the delete's batch replayed into the memtable (the premise)"
    );
    seal_held(&mut db, SealHeld::BeforeWrite);
    write_after(&mut db, &k, Then::Append);
    copy_store(&c1, &c2);
    hold(&db, SealHeld::BeforeWrite, false);
    db.settle().unwrap();
    assert_eq!(
        read_vec(&db, k.as_bytes()),
        vec![b"after-delete".to_vec()],
        "{tag}: the live store reads the write once the seal has landed"
    );
    drop(db);

    holds_the_write(&c2, &opts, &k, Del::Delete, tag);
    for x in [&d, &c1, &c2] {
        let _ = std::fs::remove_dir_all(x);
    }
}

/// The same order under a buffered policy, after a process kill: nothing is
/// acknowledged durable at a commit, but what a crash leaves is a state the
/// store was in. A batch that came after the delete of its first key, and
/// is on the file whole because the next batch filled past it, holds that
/// key with its value wherever a recovery finds the batch.
#[test]
fn a_buffered_crash_never_recovers_a_batch_without_its_first_key() {
    let d = dir("dur-del-above-max-buffered");
    let c = dir("dur-del-above-max-buffered-crash");
    let opts = Options {
        sync: supdb::SyncPolicy::EveryN(u32::MAX),
        ..Options::default()
    };
    let mut db = loaded(&d, opts.clone());
    let k = key(500_000);
    delete_and_commit(&mut db, &k, Del::Delete);
    seal_held(&mut db, SealHeld::BeforeWrite);

    // Two batches of hundred-byte values: the first of 1,000, the second of
    // 9,500. Through a direct run the second carries its file past the
    // first 1 MiB write piece, so that piece, the first batch and its
    // marker whole, is on the file while the rest is in the writer's
    // buffer; through the log, both are written at their commits.
    let v = vec![b'v'; 100];
    for i in 0..1_000u32 {
        db.append(key(500_000 + i).as_bytes(), &v);
    }
    db.commit().unwrap();
    for i in 1_000..10_500u32 {
        db.append(key(500_000 + i).as_bytes(), &v);
    }
    db.commit().unwrap();

    copy_store(&d, &c);
    hold(&db, SealHeld::BeforeWrite, false);
    db.settle().unwrap();
    drop(db);

    let r = Db::open(&c, opts.clone()).unwrap();
    assert_eq!(
        read_vec(&r, key(500_999).as_bytes()),
        vec![v.clone()],
        "the first batch after the delete was recovered (the premise)"
    );
    assert_eq!(
        read_vec(&r, k.as_bytes()),
        vec![v.clone()],
        "a recovered batch holds its first key, appended after that key's delete"
    );
    drop(r);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

/// Every key of the model read back from `db`.
fn holds_the_model(db: &Reader, model: &BTreeMap<u32, Vec<Vec<u8>>>, tag: &str) {
    for (k, want) in model {
        assert_eq!(
            &read_vec(db, key(*k).as_bytes()),
            want,
            "{tag}: {}",
            key(*k)
        );
    }
}

/// Keys appended between the store's least and its greatest, across
/// several partitions, after `prep` has done whatever it does to the
/// store's greatest: read back live, after a kill, once flushed and
/// merged, and at a reopen. A greatest lowered below the store's keys lets
/// those appends open a direct run, which is cut for keys above every
/// partition and is refused by the merge that meets it below them.
fn appends_below_the_greatest_read_back(
    tag: &str,
    prep: impl FnOnce(&Path, Db, &Options) -> Db,
    deleted: Option<u32>,
) {
    let d = dir(&format!("dur-del-above-max-{tag}"));
    let c = dir(&format!("dur-del-above-max-{tag}-crash"));
    // Small seals and partitions that follow them, so a few hundred
    // records make several partitions.
    let opts = Options {
        seal_bytes: 4 << 10,
        l0_trigger: 2,
        partition_bytes: None,
        ..Options::default()
    };
    let mut model: BTreeMap<u32, Vec<Vec<u8>>> = BTreeMap::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for round in 0..6 {
        for k in 0..100 {
            let v = format!("v{round}-{k}").into_bytes();
            db.append(key(k).as_bytes(), &v);
            model.entry(k).or_default().push(v);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(db.levels().0 > 2, "{tag}: several partitions (the premise)");
    let mut db = prep(&d, db, &opts);
    if let Some(k) = deleted {
        model.insert(k, Vec::new());
    }
    for k in 6..100 {
        db.append(key(k).as_bytes(), b"after");
        model.get_mut(&k).unwrap().push(b"after".to_vec());
    }
    db.commit().unwrap();
    holds_the_model(&db, &model, &format!("{tag}, live"));
    copy_store(&d, &c);
    db.flush().unwrap();
    db.settle().unwrap();
    holds_the_model(&db, &model, &format!("{tag}, flushed and settled"));
    drop(db);
    for (at, what) in [(&d, "reopened"), (&c, "after the kill")] {
        let mut r = Db::open(at, opts.clone()).unwrap();
        holds_the_model(&r, &model, &format!("{tag}, {what}"));
        r.flush().unwrap();
        r.settle().unwrap();
        holds_the_model(&r, &model, &format!("{tag}, {what}, settled"));
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

/// A delete below the store's greatest leaves the greatest where it was:
/// the raise is a raise and never a lowering.
#[test]
fn a_delete_below_the_greatest_leaves_it_where_it_was() {
    appends_below_the_greatest_read_back(
        "below",
        |_, mut db, _| {
            db.delete(key(5).as_bytes());
            db.commit().unwrap();
            db.flush().unwrap();
            db.settle().unwrap();
            db
        },
        Some(5),
    );
}

/// An open takes the store's greatest from its segments' last keys as well
/// as its memtable's: a store reopened from segments alone keeps it.
#[test]
fn an_open_keeps_the_greatest_its_segments_hold() {
    appends_below_the_greatest_read_back(
        "open",
        |d, db, opts| {
            drop(db);
            Db::open(d, opts.clone()).unwrap()
        },
        None,
    );
}
