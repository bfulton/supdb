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

use supdb::{
    Db, Encryption, HelperFault, Options, Reader, SegmentOptions, SegmentWriter, StoreCipher,
    SyncPolicy,
};

mod common;
use common::{kv, seg_names, vf, Gcm};

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

// ---------------------------------------------------------------------
// log-errors: what the log does after the device refuses a write or a
// sync. A failed write is put back -- the file cut to its last good length,
// the batch staged again -- and the next commit writes it as frames a
// replay accepts; a failed fdatasync or fsync of the log is kept, and every
// commit, sync and flush after it answers with it until the store is
// reopened, since Linux reports a writeback error once and a later sync of
// the same file answers `Ok` over pages it never wrote.
// ---------------------------------------------------------------------

/// The words every answer after a kept error carries.
const KEPT: &str = "until it is reopened";

/// `put` and not `append`: an `append` of a key above the store's greatest
/// opens a direct run, whose commits never reach a plain store's log, and
/// `put` stays in the log under the default options.
fn put_commit(db: &mut Db, k: &str, v: &str) -> std::io::Result<()> {
    db.put(k.as_bytes(), v.as_bytes());
    db.commit()
}

fn reads(db: &Reader, k: &str, v: &str, what: &str) {
    assert_eq!(
        read_vec(db, k.as_bytes()),
        vec![v.as_bytes().to_vec()],
        "{what}: {k} reads back once"
    );
}

/// An answer that is the kept error: refused, in its words.
fn kept(r: std::io::Result<()>, what: &str) {
    match r {
        Ok(()) => panic!("{what}: answered Ok after the log's device error"),
        Err(e) => assert!(
            e.to_string().contains(KEPT),
            "{what}: answered with an error that is not the kept one: {e}"
        ),
    }
}

/// `kept`, of the kind the error that was kept had: a caller that tells
/// a device out of space from one that failed reads the same kind from
/// every answer after it.
fn kept_as(r: std::io::Result<()>, kind: std::io::ErrorKind, what: &str) {
    if let Err(e) = &r {
        assert_eq!(e.kind(), kind, "{what}: the kept error's kind: {e}");
    }
    kept(r, what);
}

/// `a_failed_write_is_written_again`'s `after` for a record whose seal
/// fails under a cipher, before anything is written.
const SEAL: usize = usize::MAX;

/// A store whose next log write fails after `after` bytes, through `put`,
/// whose batches go to the log alone: the commit answers the error, the
/// commits after it are acknowledged, and every one of them -- the failed
/// batch too, written again by the next commit and visible from that
/// commit on, once -- reads back live and from a
/// copy of the files, which under `Always` is what a kill and a power cut
/// both leave. With `reopened`, the store is opened from a kill's copy
/// before the failure, so its log is the file an open appends to
/// (`O_APPEND`), where a write lands at the file's end whatever the
/// cursor says and only the cut back of the file keeps the next one from
/// landing behind the failed write's bytes; otherwise its log is the file
/// the creation made, written at its cursor.
fn a_failed_write_is_written_again(tag: &str, opts: Options, after: usize, reopened: bool) {
    let d = dir(&format!("dur-walwrite-{tag}"));
    let crash = dir(&format!("dur-walwrite-{tag}-crash"));
    assert!(
        matches!(opts.sync, SyncPolicy::Always),
        "{tag}: the premise"
    );
    let mut db = if reopened {
        let first = dir(&format!("dur-walwrite-{tag}-first"));
        let mut db = Db::create(&first, opts.clone()).unwrap();
        put_commit(&mut db, "k0", "v0").unwrap();
        copy_store(&first, &d);
        drop(db);
        let _ = std::fs::remove_dir_all(&first);
        let db = Db::open(&d, opts.clone()).unwrap();
        assert_eq!(db.levels(), (0, 0), "{tag}: the log alone holds k0");
        db
    } else {
        let mut db = Db::create(&d, opts.clone()).unwrap();
        put_commit(&mut db, "k0", "v0").unwrap();
        db
    };
    if after == SEAL {
        db.fail_next_wal_seal();
    } else {
        db.fail_next_wal_write(after);
    }
    match put_commit(&mut db, "k1", "v1") {
        Ok(()) => panic!("{tag}: the injected write failure is the commit's answer"),
        // A seal that fails is answered before any write is made: the
        // answer is the seal's, not a write's of whatever the seal's
        // buffer held before.
        Err(e) if after == SEAL => assert!(
            e.to_string().contains("seal failed"),
            "{tag}: the commit's answer is the seal's and no write was made: {e}"
        ),
        Err(_) => {}
    }
    let h = db.reader().unwrap();
    assert!(
        read_vec(&h, b"k1").is_empty(),
        "{tag}: a batch whose commit failed is not visible to a handle"
    );
    put_commit(&mut db, "k2", "v2").unwrap();
    put_commit(&mut db, "k3", "v3").unwrap();
    for (r, what) in [(&*db, "the writer"), (&h, "a handle")] {
        reads(r, "k1", "v1", &format!("{tag}, live through {what}"));
        reads(r, "k2", "v2", &format!("{tag}, live through {what}"));
    }
    drop(h);
    assert_eq!(
        db.levels(),
        (0, 0),
        "{tag}: nothing sealed, the log alone holds the commits"
    );
    copy_store(&d, &crash);
    drop(db);
    for (at, what) in [(&crash, "the crash"), (&d, "the closed store")] {
        let db = Db::open(at, opts.clone()).unwrap_or_else(|e| panic!("{tag}: {what} opens: {e}"));
        let what = format!("{tag}, {what}");
        reads(&db, "k0", "v0", &format!("{what}: before the failure"));
        reads(&db, "k1", "v1", &format!("{what}: the batch written again"));
        reads(
            &db,
            "k2",
            "v2",
            &format!("{what}: acknowledged after the failure"),
        );
        reads(
            &db,
            "k3",
            "v3",
            &format!("{what}: acknowledged after the failure"),
        );
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A write that failed whole: nothing of it on the file.
#[test]
fn a_failed_log_write_loses_no_commit_acknowledged_after_it() {
    a_failed_write_is_written_again("zero", Options::default(), 0, false);
}

/// A write that put part of the batch on the file and then failed: the
/// prefix on the file and the cursor past it, which is what `write_all`
/// leaves after a short write and a failed one. Cut inside the first
/// frame's header (1, 5) and at the first frame's end (20: a delete frame
/// of a two-byte key is 8 + 8 + 1 + 1 + 2 bytes), which leaves a
/// well-formed record frame in front of whatever is written next.
#[test]
fn a_log_write_that_failed_part_way_loses_no_commit_acknowledged_after_it() {
    for after in [1usize, 5, 20] {
        a_failed_write_is_written_again(&format!("part-{after}"), Options::default(), after, false);
    }
}

/// The same cuts in a store reopened before the failure, whose log is
/// appended to (`O_APPEND`): the write after the failure lands at the
/// file's end, so the failed bytes have to be cut off it.
#[test]
fn a_log_write_that_failed_part_way_in_a_reopened_store_loses_no_commit_acknowledged_after_it() {
    for after in [1usize, 5, 20] {
        a_failed_write_is_written_again(
            &format!("reopened-part-{after}"),
            Options::default(),
            after,
            true,
        );
    }
}

/// The same under a cipher, whose log takes a commit's frames as one
/// sealed record: a record cut part way and one never written.
#[test]
fn a_failed_log_write_loses_no_commit_acknowledged_after_it_under_a_cipher() {
    for after in [0usize, 5, 40] {
        a_failed_write_is_written_again(&format!("cipher-{after}"), under_cipher(), after, false);
    }
    for after in [5usize, 40] {
        a_failed_write_is_written_again(
            &format!("cipher-reopened-{after}"),
            under_cipher(),
            after,
            true,
        );
    }
}

/// A record that fails to seal under a cipher -- a commit too large for
/// one record -- is a write that failed whole: the seal's buffer holds
/// the record before it, and none of it reaches the file.
#[test]
fn a_log_record_that_fails_to_seal_loses_no_commit_acknowledged_after_it() {
    a_failed_write_is_written_again("cipher-seal", under_cipher(), SEAL, false);
}

/// A commit with nothing new staged after a failed one writes the failed
/// batch, and its `Ok` is that batch's durability.
#[test]
fn a_commit_with_nothing_staged_after_a_failed_write_makes_the_batch_durable() {
    for (tag, opts) in [("plain", Options::default()), ("cipher", under_cipher())] {
        let d = dir(&format!("dur-walwrite-retry-{tag}"));
        let crash = dir(&format!("dur-walwrite-retry-{tag}-crash"));
        let mut db = Db::create(&d, opts.clone()).unwrap();
        put_commit(&mut db, "k0", "v0").unwrap();
        db.fail_next_wal_write(5);
        assert!(put_commit(&mut db, "k1", "v1").is_err(), "{tag}");
        db.commit().unwrap();
        copy_store(&d, &crash);
        drop(db);
        let db = Db::open(&crash, opts).unwrap();
        reads(&db, "k0", "v0", tag);
        reads(
            &db,
            "k1",
            "v1",
            &format!("{tag}: committed by the commit after"),
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// Under a cipher an ordered run's commits go to the log as well
/// (`Db::log_run`), and its own file is a memory file until the run
/// closes, so the log is where they are durable. A run's batch whose log
/// write failed is in the table and counted as the run's, and is written
/// to the log by whatever comes next -- a commit with nothing new, a
/// `sync`, or a commit of the run's next batch -- once, under that call's
/// commit frame, durable at that call's `Ok` as that call's policy makes
/// it: a commit with nothing new, a `sync` (`then` 1) and a `flush` (4)
/// sync it whatever the policy, a commit of the run's next batch (2) syncs
/// it where the policy syncs that commit, and 3 is that commit under the
/// buffered policy, which leaves it unsynced, with a `flush` after it.
/// Unlike a batch
/// of the log alone it is visible from the commit that failed, since the
/// run marks its table before the log is written (`SyncPolicy`).
fn a_runs_batch_is_written_again(tag: &str, then: u8, opts: Options) {
    {
        let d = dir(&format!("dur-walwrite-run-{tag}"));
        let crash = dir(&format!("dur-walwrite-run-{tag}-crash"));
        let mut db = Db::create(&d, opts.clone()).unwrap();
        db.append(b"a0", b"v0");
        db.commit().unwrap();
        assert_eq!(db.direct_runs(), (1, 1), "{tag}: the premise, a logged run");
        db.commit().unwrap();
        db.sync().unwrap();
        assert_eq!(
            db.direct_runs(),
            (1, 1),
            "{tag}: a commit and a sync with nothing new write nothing to the log"
        );
        db.fail_next_wal_write(5);
        db.append(b"a1", b"v1");
        assert!(
            db.commit().is_err(),
            "{tag}: the failure is the commit's answer"
        );
        let h = db.reader().unwrap();
        reads(
            &h,
            "a1",
            "v1",
            &format!("{tag}: a run's batch is visible from its failed commit"),
        );
        drop(h);
        match then {
            0 => db.commit().expect("a commit with nothing new"),
            1 => db.sync().expect("a sync"),
            4 => db.flush().expect("a flush"),
            _ => {
                db.append(b"a2", b"v2");
                db.commit().expect("the run's next commit");
                if then == 3 {
                    // The buffered policy: the run's next commit writes the
                    // batch with its own and syncs neither, as it syncs no
                    // commit of its own; the flush after it makes both
                    // durable with the rest of the run.
                    let (_, synced, written) = db.wal_durable();
                    assert!(
                        synced < written,
                        "{tag}: the run's next commit, durable as its policy makes it, \
                         left the log unsynced (synced {synced}, written {written})"
                    );
                    let runs = db.direct_runs();
                    db.flush().expect("a flush after the run's next commit");
                    assert_eq!(
                        db.direct_runs(),
                        runs,
                        "{tag}: the flush wrote nothing more to the log"
                    );
                }
            }
        }
        let runs = db.direct_runs();
        if then < 3 {
            assert_eq!(
                db.levels(),
                (0, 0),
                "{tag}: the run is open, the log alone holds it"
            );
        }
        // A power cut: the log as far as a sync covered it, which is
        // every byte the call answered `Ok` for -- any call's under
        // `Always`, the `sync`'s under the buffered policy.
        let (log, synced, _) = db.wal_durable();
        copy_store(&d, &crash);
        drop(db);
        std::fs::OpenOptions::new()
            .write(true)
            .open(crash.join(log.file_name().unwrap()))
            .unwrap()
            .set_len(synced)
            .unwrap();
        let db = Db::open(&crash, opts.clone())
            .unwrap_or_else(|e| panic!("{tag}: the crash opens: {e}"));
        reads(&db, "a0", "v0", &format!("{tag}: before the failure"));
        reads(&db, "a1", "v1", &format!("{tag}: the batch written again"));
        if then == 2 || then == 3 {
            reads(&db, "a2", "v2", &format!("{tag}: the commit after"));
        }
        assert_eq!(
            runs,
            (1, 2),
            "{tag}: the run's batch went to the log at that call, once"
        );
        drop(db);
        let db = Db::open(&d, opts).unwrap();
        reads(&db, "a1", "v1", &format!("{tag}: the closed store"));
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

#[test]
fn a_runs_batch_whose_log_write_failed_under_a_cipher_is_written_by_a_commit_of_nothing() {
    a_runs_batch_is_written_again("commit", 0, under_cipher());
}

#[test]
fn a_runs_batch_whose_log_write_failed_under_a_cipher_is_written_by_a_sync() {
    a_runs_batch_is_written_again("sync", 1, under_cipher());
}

/// Under the buffered policy a `sync` is the call whose `Ok` promises
/// the batch: the write again syncs whatever the policy, since the commit
/// that failed may have owed the sync and its count is spent.
#[test]
fn a_runs_batch_whose_log_write_failed_under_a_buffered_cipher_is_written_by_a_sync() {
    a_runs_batch_is_written_again(
        "sync-buffered",
        1,
        Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..under_cipher()
        },
    );
}

#[test]
fn a_runs_batch_whose_log_write_failed_under_a_cipher_is_written_by_the_runs_next_commit() {
    a_runs_batch_is_written_again("next", 2, under_cipher());
}

/// Under the buffered policy the run's next commit writes the batch again
/// with its own and is as durable as that policy makes it: the frames are
/// on the file and not synced, and the `flush` after it is the call whose
/// `Ok` promises both (`SyncPolicy`).
#[test]
fn a_runs_batch_whose_log_write_failed_under_a_buffered_cipher_is_written_by_the_runs_next_commit()
{
    a_runs_batch_is_written_again(
        "next-buffered",
        3,
        Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..under_cipher()
        },
    );
}

/// A `flush` writes the run's batch again and syncs it whatever the
/// policy, before the run closes into a segment.
#[test]
fn a_runs_batch_whose_log_write_failed_under_a_buffered_cipher_is_written_by_a_flush() {
    a_runs_batch_is_written_again(
        "flush-buffered",
        4,
        Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..under_cipher()
        },
    );
}

/// The same failure under the buffered policy, acknowledged by a `sync`:
/// a `sync` that returns `Ok` makes everything committed before it
/// durable. The seal `sync` hands the live table to (`adaptive_shape`) is
/// held before it writes anything, so the copy is the crash that falls
/// between the answer and the seal, where the log is the store.
#[test]
fn a_sync_after_a_failed_log_write_makes_every_commit_before_it_durable() {
    let d = dir("dur-walwrite-sync");
    let crash = dir("dur-walwrite-sync-crash");
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    put_commit(&mut db, "k0", "v0").unwrap();
    db.fail_next_wal_write(0);
    assert!(put_commit(&mut db, "k1", "v1").is_err());
    put_commit(&mut db, "k2", "v2").unwrap();
    put_commit(&mut db, "k3", "v3").unwrap();
    db.hold_seal_landing(true);
    db.sync().unwrap();
    copy_store(&d, &crash);
    db.hold_seal_landing(false);
    drop(db);
    assert!(
        seg_names(&crash).is_empty(),
        "the held seal wrote nothing: the crash's store is its log alone"
    );
    let db = Db::open(&crash, opts).unwrap();
    reads(&db, "k0", "v0", "sync");
    reads(&db, "k1", "v1", "sync: the batch written again");
    reads(&db, "k2", "v2", "sync: committed before it");
    reads(&db, "k3", "v3", "sync: committed before it");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// After a failed write the log rotates by size (`seal_rotates_wal` off,
/// the default) before any seal retires the closed file: the store opens,
/// with every acknowledged commit, and with no gap in the sequence across
/// the two files.
#[test]
fn a_store_whose_log_rotated_after_a_failed_write_opens_with_every_acknowledged_commit() {
    let d = dir("dur-walwrite-rot");
    let crash = dir("dur-walwrite-rot-crash");
    let opts = Options {
        seal_rotates_wal: false,
        // The log passes `seal_bytes` before the memtable reaches the
        // seal: a frame carries a header beside its key and value, and a
        // `put` a delete frame as well.
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    put_commit(&mut db, "a0000000", "v").unwrap();
    db.fail_next_wal_write(5);
    assert!(put_commit(&mut db, "a0000001", "v").is_err());
    let val = "x".repeat(100);
    let mut acked: Vec<String> = Vec::new();
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        let k = format!("b{i:07}");
        put_commit(&mut db, &k, &val).unwrap();
        acked.push(k);
        i += 1;
        assert!(i < 10_000, "the log never rotated");
    }
    put_commit(&mut db, "c0000000", &val).unwrap();
    acked.push("c0000000".into());
    assert_eq!(
        db.levels(),
        (0, 0),
        "nothing sealed: the closed file is not retired, the window under test"
    );
    copy_store(&d, &crash);
    drop(db);
    assert!(crash.join("wal-00000000").exists(), "the closed file");
    assert!(
        crash.join("wal-00000001").exists(),
        "the file the rotation began"
    );
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the crash opens: {e}"));
    reads(&db, "a0000000", "v", "rotation");
    reads(&db, "a0000001", "v", "rotation: the batch written again");
    for k in &acked {
        reads(&db, k, &val, "rotation: acknowledged under Always");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A failed write whose bytes cannot be cut back off the file leaves a
/// log the next write would land behind a stray prefix in: the error is
/// kept, every commit, sync and flush after it answers with it while
/// reads go on, and the store opens again at what its log holds.
#[test]
fn a_failed_write_that_cannot_be_put_back_is_kept_until_the_store_is_reopened() {
    let d = dir("dur-walwrite-stuck");
    let opts = Options::default();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    put_commit(&mut db, "k0", "v0").unwrap();
    db.fail_next_wal_write(5);
    db.fail_next_wal_cut();
    kept(
        put_commit(&mut db, "k1", "v1"),
        "the commit whose write could not be put back",
    );
    kept(put_commit(&mut db, "k2", "v2"), "a commit after");
    kept(db.sync(), "a sync after");
    kept(db.flush(), "a flush after");
    kept(db.commit(), "a commit after a flush");
    reads(&db, "k0", "v0", "reads go on");
    kept(db.close(), "the close");
    let mut db = Db::open(&d, opts.clone()).unwrap();
    reads(&db, "k0", "v0", "reopened");
    for k in ["k1", "k2"] {
        let got = read_vec(&db, k.as_bytes());
        assert!(got.len() <= 1, "reopened: {k} read back {got:?}");
    }
    put_commit(&mut db, "k4", "v4").expect("a reopened store commits again");
    db.flush().unwrap();
    reads(&db, "k4", "v4", "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Every call that writes the log keeps a write it could not put back,
/// whichever it is: the call answers the kept error, and so does the
/// commit after it; the store opens again at what its log holds.
fn a_stray_write_is_kept(
    tag: &str,
    opts: Options,
    run: bool,
    call: fn(&mut Db) -> std::io::Result<()>,
) {
    let d = dir(&format!("dur-walwrite-stray-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    if run {
        db.append(b"a0", b"v0");
        db.commit().unwrap();
        assert_eq!(db.direct_runs(), (1, 1), "{tag}: the premise, a logged run");
        db.append(b"a1", b"v1");
    } else {
        put_commit(&mut db, "a0", "v0").unwrap();
        db.put(b"a1", b"v1");
    }
    db.fail_next_wal_write(5);
    db.fail_next_wal_cut();
    kept(
        call(&mut db),
        &format!("{tag}: the call whose write could not be put back"),
    );
    // The seam's own premise: the failed write's prefix reached the file,
    // past what the log counts written, as a short write leaves it.
    let (log, _, written) = db.wal_durable();
    assert!(
        std::fs::metadata(&log).unwrap().len() > written,
        "{tag}: the failed write left its bytes on the file"
    );
    db.append(b"a2", b"v2");
    kept(db.commit(), &format!("{tag}: a commit after"));
    kept(db.sync(), &format!("{tag}: a sync after"));
    reads(&db, "a0", "v0", &format!("{tag}: reads go on"));
    drop(db);
    let db = Db::open(&d, opts).unwrap_or_else(|e| panic!("{tag}: the store opens again: {e}"));
    reads(&db, "a0", "v0", &format!("{tag}: reopened"));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_write_that_cannot_be_put_back_is_kept_by_a_sync() {
    a_stray_write_is_kept("sync", Options::default(), false, |db| db.sync());
}

#[test]
fn a_write_that_cannot_be_put_back_is_kept_by_a_flush() {
    a_stray_write_is_kept("flush", Options::default(), false, |db| db.flush());
}

#[test]
fn a_write_that_cannot_be_put_back_is_kept_by_a_seal() {
    a_stray_write_is_kept("seal", Options::default(), false, |db| db.seal());
    let rotates = Options {
        seal_rotates_wal: true,
        ..Options::default()
    };
    a_stray_write_is_kept("seal-rotates", rotates, false, |db| db.seal());
}

#[test]
fn a_write_that_cannot_be_put_back_is_kept_by_a_runs_commit_under_a_cipher() {
    a_stray_write_is_kept("run", under_cipher(), true, |db| db.commit());
}

/// Under the default policy a commit whose fdatasync failed leaves the
/// store unable to answer `Ok` for anything after it until it is reopened,
/// and an `Ok` that was given anyway is durable: every commit answered
/// `Ok` reads back from the store as a power cut after a later, successful
/// sync of the same file leaves it (`Db::fail_next_wal_sync`). The store
/// is flushed first so its writes after go to the log and not to a direct
/// run (a key below the store's greatest), the path of every update.
fn a_commit_after_a_failed_sync(tag: &str, opts: Options) {
    let d = dir(&format!("dur-walsync-commit-{tag}"));
    let crash = dir(&format!("dur-walsync-commit-{tag}-crash"));
    assert!(matches!(opts.sync, SyncPolicy::Always), "the premise");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for c in 0u32..100 {
        db.append(format!("m{c:04}").as_bytes(), &c.to_le_bytes());
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.append(b"a-acked", b"A");
    db.commit().unwrap();
    db.fail_next_wal_sync();
    db.append(b"b-failed", b"B");
    assert!(
        db.commit().is_err(),
        "{tag}: the injected failure reached the log's sync"
    );
    db.append(b"c-after", b"C");
    let c = db.commit();
    copy_store(&d, &crash);
    drop(db);
    let reopened = Db::open(&crash, opts).expect("a store whose log lost unsynced frames opens");
    assert_eq!(
        read_vec(&reopened, b"a-acked"),
        vec![b"A".to_vec()],
        "{tag}"
    );
    for i in 0u32..100 {
        assert_eq!(
            read_vec(&reopened, format!("m{i:04}").as_bytes()),
            vec![i.to_le_bytes().to_vec()],
            "{tag}: the flushed keys"
        );
    }
    if c.is_ok() {
        assert_eq!(
            read_vec(&reopened, b"c-after"),
            vec![b"C".to_vec()],
            "{tag}: a commit answered Ok after a failed sync of the log is durable"
        );
    }
    kept(c, &format!("{tag}: the commit after a failed sync"));
    drop(reopened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_commit_answered_ok_after_a_failed_log_sync_survives_a_crash() {
    a_commit_after_a_failed_sync("plain", Options::default());
}

#[test]
fn a_commit_answered_ok_after_a_failed_log_sync_survives_a_crash_under_a_cipher() {
    a_commit_after_a_failed_sync("cipher", under_cipher());
}

/// Under a buffered policy `sync` is the promise: a `sync` after a failed
/// one answers with the failure, and one answered `Ok` would have made
/// every commit before it durable.
#[test]
fn a_sync_answered_ok_after_a_failed_one_makes_every_commit_before_it_durable() {
    let d = dir("dur-walsync-sync");
    let crash = dir("dur-walsync-sync-crash");
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for c in 0u32..100 {
        db.append(format!("m{c:04}").as_bytes(), &c.to_le_bytes());
    }
    db.commit().unwrap();
    db.flush().unwrap();
    for c in 0u32..20 {
        db.append(format!("a{c:04}").as_bytes(), &c.to_le_bytes());
        db.commit().unwrap();
    }
    db.fail_next_wal_sync();
    assert!(
        db.sync().is_err(),
        "the injected failure reached the log's sync"
    );
    let again = db.sync();
    copy_store(&d, &crash);
    drop(db);
    let reopened = Db::open(&crash, opts).expect("a store whose log lost unsynced frames opens");
    for c in 0u32..100 {
        assert_eq!(
            read_vec(&reopened, format!("m{c:04}").as_bytes()),
            vec![c.to_le_bytes().to_vec()],
            "the flushed keys"
        );
    }
    if again.is_ok() {
        for c in 0u32..20 {
            assert_eq!(
                read_vec(&reopened, format!("a{c:04}").as_bytes()),
                vec![c.to_le_bytes().to_vec()],
                "a sync answered Ok after a failed one made the commits before it durable"
            );
        }
    }
    kept(again, "the sync after a failed one");
    drop(reopened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A failed fdatasync of the log is kept: every commit, sync, flush and
/// transaction after it answers with it while reads go on; the close
/// answers it too and seals nothing; and the store reopens at a prefix of
/// what it acknowledged and commits again.
#[test]
fn a_failed_log_sync_is_kept_until_the_store_is_reopened() {
    let d = dir("dur-walsync-kept");
    let opts = Options::default();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    put_commit(&mut db, "a", "A").unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    put_commit(&mut db, "b", "B").unwrap();
    db.fail_next_wal_sync();
    let first = put_commit(&mut db, "c", "C");
    // The seam fails the sync as a writeback out of space would.
    let full = std::io::ErrorKind::StorageFull;
    kept_as(first, full, "the commit whose sync failed");
    kept_as(put_commit(&mut db, "d", "D"), full, "a commit after");
    kept_as(db.commit(), full, "a commit of nothing after");
    kept_as(db.sync(), full, "a sync after");
    kept_as(db.flush(), full, "a flush after");
    let mut t = db.begin();
    t.append(b"e", b"E");
    kept_as(t.commit(), full, "a transaction after");
    kept_as(db.seal(), full, "a seal after");
    reads(&db, "a", "A", "reads go on");
    reads(&db, "b", "B", "reads go on");
    let segs = seg_names(&d);
    kept(db.close(), "the close");
    assert_eq!(
        seg_names(&d),
        segs,
        "the close under a kept error sealed nothing"
    );
    let mut db = Db::open(&d, opts).unwrap();
    reads(&db, "a", "A", "reopened: flushed");
    reads(&db, "b", "B", "reopened: synced before the failure");
    assert!(
        read_vec(&db, b"c").is_empty(),
        "reopened: the frames the failed sync lost"
    );
    put_commit(&mut db, "f", "F").expect("a reopened store commits again");
    db.sync().expect("and syncs");
    reads(&db, "f", "F", "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A buffered store whose log has rotated by size with frames no sync had
/// covered: a loose file, synced by the first sync after it, behind the
/// directory fsync the new file's name is owed.
fn rotated_loose(tag: &str) -> (PathBuf, Db) {
    let d = dir(&format!("dur-walsync-{tag}"));
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("b{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "the log never rotated");
    }
    assert!(
        db.seal_waits().rotated_unsynced > 0,
        "{tag}: a loose rotation"
    );
    (d, db)
}

/// A failed fdatasync of a loose log file is kept.
#[test]
fn a_failed_sync_of_a_loose_log_file_is_kept() {
    let (d, mut db) = rotated_loose("loose");
    db.fail_next_loose_wal_sync();
    assert!(
        db.sync().is_err(),
        "the injected failure reached the loose file"
    );
    kept(db.sync(), "the sync after");
    kept(put_commit(&mut db, "z", "Z"), "a commit after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A failed fsync of the directory a rotation owes its new log file is
/// kept, where the sync by policy owes it.
#[test]
fn a_failed_sync_of_the_log_directory_is_kept() {
    let (d, mut db) = rotated_loose("dir");
    db.fail_next_wal_dir_sync();
    assert!(
        db.sync().is_err(),
        "the injected failure reached the barrier"
    );
    kept(db.sync(), "the sync after");
    kept(put_commit(&mut db, "z", "Z"), "a commit after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The same where a rotation under `Always` owes the barrier itself: the
/// commit that rotated was synced into the closed file and answers `Ok`,
/// and everything after answers the kept error.
#[test]
fn a_failed_sync_of_the_log_directory_at_a_rotation_is_kept() {
    let d = dir("dur-walsync-dir-always");
    let opts = Options {
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    db.fail_next_wal_dir_sync();
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("b{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "the log never rotated");
    }
    assert!(
        put_commit(&mut db, "y", "Y").is_err(),
        "the rotation's error"
    );
    kept(put_commit(&mut db, "z", "Z"), "a commit after");
    kept(db.sync(), "a sync after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------
// run-errors: what a direct run does after the device refuses a write or
// a sync of its file, which is the run's own log. A failed write leaves
// the batch owed: every byte of it stays with the writer, nothing on the
// file past what the file is known good to counts, and the next commit,
// commit of nothing or sync writes it again at that offset before it
// answers `Ok`. A failed fdatasync of the run's file -- the commit's own
// or the helper thread's behind the writer -- is kept, as the log's is,
// and never retried: not by a later sync, and not by the run's close.
// ---------------------------------------------------------------------

/// The run's arms (`bench/src/engines.rs`): each keeps an older shape of
/// the run alive, and each reaches the writes these tests fail.
fn run_arms() -> Vec<(&'static str, Options)> {
    let d = Options::default;
    vec![
        ("default", d()),
        (
            "copyvals",
            Options {
                direct_stream: false,
                ..d()
            },
        ),
        (
            "threecalls",
            Options {
                direct_stream: false,
                direct_put_one: false,
                ..d()
            },
        ),
        (
            "refvals",
            Options {
                direct_stream: false,
                direct_table_refs: true,
                ..d()
            },
        ),
        (
            "norecycle",
            Options {
                direct_recycle: false,
                ..d()
            },
        ),
        (
            "keysarena",
            Options {
                direct_keys_in_stream: false,
                ..d()
            },
        ),
        (
            "keycopies",
            Options {
                direct_derive_keys: false,
                ..d()
            },
        ),
        (
            "fatentries",
            Options {
                direct_slim_entries: false,
                ..d()
            },
        ),
    ]
}

/// The open run's temp file in `d`: the run's own log until it closes.
fn run_tmp_name(d: &Path) -> std::ffi::OsString {
    std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .find(|n| {
            let n = n.to_string_lossy();
            n.starts_with("direct-") && n.ends_with(".tmp")
        })
        .expect("an open run's temp file")
}

/// A power cut of a store whose run is open: its files copied from `d` to
/// `crash`, every name of the run's file (`ino`) cut to what an fdatasync
/// covered (`synced`), and the log to its synced length.
fn power_cut(db: &Db, d: &Path, crash: &Path, ino: u64, synced: u64) {
    use std::os::unix::fs::MetadataExt;
    let (log, log_synced, _) = db.wal_durable();
    copy_store(d, crash);
    for e in std::fs::read_dir(d).unwrap() {
        let e = e.unwrap();
        if e.metadata().map(|m| m.ino()).ok() == Some(ino) {
            std::fs::OpenOptions::new()
                .write(true)
                .open(crash.join(e.file_name()))
                .unwrap()
                .set_len(synced)
                .unwrap();
        }
    }
    if let Some(name) = log.file_name() {
        if let Ok(f) = std::fs::OpenOptions::new()
            .write(true)
            .open(crash.join(name))
        {
            f.set_len(log_synced).unwrap();
        }
    }
}

fn run_ino(d: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(d.join(run_tmp_name(d))).unwrap().ino()
}

fn injected(r: std::io::Result<()>, what: &str) {
    match r {
        Ok(()) => panic!("{what}: answered Ok over the run's failed write"),
        Err(e) => assert!(
            e.to_string().contains("injected"),
            "{what}: answered with an error that is not the device's: {e}"
        ),
    }
}

/// `a_run_write_that_failed`'s ways to write the owed batch again.
#[derive(Clone, Copy, Debug)]
enum Again {
    /// A commit with nothing new.
    Nothing,
    /// A `sync`.
    Sync,
    /// The run's next commit, with a batch of its own.
    Next,
}

/// A run's commit whose write fails in its sync arm -- the batch counted
/// and visible, its tail held for the file -- answers the device's error,
/// and the batch is owed: the call after it, whichever `again` names,
/// writes it at the offset the file is good to and syncs it before it
/// answers `Ok`, and a power cut after that Ok -- the run's file cut to
/// what its fdatasyncs covered -- reads it back once. With `partial`, the
/// failed write left half of a part on the file first.
fn a_run_write_that_failed(tag: &str, opts: Options, again: Again, partial: bool) {
    let d = dir(&format!("dur-runwrite-{tag}"));
    let crash = dir(&format!("dur-runwrite-{tag}-crash"));
    assert!(matches!(opts.sync, SyncPolicy::Always), "the premise");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"k1", b"v1");
    db.commit().unwrap();
    assert!(open_run(&d), "{tag}: the premise, a run");
    if partial {
        db.fail_next_run_write_partial();
    } else {
        db.fail_next_run_write();
    }
    db.append(b"k2", b"v2");
    injected(db.commit(), &format!("{tag}: the failed write's commit"));
    match again {
        Again::Nothing => db.commit().expect("a commit of nothing writes the batch"),
        Again::Sync => db.sync().expect("a sync writes the batch"),
        Again::Next => {
            db.append(b"k3", b"v3");
            db.commit().expect("the run's next commit writes both");
        }
    }
    assert_eq!(db.levels(), (0, 0), "{tag}: the run is open");
    // Paid: the writer asks nothing more of a call with nothing new, so
    // the commits after it take the path they took before the failure.
    assert_eq!(
        db.run_attends(),
        Some(false),
        "{tag}: the run's writer still attends after the batch was paid"
    );
    let ino = run_ino(&d);
    let synced = db.run_synced_bytes().expect("a run open");
    power_cut(&db, &d, &crash, ino, synced);
    let h = db.reader().unwrap();
    reads(&h, "k2", "v2", &format!("{tag}: live"));
    drop(h);
    std::mem::forget(db);
    for (at, what) in [(&crash, "the power cut"), (&d, "the kill")] {
        let db = Db::open(at, opts.clone()).unwrap_or_else(|e| panic!("{tag}: {what} opens: {e}"));
        reads(
            &db,
            "k1",
            "v1",
            &format!("{tag}, {what}: before the failure"),
        );
        reads(
            &db,
            "k2",
            "v2",
            &format!("{tag}, {what}: the batch written again"),
        );
        if matches!(again, Again::Next) {
            reads(&db, "k3", "v3", &format!("{tag}, {what}: the commit after"));
        }
        drop(db);
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_run_write_that_failed_is_written_again_by_a_commit_of_nothing() {
    for (tag, o) in run_arms() {
        a_run_write_that_failed(&format!("nothing-{tag}"), o, Again::Nothing, false);
    }
}

#[test]
fn a_run_write_that_failed_is_written_again_by_a_sync() {
    for (tag, o) in run_arms() {
        a_run_write_that_failed(&format!("sync-{tag}"), o, Again::Sync, false);
    }
}

#[test]
fn a_run_write_that_failed_is_written_again_by_the_runs_next_commit() {
    for (tag, o) in run_arms() {
        a_run_write_that_failed(&format!("next-{tag}"), o, Again::Next, false);
    }
}

/// A write that lands part of a batch and then fails leaves bytes on the
/// file the writer does not count; the write after it lands at the offset
/// the file is good to, over them, and not where the failed write left
/// the file's cursor.
#[test]
fn a_run_write_that_failed_part_way_loses_no_commit_acknowledged_after_it() {
    for (tag, o) in run_arms() {
        a_run_write_that_failed(&format!("partial-{tag}"), o.clone(), Again::Next, true);
        a_run_write_that_failed(&format!("partial-sync-{tag}"), o, Again::Sync, true);
    }
}

/// The run's first write, its file's head with the first batch, fails:
/// the batch is owed as any other, and the run goes on.
#[test]
fn a_runs_first_write_that_failed_is_written_again() {
    for (tag, o) in run_arms() {
        let d = dir(&format!("dur-runwrite-first-{tag}"));
        let mut db = Db::create(&d, o.clone()).unwrap();
        db.fail_next_run_write();
        db.append(b"k1", b"v1");
        injected(db.commit(), &format!("{tag}: the run's first commit"));
        db.append(b"k2", b"v2");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the commit after: {e}"));
        std::mem::forget(db);
        let db = Db::open(&d, o).unwrap();
        reads(&db, "k1", "v1", &format!("{tag}: the first batch"));
        reads(&db, "k2", "v2", &format!("{tag}: the commit after"));
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A commit whose batch carries the run's file across a write piece, and
/// whose piece write fails: the commit answers the device's error, the
/// piece is held, and the next commit -- a key of its own, no order
/// broken -- writes it and its own batch, and a power cut after that Ok
/// reads both back.
fn a_failed_run_piece_write(tag: &str, opts: Options) {
    let d = dir(&format!("dur-runpiece-{tag}"));
    let crash = dir(&format!("dur-runpiece-{tag}-crash"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.fail_next_run_write();
    let val = vec![b'x'; 100];
    // About 3.4 MB of records: the batch crosses the 1 MiB write piece, so
    // its first write is a whole piece's, before the batch's sync.
    for i in 1..30_000u32 {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    injected(db.commit(), &format!("{tag}: the piece's failed write"));
    db.append(b"b0000001", b"v");
    db.commit()
        .unwrap_or_else(|e| panic!("{tag}: the commit after the failed piece: {e}"));
    assert_eq!(db.levels(), (0, 0), "{tag}: the run is open");
    let ino = run_ino(&d);
    let synced = db.run_synced_bytes().expect("a run open");
    power_cut(&db, &d, &crash, ino, synced);
    std::mem::forget(db);
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
    let what = format!("{tag}: the batch taken again and the commit after it");
    reads(&db, "a0000000", "v", &what);
    let x = String::from_utf8(val).unwrap();
    reads(&db, "a0000001", &x, &what);
    reads(&db, "a0029999", &x, &what);
    reads(&db, "b0000001", "v", &what);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A write the device takes only part of answers the count it took, and
/// the rest is written at the offset that count reached: a commit whose
/// write is short once -- the tail at its sync, or with `big` the piece a
/// batch of 3.4 MB crosses -- answers `Ok`, and a power cut after the
/// commit behind it reads both back.
fn a_short_run_write(tag: &str, opts: Options, big: bool) {
    let d = dir(&format!("dur-runshort-{tag}"));
    let crash = dir(&format!("dur-runshort-{tag}-crash"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    assert!(open_run(&d), "{tag}: the premise, a run");
    db.short_next_run_write();
    let val = vec![b'x'; 100];
    let n = if big { 30_000u32 } else { 2 };
    for i in 1..n {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    db.commit()
        .unwrap_or_else(|e| panic!("{tag}: the commit whose write was short: {e}"));
    db.append(b"b0000001", b"v");
    db.commit()
        .unwrap_or_else(|e| panic!("{tag}: the commit after: {e}"));
    assert_eq!(db.levels(), (0, 0), "{tag}: the run is open");
    let ino = run_ino(&d);
    let synced = db.run_synced_bytes().expect("a run open");
    power_cut(&db, &d, &crash, ino, synced);
    std::mem::forget(db);
    for (at, what) in [(&crash, "the power cut"), (&d, "the kill")] {
        let db = Db::open(at, opts.clone()).unwrap_or_else(|e| panic!("{tag}: {what} opens: {e}"));
        let what = format!("{tag}, {what}: the short write and the commit after it");
        reads(&db, "a0000000", "v", &what);
        let x = String::from_utf8(val.clone()).unwrap();
        reads(&db, "a0000001", &x, &what);
        reads(&db, &format!("a{:07}", n - 1), &x, &what);
        reads(&db, "b0000001", "v", &what);
        drop(db);
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_short_run_write_is_written_whole_at_the_offset_it_reached() {
    for (tag, o) in run_arms() {
        a_short_run_write(&format!("tail-{tag}"), o.clone(), false);
        a_short_run_write(&format!("piece-{tag}"), o, true);
    }
}

#[test]
fn a_failed_run_piece_write_is_held_and_written_by_the_next_commit() {
    for (tag, o) in run_arms() {
        a_failed_run_piece_write(tag, o);
    }
}

/// A run with a failed write, closed by a flush: the close computes its
/// offsets from what the writer counts, and the flush answers `Ok` only
/// over a segment that holds every batch; under every close the arms keep
/// (`Options::close_presync`, `Options::close_finish_inline`).
#[test]
fn a_run_write_that_failed_part_way_is_flushed_whole() {
    let mut arms = run_arms();
    arms.push((
        "latesync",
        Options {
            close_presync: false,
            ..Options::default()
        },
    ));
    arms.push((
        "finishinline",
        Options {
            close_finish_inline: true,
            ..Options::default()
        },
    ));
    for (tag, o) in arms {
        let d = dir(&format!("dur-runflush-{tag}"));
        let mut db = Db::create(&d, o.clone()).unwrap();
        db.append(b"k1", b"v1");
        db.commit().unwrap();
        db.fail_next_run_write_partial();
        db.append(b"k2", b"v2");
        injected(db.commit(), &format!("{tag}: the failed write's commit"));
        db.append(b"k3", b"v3");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the commit after: {e}"));
        db.flush()
            .unwrap_or_else(|e| panic!("{tag}: the flush: {e}"));
        assert!(!open_run(&d) || db.levels() != (0, 0), "{tag}: flushed");
        std::mem::forget(db);
        let db = Db::open(&d, o).unwrap_or_else(|e| panic!("{tag}: opens: {e}"));
        for (k, v) in [("k1", "v1"), ("k2", "v2"), ("k3", "v3")] {
            reads(&db, k, v, &format!("{tag}: flushed"));
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A run's failed fdatasync is kept: the commit answers it, and every
/// commit, commit of nothing, sync and flush after it answers it in the
/// kept words, never with a retried fdatasync that Linux would answer
/// `Ok` over pages it failed to write back and marked clean. A power cut
/// leaves the run's file as far as the last successful fdatasync reached,
/// and the store opens there.
fn a_failed_run_sync_is_kept(tag: &str, opts: Options) {
    let d = dir(&format!("dur-runsync-{tag}"));
    let crash = dir(&format!("dur-runsync-{tag}-crash"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"k1", b"v1");
    db.commit().unwrap();
    let before = db.run_synced_bytes().expect("a run open");
    db.fail_next_run_sync();
    db.append(b"k2", b"v2");
    let first = db.commit();
    assert!(
        first
            .as_ref()
            .is_err_and(|e| e.to_string().contains("injected")),
        "{tag}: the failed fdatasync is its commit's answer: {first:?}"
    );
    assert_eq!(
        db.run_synced_bytes(),
        Some(before),
        "{tag}: nothing counts as synced over the failed fdatasync"
    );
    kept(db.commit(), &format!("{tag}: a commit of nothing after"));
    let synced = db.sync();
    db.append(b"k3", b"v3");
    kept(db.commit(), &format!("{tag}: a commit after"));
    kept(db.flush(), &format!("{tag}: a flush after"));
    reads(&db, "k1", "v1", &format!("{tag}: reads go on"));
    let ino = run_ino(&d);
    let covered = db.run_synced_bytes().expect("the run is still open");
    power_cut(&db, &d, &crash, ino, covered);
    std::mem::forget(db);
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
    reads(
        &db,
        "k1",
        "v1",
        &format!("{tag}: the commit before the failure"),
    );
    drop(db);
    // The seam fails before the fdatasync is issued, so a retried one here
    // really syncs; on Linux a writeback error is reported once per open
    // file description and the pages are marked clean, so a retry would
    // answer Ok over bytes the device never took. Only a refusal is sound.
    kept(
        synced,
        &format!("{tag}: a sync after the run's failed fdatasync"),
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_failed_run_fdatasync_is_kept_until_the_store_is_reopened() {
    for (tag, o) in run_arms() {
        a_failed_run_sync_is_kept(tag, o);
    }
}

/// A run whose fdatasync failed is not closed into a segment a manifest
/// names: a write below the greatest leaves the run, and its close would
/// finish the file and fsync it -- a retry of the failed sync, which Linux
/// answers `Ok` -- and the landing's manifest would name a file whose
/// pages the device never took. The power cut cuts every name of the
/// run's file to what its fdatasyncs covered, and the store still opens,
/// with every commit acknowledged before the failure; the reopened store
/// commits again.
#[test]
fn a_run_whose_fdatasync_failed_is_not_closed_into_a_named_segment() {
    for (tag, o) in run_arms() {
        let d = dir(&format!("dur-runsync-leave-{tag}"));
        let crash = dir(&format!("dur-runsync-leave-{tag}-crash"));
        let mut db = Db::create(&d, o.clone()).unwrap();
        db.append(b"k1", b"v1");
        db.commit().unwrap();
        db.fail_next_run_sync();
        db.append(b"k2", b"v2");
        assert!(db.commit().is_err(), "{tag}: the failed fdatasync");
        let ino = run_ino(&d);
        let covered = db.run_synced_bytes().expect("the run is open");
        // Below the greatest: the run is left, and its close begins.
        db.put(b"a", b"A");
        kept(
            db.commit(),
            &format!("{tag}: the commit of the write that left"),
        );
        let _ = db.settle();
        power_cut(&db, &d, &crash, ino, covered);
        std::mem::forget(db);
        let mut db =
            Db::open(&crash, o.clone()).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
        reads(
            &db,
            "k1",
            "v1",
            &format!("{tag}: acknowledged before the failure"),
        );
        db.put(b"z", b"Z");
        db.commit().expect("a reopened store commits");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// Under a cipher the run's file is a memory file and its commits go to
/// the log as well (`Db::log_run`); a failed write of the run's file is
/// owed as in the clear, and the call that writes it again writes the
/// batch to the log too, so a power cut -- the memory file gone, the log
/// cut to what a sync covered -- reads it back.
#[test]
fn a_run_write_that_failed_under_a_cipher_is_written_again() {
    for (tag, again) in [
        ("nothing", Again::Nothing),
        ("sync", Again::Sync),
        ("next", Again::Next),
    ] {
        let tag = format!("cipher-{tag}");
        let d = dir(&format!("dur-runwrite-{tag}"));
        let crash = dir(&format!("dur-runwrite-{tag}-crash"));
        let opts = under_cipher();
        let mut db = Db::create(&d, opts.clone()).unwrap();
        db.append(b"k1", b"v1");
        db.commit().unwrap();
        assert_eq!(db.direct_runs(), (1, 1), "{tag}: the premise, a logged run");
        db.fail_next_run_write();
        db.append(b"k2", b"v2");
        injected(db.commit(), &format!("{tag}: the failed write's commit"));
        match again {
            Again::Nothing => db.commit().expect("a commit of nothing"),
            Again::Sync => db.sync().expect("a sync"),
            Again::Next => {
                db.append(b"k3", b"v3");
                db.commit().expect("the run's next commit");
            }
        }
        let (log, synced, _) = db.wal_durable();
        copy_store(&d, &crash);
        std::mem::forget(db);
        std::fs::OpenOptions::new()
            .write(true)
            .open(crash.join(log.file_name().unwrap()))
            .unwrap()
            .set_len(synced)
            .unwrap();
        let db =
            Db::open(&crash, opts.clone()).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
        reads(&db, "k1", "v1", &format!("{tag}: before the failure"));
        reads(&db, "k2", "v2", &format!("{tag}: the batch written again"));
        if matches!(again, Again::Next) {
            reads(&db, "k3", "v3", &format!("{tag}: the commit after"));
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// Under a cipher a failed fdatasync of the run's memory file is kept as
/// in the clear: nothing is retried.
#[test]
fn a_failed_run_fdatasync_is_kept_under_a_cipher() {
    let d = dir("dur-runsync-cipher");
    let mut db = Db::create(&d, under_cipher()).unwrap();
    db.append(b"k1", b"v1");
    db.commit().unwrap();
    db.fail_next_run_sync();
    db.append(b"k2", b"v2");
    assert!(db.commit().is_err(), "the failed fdatasync");
    kept(db.commit(), "a commit of nothing after");
    kept(db.sync(), "a sync after");
    reads(&db, "k1", "v1", "reads go on");
    drop(db);
    let db = Db::open(&d, under_cipher()).unwrap();
    reads(&db, "k1", "v1", "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

fn hkey(k: u32) -> Vec<u8> {
    format!("key-{k:06}").into_bytes()
}

fn hval(k: u32) -> Vec<u8> {
    format!("value-{k:06}-{}", "x".repeat(100)).into_bytes()
}

fn wait_count(n: &std::sync::atomic::AtomicU64, at_least: u64, what: &str) {
    wait_for(what, || {
        n.load(std::sync::atomic::Ordering::Acquire) >= at_least
    });
}

/// The helper thread behind a marked segment's writer -- a direct run's --
/// fails its fdatasync of part of the records; it keeps the error for the
/// finish, and the writer's own fdatasync, on the same open file
/// description, reports nothing. The writer's `sync` answers the helper's
/// failure, and every `sync` after it.
#[test]
fn a_marked_segments_sync_reports_its_helpers_failed_sync() {
    let d = dir("dur-helper-writer");
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let opts = SegmentOptions {
        write_piece: 4096,
        sync_ahead: 16384,
        fail_helper_syncs: Some(failed.clone()),
        ..Default::default()
    };
    let mut w = SegmentWriter::create(&d.join("run.sup"), &opts).unwrap();
    // A direct run's writer: records first, a marker a batch.
    w.set_inline_max(256);
    w.set_marks(true);
    for k in 0..3000u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    w.mark().unwrap();
    // Several `sync_ahead`s are on the file: the helper's turn comes, and
    // its sync fails.
    wait_count(&failed, 1, "the helper never took its turn at a sync");
    assert!(
        w.sync().is_err(),
        "a sync over records whose earlier fdatasync failed is not answered Ok"
    );
    assert!(w.sync().is_err(), "nor is the sync after it");
    drop(w);
    let _ = std::fs::remove_dir_all(&d);
}

/// A marked segment's own fdatasync that failed is every sync's answer
/// after it, and its finish's: none of them is a retry Linux would answer
/// `Ok` over pages it marked clean.
#[test]
fn a_marked_segments_failed_sync_is_every_later_syncs_answer() {
    let d = dir("dur-writer-sync-kept");
    let opts = SegmentOptions {
        write_piece: 4096,
        ..Default::default()
    };
    let mut w = SegmentWriter::create(&d.join("run.sup"), &opts).unwrap();
    w.set_inline_max(256);
    w.set_marks(true);
    for k in 0..100u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    w.mark().unwrap();
    w.fail_next_sync();
    let first = w.sync().expect_err("the failed fdatasync");
    assert!(first.to_string().contains("injected"), "{first}");
    // A sync with nothing new to write: nothing is owed, and a retried
    // fdatasync would answer Ok over the pages the failed one left.
    let again = w
        .sync()
        .expect_err("a sync with nothing new after the failed fdatasync");
    assert!(again.to_string().contains("injected"), "{again}");
    // A batch after it: its pieces are refused by the file, the writer
    // holds them, and the sync's failure stays the answer -- not the
    // writes' that came after it.
    for k in 100..200u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    w.mark().unwrap();
    for i in 0..3 {
        let e = w.sync().expect_err("a sync after the failed fdatasync");
        assert!(
            e.to_string().contains("injected"),
            "sync {i} after the failed fdatasync answers it: {e}"
        );
    }
    assert!(w.finish(1).is_err(), "the finish, whose fsync is the retry");
    let _ = std::fs::remove_dir_all(&d);
}

/// A writer that is not a run's -- a seal's, a merge's, a caller's with
/// sorted input -- answers a failed write where it happens: it is given
/// up, and holding the segment's bytes for a sync that never comes would
/// hold them all.
#[test]
fn an_unmarked_segments_failed_write_is_answered_where_it_happens() {
    let d = dir("dur-writer-write-unmarked");
    let opts = SegmentOptions {
        write_piece: 4096,
        ..Default::default()
    };
    let mut w = SegmentWriter::create(&d.join("seg.sup"), &opts).unwrap();
    w.set_inline_max(256);
    w.fail_next_write();
    let mut failed = false;
    for k in 0..1000u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        if w.end_with(false).is_err() {
            failed = true;
            break;
        }
    }
    assert!(
        failed,
        "the failed write is the answer of the call that met it"
    );
    drop(w);
    let _ = std::fs::remove_dir_all(&d);
}

/// A commit with nothing new for a run that owes nothing syncs nothing:
/// under the buffered policy the run's file stays synced where it was.
#[test]
fn a_commit_of_nothing_in_a_run_that_owes_nothing_syncs_nothing() {
    for (tag, o) in run_arms() {
        let d = dir(&format!("dur-run-nothing-{tag}"));
        let opts = Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..o
        };
        let mut db = Db::create(&d, opts).unwrap();
        db.append(b"k1", b"v1");
        db.commit().unwrap();
        let before = db.run_synced_bytes().expect("a run open");
        db.commit().unwrap();
        assert_eq!(
            db.run_synced_bytes(),
            Some(before),
            "{tag}: a commit of nothing synced the run"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// The same through the store: under `EveryN(2)` the second commit is the
/// policy's sync of the run, and the helper's fdatasync of the first
/// batch's pieces failed before it. The commit answers the failure, and
/// every commit and sync after it answers it in the kept words.
#[test]
fn a_due_commit_is_not_acknowledged_over_a_failed_helper_sync() {
    let d = dir("dur-helper-commit");
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let opts = Options {
        sync: SyncPolicy::EveryN(2),
        segment: SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            fail_helper_syncs: Some(failed.clone()),
            ..Default::default()
        },
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    // Ascending into an empty store: a direct run.
    for k in 0..3000u32 {
        db.append(&hkey(k), &hval(k));
    }
    db.commit().unwrap(); // the policy's first of two: no sync
    wait_count(&failed, 1, "the run's helper never took its turn at a sync");
    let synced = db.run_synced_bytes();
    db.append(&hkey(3000), &hval(3000));
    assert!(
        db.commit().is_err(),
        "the policy's sync, after the run's records failed theirs, is not answered Ok"
    );
    assert_eq!(
        db.run_synced_bytes(),
        synced,
        "nothing counts as synced over the helper's failure"
    );
    kept(db.commit(), "a commit of nothing after");
    kept(db.sync(), "a sync after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A piece the device refuses at a commit the buffered policy does not
/// sync: the commit answers `Ok`, having promised nothing, and the failure
/// is held for the call that syncs. A `sync` answers it, and the run owes
/// the batch until a sync covers it: the `sync` after writes it and
/// answers `Ok`, and a power cut after that reads every key back. With
/// `twice`, the device refuses the write the first `sync` would have made
/// as well, and the run, holding every byte since the first failure for
/// the next sync, tries no write in between.
fn a_buffered_run_piece_write_that_failed(tag: &str, opts: Options, twice: bool) {
    let d = dir(&format!("dur-runpiece-buf-{tag}"));
    let crash = dir(&format!("dur-runpiece-buf-{tag}-crash"));
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..opts
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.sync().unwrap();
    if twice {
        db.fail_next_run_writes_twice();
    } else {
        db.fail_next_run_write();
    }
    let val = vec![b'x'; 100];
    for i in 1..30_000u32 {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    db.commit()
        .unwrap_or_else(|e| panic!("{tag}: a commit the policy does not sync: {e}"));
    for i in 0..30_000u32 {
        db.append(format!("b{i:07}").as_bytes(), &val);
    }
    db.commit()
        .unwrap_or_else(|e| panic!("{tag}: the commit after, not synced either: {e}"));
    injected(
        db.sync(),
        &format!("{tag}: the sync answers the held failure"),
    );
    if twice {
        injected(db.sync(), &format!("{tag}: the sync that met it again"));
    }
    db.sync()
        .unwrap_or_else(|e| panic!("{tag}: the sync that writes the batch: {e}"));
    assert_eq!(db.levels(), (0, 0), "{tag}: the run is open");
    assert_eq!(
        db.run_attends(),
        Some(false),
        "{tag}: the run's writer still attends after the sync paid it"
    );
    let ino = run_ino(&d);
    let synced = db.run_synced_bytes().expect("a run open");
    power_cut(&db, &d, &crash, ino, synced);
    std::mem::forget(db);
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
    let x = String::from_utf8(val).unwrap();
    for k in ["a0000001", "a0029999", "b0000000", "b0029999"] {
        reads(&db, k, &x, &format!("{tag}: synced after the failure"));
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_buffered_run_piece_write_that_failed_is_answered_and_written_by_a_sync() {
    for (tag, o) in run_arms() {
        a_buffered_run_piece_write_that_failed(tag, o, false);
    }
}

#[test]
fn a_run_write_the_device_refuses_again_is_tried_once_a_sync() {
    for (tag, o) in run_arms() {
        a_buffered_run_piece_write_that_failed(&format!("twice-{tag}"), o, true);
    }
}

/// H1 under the shipped policy: under `Always` every commit of a run is
/// its own sync, and the helper thread behind the writer fails its
/// fdatasync of the pieces a large batch lands. The commits that met the
/// race may answer either way; once the helper has stored its failure,
/// the next commit answers it, nothing counts as synced over it, and
/// every commit and sync after answers it in the kept words.
#[test]
fn a_commit_under_always_is_not_acknowledged_over_a_failed_helper_sync() {
    let d = dir("dur-helper-always");
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let opts = Options {
        segment: SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            fail_helper_syncs: Some(failed.clone()),
            ..Default::default()
        },
        ..Options::default()
    };
    assert!(matches!(opts.sync, SyncPolicy::Always), "the premise");
    let mut db = Db::create(&d, opts).unwrap();
    // Each batch is many `sync_ahead`s of pieces landed before its commit's
    // own sync: the helper's turn comes inside one of them.
    // The first commit that answers the failure may be one of these: the
    // count it leaves is held to the one before it wherever it falls.
    let mut k = 0u32;
    let mut answered = false;
    for _ in 0..20 {
        for _ in 0..3000 {
            db.append(&hkey(k), &hval(k));
            k += 1;
        }
        // No run before the first: nothing of it synced.
        let before = db.run_synced_bytes().unwrap_or(0);
        if db.commit().is_err() {
            assert_eq!(
                db.run_synced_bytes(),
                Some(before),
                "nothing counts as synced over the helper's failure, in the batches"
            );
            answered = true;
            break;
        }
        if failed.load(std::sync::atomic::Ordering::Acquire) > 0 {
            break;
        }
    }
    assert!(open_run(&d), "the premise, a run");
    wait_count(&failed, 1, "the run's helper never took its turn at a sync");
    if !answered {
        let synced = db.run_synced_bytes();
        db.append(&hkey(k), &hval(k));
        k += 1;
        assert!(
            db.commit().is_err(),
            "a commit's sync, after the run's records failed theirs, is not answered Ok"
        );
        assert_eq!(
            db.run_synced_bytes(),
            synced,
            "nothing counts as synced over the helper's failure"
        );
    }
    db.append(&hkey(k + 1), &hval(k + 1));
    kept(db.commit(), "a commit after");
    kept(db.commit(), "a commit of nothing after");
    kept(db.sync(), "a sync after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Where the write that failed falls (`a_run_closed_over_a_failed_write`).
#[derive(Clone, Copy, Debug)]
enum Fails {
    /// The run's first write, its file's head with the first batch.
    Head,
    /// A whole piece of a batch, written by the commit before its sync:
    /// the writer holds every byte from there for the next sync.
    Piece,
    /// The tail a sync writes (`Always` only): owed, the writer not held.
    Tail,
}

/// The run's file after the batch whose write failed is owed or held:
/// a key below the run's greatest closes the run, and the close writes
/// the batch into its segment with the rest. Once the segment has landed
/// (`settle`), a power cut -- the log cut to what its fdatasyncs covered
/// -- and a kill each read every key back, the batch whose write failed
/// among them, whether a commit had answered its failure (`Always`) or
/// none had (`EveryN`).
fn a_run_closed_over_a_failed_write(tag: &str, opts: Options, fails: Fails) {
    let d = dir(&format!("dur-runclose-{tag}"));
    let crash = dir(&format!("dur-runclose-{tag}-crash"));
    let always = matches!(opts.sync, SyncPolicy::Always);
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let val = vec![b'x'; 100];
    let x = String::from_utf8(val.clone()).unwrap();
    let mut first = 0u32;
    if !matches!(fails, Fails::Head) {
        db.append(b"a0000000", b"v");
        db.commit().unwrap();
        db.sync().unwrap();
        first = 1;
    }
    db.fail_next_run_write();
    // About 3.4 MB of records, past the 1 MiB write piece in every arm, so
    // the failed write is the commit's own and not the close's; the tail's
    // batch is one record, which only its sync writes.
    let last = if matches!(fails, Fails::Tail) {
        first
    } else {
        first + 29_999
    };
    for i in first..=last {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    let r = db.commit();
    if always {
        injected(r, &format!("{tag}: the failed write's commit"));
    } else {
        r.unwrap_or_else(|e| panic!("{tag}: a commit the policy does not sync: {e}"));
    }
    assert!(db.run_synced_bytes().is_some(), "{tag}: the premise, a run");
    if always && matches!(fails, Fails::Head) {
        // The run's next commit writes the head and the stream behind it
        // and syncs them, and the stream's blocks the file then holds are
        // given up to it (`Options::direct_recycle`): read back through the
        // file, at the offset the stream landed at.
        db.append(b"b0000000", b"v");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the commit after the failed head: {e}"));
        reads(
            &db,
            "a0000000",
            &x,
            &format!("{tag}: live, through the file"),
        );
        reads(
            &db,
            "a0029999",
            &x,
            &format!("{tag}: live, through the file"),
        );
    }
    // Below the run's greatest key: the run closes.
    db.put(b"0000", b"below");
    db.commit()
        .unwrap_or_else(|e| panic!("{tag}: the commit that closed the run: {e}"));
    db.sync()
        .unwrap_or_else(|e| panic!("{tag}: the sync after: {e}"));
    db.settle()
        .unwrap_or_else(|e| panic!("{tag}: the run's segment lands: {e}"));
    assert!(db.run_synced_bytes().is_none(), "{tag}: the run closed");
    let (log, log_synced, _) = db.wal_durable();
    copy_store(&d, &crash);
    if let Some(name) = log.file_name() {
        if let Ok(f) = std::fs::OpenOptions::new()
            .write(true)
            .open(crash.join(name))
        {
            f.set_len(log_synced).unwrap();
        }
    }
    std::mem::forget(db);
    // Under a cipher the store is held open in this process past the
    // forget, so the kill is the power cut's case alone: its memory files
    // go with it either way.
    let cipher = matches!(opts.encryption, Encryption::Cipher(_));
    let ends: &[(&std::path::PathBuf, &str)] = if cipher {
        &[(&crash, "the power cut")]
    } else {
        &[(&crash, "the power cut"), (&d, "the kill")]
    };
    for &(at, what) in ends {
        let db = Db::open(at, opts.clone()).unwrap_or_else(|e| panic!("{tag}: {what} opens: {e}"));
        let w = format!("{tag}, {what}");
        if first == 1 {
            reads(&db, "a0000000", "v", &w);
        }
        reads(&db, &format!("a{first:07}"), &x, &w);
        reads(&db, &format!("a{last:07}"), &x, &w);
        reads(&db, "0000", "below", &w);
        drop(db);
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_run_closed_over_a_failed_write_writes_the_batch_into_its_segment() {
    let mut arms = run_arms();
    arms.push(("cipher", under_cipher()));
    for (tag, o) in arms {
        for (p, sync) in [
            ("always", SyncPolicy::Always),
            ("buf", SyncPolicy::EveryN(u32::MAX)),
        ] {
            let fails: &[Fails] = if p == "always" {
                &[Fails::Head, Fails::Piece, Fails::Tail]
            } else {
                &[Fails::Head, Fails::Piece]
            };
            for &f in fails {
                let opts = Options { sync, ..o.clone() };
                a_run_closed_over_a_failed_write(&format!("{p}-{f:?}-{tag}"), opts, f);
            }
        }
    }
}

/// Under a cipher a run's batch whose piece write failed at a commit the
/// buffered policy does not sync, and whose log write failed in the same
/// commit, is owed twice: the run's file holds the failure for the next
/// sync, and the log has the frames put back. The commit of nothing that
/// answers `Ok` has written and synced both, so a crash after it -- which
/// takes every memory file the store had -- reads the batch back from
/// the log.
#[test]
fn a_cipher_runs_batch_owed_to_its_file_and_its_log_is_logged_again() {
    let d = dir("dur-runlog-cipher-both");
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..under_cipher()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.sync().unwrap();
    assert_eq!(db.direct_runs(), (1, 1), "the premise, a logged run");
    db.fail_next_run_write();
    db.fail_next_wal_write(0);
    let val = vec![b'x'; 100];
    for i in 1..30_000u32 {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    assert!(
        db.commit().is_err(),
        "the log's failed write is the commit's answer"
    );
    injected(db.commit(), "the run's held failure, answered by the sync");
    db.commit()
        .expect("a commit of nothing writes the run's batch and logs it");
    let crash = dir("dur-runlog-cipher-both-crash");
    let (log, synced, _) = db.wal_durable();
    copy_store(&d, &crash);
    std::mem::forget(db);
    std::fs::OpenOptions::new()
        .write(true)
        .open(crash.join(log.file_name().unwrap()))
        .unwrap()
        .set_len(synced)
        .unwrap();
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the crash opens: {e}"));
    let x = String::from_utf8(val).unwrap();
    reads(&db, "a0000000", "v", "before the failure");
    reads(&db, "a0000001", &x, "the batch both failed");
    reads(&db, "a0029999", &x, "the batch both failed");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// Under a cipher a run's batch whose log write failed, and whose run
/// write did not, is logged by the commit of nothing after it even where
/// the run's writer has something to attend to that owes nothing -- here
/// a test's armed failure of its next fdatasync: the writer's one byte of
/// attention sends the call to the cold path, which still closes the
/// frames the log put back, and a crash after its `Ok` reads the batch.
#[test]
fn a_cipher_runs_unlogged_batch_is_logged_by_a_commit_of_nothing_beside_an_armed_writer() {
    let d = dir("dur-runlog-cipher-armed");
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..under_cipher()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.sync().unwrap();
    assert_eq!(db.direct_runs(), (1, 1), "the premise, a logged run");
    db.fail_next_wal_write(0);
    db.append(b"a0000001", b"w");
    assert!(
        db.commit().is_err(),
        "the log's failed write is the commit's answer"
    );
    db.fail_next_run_sync();
    assert_eq!(
        db.run_attends(),
        Some(true),
        "the premise: the writer attends"
    );
    db.commit()
        .expect("a commit of nothing logs the batch the log put back");
    let crash = dir("dur-runlog-cipher-armed-crash");
    let (log, synced, _) = db.wal_durable();
    copy_store(&d, &crash);
    std::mem::forget(db);
    std::fs::OpenOptions::new()
        .write(true)
        .open(crash.join(log.file_name().unwrap()))
        .unwrap()
        .set_len(synced)
        .unwrap();
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the crash opens: {e}"));
    reads(&db, "a0000000", "v", "before the failure");
    reads(&db, "a0000001", "w", "the batch the log put back");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A run's batch written again by a call with nothing new pays the
/// policy's count: the commits after it count from it, as from any sync.
#[test]
fn a_run_written_again_restarts_the_policys_count() {
    let d = dir("dur-run-again-count");
    let opts = Options {
        sync: SyncPolicy::EveryN(2),
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.fail_next_run_write();
    let val = vec![b'x'; 100];
    for i in 1..30_000u32 {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    injected(db.commit(), "the policy's sync answers the held failure");
    db.commit().expect("a commit of nothing writes the batch");
    let synced = db.run_synced_bytes();
    db.append(b"b0000000", b"v");
    db.commit().unwrap();
    assert_eq!(
        db.run_synced_bytes(),
        synced,
        "the first commit after the batch was written again is not the policy's sync"
    );
    db.append(b"b0000001", b"v");
    db.commit().unwrap();
    assert_ne!(db.run_synced_bytes(), synced, "the second is");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Under the buffered policy the run's first write, its file's head with
/// the first batch's records, fails at a commit the policy does not sync:
/// the failure is held, and the stream's next range -- the same commit's
/// marker -- tries the head again before it is held behind it, so the
/// file holds the head and the records before any sync, where a writer
/// that tried nothing would buffer the run until its sync; the sync after
/// still answers the failure it held, and the sync after that writes the
/// rest.
#[test]
fn a_buffered_runs_failed_first_write_is_tried_again_before_its_sync() {
    for (tag, o) in run_arms().into_iter().filter(|(_, o)| o.direct_stream) {
        let d = dir(&format!("dur-runhead-buf-{tag}"));
        let crash = dir(&format!("dur-runhead-buf-{tag}-crash"));
        let opts = Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..o
        };
        let mut db = Db::create(&d, opts.clone()).unwrap();
        db.fail_next_run_write();
        db.append(b"k1", b"v1");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: a commit the policy does not sync: {e}"));
        let len = || std::fs::metadata(d.join(run_tmp_name(&d))).unwrap().len();
        assert!(len() > 0, "{tag}: the marker's stream tried the head again");
        db.append(b"k2", b"v2");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the commit after, not synced either: {e}"));
        injected(
            db.sync(),
            &format!("{tag}: the sync answers the held failure"),
        );
        db.sync()
            .unwrap_or_else(|e| panic!("{tag}: the sync that writes the rest: {e}"));
        let ino = run_ino(&d);
        let synced = db.run_synced_bytes().expect("a run open");
        power_cut(&db, &d, &crash, ino, synced);
        std::mem::forget(db);
        let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
        reads(&db, "k1", "v1", &format!("{tag}: the first batch"));
        reads(&db, "k2", "v2", &format!("{tag}: the commit after"));
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// A held failure answered is gone: the head refused twice, by the write
/// and by the marker's try, is held and answered by a `sync` that writes
/// nothing; the next commit's try of the head fails part way, and the
/// sync after answers that failure in its own words, not the first's;
/// the sync after it writes everything, and a power cut reads both
/// batches back.
#[test]
fn a_run_write_failure_after_one_answered_is_answered_in_its_own_words() {
    for (tag, o) in run_arms().into_iter().filter(|(_, o)| o.direct_stream) {
        let d = dir(&format!("dur-runhead-again-{tag}"));
        let crash = dir(&format!("dur-runhead-again-{tag}-crash"));
        let opts = Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..o
        };
        let mut db = Db::create(&d, opts.clone()).unwrap();
        db.fail_next_run_writes_twice();
        db.append(b"k1", b"v1");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: a commit the policy does not sync: {e}"));
        let len = || std::fs::metadata(d.join(run_tmp_name(&d))).unwrap().len();
        assert_eq!(len(), 0, "{tag}: the premise, the head refused twice");
        let first = db.sync().expect_err("the sync answers the held failure");
        assert!(
            first.to_string().contains("injected") && !first.to_string().contains("partial"),
            "{tag}: the first failure's words: {first}"
        );
        db.fail_next_run_write_partial();
        db.append(b"k2", b"v2");
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the commit whose try fails part way: {e}"));
        let second = db.sync().expect_err("the sync answers the second failure");
        assert!(
            second.to_string().contains("after a partial write"),
            "{tag}: the second failure in its own words: {second}"
        );
        db.sync()
            .unwrap_or_else(|e| panic!("{tag}: the sync that writes the rest: {e}"));
        let ino = run_ino(&d);
        let synced = db.run_synced_bytes().expect("a run open");
        power_cut(&db, &d, &crash, ino, synced);
        std::mem::forget(db);
        let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
        reads(&db, "k1", "v1", &format!("{tag}: the first batch"));
        reads(&db, "k2", "v2", &format!("{tag}: the commit after"));
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// A marked writer's sync that meets a write failure of its own -- the
/// records it writes ahead of their marker cross a piece and the piece's
/// write fails -- and then writes and syncs every byte it holds answers
/// `Ok`, which is true; the failure is paid by it, so the sync after a
/// new batch writes that batch and answers `Ok` too, and the file holds
/// both batches under the one marker.
#[test]
fn a_marked_segments_write_that_failed_inside_a_sync_is_paid_by_that_sync() {
    let d = dir("dur-writer-insync");
    let opts = SegmentOptions {
        write_piece: 4096,
        ..Default::default()
    };
    let path = d.join("run.sup");
    let mut w = SegmentWriter::create(&path, &opts).unwrap();
    w.set_inline_max(256);
    w.set_marks(true);
    for k in 0..100u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    w.fail_next_write();
    w.sync()
        .expect("a sync that wrote and synced every byte after the failure");
    for k in 100..200u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    w.mark().unwrap();
    w.sync()
        .expect("the sync after: the failure was paid, and this batch is written");
    let got = SegmentWriter::recover_direct(&path).unwrap();
    assert_eq!(got.len(), 200, "both batches, under the marker");
    assert_eq!(got[199].0, hkey(199));
    drop(w);
    let _ = std::fs::remove_dir_all(&d);
}

/// The same at the edge: the records a sync writes ahead of their marker
/// end exactly at a piece's boundary, so the failed write was of every
/// byte the writer held and the sync's own write of them is the last;
/// what the failure owed is exactly what the sync covered, and it is paid.
#[test]
fn a_write_that_failed_inside_a_sync_and_owed_exactly_what_it_synced_is_paid() {
    let d = dir("dur-writer-insync-edge");
    let opts = SegmentOptions {
        write_piece: 4096,
        ..Default::default()
    };
    let path = d.join("run.sup");
    let mut found = false;
    for l in 1..=600usize {
        let _ = std::fs::remove_file(&path);
        let mut w = SegmentWriter::create(&path, &opts).unwrap();
        w.set_inline_max(1024);
        w.set_marks(true);
        for k in 0..28u32 {
            w.begin(&hkey(k)).unwrap();
            w.value(&hval(k));
            w.end_with(false).unwrap();
        }
        w.begin(&hkey(28)).unwrap();
        w.value(&vec![b'y'; l]);
        w.end_with(false).unwrap();
        w.fail_next_write();
        let first = w.sync();
        let len = std::fs::metadata(&path).unwrap().len();
        if first.is_err() || !len.is_multiple_of(4096) {
            continue;
        }
        found = true;
        w.begin(&hkey(29)).unwrap();
        w.value(&hval(29));
        w.end_with(false).unwrap();
        w.mark().unwrap();
        w.sync()
            .expect("the sync after: the failure was paid to the byte");
        assert_eq!(SegmentWriter::recover_direct(&path).unwrap().len(), 30);
        break;
    }
    assert!(found, "the premise: records that end at a piece's boundary");
    let _ = std::fs::remove_dir_all(&d);
}

/// The same where the log rotated by size behind the logged batch: the
/// frames are in the file the rotation left loose and the live file holds
/// none, and the commit of nothing that answers `Ok` has synced the loose
/// file as a sync by policy would.
#[test]
fn a_cipher_runs_batch_written_again_is_synced_in_a_log_file_a_rotation_left_loose() {
    let d = dir("dur-runlog-cipher-loose");
    // The log's frames pass `seal_bytes` at the batch while the table's
    // bytes stay under it: the log rotates and the run stays open.
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        seal_rotates_wal: false,
        seal_bytes: 3_300_000,
        seal_max_pct: 0,
        ..under_cipher()
    };
    let mut db = Db::create(&d, opts).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.sync().unwrap();
    assert_eq!(db.direct_runs(), (1, 1), "the premise, a logged run");
    db.fail_next_run_write();
    let val = vec![b'x'; 100];
    for i in 1..30_000u32 {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    db.commit()
        .expect("a commit the policy does not sync holds the piece's failure");
    assert_eq!(
        db.seal_waits().wal_rotations,
        1,
        "the premise: the log rotated"
    );
    assert_eq!(db.levels(), (0, 0), "the premise: the run is open");
    assert!(db.wal_unsynced(), "the premise: the loose file is unsynced");
    injected(db.commit(), "the run's held failure, answered by the sync");
    db.commit()
        .expect("a commit of nothing writes the run's batch and syncs its log");
    assert!(
        !db.wal_unsynced(),
        "the commit of nothing answered Ok over a log file left loose"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Under a cipher a run's batch whose piece write failed at a commit the
/// buffered policy does not sync was logged by that commit, unsynced, as
/// the policy leaves a commit's frames. The commit of nothing that writes
/// the run's batch again and answers `Ok` has synced the log as well, the
/// batch's one durable form under a cipher, so a crash after it -- which
/// takes every memory file the store had -- reads the batch back.
#[test]
fn a_cipher_runs_batch_written_again_by_a_commit_of_nothing_is_synced_in_its_log() {
    let d = dir("dur-runlog-cipher-again");
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..under_cipher()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.append(b"a0000000", b"v");
    db.commit().unwrap();
    db.sync().unwrap();
    assert_eq!(db.direct_runs(), (1, 1), "the premise, a logged run");
    db.fail_next_run_write();
    let val = vec![b'x'; 100];
    for i in 1..30_000u32 {
        db.append(format!("a{i:07}").as_bytes(), &val);
    }
    db.commit()
        .expect("a commit the policy does not sync holds the piece's failure");
    injected(db.commit(), "the run's held failure, answered by the sync");
    db.commit()
        .expect("a commit of nothing writes the run's batch and syncs its log");
    let crash = dir("dur-runlog-cipher-again-crash");
    let (log, synced, _) = db.wal_durable();
    copy_store(&d, &crash);
    std::mem::forget(db);
    std::fs::OpenOptions::new()
        .write(true)
        .open(crash.join(log.file_name().unwrap()))
        .unwrap()
        .set_len(synced)
        .unwrap();
    let db = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the crash opens: {e}"));
    let x = String::from_utf8(val).unwrap();
    reads(&db, "a0000001", &x, "the batch written again");
    reads(&db, "a0029999", &x, "the batch written again");
    reads(
        &db,
        "a0000000",
        "v",
        "the batch before it, in the log it synced",
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

// ---------------------------------------------------------------------
// log-at-open: what an open makes of the log a crash left. A process kill
// keeps the page cache, so the frames an open replays are visible whether
// or not any fdatasync covered them, and the first sync or flush after the
// open has to make them durable; and a log file no sync ever covered can
// come back from a power cut with its first sector zero, which is a log
// with nothing in it and never a refusal.
// ---------------------------------------------------------------------

/// A plain log's header length; the kind byte is its last.
const WAL_HEAD: u64 = 8;
const WAL_KIND_AT: u64 = 7;
const WAL_LOOSE_KIND: u8 = 5;

fn bkey(batch: char, i: u32) -> Vec<u8> {
    format!("{batch}{i:04}").into_bytes()
}

fn bval(batch: char, i: u32) -> Vec<u8> {
    format!("v-{batch}-{i}").into_bytes()
}

fn put_batch(db: &mut Db, batch: char, n: u32) {
    for i in 0..n {
        db.put(&bkey(batch, i), &bval(batch, i));
    }
    db.commit().unwrap();
}

/// The log files of a store, by name, in id order.
fn wal_names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wal-"))
        .collect();
    v.sort();
    v
}

/// Every file of a store by name with its bytes: what a refused open must
/// leave as it found.
fn files_of(d: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

/// The power cut, applied to a copy of the store's files in `crash`: each
/// log file cut to the longest length an fdatasync of it covered in any
/// of the `lives` traced (`Db::wal_syncs_traced`), and never below its
/// header of `head` bytes, which a power cut may keep whether or not it
/// was synced; and a header whose kind was rewritten chained after its
/// file's last fdatasync put back loose, as the device may still hold it.
/// Nothing a sync covered is taken. The files named in `whole` keep what
/// no sync covered as well, as the device may have written it back: the
/// state where a later file's frames survive a cut of an earlier one's.
fn power_cut_logs(crash: &Path, lives: &[&[(PathBuf, Option<u64>)]], head: u64, whole: &[&str]) {
    for name in wal_names(crash) {
        let mut durable = 0u64;
        let mut chained_unsynced = false;
        for (p, ev) in lives.iter().flat_map(|l| l.iter()) {
            if p.file_name().map(|n| n.to_string_lossy() == name.as_str()) != Some(true) {
                continue;
            }
            match ev {
                Some(len) => {
                    durable = durable.max(*len);
                    chained_unsynced = false;
                }
                None => chained_unsynced = true,
            }
        }
        let path = crash.join(&name);
        let len = std::fs::metadata(&path).unwrap().len();
        let keep = if whole.contains(&name.as_str()) {
            len
        } else {
            durable.max(head.min(len))
        };
        assert!(
            durable <= len,
            "{name}: a sync covered more than the file holds"
        );
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        if keep < len {
            f.set_len(keep).unwrap();
        }
        if chained_unsynced && len > WAL_KIND_AT {
            use std::os::unix::fs::FileExt;
            f.write_at(&[WAL_LOOSE_KIND], WAL_KIND_AT).unwrap();
        }
    }
}

fn every_key_of(db: &Reader, batches: &[(char, u32)], what: &str) {
    for &(batch, n) in batches {
        for i in 0..n {
            assert_eq!(
                read_vec(db, &bkey(batch, i)),
                vec![bval(batch, i)],
                "{what}: key {batch}{i:04}, acknowledged before the power cut, is gone after it"
            );
        }
    }
}

/// The batches of `batches` an open shows, which under a buffered policy
/// must be a prefix of them in commit order, each whole or absent.
fn a_prefix_of(db: &Reader, batches: &[(char, u32)], what: &str) -> usize {
    let mut shown = 0;
    for (at, &(batch, n)) in batches.iter().enumerate() {
        let here = (0..n)
            .filter(|&i| !read_vec(db, &bkey(batch, i)).is_empty())
            .count() as u32;
        assert!(
            here == 0 || here == n,
            "{what}: batch {batch} is in part, {here} keys of {n}"
        );
        if here == n {
            assert_eq!(shown, at, "{what}: batch {batch} without a batch before it");
            shown += 1;
        }
    }
    shown
}

fn buffered() -> Options {
    Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..Options::default()
    }
}

/// Life 1: batch `a` committed and synced, batch `b` committed and left
/// unsynced, then a process kill: the page cache keeps batch `b`. `put`
/// throughout, so every write goes through the log. Under `Always` the
/// same file is what a kill between batch `b`'s write and its fdatasync
/// leaves: `b` not acknowledged before the kill, visible after the reopen.
/// The kill is a copy of the files at that instant, opened in place of
/// the store (`killed`). Returns what life 1's syncs covered.
fn life_one(d: &Path, opts: Options) -> Vec<(PathBuf, Option<u64>)> {
    let mut db = Db::create(&made(d), opts).unwrap();
    db.trace_wal_syncs(true);
    put_batch(&mut db, 'a', 100);
    db.sync().unwrap();
    put_batch(&mut db, 'b', 100);
    let (_, synced, written) = db.wal_durable();
    assert!(
        written > synced,
        "life 1 leaves batch b unsynced: synced {synced}, written {written}"
    );
    assert_eq!(
        db.levels(),
        (0, 0),
        "life 1: the log alone holds both batches"
    );
    let traced = db.wal_syncs_traced();
    killed(db, d);
    traced
}

/// Where a store whose kill `killed` copies into `d` is made.
fn made(d: &Path) -> PathBuf {
    let m = d.with_extension("made");
    let _ = std::fs::remove_dir_all(&m);
    m
}

/// A process kill of `db`, made where `made(d)` names: its files copied
/// into `d` as they stand, which keeps whatever the page cache holds, and
/// the store closed, its own directory gone. The copy is opened in the
/// store's place; a store under a cipher is one open in the process, so
/// the original is dropped before that open, never forgotten.
fn killed(db: Db, d: &Path) {
    let m = d.with_extension("made");
    copy_store(&m, d);
    drop(db);
    let _ = std::fs::remove_dir_all(&m);
}

/// The reopen that replays batch `b` out of the page cache, with its syncs
/// traced from the open on.
fn life_two(d: &Path, opts: Options) -> Db {
    let mut db = Db::open(d, opts).unwrap();
    db.trace_wal_syncs(true);
    assert_eq!(
        read_vec(&db, &bkey('b', 7)),
        vec![bval('b', 7)],
        "precondition: the reopen made batch b visible"
    );
    db
}

fn a_sync_after_a_reopen(tag: &str, life1: Options, opts: Options, head: u64) {
    let d = dir(&format!("dur-reopen-sync-{tag}"));
    let crash = dir(&format!("dur-reopen-sync-{tag}-cut"));
    let one = life_one(&d, life1);
    let mut db = life_two(&d, opts.clone());
    // The acknowledgement: everything visible before it is durable.
    db.sync().unwrap();
    copy_store(&d, &crash);
    power_cut_logs(&crash, &[&one, &db.wal_syncs_traced()], head, &[]);
    drop(db);
    let db = Db::open(&crash, opts).unwrap();
    every_key_of(
        &db,
        &[('a', 100), ('b', 100)],
        &format!("{tag}: sync after a reopen"),
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_sync_after_a_reopen_makes_the_replayed_writes_durable_buffered() {
    a_sync_after_a_reopen("everyn", buffered(), buffered(), WAL_HEAD);
}

#[test]
fn a_sync_after_a_reopen_makes_the_replayed_writes_durable_always() {
    a_sync_after_a_reopen("always", buffered(), Options::default(), WAL_HEAD);
}

/// The sealed log's header, the kind and the file's wrapped key, is longer
/// than the plain one's; the power cut keeps all of it or a prefix only
/// where no sync covered it, and here life 1's sync did.
#[test]
fn a_sync_after_a_reopen_makes_the_replayed_writes_durable_under_a_cipher() {
    let o = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..under_cipher()
    };
    a_sync_after_a_reopen("cipher", o.clone(), o, WAL_HEAD);
}

/// Run with the store's merges on and off: on, the drain joins a merge
/// whose synced publish names its segment before the answer, so a slot
/// left owed is superseded there; off, the drain's own slot is all that
/// names it, and only the order of the answer and the slot's fdatasync
/// keeps the replayed writes.
fn a_flush_after_a_reopen(tag: &str, life1: Options, opts: Options) {
    a_flush_after_a_reopen_once(tag, life1.clone(), opts.clone());
    let off = Options {
        compact: false,
        ..opts
    };
    a_flush_after_a_reopen_once(&format!("{tag}-nomerge"), life1, off);
}

fn a_flush_after_a_reopen_once(tag: &str, life1: Options, opts: Options) {
    let d = dir(&format!("dur-reopen-flush-{tag}"));
    let crash = dir(&format!("dur-reopen-flush-{tag}-cut"));
    let one = life_one(&d, life1);
    let mut db = life_two(&d, opts.clone());
    // The slots as they stand, durable as far as this test is concerned.
    let slots = ["manifest", "manifest.alt"];
    let before: Vec<Option<Vec<u8>>> = slots
        .iter()
        .map(|s| std::fs::read(d.join(s)).ok())
        .collect();
    // A slot the drain leaves owed is not synced until the hold lifts, so
    // the instant after the answer can be copied.
    db.hold_owed_slot(true);
    db.flush().unwrap();
    let owed = db.owed_slots_held();
    copy_store(&d, &crash);
    power_cut_logs(&crash, &[&one, &db.wal_syncs_traced()], WAL_HEAD, &[]);
    if owed > 0 {
        // The slot the drain wrote, its fdatasync not yet run: a power cut
        // may keep the slots as they were before the write.
        for (s, b) in slots.iter().zip(&before) {
            match b {
                Some(bytes) => std::fs::write(crash.join(s), bytes).unwrap(),
                None => {
                    let _ = std::fs::remove_file(crash.join(s));
                }
            }
        }
    }
    db.hold_owed_slot(false);
    drop(db);
    let db = Db::open(&crash, opts).unwrap();
    every_key_of(
        &db,
        &[('a', 100), ('b', 100)],
        &format!("{tag}: flush after a reopen"),
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_flush_after_a_reopen_is_answered_only_once_its_writes_are_durable_buffered() {
    a_flush_after_a_reopen("everyn", buffered(), buffered());
}

#[test]
fn a_flush_after_a_reopen_is_answered_only_once_its_writes_are_durable_always() {
    a_flush_after_a_reopen("always", buffered(), Options::default());
}

/// The same under a cipher: the drain's segment gets its envelope and its
/// slot names it, and the log's sealed records are what the flush covers.
#[test]
fn a_flush_after_a_reopen_is_answered_only_once_its_writes_are_durable_under_a_cipher() {
    let o = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..under_cipher()
    };
    a_flush_after_a_reopen("cipher", o.clone(), o);
}

/// The instrument the flush tests above read their power cut by, held to
/// firing: a drain over a log that owes nothing is answered before its
/// slot's fdatasync, so with the hold on the answer comes with a slot
/// counted owed, and the closed log file the slot covers is still in the
/// directory however long the hold stands -- a power cut there that
/// takes the slot replays it. Without this, a hold that counted nothing
/// would let those tests keep the drain's slot through the cut, and a
/// drain answered before the slot of a log that owes frames would pass
/// them; and a hold that held nothing would retire the file before the
/// copy. And the count is of slots still owed: one a merge's synced
/// publish superseded before the answer is not, or those tests would put
/// back a slot the device holds and fail a drain for nothing.
#[test]
fn a_drain_answered_before_its_slots_sync_keeps_the_log_file_the_slot_covers() {
    let d = dir("dur-owed-slot");
    let crash = dir("dur-owed-slot-cut");
    // The seal syncs and closes the log file it covers, so the drain's
    // slot is what retires a file, and the log owes nothing at the drain;
    // no merge, whose synced publish would supersede the slot before the
    // answer.
    let opts = Options {
        seal_rotates_wal: true,
        compact: false,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    put_batch(&mut db, 'a', 100);
    assert!(!db.wal_unsynced(), "precondition: the log owes nothing");
    let closed = wal_names(&d)[0].clone();
    let slots = ["manifest", "manifest.alt"];
    let before: Vec<Option<Vec<u8>>> = slots
        .iter()
        .map(|s| std::fs::read(d.join(s)).ok())
        .collect();
    db.hold_owed_slot(true);
    db.flush().unwrap();
    assert!(
        db.owed_slots_held() > 0,
        "the drain over a log that owes nothing left its slot owed"
    );
    // Held, the slot's fdatasync and the retirement behind it wait; a hold
    // that does not hold retires the file within this.
    for _ in 0..30 {
        assert!(
            d.join(&closed).exists(),
            "the closed log file {closed} retired before the slot that covers it was synced"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    copy_store(&d, &crash);
    for (s, b) in slots.iter().zip(&before) {
        match b {
            Some(bytes) => std::fs::write(crash.join(s), bytes).unwrap(),
            None => {
                let _ = std::fs::remove_file(crash.join(s));
            }
        }
    }
    db.hold_owed_slot(false);
    db.settle().unwrap();
    assert!(
        !d.join(&closed).exists(),
        "the slot, synced, retires the closed log file {closed}"
    );
    assert!(
        wal_names(&crash).contains(&closed),
        "the copy holds the closed log file"
    );
    drop(db);
    let db = Db::open(&crash, opts.clone())
        .unwrap_or_else(|e| panic!("the slot a power cut took before its sync: {e}"));
    every_key_of(&db, &[('a', 100)], "a drain whose slot a power cut took");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
    // With the merges on, the drain joins a merge whose synced publish
    // supersedes the slot before the answer: nothing is owed then, and
    // the closed file is retired behind the durable slot.
    let opts = Options {
        compact: true,
        ..opts
    };
    let mut db = Db::create(&d, opts).unwrap();
    put_batch(&mut db, 'a', 100);
    let closed = wal_names(&d)[0].clone();
    db.hold_owed_slot(true);
    db.flush().unwrap();
    assert_eq!(
        db.owed_slots_held(),
        0,
        "a slot a merge's publish superseded before the answer counted owed"
    );
    assert!(
        !d.join(&closed).exists(),
        "the superseding slot, synced, retired the closed log file {closed}"
    );
    db.hold_owed_slot(false);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A buffered store whose log rotated by size with frames no sync had
/// covered, killed: two log files, the closed one loose, the live one's
/// header saying so, both read back by the open out of the page cache.
fn rotated_and_killed(d: &Path, opts: Options) -> (Vec<(PathBuf, Option<u64>)>, u32) {
    let mut db = Db::create(&made(d), opts).unwrap();
    db.trace_wal_syncs(true);
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "the log never rotated");
    }
    put_commit(&mut db, "s0000000", &val).unwrap();
    assert!(db.seal_waits().rotated_unsynced > 0, "a loose rotation");
    assert_eq!(
        db.levels(),
        (0, 0),
        "nothing sealed: both files hold frames"
    );
    let traced = db.wal_syncs_traced();
    assert!(traced.is_empty(), "nothing synced in life 1: {traced:?}");
    killed(db, d);
    assert_eq!(wal_names(d).len(), 2, "two log files");
    (traced, i)
}

fn rotating() -> Options {
    Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..Options::default()
    }
}

/// The reopen of a store whose closed log file was left loose: a sync
/// after it makes the closed file's replayed frames durable as well as the
/// live file's. Synced alone, the live file's header is rewritten chained
/// over a closed file a power cut can still cut, and the open after that
/// cut reads a gap at a chained boundary and refuses the store.
#[test]
fn a_sync_after_a_reopen_makes_a_replayed_loose_log_file_durable() {
    let d = dir("dur-reopen-loose");
    let crash = dir("dur-reopen-loose-cut");
    let opts = rotating();
    let (one, n) = rotated_and_killed(&d, opts.clone());
    let mut db = Db::open(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    db.sync().unwrap();
    copy_store(&d, &crash);
    power_cut_logs(&crash, &[&one, &db.wal_syncs_traced()], WAL_HEAD, &[]);
    drop(db);
    let db = Db::open(&crash, opts)
        .unwrap_or_else(|e| panic!("a store synced after its reopen is refused: {e}"));
    let val = "x".repeat(100);
    for i in 0..n {
        reads(&db, &format!("r{i:07}"), &val, "the closed file's frames");
    }
    reads(&db, "s0000000", &val, "the live file's frame");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// The reopen of a store whose live log holds replayed frames no sync
/// covered, past the size a rotation closes a file at, and a commit of
/// nothing, which rotates it before anything else is written or synced:
/// the file it closes is loose, and the new file's header says so, so a
/// power cut that takes the replayed frames and keeps a commit after the
/// rotation leaves a log that ends at the boundary. Closed as chained, the
/// commit after reads as a gap after a synced file, a record lost, and the
/// open refuses.
#[test]
fn a_rotation_after_a_reopen_leaves_the_replayed_file_loose() {
    let d = dir("dur-reopen-rotate");
    let crash = dir("dur-reopen-rotate-cut");
    let one = life_one(&d, buffered());
    let opts = Options {
        seal_bytes: 4096,
        ..rotating()
    };
    let mut db = life_two(&d, opts.clone());
    assert!(
        db.wal_durable().2 >= 4096,
        "the replayed file is past the size"
    );
    // The table is past `seal_bytes` too, so the commits may hand it to a
    // seal: held before its landing, so the copy is of a store whose seal
    // is in flight, and never of a listing taken before a segment's rename
    // beside a manifest copied after the slot that names it, which is no
    // state a crash leaves. Let go before anything here can fail, so a
    // red run is not a hung one.
    db.hold_seal_landing(true);
    let rotate = db.commit();
    let rotations = db.seal_waits().wal_rotations;
    let after = put_commit(&mut db, "s0000000", "S");
    let levels = db.levels();
    copy_store(&d, &crash);
    db.hold_seal_landing(false);
    rotate.unwrap();
    assert_eq!(rotations, 1, "the commit of nothing rotated");
    after.unwrap();
    assert_eq!(levels, (0, 0), "nothing landed");
    // The new file's frame written back, the replayed file's tail lost.
    let newest = wal_names(&crash).pop().unwrap();
    assert_eq!(newest, "wal-00000001", "the rotation's file");
    power_cut_logs(
        &crash,
        &[&one, &db.wal_syncs_traced()],
        WAL_HEAD,
        &[newest.as_str()],
    );
    drop(db);
    let db = Db::open(&crash, opts).unwrap_or_else(|e| {
        panic!("a power cut a buffered store may meet after a rotation is refused: {e}")
    });
    every_key_of(&db, &[('a', 100)], "synced in life 1");
    a_prefix_of(&db, &[('b', 100)], "replayed in life 2");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A reopen whose live log file's header a power cut had lost, beside a
/// closed file it replayed frames out of the page cache from: the header
/// the open writes says the file before it is loose, since no sync has
/// covered that file's frames, so a commit after it that the device kept
/// while it lost the closed file's tail reads as the log's end. Written
/// chained, it reads as a record lost and the open refuses.
#[test]
fn a_header_an_open_writes_beside_an_unsynced_closed_file_is_loose() {
    let d = dir("dur-reopen-newhead");
    let crash = dir("dur-reopen-newhead-cut");
    let opts = rotating();
    let (one, _) = rotated_and_killed(&d, opts.clone());
    // The live file's header lost, its page with it.
    let live = d.join(wal_names(&d).last().unwrap());
    let mut bytes = std::fs::read(&live).unwrap();
    let n = bytes.len().min(4096);
    bytes[..n].fill(0);
    std::fs::write(&live, &bytes).unwrap();
    let mut db = Db::open(&d, opts.clone())
        .unwrap_or_else(|e| panic!("a lost header of a never-synced log is refused: {e}"));
    db.trace_wal_syncs(true);
    put_commit(&mut db, "t0000000", "T").unwrap();
    copy_store(&d, &crash);
    // The closed file's frames lost, no sync in either life having covered
    // them, and the live file's written back.
    let newest = wal_names(&crash).pop().unwrap();
    power_cut_logs(
        &crash,
        &[&one, &db.wal_syncs_traced()],
        WAL_HEAD,
        &[newest.as_str()],
    );
    let at_once = Db::open(&crash, opts.clone())
        .unwrap_or_else(|e| panic!("a power cut a buffered store may meet is refused: {e}"));
    drop(at_once);
    // Then a sync, which syncs the closed file and rewrites the header
    // chained, both durable with it: damage after the power cut that takes
    // the closed file's frames is a gap the open refuses, never the log's
    // end with the acknowledged commit dropped.
    db.sync().unwrap();
    copy_store(&d, &crash);
    power_cut_logs(&crash, &[&one, &db.wal_syncs_traced()], WAL_HEAD, &[]);
    drop(db);
    let names = wal_names(&crash);
    std::fs::OpenOptions::new()
        .write(true)
        .open(crash.join(&names[0]))
        .unwrap()
        .set_len(WAL_HEAD)
        .unwrap();
    let found = files_of(&crash);
    match Db::open(&crash, opts) {
        Ok(db) => reads(&db, "t0000000", "T", "acknowledged by the sync"),
        Err(e) => {
            assert!(
                e.to_string().contains("sequence gap"),
                "refused for the gap: {e}"
            );
            assert_eq!(files_of(&crash), found, "a refused open leaves the files");
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A store that crashed with nothing to replay -- closed; killed after a
/// flush whose manifest covers every frame; killed with a closed file the
/// manifest covers still beside the live one, as a kill between the
/// manifest's sync and the file's removal leaves it -- or whose replayed
/// frames the open synced itself, cutting a torn tail off them, reopens to
/// a log nothing is owed: a sync and a flush after it issue no fdatasync
/// of the log beyond the open's own, which the trace begins with where the
/// open cut a tail and nowhere else. (A commit of nothing under `Always`
/// syncs the log whatever was replayed.)
#[test]
fn a_reopen_with_nothing_replayed_owes_its_log_no_sync() {
    for (tag, how) in [
        ("closed", 0u8),
        ("killed after a flush", 1),
        ("killed with a torn tail", 2),
        ("killed beside a covered closed file", 3),
        (
            "killed beside a covered closed file, the live header lost",
            4,
        ),
    ] {
        let d = dir(&format!("dur-reopen-clean-{how}"));
        for opts in [buffered(), Options::default()] {
            let _ = std::fs::remove_dir_all(&d);
            let life1 = if how >= 3 { rotating() } else { buffered() };
            let mut db = Db::create(&made(&d), life1).unwrap();
            put_batch(&mut db, 'a', 100);
            if how == 0 {
                db.close().unwrap();
                copy_store(&d.with_extension("made"), &d);
            } else if how == 1 {
                db.flush().unwrap();
                db.settle().unwrap();
                killed(db, &d);
            } else if how == 2 {
                put_batch(&mut db, 'b', 100);
                killed(db, &d);
                let wal = d.join("wal-00000000");
                let mut f = std::fs::OpenOptions::new().append(true).open(&wal).unwrap();
                std::io::Write::write_all(&mut f, &[9, 0, 0, 0, 1]).unwrap();
            } else {
                let val = "x".repeat(100);
                let mut i = 0u32;
                while db.seal_waits().wal_rotations == 0 {
                    put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
                    i += 1;
                    assert!(i < 10_000, "the log never rotated");
                }
                let before = d.with_extension("before");
                copy_store(&d.with_extension("made"), &before);
                db.flush().unwrap();
                db.settle().unwrap();
                killed(db, &d);
                assert!(
                    !d.join("wal-00000000").exists(),
                    "the flush retired the closed file"
                );
                std::fs::copy(before.join("wal-00000000"), d.join("wal-00000000")).unwrap();
                let _ = std::fs::remove_dir_all(&before);
                if how == 4 {
                    // Whatever the live file holds the flush covered;
                    // its every byte zero is a first sector a power cut
                    // took, with nothing behind it a replay could keep.
                    let names = wal_names(&d);
                    let last = d.join(names.last().unwrap());
                    let len = std::fs::metadata(&last).unwrap().len() as usize;
                    assert!(
                        names.len() == 2 && len >= 8,
                        "{tag}: two log files, the live one its header at least: {names:?}, {len}"
                    );
                    std::fs::write(&last, vec![0u8; len]).unwrap();
                }
            }
            let mut db = Db::open(&d, opts.clone()).unwrap();
            db.trace_wal_syncs(true);
            let at_open = db.wal_syncs_traced();
            let (live, _, written) = db.wal_durable();
            if how == 2 {
                assert_eq!(
                    at_open,
                    vec![(live.clone(), Some(written))],
                    "{tag}: the open's cut synced the live file to its frames"
                );
            } else if how == 4 {
                assert_eq!(
                    at_open,
                    vec![(live.clone(), Some(0))],
                    "{tag}: the open's cut of the lost header synced the live file empty"
                );
            } else {
                assert_eq!(at_open, vec![], "{tag}: the open synced nothing");
            }
            every_key_of(&db, &[('a', 100)], tag);
            assert!(!db.wal_unsynced(), "{tag}: the reopened log owes nothing");
            db.sync().unwrap();
            db.flush().unwrap();
            assert_eq!(
                db.wal_syncs_traced(),
                at_open,
                "{tag}: a reopen with nothing replayed synced its log"
            );
            if how == 4 {
                // No file before the header the open wrote is owed a
                // sync, so it says chained, and the first sync with frames
                // to cover rewrites nothing.
                assert_ne!(
                    std::fs::read(&live).unwrap()[WAL_KIND_AT as usize],
                    WAL_LOOSE_KIND,
                    "{tag}: the header the open wrote beside covered files says loose"
                );
                put_batch(&mut db, 'c', 10);
                db.sync().unwrap();
                let after = db.wal_syncs_traced();
                assert!(
                    after[at_open.len()..].iter().all(|(_, e)| e.is_some()),
                    "{tag}: the first sync after the open rewrote the live header: {after:?}"
                );
            }
            drop(db);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// The sync that syncs a loose file and rewrites the live file's header
/// chained makes that header durable with it: after a power cut the
/// boundary still says the file before was synced, so damage that takes
/// that file's frames is a gap the open refuses, where a header left loose
/// on the device would read it as the log's end and drop the live file's
/// acknowledged frames with nothing said.
#[test]
fn a_sync_that_chains_a_loose_boundary_makes_the_chained_header_durable() {
    let d = dir("dur-chain-durable");
    let crash = dir("dur-chain-durable-cut");
    let opts = rotating();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "the log never rotated");
    }
    put_commit(&mut db, "s0000000", &val).unwrap();
    db.sync().unwrap();
    copy_store(&d, &crash);
    power_cut_logs(&crash, &[&db.wal_syncs_traced()], WAL_HEAD, &[]);
    drop(db);
    // Damage after the power cut: the closed file's frames gone.
    let names = wal_names(&crash);
    assert_eq!(names.len(), 2, "the closed file and the live one");
    std::fs::OpenOptions::new()
        .write(true)
        .open(crash.join(&names[0]))
        .unwrap()
        .set_len(WAL_HEAD)
        .unwrap();
    let found = files_of(&crash);
    match Db::open(&crash, opts) {
        Ok(db) => {
            // Opened: it must hold every acknowledged frame, which it
            // cannot, the damage having taken some.
            for i in 0..i {
                reads(&db, &format!("r{i:07}"), &val, "acknowledged by the sync");
            }
            reads(&db, "s0000000", &val, "acknowledged by the sync");
        }
        Err(e) => {
            assert!(
                e.to_string().contains("sequence gap"),
                "refused for the gap: {e}"
            );
            assert_eq!(files_of(&crash), found, "a refused open leaves the files");
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// `n` bytes from the start of `p` made zero, the length kept: what a
/// power cut leaves of the pages of a file no fdatasync covered, the
/// inode's size and any later page surviving.
fn zero_prefix(p: &Path, n: usize) {
    let mut bytes = std::fs::read(p).unwrap();
    let n = n.min(bytes.len());
    bytes[..n].fill(0);
    std::fs::write(p, &bytes).unwrap();
}

/// The live log file's real path (under a cipher, where the directory the
/// store names is not the real one) and its name.
fn live_wal_name(db: &Db) -> String {
    let (p, _, _) = db.wal_durable();
    p.file_name().unwrap().to_string_lossy().into_owned()
}

fn a_store_just_created(tag: &str, opts: Options) {
    let d = dir(&format!("dur-created-header-{tag}"));
    let crash = dir(&format!("dur-created-header-{tag}-cut"));
    let db = Db::create(&d, opts.clone()).unwrap();
    let wal = live_wal_name(&db);
    let (p, synced, written) = db.wal_durable();
    assert_eq!(synced, 0, "{tag}: no sync covered the header");
    drop(db);
    let real = p.parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::metadata(real.join(&wal)).unwrap().len(),
        written,
        "{tag}: a fresh store's log holds only its header"
    );
    copy_store(&real, &crash);
    zero_prefix(&crash.join(&wal), usize::MAX);
    let mut db = Db::open(&crash, opts.clone()).unwrap_or_else(|e| {
        panic!("{tag}: a store whose log header never reached the device is refused: {e}")
    });
    put_commit(&mut db, "after", "A").unwrap();
    drop(db);
    let db = Db::open(&crash, opts).unwrap();
    reads(
        &db,
        "after",
        "A",
        &format!("{tag}: committed after the open"),
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A store just created: `Db::create` returned `Ok`, the log's header was
/// written and never synced, and a power cut that keeps the file's size
/// and not its page leaves zeroes under `wal-00000000`. The store opens,
/// empty, and takes writes that a reopen reads back.
#[test]
fn a_store_just_created_opens_after_a_power_cut_loses_its_logs_unsynced_header() {
    a_store_just_created("plain", Options::default());
}

#[test]
fn a_store_just_created_opens_after_a_power_cut_loses_its_logs_unsynced_header_under_a_cipher() {
    a_store_just_created("cipher", under_cipher());
}

/// The default store, durable, loaded in key order: every commit goes to
/// the direct run and fdatasyncs the run's own file, so each one is
/// acknowledged, and the log is never synced -- it holds its header alone,
/// unsynced, for the whole load. A power cut that loses that header must
/// not cost the acknowledged commits: the store opens and reads every one,
/// at this open and at the next.
#[test]
fn acknowledged_direct_run_commits_survive_a_power_cut_that_loses_the_logs_unsynced_header() {
    let d = dir("dur-run-header");
    let crash = dir("dur-run-header-cut");
    let opts = Options::default();
    assert!(
        matches!(opts.sync, SyncPolicy::Always),
        "the default is durable"
    );
    let k = |i: u32| format!("k{i:05}").into_bytes();
    let val = vec![7u8; 100];
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut n = 0u32;
    for _ in 0..64 {
        for _ in 0..8 {
            db.append(&k(n), &val);
            n += 1;
        }
        db.commit().unwrap();
    }
    drop(db);
    assert!(
        open_run(&d),
        "an ordered load on a fresh store opens a direct run"
    );
    assert_eq!(
        std::fs::metadata(d.join("wal-00000000")).unwrap().len(),
        WAL_HEAD,
        "the log holds only its header, which no fdatasync of it has covered"
    );
    copy_store(&d, &crash);
    zero_prefix(&crash.join("wal-00000000"), usize::MAX);
    let mut refused = Vec::new();
    for round in 0..2 {
        match Db::open(&crash, opts.clone()) {
            Ok(db) => {
                for i in 0..n {
                    assert_eq!(
                        read_vec(&db, &k(i)),
                        vec![val.clone()],
                        "open {round}, key {i}"
                    );
                }
                drop(db);
            }
            Err(e) => refused.push(format!("open {round}: {e}")),
        }
    }
    assert!(
        refused.is_empty(),
        "acknowledged commits made unreachable by a lost log header: {refused:#?}"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A buffered store: commits never sync, and a flush acknowledges them by
/// a segment and a manifest without syncing the log
/// (`Options::drain_syncs_wal` off). The log's header and frames may then
/// never be synced in the file's life, and a power cut can leave its first
/// page zero -- with the later pages written back by the kernel, or with
/// the whole file zero. Everything the flush acknowledged is in the
/// segment, and the store opens and reads it, in either case.
fn a_flush_survives_a_lost_first_page(tag: &str, base: Options) {
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        direct_ingest: false,
        ..base
    };
    let k = |i: u32| format!("k{:05}", (i * 7919) % 100_000).into_bytes();
    let val = vec![7u8; 100];
    let d = dir(&format!("dur-buffered-header-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let wal = live_wal_name(&db);
    let n = 400u32;
    for c in 0..n / 8 {
        for i in c * 8..c * 8 + 8 {
            db.append(&k(i), &val);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    assert_eq!(db.wal_durable().1, 0, "{tag}: nothing synced the log");
    drop(db);
    let len = std::fs::metadata(d.join(&wal)).unwrap().len();
    assert!(
        len > 2 * 4096,
        "{tag}: the flush left the live log with its frames, over several pages: {len}"
    );
    let mut failed = Vec::new();
    for (name, zeroed) in [("page", 4096usize), ("whole", usize::MAX)] {
        let crash = dir(&format!("dur-buffered-header-{tag}-{name}"));
        copy_store(&d, &crash);
        zero_prefix(&crash.join(&wal), zeroed);
        match Db::open(&crash, opts.clone()) {
            Ok(db) => {
                for i in 0..n {
                    assert_eq!(
                        read_vec(&db, &k(i)),
                        vec![val.clone()],
                        "{tag} {name}, key {i}"
                    );
                }
                drop(db);
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
        let _ = std::fs::remove_dir_all(&crash);
    }
    assert!(
        failed.is_empty(),
        "{tag}: a flushed store refused for a log no sync ever covered: {failed:#?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_flush_survives_a_power_cut_that_loses_the_first_page_of_a_never_synced_log() {
    a_flush_survives_a_lost_first_page("plain", Options::default());
}

#[test]
fn a_flush_survives_a_power_cut_that_loses_the_first_page_of_a_never_synced_log_under_a_cipher() {
    a_flush_survives_a_lost_first_page("cipher", under_cipher());
}

/// A durable store whose log rotated by size: the commit that crossed the
/// boundary was synced into the closed file, and the new file's header
/// was written and not synced. A power cut that loses it leaves the
/// newest file zero, and the store opens with every commit the closed
/// file holds.
#[test]
fn a_log_file_a_rotation_began_whose_header_a_power_cut_lost_opens_with_every_commit_before_it() {
    let d = dir("dur-rotated-header");
    let crash = dir("dur-rotated-header-cut");
    let opts = Options {
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "the log never rotated");
    }
    assert_eq!(db.levels(), (0, 0), "nothing sealed");
    copy_store(&d, &crash);
    drop(db);
    let names = wal_names(&crash);
    assert_eq!(
        names.len(),
        2,
        "the closed file and the one the rotation began"
    );
    assert_eq!(
        std::fs::metadata(crash.join(&names[1])).unwrap().len(),
        WAL_HEAD,
        "the new file holds its header alone"
    );
    zero_prefix(&crash.join(&names[1]), usize::MAX);
    let db = Db::open(&crash, opts)
        .unwrap_or_else(|e| panic!("a lost header of a never-synced log file is refused: {e}"));
    for i in 0..i {
        reads(&db, &format!("r{i:07}"), &val, "acknowledged under Always");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A closed log file no sync ever covered, its first page lost: a buffered
/// store's first file after a loose rotation. The log ends there; the
/// files after it continue a tail the power cut took, and the store opens
/// as the state before it.
#[test]
fn an_older_log_file_whose_unsynced_header_a_power_cut_lost_opens_as_the_logs_end() {
    let d = dir("dur-older-header");
    let crash = dir("dur-older-header-cut");
    let opts = rotating();
    let (_, _) = rotated_and_killed(&d, opts.clone());
    copy_store(&d, &crash);
    let names = wal_names(&crash);
    zero_prefix(&crash.join(&names[0]), 4096);
    let db = Db::open(&crash, opts)
        .unwrap_or_else(|e| panic!("a lost header of a never-synced log file is refused: {e}"));
    // Nothing of either file was acknowledged; what shows is a prefix,
    // which with the first file's first page gone is nothing.
    assert!(
        read_vec(&db, b"r0000000").is_empty(),
        "the frames behind a lost first page are not replayed"
    );
    assert!(
        read_vec(&db, b"s0000000").is_empty(),
        "nothing after a lost tail is replayed"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// Under `Always`, a frame behind a zero header is a frame no sync
/// covered, since a sync covers the header too: here a buffered life's
/// commits after a durable life's, in the file the durable life's rotation
/// began, its first sector lost -- with the rest of its first page kept,
/// and with that page lost whole. The open drops them, as a power cut may,
/// and reads every commit the durable life acknowledged.
#[test]
fn frames_behind_a_lost_first_sector_are_dropped_as_unsynced() {
    for lost in [512usize, 4096] {
        let d = dir(&format!("dur-behind-header-{lost}"));
        let crash = dir(&format!("dur-behind-header-{lost}-cut"));
        let always = Options {
            seal_rotates_wal: false,
            seal_bytes: 64 << 10,
            seal_max_pct: 0,
            ..Options::default()
        };
        let mut db = Db::create(&made(&d), always.clone()).unwrap();
        let val = "x".repeat(100);
        let mut i = 0u32;
        while db.seal_waits().wal_rotations == 0 {
            put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
            i += 1;
            assert!(i < 10_000, "the log never rotated");
        }
        killed(db, &d);
        let buffered = Options {
            sync: SyncPolicy::EveryN(u32::MAX),
            ..always.clone()
        };
        let mut db = Db::open(&d, buffered).unwrap();
        let later = [('c', 100), ('d', 100), ('e', 100), ('f', 100)];
        for (b, n) in later {
            put_batch(&mut db, b, n);
        }
        assert_eq!(db.levels(), (0, 0), "nothing sealed");
        copy_store(&d, &crash);
        drop(db);
        let names = wal_names(&crash);
        assert_eq!(names.len(), 2);
        let newest = crash.join(&names[1]);
        assert!(std::fs::metadata(&newest).unwrap().len() > 2 * 4096);
        zero_prefix(&newest, lost);
        let bytes = std::fs::read(&newest).unwrap();
        assert!(
            bytes[lost..4096.max(lost)].iter().all(|&b| b == 0) == (lost == 4096)
                && bytes[4096..].iter().any(|&b| b != 0),
            "{lost}: frames kept behind the lost bytes, in the first page where a sector alone was lost"
        );
        let db = Db::open(&crash, always).unwrap_or_else(|e| {
            panic!("{lost}: a lost first sector of a never-synced log file is refused: {e}")
        });
        for i in 0..i {
            reads(&db, &format!("r{i:07}"), &val, "acknowledged under Always");
        }
        assert_eq!(
            a_prefix_of(&db, &later, "the buffered life"),
            0,
            "{lost}: frames behind a lost first sector are not replayed"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// A zero header over a first sector that still holds frames is not a
/// page a power cut lost -- the page would have gone whole -- but bytes
/// changed on the device, under acknowledged commits: refused, in words,
/// with the store's files left as they were.
#[test]
fn a_zeroed_header_over_frames_in_its_first_sector_is_refused_as_damage() {
    let d = dir("dur-header-damage");
    let opts = Options::default();
    let mut db = Db::create(&made(&d), opts.clone()).unwrap();
    put_commit(&mut db, "k0", "v0").unwrap();
    put_commit(&mut db, "k1", "v1").unwrap();
    killed(db, &d);
    zero_prefix(&d.join("wal-00000000"), WAL_HEAD as usize);
    let found = files_of(&d);
    match Db::open(&d, opts) {
        Ok(_) => panic!("a log whose header was zeroed over acknowledged frames opened"),
        Err(e) => assert!(
            e.to_string().contains("not a supdb WAL"),
            "refused as a log that is not one: {e}"
        ),
    }
    assert_eq!(files_of(&d), found, "a refused open leaves the files");
    let _ = std::fs::remove_dir_all(&d);
}

/// A header that is neither a log's nor zeroes is refused, in words, with
/// the files left as they were: a power cut leaves a page lost or whole.
#[test]
fn a_foreign_log_header_is_refused() {
    for (tag, head) in [
        ("other-name", *b"SUPDBWX\x04"),
        ("other-kind", *b"SUPDBWL\x09"),
        ("noise", *b"\x00\x00\x00\x00\x00\x00\x00\x01"),
    ] {
        let d = dir(&format!("dur-header-foreign-{tag}"));
        let opts = Options::default();
        let db = Db::create(&d, opts.clone()).unwrap();
        drop(db);
        {
            use std::os::unix::fs::FileExt;
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(d.join("wal-00000000"))
                .unwrap();
            f.write_at(&head, 0).unwrap();
        }
        let found = files_of(&d);
        match Db::open(&d, opts) {
            Ok(_) => panic!("{tag}: a foreign log header opened"),
            Err(e) => assert!(
                e.to_string().contains("not a supdb WAL"),
                "{tag}: refused as a log that is not one: {e}"
            ),
        }
        assert_eq!(
            files_of(&d),
            found,
            "{tag}: a refused open leaves the files"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A closed log file whose first sector is zeroes, before a file whose
/// header says chained: the chained header was written by the sync that
/// synced the closed file, header and frames, first, so no power cut
/// leaves that file's first sector lost, and the zeroes are bytes changed
/// on the device under acknowledged commits. The gap they leave before the
/// chained file is refused, in words, plain and sealed, with the store's
/// files as they were found -- never read as the log's end, which would
/// drop the chained file's acknowledged frames with nothing said.
fn a_zero_older_file_before_a_chained_one(tag: &str, base: Options) {
    let d = dir(&format!("dur-older-zero-chained-{tag}"));
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..base
    };
    let mut db = Db::create(&made(&d), opts.clone()).unwrap();
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "{tag}: the log never rotated");
    }
    put_commit(&mut db, "s0000000", &val).unwrap();
    assert!(
        db.seal_waits().rotated_unsynced > 0,
        "{tag}: a loose rotation"
    );
    // The sync that syncs the closed file and chains the live one.
    db.sync().unwrap();
    assert_eq!(db.levels(), (0, 0), "{tag}: nothing sealed");
    killed(db, &d);
    let names = wal_names(&d);
    assert_eq!(names.len(), 2, "{tag}: the closed file and the live one");
    let live = std::fs::read(d.join(&names[1])).unwrap();
    assert_ne!(
        live[WAL_KIND_AT as usize], WAL_LOOSE_KIND,
        "{tag}: the live header says chained"
    );
    zero_prefix(&d.join(&names[0]), 512);
    let found = files_of(&d);
    match Db::open(&d, opts) {
        Ok(_) => panic!("{tag}: a zeroed log file before a chained one opened as the log's end"),
        Err(e) => assert!(
            e.to_string().contains("sequence gap"),
            "{tag}: refused for the gap: {e}"
        ),
    }
    assert_eq!(
        files_of(&d),
        found,
        "{tag}: a refused open leaves the files"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_zeroed_older_log_file_before_a_chained_one_is_refused_as_damage() {
    a_zero_older_file_before_a_chained_one("plain", Options::default());
}

#[test]
fn a_zeroed_older_log_file_before_a_chained_one_is_refused_as_damage_under_a_cipher() {
    a_zero_older_file_before_a_chained_one("cipher", under_cipher());
}

/// A kill with batch `b` unsynced and a torn batch behind it: the open cuts
/// the tail off and fdatasyncs the live file, which makes `b` durable and
/// is the trace's first record; the sync after it issues nothing more, and
/// a power cut that keeps what the syncs covered -- the open's included,
/// and only because the trace records it -- reads both batches back.
fn a_sync_after_a_reopen_that_cut_a_torn_tail(tag: &str, opts: Options) {
    let d = dir(&format!("dur-reopen-torn-{tag}"));
    let crash = dir(&format!("dur-reopen-torn-{tag}-cut"));
    let one = life_one(&d, opts.clone());
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(d.join("wal-00000000"))
            .unwrap();
        std::io::Write::write_all(&mut f, &[9, 0, 0, 0, 1, 2, 3]).unwrap();
    }
    let mut db = life_two(&d, opts.clone());
    let at_open = db.wal_syncs_traced();
    let (live, _, written) = db.wal_durable();
    assert_eq!(
        at_open,
        vec![(live, Some(written))],
        "{tag}: the open's cut synced the live file"
    );
    db.sync().unwrap();
    assert_eq!(
        db.wal_syncs_traced(),
        at_open,
        "{tag}: nothing more owed after the open's sync"
    );
    copy_store(&d, &crash);
    power_cut_logs(&crash, &[&one, &db.wal_syncs_traced()], WAL_HEAD, &[]);
    drop(db);
    let db = Db::open(&crash, opts).unwrap();
    every_key_of(
        &db,
        &[('a', 100), ('b', 100)],
        &format!("{tag}: sync after a reopen that cut a torn tail"),
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_reopen_that_cuts_a_torn_tail_makes_the_replayed_writes_durable() {
    a_sync_after_a_reopen_that_cut_a_torn_tail("plain", buffered());
}

#[test]
fn a_reopen_that_cuts_a_torn_tail_makes_the_replayed_writes_durable_under_a_cipher() {
    let o = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..under_cipher()
    };
    a_sync_after_a_reopen_that_cut_a_torn_tail("cipher", o);
}

/// A sync whose fdatasync of a loose file fails returns before it rewrites
/// the live file's header: the kind stays loose in the page cache, so the
/// kernel's write-back of that header beside a power cut that takes the
/// loose file's tail leaves a log that ends at the boundary. A header
/// chained before the loose file was synced, written back, reads that gap
/// as a record lost and the store is refused for a sync that failed.
fn a_failed_loose_sync_leaves_the_header(tag: &str, base: Options) {
    let d = dir(&format!("dur-loose-fail-head-{tag}"));
    let crash = dir(&format!("dur-loose-fail-head-{tag}-cut"));
    let opts = Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        seal_max_pct: 0,
        ..base
    };
    let mut db = Db::create(&made(&d), opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    // The header's length, plain or sealed: a fresh log holds it alone.
    let head = db.wal_durable().2;
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "{tag}: the log never rotated");
    }
    put_commit(&mut db, "s0000000", &val).unwrap();
    db.fail_next_loose_wal_sync();
    assert!(db.sync().is_err(), "{tag}: the injected failure answers");
    assert_eq!(db.levels(), (0, 0), "{tag}: nothing sealed");
    let traced = db.wal_syncs_traced();
    killed(db, &d);
    let names = wal_names(&d);
    assert_eq!(names.len(), 2, "{tag}: the closed file and the live one");
    copy_store(&d, &crash);
    // The live file's header as the page cache holds it, written back whole
    // with the file; the loose file's frames, which no sync covered, lost.
    let header: Vec<u8> = std::fs::read(crash.join(&names[1])).unwrap()[..head as usize].to_vec();
    power_cut_logs(&crash, &[&traced], head, &[names[1].as_str()]);
    {
        use std::os::unix::fs::FileExt;
        std::fs::OpenOptions::new()
            .write(true)
            .open(crash.join(&names[1]))
            .unwrap()
            .write_at(&header, 0)
            .unwrap();
    }
    let db = Db::open(&crash, opts).unwrap_or_else(|e| {
        panic!("{tag}: a store whose loose file's sync failed is refused after a power cut: {e}")
    });
    assert!(
        read_vec(&db, b"r0000000").is_empty() && read_vec(&db, b"s0000000").is_empty(),
        "{tag}: the log ends at the lost tail"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_failed_sync_of_a_loose_log_file_leaves_the_live_header_loose() {
    a_failed_loose_sync_leaves_the_header("plain", Options::default());
}

#[test]
fn a_failed_sync_of_a_loose_log_file_leaves_the_live_header_loose_under_a_cipher() {
    a_failed_loose_sync_leaves_the_header("cipher", under_cipher());
}

/// Three log files, the two older left loose by their rotations, killed and
/// reopened: the sync after the reopen syncs every older file replay read,
/// oldest first, before it chains and syncs the live one, and a power cut
/// that keeps what those syncs covered reads every commit back. A file left
/// off the list is cut by the power cut, and the loose header after it
/// ends the log there. Two rotations' worth of frames is a table past the
/// seal size, so every seal is held before its landing in both lives: a
/// landing would retire the older files it covers, and the files asked
/// for here are the ones the sync has to make durable.
#[test]
fn a_sync_after_a_reopen_syncs_every_replayed_older_file_oldest_first() {
    let d = dir("dur-reopen-three");
    let crash = dir("dur-reopen-three-cut");
    let opts = Options {
        seal_idle: false,
        ..rotating()
    };
    let m = made(&d);
    let mut db = Db::create(&m, opts.clone()).unwrap();
    db.hold_seal_landing(true);
    db.trace_wal_syncs(true);
    let val = "x".repeat(100);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations < 2 {
        put_commit(&mut db, &format!("r{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 20_000, "the log never rotated twice");
    }
    put_commit(&mut db, "s0000000", &val).unwrap();
    assert_eq!(db.levels(), (0, 0), "nothing landed");
    let one = db.wal_syncs_traced();
    assert!(one.is_empty(), "nothing synced in life 1: {one:?}");
    // The kill: the files as they stand, then the original let go.
    copy_store(&m, &d);
    db.hold_seal_landing(false);
    drop(db);
    let _ = std::fs::remove_dir_all(&m);
    let names = wal_names(&d);
    assert_eq!(names.len(), 3, "three log files");
    for n in &names[1..] {
        assert_eq!(
            std::fs::read(d.join(n)).unwrap()[WAL_KIND_AT as usize],
            WAL_LOOSE_KIND,
            "{n}: both rotations left the file before loose"
        );
    }
    let mut db = Db::open(&d, opts.clone()).unwrap();
    db.hold_seal_landing(true);
    db.trace_wal_syncs(true);
    db.sync().unwrap();
    assert_eq!(db.levels(), (0, 0), "nothing landed after the reopen");
    let two = db.wal_syncs_traced();
    let order: Vec<(String, bool)> = two
        .iter()
        .map(|(p, ev)| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                ev.is_some(),
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            (names[0].clone(), true),
            (names[1].clone(), true),
            (names[2].clone(), false),
            (names[2].clone(), true),
        ],
        "the older files synced oldest first, then the live one chained and synced"
    );
    copy_store(&d, &crash);
    db.hold_seal_landing(false);
    drop(db);
    power_cut_logs(&crash, &[&one, &two], WAL_HEAD, &[]);
    let db = Db::open(&crash, opts)
        .unwrap_or_else(|e| panic!("a store synced after its reopen is refused: {e}"));
    for i in 0..i {
        reads(&db, &format!("r{i:07}"), &val, "an older file's frames");
    }
    reads(&db, "s0000000", &val, "the live file's frame");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// The log-errors section's failed write, in a store that recycles its log
/// files (`Options::recycle_wal`): the live file was written through to
/// `seal_bytes` and synced at its creation, so the cut back of a failed
/// write shortens a pre-written file. Every commit reads back.
#[test]
fn a_failed_log_write_in_a_recycled_log_loses_no_commit_acknowledged_after_it() {
    let opts = Options {
        recycle_wal: true,
        ..Options::default()
    };
    a_failed_write_is_written_again("recycled", opts.clone(), 5, false);
    a_failed_write_is_written_again("recycled-reopened", opts, 5, true);
}

/// The log-errors section's rotation after a failed write, with the power
/// cut applied: each log file cut to what its syncs covered. Under
/// `Always` every acknowledged commit was synced, the new file's name by
/// the rotation's directory fsync, and every one reads back.
#[test]
fn a_store_whose_log_rotated_after_a_failed_write_survives_a_power_cut() {
    for recycle_wal in [false, true] {
        let tag = if recycle_wal { "recycled" } else { "plain" };
        let d = dir(&format!("dur-walwrite-rotcut-{tag}"));
        let crash = dir(&format!("dur-walwrite-rotcut-{tag}-cut"));
        let opts = Options {
            seal_rotates_wal: false,
            seal_bytes: 64 << 10,
            seal_max_pct: 0,
            recycle_wal,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts.clone()).unwrap();
        db.trace_wal_syncs(true);
        put_commit(&mut db, "a0000000", "v").unwrap();
        db.fail_next_wal_write(5);
        assert!(put_commit(&mut db, "a0000001", "v").is_err());
        let val = "x".repeat(100);
        let mut acked: Vec<String> = Vec::new();
        let mut i = 0u32;
        while db.seal_waits().wal_rotations == 0 {
            let k = format!("b{i:07}");
            put_commit(&mut db, &k, &val).unwrap();
            acked.push(k);
            i += 1;
            assert!(i < 10_000, "{tag}: the log never rotated");
        }
        put_commit(&mut db, "c0000000", &val).unwrap();
        acked.push("c0000000".into());
        assert_eq!(db.levels(), (0, 0), "{tag}: nothing sealed");
        copy_store(&d, &crash);
        let traced = db.wal_syncs_traced();
        drop(db);
        // A pre-written file's tail of zeroes is synced with it; the cut
        // keeps what the syncs covered and no more.
        power_cut_logs(&crash, &[&traced], WAL_HEAD, &[]);
        let db =
            Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the power cut opens: {e}"));
        reads(&db, "a0000000", "v", tag);
        reads(
            &db,
            "a0000001",
            "v",
            &format!("{tag}: the batch written again"),
        );
        for k in &acked {
            reads(&db, k, &val, &format!("{tag}: acknowledged under Always"));
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

// ---------------------------------------------------------------------
// sync-run: a `sync` makes an open run's commits durable whatever the
// policy left of them -- the tail its writer still held written, the
// run's file synced -- and syncs the log wherever it holds frames no
// sync covered, the live table ordered or not.
// ---------------------------------------------------------------------

/// A store whose writes above its greatest key go to one direct run for
/// as long as these tests write, sealed by nothing but a test's own call,
/// under `sync` and the run's arm `arm`.
fn sr_opts(sync: SyncPolicy, arm: Options) -> Options {
    Options {
        sync,
        seal_bytes: 64 << 20,
        seal_max_pct: 0,
        seal_idle: false,
        partition_bytes: Some(1 << 20),
        ..arm
    }
}

fn sr_key(k: u32) -> Vec<u8> {
    format!("key-{k:06}").into_bytes()
}

/// A hundred bytes naming its key, so a value read back is the key's own.
fn sr_val(k: u32) -> Vec<u8> {
    let mut v = format!("v{k:06}-").into_bytes();
    v.resize(100, b'b');
    v
}

/// Keys `from..to` appended above the store's greatest, in commits of
/// `per`: every one goes to the run. `append`, since a `put` is a replace,
/// which no run takes.
fn sr_commits(db: &mut Db, from: u32, to: u32, per: u32) {
    let mut k = from;
    while k < to {
        for k in k..(k + per).min(to) {
            db.append(&sr_key(k), &sr_val(k));
        }
        db.commit().unwrap();
        k += per;
    }
}

/// The keys of `0..n` that do not read back their one value in `db`, as
/// a failure's words, or `None`.
fn sr_missing(db: &Reader, n: u32, what: &str) -> Option<String> {
    let missing: Vec<u32> = (0..n)
        .filter(|&k| read_vec(db, &sr_key(k)) != vec![sr_val(k)])
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "{what}: {} of {n} keys written before an Ok sync did not read back, the first {:?}",
            missing.len(),
            String::from_utf8_lossy(&sr_key(missing[0]))
        )
    })
}

/// What a run holds when `sync` is called: one commit of a hundred, the
/// one commit no sync has covered; three commits of a hundred, none of
/// them on the file at all; twelve of a thousand, whole write
/// pieces on the file and the tail held; three of a hundred and a hundred
/// more staged, which the sync itself commits; three of a hundred
/// synced, then three more, then synced again; and three of a hundred
/// synced, then a hundred staged, which the second sync commits as the
/// one commit no sync has covered; and six of a hundred, under `EveryN(5)`
/// one commit past the policy's due sync.
#[derive(Clone, Copy, Debug)]
enum RunHeld {
    One,
    Small,
    Large,
    Staged,
    Twice,
    SyncedThenStaged,
    AfterDue,
}

const RUN_HELD: [RunHeld; 7] = [
    RunHeld::One,
    RunHeld::Small,
    RunHeld::Large,
    RunHeld::Staged,
    RunHeld::Twice,
    RunHeld::SyncedThenStaged,
    RunHeld::AfterDue,
];

/// The run `held` names written and synced: the keys committed before the
/// last `sync`, which answered `Ok`.
fn sr_written_and_synced(db: &mut Db, held: RunHeld) -> u32 {
    match held {
        RunHeld::One => sr_commits(db, 0, 100, 100),
        RunHeld::Small => sr_commits(db, 0, 300, 100),
        RunHeld::Large => sr_commits(db, 0, 12_000, 1000),
        RunHeld::AfterDue => sr_commits(db, 0, 600, 100),
        RunHeld::Staged => {
            sr_commits(db, 0, 300, 100);
            for k in 300..400 {
                db.append(&sr_key(k), &sr_val(k));
            }
        }
        RunHeld::Twice => {
            sr_commits(db, 0, 300, 100);
            db.sync().unwrap();
            sr_commits(db, 300, 600, 100);
        }
        RunHeld::SyncedThenStaged => {
            sr_commits(db, 0, 300, 100);
            db.sync().unwrap();
            for k in 300..400 {
                db.append(&sr_key(k), &sr_val(k));
            }
        }
    }
    db.sync().unwrap();
    match held {
        RunHeld::One => 100,
        RunHeld::Small => 300,
        RunHeld::Large => 12_000,
        RunHeld::Staged => 400,
        RunHeld::Twice => 600,
        RunHeld::SyncedThenStaged => 400,
        RunHeld::AfterDue => 600,
    }
}

const SR_POLICIES: [(&str, SyncPolicy); 2] = [
    ("every5", SyncPolicy::EveryN(5)),
    ("buffered", SyncPolicy::EveryN(u32::MAX)),
];

/// A process killed after `sync` answered `Ok`, with a run open under a
/// policy that does not sync every commit: the page cache is kept, and
/// every commit before the sync reads back, the batch's tail the writer
/// held among them. Under `EveryN(5)` the large run's commits end two past
/// a due sync; under `EveryN(u32::MAX)` none is ever due. Every arm of the
/// run, since each keeps the records on their way to the file in its own
/// shape.
#[test]
fn a_sync_puts_an_open_runs_commits_where_a_process_kill_keeps_them() {
    let mut failures = Vec::new();
    for (arm, base) in run_arms() {
        for (pol, sync) in SR_POLICIES {
            for held in RUN_HELD {
                let tag = format!("{arm}-{pol}-{held:?}");
                let d = dir(&format!("dur-srkill-{tag}"));
                let crash = dir(&format!("dur-srkill-{tag}-crash"));
                let opts = sr_opts(sync, base.clone());
                let mut db = Db::create(&d, opts.clone()).unwrap();
                let n = sr_written_and_synced(&mut db, held);
                assert!(open_run(&d), "{tag}: the run is open");
                assert!(!db.in_flight().0, "{tag}: no seal in flight to write it");
                copy_store(&d, &crash);
                drop(db);
                let opened = Db::open(&crash, opts).unwrap();
                failures.extend(sr_missing(
                    &opened,
                    n,
                    &format!("{tag}: killed after the sync"),
                ));
                drop(opened);
                let _ = std::fs::remove_dir_all(&d);
                let _ = std::fs::remove_dir_all(&crash);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same runs, and a power cut after `sync` answered `Ok`: the run's
/// file keeps only what its writer's fdatasyncs covered, and the log what
/// its syncs did, and that is every commit before the sync.
#[test]
fn a_sync_makes_an_open_runs_commits_survive_a_power_cut() {
    let mut failures = Vec::new();
    for (arm, base) in run_arms() {
        for (pol, sync) in SR_POLICIES {
            for held in RUN_HELD {
                let tag = format!("{arm}-{pol}-{held:?}");
                let d = dir(&format!("dur-srcut-{tag}"));
                let crash = dir(&format!("dur-srcut-{tag}-crash"));
                let opts = sr_opts(sync, base.clone());
                let mut db = Db::create(&d, opts.clone()).unwrap();
                let n = sr_written_and_synced(&mut db, held);
                let synced = db
                    .run_synced_bytes()
                    .expect("the run is open after a sync that seals nothing");
                power_cut(&db, &d, &crash, run_ino(&d), synced);
                drop(db);
                let opened = Db::open(&crash, opts).unwrap();
                failures.extend(sr_missing(
                    &opened,
                    n,
                    &format!("{tag}: a power cut after the sync"),
                ));
                drop(opened);
                let _ = std::fs::remove_dir_all(&d);
                let _ = std::fs::remove_dir_all(&crash);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Commits into a hashed table, in the log and unsynced under the policy;
/// the table frozen and handed to a seal held before its fsyncs, so no
/// manifest names its segment; then a run opened beside it, and `sync`.
/// The live table being ordered is no reason to leave the log unsynced:
/// after a power cut the log's frames are the only copy of the first
/// commits, and the run's file of the second, and both read back.
#[test]
fn a_sync_with_a_run_open_makes_the_logs_unsynced_frames_survive_a_power_cut() {
    let opts = sr_opts(SyncPolicy::EveryN(u32::MAX), Options::default());
    let d = dir("dur-srlog");
    let crash = dir("dur-srlog-crash");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // Values the record cannot hold inline: the batch goes to the log and
    // a hashed table, and no run opens.
    let big = "x".repeat(opts.inline_bytes + 1);
    for k in 0..200u32 {
        db.append(format!("a-{k:06}").as_bytes(), big.as_bytes());
    }
    db.commit().unwrap();
    db.hold_seal_durable(true);
    // Everything between the hold and its release under `catch_unwind`, so
    // a failure lets the seal thread go before the store is dropped.
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        sr_commits(&mut db, 0, 300, 100);
        assert!(
            db.run_synced_bytes().is_some(),
            "the keys above the greatest opened a run beside the seal in flight"
        );
        assert!(
            db.in_flight().0,
            "the hashed table's seal is held in flight"
        );
        db.sync().unwrap();
        let synced = db.run_synced_bytes().expect("the run is open");
        power_cut(&db, &d, &crash, run_ino(&d), synced);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap();
    let a_missing = (0..200u32)
        .filter(|k| {
            read_vec(&opened, format!("a-{k:06}").as_bytes()) != vec![big.as_bytes().to_vec()]
        })
        .count();
    let run = sr_missing(&opened, 300, "the run's keys");
    assert!(
        a_missing == 0 && run.is_none(),
        "a power cut after an Ok sync: {a_missing} of 200 keys committed to the log \
         did not read back; {run:?}"
    );
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// Under `adaptive_shape` a `sync` hands the open run to a seal without
/// waiting for it. The commits it answered for are on the run's file
/// before the hand-off, not left to the seal thread: with the seal held
/// before it writes anything, a process killed after the answer keeps
/// every one. A second `sync` in a row, the run handed and nothing
/// committed since, answers `Ok` and leaves the handed run as it was.
#[test]
fn a_sync_that_hands_an_open_run_to_a_seal_has_put_its_commits_on_the_file() {
    let mut failures = Vec::new();
    for (arm, base) in [
        ("default", Options::default()),
        (
            "copyvals",
            Options {
                direct_stream: false,
                ..Options::default()
            },
        ),
    ] {
        for held in [RunHeld::Small, RunHeld::Large] {
            let tag = format!("{arm}-{held:?}");
            let opts = Options {
                adaptive_shape: true,
                ..sr_opts(SyncPolicy::EveryN(u32::MAX), base.clone())
            };
            let d = dir(&format!("dur-srhand-{tag}"));
            let crash = dir(&format!("dur-srhand-{tag}-crash"));
            let mut db = Db::create(&d, opts.clone()).unwrap();
            db.hold_seal_landing(true);
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let n = sr_written_and_synced(&mut db, held);
                db.sync()
                    .unwrap_or_else(|e| panic!("{tag}: a second sync after the hand-off: {e}"));
                assert!(
                    db.run_synced_bytes().is_none(),
                    "{tag}: the sync handed the run to a seal"
                );
                assert!(db.in_flight().0, "{tag}: its seal is held in flight");
                assert!(
                    open_run(&d),
                    "{tag}: the run's temp name stays until a manifest names it"
                );
                copy_store(&d, &crash);
                n
            }));
            db.hold_seal_landing(false);
            let n = match caught {
                Ok(n) => n,
                Err(p) => std::panic::resume_unwind(p),
            };
            drop(db);
            let opened = Db::open(&crash, opts).unwrap();
            failures.extend(sr_missing(
                &opened,
                n,
                &format!("{tag}: killed after a sync that handed the run"),
            ));
            drop(opened);
            let _ = std::fs::remove_dir_all(&d);
            let _ = std::fs::remove_dir_all(&crash);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Under a cipher a run's commits go to the log as well (`Db::log_run`),
/// and the log is their durable form, since the run's file is a memory
/// file until the run closes. Under a policy that does not sync every
/// commit, a `sync` with the run open syncs the frames the run left in
/// the log: after a power cut -- the memory files gone, the log cut to
/// what its syncs covered -- every commit before the sync reads back.
#[test]
fn a_sync_makes_an_open_runs_logged_commits_survive_a_power_cut_under_a_cipher() {
    let mut failures = Vec::new();
    for (pol, sync) in SR_POLICIES {
        for held in RUN_HELD {
            let tag = format!("{pol}-{held:?}");
            let d = dir(&format!("dur-srcipher-{tag}"));
            let crash = dir(&format!("dur-srcipher-{tag}-crash"));
            let opts = sr_opts(sync, under_cipher());
            let mut db = Db::create(&d, opts.clone()).unwrap();
            let n = sr_written_and_synced(&mut db, held);
            assert!(db.run_synced_bytes().is_some(), "{tag}: the run is open");
            let (log, log_synced, _) = db.wal_durable();
            copy_store(&d, &crash);
            let name = log.file_name().expect("the log's file name");
            std::fs::OpenOptions::new()
                .write(true)
                .open(crash.join(name))
                .unwrap()
                .set_len(log_synced)
                .unwrap();
            drop(db);
            let opened = Db::open(&crash, opts).unwrap();
            failures.extend(sr_missing(
                &opened,
                n,
                &format!("{tag}: a power cut after the sync"),
            ));
            drop(opened);
            let _ = std::fs::remove_dir_all(&d);
            let _ = std::fs::remove_dir_all(&crash);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A `sync` whose fdatasync of the open run's file fails answers the
/// failure, and the store keeps it: every commit, `sync` and `flush` after
/// it answers the kept error until the store is reopened, as after the
/// run's own commit's failed fdatasync (`SyncPolicy`), since a sync
/// retried would answer `Ok` over pages the device never took.
#[test]
fn a_sync_whose_run_fdatasync_fails_answers_it_and_keeps_it() {
    for (arm, base) in run_arms() {
        let tag = format!("{arm}-buffered");
        let d = dir(&format!("dur-srsyncfail-{tag}"));
        let opts = sr_opts(SyncPolicy::EveryN(u32::MAX), base);
        let mut db = Db::create(&d, opts).unwrap();
        sr_commits(&mut db, 0, 300, 100);
        db.fail_next_run_sync();
        injected(
            db.sync(),
            &format!("{tag}: the sync over the failed fdatasync"),
        );
        kept(db.commit(), &format!("{tag}: a commit of nothing after"));
        kept(db.sync(), &format!("{tag}: a sync after"));
        db.append(&sr_key(300), &sr_val(300));
        kept(db.commit(), &format!("{tag}: a commit after"));
        kept(db.flush(), &format!("{tag}: a flush after"));
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A `sync` whose write of what the open run holds fails answers the
/// write's failure, and the batch is owed: the next `sync` writes it at
/// the offset the file is good to and syncs it, and a power cut after that
/// `Ok` reads every commit back.
#[test]
fn a_sync_whose_write_of_the_runs_tail_fails_is_written_by_the_next_sync() {
    for (arm, base) in run_arms() {
        let tag = format!("{arm}-buffered");
        let d = dir(&format!("dur-srwritefail-{tag}"));
        let crash = dir(&format!("dur-srwritefail-{tag}-crash"));
        let opts = sr_opts(SyncPolicy::EveryN(u32::MAX), base);
        let mut db = Db::create(&d, opts.clone()).unwrap();
        sr_commits(&mut db, 0, 300, 100);
        db.fail_next_run_write();
        injected(db.sync(), &format!("{tag}: the sync over the failed write"));
        db.sync()
            .unwrap_or_else(|e| panic!("{tag}: the sync after writes the batch again: {e}"));
        let synced = db.run_synced_bytes().expect("the run is open");
        power_cut(&db, &d, &crash, run_ino(&d), synced);
        drop(db);
        let opened = Db::open(&crash, opts).unwrap();
        if let Some(m) = sr_missing(&opened, 300, &format!("{tag}: a power cut after")) {
            panic!("{m}");
        }
        drop(opened);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// The order inside a `sync`: the log's frames are older than every
/// commit of the open run, so the log is synced first, and a sync of the
/// log that fails leaves the run's file as the policy left it. A power cut
/// after that failure keeps a prefix of the commit order -- the run's
/// commits only with the log's before them.
#[test]
fn a_sync_with_a_run_open_syncs_the_log_before_the_run() {
    let opts = sr_opts(SyncPolicy::EveryN(u32::MAX), Options::default());
    let d = dir("dur-srorder");
    let crash = dir("dur-srorder-crash");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let big = "x".repeat(opts.inline_bytes + 1);
    for k in 0..200u32 {
        db.append(format!("a-{k:06}").as_bytes(), big.as_bytes());
    }
    db.commit().unwrap();
    db.hold_seal_durable(true);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        sr_commits(&mut db, 0, 300, 100);
        assert!(
            db.run_synced_bytes().is_some(),
            "a run opened beside the seal"
        );
        db.fail_next_wal_sync();
        kept(db.sync(), "the sync whose log sync failed");
        let synced = db.run_synced_bytes().expect("the run is open");
        power_cut(&db, &d, &crash, run_ino(&d), synced);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap();
    let a_missing = (0..200u32)
        .filter(|k| read_vec(&opened, format!("a-{k:06}").as_bytes()).is_empty())
        .count();
    let run_kept = (0..300u32)
        .filter(|&k| !read_vec(&opened, &sr_key(k)).is_empty())
        .count();
    assert!(
        a_missing == 0 || run_kept == 0,
        "a power cut after a sync whose log sync failed kept {run_kept} of the run's 300 \
         keys and lost {a_missing} of the 200 committed to the log before them"
    );
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A `sync` with nothing of the open run unsynced syncs nothing of it:
/// under `Always` every commit synced the run, and under `EveryN` a due
/// commit did. The run's fdatasync is armed to fail, and the `sync` that
/// owes nothing answers `Ok` with the failure still armed, which the
/// run's next commit then meets.
#[test]
fn a_sync_with_nothing_of_the_run_unsynced_syncs_nothing_of_it() {
    for (pol, sync, commits) in [
        ("always", SyncPolicy::Always, 1u32),
        ("every5-due", SyncPolicy::EveryN(5), 5),
    ] {
        let d = dir(&format!("dur-srnothing-{pol}"));
        let mut db = Db::create(&d, sr_opts(sync, Options::default())).unwrap();
        sr_commits(&mut db, 0, commits * 100, 100);
        let synced = db.run_synced_bytes().expect("the run is open");
        db.fail_next_run_sync();
        db.sync()
            .unwrap_or_else(|e| panic!("{pol}: a sync that owes the run nothing: {e}"));
        assert_eq!(db.run_synced_bytes(), Some(synced), "{pol}: nothing synced");
        let mut r = Ok(());
        for c in commits..commits + 5 {
            for k in c * 100..(c + 1) * 100 {
                db.append(&sr_key(k), &sr_val(k));
            }
            r = db.commit();
            if r.is_err() {
                break;
            }
        }
        injected(
            r,
            &format!("{pol}: the policy's next sync meets the armed failure"),
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A `sync` starts the policy's count over for the open run as it does
/// for the log: under `EveryN(5)`, three commits, a `sync`, and four
/// commits after it leave the run as the `sync` left it; the fifth after
/// it is due.
#[test]
fn a_sync_starts_the_policys_count_over_for_an_open_run() {
    let d = dir("dur-srcount");
    let mut db = Db::create(&d, sr_opts(SyncPolicy::EveryN(5), Options::default())).unwrap();
    sr_commits(&mut db, 0, 300, 100);
    db.sync().unwrap();
    let synced = db.run_synced_bytes().expect("the run is open");
    sr_commits(&mut db, 300, 700, 100);
    assert_eq!(
        db.run_synced_bytes(),
        Some(synced),
        "four commits after a sync are not due under EveryN(5)"
    );
    sr_commits(&mut db, 700, 800, 100);
    assert!(
        db.run_synced_bytes() > Some(synced),
        "the fifth commit after the sync is due"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A `sync` that finds only the open run owing syncs the run and nothing
/// of the log: under `EveryN(u32::MAX)` a `sync` leaves the log owing
/// nothing, the run's commits after it write nothing to the log, and the next
/// `sync` writes and syncs the run with no fdatasync of a log file
/// (`Db::trace_wal_syncs`).
#[test]
fn a_sync_with_only_the_run_owing_syncs_nothing_of_the_log() {
    let d = dir("dur-srlogclean");
    let mut db = Db::create(
        &d,
        sr_opts(SyncPolicy::EveryN(u32::MAX), Options::default()),
    )
    .unwrap();
    sr_commits(&mut db, 0, 300, 100);
    db.sync().unwrap();
    db.trace_wal_syncs(true);
    let traced = db.wal_syncs_traced().len();
    sr_commits(&mut db, 300, 600, 100);
    let synced = db.run_synced_bytes().expect("the run is open");
    db.sync().unwrap();
    assert!(
        db.run_synced_bytes() > Some(synced),
        "the sync synced the run's commits"
    );
    assert_eq!(
        db.wal_syncs_traced().len(),
        traced,
        "a sync that owed the log nothing synced a log file: {:?}",
        db.wal_syncs_traced()
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A due commit whose write of the run fails spends nothing of the
/// policy's count: under `EveryN(3)` the third commit's sync fails at the
/// write, and the fourth, which takes the third's batch with its own, is
/// the policy's sync -- a power cut after it keeps all four commits,
/// where a count started over would leave the first, second and fourth
/// unsynced past the policy's bound.
#[test]
fn a_run_commit_after_a_failed_due_write_is_due() {
    let d = dir("dur-srdue-after-fail");
    let crash = dir("dur-srdue-after-fail-crash");
    let opts = sr_opts(SyncPolicy::EveryN(3), Options::default());
    let mut db = Db::create(&d, opts.clone()).unwrap();
    sr_commits(&mut db, 0, 200, 100);
    let ino = run_ino(&d);
    db.fail_next_run_write();
    for k in 200..300 {
        db.append(&sr_key(k), &sr_val(k));
    }
    injected(db.commit(), "the due commit's write");
    let synced = db.run_synced_bytes().expect("the run is open");
    for k in 300..400 {
        db.append(&sr_key(k), &sr_val(k));
    }
    db.commit()
        .expect("the commit after the failure writes both batches");
    let now = db.run_synced_bytes().expect("the run is open");
    assert!(
        now > synced,
        "the commit after a failed due write was not the policy's sync: synced {synced} -> {now}"
    );
    power_cut(&db, &d, &crash, ino, now);
    drop(db);
    let opened = Db::open(&crash, opts).unwrap();
    if let Some(f) = sr_missing(&opened, 400, "a power cut after the due commit") {
        panic!("{f}");
    }
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A run closed under `EveryN(u32::MAX)` -- by a key below its greatest,
/// or by `Db::seal` -- goes to a seal with its commits unsynced; what
/// makes them durable is that seal's landing, which a `flush` waits for.
/// Cut to what the device was told to keep after the flush's `Ok`, every
/// commit before it reads back. (A `sync` in that window does not cover
/// the closed run: `SyncPolicy`, and `ROADMAP.md`.)
#[test]
fn a_flush_after_a_run_closed_into_a_seal_makes_the_runs_commits_durable() {
    for (closed, how) in [(false, "lower-key"), (true, "seal")] {
        let d = dir(&format!("dur-srclosedflush-{how}"));
        let crash = dir(&format!("dur-srclosedflush-{how}-crash"));
        let opts = sr_opts(SyncPolicy::EveryN(u32::MAX), Options::default());
        let mut db = Db::create(&d, opts.clone()).unwrap();
        sr_commits(&mut db, 0, 300, 100);
        if closed {
            db.seal().unwrap();
        } else {
            db.append(b"a-lower", b"below the run");
            db.commit().unwrap();
        }
        assert!(db.run_synced_bytes().is_none(), "{how}: the run closed");
        db.flush().unwrap();
        let (log, log_synced, _) = db.wal_durable();
        copy_store(&d, &crash);
        if let Some(name) = log.file_name() {
            if let Ok(f) = std::fs::OpenOptions::new()
                .write(true)
                .open(crash.join(name))
            {
                f.set_len(log_synced).unwrap();
            }
        }
        drop(db);
        let opened = Db::open(&crash, opts).unwrap();
        if let Some(m) = sr_missing(&opened, 300, &format!("{how}: a power cut after the flush")) {
            panic!("{m}");
        }
        if !closed {
            assert_eq!(
                read_vec(&opened, b"a-lower"),
                vec![b"below the run".to_vec()],
                "{how}: the lower key's commit"
            );
        }
        drop(opened);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// Under a cipher a run's commits are frames of the log (`Db::log_run`),
/// so a run closed into a seal in flight leaves them where a `sync`
/// covers them: with the seal held, a `sync` answered `Ok`, and a power
/// cut -- the memory files gone, the log cut to what its syncs covered --
/// every commit before the sync reads back, under both policies.
#[test]
fn a_sync_after_a_run_closed_into_a_seal_covers_its_logged_commits_under_a_cipher() {
    let mut failures = Vec::new();
    for (pol, sync) in SR_POLICIES {
        for (closed, how) in [(false, "lower-key"), (true, "seal")] {
            let tag = format!("{pol}-{how}");
            let d = dir(&format!("dur-srclosedcipher-{tag}"));
            let crash = dir(&format!("dur-srclosedcipher-{tag}-crash"));
            let opts = sr_opts(sync, under_cipher());
            let mut db = Db::create(&d, opts.clone()).unwrap();
            sr_commits(&mut db, 0, 300, 100);
            db.hold_seal_landing(true);
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if closed {
                    db.seal().unwrap();
                } else {
                    db.append(b"a-lower", b"below the run");
                    db.commit().unwrap();
                }
                assert!(db.run_synced_bytes().is_none(), "{tag}: the run closed");
                assert!(db.in_flight().0, "{tag}: its seal is held in flight");
                db.sync().unwrap();
                let (log, log_synced, _) = db.wal_durable();
                copy_store(&d, &crash);
                let name = log.file_name().expect("the log's file name");
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(crash.join(name))
                    .unwrap()
                    .set_len(log_synced)
                    .unwrap();
            }));
            db.hold_seal_landing(false);
            if let Err(p) = caught {
                std::panic::resume_unwind(p);
            }
            drop(db);
            let opened = Db::open(&crash, opts).unwrap();
            failures.extend(sr_missing(
                &opened,
                300,
                &format!("{tag}: a power cut after the sync"),
            ));
            if !closed && read_vec(&opened, b"a-lower").is_empty() {
                failures.push(format!("{tag}: the lower key's commit lost"));
            }
            drop(opened);
            let _ = std::fs::remove_dir_all(&d);
            let _ = std::fs::remove_dir_all(&crash);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The helper thread behind the run's writer syncs the run's pieces as
/// they reach the file, and its fdatasync fails before the `sync`: a
/// `sync` over the run answers the helper's failure, since the writer's
/// own fdatasync on the same open file description reports nothing, and
/// the store keeps it.
#[test]
fn a_sync_over_a_run_whose_helper_sync_failed_answers_it_and_keeps_it() {
    let d = dir("dur-srhelper");
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let opts = Options {
        segment: SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            fail_helper_syncs: Some(failed.clone()),
            ..Default::default()
        },
        ..sr_opts(SyncPolicy::EveryN(u32::MAX), Options::default())
    };
    let mut db = Db::create(&d, opts).unwrap();
    sr_commits(&mut db, 0, 3000, 1000);
    wait_count(&failed, 1, "the run's helper never took its turn at a sync");
    assert!(db.run_synced_bytes().is_some(), "the run is open");
    assert!(
        db.sync().is_err(),
        "a sync over the run, after its records failed their helper's sync, is not answered Ok"
    );
    kept(db.commit(), "a commit of nothing after");
    kept(db.sync(), "a sync after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A run's commits count toward the policy's `n` across the run's close:
/// under `EveryN(5)`, three commits of a run, the run closed -- by a key
/// below its greatest, committed to the log, or by `Db::seal` and a
/// commit below it -- and the fifth commit is the policy's sync of the
/// log.
#[test]
fn a_runs_commits_count_toward_the_policys_sync_after_it_closes() {
    for (seal, how) in [(false, "lower-key"), (true, "seal")] {
        let d = dir(&format!("dur-srclosedcount-{how}"));
        let mut db = Db::create(&d, sr_opts(SyncPolicy::EveryN(5), Options::default())).unwrap();
        sr_commits(&mut db, 0, 300, 100);
        if seal {
            db.seal().unwrap();
        }
        db.append(b"a-lower-1", b"below the run");
        db.commit().unwrap();
        assert!(db.run_synced_bytes().is_none(), "{how}: the run closed");
        let (_, synced, written) = db.wal_durable();
        assert!(synced < written, "{how}: the fourth commit is not due");
        db.append(b"a-lower-2", b"below the run");
        db.commit().unwrap();
        let (_, synced, written) = db.wal_durable();
        assert_eq!(
            synced, written,
            "{how}: the fifth commit is the policy's sync"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A run whose commits end on a write piece's boundary: every byte of
/// them on the file and nothing held by the writer, and none of it synced
/// under `EveryN(u32::MAX)`. A `sync` syncs the file all the same, so a
/// power cut after its `Ok` keeps every commit. Each commit is one record,
/// and the last one's value is sized to end the stream on the boundary.
#[test]
fn a_sync_syncs_a_run_whose_commits_end_on_a_piece_boundary() {
    const PIECE: u64 = 1 << 20;
    let mut failures = Vec::new();
    for (arm, base) in run_arms() {
        let d = dir(&format!("dur-srpiece-{arm}"));
        let crash = dir(&format!("dur-srpiece-{arm}-crash"));
        let opts = sr_opts(SyncPolicy::EveryN(u32::MAX), base);
        let mut db = Db::create(&d, opts.clone()).unwrap();
        let end = |db: &Db| {
            let (w, h) = db.run_written_and_held().expect("the run is open");
            w + h
        };
        let mut k = 0u32;
        let mut prev = 0u64;
        let last = loop {
            assert!(k < 20_000, "{arm}: the stream never came near a boundary");
            let now = if k == 0 { 0 } else { end(&db) };
            // What one commit of one key and a value of a hundred bytes
            // adds to the stream, read off the last such commit.
            let step = now - prev;
            let left = (now / PIECE + 1) * PIECE - now;
            if k > 2 && left + 90 >= step && left <= step + 20 {
                let len = (100 + left - step) as usize;
                let mut last = format!("v{k:06}-").into_bytes();
                last.resize(len, b'b');
                db.append(&sr_key(k), &last);
                db.commit().unwrap();
                k += 1;
                break last;
            }
            prev = now;
            sr_commits(&mut db, k, k + 1, 1);
            k += 1;
        };
        let (written, held) = db.run_written_and_held().expect("the run is open");
        let synced = db.run_synced_bytes().expect("the run is open");
        assert!(
            held == 0 && written % PIECE == 0 && synced < written,
            "{arm}: the commits end on a piece boundary, all of it on the file and none of it \
             synced: written {written}, held {held}, synced {synced}"
        );
        db.sync().unwrap();
        let synced = db.run_synced_bytes().expect("the run is open");
        power_cut(&db, &d, &crash, run_ino(&d), synced);
        drop(db);
        let opened = Db::open(&crash, opts).unwrap();
        failures.extend(sr_missing(
            &opened,
            k - 1,
            &format!("{arm}: a power cut after the sync"),
        ));
        if read_vec(&opened, &sr_key(k - 1)) != vec![last] {
            failures.push(format!("{arm}: the last commit's key did not read back"));
        }
        drop(opened);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The directory barrier a loose rotation owes the new log file's name is
/// paid by a `sync` with a run open, as with a hashed table live: a log
/// rotated loose under `EveryN(u32::MAX)`, its table sealed, a run opened
/// above it, and the `sync` meets the directory's fsync, armed to fail,
/// and keeps the failure.
#[test]
fn a_sync_with_a_run_open_pays_the_log_directorys_barrier() {
    let (d, mut db) = rotated_loose("srdir");
    db.seal().unwrap();
    sr_commits(&mut db, 0, 300, 100);
    assert!(
        db.run_synced_bytes().is_some(),
        "a run opened above the sealed table"
    );
    db.fail_next_wal_dir_sync();
    match db.sync() {
        Ok(()) => panic!("a sync with a run open answered Ok over the directory's barrier"),
        Err(e) => assert!(
            e.to_string().contains("injected"),
            "the sync answered with an error that is not the barrier's: {e}"
        ),
    }
    kept(db.sync(), "the sync after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------
// seal-sync: a segment whose sync failed -- its data's, its directory
// entry's at the readable landing, the presync of a closing run's records
// -- is named durable by no manifest, its sequence is covered by none,
// and the log that holds its writes is retired by nothing, whichever path
// would write the next manifest: a merge's landing, a later seal's, a
// drain's, the close. The seal queue stops (`Db::seals_wedged`), readers
// go on reading what was published, and every `flush` answers the failure
// until the store is reopened; a `sync`, which promises nothing of a
// seal's writes, answers nothing of it.
// ---------------------------------------------------------------------

/// The words every `flush` of a store whose seal failed to make its
/// segments durable answers with, after the call that met the failure,
/// before the failure's own.
const SS_WEDGED: &str = "a seal failed to make its segments durable";

type SsModel = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

/// A slot as `ss_newest` reads it: its publish sequence, the log sequence
/// it covers, and its names with their pending flags.
type SsSlot = (u64, u64, Vec<(String, bool)>);

/// What the newest valid slot says: its publish sequence, the log
/// sequence it covers, and the names it calls live with their pending
/// flags. Version 3: an eight-byte magic, the version byte, the sequence,
/// the body's length and its CRC, then the covered sequence, a count, and
/// per name a u16 length, the bytes and a flag byte. A slot that does not
/// parse is passed over, so a slot read while the engine writes it is
/// not mistaken for one; `None` where no slot parses.
fn ss_newest(d: &Path) -> Option<SsSlot> {
    let mut best: Option<SsSlot> = None;
    for slot in ["manifest", "manifest.alt"] {
        let Ok(b) = std::fs::read(d.join(slot)) else {
            continue;
        };
        if b.len() < 37 || &b[..8] != b"SUPDBMAN" || b[8] != 3 {
            continue;
        }
        let seq = u64::from_le_bytes(b[9..17].try_into().unwrap());
        let body = &b[25..];
        let covered = u64::from_le_bytes(body[..8].try_into().unwrap());
        let n = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
        let mut p = 12;
        let mut names = Vec::new();
        let mut whole = true;
        for _ in 0..n {
            let Some(l) = body.get(p..p + 2) else {
                whole = false;
                break;
            };
            let l = u16::from_le_bytes(l.try_into().unwrap()) as usize;
            p += 2;
            let (Some(name), Some(flag)) = (body.get(p..p + l), body.get(p + l)) else {
                whole = false;
                break;
            };
            names.push((String::from_utf8_lossy(name).into_owned(), flag & 1 != 0));
            p += l + 1;
        }
        if whole && best.as_ref().is_none_or(|x| seq > x.0) {
            best = Some((seq, covered, names));
        }
    }
    best
}

fn ss_names(d: &Path) -> Vec<String> {
    ss_newest(d)
        .expect("a manifest")
        .2
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

/// The log sequence a seal's segment ends at, from its name:
/// `seg-<id>-<end>.sup`, `par-<id>-<end>-...` or `pcs-<id>-<end>-...`.
fn ss_end_seq(name: &str) -> u64 {
    let field = name.split('-').nth(2).unwrap();
    field.trim_end_matches(".sup").parse().unwrap()
}

fn ss_logs(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wal-"))
        .collect();
    v.sort();
    v
}

/// Every key of `model` read back through `r`, by point read and by one
/// scan of the whole store.
fn ss_check(r: &Reader, model: &SsModel, state: &str) {
    for (k, want) in model {
        assert_eq!(
            &read_vec(r, k),
            want,
            "{state}: an acknowledged write of {} reads back",
            String::from_utf8_lossy(k)
        );
    }
    let mut seen: SsModel = BTreeMap::new();
    r.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| seen.entry(k.to_vec()).or_default().push(v.to_vec())),
    )
    .unwrap();
    assert!(seen == *model, "{state}: the scan disagrees with the model");
}

fn ss_key(k: u32) -> Vec<u8> {
    format!("key-{k:06}").into_bytes()
}

fn ss_between(k: u32, i: u32) -> Vec<u8> {
    format!("key-{k:06}x{i}").into_bytes()
}

fn ss_put(db: &mut Db, model: &mut SsModel, k: &[u8], v: &str) {
    db.append(k, v.as_bytes());
    model
        .entry(k.to_vec())
        .or_default()
        .push(v.as_bytes().to_vec());
}

/// The merge in flight either landed or, finished, is held behind the
/// stopped queue: a stopped queue holds every landing that writes a
/// manifest as the window between a seal's phases does, for good, so a
/// finished merge is counted held at every look of the segment work.
fn ss_merge_landed_or_held(db: &Db) {
    let held = db.seal_waits().held_landings;
    wait_for("the merge's end or its hold", || {
        !db.in_flight().1 || db.seal_waits().held_landings > held + 1
    });
}

/// A `sync` after a seal failed answers `Ok`: what it promises is in the
/// log, which no manifest retires while the queue is stopped, and the
/// commits of a run closed into the seal are durable once that seal lands,
/// which a `sync` never covers (`SyncPolicy`); a `flush` is the call that
/// answers the failure.
fn ss_sync_answers_nothing(r: std::io::Result<()>, what: &str) {
    if let Err(e) = r {
        panic!("{what}: answered the seal's failure, which a sync does not cover: {e}");
    }
}

/// `r` an error carrying the wedge's words and the failure's own (`cause`).
fn ss_wedged(r: std::io::Result<()>, cause: &str, what: &str) {
    match r {
        Ok(()) => panic!("{what}: answered Ok after a seal failed to make its segments durable"),
        Err(e) => {
            let s = e.to_string();
            assert!(
                s.contains(SS_WEDGED) && s.contains(cause),
                "{what}: answered with an error that does not say the seal failed \
                 ({SS_WEDGED:?}) and why ({cause:?}): {s}"
            );
        }
    }
}

/// The commit that answers a failure the segment work met on its own:
/// commits with nothing staged until one answers it, since the work stops
/// the queue before it leaves its error for the writer (`seals_wedged`
/// reads true a moment before a commit can answer).
fn ss_commit_answer(db: &mut Db) -> std::io::Error {
    let t = std::time::Instant::now();
    loop {
        match db.commit() {
            Err(e) => return e,
            Ok(()) => {
                assert!(
                    t.elapsed() < std::time::Duration::from_secs(120),
                    "no commit answered the segment work's failure"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

/// A store of one partition over 400,000 keys and two pieces over keys it
/// holds, so that the second piece's durable landing starts a merge of the
/// range, as `tests/db.rs`'s held-landing tests build it. `before_second`
/// runs before the second piece's seal. Every commit is under
/// `SyncPolicy::Always`, so every write is acknowledged when it returns.
fn ss_store_with_a_merge_due(
    d: &Path,
    base: Options,
    model: &mut SsModel,
    before_second: impl FnOnce(&Db),
) -> (Db, Options) {
    let mut before_second = Some(before_second);
    let opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 20),
        l0_trigger: 2,
        ..base
    };
    let mut db = Db::create(d, opts.clone()).unwrap();
    for k in 0..400_000 {
        ss_put(&mut db, model, &ss_key(k), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    assert_eq!(db.levels(), (1, 0), "one partition over every key");
    for (n, (tag, base)) in [("a", 1000u32), ("b", 2000u32)].into_iter().enumerate() {
        for i in 0..10 {
            ss_put(&mut db, model, &ss_key(base + i), tag);
            ss_put(&mut db, model, &ss_between(base, i), tag);
        }
        db.commit().unwrap();
        if n == 1 {
            (before_second.take().unwrap())(&db);
        }
        db.seal().unwrap();
        wait_for("the seal's landing", || !db.in_flight().0);
    }
    wait_for("the merge's start", || db.in_flight().1);
    (db, opts)
}

/// The third piece's keys, written and committed: acknowledged.
fn ss_third(db: &mut Db, model: &mut SsModel) {
    for i in 0..10 {
        ss_put(db, model, &ss_key(3000 + i), "c");
        ss_put(db, model, &ss_between(3000, i), "c");
    }
    db.commit().unwrap();
}

/// The one segment on disk that was not in `before`.
fn ss_new_piece(d: &Path, before: &[String]) -> String {
    let new: Vec<String> = seg_names(d)
        .into_iter()
        .filter(|n| !before.contains(n))
        .collect();
    assert_eq!(new.len(), 1, "the seal's one segment: {new:?}");
    new.into_iter().next().unwrap()
}

/// The slot an open reads names `piece` nowhere, and covers none of its
/// sequence.
fn ss_named_by_no_manifest(d: &Path, piece: &str, why: &str) {
    let (_, covered, names) = ss_newest(d).expect("a manifest");
    assert!(
        !names.iter().any(|(n, _)| n == piece),
        "a manifest names the segment {piece} whose {why} failed: {names:?}"
    );
    assert!(
        covered < ss_end_seq(piece),
        "a manifest covers sequence {covered}, past the writes of {piece} whose {why} failed"
    );
}

/// A seal whose directory fsync fails at its readable landing has its
/// segments published and its sequence counted, and the seal queue
/// stops; a merge in flight across it, whose outputs were written and
/// their directory fsynced before the seal renamed its piece into place,
/// must not then land a manifest that names the piece or covers its
/// sequence. A power cut may lose the piece's directory entry -- no fsync
/// that succeeded covered it -- and the reopen replays the log that holds
/// the piece's acknowledged writes.
#[test]
fn a_seal_whose_directory_fsync_failed_is_named_by_no_manifest() {
    let d = dir("dur-ss-dirsync");
    let mut model = SsModel::new();
    let (mut db, opts) = ss_store_with_a_merge_due(&d, Options::default(), &mut model, |db| {
        db.hold_merge_landing(true)
    });
    // The merge's outputs written and their directory fsynced, before the
    // third piece exists: no directory fsync after this succeeds.
    wait_for("the merge at its landing hold", || {
        db.merge_waits_at_landing()
    });
    assert_eq!(db.levels(), (1, 2));
    let pieces_before = seg_names(&d);
    let covered_before = ss_newest(&d).unwrap().1;

    db.fail_next_seal_dir_sync();
    ss_third(&mut db, &mut model);
    db.seal().unwrap();
    wait_for("the failed landing", || db.seals_wedged());
    assert!(
        !db.seal_sync_fault_pending(),
        "the seal's directory fsync was failed; the case was reached"
    );
    let third = ss_new_piece(&d, &pieces_before);
    assert!(ss_end_seq(&third) > covered_before, "the premise");

    // The merge lands, or is refused, and the store closes.
    db.hold_merge_landing(false);
    ss_merge_landed_or_held(&db);
    ss_check(&db, &model, "after the failure, before the close");
    // The answers, read once the files are: a store that names the piece
    // is wrong first in what it left on disk.
    let sync_ans = db.sync();
    let answers = [
        ("a flush after the failure", db.flush()),
        ("the flush after that", db.flush()),
    ];
    drop(db);
    ss_named_by_no_manifest(&d, &third, "directory fsync");

    // The power cut: the piece's link, which no successful directory fsync
    // covered, is gone.
    std::fs::remove_file(d.join(&third)).unwrap();
    let db = Db::open(&d, opts).expect("the store opens after the power cut");
    ss_check(&db, &model, "reopened after the power cut");
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(r, "directory fsync", what);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A seal whose step that makes its data durable fails after its readable
/// landing has published segments whose bytes may not be on the device.
/// The store writes no manifest that names them or covers their sequence
/// from there -- the merge held behind the seal lands no slot -- so a
/// power cut that loses the unsynced bytes leaves a store whose log still
/// holds the acknowledged writes. The failure is the seal's own and is
/// not tried again: the queue stops.
#[test]
fn a_seal_whose_data_fsync_failed_after_its_publish_is_named_by_no_manifest() {
    let d = dir("dur-ss-datasync");
    let mut model = SsModel::new();
    let (mut db, opts) = ss_store_with_a_merge_due(&d, Options::default(), &mut model, |_| {});
    assert_eq!(db.levels(), (1, 2));
    let pieces_before = seg_names(&d);
    let covered_before = ss_newest(&d).unwrap().1;

    // The third piece, held between its phases: published, not durable.
    db.hold_seal_durable(true);
    ss_third(&mut db, &mut model);
    db.seal().unwrap();
    wait_for("the third piece's publish", || db.levels() == (1, 3));
    let third = ss_new_piece(&d, &pieces_before);
    assert!(ss_end_seq(&third) > covered_before, "the premise");
    // The merge finished and held behind the seal, so it lands after the
    // seal's failure and not before it.
    wait_for("a merge held for the seal", || {
        db.seal_waits().held_landings > 0
    });

    db.fail_next_seal_sync();
    db.hold_seal_durable(false);
    wait_for("the seal's end", || !db.in_flight().0);
    assert!(
        !db.seal_sync_fault_pending(),
        "the seal's fsync was failed; the case was reached"
    );
    ss_merge_landed_or_held(&db);
    ss_check(&db, &model, "after the failure, before the close");
    let sync_ans = db.sync();
    let answers = [
        ("a flush after the failure", db.flush()),
        ("a settle after the failure", db.settle()),
    ];
    let wedged = db.seals_wedged();
    drop(db);
    ss_named_by_no_manifest(&d, &third, "fsync");

    // The power cut: the piece's data, which no fsync that succeeded
    // covered, is lost whole.
    std::fs::OpenOptions::new()
        .write(true)
        .open(d.join(&third))
        .unwrap()
        .set_len(0)
        .unwrap();
    let db = Db::open(&d, opts).expect("the store opens after the power cut");
    ss_check(&db, &model, "reopened after the power cut");
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(r, "a test failed this seal's fsync", what);
    }
    assert!(
        wedged,
        "a seal that failed after its readable landing stops the queue, \
         and is not tried again"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// The same failure, met by the seal after it rather than by a merge: no
/// later seal lands a manifest naming the failed piece or covering its
/// sequence, and no log file that held its writes is retired -- each file
/// on disk when the failed seal was handed is there after the close.
/// The commit after the failure answers it once in the seal's own words,
/// the commits after it answer `Ok`, durable in the log, and every `flush`
/// answers the failure until the store is reopened, while a `sync`, which
/// promises nothing of a seal's writes, answers nothing of it.
#[test]
fn a_later_seal_names_no_seal_whose_fsync_failed_and_retires_none_of_its_log() {
    let d = dir("dur-ss-later");
    let opts = Options {
        direct_ingest: false,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let value = "v".repeat(400);
    let batch = |db: &mut Db, model: &mut SsModel, from: u32| {
        for k in from..from + 3000 {
            ss_put(db, model, &ss_key(k), &value);
        }
        db.commit().unwrap();
    };
    batch(&mut db, &mut model, 0);
    db.settle().unwrap();
    let before = seg_names(&d);
    assert!(!before.is_empty(), "the first batch sealed");

    db.hold_seal_durable(true);
    batch(&mut db, &mut model, 10_000);
    db.seal().unwrap();
    wait_for("the second seal's publish", || {
        let (p, l0) = db.levels();
        seg_names(&d).len() > before.len() && p + l0 > before.len()
    });
    let failed = ss_new_piece(&d, &before);
    let logs = ss_logs(&d);
    db.fail_next_seal_sync();
    db.hold_seal_durable(false);
    wait_for("the failed seal's end", || !db.in_flight().0);

    // A commit answers the seal's failure once; the one after it takes
    // the batch, durable in the log.
    let cause = "a test failed this seal's fsync";
    let e = ss_commit_answer(&mut db);
    assert!(
        e.to_string().contains(cause),
        "the commit after the failure answers it: {e}"
    );
    for k in 20_000..21_000 {
        ss_put(&mut db, &mut model, &ss_key(k), &value);
    }
    db.commit().unwrap();
    // A seal of that batch is handed and lands nothing.
    db.seal().unwrap();
    let sync_ans = db.sync();
    let answers = [("a flush after the failure", db.flush())];
    ss_check(&db, &model, "after the failure, before the close");
    let wedged = db.seals_wedged();
    drop(db);

    ss_named_by_no_manifest(&d, &failed, "fsync");
    let after = ss_logs(&d);
    for l in &logs {
        assert!(
            after.contains(l),
            "{l}, a log file holding the failed seal's writes, was retired: {after:?}"
        );
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(d.join(&failed))
        .unwrap()
        .set_len(0)
        .unwrap();
    let db = Db::open(&d, opts).expect("the store opens after the power cut");
    ss_check(&db, &model, "reopened after the power cut");
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(r, cause, what);
    }
    assert!(wedged, "the queue stopped");
    let _ = std::fs::remove_dir_all(&d);
}

/// Under a cipher a segment is made durable by its envelope, written by
/// its seal (`vault::persist`), and a manifest writes the envelope of any
/// segment it names that has none: a seal whose envelope failed after its
/// readable landing would be made durable, and named, by the next
/// manifest's own write. No manifest is written from the failure on, so
/// none writes the envelope late.
#[test]
fn a_seal_whose_envelope_failed_under_a_cipher_is_made_durable_by_no_manifest() {
    let d = dir("dur-ss-cipher");
    let mut model = SsModel::new();
    let (mut db, opts) = ss_store_with_a_merge_due(&d, under_cipher(), &mut model, |_| {});
    assert_eq!(db.levels(), (1, 2));
    let late_before = db.cipher_counts().unwrap().late;
    db.hold_seal_durable(true);
    ss_third(&mut db, &mut model);
    db.seal().unwrap();
    wait_for("the third piece's publish", || db.levels() == (1, 3));
    wait_for("a merge held for the seal", || {
        db.seal_waits().held_landings > 0
    });
    db.fail_next_seal_sync();
    db.hold_seal_durable(false);
    wait_for("the seal's end", || !db.in_flight().0);
    assert!(!db.seal_sync_fault_pending(), "the case was reached");
    ss_merge_landed_or_held(&db);
    ss_check(&db, &model, "after the failure, before the close");
    let late = db.cipher_counts().unwrap().late;
    let sync_ans = db.sync();
    let answers = [("a flush after the failure", db.flush())];
    let wedged = db.seals_wedged();
    drop(db);
    assert_eq!(
        late, late_before,
        "a manifest wrote the envelope of a segment whose seal failed to"
    );
    let db = Db::open(&d, opts).expect("the store opens");
    ss_check(&db, &model, "reopened");
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(r, "a test failed this seal's fsync", what);
    }
    assert!(wedged, "the queue stopped");
    let _ = std::fs::remove_dir_all(&d);
}

/// The store's files copied from `from` to `to`, with every whole page of
/// `[from_off, upto)` zeroed in each file whose inode is `ino`: what a power
/// cut leaves of a file whose fdatasync of that range failed and which
/// nothing wrote again. A file the copy cannot read (gone between the
/// listing and the read) is skipped. Returns the files cut.
fn ss_copy_with_lost_pages(from: &Path, to: &Path, ino: u64, from_off: u64, upto: u64) -> usize {
    use std::os::unix::fs::MetadataExt;
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    let mut hit = 0;
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let Ok(md) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        if !md.is_file() {
            continue;
        }
        let Ok(mut bytes) = std::fs::read(e.path()) else {
            continue;
        };
        if md.ino() == ino {
            let lo = from_off.div_ceil(4096) * 4096;
            let hi = (upto / 4096 * 4096).min(bytes.len() as u64);
            if hi > lo {
                bytes[lo as usize..hi as usize].fill(0);
                hit += 1;
            }
        }
        std::fs::write(to.join(e.file_name()), &bytes).unwrap();
    }
    hit
}

fn ss_val(k: u32) -> Vec<u8> {
    format!("value-{k:06}-{}", "x".repeat(100)).into_bytes()
}

/// A run of 3000 keys under `EveryN(1_000_000)`, committed in batches the
/// file takes a page at a time, then a flush whose presync of the run's
/// records fails: the presync is the records' first sync. `inline` takes
/// the arm that syncs on a thread of its own and joins it into the seal
/// job (`Options::close_finish_inline`).
///
/// Off the inline arm the presync runs on the writer's thread after the
/// seal job is handed, and the job's own fsync of the same description
/// reports nothing: the landing that names the segment durable has to
/// wait for the writer's word, and here the writer is held at its presync
/// (`Reader::hold_presync`) until the seal thread has finished and any
/// landing that did not wait would have written its slot. On the inline
/// arm the job fails after its readable landing, which a hold makes come
/// first (`hold_seal_durable`). Either way the flush answers the failure,
/// every `sync` and `flush` after it answers it again, no slot names the
/// run's segment, and a power cut that loses the pages the failed sync
/// covered leaves a store that opens.
///
/// `both` holds the segment work's poll from the seal's readable phase
/// (`Db::hold_readable_landing`) until the seal thread has returned, so
/// one poll meets both phases due: a landing that took the job whole
/// there would name it durable without the writer's word. `pending`
/// writes the slot that names the seal's segments pending at its
/// readable landing (`Options::manifest_pending`), and the close must not
/// name them durable; the power cut is not taken there, the open's rule
/// for a pending name being another unit's, and the store is reopened as
/// the close left it.
fn ss_failed_presync(tag: &str, inline: bool, both: bool, pending: bool) {
    let d = dir(&format!("dur-ss-presync-{tag}"));
    let opts = Options {
        sync: SyncPolicy::EveryN(1_000_000),
        close_finish_inline: inline,
        manifest_pending: pending,
        segment: SegmentOptions {
            // The run's records go to the file at each commit, a page at a
            // time, so the presync has something on the file to cover.
            write_piece: 4096,
            ..Default::default()
        },
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..3000u32 {
        db.append(&ss_key(k), &ss_val(k));
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    db.fail_next_presync();
    let h = db.reader().unwrap();
    if inline {
        db.hold_seal_durable(true);
    } else {
        db.hold_presync(true);
    }
    if both {
        db.hold_readable_landing(true);
    }
    let dd = d.clone();
    let lifter = std::thread::spawn(move || {
        let t = std::time::Instant::now();
        while !both && h.levels() == (0, 0) {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "the run's segment was never published"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if both {
            // The seal thread's readable signal, its fsyncs and its
            // return given their time with the poll held, then the poll
            // let go with the writer still held at its presync: the look
            // that meets both phases due lands the readable one, and a
            // landing that took the job whole there writes a slot that
            // ends the second wait early.
            std::thread::sleep(std::time::Duration::from_millis(500));
            h.hold_readable_landing_from_handle(false);
            let t = std::time::Instant::now();
            while h.levels() == (0, 0) {
                assert!(
                    t.elapsed() < std::time::Duration::from_secs(20),
                    "the run's segment was never published"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_secs(2)
                && ss_newest(&dd).is_none_or(|(_, _, n)| n.is_empty())
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            h.hold_presync(false);
        } else if inline {
            h.hold_seal_durable_from_handle(false);
        } else {
            // The seal thread's finish and fsyncs given their time, and a
            // landing that did not wait for the writer meanwhile: a slot
            // that names the segment ends the wait early.
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_secs(2)
                && ss_newest(&dd).is_none_or(|(_, _, n)| n.is_empty())
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            h.hold_presync(false);
        }
    });
    match db.flush() {
        Ok(()) => panic!("{tag}: the flush answered Ok over its records' failed sync"),
        Err(e) => assert!(
            e.to_string()
                .contains("the records' sync failed (injected)"),
            "{tag}: the flush answers the records' failed sync: {e}"
        ),
    }
    lifter.join().unwrap();
    let (ino, upto) = db
        .failed_presync_extent()
        .expect("the flush's close ran the injected presync");
    assert!(
        upto >= 16 * 4096,
        "{tag}: premise: the run's records were on its file when the presync failed, {upto} bytes"
    );
    let answers = [("the flush after", db.flush())];
    let sync_ans = db.sync();
    for k in [0u32, 1499, 2999] {
        assert_eq!(
            read_vec(&db, &ss_key(k)),
            vec![ss_val(k)],
            "{tag}: reads go on"
        );
    }
    let settle_ans = db.settle();
    drop(db);
    let names = ss_newest(&d).expect("a manifest").2;
    assert!(
        names.iter().all(|(_, p)| pending && *p),
        "{tag}: a slot names the run's segment durable, whose records' sync failed: {names:?}"
    );
    ss_wedged(
        settle_ans,
        "the records' sync failed (injected)",
        &format!("{tag}: a settle after the failure"),
    );
    if pending {
        let db = Db::open(&d, opts)
            .unwrap_or_else(|e| panic!("{tag}: the store refused to open after its close: {e}"));
        for k in [0u32, 1499, 2999] {
            assert_eq!(
                read_vec(&db, &ss_key(k)),
                vec![ss_val(k)],
                "{tag}: reopened"
            );
        }
        drop(db);
        ss_sync_answers_nothing(sync_ans, "a sync after the failure");
        for (what, r) in answers {
            ss_wedged(
                r,
                "the records' sync failed (injected)",
                &format!("{tag}: {what}"),
            );
        }
        let _ = std::fs::remove_dir_all(&d);
        return;
    }
    let cut = d.with_extension("powercut");
    // The run's records start after the head the finish patches
    // (superblock and section header, 0..8192 under `write_piece` 4096);
    // the pages from there to what the file held at the failure were
    // covered by the failed sync and by nothing after it.
    let hit = ss_copy_with_lost_pages(&d, &cut, ino, 8192, upto);
    assert!(hit > 0, "{tag}: the run's file is in the store");
    let db = Db::open(&cut, opts)
        .unwrap_or_else(|e| panic!("{tag}: the store refused to open after a power cut: {e}"));
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(
            r,
            "the records' sync failed (injected)",
            &format!("{tag}: {what}"),
        );
    }
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&cut);
}

#[test]
fn a_flush_after_a_failed_presync_is_not_ok_over_the_lost_records() {
    ss_failed_presync("writer", false, false, false);
}

/// The same, with the seal thread's two phases met by one poll of the
/// segment work, and with the slot that names the seal's segments pending.
#[test]
fn a_flush_after_a_failed_presync_is_not_ok_whichever_way_its_seal_lands() {
    ss_failed_presync("both", false, true, false);
    ss_failed_presync("pending", false, false, true);
}

#[test]
fn a_flush_after_a_failed_inline_presync_is_not_ok_over_the_lost_records() {
    ss_failed_presync("inline", true, false, false);
}

/// A direct run closed -- by `Db::seal`, or by a flush on either arm of
/// its finish -- whose seal fails at the step that makes its segment
/// durable, after its readable landing: under `Always` every commit of the
/// run synced its records, so the reopen recovers them from the run's
/// temp name, which nothing retires; and no later landing names the
/// run's segment, whose trailer no fsync covered.
fn ss_run_close_fails(tag: &str, by_flush: bool, inline: bool) {
    let d = dir(&format!("dur-ss-runclose-{tag}"));
    let crash = dir(&format!("dur-ss-runclose-{tag}-crash"));
    let opts = Options {
        close_finish_inline: inline,
        ..Options::default()
    };
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..3000u32 {
        ss_put(&mut db, &mut model, &ss_key(k + 100_000), "r");
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    let ino = run_ino(&d);
    let synced = db.run_synced_bytes().expect("a run is open");
    db.fail_next_seal_sync();
    db.hold_seal_durable(true);
    let cause = "a test failed this seal's fsync";
    if by_flush {
        let h = db.reader().unwrap();
        let lifter = std::thread::spawn(move || {
            let t = std::time::Instant::now();
            while h.levels() == (0, 0) {
                assert!(
                    t.elapsed() < std::time::Duration::from_secs(20),
                    "the run's segment was never published"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            h.hold_seal_durable_from_handle(false);
        });
        match db.flush() {
            Ok(()) => panic!("{tag}: the flush answered Ok over its seal's failure"),
            Err(e) => assert!(
                e.to_string().contains(cause),
                "{tag}: the flush answers its seal's failure: {e}"
            ),
        }
        lifter.join().unwrap();
    } else {
        db.seal().unwrap();
        wait_for("the run's segment published", || db.levels() != (0, 0));
        db.hold_seal_durable(false);
        wait_for("the failed seal's end", || !db.in_flight().0);
    }
    assert!(!db.seal_sync_fault_pending(), "{tag}: the case was reached");
    // A write below the run's keys, through the log, acknowledged: the
    // segment work's error, left for the writer where the work met the
    // failure on its own, answered by a commit first.
    if !by_flush {
        let e = ss_commit_answer(&mut db);
        assert!(
            e.to_string().contains(cause),
            "{tag}: a commit answers the seal's failure: {e}"
        );
    }
    ss_put(&mut db, &mut model, &ss_key(7), "low");
    if db.commit().is_err() {
        assert!(
            by_flush,
            "{tag}: a second commit answered the seal's failure"
        );
        db.commit().unwrap();
    }
    let answers = [("a flush after", db.flush())];
    let sync_ans = db.sync();
    ss_check(&db, &model, &format!("{tag}: after the failure"));
    let wedged = db.seals_wedged();
    drop(db);
    let names = ss_names(&d);
    assert!(
        names.is_empty(),
        "{tag}: a slot names a segment, the run's whose seal failed among them: {names:?}"
    );
    // The power cut: the run's file cut to what its commits' fdatasyncs
    // covered, the trailer the failed seal wrote gone.
    copy_store(&d, &crash);
    use std::os::unix::fs::MetadataExt;
    for e in std::fs::read_dir(&d).unwrap() {
        let e = e.unwrap();
        if e.metadata().map(|m| m.ino()).ok() == Some(ino) {
            std::fs::OpenOptions::new()
                .write(true)
                .open(crash.join(e.file_name()))
                .unwrap()
                .set_len(synced)
                .unwrap();
        }
    }
    let db = Db::open(&crash, opts)
        .unwrap_or_else(|e| panic!("{tag}: the store refused to open after a power cut: {e}"));
    ss_check(&db, &model, &format!("{tag}: reopened after a power cut"));
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(r, cause, &format!("{tag}: {what}"));
    }
    assert!(wedged, "{tag}: the queue stopped");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_runs_close_whose_fsync_failed_is_named_by_no_manifest() {
    ss_run_close_fails("seal", false, false);
    ss_run_close_fails("flush", true, false);
    ss_run_close_fails("flushinline", true, true);
}

/// Under `Options::manifest_pending` the slot naming a seal's segments
/// pending is written at its readable landing, and the close writes a
/// slot naming them durable. A seal whose fsync failed after its readable
/// landing is not durable, and the pending slot is the only truth about
/// it: the close (the drop, which `Db::close` also ends in) writes no slot
/// naming it durable, since that slot's covered sequence skips the log
/// frames that are the commits' only durable copy. With the segment work
/// on its own thread the failure may be landed before the close or joined
/// by it; `inline` runs the segment work on the writer's thread, where
/// nothing lands it but the close's own join (`Maint::stop`).
fn ss_pending_close(tag: &str, inline: bool) {
    let d = dir(&format!("dur-ss-pending-close-{tag}"));
    let opts = Options {
        seal_bytes: 1 << 20,
        manifest_pending: true,
        publish_in_background: !inline,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // High to low, so the seal is the memtable's and the log holds them.
    let mut model = SsModel::new();
    for k in (0..3000u32).rev() {
        ss_put(&mut db, &mut model, &ss_key(k), "v");
    }
    db.commit().unwrap();
    // The seal held between its phases: its segments renamed into place
    // and published, the pending slot naming them written, the fsync not
    // paid. Inline, a commit is what lands the readable phase.
    db.hold_seal_durable(true);
    db.seal().unwrap();
    wait_for("the piece's publish and its pending slot", || {
        if inline {
            db.commit().unwrap();
        }
        db.levels().1 == 1 && ss_newest(&d).is_some_and(|(_, _, n)| n.iter().any(|(_, p)| *p))
    });
    let names = ss_newest(&d).unwrap().2;
    assert_eq!(
        names.len(),
        1,
        "{tag}: the slot names the seal's piece: {names:?}"
    );
    let piece = names[0].0.clone();
    // The fsync the hold keeps from starting fails once the drop lifts the
    // hold, and the close writes its slot after.
    db.fail_next_seal_sync();
    drop(db);
    let names = ss_newest(&d).unwrap().2;
    assert!(
        !names.iter().any(|(n, p)| n == &piece && !p),
        "{tag}: the close named durable a segment whose fsync failed: {names:?}"
    );
    // The power cut: the segment's writeback failed, and its bytes on the
    // device are whatever the failure left.
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(d.join(&piece))
            .unwrap();
        f.write_all_at(&[0u8; 4096], 8192).unwrap();
    }
    let db = Db::open(&d, opts)
        .unwrap_or_else(|e| panic!("{tag}: the store refused its acknowledged commits: {e}"));
    ss_check(&db, &model, &format!("{tag}: reopened after a power cut"));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_seal_whose_fsync_failed_is_not_named_durable_by_the_close() {
    ss_pending_close("thread", false);
    ss_pending_close("inline", true);
}

/// A run's fdatasync whose failure is kept (`Db::lost`), then a write the
/// run cannot take: the run closes into a seal that refuses to finish the
/// file and stops the queue. Nothing panics, every call that answers the
/// caller says the device's error, reads go on, and a reopen after a kill
/// reads every commit acknowledged before the failure.
#[test]
fn a_run_closed_after_its_kept_sync_failure_answers_the_device_and_reads_on() {
    let d = dir("dur-ss-runlost");
    let crash = dir("dur-ss-runlost-crash");
    let opts = Options::default();
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..900u32 {
        ss_put(&mut db, &mut model, &ss_key(k + 100_000), "r");
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    db.fail_next_run_sync();
    // The batch whose sync fails: visible from its commit on, as a run's
    // batch is before its sync, and on the file a kill keeps.
    for k in 900..1200u32 {
        ss_put(&mut db, &mut model, &ss_key(k + 100_000), "r");
    }
    let device = "injected";
    match db.commit() {
        Ok(()) => panic!("the commit answered Ok over the run's failed fdatasync"),
        Err(e) => assert!(e.to_string().contains(device), "the device's error: {e}"),
    }
    // Below the run's keys: the run closes.
    db.append(&ss_key(7), b"low");
    for (what, r) in [
        ("a commit", db.commit()),
        ("a sync", db.sync()),
        ("a flush", db.flush()),
        ("a seal", db.seal()),
    ] {
        match r {
            Ok(()) => panic!("{what} after the run's failed fdatasync answered Ok"),
            Err(e) => assert!(
                e.to_string().contains(device),
                "{what} answers with the device's error: {e}"
            ),
        }
    }
    // Through a handle, which reads what was committed: the writer's own
    // reads see the write staged after the failure too.
    let h = db.reader().unwrap();
    ss_check(&h, &model, "after the failure");
    drop(h);
    copy_store(&d, &crash);
    drop(db);
    let db = Db::open(&crash, opts).expect("the store opens after a kill");
    ss_check(&db, &model, "reopened after a kill");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A run's close that cannot be made -- the seal queue stopped by a seal
/// whose landing failed and the frozen tables full behind it, so the
/// run's table has nowhere to go -- leaves the run open. A write the run
/// cannot take is dropped and kept as the store's failure, so nothing
/// after it is acknowledged; with `seal_first`, `Db::seal` answers the
/// failure first and the run takes its next commit as before. Nothing
/// panics, every call that answers says why, and reads go on.
fn ss_no_room_for_the_run(tag: &str, seal_first: bool) {
    let d = dir(&format!("dur-ss-noroom-{tag}"));
    let crash = dir(&format!("dur-ss-noroom-{tag}-crash"));
    let opts = Options {
        seal_bytes: 64 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // A hashed table, whose seal's landing fails.
    for k in (0..300u32).rev() {
        ss_put(&mut db, &mut model, &ss_key(k), "h");
    }
    db.commit().unwrap();
    db.fail_next_landing();
    db.seal().unwrap();
    wait_for("the failed landing", || db.seals_wedged());
    let cause = "a test failed this landing";
    // The frozen tables filled behind it: each seal is handed, lands
    // nothing, and keeps its table frozen.
    let mut first = true;
    // The failed landing's table stays frozen: two more fill the list.
    for t in 0..2u32 {
        for k in (0..100u32).rev() {
            ss_put(&mut db, &mut model, &ss_key(1000 * (t + 1) + k), "f");
        }
        if db.commit().is_err() {
            assert!(
                first,
                "{tag}: only the first commit answers the segment work's error"
            );
            db.commit().unwrap();
        }
        first = false;
        db.seal().unwrap();
    }
    // A run above every key, committed and acknowledged.
    for k in 0..300u32 {
        ss_put(&mut db, &mut model, &ss_key(100_000 + k), "r");
    }
    db.commit().unwrap();
    assert!(db.run_synced_bytes().is_some(), "{tag}: a run is open");
    if seal_first {
        // Its close has nowhere to go: the seal answers, the run stays.
        ss_wedged(db.seal(), cause, &format!("{tag}: a seal of the run"));
        assert!(db.run_synced_bytes().is_some(), "{tag}: the run stays open");
        for k in 300..600u32 {
            ss_put(&mut db, &mut model, &ss_key(100_000 + k), "r");
        }
        db.commit().unwrap();
    }
    // A write the run cannot take: dropped, and the failure kept.
    db.append(&ss_key(7), b"low");
    for (what, r) in [
        ("a commit", db.commit()),
        ("a sync", db.sync()),
        ("a flush", db.flush()),
    ] {
        match r {
            Ok(()) => panic!("{tag}: {what} after a write the store dropped answered Ok"),
            Err(e) => assert!(
                e.to_string().contains(cause),
                "{tag}: {what} answers with the failure: {e}"
            ),
        }
    }
    db.append(&ss_key(100_000 + 600), b"r");
    assert!(
        db.commit().is_err(),
        "{tag}: nothing after the dropped write is acknowledged"
    );
    // Through a handle, which reads what was committed: the writer's own
    // reads see the write staged after the failure too.
    let h = db.reader().unwrap();
    ss_check(&h, &model, &format!("{tag}: after the failure"));
    drop(h);
    copy_store(&d, &crash);
    drop(db);
    let db = Db::open(&crash, opts).expect("the store opens after a kill");
    ss_check(&db, &model, &format!("{tag}: reopened after a kill"));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_write_that_ends_a_run_whose_close_cannot_be_made_is_answered_and_panics_nothing() {
    ss_no_room_for_the_run("write", false);
}

#[test]
fn a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open() {
    ss_no_room_for_the_run("seal", true);
}

/// The frozen list full of seals held before their landing -- ordinary
/// backpressure, the queue alive -- and the segment work's own error
/// waiting for the writer, as a merge's failure leaves it: a write that
/// ends an ordered run waits for room as any close does, and is taken.
/// The error is the next commit's answer, once, and the store goes on;
/// nothing is kept as lost.
#[test]
fn a_write_that_ends_a_run_beside_a_segment_work_error_waits_for_room_and_is_taken() {
    let d = dir("dur-ss-roomerr");
    let crash = dir("dur-ss-roomerr-crash");
    let opts = Options {
        seal_bytes: 64 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let cause = "a test's error of the segment work";
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.hold_seal_landing(true);
    for t in 0..3u32 {
        for k in (0..100u32).rev() {
            ss_put(&mut db, &mut model, &ss_key(1000 * (t + 1) + k), "f");
        }
        db.commit().unwrap();
        db.seal().unwrap();
    }
    assert_eq!(
        db.frozen_tables(),
        3,
        "the frozen list is full, its seals held"
    );
    for k in 0..300u32 {
        ss_put(&mut db, &mut model, &ss_key(100_000 + k), "r");
    }
    db.commit().unwrap();
    assert!(db.run_synced_bytes().is_some(), "a run is open");
    db.put_segment_work_err(cause);
    // Let go once the writer, waiting for room, has met the error.
    let release = db.release_seal_landing_once_taken();
    ss_put(&mut db, &mut model, &ss_key(7), "low");
    release.join().unwrap();
    assert!(!db.seals_wedged(), "no seal failed");
    assert!(db.run_synced_bytes().is_none(), "the run closed");
    match db.commit() {
        Ok(()) => panic!("the commit after the error answered Ok"),
        Err(e) => assert!(
            e.to_string().contains(cause),
            "the next commit answers the segment work's error: {e}"
        ),
    }
    db.commit()
        .expect("the commit after that writes the batch and answers Ok");
    ss_put(&mut db, &mut model, &ss_key(8), "after");
    db.commit().expect("and the commits after it");
    db.sync().expect("a sync after it");
    db.flush().expect("a flush after it");
    let h = db.reader().unwrap();
    ss_check(&h, &model, "after the error");
    drop(h);
    copy_store(&d, &crash);
    drop(db);
    let db = Db::open(&crash, opts).expect("the store opens after a kill");
    ss_check(&db, &model, "reopened after a kill");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A store whose seal queue stopped and whose `sync` then handed its live
/// table to a seal without a freeze (`Options::adaptive_shape`): the next
/// write replaces the handed table first. With room in the frozen list
/// the table is frozen as any table is and the write is taken, durable in
/// the log; with none, and none to come, the write is dropped and every
/// call after it answers the failure until the store is reopened.
fn ss_handed_over_a_stopped_queue(tag: &str, full: bool) {
    let d = dir(&format!("dur-ss-handed-{tag}"));
    let crash = dir(&format!("dur-ss-handed-{tag}-crash"));
    let opts = Options {
        seal_bytes: 64 << 20,
        l0_trigger: 1000,
        adaptive_shape: true,
        ..Options::default()
    };
    let cause = "a test failed this landing";
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in (0..300u32).rev() {
        ss_put(&mut db, &mut model, &ss_key(k), "h");
    }
    db.commit().unwrap();
    db.fail_next_landing();
    db.seal().unwrap();
    wait_for("the failed landing", || db.seals_wedged());
    let mut first = true;
    for t in 0..if full { 2u32 } else { 0 } {
        for k in (0..100u32).rev() {
            ss_put(&mut db, &mut model, &ss_key(1000 * (t + 1) + k), "f");
        }
        if db.commit().is_err() {
            assert!(
                first,
                "{tag}: only the first commit answers the segment work's error"
            );
            db.commit().unwrap();
        }
        first = false;
        db.seal().unwrap();
    }
    assert_eq!(
        db.frozen_tables() == 3,
        full,
        "{tag}: the premise, the list full or not"
    );
    // A hashed table, below every key, synced and handed to a seal.
    for k in (0..50u32).rev() {
        ss_put(&mut db, &mut model, &ss_key(500 + k), "s");
    }
    if db.commit().is_err() {
        assert!(
            first,
            "{tag}: only the first commit answers the segment work's error"
        );
        db.commit().unwrap();
    }
    db.sync()
        .unwrap_or_else(|e| panic!("{tag}: the sync that hands the table: {e}"));
    assert!(db.table_handed(), "{tag}: the sync handed the live table");
    db.append(&ss_key(7), b"next");
    if full {
        // Dropped, not written into the handed table: not even the
        // writer's own reads, which honour no watermark, see it.
        assert_eq!(
            read_vec(&db, &ss_key(7)),
            model[&ss_key(7)],
            "{tag}: the dropped write is in no table"
        );
        for (what, r) in [
            ("a commit", db.commit()),
            ("a sync", db.sync()),
            ("a flush", db.flush()),
            ("a commit after", db.commit()),
        ] {
            ss_wedged(
                r,
                cause,
                &format!("{tag}: {what} after a write the store dropped"),
            );
        }
    } else {
        model.entry(ss_key(7)).or_default().push(b"next".to_vec());
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the write after the hand-off is taken: {e}"));
        ss_put(&mut db, &mut model, &ss_key(8), "after");
        db.commit().unwrap();
        db.sync().unwrap();
        ss_wedged(db.flush(), cause, &format!("{tag}: a flush"));
    }
    let h = db.reader().unwrap();
    ss_check(&h, &model, &format!("{tag}: after the failure"));
    drop(h);
    copy_store(&d, &crash);
    drop(db);
    let db = Db::open(&crash, opts).expect("the store opens after a kill");
    ss_check(&db, &model, &format!("{tag}: reopened after a kill"));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_write_after_a_hand_off_over_a_stopped_queue_with_room_is_taken() {
    ss_handed_over_a_stopped_queue("room", false);
}

#[test]
fn a_write_after_a_hand_off_over_a_stopped_queue_without_room_is_answered_until_reopen() {
    ss_handed_over_a_stopped_queue("full", true);
}

/// A store whose seal queue stopped, with room in the frozen list: a
/// write that ends an ordered run is taken and the run closes, its
/// segment one more that never lands. Inline the wait for room is the
/// queue's own join, which answers the stopped queue every time it is
/// asked, so the close goes on where there is room. The next commit
/// answers the failure once and the commits after it are durable in the
/// log; a `sync` answers nothing of it, a `flush` answers it every time.
#[test]
fn a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken() {
    for background in [false, true] {
        let tag = if background { "background" } else { "inline" };
        let d = dir(&format!("dur-ss-wedgeroom-{tag}"));
        let crash = dir(&format!("dur-ss-wedgeroom-{tag}-crash"));
        let opts = Options {
            seal_bytes: 64 << 20,
            l0_trigger: 1000,
            publish_in_background: background,
            ..Options::default()
        };
        let cause = "a test failed this landing";
        let mut model = SsModel::new();
        let mut db = Db::create(&d, opts.clone()).unwrap();
        for k in (0..300u32).rev() {
            ss_put(&mut db, &mut model, &ss_key(k), "h");
        }
        db.commit().unwrap();
        db.fail_next_landing();
        db.seal().unwrap();
        if background {
            wait_for("the failed landing", || db.seals_wedged());
        } else {
            assert!(db.settle().is_err(), "{tag}: the settle meets the failure");
        }
        assert!(db.seals_wedged(), "{tag}: the queue stopped");
        assert!(db.frozen_tables() < 3, "{tag}: the frozen list has room");
        for k in 0..300u32 {
            ss_put(&mut db, &mut model, &ss_key(100_000 + k), "r");
        }
        if db.commit().is_err() {
            assert!(
                background,
                "{tag}: only the segment work's error is answered"
            );
            db.commit().unwrap();
        }
        assert!(db.run_synced_bytes().is_some(), "{tag}: a run is open");
        ss_put(&mut db, &mut model, &ss_key(7), "low");
        assert!(db.run_synced_bytes().is_none(), "{tag}: the run closed");
        if !background {
            ss_wedged(db.commit(), cause, &format!("{tag}: the next commit"));
        }
        db.commit()
            .unwrap_or_else(|e| panic!("{tag}: the commit after that answers Ok: {e}"));
        ss_put(&mut db, &mut model, &ss_key(8), "after");
        db.commit().unwrap();
        db.sync()
            .unwrap_or_else(|e| panic!("{tag}: a sync promises nothing of the seal: {e}"));
        ss_wedged(db.flush(), cause, &format!("{tag}: a flush"));
        ss_wedged(db.flush(), cause, &format!("{tag}: the flush after"));
        let h = db.reader().unwrap();
        ss_check(&h, &model, &format!("{tag}: after the failure"));
        drop(h);
        copy_store(&d, &crash);
        drop(db);
        let db = Db::open(&crash, opts).expect("the store opens after a kill");
        ss_check(&db, &model, &format!("{tag}: reopened after a kill"));
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&crash);
    }
}

/// A watchdog for a test whose writer would wait on a seal or a merge the
/// test holds: past `secs` it lifts the holds from a handle, so the
/// writer's call returns and the test fails at its own assertion rather
/// than hanging. `done` ends it.
struct SsWatchdog {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SsWatchdog {
    fn new(db: &Db, secs: u64) -> SsWatchdog {
        let h = db.reader().unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = done.clone();
        let thread = std::thread::spawn(move || {
            let t = std::time::Instant::now();
            while !d.load(std::sync::atomic::Ordering::Acquire) {
                if t.elapsed() > std::time::Duration::from_secs(secs) {
                    h.lift_landing_holds_from_handle();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        SsWatchdog {
            done,
            thread: Some(thread),
        }
    }

    fn end(mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

/// A run's close whose presync fails on the writer while its seal has not
/// yet been published -- the seal thread held before its finish
/// (`Db::hold_seal_landing`) -- stops the queue on the writer's side, and
/// the segment work holds every manifest from there as it does for a
/// failure it met itself: the merge in flight finishes and is held, the
/// newest slot is the one that stood at the failure, and the close writes
/// none after it.
#[test]
fn a_presync_that_fails_before_its_seals_publish_stops_every_manifest() {
    let d = dir("dur-ss-presync-early");
    let mut model = SsModel::new();
    let (mut db, opts) = ss_store_with_a_merge_due(&d, Options::default(), &mut model, |db| {
        db.hold_merge_landing(true)
    });
    wait_for("the merge at its landing hold", || {
        db.merge_waits_at_landing()
    });
    // An ordered run above every key the store holds.
    for k in 500_000..503_000u32 {
        ss_put(&mut db, &mut model, &ss_key(k), "run");
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    assert!(db.run_synced_bytes().is_some(), "premise: a run is open");
    let (seq, covered, names) = ss_newest(&d).expect("a manifest");
    db.hold_seal_landing(true);
    db.fail_next_presync();
    let cause = "the records' sync failed (injected)";
    // A flush that waited for the held seal would wait for good.
    let watchdog = SsWatchdog::new(&db, 60);
    let flushed = db.flush();
    watchdog.end();
    match flushed {
        Ok(()) => panic!("the flush answered Ok over its records' failed sync"),
        Err(e) => assert!(
            e.to_string().contains(cause),
            "the flush answers the records' failed sync: {e}"
        ),
    }
    assert!(db.seals_wedged(), "premise: the presync stopped the queue");
    let held = db.seal_waits().held_landings;
    db.hold_merge_landing(false);
    // Held at every look of the segment work, or landed with a slot.
    wait_for("the merge's hold or its slot", || {
        db.seal_waits().held_landings > held + 1 || ss_newest(&d).is_some_and(|(s, _, _)| s != seq)
    });
    let now = ss_newest(&d).expect("a manifest");
    assert_eq!(
        (now.0, now.1),
        (seq, covered),
        "a manifest was written after the seal queue stopped: {:?} against {names:?}",
        now.2
    );
    assert!(
        db.in_flight().1,
        "the merge landed after the seal queue stopped"
    );
    db.hold_seal_landing(false);
    ss_check(&db, &model, "after the failure");
    let sync_ans = db.sync();
    let answers = [
        ("a flush after the failure", db.flush()),
        ("a settle after the failure", db.settle()),
    ];
    drop(db);
    let now = ss_newest(&d).expect("a manifest");
    assert_eq!(
        (now.0, now.1, &now.2),
        (seq, covered, &names),
        "the close wrote a manifest after the seal queue stopped"
    );
    let db = Db::open(&d, opts).expect("the store opens");
    ss_check(&db, &model, "reopened");
    drop(db);
    ss_sync_answers_nothing(sync_ans, "a sync after the failure");
    for (what, r) in answers {
        ss_wedged(r, cause, what);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The same with the segment work inline (`Options::publish_in_background`
/// off), where the writer lands the merge a flush scheduled
/// (`Options::flush_schedules`) at its next commit: a run's presync that
/// fails on the writer stops the queue before the run's seal has landed
/// even readable, and the commit after the merge has finished holds it as
/// the segment work's own look does -- the slot stands where it stood at
/// the failure, the merge stays in flight, and the close writes no slot
/// after it.
#[test]
fn a_presync_that_fails_before_its_seals_publish_stops_every_manifest_inline() {
    let d = dir("dur-ss-presync-early-inline");
    let opts = Options {
        publish_in_background: false,
        partition_on_flush: false,
        flush_schedules: true,
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 20),
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..400_000 {
        ss_put(&mut db, &mut model, &ss_key(k), "p");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    assert_eq!(db.levels(), (1, 0), "premise: one partition over every key");
    // A piece over keys the partition holds, which a flush cannot promote:
    // the flush schedules a merge of the range and leaves it in flight,
    // held before its landing.
    for i in 0..10 {
        ss_put(&mut db, &mut model, &ss_key(1000 + i), "a");
        ss_put(&mut db, &mut model, &ss_between(1000, i), "a");
    }
    db.commit().unwrap();
    db.hold_merge_landing(true);
    db.flush().unwrap();
    assert!(
        db.in_flight().1,
        "premise: the flush left a merge in flight"
    );
    wait_for("the merge at its landing hold", || {
        db.merge_waits_at_landing()
    });
    // An ordered run above every key the store holds.
    for k in 500_000..503_000u32 {
        ss_put(&mut db, &mut model, &ss_key(k), "run");
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    assert!(db.run_synced_bytes().is_some(), "premise: a run is open");
    let (seq, covered, names) = ss_newest(&d).expect("a manifest");
    db.fail_next_presync();
    let cause = "the records' sync failed (injected)";
    // A flush that joined the held merge would wait for good.
    let watchdog = SsWatchdog::new(&db, 60);
    let flushed = db.flush();
    watchdog.end();
    match flushed {
        Ok(()) => panic!("the flush answered Ok over its records' failed sync"),
        Err(e) => assert!(
            e.to_string().contains(cause),
            "the flush answers the records' failed sync: {e}"
        ),
    }
    assert!(db.seals_wedged(), "premise: the presync stopped the queue");
    // The merge let go and given time to finish: the next commit is the
    // look that would land it.
    db.hold_merge_landing(false);
    wait_for("the merge past its landing hold", || {
        !db.merge_waits_at_landing()
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    ss_put(&mut db, &mut model, &ss_key(9), "after");
    db.commit()
        .unwrap_or_else(|e| panic!("a commit under the threshold answers Ok: {e}"));
    let now = ss_newest(&d).expect("a manifest");
    assert_eq!(
        (now.0, now.1),
        (seq, covered),
        "a manifest was written after the seal queue stopped: {:?} against {names:?}",
        now.2
    );
    assert!(
        db.in_flight().1,
        "the merge landed after the seal queue stopped"
    );
    ss_check(&db, &model, "after the failure");
    ss_sync_answers_nothing(db.sync(), "a sync after the failure");
    ss_wedged(db.flush(), cause, "a flush after the failure");
    ss_wedged(db.settle(), cause, "a settle after the failure");
    drop(db);
    let now = ss_newest(&d).expect("a manifest");
    assert_eq!(
        (now.0, now.1, &now.2),
        (seq, covered, &names),
        "the close wrote a manifest after the seal queue stopped"
    );
    let db = Db::open(&d, opts).expect("the store opens");
    ss_check(&db, &model, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// With the segment work inline (`Options::publish_in_background` off) the
/// writer lands its seals itself, and every call that would land one
/// answers a stopped queue: the settle that meets the failure answers it
/// in the seal's own words, and from then every `Db::seal`, every commit
/// that crosses the seal threshold -- its batch in the log first, as its
/// policy makes it -- every settle and every flush answers it in words
/// that say a seal failed and why, while a commit under the threshold and
/// a `sync` answer `Ok`. A reopen reads every acknowledged commit and the
/// batches the threshold's commits answered with the failure.
#[test]
fn a_stopped_queue_is_answered_by_every_seal_inline() {
    let d = dir("dur-ss-inline-answers");
    let opts = Options {
        direct_ingest: false,
        publish_in_background: false,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut model = SsModel::new();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let value = "v".repeat(400);
    let batch = |db: &mut Db, model: &mut SsModel, from: u32, n: u32| {
        for k in from..from + n {
            ss_put(db, model, &ss_key(k), &value);
        }
        db.commit()
    };
    batch(&mut db, &mut model, 0, 100).unwrap();
    db.fail_next_seal_sync();
    db.seal().unwrap();
    let cause = "a test failed this seal's fsync";
    match db.settle() {
        Ok(()) => panic!("the settle that landed the failed seal answered Ok"),
        Err(e) => assert!(
            e.to_string().contains(cause),
            "the settle answers the seal's failure: {e}"
        ),
    }
    assert!(
        db.seals_wedged(),
        "premise: the seal's failure stopped the queue"
    );
    batch(&mut db, &mut model, 1000, 10).unwrap();
    ss_wedged(db.seal(), cause, "a seal after the failure");
    // A batch past the threshold: its commit writes it to the log and then
    // answers the seal it cannot land.
    ss_wedged(
        batch(&mut db, &mut model, 2000, 3000),
        cause,
        "a commit that crosses the seal threshold",
    );
    ss_wedged(
        batch(&mut db, &mut model, 6000, 10),
        cause,
        "the commit after that, still past the threshold",
    );
    ss_sync_answers_nothing(db.sync(), "a sync after the failure");
    ss_wedged(db.settle(), cause, "a settle after the failure");
    ss_wedged(db.flush(), cause, "a flush after the failure");
    ss_check(&db, &model, "after the failure");
    let crash = d.with_extension("crash");
    copy_store(&d, &crash);
    drop(db);
    let db = Db::open(&crash, opts).expect("the store opens after a kill");
    ss_check(&db, &model, "reopened after a kill");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A second failure after the queue stopped -- a run's presync, failed in
/// a flush after a seal's fsync had failed -- is answered by the flush
/// that met it, and the stopped queue's answer after it is the first
/// failure's: what stopped the queue, and not whatever failed since.
#[test]
fn a_stopped_queue_answers_the_failure_that_stopped_it() {
    let d = dir("dur-ss-first-cause");
    let mut model = SsModel::new();
    let mut db = Db::create(&d, Options::default()).unwrap();
    // High to low, so the seal is the memtable's.
    for k in (0..3000u32).rev() {
        ss_put(&mut db, &mut model, &ss_key(k), "v");
    }
    db.commit().unwrap();
    db.fail_next_seal_sync();
    db.seal().unwrap();
    wait_for("the seal's failure", || db.seals_wedged());
    let first = "a test failed this seal's fsync";
    let e = ss_commit_answer(&mut db);
    assert!(
        e.to_string().contains(first),
        "the commit after the failure answers it: {e}"
    );
    // An ordered run above every key, closed by a flush whose presync
    // fails.
    for k in 10_000..13_000u32 {
        ss_put(&mut db, &mut model, &ss_key(k), "run");
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    assert!(db.run_synced_bytes().is_some(), "premise: a run is open");
    db.fail_next_presync();
    let second = "the records' sync failed (injected)";
    match db.flush() {
        Ok(()) => panic!("the flush answered Ok over its records' failed sync"),
        Err(e) => assert!(
            e.to_string().contains(second),
            "the flush answers the records' failed sync: {e}"
        ),
    }
    assert!(
        db.failed_presync_extent().is_some(),
        "premise: the flush's close ran the injected presync"
    );
    for (what, r) in [
        ("a flush after both", db.flush()),
        ("a settle after both", db.settle()),
    ] {
        let s = r.map(|()| String::new()).unwrap_or_else(|e| e.to_string());
        assert!(
            s.contains(SS_WEDGED) && s.contains(first) && !s.contains(second),
            "{what}: answers the failure that stopped the queue, not the one after it: {s:?}"
        );
    }
    ss_check(&db, &model, "after both failures");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// One look of the segment work that meets a seal's failure and a merge
/// finished beside it -- the seal's readable landing failing at its
/// directory fsync, the poll held until both are due
/// (`Db::hold_readable_landing`) -- holds the merge: the window is read
/// after the seal's landing has stopped the queue, not before it.
#[test]
fn a_merge_finished_beside_a_failing_seal_is_held_in_the_same_look() {
    let d = dir("dur-ss-same-look");
    let mut model = SsModel::new();
    let (mut db, opts) = ss_store_with_a_merge_due(&d, Options::default(), &mut model, |db| {
        db.hold_merge_landing(true)
    });
    wait_for("the merge at its landing hold", || {
        db.merge_waits_at_landing()
    });
    let pieces_before = seg_names(&d);
    let (seq, covered, names) = ss_newest(&d).expect("a manifest");
    db.hold_readable_landing(true);
    db.fail_next_seal_dir_sync();
    ss_third(&mut db, &mut model);
    db.seal().unwrap();
    wait_for("the third piece renamed into place", || {
        seg_names(&d).len() > pieces_before.len()
    });
    let third = ss_new_piece(&d, &pieces_before);
    db.hold_merge_landing(false);
    wait_for("the merge past its hold", || !db.merge_waits_at_landing());
    // The merge thread returns its outputs once let go; given its moment.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let held = db.seal_waits().held_landings;
    db.hold_readable_landing(false);
    wait_for("the seal's failure", || db.seals_wedged());
    let t = std::time::Instant::now();
    while t.elapsed() < std::time::Duration::from_secs(10)
        && db.seal_waits().held_landings <= held
        && ss_newest(&d).is_some_and(|(s, _, _)| s == seq)
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let now = ss_newest(&d).expect("a manifest");
    assert_eq!(
        (now.0, now.1),
        (seq, covered),
        "a manifest was written in the look that met the seal's failure: {:?} against {names:?}",
        now.2
    );
    assert!(
        db.seal_waits().held_landings > held,
        "the merge finished beside the failure was not held"
    );
    let answers = [("a flush after the failure", db.flush())];
    ss_check(&db, &model, "after the failure");
    drop(db);
    ss_named_by_no_manifest(&d, &third, "directory fsync");
    let db = Db::open(&d, opts).expect("the store opens");
    ss_check(&db, &model, "reopened");
    drop(db);
    for (what, r) in answers {
        ss_wedged(r, "directory fsync", what);
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------
// slot-choice: the manifest's two slots are written in place, and a
// publish never writes over the slot that holds the newest manifest known
// durable while no other slot is: not while a drain's slot is owed its
// fdatasync, not while a pending slot's sync is in flight, and not at the
// first publish after an open whose slot a process kill left in the page
// cache alone. Each test watches every slot write the segment work makes
// (`Db::watch_slot_writes`) and, at each, builds what a power cut during
// that write may leave: the store's files as the page cache holds them,
// the slot being written torn, and the other slot as the device holds it
// -- its last write where an fdatasync of that file covered it, otherwise
// what the file held at its last sync, or that torn by the unsynced write.
// Every commit is made under `SyncPolicy::Always` and acknowledged before
// the flush the states are taken in, so every state must open and read
// every key back.
// ---------------------------------------------------------------------

/// The manifest's two slot files, in the index order `SlotWrite::slot` uses.
const OS_SLOTS: [&str; 2] = ["manifest", "manifest.alt"];

/// The publish sequence a slot file's bytes carry, in the clear in both
/// forms -- the plain manifest's header, and under a cipher the sealed
/// slot's, where the tag binds it -- or `None` for an empty or
/// unrecognised one.
fn os_seq_of(bytes: &[u8]) -> Option<u64> {
    let plain = bytes.len() >= 25 && &bytes[..8] == b"SUPDBMAN" && matches!(bytes[8], 2 | 3);
    let sealed = bytes.len() >= 21 && &bytes[..9] == b"SUPDBMAE\x01";
    if !(plain || sealed) {
        return None;
    }
    Some(u64::from_le_bytes(bytes[9..17].try_into().unwrap()))
}

/// The write of `seq` torn over `held`, what the device held: its header's
/// sequence landed and the body did not, which the slot's checksum -- or
/// under a cipher its tag, which covers the sequence -- refuses.
fn os_torn(held: &[u8], seq: u64) -> Vec<u8> {
    let mut t = held.to_vec();
    if t.len() < 25 {
        t = b"SUPDBMAN\x03".to_vec();
        t.extend_from_slice(&seq.to_le_bytes());
        t.extend_from_slice(&[0u8; 8]);
    }
    t[9..17].copy_from_slice(&seq.to_le_bytes());
    t
}

/// Every file of `src` copied into a fresh `dst`, never panicking: it runs
/// on the segment work's thread. A file gone between the listing and the
/// copy is one being renamed or retired, and no slot this test builds
/// names it.
fn os_copy(src: &Path, dst: &Path) {
    let _ = std::fs::remove_dir_all(dst);
    let _ = std::fs::create_dir_all(dst);
    if let Ok(rd) = std::fs::read_dir(src) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let _ = std::fs::copy(e.path(), dst.join(e.file_name()));
            }
        }
    }
}

/// One slot write as the watch saw it.
#[derive(Clone, Copy, Debug)]
struct OsWrite {
    seq: u64,
    slot: usize,
    deferred: bool,
    armed: bool,
    /// Per slot file, the sequence its last fdatasync covered when this
    /// write was about to be made.
    synced: [u64; 2],
}

/// What the watch keeps: per slot file the bytes the device holds, the
/// writes seen, and the crash states built while armed -- while the writer
/// waits in a flush, so every commit of the model was acknowledged before
/// the state's instant.
struct OsTracker {
    store: PathBuf,
    root: PathBuf,
    durable: [Vec<u8>; 2],
    armed: bool,
    writes: Vec<OsWrite>,
    /// A crash state and what it is.
    crashes: Vec<(PathBuf, String)>,
    /// A process kill before a write that followed a deferred one whose
    /// sync was still owed: the page cache as it is, the bytes the device
    /// held for each slot, and the owed sequence the newest slot carries.
    kills: Vec<(PathBuf, [Vec<u8>; 2], u64)>,
    /// The armed writes at which the other slot, as the device holds it,
    /// held no manifest: a tear there leaves the store none.
    bare: Vec<u64>,
}

type OsShared = std::sync::Arc<std::sync::Mutex<OsTracker>>;

impl OsTracker {
    /// Started over a store whose slot files the test has just fsynced, or
    /// whose device bytes it knows (`durable`).
    fn new(store: &Path, root: &Path, durable: [Vec<u8>; 2]) -> OsShared {
        std::fs::create_dir_all(root).unwrap();
        std::sync::Arc::new(std::sync::Mutex::new(OsTracker {
            store: store.to_path_buf(),
            root: root.to_path_buf(),
            durable,
            armed: false,
            writes: Vec::new(),
            crashes: Vec::new(),
            kills: Vec::new(),
            bare: Vec::new(),
        }))
    }

    fn arm(t: &OsShared, on: bool) {
        t.lock().unwrap_or_else(|e| e.into_inner()).armed = on;
    }

    /// Whether, while armed, the window was met: a deferred write of N,
    /// then the write of N+1 while N's fdatasync was still owed.
    /// `deferred` says of the second write whether it was deferred too
    /// (two seals landed by one drain) or synced (a merge's or a
    /// promotion's publish in the same drain). Which slot either went to is
    /// the engine's choice, and not asked.
    fn reached(&self, deferred: bool) -> bool {
        self.writes.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            a.armed
                && b.armed
                && a.deferred
                && b.seq == a.seq + 1
                && b.deferred == deferred
                && b.synced[a.slot] != a.seq
        })
    }

    /// Whether, while armed, a write followed a pending slot (deferred,
    /// its sync on the helper) whose sync had returned: the slot the
    /// helper made durable is the one that write must keep off.
    fn reached_pending(&self) -> bool {
        self.writes.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            a.armed && b.armed && a.deferred && b.seq == a.seq + 1 && b.synced[a.slot] == a.seq
        })
    }

    fn taken(t: OsShared) -> OsTracker {
        std::sync::Arc::try_unwrap(t)
            .ok()
            .expect("the watch released")
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
    }
}

/// The watch: never panics, since it runs on the segment work's thread and
/// a panic there leaves the flush waiting for an answer.
fn os_watcher(t: OsShared) -> supdb::db::SlotWatch {
    std::sync::Arc::new(move |w: &supdb::db::SlotWrite| {
        let mut t = t.lock().unwrap_or_else(|e| e.into_inner());
        let cur: [Vec<u8>; 2] =
            [0, 1].map(|j| std::fs::read(t.store.join(OS_SLOTS[j])).unwrap_or_default());
        // A slot file whose last write an fdatasync covered holds it on
        // the device; otherwise the device holds what it held at the
        // file's last sync, which `durable` already has.
        for (j, c) in cur.iter().enumerate() {
            if let Some(s) = os_seq_of(c) {
                if s != 0 && w.synced[j] == s {
                    t.durable[j] = c.clone();
                }
            }
        }
        let (i, o) = (w.slot, 1 - w.slot);
        let prev = t.writes.last().copied();
        let armed = t.armed;
        t.writes.push(OsWrite {
            seq: w.seq,
            slot: w.slot,
            deferred: w.deferred,
            armed,
            synced: w.synced,
        });
        if !armed {
            return;
        }
        let other_unsynced = cur[o] != t.durable[o];
        if os_seq_of(&t.durable[o]).is_none() {
            t.bare.push(w.seq);
        }
        let n = t.crashes.len();
        // (a) This write torn; the other slot as the device holds it.
        let a = t.root.join(format!("crash-{n}"));
        os_copy(&t.store, &a);
        let _ = std::fs::write(a.join(OS_SLOTS[i]), os_torn(&t.durable[i], w.seq));
        let _ = std::fs::write(a.join(OS_SLOTS[o]), &t.durable[o]);
        let what = format!(
            "publish {} into {} torn{}, {} holding seq {:?} (its last write, seq {:?}, {})",
            w.seq,
            OS_SLOTS[i],
            if w.deferred { " (deferred)" } else { "" },
            OS_SLOTS[o],
            os_seq_of(&t.durable[o]),
            os_seq_of(&cur[o]),
            if other_unsynced {
                "never synced"
            } else {
                "synced"
            },
        );
        t.crashes.push((a, what));
        if other_unsynced {
            // (b) The other slot's unsynced write landed in part as well.
            if let Some(s) = os_seq_of(&cur[o]) {
                let b = t.root.join(format!("crash-{n}-both"));
                os_copy(&t.store, &b);
                let _ = std::fs::write(b.join(OS_SLOTS[i]), os_torn(&t.durable[i], w.seq));
                let _ = std::fs::write(b.join(OS_SLOTS[o]), os_torn(&t.durable[o], s));
                let what = format!(
                    "publish {} torn and the unsynced publish {s} before it torn too",
                    w.seq
                );
                t.crashes.push((b, what));
            }
        }
        // A process kill here, before this write: the deferred write
        // before it in the page cache with its sync still owed, and the
        // device under it as `durable` says.
        if let Some(p) = prev {
            if p.deferred && p.seq + 1 == w.seq && w.synced[p.slot] != p.seq {
                let k = t.root.join(format!("kill-{}", t.kills.len()));
                os_copy(&t.store, &k);
                let d = t.durable.clone();
                t.kills.push((k, d, p.seq));
            }
        }
    })
}

/// The bytes of both slot files made durable by the test's own fsync,
/// whatever the engine synced: from here they are what the device holds.
fn os_sync_slots(d: &Path) -> [Vec<u8>; 2] {
    [0, 1].map(|j| {
        let p = d.join(OS_SLOTS[j]);
        std::fs::File::open(&p).unwrap().sync_all().unwrap();
        std::fs::read(&p).unwrap()
    })
}

/// Small seals and log files, so the rounds below seal, rotate the log,
/// retire closed log files and merge in the background as a larger store
/// does; partitions small enough that a flush's piece merges.
fn os_opts(base: &Options) -> Options {
    Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 10),
        ..base.clone()
    }
}

const OS_KEYS: u32 = 20_000;

fn os_key(i: u32) -> Vec<u8> {
    format!("key-{:06}", (i.wrapping_mul(7919)) % OS_KEYS).into_bytes()
}

/// `count` puts over the key space, committed every 500 under the default
/// policy (`SyncPolicy::Always`): each commit that returned is durable.
fn os_round(db: &mut Db, model: &mut BTreeMap<Vec<u8>, Vec<u8>>, tag: &str, from: u32, count: u32) {
    for i in from..from + count {
        let k = os_key(i);
        let v = format!("{tag}-{i:08}-{}", "x".repeat(80)).into_bytes();
        db.put(&k, &v);
        model.insert(k, v);
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
}

/// Every crash state opened, and every acknowledged key read back with its
/// last acknowledged value. The failures, not the first.
fn os_check_all(
    crashes: &[(PathBuf, String)],
    model: &BTreeMap<Vec<u8>, Vec<u8>>,
    opts: &Options,
) -> Vec<String> {
    let mut failures = Vec::new();
    for (path, what) in crashes {
        match Db::open(path, opts.clone()) {
            Err(e) => failures.push(format!("{what}: the open refused: {e}")),
            Ok(db) => {
                let mut wrong = 0usize;
                let mut first = None;
                for (k, v) in model {
                    let mut got = Vec::new();
                    let _ = db.read_all(k, vf(&mut |x| got.push(x.to_vec())));
                    if got != vec![v.clone()] {
                        wrong += 1;
                        first.get_or_insert_with(|| {
                            format!(
                                "{}: {} value(s), first {:?}",
                                String::from_utf8_lossy(k),
                                got.len(),
                                got.first()
                                    .map(|g| String::from_utf8_lossy(&g[..16.min(g.len())])
                                        .into_owned())
                            )
                        });
                    }
                }
                if wrong > 0 {
                    failures.push(format!(
                        "{what}: {wrong} of {} acknowledged keys wrong, e.g. {}",
                        model.len(),
                        first.unwrap()
                    ));
                }
            }
        }
        let _ = std::fs::remove_dir_all(path);
    }
    failures
}

/// A store loaded, flushed and settled, its slots fsynced by the test, and
/// the watch on: the start of each test below.
fn os_start(name: &str, opts: &Options) -> (PathBuf, Db, BTreeMap<Vec<u8>, Vec<u8>>, OsShared) {
    let d = dir(name);
    let root = dir(&format!("{name}-crashes"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    let t = OsTracker::new(&d, &root, os_sync_slots(&d));
    db.watch_slot_writes(Some(os_watcher(t.clone())));
    (d, db, model, t)
}

/// The crash states the watch built since the last call, checked against
/// what had been acknowledged when they were built: the model as it stands
/// after the flush they were taken in.
fn os_check_built(
    t: &OsShared,
    model: &BTreeMap<Vec<u8>, Vec<u8>>,
    opts: &Options,
) -> (usize, Vec<String>) {
    let built = std::mem::take(&mut t.lock().unwrap_or_else(|e| e.into_inner()).crashes);
    (built.len(), os_check_all(&built, model, opts))
}

/// A flush's seal writes its slot with the fdatasync owed, and a merge or
/// a promotion in the same drain publishes the next slot before that sync:
/// a power cut during that write must leave a store that opens with every
/// acknowledged commit. Four drains, each after the last one's owed sync,
/// so a slot a sync made durable is the one each next drain must keep.
fn a_publish_after_an_owed_slot(tag: &str, base: Options) {
    a_publish_after_a_slot(tag, base, OS_KEYS / 4, |t| t.reached(false));
}

/// Drains, each after a round of `count` puts and each a seal's slot and
/// whatever the drain publishes after it, every slot write's crash states
/// built and opened; `reached` says the window the arm is for was met.
fn a_publish_after_a_slot(tag: &str, base: Options, count: u32, reached: fn(&OsTracker) -> bool) {
    let opts = os_opts(&base);
    let (d, mut db, mut model, t) = os_start(&format!("os-merge-{tag}"), &opts);
    let (mut states, mut failures) = (0, Vec::new());
    for r in 1..=4u32 {
        // Under the default a quarter of the keys rewritten, spread over
        // every partition and under `seal_bytes`, so nothing seals before
        // the flush: the drain's one seal defers its slot, and the piece
        // it leaves is merged or promoted in the same drain
        // (`partition_on_flush`).
        os_round(&mut db, &mut model, &format!("r{r}"), r * OS_KEYS, count);
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        let (n, f) = os_check_built(&t, &model, &opts);
        states += n;
        failures.extend(f);
    }
    db.watch_slot_writes(None);
    drop(db);
    let t = OsTracker::taken(t);
    assert!(
        reached(&t),
        "{tag}: the window was not met; the case was not reached: {:?}",
        t.writes
    );
    assert!(states > 0, "{tag}: no crash state was built");
    assert!(
        failures.is_empty(),
        "{tag}: {} of {states} crash states lost acknowledged commits:\n{}\nslot writes: {:?}",
        failures.len(),
        failures.join("\n"),
        t.writes
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&t.root);
}

#[test]
fn a_publish_after_a_drains_owed_slot_never_overwrites_the_last_durable_slot() {
    a_publish_after_an_owed_slot("plain", Options::default());
}

/// The same under a cipher, whose slots are sealed and written through the
/// vault's door.
#[test]
fn a_publish_after_a_drains_owed_slot_never_overwrites_the_last_durable_slot_under_a_cipher() {
    a_publish_after_an_owed_slot("cipher", under_cipher());
}

/// The arms that write the slots another way: a drain that waits for its
/// slot's fdatasync (`flush_waits_manifest`), and a seal's slot written
/// pending at its readable landing and synced by the helper beside its
/// fsync (`manifest_pending`), after which the slot the helper made
/// durable is the one the next publish keeps off. And the drain that
/// fsyncs the log first (`drain_syncs_wal`), which changes nothing here.
#[test]
fn every_arm_that_writes_a_slot_keeps_the_last_durable_one() {
    a_publish_after_a_slot(
        "waits",
        Options {
            flush_waits_manifest: true,
            ..Options::default()
        },
        OS_KEYS / 4,
        |t| t.writes.iter().any(|w| w.armed && !w.deferred),
    );
    a_publish_after_a_slot(
        "pending",
        Options {
            manifest_pending: true,
            ..Options::default()
        },
        OS_KEYS,
        OsTracker::reached_pending,
    );
    a_publish_after_a_slot(
        "pending-cipher",
        Options {
            manifest_pending: true,
            ..under_cipher()
        },
        OS_KEYS,
        OsTracker::reached_pending,
    );
    a_publish_after_a_slot(
        "inline",
        Options {
            publish_in_background: false,
            ..Options::default()
        },
        OS_KEYS / 4,
        |t| t.reached(false),
    );
    a_publish_after_a_slot(
        "walsync",
        Options {
            drain_syncs_wal: true,
            ..Options::default()
        },
        OS_KEYS / 4,
        |t| t.reached(false),
    );
}

/// Under the rename shape (`manifest_slots` off, with and without the
/// spare) nothing is written in place: every publish renames, and a store
/// moved between the shapes opens to every acknowledged write, the slot an
/// open under the slots takes being one a rename left synced.
#[test]
fn a_store_moved_between_the_manifest_shapes_keeps_its_writes() {
    for spare in [false, true] {
        let d = dir(&format!("os-shapes-{spare}"));
        let slots = os_opts(&Options::default());
        let rename = Options {
            manifest_slots: false,
            manifest_spare: spare,
            ..slots.clone()
        };
        let mut model = BTreeMap::new();
        let mut db = Db::create(&d, slots.clone()).unwrap();
        for (r, opts) in [&rename, &slots, &rename, &slots].into_iter().enumerate() {
            os_round(
                &mut db,
                &mut model,
                &format!("s{r}"),
                r as u32 * OS_KEYS,
                OS_KEYS / 4,
            );
            db.flush().unwrap();
            db.settle().unwrap();
            drop(db);
            db = Db::open(&d, opts.clone()).unwrap();
            for (k, v) in model.iter().step_by(37) {
                assert_eq!(
                    read_vec(&db, k),
                    vec![v.clone()],
                    "spare {spare}, round {r}"
                );
            }
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Two seals landed by one drain each write their slot with the fdatasync
/// owed, and the second's sync waits until after the flush has answered: a
/// power cut while the second is written, or after the flush returned Ok
/// and before that sync, must leave every acknowledged commit.
#[test]
fn a_second_seal_landed_by_a_drain_never_overwrites_the_last_durable_slot() {
    let opts = os_opts(&Options::default());
    let (d, mut db, mut model, t) = os_start("os-two-seals", &opts);
    // A seal in flight when the flush's own seal is handed: the drain
    // lands both (`join_seal(true)`). The first table is large enough that
    // its seal is still writing when the flush asks for the drain; timing,
    // so the attempt is repeated until the watch saw it.
    let mut n = OS_KEYS;
    let (mut states, mut failures) = (0, Vec::new());
    for attempt in 0..20u32 {
        os_round(&mut db, &mut model, &format!("a{attempt}"), n, OS_KEYS / 2);
        n += OS_KEYS / 2;
        db.seal().unwrap();
        os_round(&mut db, &mut model, &format!("b{attempt}"), n, 100);
        n += 100;
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        let (k, f) = os_check_built(&t, &model, &opts);
        states += k;
        failures.extend(f);
        if t.lock().unwrap().reached(true) {
            break;
        }
    }
    db.watch_slot_writes(None);
    drop(db);
    let t = OsTracker::taken(t);
    assert!(
        t.reached(true),
        "no drain landed two seals with their slots deferred; the case was not reached: {:?}",
        t.writes
    );
    assert!(
        failures.is_empty(),
        "{} of {states} crash states lost acknowledged commits:\n{}\nslot writes: {:?}",
        failures.len(),
        failures.join("\n"),
        t.writes
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&t.root);
}

/// A process killed with a drain's slot written and its fdatasync owed
/// leaves that slot in the page cache only. The open takes it as the store;
/// the reopened store's first publish must not leave the store without a
/// durable slot that names it: the open makes the slot it read durable, or
/// the publish keeps off the one that is.
fn an_open_over_an_owed_slot(tag: &str, base: Options) {
    let opts = os_opts(&base);
    let (d, mut db, mut model, t) = os_start(&format!("os-reopen-{tag}"), &opts);
    for r in 1..=6u32 {
        os_round(&mut db, &mut model, &format!("r{r}"), r * OS_KEYS, OS_KEYS);
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        if !t.lock().unwrap().kills.is_empty() {
            break;
        }
    }
    db.watch_slot_writes(None);
    drop(db);
    let mut t = OsTracker::taken(t);
    assert!(
        !t.kills.is_empty(),
        "{tag}: no write followed a deferred slot whose sync was owed; the case was not \
         reached: {:?}",
        t.writes
    );
    let (kill, device, owed) = t.kills.remove(0);
    // Every acknowledged commit is in the model: the kill state is taken
    // while the writer waits in its flush.
    let mut db = Db::open(&kill, opts.clone()).expect("the kill state opens");
    assert_eq!(
        db.manifest_seq(),
        owed,
        "{tag}: the open took the unsynced slot as the store and published nothing"
    );
    let root2 = kill.with_file_name(format!(
        "{}-reopened",
        kill.file_name().unwrap().to_str().unwrap()
    ));
    // The device as the kill left it; what the open syncs, the watch is
    // told of by the sequences it carries (`SlotWrite::synced`).
    let t2 = OsTracker::new(&kill, &root2, device);
    db.watch_slot_writes(Some(os_watcher(t2.clone())));
    // Small enough to seal nothing before the flush, so the reopened
    // store's first publish is the flush's.
    os_round(&mut db, &mut model, "after-kill", 50 * OS_KEYS, 2000);
    OsTracker::arm(&t2, true);
    db.flush().unwrap();
    OsTracker::arm(&t2, false);
    db.settle().unwrap();
    db.watch_slot_writes(None);
    drop(db);
    let t2 = OsTracker::taken(t2);
    assert_eq!(
        t2.writes.first().map(|w| w.seq),
        Some(owed + 1),
        "{tag}: the reopened store's first publish was watched"
    );
    let failures = os_check_all(&t2.crashes, &model, &opts);
    assert!(
        failures.is_empty(),
        "{tag}: {} of {} crash states lost acknowledged commits:\n{}\nslot writes before the \
         kill: {:?}\nafter the reopen: {:?}",
        failures.len(),
        t2.crashes.len(),
        failures.join("\n"),
        t.writes,
        t2.writes
    );
    for (p, _) in &t.crashes {
        let _ = std::fs::remove_dir_all(p);
    }
    for (p, _, _) in &t.kills {
        let _ = std::fs::remove_dir_all(p);
    }
    let _ = std::fs::remove_dir_all(&kill);
    let _ = std::fs::remove_dir_all(&root2);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&t.root);
}

#[test]
fn an_open_that_takes_an_unsynced_slot_never_overwrites_the_durable_one() {
    an_open_over_an_owed_slot("plain", Options::default());
}

#[test]
fn an_open_that_takes_an_unsynced_slot_never_overwrites_the_durable_one_under_a_cipher() {
    an_open_over_an_owed_slot("cipher", under_cipher());
}

/// Whether the page cache holds the first page of `path` dirty or under
/// writeback -- not yet what the device holds -- read from the kernel
/// (`/proc/self/pagemap` over a mapping of the file, then
/// `/proc/kpageflags` at its frame); `None` where the kernel does not say,
/// which it does only to a privileged reader -- and, with
/// `SUPDB_TEST_UNPRIVILEGED` set, to nobody, so that a privileged run can
/// show what the tests hold where the gate runs without the privilege.
fn os_page_unwritten(path: &Path) -> Option<bool> {
    use std::io::{Read, Seek, SeekFrom};
    if std::env::var_os("SUPDB_TEST_UNPRIVILEGED").is_some() {
        return None;
    }
    let f = std::fs::File::open(path).ok()?;
    let m = unsafe { memmap2::Mmap::map(&f) }.ok()?;
    std::hint::black_box(m.first().copied()?);
    let word = |file: &str, at: u64| -> Option<u64> {
        let mut h = std::fs::File::open(file).ok()?;
        h.seek(SeekFrom::Start(at * 8)).ok()?;
        let mut b = [0u8; 8];
        h.read_exact(&mut b).ok()?;
        Some(u64::from_le_bytes(b))
    };
    let e = word("/proc/self/pagemap", m.as_ptr() as u64 / 4096)?;
    let pfn = e & ((1 << 55) - 1);
    if e >> 63 == 0 || pfn == 0 {
        return None;
    }
    let flags = word("/proc/kpageflags", pfn)?;
    // KPF_DIRTY and KPF_WRITEBACK.
    Some(flags & (1 << 4 | 1 << 8) != 0)
}

/// An open makes both slots durable as the page cache holds them before
/// anything can write over either: the slot it takes, which a process kill
/// may have left in the page cache alone, and the other, which a kill may
/// have left torn in the cache over a record the device still holds whole.
/// Held by the calls themselves (`Db::trace_dir_io`), wherever the test
/// runs, and by the kernel's own word on each slot file's page where it
/// says, with a file the open never touches, written the same way, the
/// control that the instrument sees a page not yet written back.
fn an_open_syncs_its_slot(tag: &str, base: Options) {
    use std::os::unix::fs::FileExt;
    let opts = os_opts(&base);
    let d = dir(&format!("os-open-sync-{tag}"));
    let control = d.with_extension("control");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, 4000);
    db.flush().unwrap();
    db.settle().unwrap();
    drop(db);
    let bytes: [Vec<u8>; 2] = [0, 1].map(|j| std::fs::read(d.join(OS_SLOTS[j])).unwrap());
    let seqs = [0, 1].map(|j| os_seq_of(&bytes[j]));
    let newest = if seqs[0] > seqs[1] { 0 } else { 1 };
    assert!(seqs[newest].is_some(), "{tag}: a slot: {seqs:?}");
    // Both slots written again as they are, so neither is on the device
    // until something syncs it: what a kill after their writes leaves.
    for j in 0..2 {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(d.join(OS_SLOTS[j]))
            .unwrap();
        f.write_all_at(&bytes[j], 0).unwrap();
    }
    std::fs::write(&control, &bytes[newest]).unwrap();
    let before = [0, 1].map(|j| os_page_unwritten(&d.join(OS_SLOTS[j])));
    let from = Db::trace_dir_io(true).len();
    let db = Db::open(&d, opts.clone()).unwrap();
    let calls = osl_calls(&d, from);
    // The calls: an fdatasync of each slot file that returned.
    for slot in OS_SLOTS {
        assert!(
            calls
                .iter()
                .any(|(what, p)| *what == "sync_slot" && *p == d.join(slot)),
            "{tag}: the open fdatasynced {slot}: {calls:?}"
        );
    }
    if before.iter().any(|b| b.is_none()) {
        eprintln!(
            "{tag}: the kernel does not say which pages are dirty to this process; the page \
             flags not read"
        );
    } else {
        assert_eq!(
            before,
            [Some(true), Some(true)],
            "{tag}: the instrument sees the slots just written as not on the device"
        );
        let after = [0, 1].map(|j| os_page_unwritten(&d.join(OS_SLOTS[j])));
        assert_eq!(
            after,
            [Some(false), Some(false)],
            "{tag}: both slots (the open took {}) are on the device once the open has returned",
            OS_SLOTS[newest]
        );
        assert_eq!(
            os_page_unwritten(&control),
            Some(true),
            "{tag}: the control, a file the open does not touch, still waits for writeback"
        );
    }
    for (k, v) in model.iter().step_by(97) {
        assert_eq!(read_vec(&db, k), vec![v.clone()], "{tag}");
    }
    drop(db);
    let _ = std::fs::remove_file(&control);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_open_makes_the_slot_it_takes_durable() {
    an_open_syncs_its_slot("plain", Options::default());
}

#[test]
fn an_open_makes_the_slot_it_takes_durable_under_a_cipher() {
    an_open_syncs_its_slot("cipher", under_cipher());
}

/// Drains that publish nothing after the seal's slot -- no merge, no
/// promotion, no partitioning at the flush -- each seal rotating the log
/// (`seal_rotates_wal`), so each slot's owed fdatasync runs on its own
/// after the answer and retires the log file behind it, and the next
/// drain's slot must keep off the one that sync made durable: the slot
/// before it covers less than the log that is left.
fn drains_after_an_owed_sync(tag: &str, base: Options) {
    a_publish_after_a_slot(
        tag,
        Options {
            compact: false,
            promote: false,
            partition_on_flush: false,
            seal_rotates_wal: true,
            ..base
        },
        OS_KEYS / 4,
        |t| {
            t.writes.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                a.armed && b.armed && a.deferred && b.seq == a.seq + 1 && b.synced[a.slot] == a.seq
            })
        },
    );
}

#[test]
fn a_drain_after_an_owed_slot_was_synced_keeps_that_slot() {
    drains_after_an_owed_sync("plain", Options::default());
}

#[test]
fn a_drain_after_an_owed_slot_was_synced_keeps_that_slot_under_a_cipher() {
    drains_after_an_owed_sync("cipher", under_cipher());
}

/// A drain's slot is written with its fdatasync owed, and the publish
/// after it in the same drain -- a merge's or a promotion's, or a second
/// seal's -- writes the same slot file, since the slot the last durable
/// publish did not write is the one the owed slot is in. That write fails
/// part way (`Db::tear_next_slot_write`: half the image, then an error, as
/// a device out of space would). The owed fdatasync, run after it, would
/// cover the torn bytes and not the manifest it was owed for: the slot it
/// names durable is one that does not parse, the log files behind it
/// retire, and the next publish writes over the only valid slot. After the
/// failure and every sync the store makes, the store as it stands must
/// open with every acknowledged commit, and so must the store after the
/// drains that follow.
fn a_torn_write_over_an_owed_slot(tag: &str, base: Options) {
    // The seal rotates the log, so what waits behind the owed slot is a
    // closed log file the slot's sync would retire.
    let opts = os_opts(&Options {
        seal_rotates_wal: true,
        ..base
    });
    let d = dir(&format!("os-tear-{tag}"));
    let c = dir(&format!("os-tear-{tag}-copy"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    let mut torn = false;
    let mut answers = Vec::new();
    // Every slot write seen, unarmed: which write the seam tore.
    let seen = OsTracker::new(&d, &dir(&format!("os-tear-{tag}-seen")), Default::default());
    db.watch_slot_writes(Some(os_watcher(seen.clone())));
    for r in 1..=6u32 {
        db.tear_next_slot_write();
        os_round(
            &mut db,
            &mut model,
            &format!("r{r}"),
            r * OS_KEYS,
            OS_KEYS / 4,
        );
        answers.push(db.flush().err().map(|e| e.to_string()));
        answers.push(db.settle().err().map(|e| e.to_string()));
        // A commit with nothing staged answers whatever the segment work
        // left for the writer.
        answers.push(db.commit().err().map(|e| e.to_string()));
        torn = answers
            .iter()
            .flatten()
            .any(|e| e.contains("a test tore this manifest slot write"));
        if torn {
            break;
        }
    }
    assert!(
        torn,
        "{tag}: no slot write was made over an owed slot; the case was not reached: {answers:?}"
    );
    db.watch_slot_writes(None);
    let seen = OsTracker::taken(seen);
    let _ = std::fs::remove_dir_all(&seen.root);
    // Nothing in flight after the settle: the store's files as they stand
    // are a state a power cut may leave, the torn slot and the slots the
    // owed sync would have named durable among them.
    copy_store(&d, &c);
    // The control: the write left a slot the open refuses, a sequence
    // newer than the store it opens as.
    let newest = OS_SLOTS
        .iter()
        .filter_map(|s| os_seq_of(&std::fs::read(c.join(s)).unwrap_or_default()))
        .max();
    {
        let o = dir(&format!("os-tear-{tag}-control"));
        copy_store(&c, &o);
        let db = Db::open(&o, opts.clone()).expect("the store after the tear opens");
        assert!(
            newest.is_some_and(|n| db.manifest_seq() < n),
            "{tag}: the slot write was not left torn: the open took {} of slots up to {newest:?}",
            db.manifest_seq()
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&o);
    }
    // The write torn is the newest slot's, and it was made over a drain's
    // slot whose fdatasync was still owed: the write before it deferred,
    // and not covered by a sync when the torn one began -- the case the
    // seam is for, not the first slot write it happened to meet.
    let at = seen
        .writes
        .iter()
        .position(|w| Some(w.seq) == newest)
        .expect("the torn write was watched");
    assert!(
        at > 0 && {
            let (p, w) = (seen.writes[at - 1], seen.writes[at]);
            p.deferred && p.seq + 1 == w.seq && w.synced[p.slot] != p.seq
        },
        "{tag}: the write torn, seq {newest:?}, was not over an owed slot: {:?}",
        seen.writes
    );
    let failures = os_check_all(&[(c.clone(), "after the torn write".into())], &model, &opts);
    assert!(
        failures.is_empty(),
        "{tag}: the store after a torn slot write over an owed slot: {failures:?}; answers \
         {answers:?}"
    );
    // The drains after it: their crash states, at every slot write, must
    // open too, the torn slot being whatever the device holds of it.
    let root = dir(&format!("os-tear-{tag}-crashes"));
    let t = OsTracker::new(&d, &root, os_sync_slots(&d));
    db.watch_slot_writes(Some(os_watcher(t.clone())));
    let (mut states, mut later) = (0, Vec::new());
    for r in 7..=8u32 {
        os_round(
            &mut db,
            &mut model,
            &format!("r{r}"),
            r * OS_KEYS,
            OS_KEYS / 4,
        );
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        let (n, f) = os_check_built(&t, &model, &opts);
        states += n;
        later.extend(f);
    }
    db.watch_slot_writes(None);
    drop(db);
    let t = OsTracker::taken(t);
    assert!(states > 0, "{tag}: no crash state was built after the tear");
    assert!(
        later.is_empty(),
        "{tag}: {} of {states} crash states after the tear lost acknowledged commits:\n{}\nslot \
         writes: {:?}",
        later.len(),
        later.join("\n"),
        t.writes
    );
    let failures = os_check_all(&[(d.clone(), "reopened".into())], &model, &opts);
    assert!(failures.is_empty(), "{tag}: {failures:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_slot_write_torn_over_an_owed_slot_is_not_named_durable_by_the_owed_sync() {
    a_torn_write_over_an_owed_slot("plain", Options::default());
}

#[test]
fn a_slot_write_torn_over_an_owed_slot_is_not_named_durable_by_the_owed_sync_under_a_cipher() {
    a_torn_write_over_an_owed_slot("cipher", under_cipher());
}

/// Every file of a store directory and its bytes.
fn os_files(d: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

/// A process killed between a merge's or a promotion's slot write and its
/// fdatasync: the slot in the page cache names fewer files than the slot
/// before it, and the files it no longer names -- the merge's inputs, the
/// promotion's old names -- are still on disk, since they go only after
/// the publish. The open takes that slot, and makes it durable before it
/// sweeps those files: swept first, a power cut or a failed sync before
/// the slot's fdatasync leaves the device's other slot naming files that
/// are gone. A failed fdatasync there (`Options::fail_open_slot_sync`) is
/// the open's answer, and the directory is left as the open found it.
fn an_open_whose_slot_sync_fails(tag: &str, base: Options) {
    an_open_whose_slot_sync_fails_in(tag, base, false);
}

/// The same kill reopened under the slots, or with `rename` under the
/// rename shape (`manifest_slots` off), whose open makes the slot it takes
/// durable as the slots' open does.
fn an_open_whose_slot_sync_fails_in(tag: &str, base: Options, rename: bool) {
    let under = matches!(base.encryption, Encryption::Cipher(_));
    let opts = os_opts(&base);
    let reopen = Options {
        manifest_slots: !rename,
        ..opts.clone()
    };
    let d = dir(&format!("os-opensync-{tag}"));
    let k = dir(&format!("os-opensync-{tag}-kill"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    // At a synced publish's write, the store as it is just before it.
    type Seen = std::sync::Arc<std::sync::Mutex<Option<(u64, usize)>>>;
    let seen: Seen = Default::default();
    let watch: supdb::db::SlotWatch = {
        let (seen, d, k) = (seen.clone(), d.clone(), k.clone());
        std::sync::Arc::new(move |w: &supdb::db::SlotWrite| {
            let mut s = seen.lock().unwrap_or_else(|e| e.into_inner());
            if s.is_none() && !w.deferred {
                os_copy(&d, &k);
                *s = Some((w.seq, w.slot));
            }
        })
    };
    db.watch_slot_writes(Some(watch));
    for r in 1..=4u32 {
        os_round(
            &mut db,
            &mut model,
            &format!("r{r}"),
            r * OS_KEYS,
            OS_KEYS / 4,
        );
        db.flush().unwrap();
        if seen.lock().unwrap().is_some() {
            break;
        }
    }
    db.watch_slot_writes(None);
    let (seq, slot) = seen
        .lock()
        .unwrap()
        .expect("a drain published a merge or a promotion; the case was not reached");
    // The kill state: the files as they were before the write, the slot
    // as the write left it, and every file written since the copy that
    // the slot may name (under a cipher, the envelopes its manifest image
    // wrote) -- not the files the publish went on to remove.
    let now = os_files(&d);
    let bytes = now
        .get(OS_SLOTS[slot])
        .cloned()
        .expect("the slot file the publish wrote");
    assert_eq!(
        os_seq_of(&bytes),
        Some(seq),
        "{tag}: the slot still holds the publish watched"
    );
    std::fs::write(k.join(OS_SLOTS[slot]), &bytes).unwrap();
    let before_copy = os_files(&k);
    for (name, b) in &now {
        if !before_copy.contains_key(name) {
            std::fs::write(k.join(name), b).unwrap();
        }
    }
    db.settle().unwrap();
    drop(db);
    let found = os_files(&k);
    // The files the slot before the publish names: what a store opened
    // from that slot alone leaves of the kill state.
    let named: std::collections::BTreeSet<String> = {
        let o = dir(&format!("os-opensync-{tag}-older"));
        copy_store(&k, &o);
        std::fs::write(o.join(OS_SLOTS[slot]), &found[OS_SLOTS[1 - slot]]).unwrap();
        drop(Db::open(&o, opts.clone()).expect("the slot before the publish opens"));
        let left = os_files(&o);
        let _ = std::fs::remove_dir_all(&o);
        found
            .keys()
            .filter(|n| left.contains_key(*n))
            .cloned()
            .collect()
    };
    let refused = match Db::open(
        &k,
        Options {
            fail_open_slot_sync: true,
            ..reopen.clone()
        },
    ) {
        Ok(_) => panic!("{tag}: the open answered Ok over a failed fdatasync of its slot"),
        Err(e) => e.to_string(),
    };
    assert!(
        refused.contains("a test failed the open's fdatasync of its manifest slot"),
        "{tag}: the open refused for another reason: {refused}"
    );
    // The sync was of the slot the open took, the one the publish wrote --
    // not of the other, which the device holds already: the seam names the
    // inode of the file it failed for.
    let ino = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(k.join(OS_SLOTS[slot])).unwrap().ino()
    };
    assert!(
        refused.ends_with(&format!("(inode {ino})")),
        "{tag}: the open's fdatasync was not for the slot it took, {} (inode {ino}): {refused}",
        OS_SLOTS[slot]
    );
    let left = os_files(&k);
    // Nothing removed, changed or made: under a cipher too, where the
    // envelopes neither slot names -- a crash's leftovers whichever slot
    // the device holds -- are swept only once the slot taken is durable.
    let gone: Vec<_> = found.keys().filter(|n| !left.contains_key(*n)).collect();
    let changed: Vec<_> = found
        .iter()
        .filter(|(n, b)| left.get(*n).is_some_and(|l| l != *b))
        .map(|(n, _)| n)
        .collect();
    let made: Vec<_> = left.keys().filter(|n| !found.contains_key(*n)).collect();
    assert!(
        gone.is_empty() && changed.is_empty() && made.is_empty(),
        "{tag}: the open that failed its slot's fdatasync changed the directory: removed \
         {gone:?}, changed {changed:?}, made {made:?}"
    );
    // The control: the open that succeeds removes files the slot before
    // the publish names (`named`), the ones a power cut before the slot's
    // fdatasync would have left that slot without, so the failed open had
    // something to leave; and under a cipher it sweeps envelopes neither
    // slot names, which the failed open left too.
    let db = Db::open(&k, reopen.clone()).expect("the kill state opens");
    drop(db);
    let after = os_files(&k);
    assert!(
        named.iter().any(|n| !after.contains_key(n)),
        "{tag}: the kill state held no file the slot before the publish names that the open \
         removes; the case was not reached"
    );
    if under {
        assert!(
            found.keys().any(|n| !named.contains(n)
                && n != OS_SLOTS[0]
                && n != OS_SLOTS[1]
                && !after.contains_key(n)),
            "{tag}: the kill state held no envelope the open sweeps; the case was not reached"
        );
    }
    let failures = os_check_all(&[(k.clone(), "the kill state".into())], &model, &reopen);
    assert!(failures.is_empty(), "{tag}: {failures:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it() {
    an_open_whose_slot_sync_fails("plain", Options::default());
}

#[test]
fn an_open_whose_slot_sync_fails_leaves_the_directory_as_it_found_it_under_a_cipher() {
    an_open_whose_slot_sync_fails("cipher", under_cipher());
}

/// A store under the slots killed between a merge's slot write and its
/// fdatasync, reopened under the rename shape: the open makes the slot it
/// takes durable before it sweeps what that slot stopped naming, whatever
/// the shape it is opened in.
#[test]
fn an_open_under_the_rename_shape_whose_slot_sync_fails_leaves_the_directory_as_it_found_it() {
    an_open_whose_slot_sync_fails_in("to-rename", Options::default(), true);
}

#[test]
fn an_open_under_the_rename_shape_whose_slot_sync_fails_leaves_the_directory_under_a_cipher() {
    an_open_whose_slot_sync_fails_in("to-rename-cipher", under_cipher(), true);
}

/// A slot's fdatasync fails -- a drain's owed one, run after its answer,
/// or the synced publish after it in the same drain, whichever the drain
/// reaches first -- after the test made both slots durable itself. The
/// slot whose sync failed is not one the device is known to hold, so it
/// is not the slot a publish keeps off: the first slot write after the
/// failure goes into that slot again, and a power cut during it leaves
/// the other slot, which the test made durable. Built at that write: the
/// slot being written torn, and the other slot as the test's own sync
/// left it where its bytes have not changed since, torn where they have.
fn a_failed_slot_sync(tag: &str, base: Options, owed_only: bool) {
    // `owed_only`: drains that publish nothing after the seal's slot, so
    // the sync that fails is the owed one, run after the answer; otherwise
    // a merge or a promotion publishes over the owed slot and its own sync
    // is the one that fails.
    let base = if owed_only {
        Options {
            compact: false,
            promote: false,
            partition_on_flush: false,
            ..base
        }
    } else {
        base
    };
    let opts = os_opts(&Options {
        seal_rotates_wal: true,
        ..base
    });
    let d = dir(&format!("os-syncfail-{tag}"));
    let root = dir(&format!("os-syncfail-{tag}-crashes"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    // The watch is on from the store's creation, so the engine keeps its
    // record of what its slot syncs covered (`SlotWrite::synced`) from the
    // start, and looks only once the failure is in.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(PathBuf, String)>>>;
    let seen: Seen = Default::default();
    let looking = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let record: std::sync::Arc<std::sync::Mutex<Option<[u64; 2]>>> = Default::default();
    let synced: std::sync::Arc<std::sync::Mutex<[Vec<u8>; 2]>> = Default::default();
    let watch: supdb::db::SlotWatch = {
        let (seen, record, looking, synced, d, root) = (
            seen.clone(),
            record.clone(),
            looking.clone(),
            synced.clone(),
            d.clone(),
            root.clone(),
        );
        std::sync::Arc::new(move |w: &supdb::db::SlotWrite| {
            if !looking.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let mut s = seen.lock().unwrap_or_else(|e| e.into_inner());
            if !s.is_empty() {
                return;
            }
            *record.lock().unwrap_or_else(|e| e.into_inner()) = Some(w.synced);
            let synced = synced.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let (i, o) = (w.slot, 1 - w.slot);
            let cur = |j: usize| std::fs::read(d.join(OS_SLOTS[j])).unwrap_or_default();
            let c = root.join("crash");
            os_copy(&d, &c);
            let _ = std::fs::write(c.join(OS_SLOTS[i]), os_torn(&cur(i), w.seq));
            let other = cur(o);
            let same = other == synced[o];
            let held = if same {
                other
            } else {
                os_torn(&synced[o], os_seq_of(&other).unwrap_or(0))
            };
            let _ = std::fs::write(c.join(OS_SLOTS[o]), held);
            s.push((
                c,
                format!(
                    "publish {} into {} torn, {} {}",
                    w.seq,
                    OS_SLOTS[i],
                    OS_SLOTS[o],
                    if same {
                        "as the test synced it"
                    } else {
                        "written since and never known synced, torn"
                    }
                ),
            ));
        })
    };
    db.watch_slot_writes(Some(watch));
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    // Both slots on the device, each by an fdatasync of the engine's own
    // that returned: the drain's owed one has run by the settle.
    let device = os_sync_slots(&d);
    let tested = [0, 1].map(|j| os_seq_of(&device[j]).unwrap_or(0));
    *synced.lock().unwrap() = device;
    db.fail_next_slot_sync();
    os_round(&mut db, &mut model, "r1", OS_KEYS, OS_KEYS / 4);
    let mut answers = vec![db.flush().err().map(|e| e.to_string())];
    answers.push(db.settle().err().map(|e| e.to_string()));
    answers.push(db.commit().err().map(|e| e.to_string()));
    assert!(
        answers
            .iter()
            .flatten()
            .any(|e| e.contains("a test failed this manifest slot's fdatasync")),
        "{tag}: no slot sync failed; the case was not reached: {answers:?}"
    );
    // The first slot write after the failure, and its crash state.
    looking.store(true, std::sync::atomic::Ordering::Release);
    os_round(&mut db, &mut model, "r2", 2 * OS_KEYS, OS_KEYS / 4);
    let flushed = db.flush();
    db.watch_slot_writes(None);
    let built = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(
        built.len(),
        1,
        "{tag}: no slot was written after the failure: {flushed:?}"
    );
    // No sync of a slot returned between the test's own and the watched
    // write but the one that failed, so the record names what the test
    // synced: a slot recorded synced where its fdatasync failed is a crash
    // state this test would build too kindly.
    let record = record.lock().unwrap().expect("the watched write's record");
    assert_eq!(
        record, tested,
        "{tag}: the engine records a slot synced that the test did not see synced (the test's \
         slots {tested:?}); answers {answers:?}"
    );
    let failures = os_check_all(&built, &model, &opts);
    assert!(
        failures.is_empty(),
        "{tag}: after a failed slot sync: {failures:?}; answers {answers:?}"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&root);
}

/// Under the slots a seal's own publish whose slot fdatasync fails is
/// answered and stops nothing: the slot was never known durable, so the
/// next publish writes it again, whole, and the store goes on and reopens
/// with every acknowledged write. (The rename shape stops the queue there
/// instead, since its publish's directory fsync is the seal's names'.)
fn a_seal_publish_slot_sync_fails_under_the_slots(tag: &str, base: Options) {
    let opts = os_opts(&Options {
        compact: false,
        promote: false,
        partition_on_flush: false,
        ..base
    });
    let d = dir(&format!("os-sealsyncfail-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS / 4);
    db.flush().unwrap();
    db.settle().unwrap();
    os_round(&mut db, &mut model, "r1", OS_KEYS, OS_KEYS / 8);
    db.fail_next_slot_sync();
    let sealed = db.seal();
    let settled = db.settle();
    let answers = [sealed.err(), settled.err(), db.commit().err()];
    assert!(
        answers.iter().flatten().any(|e| e
            .to_string()
            .contains("a test failed this manifest slot's fdatasync")),
        "{tag}: premise: the seal's publish failed its slot sync: {answers:?}"
    );
    os_round(&mut db, &mut model, "r2", 2 * OS_KEYS, OS_KEYS / 8);
    db.flush()
        .unwrap_or_else(|e| panic!("{tag}: the flush after a failed slot sync: {e}"));
    db.settle().unwrap();
    drop(db);
    let failures = os_check_all(
        &[(
            d.clone(),
            format!("{tag}: reopened after a seal's slot sync failed"),
        )],
        &model,
        &opts,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_seals_publish_whose_slot_sync_fails_is_written_again_by_the_next_under_the_slots() {
    a_seal_publish_slot_sync_fails_under_the_slots("plain", Options::default());
}

#[test]
fn a_seals_publish_whose_slot_sync_fails_is_written_again_under_the_slots_and_a_cipher() {
    a_seal_publish_slot_sync_fails_under_the_slots("cipher", under_cipher());
}

#[test]
fn a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off() {
    a_failed_slot_sync("plain", Options::default(), false);
}

#[test]
fn a_slot_whose_fdatasync_failed_is_not_the_one_a_publish_keeps_off_under_a_cipher() {
    a_failed_slot_sync("cipher", under_cipher(), false);
}

#[test]
fn a_slot_whose_owed_fdatasync_failed_is_not_the_one_a_publish_keeps_off() {
    a_failed_slot_sync("owed-plain", Options::default(), true);
}

#[test]
fn a_slot_whose_owed_fdatasync_failed_is_not_the_one_a_publish_keeps_off_under_a_cipher() {
    a_failed_slot_sync("owed-cipher", under_cipher(), true);
}

/// The same tear at a drain that lands two seals: the first seal's slot is
/// written with its fdatasync owed, and the second seal's slot, deferred
/// too, goes into the same file and fails part way. With no merge, no
/// promotion and no partitioning at the flush, the second seal's is the
/// only write that can meet the owed slot. The owed fdatasync, run after
/// the answer, would cover the torn bytes and retire the log behind the
/// first seal.
fn a_torn_second_seal_slot(tag: &str, base: Options) {
    let opts = os_opts(&Options {
        seal_rotates_wal: true,
        compact: false,
        promote: false,
        partition_on_flush: false,
        ..base
    });
    let d = dir(&format!("os-tear2-{tag}"));
    let c = dir(&format!("os-tear2-{tag}-copy"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    // A seal in flight when the flush's own seal is handed: the drain
    // lands both. Timing, so repeated until the tear was met.
    let mut n = OS_KEYS;
    let mut answers = Vec::new();
    let mut torn = false;
    for attempt in 0..20u32 {
        os_round(&mut db, &mut model, &format!("a{attempt}"), n, OS_KEYS / 2);
        n += OS_KEYS / 2;
        db.seal().unwrap();
        os_round(&mut db, &mut model, &format!("b{attempt}"), n, 100);
        n += 100;
        db.tear_next_slot_write();
        answers.push(db.flush().err().map(|e| e.to_string()));
        answers.push(db.settle().err().map(|e| e.to_string()));
        torn = answers
            .iter()
            .flatten()
            .any(|e| e.contains("a test tore this manifest slot write"));
        if torn {
            break;
        }
    }
    assert!(
        torn,
        "{tag}: no drain landed a second seal over an owed slot; the case was not reached: \
         {answers:?}"
    );
    copy_store(&d, &c);
    drop(db);
    let failures = os_check_all(&[(c.clone(), "after the torn write".into())], &model, &opts);
    assert!(
        failures.is_empty(),
        "{tag}: the store after a torn second seal's slot over an owed slot: {failures:?}; \
         answers {answers:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_second_seals_slot_torn_over_an_owed_slot_is_not_named_durable_by_the_owed_sync() {
    a_torn_second_seal_slot("plain", Options::default());
}

#[test]
fn a_second_seals_slot_torn_over_an_owed_slot_is_not_named_durable_by_the_owed_sync_under_a_cipher()
{
    a_torn_second_seal_slot("cipher", under_cipher());
}

/// Under `Options::manifest_pending` a seal's readable landing writes its
/// slot naming the seal's segments pending and hands the slot's fdatasync
/// to the sync helper. That sync failing is a seal that failed after its
/// segments were published: the flush answers it, the seal queue stops,
/// and no slot is written until the store is reopened, so the slot whose
/// sync failed is never one a publish keeps off. A power cut then leaves
/// the pending slot as written or torn, and either opens to every
/// acknowledged commit, the log behind it unretired.
fn a_failed_pending_slot_sync(tag: &str, base: Options) {
    let opts = os_opts(&Options {
        manifest_pending: true,
        ..base
    });
    let d = dir(&format!("os-pendfail-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    // Each slot write, with the bytes its file held before it.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(u64, usize, bool, Vec<u8>)>>>;
    let seen: Seen = Default::default();
    let watch: supdb::db::SlotWatch = {
        let (seen, d) = (seen.clone(), d.clone());
        std::sync::Arc::new(move |w: &supdb::db::SlotWrite| {
            let held = std::fs::read(d.join(OS_SLOTS[w.slot])).unwrap_or_default();
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((w.seq, w.slot, w.deferred, held));
        })
    };
    db.watch_slot_writes(Some(watch));
    os_round(&mut db, &mut model, "r1", OS_KEYS, OS_KEYS / 4);
    db.fail_next_slot_sync();
    let refused = db
        .flush()
        .expect_err("the flush answered Ok over a failed fdatasync of its pending slot")
        .to_string();
    assert!(
        refused.contains("a test failed this manifest slot's fdatasync"),
        "{tag}: the flush failed for another reason: {refused}"
    );
    let (seq, slot, deferred, held) = seen
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the drain wrote a slot; the case was not reached");
    assert!(
        deferred,
        "{tag}: the slot whose sync failed was the pending one, synced by the helper"
    );
    let written = seen.lock().unwrap().len();
    let again = db
        .flush()
        .expect_err("a flush after the failure answered Ok");
    assert!(
        again
            .to_string()
            .contains("a test failed this manifest slot's fdatasync"),
        "{tag}: the next flush answers the failure: {again}"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        written,
        "{tag}: a slot was written after a slot's failed fdatasync, before a reopen"
    );
    let as_written = dir(&format!("os-pendfail-{tag}-written"));
    let torn = dir(&format!("os-pendfail-{tag}-torn"));
    copy_store(&d, &as_written);
    copy_store(&d, &torn);
    std::fs::write(torn.join(OS_SLOTS[slot]), os_torn(&held, seq)).unwrap();
    db.watch_slot_writes(None);
    drop(db);
    let failures = os_check_all(
        &[
            (as_written.clone(), "the pending slot as written".into()),
            (torn.clone(), "the pending slot torn".into()),
        ],
        &model,
        &opts,
    );
    assert!(failures.is_empty(), "{tag}: {failures:?}");
    for p in [&d, &as_written, &torn] {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[test]
fn a_pending_slot_whose_fdatasync_failed_stops_the_slots_until_a_reopen() {
    a_failed_pending_slot_sync("plain", Options::default());
}

#[test]
fn a_pending_slot_whose_fdatasync_failed_stops_the_slots_until_a_reopen_under_a_cipher() {
    a_failed_pending_slot_sync("cipher", under_cipher());
}

/// A store from before the pending flags opens with both slots rewritten
/// in this version, the newest first, so the slot the store was read from
/// stands until the other is durable and a write the device tears leaves
/// it for the next open. Built with the other slot torn already -- a
/// publish of the older writer a power cut cut short -- so the slot read
/// is the only manifest on the device. Plain only: a store under a cipher
/// seals its slots and has none from before the flags.
#[test]
fn an_upgrade_never_writes_first_over_the_slot_the_store_was_read_from() {
    let opts = os_opts(&Options::default());
    let d = dir("os-upgrade-torn");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    drop(db);
    let bytes: [Vec<u8>; 2] = [0, 1].map(|j| std::fs::read(d.join(OS_SLOTS[j])).unwrap());
    let seqs = [0, 1].map(|j| os_seq_of(&bytes[j]).unwrap_or(0));
    let newest = if seqs[0] > seqs[1] { 0 } else { 1 };
    let v2 = common::manifest_v2_of(&bytes[newest], seqs[newest] + 1);
    std::fs::write(d.join(OS_SLOTS[newest]), &v2).unwrap();
    std::fs::write(
        d.join(OS_SLOTS[1 - newest]),
        os_torn(&bytes[1 - newest], seqs[newest] + 2),
    )
    .unwrap();
    let refused = match Db::open(
        &d,
        Options {
            tear_open_slot_write: true,
            ..opts.clone()
        },
    ) {
        Ok(_) => panic!("the open answered Ok over a torn slot write"),
        Err(e) => e.to_string(),
    };
    assert!(
        refused.contains("a test tore this manifest slot write"),
        "the open failed for another reason: {refused}"
    );
    assert!(
        std::fs::read(d.join(OS_SLOTS[newest])).unwrap() == v2,
        "the slot the store was read from was written over before the other was durable"
    );
    let db = Db::open(&d, opts.clone()).expect("the store opens after the torn upgrade");
    for (k, v) in model.iter().step_by(37) {
        assert_eq!(read_vec(&db, k), vec![v.clone()], "after the torn upgrade");
    }
    drop(db);
    for slot in OS_SLOTS {
        assert_eq!(
            std::fs::read(d.join(slot)).unwrap()[8],
            3,
            "{slot} rewritten in this version"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A store's first publishes after a create or an open that wrote a slot
/// of its own, every slot write's crash states built and opened, and none
/// of them over the only manifest the device holds. `db` is that store, as
/// the create or the open handed it over, and `device` what the device held
/// of its slots before the open.
fn os_after_open(
    tag: &str,
    mut db: Db,
    k: &Path,
    device: [Vec<u8>; 2],
    mut model: BTreeMap<Vec<u8>, Vec<u8>>,
    opts: &Options,
) {
    let root = k.with_file_name(format!(
        "{}-crashes",
        k.file_name().unwrap().to_str().unwrap()
    ));
    let t = OsTracker::new(k, &root, device);
    db.watch_slot_writes(Some(os_watcher(t.clone())));
    let (mut states, mut failures) = (0, Vec::new());
    for r in 0..2u32 {
        os_round(
            &mut db,
            &mut model,
            &format!("after-{r}"),
            (60 + r) * OS_KEYS,
            OS_KEYS / 4,
        );
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        let (n, f) = os_check_built(&t, &model, opts);
        states += n;
        failures.extend(f);
    }
    db.watch_slot_writes(None);
    drop(db);
    let t = OsTracker::taken(t);
    assert!(states > 0, "{tag}: no slot write was watched");
    assert!(
        t.bare.is_empty(),
        "{tag}: publishes {:?} were written over the only manifest the device held: {:?}",
        t.bare,
        t.writes
    );
    assert!(
        failures.is_empty(),
        "{tag}: {} of {states} crash states lost acknowledged commits:\n{}\nslot writes: {:?}",
        failures.len(),
        failures.join("\n"),
        t.writes
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A created store's slot is the one its first publishes keep off: the
/// other slot is an empty file until a publish writes it.
fn the_first_publish_after_create(tag: &str, base: Options) {
    let opts = os_opts(&base);
    let d = dir(&format!("os-create-{tag}"));
    let db = Db::create(&d, opts.clone()).unwrap();
    let device = os_sync_slots(&d);
    assert!(
        os_seq_of(&device[0]).is_some() && os_seq_of(&device[1]).is_none(),
        "{tag}: a created store holds one manifest, in the first slot (the premise)"
    );
    os_after_open(tag, db, &d, device, BTreeMap::new(), &opts);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_first_publish_after_create_keeps_off_the_slot_create_wrote() {
    the_first_publish_after_create("plain", Options::default());
}

#[test]
fn the_first_publish_after_create_keeps_off_the_slot_create_wrote_under_a_cipher() {
    the_first_publish_after_create("cipher", under_cipher());
}

/// A store with no manifest -- one from before manifests -- gets one from
/// its open, and the open's slot is the one the store's first publishes
/// keep off. Plain only: a store under a cipher has had a manifest from
/// its birth.
#[test]
fn a_store_opened_without_a_manifest_keeps_the_slot_its_open_wrote() {
    let opts = os_opts(&Options::default());
    let d = dir("os-nomanifest");
    drop(Db::create(&d, opts.clone()).unwrap());
    for s in OS_SLOTS {
        std::fs::remove_file(d.join(s)).unwrap();
    }
    let db = Db::open(&d, opts.clone()).expect("the store without a manifest opens");
    os_after_open(
        "no-manifest",
        db,
        &d,
        [Vec::new(), Vec::new()],
        BTreeMap::new(),
        &opts,
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A process killed with an ordered run open leaves the run's temp file,
/// and the open rewrites its acknowledged batches as a piece, names it in
/// a slot and removes the temp file: that slot is the one the store's first
/// publishes keep off, since nothing else on the device holds the run.
/// Plain only: under a cipher a run's commits are in the log and its file
/// is never in the directory.
#[test]
fn an_open_that_recovers_a_run_keeps_the_slot_that_names_it() {
    let opts = os_opts(&Options::default());
    let d = dir("os-recover");
    let k = dir("os-recover-kill");
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    // Above every key the store holds: an ordered run, its commits synced
    // in its own file under the default policy.
    for i in 0..4_000u32 {
        let key = format!("run-{i:06}").into_bytes();
        let v = format!("run-{i:08}-{}", "y".repeat(80)).into_bytes();
        db.append(&key, &v);
        model.insert(key, v);
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    os_copy(&d, &k);
    drop(db);
    assert!(
        open_run(&k),
        "the kill state holds the run's temp file (the premise)"
    );
    let device = os_sync_slots(&k);
    let db = Db::open(&k, opts.clone()).expect("the kill state opens");
    assert!(
        !open_run(&k),
        "the open recovered the run and removed its temp file (the premise)"
    );
    assert_eq!(
        read_vec(&db, b"run-003999"),
        vec![model[b"run-003999".as_slice()].clone()],
        "the run was recovered (the premise)"
    );
    os_after_open("recover", db, &k, device, model, &opts);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&k);
}

// ---------------------------------------------------------------------
// open-order: an open removes nothing until it has chosen the manifest
// slot that is the store and checked it against the directory, and it
// refuses, by name and with every file left as it found it, where what
// that slot names cannot be the store: a name the directory does not
// hold, no readable slot beside partitions only a manifest names, or a
// fall back past a slot that failed its check onto one beside a segment
// whose records neither it nor the log holds. Every state a crash leaves
// still opens -- a torn publish's slot is the one the open passes over,
// and what it named is swept as it always was -- and a slot whose pending
// names the open takes is written durable only once the open has made
// their files and the directory durable (`Options::manifest_pending`).
// ---------------------------------------------------------------------

/// The words of each refusal, which a refusal further in -- a segment the
/// open fails to find once it has swept -- does not carry.
const OO_GONE: &str = "is missing from the directory";
const OO_KEYS: &str = "holds keys nothing the slot names";
const OO_NO_SLOT: &str = "no manifest slot can be read";

const OO_SLOTS: [&str; 2] = ["manifest", "manifest.alt"];

/// A slot's publish sequence, for either a plain or a sealed slot: both
/// carry it in the clear behind a nine-byte magic.
fn oo_seq(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 25 || !bytes.starts_with(b"SUPDBMA") {
        return None;
    }
    Some(u64::from_le_bytes(bytes[9..17].try_into().unwrap()))
}

/// A plain version-3 slot's covered sequence and names, each with its
/// pending flag.
fn oo_names(bytes: &[u8]) -> (u64, Vec<(String, bool)>) {
    assert_eq!(&bytes[..9], b"SUPDBMAN\x03", "a plain slot of this version");
    let len = u32::from_le_bytes(bytes[17..21].try_into().unwrap()) as usize;
    let body = &bytes[25..25 + len];
    let covered = u64::from_le_bytes(body[..8].try_into().unwrap());
    let n = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
    let (mut p, mut out) = (12usize, Vec::with_capacity(n));
    for _ in 0..n {
        let l = u16::from_le_bytes(body[p..p + 2].try_into().unwrap()) as usize;
        p += 2;
        out.push((
            String::from_utf8(body[p..p + l].to_vec()).unwrap(),
            body[p + l] & 1 != 0,
        ));
        p += l + 1;
    }
    (covered, out)
}

/// The two slots' paths, newest first by their sequence.
fn oo_by_age(d: &Path) -> [PathBuf; 2] {
    let seq = |s: &str| std::fs::read(d.join(s)).ok().and_then(|b| oo_seq(&b));
    if seq(OO_SLOTS[0]) >= seq(OO_SLOTS[1]) {
        [d.join(OO_SLOTS[0]), d.join(OO_SLOTS[1])]
    } else {
        [d.join(OO_SLOTS[1]), d.join(OO_SLOTS[0])]
    }
}

/// One byte of a slot changed inside what its check covers: the covered
/// sequence of a plain slot, a byte of a sealed one's ciphertext.
fn oo_damage(slot: &Path) {
    let mut b = std::fs::read(slot).unwrap();
    let at = if b.starts_with(b"SUPDBMAN") { 25 } else { 40 };
    b[at] ^= 0x5A;
    std::fs::write(slot, &b).unwrap();
}

/// The window a publish torn by a crash leaves, staged by hand: the slot
/// that does not hold the newest manifest written with the newest's bytes
/// under the next sequence, which its check refuses.
fn oo_tear_next(d: &Path) {
    let [newest, other] = oo_by_age(d);
    let mut b = std::fs::read(&newest).unwrap();
    let seq = oo_seq(&b).unwrap() + 1;
    b[9..17].copy_from_slice(&seq.to_le_bytes());
    std::fs::write(&other, &b).unwrap();
}

/// The open of `d` refused with `words`, every file left as it was.
fn oo_refused(d: &Path, opts: &Options, words: &str, tag: &str) {
    let before = files_of(d);
    match Db::open(d, opts.clone()) {
        Ok(_) => panic!("{tag}: the open answered Ok where it had to refuse ({words})"),
        Err(e) => assert!(
            e.to_string().contains(words),
            "{tag}: the refusal says why ({words}): {e}"
        ),
    }
    let after = files_of(d);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "{tag}: a refused open leaves the directory's names as they were"
    );
    assert!(
        before == after,
        "{tag}: a refused open leaves every file's bytes as they were"
    );
}

/// Every key of `model` reads its values, in `db`.
fn oo_holds(db: &Reader, model: &BTreeMap<Vec<u8>, Vec<Vec<u8>>>, tag: &str) {
    for (k, want) in model {
        assert_eq!(
            &read_vec(db, k),
            want,
            "{tag}: acknowledged key {} reads back",
            String::from_utf8_lossy(k)
        );
    }
}

/// An ordered load goes into a direct run and never into the log; the
/// flush closes it, the slot naming its segment is synced, and the run's
/// temp name is retired behind it. With that slot damaged, the slot
/// before it -- the birth slot -- names nothing, and the run's segment,
/// the only copy of its records, holds keys nothing it names holds. The
/// open refuses, and the store whose slot is put back opens whole.
///
/// And the same with the run's first key, or its last, written again after
/// the flush, by a commit the log holds and the store killed after it: the
/// replayed log holds that key, and is no holder of a run's records, which
/// in the clear never go to it. And the same over a segment flushed
/// before the run, which the slot before names: a segment that holds
/// keys, and none of them the run's. And a run of one key, whose segment
/// is no less the only copy of its record for holding one.
#[test]
fn a_flushed_run_is_not_lost_when_the_newest_slot_is_damaged_after_its_temp_retired() {
    for again in [None, Some(0), Some(1999)] {
        a_damaged_slot_over_a_flushed_run(again, false, 2000);
    }
    a_damaged_slot_over_a_flushed_run(None, true, 2000);
    a_damaged_slot_over_a_flushed_run(None, false, 1);
}

fn a_damaged_slot_over_a_flushed_run(again: Option<u32>, below: bool, n: u32) {
    let d = dir(&format!("oo-run-{again:?}-{below}-{n}"));
    let c = dir(&format!("oo-run-{again:?}-{below}-{n}-kill"));
    // Below, the run's flush leaves its segment a piece, so the partition
    // the slot before names stands under the name that slot gives it.
    let opts = Options {
        partition_on_flush: !below,
        flush_schedules: !below,
        drain_names_partition: !below,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    if below {
        for k in 0..2000u32 {
            let k = format!("a-{k:06}").into_bytes();
            db.append(&k, b"below");
            model.insert(k, vec![b"below".to_vec()]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
    }
    for k in 0..n {
        db.append(key(k).as_bytes(), b"acked");
        model.insert(key(k).into_bytes(), vec![b"acked".to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    if let Some(k) = again {
        db.settle().unwrap();
        db.put(key(k).as_bytes(), b"again");
        model.insert(key(k).into_bytes(), vec![b"again".to_vec()]);
        db.commit().unwrap();
        copy_store(&d, &c);
        drop(db);
    } else {
        db.close().unwrap();
        copy_store(&d, &c);
    }
    let d = c;
    assert!(!open_run(&d), "premise: the run's temp name retired");
    let [newest, older] = oo_by_age(&d);
    let named = |p: &Path| oo_names(&std::fs::read(p).unwrap()).1;
    let segs = seg_names(&d);
    assert!(
        segs.iter()
            .any(|n| !named(&older).iter().any(|(o, _)| o == n)),
        "premise: a segment only the newest slot names: {segs:?}"
    );
    assert!(
        !below || !named(&older).is_empty(),
        "premise: the slot before names the segment below the run"
    );
    let saved = std::fs::read(&newest).unwrap();
    oo_damage(&newest);
    oo_refused(&d, &opts, OO_KEYS, "run");
    std::fs::write(&newest, &saved).unwrap();
    let db = Db::open(&d, opts).expect("the store with its slot put back opens");
    oo_holds(&db, &model, "run, repaired");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(dir(&format!("oo-run-{again:?}-{below}-{n}")));
}

/// A refusal made once the log is replayed leaves the log as it found it.
/// A buffered store's log in three files, each rotation leaving the file
/// before it loose, and a power cut that took the first file's frames, so
/// replay ends where the second begins and the third holds a tail the cut
/// came before; beside it a flushed run whose slot is damaged, which the
/// open refuses. The files after the end of the log are removed only once
/// the open has decided the store is what it reads.
#[test]
fn a_refused_open_leaves_the_log_files_its_replay_ended_before() {
    let d = dir("oo-loose");
    let c = dir("oo-loose-kill");
    let opts = rotating();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..200u32 {
        db.append(key(k).as_bytes(), b"acked");
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(!open_run(&d), "premise: the run's temp name retired");
    // Frames far larger than what they put in the table, so the log
    // rotates twice before the table is a seal's worth.
    let mut i = 0u32;
    while db.seal_waits().wal_rotations < 2 {
        put_commit(&mut db, &format!("r{i:07}"), "x").unwrap();
        i += 1;
        assert!(i < 100_000, "the log rotated twice");
    }
    put_commit(&mut db, "s0000000", "x").unwrap();
    copy_store(&d, &c);
    drop(db);
    let wals = wal_names(&c);
    assert_eq!(wals.len(), 3, "premise: three log files: {wals:?}");
    let [newest, older] = oo_by_age(&c);
    let (_, new_names) = oo_names(&std::fs::read(&newest).unwrap());
    let (_, old_names) = oo_names(&std::fs::read(&older).unwrap());
    assert!(
        new_names.iter().any(|n| !old_names.contains(n)),
        "premise: the run's segment only the newest slot names: {new_names:?} beside {old_names:?}"
    );
    // The power cut: the first file's frames, which no sync covered, gone.
    std::fs::OpenOptions::new()
        .write(true)
        .open(c.join(&wals[0]))
        .unwrap()
        .set_len(WAL_HEAD)
        .unwrap();
    oo_damage(&newest);
    oo_refused(&c, &opts, OO_KEYS, "loose");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

/// The same three log files and the same cut, with the slot whole: the
/// open takes the store, the log ends where the second file begins, and
/// the third file, whose records continue a tail the cut took, is removed
/// once the open has decided, so the open after a commit reads no gap.
#[test]
fn an_open_removes_the_log_files_after_the_end_it_replayed_to() {
    let d = dir("oo-loose-ok");
    let c = dir("oo-loose-ok-kill");
    let opts = rotating();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut i = 0u32;
    while db.seal_waits().wal_rotations < 2 {
        put_commit(&mut db, &format!("r{i:07}"), "x").unwrap();
        i += 1;
        assert!(i < 100_000, "the log rotated twice");
    }
    put_commit(&mut db, "s0000000", "x").unwrap();
    copy_store(&d, &c);
    drop(db);
    let wals = wal_names(&c);
    assert_eq!(wals.len(), 3, "premise: three log files: {wals:?}");
    std::fs::OpenOptions::new()
        .write(true)
        .open(c.join(&wals[0]))
        .unwrap()
        .set_len(WAL_HEAD)
        .unwrap();
    let mut db = Db::open(&c, opts.clone()).expect("the cut store opens");
    let after = wal_names(&c);
    assert!(
        !after.contains(&wals[2]),
        "the log file after the end the replay reached is removed: {after:?}"
    );
    put_commit(&mut db, "t0000000", "x").unwrap();
    db.sync().unwrap();
    drop(db);
    let db = Db::open(&c, opts).expect("the store opens again after a commit");
    assert_eq!(read_vec(&db, b"t0000000"), vec![b"x".to_vec()]);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

/// A store that recycled its log killed with a spare standing, then
/// opened recycling none: the spare is nobody's pool and the open removes
/// it, once it has decided the store is what it reads.
#[test]
fn an_open_that_recycles_no_log_removes_the_spare_it_finds() {
    let d = dir("oo-spare");
    let opts = Options {
        recycle_wal: true,
        seal_bytes: 32 << 10,
        l0_trigger: 2,
        ..Options::default()
    };
    let spares = |d: &Path| -> Vec<String> {
        files_of(d)
            .into_keys()
            .filter(|n| n.starts_with("spare-"))
            .collect()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut i = 0u32;
    while spares(&d).is_empty() {
        for _ in 0..400 {
            db.append(format!("k{:06}", i % 5000).as_bytes(), b"v");
            i += 1;
        }
        db.commit().unwrap();
        db.settle().unwrap();
        assert!(i < 2_000_000, "premise: a retired log kept as a spare");
    }
    std::mem::forget(db);
    assert!(!spares(&d).is_empty(), "premise: the kill leaves the spare");
    let db = Db::open(
        &d,
        Options {
            recycle_wal: false,
            ..opts
        },
    )
    .unwrap();
    assert!(
        spares(&d).is_empty(),
        "an open that recycles nothing removes the spare: {:?}",
        spares(&d)
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A partition, then every key rewritten by commits through the log until
/// it rotates, and the close: its drain seals the rewrites and its merge
/// folds them into a new partition, unlinking the files the slot before
/// it names. With the newest slot damaged the open falls back to a slot
/// that names files the directory does not hold, and refuses before it
/// removes anything -- not at a segment it fails to find once it has
/// swept the newest slot's partition away.
fn a_fallback_onto_a_slot_naming_files_gone(tag: &str, base: Options) {
    let d = dir(&format!("oo-gone-{tag}"));
    let opts = Options {
        direct_ingest: false,
        seal_bytes: 256 << 10,
        ..base
    };
    let k12 = |k: u32| format!("key-{k:012}").into_bytes();
    let val = |k: u32, round: u32| format!("{round:04}-{k:0>96}").into_bytes();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for k in 0..2000 {
        db.put(&k12(k), &val(k, 0));
        model.insert(k12(k), vec![val(k, 0)]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    let rotations = db.seal_waits().wal_rotations;
    let (mut k, mut round) = (0u32, 1u32);
    while db.seal_waits().wal_rotations == rotations {
        for _ in 0..100 {
            db.put(&k12(k), &val(k, round));
            model.insert(k12(k), vec![val(k, round)]);
            k += 1;
            if k == 2000 {
                k = 0;
                round += 1;
            }
        }
        db.commit().unwrap();
    }
    db.close().unwrap();
    let [newest, older] = oo_by_age(&d);
    if tag == "plain" {
        let gone: Vec<String> = oo_names(&std::fs::read(&older).unwrap())
            .1
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| !d.join(n).exists())
            .collect();
        assert!(!gone.is_empty(), "premise: the older slot names files gone");
    }
    let saved = std::fs::read(&newest).unwrap();
    oo_damage(&newest);
    oo_refused(&d, &opts, OO_GONE, tag);
    std::fs::write(&newest, &saved).unwrap();
    let db = Db::open(&d, opts).expect("the store with its slot put back opens");
    oo_holds(&db, &model, &format!("{tag}, repaired"));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_fallback_onto_a_slot_naming_files_gone_is_refused_before_anything_is_removed() {
    a_fallback_onto_a_slot_naming_files_gone("plain", Options::default());
}

#[test]
fn a_fallback_onto_a_slot_naming_files_gone_is_refused_under_a_cipher() {
    a_fallback_onto_a_slot_naming_files_gone("cipher", under_cipher());
}

/// The newest slot whole and the slot before it whole too, no fall back at
/// all, and a segment the newest names removed from the directory: damage
/// from outside the engine, which removes nothing a durable slot names.
/// The open refuses by the missing name and removes nothing, and the store
/// whose file is put back opens whole.
#[test]
fn an_open_whose_newest_slot_names_a_file_gone_refuses_and_leaves_every_file() {
    let d = dir("oo-gone-newest");
    let opts = oo_slot_opts(Options::default());
    let model = oo_populated(&d, &opts, 5_000);
    let [newest, older] = oo_by_age(&d);
    let names = oo_names(&std::fs::read(&newest).unwrap()).1;
    assert!(
        oo_seq(&std::fs::read(&older).unwrap()).is_some(),
        "premise: the slot before the newest reads too"
    );
    let gone = names
        .iter()
        .map(|(n, _)| n.clone())
        .find(|n| n.starts_with("pcs-"))
        .expect("premise: the newest slot names a piece");
    let saved = std::fs::read(d.join(&gone)).unwrap();
    std::fs::remove_file(d.join(&gone)).unwrap();
    oo_refused(&d, &opts, OO_GONE, "newest");
    std::fs::write(d.join(&gone), &saved).unwrap();
    let db = Db::open(&d, opts).expect("the store with its file put back opens");
    oo_holds(&db, &model, "newest, repaired");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Under a cipher and `Options::manifest_pending`, a store killed with a
/// seal held between its pending slot and its durable landing: the copy
/// opens, takes the pending name (its envelope was written and synced
/// before the slot that names it), writes the slot naming it durable, and
/// opens again with every acknowledged write.
#[test]
fn an_open_under_a_cipher_adopts_a_pending_segment_and_opens_again() {
    let d = dir("oo-pend-cipher");
    let c = dir("oo-pend-cipher-kill");
    let opts = Options {
        manifest_pending: true,
        direct_ingest: false,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        ..under_cipher()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for k in (0..3000u32).rev() {
        db.put(key(k).as_bytes(), b"v");
        model.insert(key(k).into_bytes(), vec![b"v".to_vec()]);
    }
    db.commit().unwrap();
    let seq = |d: &Path| {
        OO_SLOTS
            .iter()
            .filter_map(|s| std::fs::read(d.join(s)).ok().and_then(|b| oo_seq(&b)))
            .max()
            .unwrap_or(0)
    };
    let at = seq(&d);
    db.hold_seal_durable(true);
    db.seal().unwrap();
    wait_for("the seal's pending slot", || seq(&d) > at);
    copy_store(&d, &c);
    db.hold_seal_durable(false);
    drop(db);
    let copied = seq(&c);
    let db = Db::open(&c, opts.clone()).expect("the crash copy opens");
    oo_holds(&db, &model, "cipher pending: after the kill");
    assert!(
        seq(&c) > copied,
        "premise: the open wrote a slot over the pending one it took"
    );
    drop(db);
    let db = Db::open(&c, opts).expect("the store the open wrote opens again");
    oo_holds(&db, &model, "cipher pending: reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

/// The windows a fall back has to open through, each staged as a crash
/// leaves it -- the store's files copied with a job held at its landing --
/// and then the publish after it torn: a seal's segments renamed into
/// place and unnamed, with the log that holds their writes; a run's
/// segment linked under its piece name beside its temp name; a merge's
/// outputs renamed into place beside the inputs the slot before names; and
/// a merge's inputs left beside the outputs a durable slot names, their
/// first and last keys among those the merge dropped, and the same beside
/// an older partition the merge left alone; and under a buffered
/// policy, a seal's segments synced beside a log whose frames for them a
/// power cut took, the publish of their landing torn -- the store as it
/// was before those writes, which no sync acknowledged; a run's segment
/// whose superblock the power cut took, under both its names, so that
/// neither opens as a segment; and a merge's output of no record, every
/// key of its partition deleted. Each opens with every acknowledged
/// write, the unnamed files swept.
#[test]
fn the_windows_a_torn_publish_leaves_open_through_the_slot_before() {
    for case in [
        "seal",
        "run",
        "run-torn",
        "merge",
        "empty",
        "inputs",
        "inputs-old",
        "buffered",
    ] {
        let d = dir(&format!("oo-win-{case}"));
        let c = dir(&format!("oo-win-{case}-kill"));
        let opts = Options {
            partition_on_flush: case == "seal" || case.starts_with("run") || case == "buffered",
            sync: if case == "buffered" {
                SyncPolicy::EveryN(u32::MAX)
            } else {
                SyncPolicy::Always
            },
            flush_schedules: case == "merge" || case == "empty" || case.starts_with("inputs"),
            seal_bytes: 1 << 20,
            l0_trigger: 1000,
            ..Options::default()
        };
        let mut db = Db::create(&d, opts.clone()).unwrap();
        let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
        fn put(db: &mut Db, model: &mut BTreeMap<Vec<u8>, Vec<Vec<u8>>>, k: Vec<u8>, v: &[u8]) {
            db.put(&k, v);
            model.insert(k, vec![v.to_vec()]);
        }
        if case == "inputs-old" {
            // A partition below the rest, older than every segment after
            // it and left alone by the merge below.
            for k in (0..2000u32).rev() {
                put(&mut db, &mut model, format!("a-{k:06}").into_bytes(), b"a");
            }
            db.commit().unwrap();
            db.flush().unwrap();
            db.settle().unwrap();
        }
        // A partition first, from writes through the log.
        for k in (0..2000u32).rev() {
            put(&mut db, &mut model, key(k).into_bytes(), b"v0");
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let before = seg_names(&d);
        // The log as the flush left it durable, for the buffered case's cut.
        let logged: Vec<(String, u64)> = wal_names(&d)
            .into_iter()
            .map(|w| {
                let n = std::fs::metadata(d.join(&w)).unwrap().len();
                (w, n)
            })
            .collect();
        match case {
            "seal" => {
                for k in (0..2000u32).step_by(3) {
                    put(
                        &mut db,
                        &mut model,
                        format!("new-{k:06}").into_bytes(),
                        b"s",
                    );
                    put(&mut db, &mut model, key(k).into_bytes(), b"v1");
                }
                db.commit().unwrap();
                db.hold_seal_durable(true);
                db.seal().unwrap();
                wait_for("the seal's segments in place", || {
                    seg_names(&d).iter().any(|n| !before.contains(n))
                });
            }
            "buffered" => {
                // Writes no sync covers, sealed: the seal's segments synced,
                // and the publish of their landing the one a power cut tore.
                for k in (0..2000u32).step_by(3) {
                    db.put(format!("new-{k:06}").as_bytes(), b"s");
                    db.put(key(k).as_bytes(), b"v1");
                }
                db.commit().unwrap();
                db.hold_seal_durable(true);
                db.seal().unwrap();
                wait_for("the seal's segments in place", || {
                    seg_names(&d).iter().any(|n| !before.contains(n))
                });
            }
            "run" | "run-torn" => {
                for k in 0..3000u32 {
                    let k = format!("run-{k:06}").into_bytes();
                    db.append(&k, b"r");
                    model.insert(k, vec![b"r".to_vec()]);
                }
                db.commit().unwrap();
                assert!(open_run(&d), "premise: an ordered run is open");
                db.hold_seal_durable(true);
                db.seal().unwrap();
                wait_for("the run's segment in place", || {
                    seg_names(&d).iter().any(|n| !before.contains(n))
                });
                assert!(open_run(&d), "premise: the run's temp name stands");
            }
            _ => {
                if case == "merge" {
                    for k in (0..2000u32).step_by(3) {
                        put(&mut db, &mut model, key(k).into_bytes(), b"v1");
                    }
                } else if case == "empty" {
                    // Every key of the partition deleted: the merge's
                    // output is a partition of no record.
                    for k in 0..2000u32 {
                        db.delete(key(k).as_bytes());
                        model.insert(key(k).into_bytes(), Vec::new());
                    }
                } else {
                    // The partition's first and last keys deleted, which the
                    // merge drops from its outputs with what they masked.
                    for k in [0u32, 1999] {
                        db.delete(key(k).as_bytes());
                        model.insert(key(k).into_bytes(), Vec::new());
                    }
                }
                db.commit().unwrap();
                db.hold_merge_landing(true);
                db.flush().unwrap();
                wait_for("the merge at its landing hold", || {
                    db.merge_waits_at_landing()
                });
            }
        }
        copy_store(&d, &c);
        db.hold_seal_durable(false);
        db.hold_merge_landing(false);
        if case.starts_with("inputs") {
            // The merge landed durable and the crash before it unlinked
            // its inputs: its outputs named, and the inputs put back from
            // the copy taken while it was held.
            db.settle().unwrap();
            let b = dir("oo-win-inputs-landed");
            copy_store(&d, &b);
            for (n, bytes) in files_of(&c) {
                let seg = ["par-", "pcs-", "seg-", "ord-"]
                    .iter()
                    .any(|p| n.starts_with(p));
                if seg && !b.join(&n).exists() {
                    std::fs::write(b.join(&n), bytes).unwrap();
                }
            }
            let _ = std::fs::remove_dir_all(&c);
            std::fs::rename(&b, &c).unwrap();
        }
        drop(db);
        let unnamed: Vec<String> = {
            let (_, names) = oo_names(&std::fs::read(&oo_by_age(&c)[0]).unwrap());
            seg_names(&c)
                .into_iter()
                .filter(|n| !names.iter().any(|(m, _)| m == n))
                .collect()
        };
        assert!(
            !unnamed.is_empty(),
            "{case}: premise: segments no slot names stand"
        );
        if case == "buffered" {
            // The power cut: the log's frames past the flush never synced.
            for w in wal_names(&c) {
                let n = logged.iter().find(|(l, _)| *l == w).map_or(8, |l| l.1);
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(c.join(&w))
                    .unwrap()
                    .set_len(n)
                    .unwrap();
            }
        }
        if case == "run-torn" {
            // The power cut before the close's fsync: the run's segment is
            // its temp file under a second name, and its superblock, written
            // at the close, never reached the device. Neither name opens as
            // a segment; the temp name's records, synced at each commit, are
            // the run.
            let torn: Vec<String> = unnamed
                .iter()
                .cloned()
                .chain(
                    std::fs::read_dir(&c)
                        .unwrap()
                        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .filter(|n| n.starts_with("direct-") && n.ends_with(".tmp")),
                )
                .collect();
            assert!(
                torn.len() >= 2,
                "{case}: premise: the run's two names: {torn:?}"
            );
            for n in &torn {
                use std::os::unix::fs::FileExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(c.join(n))
                    .unwrap()
                    .write_all_at(&[0u8; 4096], 0)
                    .unwrap();
            }
        }
        oo_tear_next(&c);
        let db = Db::open(&c, opts.clone())
            .unwrap_or_else(|e| panic!("{case}: the window refused: {e}"));
        oo_holds(&db, &model, case);
        if case == "buffered" {
            assert!(
                read_vec(&db, b"new-000000").is_empty(),
                "{case}: the store as it was before the writes no sync covered"
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&c);
    }
}

/// Small partitions, so a load of tens of thousands of keys is several
/// `par-` files.
fn oo_slot_opts(base: Options) -> Options {
    Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 10),
        ..base
    }
}

/// A store of partitions and a level-0 piece: an ordered load flushed into
/// partitions, then every seventh key replaced through the log and sealed
/// into a piece, every write acknowledged. Dropped, not closed, so the
/// piece stands. Returns what every key reads.
fn oo_populated(d: &Path, opts: &Options, n: u32) -> BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut model = BTreeMap::new();
    let mut db = Db::create(d, opts.clone()).unwrap();
    for k in 0..n {
        db.append(key(k).as_bytes(), b"v0");
        model.insert(key(k).into_bytes(), vec![b"v0".to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    for k in (0..n).step_by(7) {
        db.put(key(k).as_bytes(), b"v1");
        model.insert(key(k).into_bytes(), vec![b"v1".to_vec()]);
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    drop(db);
    model
}

/// Both slots emptied, or removed, in a store whose data is in `par-` and
/// `pcs-` files -- damage from outside the engine, which never empties a
/// slot: an open reads no manifest, and the scan kept for stores from
/// before manifests would take `seg-` files alone as live and remove every
/// partition and piece. Refused, every file left; the store whose slots
/// are put back opens whole. And the same with the partitions gone too,
/// so a piece alone beside the empty slots is what the refusal sees.
#[test]
fn an_open_with_no_readable_slot_beside_partitions_refuses_and_leaves_every_file() {
    for how in ["emptied", "removed", "pieces"] {
        let d = dir(&format!("oo-noslot-{how}"));
        let opts = oo_slot_opts(Options::default());
        let model = oo_populated(&d, &opts, 20_000);
        let names = seg_names(&d);
        assert!(
            names.iter().any(|n| n.starts_with("par-"))
                && names.iter().any(|n| n.starts_with("pcs-")),
            "{how}: premise: partitions and a piece: {names:?}"
        );
        let saved: Vec<Vec<u8>> = OO_SLOTS
            .iter()
            .map(|s| std::fs::read(d.join(s)).unwrap())
            .collect();
        for s in OO_SLOTS {
            match how {
                "removed" => std::fs::remove_file(d.join(s)).unwrap(),
                _ => std::fs::write(d.join(s), b"").unwrap(),
            }
        }
        if how == "pieces" {
            for n in names.iter().filter(|n| n.starts_with("par-")) {
                std::fs::remove_file(d.join(n)).unwrap();
            }
        }
        oo_refused(&d, &opts, OO_NO_SLOT, how);
        if how != "pieces" {
            for (s, b) in OO_SLOTS.iter().zip(&saved) {
                std::fs::write(d.join(s), b).unwrap();
            }
            let db = Db::open(&d, opts).expect("the store with its slots put back opens");
            oo_holds(&db, &model, how);
            drop(db);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Control, as the open always did: both slots zeroes, which fail their
/// magic, are refused before anything is listed.
#[test]
fn an_open_with_both_slots_zeroed_refuses_and_leaves_every_file() {
    let d = dir("oo-zeroed");
    let opts = oo_slot_opts(Options::default());
    let model = oo_populated(&d, &opts, 20_000);
    let saved: Vec<Vec<u8>> = OO_SLOTS
        .iter()
        .map(|s| std::fs::read(d.join(s)).unwrap())
        .collect();
    for (s, b) in OO_SLOTS.iter().zip(&saved) {
        std::fs::write(d.join(s), vec![0u8; b.len().max(64)]).unwrap();
    }
    oo_refused(&d, &opts, "manifest magic is wrong", "zeroed");
    for (s, b) in OO_SLOTS.iter().zip(&saved) {
        std::fs::write(d.join(s), b).unwrap();
    }
    let db = Db::open(&d, opts).expect("the store with its slots put back opens");
    oo_holds(&db, &model, "zeroed");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Under a cipher a slot is the only place a segment's envelope is named
/// and keyed, and an open sweeps every envelope no slot it read names:
/// with both slots emptied, every envelope of the store. Refused, every
/// file left.
#[test]
fn an_open_with_no_readable_slot_beside_envelopes_refuses_under_a_cipher() {
    let d = dir("oo-noslot-cipher");
    let opts = oo_slot_opts(under_cipher());
    let model = oo_populated(&d, &opts, 5_000);
    let saved: Vec<Vec<u8>> = OO_SLOTS
        .iter()
        .map(|s| std::fs::read(d.join(s)).unwrap())
        .collect();
    assert!(
        files_of(&d).keys().any(|n| n.starts_with("x-")),
        "premise: envelopes"
    );
    for s in OO_SLOTS {
        std::fs::write(d.join(s), b"").unwrap();
    }
    oo_refused(&d, &opts, OO_NO_SLOT, "cipher");
    for (s, b) in OO_SLOTS.iter().zip(&saved) {
        std::fs::write(d.join(s), b).unwrap();
    }
    let db = Db::open(&d, opts).expect("the store with its slots put back opens");
    oo_holds(&db, &model, "cipher");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// How a power cut leaves a file whose writes nothing fsynced.
#[derive(Clone, Copy, Debug)]
enum OoTear {
    /// A run of its pages never written back: 4 KiB of zeroes at 8 KiB.
    Run,
    /// None of it written back.
    Empty,
}

fn oo_tear(path: &Path, how: OoTear) {
    use std::os::unix::fs::FileExt;
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    match how {
        OoTear::Run => {
            if f.metadata().unwrap().len() > 8192 {
                f.write_all_at(&[0u8; 4096], 8192).unwrap();
            } else {
                f.set_len(0).unwrap();
            }
        }
        OoTear::Empty => f.set_len(0).unwrap(),
    }
}

/// The first page of `path` written again as it is, so that it waits for
/// writeback as a write a kill came after would.
fn oo_dirty(path: &Path) {
    use std::os::unix::fs::FileExt;
    let b = std::fs::read(path).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.write_all_at(&b[..b.len().min(4096)], 0).unwrap();
}

/// A process killed between a seal's pending slot and its fsync leaves the
/// seal's segments whole in the page cache and synced by nobody -- the kill
/// taken, as it may be, before the sync helper's directory fsync, so the
/// segments' names are no more durable than their bytes. The open after
/// it reads them whole and writes a slot naming them durable, which covers
/// the log frames that are the commits' only durable copy. A power cut
/// after that open loses whatever of the segments, their ordered indexes
/// and their names no fsync covered since, and every commit acknowledged
/// before the kill has to read back. What the open fsynced is its own
/// record (`Db::open_synced_files`), held to the calls that made the
/// fsyncs (`Db::trace_dir_io`), and where the kernel says to the page
/// cache's word on each file's first page.
fn a_pending_segment_adopted_by_an_open(how: OoTear, ord_durable: bool, pieces: bool) {
    let tag = format!("{how:?}-ord{ord_durable}-pieces{pieces}");
    let d = dir(&format!("oo-pend-{tag}"));
    let opts = Options {
        manifest_pending: true,
        ord_durable,
        direct_ingest: false,
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 10),
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let n = if pieces { 20_000 } else { 3000 };
    if pieces {
        // Partitions first, so the seal below cuts its table into a piece
        // over each of several.
        for k in (0..n).rev() {
            db.put(key(k).as_bytes(), b"v0");
            model.insert(key(k).into_bytes(), vec![b"v0".to_vec()]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
    }
    for k in (0..n).rev().step_by(if pieces { 5 } else { 1 }) {
        db.put(key(k).as_bytes(), b"v");
        model.insert(key(k).into_bytes(), vec![b"v".to_vec()]);
    }
    db.commit().unwrap();
    db.hold_seal_durable(true);
    db.seal().unwrap();
    let pending = || -> Vec<String> {
        let b = std::fs::read(&oo_by_age(&d)[0]).unwrap();
        oo_names(&b)
            .1
            .into_iter()
            .filter(|(_, p)| *p)
            .map(|(n, _)| n)
            .collect()
    };
    wait_for("the seal's pending slot", || !pending().is_empty());
    let pend = pending();
    assert!(
        !pieces || pend.len() >= 2,
        "{tag}: premise: the seal is several pieces: {pend:?}"
    );
    // The kill: the seal thread parked before its fsync for good; a drop
    // would join it and let it sync.
    std::mem::forget(db);
    let ords: Vec<String> = pend
        .iter()
        .map(|n| {
            let f: Vec<&str> = n.split('-').collect();
            format!("ord-{}-{}.oidx", f[1], f[2].trim_end_matches(".sup"))
        })
        .collect();
    let files: Vec<&String> = pend.iter().chain(&ords).collect();
    for f in &files {
        assert!(d.join(f).is_file(), "{tag}: premise: {f} in place");
        oo_dirty(&d.join(f));
    }
    let kernel = os_page_unwritten(&d.join(&pend[0])).is_some();
    // The calls themselves, process-wide: left on, since tests beside this
    // one may be reading it too, and read for this directory alone.
    let from = Db::trace_dir_io(true).len();
    let db = Db::open(&d, opts.clone()).unwrap();
    let calls: Vec<(&str, PathBuf)> = Db::trace_dir_io(true)
        .split_off(from)
        .into_iter()
        .filter(|(what, p)| match *what {
            "sync_dir" => p == &d,
            _ => p.parent() == Some(d.as_path()),
        })
        .collect();
    oo_holds(&db, &model, &format!("{tag}: after the kill"));
    let synced = db.open_synced_files();
    let [newest, _] = oo_by_age(&d);
    let durable = oo_names(&std::fs::read(&newest).unwrap())
        .1
        .iter()
        .any(|(n, p)| pend.contains(n) && !p);
    // The order: the slot that names the segments durable covers the log
    // frames that were their writes' only durable copy, so every pending
    // segment, its durable index and the directory's entries were durable
    // before it -- a power cut between that slot's fdatasync and any of
    // theirs leaves a durable slot naming what may be gone.
    assert!(durable, "{tag}: premise: the open adopted {pend:?}");
    {
        let slot = newest.file_name().unwrap().to_str().unwrap().to_string();
        let at = synced
            .iter()
            .position(|s| *s == slot)
            .unwrap_or_else(|| panic!("{tag}: the open records its slot {slot}: {synced:?}"));
        let owed = pend
            .iter()
            .chain(ords.iter().filter(|_| ord_durable))
            .cloned()
            .chain(std::iter::once(".".to_string()));
        for f in owed {
            assert!(
                synced[..at].contains(&f),
                "{tag}: {f} fsynced before the slot {slot} named it durable: {synced:?}"
            );
        }
        // The same order read off the calls, which a record the open keeps
        // of its own fsyncs cannot vouch for: a directory fsync that
        // returned before the slot was opened for the write.
        let write = calls
            .iter()
            .position(|(what, p)| *what == "open_slot" && *p == newest)
            .unwrap_or_else(|| panic!("{tag}: the open wrote {slot}: {calls:?}"));
        assert!(
            calls[..write].iter().any(|(what, _)| *what == "sync_dir"),
            "{tag}: the directory fsynced before the slot {slot} was written: {calls:?}"
        );
        // Both slots made durable as the open found them before it wrote
        // either: the one it took and the one it is about to write over.
        for s in OS_SLOTS {
            assert!(
                calls[..write]
                    .iter()
                    .any(|(what, p)| *what == "sync_slot" && *p == d.join(s)),
                "{tag}: {s} fdatasynced before the slot {slot} was written: {calls:?}"
            );
        }
        // And each file's fsync, returned before the slot was opened, read
        // off the call that made it: the record above is the open's word,
        // and the kernel's word below is read only by root.
        for f in pend.iter().chain(ords.iter().filter(|_| ord_durable)) {
            assert!(
                calls[..write]
                    .iter()
                    .any(|(what, p)| *what == "sync_file" && *p == d.join(f)),
                "{tag}: {f} fsynced before the slot {slot} was written: {calls:?}"
            );
        }
    }
    if kernel {
        for f in &files {
            let unwritten = os_page_unwritten(&d.join(f));
            if synced.contains(f) {
                assert_eq!(
                    unwritten,
                    Some(false),
                    "{tag}: {f}, which the open says it fsynced, is on the device"
                );
            } else if !ord_durable && f.starts_with("ord-") {
                assert_eq!(
                    unwritten,
                    Some(true),
                    "{tag}: the control, an index no fsync covered, still waits for writeback"
                );
            }
        }
    } else {
        eprintln!("{tag}: the kernel does not say which pages are dirty to this process");
    }
    // The power cut: nothing of this store runs on, a file no fsync covered
    // comes back torn, and a name no directory fsync covered is gone.
    std::mem::forget(db);
    for f in &files {
        if !synced.contains(f) {
            oo_tear(&d.join(f), how);
        }
    }
    if !synced.iter().any(|s| s == ".") {
        for f in &files {
            std::fs::remove_file(d.join(f)).unwrap();
        }
    }
    let state = format!(
        "{tag}: a power cut after the open (named {pend:?} durable: {durable}; \
         it fsynced {synced:?})"
    );
    let db = Db::open(&d, opts)
        .unwrap_or_else(|e| panic!("{state}: the store refused its acknowledged commits: {e}"));
    oo_holds(&db, &model, &state);
    drop(db);
    // An ordered index torn by the cut is composed again at the open, so
    // the reads above cannot tell whether the open synced one; a store
    // that keeps them durable (`ord_durable`) has each in the open's
    // record, which the calls above, and where it says the kernel's word,
    // hold to the device.
    if ord_durable {
        for o in &ords {
            assert!(
                synced.contains(o),
                "{tag}: the open fsynced the durable index {o}: {synced:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The open's fsync of the pending segment it adopts fails
/// (`Options::fail_open_pending_sync`): the open answers the failure, and
/// no slot it wrote names the segment durable, since the slot that does
/// is written only once the fsyncs have returned. Both slots are as the
/// kill left them, and the store opens with every write after.
#[test]
fn an_open_whose_pending_fsync_fails_names_nothing_durable() {
    let d = dir("oo-pend-fail");
    let opts = Options {
        manifest_pending: true,
        direct_ingest: false,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for k in (0..3000u32).rev() {
        db.put(key(k).as_bytes(), b"v");
        model.insert(key(k).into_bytes(), vec![b"v".to_vec()]);
    }
    db.commit().unwrap();
    db.hold_seal_durable(true);
    db.seal().unwrap();
    let pending = || -> Vec<String> {
        let b = std::fs::read(&oo_by_age(&d)[0]).unwrap();
        oo_names(&b)
            .1
            .into_iter()
            .filter(|(_, p)| *p)
            .map(|(n, _)| n)
            .collect()
    };
    wait_for("the seal's pending slot", || !pending().is_empty());
    let pend = pending();
    std::mem::forget(db);
    let slots = |d: &Path| -> Vec<Option<Vec<u8>>> {
        OO_SLOTS
            .iter()
            .map(|s| std::fs::read(d.join(s)).ok())
            .collect()
    };
    let before = slots(&d);
    match Db::open(
        &d,
        Options {
            fail_open_pending_sync: true,
            ..opts.clone()
        },
    ) {
        Ok(_) => panic!("the open answered Ok over a failed fsync of {pend:?}"),
        Err(e) => assert!(
            e.to_string().contains("fsync of the pending"),
            "the open answers the failed fsync: {e}"
        ),
    }
    for b in slots(&d).into_iter().flatten() {
        assert!(
            !oo_names(&b).1.iter().any(|(n, p)| pend.contains(n) && !p),
            "no slot names {pend:?} durable after its fsync failed"
        );
    }
    assert!(
        slots(&d) == before,
        "the slots are as the kill left them after the failed open"
    );
    let db = Db::open(&d, opts).expect("the store opens once the device answers");
    oo_holds(&db, &model, "after the failed open");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A power cut between a seal's pending slot and its fsync, under
/// `Options::ord_durable`: the slot's fdatasync returned, the segment's
/// pages reached the device by write-back and its ordered index's name
/// did not. The open's read of the segment back whole composes the index
/// again where it is missing and writes it synced, so the fsync owed it
/// before the slot that names the segment durable finds it there: the
/// open answers with every acknowledged write, and the index is back.
#[test]
fn an_open_adopts_a_pending_segment_whose_durable_index_a_power_cut_took() {
    let d = dir("oo-pend-noord");
    let opts = Options {
        manifest_pending: true,
        ord_durable: true,
        direct_ingest: false,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for k in (0..3000u32).rev() {
        db.put(key(k).as_bytes(), b"v");
        model.insert(key(k).into_bytes(), vec![b"v".to_vec()]);
    }
    db.commit().unwrap();
    db.hold_seal_durable(true);
    db.seal().unwrap();
    let pending = || -> Vec<String> {
        let b = std::fs::read(&oo_by_age(&d)[0]).unwrap();
        oo_names(&b)
            .1
            .into_iter()
            .filter(|(_, p)| *p)
            .map(|(n, _)| n)
            .collect()
    };
    wait_for("the seal's pending slot", || !pending().is_empty());
    let pend = pending();
    // The kill, and the store copied as it stands: the process's threads
    // go on in the original -- the landing opens the segment it published
    // and composes an index it finds missing -- where a killed process's
    // would not.
    std::mem::forget(db);
    let c = dir("oo-pend-noord-kill");
    copy_store(&d, &c);
    let d = c;
    let ords: Vec<String> = pend
        .iter()
        .map(|n| {
            let f: Vec<&str> = n.split('-').collect();
            format!("ord-{}-{}.oidx", f[1], f[2].trim_end_matches(".sup"))
        })
        .collect();
    for o in &ords {
        std::fs::remove_file(d.join(o))
            .unwrap_or_else(|e| panic!("premise: the seal wrote {o}: {e}"));
    }
    let db = Db::open(&d, opts.clone())
        .unwrap_or_else(|e| panic!("the store refused its acknowledged commits: {e}"));
    oo_holds(&db, &model, "a pending segment whose index was lost");
    let [newest, older] = oo_by_age(&d);
    let read = |p: &Path| oo_names(&std::fs::read(p).unwrap());
    assert!(
        read(&newest).1.iter().any(|(n, p)| pend.contains(n) && !p),
        "premise: the open adopted {pend:?}: newest {:?}, older {:?}, files {:?}",
        read(&newest),
        read(&older),
        files_of(&d).keys().collect::<Vec<_>>()
    );
    drop(db);
    for o in &ords {
        assert!(d.join(o).is_file(), "the open composed {o} again");
    }
    let db = Db::open(&d, opts).expect("the store opens again");
    oo_holds(&db, &model, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(dir("oo-pend-noord"));
}

#[test]
fn acknowledged_commits_survive_a_power_cut_after_an_open_that_adopted_a_pending_segment() {
    for how in [OoTear::Run, OoTear::Empty] {
        a_pending_segment_adopted_by_an_open(how, false, false);
    }
}

#[test]
fn an_open_that_adopts_a_pending_segment_makes_its_durable_index_durable() {
    for how in [OoTear::Run, OoTear::Empty] {
        a_pending_segment_adopted_by_an_open(how, true, false);
    }
}

#[test]
fn an_open_that_adopts_several_pending_pieces_makes_each_durable() {
    a_pending_segment_adopted_by_an_open(OoTear::Run, true, true);
}

// ---------------------------------------------------------------------
// other-slot: an open makes the manifest slot it does not take durable
// as the page cache holds it, beside the one it takes, before it removes
// or writes anything. A publish into a slot with its fdatasync owed (a
// drain's), then the next publish into the same slot -- the other holds
// the newest manifest known durable -- killed part way through its write:
// the page cache holds that slot torn, and the device may hold the owed
// publish in it whole, its writeback having run before the torn write
// dirtied the page again. The reopen passes over the torn slot, takes the
// other and sweeps what it does not name; a power cut before anything
// writes the torn image back must not leave the device's whole record the
// newest slot the next open reads.
// ---------------------------------------------------------------------

/// The calls the process-wide record holds from `from` on for files of
/// `d`: what an open of `d` made (`Db::trace_dir_io`).
fn osl_calls(d: &Path, from: usize) -> Vec<(&'static str, PathBuf)> {
    Db::trace_dir_io(true)
        .split_off(from)
        .into_iter()
        .filter(|(_, p)| p.parent() == Some(d))
        .collect()
}

/// The file names of `d` whose bytes differ, are gone or were made against
/// `found`.
fn osl_changed(found: &BTreeMap<String, Vec<u8>>, d: &Path) -> Vec<String> {
    let left = os_files(d);
    let mut out: Vec<String> = found
        .iter()
        .filter(|(n, b)| left.get(*n) != Some(*b))
        .map(|(n, _)| n.clone())
        .collect();
    out.extend(left.keys().filter(|n| !found.contains_key(*n)).cloned());
    out
}

fn a_torn_publish_over_an_owed_slot(tag: &str, base: Options) {
    let opts = os_opts(&base);
    let (d, mut db, mut model, t) = os_start(&format!("osl-torn-{tag}"), &opts);
    for r in 1..=6u32 {
        os_round(&mut db, &mut model, &format!("r{r}"), r * OS_KEYS, OS_KEYS);
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        if !t.lock().unwrap().kills.is_empty() {
            break;
        }
    }
    db.watch_slot_writes(None);
    drop(db);
    let mut t = OsTracker::taken(t);
    for (p, _) in std::mem::take(&mut t.crashes) {
        let _ = std::fs::remove_dir_all(p);
    }
    assert!(
        !t.kills.is_empty(),
        "{tag}: no write followed a deferred slot whose sync was owed; the case was not \
         reached: {:?}",
        t.writes
    );
    let (k, _, owed) = t.kills.remove(0);
    // The owed publish's slot, and the next publish's: the same, since the
    // other holds the newest manifest known durable.
    let slot_of = |seq: u64| t.writes.iter().find(|w| w.seq == seq).map(|w| w.slot);
    let b = slot_of(owed).expect("the owed publish was watched");
    assert_eq!(
        slot_of(owed + 1),
        Some(b),
        "{tag}: premise: the publish after the owed one writes the same slot: {:?}",
        t.writes
    );
    let a = 1 - b;
    let whole = std::fs::read(k.join(OS_SLOTS[b])).unwrap();
    assert_eq!(
        os_seq_of(&whole),
        Some(owed),
        "{tag}: premise: the cache holds the owed publish whole in {}",
        OS_SLOTS[b]
    );
    let a_seq = os_seq_of(&std::fs::read(k.join(OS_SLOTS[a])).unwrap())
        .expect("premise: the other slot holds a manifest");
    assert!(a_seq < owed, "{tag}: premise: {a_seq} before {owed}");
    // The publish after it, killed part way through its write: the page
    // cache holds the slot torn.
    let torn = os_torn(&whole, owed + 1);
    std::fs::write(k.join(OS_SLOTS[b]), &torn).unwrap();

    // A copy for the second sync's failure, below.
    let k2 = dir(&format!("osl-torn-{tag}-fail"));
    copy_store(&k, &k2);
    // And one for the newest slot passed over for pending segments, below:
    // plain only, since a sealed slot is not forged here.
    let skipped_case = whole.starts_with(b"SUPDBMAN");
    let k3 = dir(&format!("osl-torn-{tag}-skipped"));
    let at_kill = model.clone();
    if skipped_case {
        copy_store(&k, &k3);
    }

    // The reopen, the slot written torn passed over.
    let from = Db::trace_dir_io(true).len();
    let mut db = Db::open(&k, opts.clone()).expect("the kill state opens");
    let calls = osl_calls(&k, from);
    db.hold_merge_landing(true);
    db.hold_seal_landing(true);
    assert_eq!(
        db.manifest_seq(),
        a_seq,
        "{tag}: premise: the open took {} and published nothing",
        OS_SLOTS[a]
    );
    // Commits acknowledged on the reopened store, too few to seal, so
    // nothing publishes and the torn slot stays as the kill left it.
    os_round(&mut db, &mut model, "after", 60 * OS_KEYS, 2000);
    assert_eq!(
        std::fs::read(k.join(OS_SLOTS[b])).unwrap(),
        torn,
        "{tag}: premise: nothing rewrote {} since the open",
        OS_SLOTS[b]
    );
    assert_eq!(
        db.manifest_seq(),
        a_seq,
        "{tag}: premise: nothing published"
    );
    // The power cut, the landings held across the copy: every file as the
    // page cache holds it, the slot taken as the open synced it, and the
    // other as the device may hold it -- the owed publish whole where no
    // fdatasync of that file ran since the kill, the torn image where the
    // open's did.
    let cut = dir(&format!("osl-torn-{tag}-cut"));
    copy_store(&k, &cut);
    db.hold_seal_landing(false);
    db.hold_merge_landing(false);
    let b_synced = calls
        .iter()
        .any(|(what, p)| *what == "sync_slot" && *p == k.join(OS_SLOTS[b]));
    if !b_synced {
        std::fs::write(cut.join(OS_SLOTS[b]), &whole).unwrap();
    }
    // The first slot the store opens for writing after the reopen, read
    // off the calls by its path in the filesystem as the syncs are, comes
    // after both slots' fdatasyncs.
    db.flush().unwrap();
    db.settle().unwrap();
    let later = osl_calls(&k, from);
    drop(db);
    let failures = os_check_all(
        &[(
            cut.clone(),
            format!(
                "a power cut after the reopen, {} holding {}",
                OS_SLOTS[b],
                if b_synced {
                    "the torn image the open synced"
                } else {
                    "the owed publish whole"
                }
            ),
        )],
        &model,
        &opts,
    );
    assert!(failures.is_empty(), "{tag}: {failures:?}");
    // Both slots fdatasynced by the open, read off the calls.
    for s in OS_SLOTS {
        assert!(
            calls
                .iter()
                .any(|(what, p)| *what == "sync_slot" && *p == k.join(s)),
            "{tag}: the open fdatasynced {s}: {calls:?}"
        );
    }
    let write = later
        .iter()
        .position(|(what, _)| *what == "open_slot")
        .unwrap_or_else(|| panic!("{tag}: the flush after the reopen wrote a slot: {later:?}"));
    for s in OS_SLOTS {
        assert!(
            later[..write]
                .iter()
                .any(|(what, p)| *what == "sync_slot" && *p == k.join(s)),
            "{tag}: {s} fdatasynced before the first slot was written: {later:?}"
        );
    }
    // The second sync's failure is the open's answer, and the open that
    // gives it has removed and written nothing; the failure names the
    // inode of the slot the open did not take.
    let found = os_files(&k2);
    let from2 = Db::trace_dir_io(true).len();
    let refused = match Db::open(
        &k2,
        Options {
            fail_open_other_slot_sync: true,
            ..opts.clone()
        },
    ) {
        Ok(_) => panic!("{tag}: the open answered Ok over a failed fdatasync of its other slot"),
        Err(e) => e.to_string(),
    };
    assert!(
        refused.contains("a test failed the open's fdatasync of the manifest slot it did not take"),
        "{tag}: the open refused for another reason: {refused}"
    );
    let ino = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(k2.join(OS_SLOTS[b])).unwrap().ino()
    };
    assert!(
        refused.ends_with(&format!("(inode {ino})")),
        "{tag}: the sync that failed was not of the slot the open passed over, {} (inode \
         {ino}): {refused}",
        OS_SLOTS[b]
    );
    let changed = osl_changed(&found, &k2);
    assert!(
        changed.is_empty(),
        "{tag}: the open that failed its other slot's fdatasync changed the directory: \
         {changed:?}"
    );
    // The call that records an fdatasync once it returns records the taken
    // slot's and not the one that failed.
    let failed_calls = osl_calls(&k2, from2);
    let synced_of = |s: &str| {
        failed_calls
            .iter()
            .any(|(what, p)| *what == "sync_slot" && *p == k2.join(s))
    };
    assert!(
        synced_of(OS_SLOTS[a]) && !synced_of(OS_SLOTS[b]),
        "{tag}: the record holds the taken slot's fdatasync and not the failed one: \
         {failed_calls:?}"
    );
    // The control: the open that succeeds removes something here (what
    // the owed publish named), so the failed open had something to leave.
    drop(Db::open(&k2, opts.clone()).expect("the kill state opens"));
    assert!(
        found.keys().any(|n| !k2.join(n).exists()),
        "{tag}: the open removed nothing from the kill state; the case was not reached"
    );
    let _ = std::fs::remove_dir_all(&k2);
    if skipped_case {
        osl_skipped(tag, &k3, [a, b], &whole, [a_seq, owed], &at_kill, &opts);
    }
    let _ = std::fs::remove_dir_all(&k3);

    for (p, _, _) in &t.kills {
        let _ = std::fs::remove_dir_all(p);
    }
    let _ = std::fs::remove_dir_all(&k);
    let _ = std::fs::remove_dir_all(&cut);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&t.root);
}

/// A plain slot of this version: `whole`'s manifest under the sequence
/// `seq`, each of `pending` flagged pending, its checksum made again.
fn osl_pending_of(whole: &[u8], seq: u64, pending: &[String]) -> Vec<u8> {
    assert_eq!(&whole[..9], b"SUPDBMAN\x03", "a plain slot of this version");
    let len = u32::from_le_bytes(whole[17..21].try_into().unwrap()) as usize;
    let mut body = whole[25..25 + len].to_vec();
    let n = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
    let mut p = 12usize;
    for _ in 0..n {
        let l = u16::from_le_bytes(body[p..p + 2].try_into().unwrap()) as usize;
        let name = std::str::from_utf8(&body[p + 2..p + 2 + l]).unwrap();
        if pending.iter().any(|x| x == name) {
            body[p + 2 + l] = 1;
        }
        p += 2 + l + 1;
    }
    let crc = common::crc32c_from(common::crc32c_from(0, &seq.to_le_bytes()), &body);
    let mut out = b"SUPDBMAN\x03".to_vec();
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// The other slot passed over because it is the newest and its pending
/// segments do not read whole: the open writes over that slot itself, but
/// only after its sweep. `k` is a kill state whose cache holds, in slot
/// `b`, a publish after the owed one -- forged here, the owed publish's new
/// segments flagged pending under the next sequence, and one of them lost
/// -- and whose device may hold the owed publish whole there. The open's
/// own slot write torn (`Options::tear_open_slot_write`) stops it after the
/// sweep has removed what the slot it takes does not name and before
/// anything rewrites `b`; a power cut then hands the next open what the
/// device holds of `b`, which must not be the owed publish.
fn osl_skipped(
    tag: &str,
    k: &Path,
    [a, b]: [usize; 2],
    whole: &[u8],
    [a_seq, owed]: [u64; 2],
    model: &BTreeMap<Vec<u8>, Vec<u8>>,
    opts: &Options,
) {
    let names_of =
        |bytes: &[u8]| -> Vec<String> { oo_names(bytes).1.into_iter().map(|(n, _)| n).collect() };
    let a_names = names_of(&std::fs::read(k.join(OS_SLOTS[a])).unwrap());
    let new: Vec<String> = names_of(whole)
        .into_iter()
        .filter(|n| n.ends_with(".sup") && !a_names.contains(n))
        .collect();
    assert!(
        !new.is_empty(),
        "{tag}: premise: the owed publish names a segment {} does not",
        OS_SLOTS[a]
    );
    std::fs::write(k.join(OS_SLOTS[b]), osl_pending_of(whole, owed + 1, &new)).unwrap();
    // Premises, each on a copy: with its segments whole the open takes the
    // forged slot, and with one lost it passes the slot over for the other.
    let w = dir(&format!("osl-skip-{tag}-whole"));
    copy_store(k, &w);
    let seq = Db::open(&w, opts.clone())
        .expect("the forged slot's store opens")
        .manifest_seq();
    assert!(
        seq > owed + 1,
        "{tag}: premise: the open took the forged slot {} at {}: {seq}",
        OS_SLOTS[b],
        owed + 1
    );
    oo_tear(&k.join(&new[0]), OoTear::Empty);
    copy_store(k, &w);
    let seq = Db::open(&w, opts.clone())
        .expect("the store with a pending segment lost opens")
        .manifest_seq();
    assert!(
        seq > a_seq && seq <= owed,
        "{tag}: premise: the open passed the forged slot over for {} at {a_seq}: {seq}",
        OS_SLOTS[a]
    );
    let _ = std::fs::remove_dir_all(&w);
    // The open stopped between its sweep and its own write over `b`.
    let from = Db::trace_dir_io(true).len();
    let refused = match Db::open(
        k,
        Options {
            tear_open_slot_write: true,
            ..opts.clone()
        },
    ) {
        Ok(_) => panic!("{tag}: the open answered Ok over a torn slot write"),
        Err(e) => e.to_string(),
    };
    assert!(
        refused.contains("a test tore this manifest slot write"),
        "{tag}: the open failed for another reason: {refused}"
    );
    let calls = osl_calls(k, from);
    assert!(
        new.iter().all(|n| !k.join(n).exists()),
        "{tag}: premise: the open swept {new:?} before its slot write"
    );
    // The power cut: every file as the page cache holds it, and `b` the
    // owed publish whole where no fdatasync of that file ran since the kill.
    let cut = dir(&format!("osl-skip-{tag}-cut"));
    copy_store(k, &cut);
    let b_synced = calls
        .iter()
        .any(|(what, p)| *what == "sync_slot" && *p == k.join(OS_SLOTS[b]));
    if !b_synced {
        std::fs::write(cut.join(OS_SLOTS[b]), whole).unwrap();
    }
    let failures = os_check_all(
        &[(
            cut.clone(),
            format!(
                "a power cut after an open that passed over {} for pending segments, it \
                 holding {}",
                OS_SLOTS[b],
                if b_synced {
                    "the image the open synced and then tore"
                } else {
                    "the owed publish whole"
                }
            ),
        )],
        model,
        opts,
    );
    assert!(failures.is_empty(), "{tag}: {failures:?}");
    let _ = std::fs::remove_dir_all(&cut);
}

#[test]
fn a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen() {
    a_torn_publish_over_an_owed_slot("plain", Options::default());
}

#[test]
fn a_torn_publish_over_an_owed_slot_survives_a_power_cut_after_the_reopen_under_a_cipher() {
    a_torn_publish_over_an_owed_slot("cipher", under_cipher());
}

/// A store whose other slot file is not there -- one born under the rename
/// shape (`manifest_slots` off) -- is opened without the open making it: an
/// open that refuses leaves the directory as it found it, the file still
/// absent, and an open under the rename shape that answers Ok makes none.
fn an_absent_other_slot(tag: &str, base: Options) {
    let rename = Options {
        manifest_slots: false,
        ..os_opts(&base)
    };
    let slots = os_opts(&base);
    let d = dir(&format!("osl-absent-{tag}"));
    let mut db = Db::create(&d, rename.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    drop(db);
    assert!(
        !d.join(OS_SLOTS[1]).exists(),
        "{tag}: premise: the rename shape has no {}",
        OS_SLOTS[1]
    );
    // A segment the manifest names taken away, so the open refuses at its
    // check of the slot against the directory, after its slot syncs.
    let found = os_files(&d);
    let seg = found
        .iter()
        .filter(|(n, _)| !n.starts_with("manifest") && !n.starts_with("wal-"))
        .max_by_key(|(_, b)| b.len())
        .map(|(n, _)| n.clone())
        .expect("a segment");
    std::fs::remove_file(d.join(&seg)).unwrap();
    let mut gone = found.clone();
    gone.remove(&seg);
    for (shape, opts) in [("rename", &rename), ("slots", &slots)] {
        let refused = match Db::open(&d, opts.clone()) {
            Ok(_) => panic!("{tag}, {shape}: the open answered Ok without {seg}"),
            Err(e) => e.to_string(),
        };
        assert!(
            refused.contains("which is missing from the directory"),
            "{tag}, {shape}: the open refused for another reason: {refused}"
        );
        let changed = osl_changed(&gone, &d);
        assert!(
            changed.is_empty(),
            "{tag}, {shape}: the open that refused changed the directory: {changed:?}"
        );
    }
    std::fs::write(d.join(&seg), &found[&seg]).unwrap();
    let db = Db::open(&d, rename.clone()).expect("the store opens");
    for (k, v) in model.iter().step_by(97) {
        assert_eq!(read_vec(&db, k), vec![v.clone()], "{tag}");
    }
    drop(db);
    assert!(
        !d.join(OS_SLOTS[1]).exists(),
        "{tag}: an open under the rename shape made {}",
        OS_SLOTS[1]
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_open_makes_no_slot_file_it_does_not_find() {
    an_absent_other_slot("plain", Options::default());
}

#[test]
fn an_open_makes_no_slot_file_it_does_not_find_under_a_cipher() {
    an_absent_other_slot("cipher", under_cipher());
}

// ---------------------------------------------------------------------
// rename-arm: the manifest's shape before the slots (`manifest_slots`
// off), kept as an arm: a publish writes the manifest under a temp name,
// fsyncs it, renames it over `manifest` and fsyncs the directory, and the
// seal's own directory entries are made durable by that fsync, so the
// shape rests on a rename reaching the device no sooner than the renames
// made before it -- the seal's segment before the manifest that names it.
// The states below are built on that: a power cut keeps a prefix, in the
// order they were made, of the directory operations no directory fsync
// that returned covered. What an open removes it removes after reading a
// manifest an earlier process renamed into place, and no order of the
// open's own puts the removals behind that rename: an open that removes
// what the manifest stopped naming makes the rename durable first, or a
// power cut may keep the removals and not the rename. Every commit is
// made under `SyncPolicy::Always` and acknowledged before the instant a
// state is taken in.
// ---------------------------------------------------------------------

/// The rename shape over `os_opts`.
fn ra_opts(base: &Options) -> Options {
    Options {
        manifest_slots: false,
        ..os_opts(base)
    }
}

/// Whether `name` is a file a power cut's loss of the directory's entries
/// is modelled for: a segment, an ordered index, a form file or an
/// envelope, and not the log, the manifest or its spares, which the states
/// below set by hand.
fn ra_entry(name: &str) -> bool {
    !(name.starts_with("wal-") || name.starts_with("spare-") || name.starts_with("manifest"))
}

fn ra_names(d: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// One rename publish as the watch saw it, between its rename and its
/// directory fsync: the store's files copied there, the manifest before it
/// and after it, and the entries made since the watch before it -- the
/// publish's own new names among them, which only its directory fsync
/// makes durable.
struct RaPublish {
    seq: u64,
    at: PathBuf,
    before: Vec<u8>,
    now: Vec<u8>,
    new: Vec<String>,
}

struct RaTracker {
    store: PathBuf,
    root: PathBuf,
    before: Vec<u8>,
    names: std::collections::BTreeSet<String>,
    armed: bool,
    seen: Vec<u64>,
    publishes: Vec<RaPublish>,
}

type RaShared = std::sync::Arc<std::sync::Mutex<RaTracker>>;

impl RaTracker {
    /// Started over a store at rest, its manifest durable.
    fn new(store: &Path, root: &Path) -> RaShared {
        std::fs::create_dir_all(root).unwrap();
        std::sync::Arc::new(std::sync::Mutex::new(RaTracker {
            store: store.to_path_buf(),
            root: root.to_path_buf(),
            before: std::fs::read(store.join("manifest")).unwrap(),
            names: ra_names(store),
            armed: false,
            seen: Vec::new(),
            publishes: Vec::new(),
        }))
    }

    fn arm(t: &RaShared, on: bool) {
        t.lock().unwrap_or_else(|e| e.into_inner()).armed = on;
    }

    fn take(t: &RaShared) -> Vec<RaPublish> {
        std::mem::take(&mut t.lock().unwrap_or_else(|e| e.into_inner()).publishes)
    }
}

/// The watch: never panics, since it runs on the segment work's thread.
fn ra_watcher(t: RaShared) -> supdb::db::SlotWatch {
    std::sync::Arc::new(move |w: &supdb::db::SlotWrite| {
        let mut t = t.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::fs::read(t.store.join("manifest")).unwrap_or_default();
        let names = ra_names(&t.store);
        t.seen.push(w.seq);
        if t.armed {
            let at = t.root.join(format!("pub-{}", t.seen.len()));
            os_copy(&t.store, &at);
            let new = names
                .iter()
                .filter(|n| !t.names.contains(*n) && ra_entry(n))
                .cloned()
                .collect();
            let before = t.before.clone();
            t.publishes.push(RaPublish {
                seq: w.seq,
                at,
                before,
                now: now.clone(),
                new,
            });
        }
        t.before = now;
        t.names = names;
    })
}

/// A copy of `from` at `to`, then made what a crash leaves.
fn ra_state(from: &Path, to: &Path, edit: impl FnOnce(&Path)) -> PathBuf {
    os_copy(from, to);
    edit(to);
    to.to_path_buf()
}

/// The crash states one rename publish leaves, at each of its steps.
fn ra_states(p: &RaPublish, root: &Path, n: usize) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    // A kill, or a power cut, after the directory fsync: what the watch
    // copied, the rename made.
    out.push((
        ra_state(&p.at, &root.join(format!("s{n}-renamed")), |_| {}),
        format!("publish {}: a kill after its rename", p.seq),
    ));
    // A kill with the temp written and fsynced, not renamed.
    out.push((
        ra_state(&p.at, &root.join(format!("s{n}-temp")), |d| {
            std::fs::write(d.join("manifest"), &p.before).unwrap();
            std::fs::write(d.join("manifest.tmp"), &p.now).unwrap();
        }),
        format!(
            "publish {}: a kill with its temp written (manifest seq {:?} put back, {:?} in the \
             temp, manifest.alt seq {:?})",
            p.seq,
            os_seq_of(&p.before),
            os_seq_of(&p.now),
            std::fs::read(p.at.join("manifest.alt"))
                .ok()
                .and_then(|b| os_seq_of(&b))
        ),
    ));
    // A power cut before the directory fsync that keeps none of the
    // entries made since the watch before: the rename lost, and every name
    // made since with it.
    out.push((
        ra_state(&p.at, &root.join(format!("s{n}-cut")), |d| {
            std::fs::write(d.join("manifest"), &p.before).unwrap();
            for f in &p.new {
                let _ = std::fs::remove_file(d.join(f));
            }
        }),
        format!(
            "publish {}: a power cut before its directory fsync, {:?} lost",
            p.seq, p.new
        ),
    ));
    out
}

/// What had been acknowledged when a publish was watched.
type RaModel = BTreeMap<Vec<u8>, Vec<u8>>;

/// A rename-shape store loaded, flushed and settled, then rounds of puts
/// each flushed with the watch armed. The publishes watched, each with the
/// model as it stood when its flush was asked for: every commit of it
/// acknowledged before the publish's instant.
fn ra_rounds(
    tag: &str,
    opts: &Options,
    rounds: u32,
) -> (PathBuf, PathBuf, Vec<(RaPublish, RaModel)>) {
    ra_rounds_born(tag, None, opts, rounds)
}

/// `ra_rounds`, the store born under `born` where it is given -- the
/// slots, with its newest manifest in `manifest.alt` where `alt` says so --
/// and reopened under `opts` for the rounds watched: a store that ran under
/// the slots and moved to the rename shape, its other slot file left.
fn ra_rounds_born(
    tag: &str,
    born: Option<(&Options, bool)>,
    opts: &Options,
    rounds: u32,
) -> (PathBuf, PathBuf, Vec<(RaPublish, RaModel)>) {
    let d = dir(&format!("ra-{tag}"));
    let root = dir(&format!("ra-{tag}-crashes"));
    let mut db = Db::create(&d, born.map_or(opts, |b| b.0).clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    if let Some((_, alt)) = born {
        ra_slots_life_ends_in(tag, &d, &mut db, &mut model, alt);
        drop(db);
        db = Db::open(&d, opts.clone()).unwrap();
        db.settle().unwrap();
    }
    // Watched from the manifest the tracker reads: nothing lands between.
    db.hold_seal_landing(true);
    db.hold_merge_landing(true);
    let t = RaTracker::new(&d, &root);
    db.watch_slot_writes(Some(ra_watcher(t.clone())));
    db.hold_seal_landing(false);
    db.hold_merge_landing(false);
    let mut pubs = Vec::new();
    for r in 1..=rounds {
        os_round(
            &mut db,
            &mut model,
            &format!("r{r}"),
            r * OS_KEYS,
            OS_KEYS / 4,
        );
        RaTracker::arm(&t, true);
        db.flush().unwrap();
        RaTracker::arm(&t, false);
        pubs.extend(RaTracker::take(&t).into_iter().map(|p| (p, model.clone())));
        db.settle().unwrap();
    }
    db.watch_slot_writes(None);
    drop(db);
    (d, root, pubs)
}

/// (a) At every step of every publish of a rename-shape store -- the temp
/// written, renamed, the directory fsynced or not -- a crash leaves a
/// manifest whose every name is in the directory, with the log holding
/// every write since.
fn a_rename_publish_at_every_step(tag: &str, base: Options) {
    let opts = ra_opts(&base);
    let (d, root, pubs) = ra_rounds(&format!("steps-{tag}"), &opts, 4);
    assert!(
        pubs.len() >= 4,
        "{tag}: premise: every flush published under the watch: {}",
        pubs.len()
    );
    assert!(
        pubs.iter().any(|(p, _)| !p.new.is_empty()),
        "{tag}: premise: a publish named entries the watch before it had not seen"
    );
    let mut failures = Vec::new();
    let mut states = 0usize;
    for (n, (p, model)) in pubs.iter().enumerate() {
        let built = ra_states(p, &root, n);
        states += built.len();
        failures.extend(os_check_all(&built, model, &opts));
    }
    assert!(
        failures.is_empty(),
        "{tag}: {} of {states} crash states lost acknowledged commits:\n{}",
        failures.len(),
        failures.join("\n")
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_rename_shape_publish_leaves_a_whole_manifest_at_every_step() {
    a_rename_publish_at_every_step("plain", Options::default());
}

#[test]
fn a_rename_shape_publish_leaves_a_whole_manifest_at_every_step_under_a_cipher() {
    a_rename_publish_at_every_step("cipher", under_cipher());
}

/// The trace's directory fsyncs of `d` from `from` on.
fn ra_dir_synced(d: &Path, from: usize) -> bool {
    Db::trace_dir_io(true)
        .split_off(from)
        .iter()
        .any(|(what, p)| *what == "sync_dir" && p.starts_with(d))
}

/// (c), and (e) where `reopen` is the other shape: a kill between a
/// publish's rename and its directory fsync, an open under `reopen`,
/// commits acknowledged, then a power cut that takes back the directory's
/// entries no directory fsync covered -- the rename the killed process
/// made, and its new names with it, unless the open fsynced the directory
/// -- and keeps what the open removed, which nothing orders behind the
/// rename. The store opens with every acknowledged write.
fn a_kill_between_the_rename_and_its_dir_sync(tag: &str, base: Options, reopen: &Options) {
    a_kill_between_the_rename_and_its_dir_sync_born(tag, base, None, reopen)
}

/// Rounds of a store under the slots until its newest manifest is in
/// `manifest.alt` where `alt` says so, and in `manifest` where it does not.
fn ra_slots_life_ends_in(
    tag: &str,
    d: &Path,
    db: &mut Db,
    model: &mut BTreeMap<Vec<u8>, Vec<u8>>,
    alt: bool,
) {
    let mut i = 0u32;
    loop {
        let s = OS_SLOTS.map(|f| std::fs::read(d.join(f)).ok().and_then(|b| os_seq_of(&b)));
        if (s[1] > s[0]) == alt {
            return;
        }
        i += 1;
        assert!(
            i < 8,
            "{tag}: premise: the slots life's newest manifest in the slot asked"
        );
        os_round(
            db,
            model,
            &format!("b{i}"),
            (100 + i) * OS_KEYS,
            OS_KEYS / 4,
        );
        db.flush().unwrap();
        db.settle().unwrap();
    }
}

/// (e) A store under the slots whose newest manifest is in `manifest.alt`,
/// opened under the rename shape and killed at once, its landings held:
/// opened under either shape, it is every acknowledged write. The rename
/// open takes `manifest.alt` away, and the store's manifest is then the one
/// it wrote as `manifest`, or no manifest names the store.
fn a_kill_at_the_rename_open_of_an_alt_store(tag: &str, base: Options) {
    let slots = os_opts(&base);
    let rename = ra_opts(&base);
    let d = dir(&format!("ra-altopen-{tag}"));
    let mut db = Db::create(&d, slots.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    ra_slots_life_ends_in(tag, &d, &mut db, &mut model, true);
    drop(db);
    let db = Db::open(&d, rename.clone()).unwrap();
    db.hold_seal_landing(true);
    db.hold_merge_landing(true);
    let k = dir(&format!("ra-altopen-{tag}-killed"));
    os_copy(&d, &k);
    db.hold_seal_landing(false);
    db.hold_merge_landing(false);
    drop(db);
    let mut states = Vec::new();
    for (i, o) in ["the rename shape", "the slots"].into_iter().enumerate() {
        let c = dir(&format!("ra-altopen-{tag}-killed-{i}"));
        os_copy(&k, &c);
        states.push((c, o, i));
    }
    let mut failures = Vec::new();
    for (c, o, i) in &states {
        let opts = if *i == 0 { &rename } else { &slots };
        failures.extend(os_check_all(
            &[(
                c.clone(),
                format!("{tag}: killed at its rename-shape open, reopened under {o}"),
            )],
            &model,
            opts,
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    for (c, _, _) in &states {
        let _ = std::fs::remove_dir_all(c);
    }
    let _ = std::fs::remove_dir_all(&k);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_store_moved_from_the_alt_slot_killed_at_its_rename_open_is_whole() {
    a_kill_at_the_rename_open_of_an_alt_store("plain", Options::default());
}

#[test]
fn a_store_moved_from_the_alt_slot_killed_at_its_rename_open_is_whole_under_a_cipher() {
    a_kill_at_the_rename_open_of_an_alt_store("cipher", under_cipher());
}

/// The same, the store born under the slots and moved to the rename shape
/// before the kill where `born` says so (`ra_rounds_born`): a store that
/// goes slots, rename, slots, its `manifest.alt` from the first life.
fn a_kill_between_the_rename_and_its_dir_sync_born(
    tag: &str,
    base: Options,
    born: Option<bool>,
    reopen: &Options,
) {
    ra_kill_and_reopen(tag, base, born, reopen, false)
}

/// The same, where `prefail` says so the open under `reopen` made first
/// with its directory fsync failed (`Options::fail_open_dir_sync`): a slots
/// open that made the slot file the rename life took away and never
/// reached the fsync after it, as a kill between the two leaves it; the
/// open after it finds both slot files there.
fn ra_kill_and_reopen(
    tag: &str,
    base: Options,
    born: Option<bool>,
    reopen: &Options,
    prefail: bool,
) {
    let opts = ra_opts(&base);
    let reopen = Options {
        encryption: base.encryption.clone(),
        ..reopen.clone()
    };
    let slots = os_opts(&base);
    let (d, root, pubs) = ra_rounds_born(
        &format!("kill-{tag}"),
        born.map(|alt| (&slots, alt)),
        &opts,
        4,
    );
    let mut reached = 0usize;
    let mut failures = Vec::new();
    for (n, (p, model)) in pubs.iter().enumerate() {
        let x = ra_state(&p.at, &root.join(format!("k{n}")), |_| {});
        let found = ra_names(&x);
        let from = Db::trace_dir_io(true).len();
        if prefail {
            assert!(
                !x.join("manifest.alt").exists(),
                "{tag}: premise: the rename life took the slots' other file away"
            );
            let failing = Options {
                fail_open_dir_sync: true,
                ..reopen.clone()
            };
            match Db::open(&x, failing) {
                Ok(_) => {
                    failures.push(format!("publish {}: the failing open answered Ok", p.seq));
                    continue;
                }
                Err(e) => assert!(
                    e.to_string()
                        .contains("a test failed the open's directory fsync"),
                    "{tag}: publish {}: the open refused for another reason: {e}",
                    p.seq
                ),
            }
            assert!(
                x.join("manifest.alt").exists(),
                "{tag}: premise: the failed open left the slot file it made"
            );
        }
        let mut db = match Db::open(&x, reopen.clone()) {
            Ok(db) => db,
            Err(e) => {
                failures.push(format!("publish {}: the kill state refused: {e}", p.seq));
                continue;
            }
        };
        let removed: Vec<String> = found
            .difference(&ra_names(&x))
            .filter(|f| ra_entry(f))
            .cloned()
            .collect();
        if !removed.is_empty() {
            reached += 1;
        }
        let mut model = model.clone();
        db.hold_seal_landing(true);
        db.hold_merge_landing(true);
        for i in 0..300u32 {
            let k = format!("after-{n}-{i:04}").into_bytes();
            let v = format!("v-{n}-{i}").into_bytes();
            db.put(&k, &v);
            model.insert(k, v);
        }
        db.commit().unwrap();
        let y = root.join(format!("k{n}-cut"));
        os_copy(&x, &y);
        let synced = ra_dir_synced(&x, from);
        std::mem::forget(db);
        if !synced {
            std::fs::write(y.join("manifest"), &p.before).unwrap();
            for f in &p.new {
                let _ = std::fs::remove_file(y.join(f));
            }
        }
        let what = format!(
            "publish {}: killed before its directory fsync, reopened (removed {removed:?}, \
             directory fsynced: {synced}), then a power cut",
            p.seq
        );
        failures.extend(os_check_all(&[(y, what)], &model, &reopen));
        let _ = std::fs::remove_dir_all(&x);
    }
    assert!(
        reached > 0,
        "{tag}: premise: an open removed what the manifest it took stopped naming"
    );
    assert!(
        failures.is_empty(),
        "{tag}: {} of {} states lost acknowledged commits:\n{}",
        failures.len(),
        pubs.len(),
        failures.join("\n")
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_kill_between_a_rename_and_its_directory_fsync_survives_a_power_cut_after_the_reopen() {
    let r = ra_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync("plain", Options::default(), &r);
}

#[test]
fn a_kill_between_a_rename_and_its_directory_fsync_survives_a_power_cut_under_a_cipher() {
    let r = ra_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync("cipher", under_cipher(), &r);
}

/// (e) The same kill, the store reopened under the slots.
#[test]
fn a_rename_shape_store_killed_before_its_directory_fsync_reopens_under_the_slots() {
    let s = os_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync("to-slots", Options::default(), &s);
}

#[test]
fn a_rename_shape_store_killed_before_its_directory_fsync_reopens_under_the_slots_cipher() {
    let s = os_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync("to-slots-cipher", under_cipher(), &s);
}

/// (e) The slots open of a store that went slots, rename, slots, killed in
/// its rename life between a rename and its directory fsync: the fsync the
/// open makes for the slot file the rename life took away fails
/// (`Options::fail_open_dir_sync`), and the open answers it having removed
/// nothing -- under a cipher, no envelope the renamed manifest stopped
/// naming swept before that fsync.
fn a_crossing_slots_open_whose_dir_sync_fails(tag: &str, base: Options) {
    let opts = ra_opts(&base);
    let slots = os_opts(&base);
    let reopen = Options {
        fail_open_dir_sync: true,
        ..slots.clone()
    };
    let (d, root, pubs) = ra_rounds_born(&format!("xfail-{tag}"), Some((&slots, false)), &opts, 4);
    let mut reached = 0usize;
    for (n, (p, _)) in pubs.iter().enumerate() {
        let x = ra_state(&p.at, &root.join(format!("x{n}")), |_| {});
        assert!(
            !x.join("manifest.alt").exists(),
            "{tag}: premise: the rename life took the slots' other file away"
        );
        let found = ra_names(&x);
        let from = Db::trace_dir_io(true).len();
        let e = match Db::open(&x, reopen.clone()) {
            Ok(_) => panic!("{tag}: publish {}: the open answered no failure", p.seq),
            Err(e) => e.to_string(),
        };
        // The seam stands in for the fsync after the slot file is made, so
        // the one directory fsync the failed open made is the one before
        // the file was made.
        let calls: Vec<(&str, PathBuf)> = Db::trace_dir_io(true)
            .split_off(from)
            .into_iter()
            .filter(|(_, q)| q.starts_with(&x))
            .collect();
        let syncs: Vec<usize> = (0..calls.len())
            .filter(|&i| calls[i].0 == "sync_dir")
            .collect();
        let made = calls
            .iter()
            .position(|(what, q)| *what == "open_slot" && q.ends_with("manifest.alt"));
        assert!(
            syncs.len() == 1 && made.is_some_and(|m| syncs[0] < m),
            "{tag}: publish {}: the failed open's directory fsync before it made the slot \
             file, and none after: {calls:?}",
            p.seq
        );
        assert!(
            e.contains("a test failed the open's directory fsync"),
            "{tag}: publish {}: the open refused for another reason: {e}",
            p.seq
        );
        let gone: Vec<String> = found
            .difference(&ra_names(&x))
            .filter(|f| ra_entry(f))
            .cloned()
            .collect();
        assert!(
            gone.is_empty(),
            "{tag}: publish {}: removed before the directory fsync: {gone:?}",
            p.seq
        );
        if Db::open(&x, slots.clone()).is_ok() {
            reached += 1;
        }
        let _ = std::fs::remove_dir_all(&x);
    }
    assert!(
        reached > 0,
        "{tag}: premise: the states open without the failure"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing() {
    a_crossing_slots_open_whose_dir_sync_fails("plain", Options::default());
}

#[test]
fn a_crossing_slots_open_whose_directory_fsync_fails_removes_nothing_under_a_cipher() {
    a_crossing_slots_open_whose_dir_sync_fails("cipher", under_cipher());
}

/// (e) Both ways in one store: born under the slots, moved to the rename
/// shape -- its newest manifest in `manifest` and, the second pair, in
/// `manifest.alt` when it moved -- killed between a rename and its
/// directory fsync, reopened under the slots, commits acknowledged, then a
/// power cut. The slot file the first life left is the slots open's reason
/// to make none and fsync no directory, unless the rename life took it away.
#[test]
fn a_store_moved_slots_rename_slots_killed_before_a_directory_fsync_survives_a_power_cut() {
    let s = os_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync_born("srs", Options::default(), Some(false), &s);
}

#[test]
fn a_store_moved_slots_rename_slots_killed_before_a_directory_fsync_survives_under_a_cipher() {
    let s = os_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync_born("srs-cipher", under_cipher(), Some(false), &s);
}

/// (e) The slots open of a store that went slots, rename, slots, killed in
/// its rename life between a rename and its directory fsync, whose first
/// slots open made the slot file the rename life took away and failed its
/// directory fsync: the slots open after it finds both files, and the
/// rename life's renames are durable before it removes what they stopped
/// naming, commits acknowledged and a power cut after.
#[test]
fn a_slots_open_after_a_crossing_open_that_failed_survives_a_power_cut() {
    let s = os_opts(&Options::default());
    ra_kill_and_reopen("srsf", Options::default(), Some(false), &s, true);
}

#[test]
fn a_slots_open_after_a_crossing_open_that_failed_survives_a_power_cut_under_a_cipher() {
    let s = os_opts(&Options::default());
    ra_kill_and_reopen("srsf-cipher", under_cipher(), Some(false), &s, true);
}

#[test]
fn a_store_moved_from_the_alt_slot_killed_before_a_directory_fsync_survives_a_power_cut() {
    let s = os_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync_born("srs-alt", Options::default(), Some(true), &s);
}

#[test]
fn a_store_moved_from_the_alt_slot_killed_before_a_directory_fsync_survives_under_a_cipher() {
    let s = os_opts(&Options::default());
    a_kill_between_the_rename_and_its_dir_sync_born(
        "srs-alt-cipher",
        under_cipher(),
        Some(true),
        &s,
    );
}

/// The open's directory fsync under the rename shape fails
/// (`Options::fail_open_dir_sync`): the open answers the failure, having
/// removed nothing, over a state in which it would have removed what the
/// manifest it took stopped naming.
fn an_open_whose_dir_sync_fails(tag: &str, base: Options) {
    let opts = ra_opts(&base);
    let (d, root, pubs) = ra_rounds(&format!("dirfail-{tag}"), &opts, 4);
    let mut reached = false;
    for (n, (p, _)) in pubs.iter().enumerate() {
        // Which states the open removes in, read off an open that succeeds.
        let probe = ra_state(&p.at, &root.join(format!("p{n}")), |_| {});
        let found = ra_names(&probe);
        drop(Db::open(&probe, opts.clone()).expect("the kill state opens"));
        let removes = found.difference(&ra_names(&probe)).any(|f| ra_entry(f));
        let _ = std::fs::remove_dir_all(&probe);
        if !removes {
            continue;
        }
        reached = true;
        let x = ra_state(&p.at, &root.join(format!("f{n}")), |_| {});
        let before = os_files(&x);
        let failing = Options {
            fail_open_dir_sync: true,
            ..opts.clone()
        };
        match Db::open(&x, failing) {
            Ok(_) => panic!(
                "{tag}: publish {}: the open answered Ok over a failed directory fsync",
                p.seq
            ),
            Err(e) => assert!(
                e.to_string().contains("directory fsync"),
                "{tag}: publish {}: the open refused for another reason: {e}",
                p.seq
            ),
        }
        let changed = osl_changed(&before, &x);
        assert!(
            changed.is_empty(),
            "{tag}: publish {}: the open whose directory fsync failed changed {changed:?}",
            p.seq
        );
        let _ = std::fs::remove_dir_all(&x);
        break;
    }
    assert!(reached, "{tag}: premise: an open that removes was built");
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_open_under_the_rename_shape_whose_directory_fsync_fails_removes_nothing() {
    an_open_whose_dir_sync_fails("plain", Options::default());
}

#[test]
fn an_open_under_the_rename_shape_whose_directory_fsync_fails_removes_nothing_under_a_cipher() {
    an_open_whose_dir_sync_fails("cipher", under_cipher());
}

/// (b) A seal's landing whose publish fails at the directory fsync after
/// its rename (`Db::fail_next_seal_dir_sync`, which under the rename shape
/// fails the publish's): the seal's segments' names and the manifest's
/// rename are what that fsync was for, and a later fsync that returns does
/// not make them durable. Nothing names the seal durable afterwards and no
/// log retires: every flush answers the failure, the manifest is the one
/// the failed publish left, and every log file is still there. A power cut
/// then loses the failed fsync's entries -- and keeps what a later
/// directory fsync that returned covered -- and the store opens with every
/// acknowledged commit.
fn a_failed_publish_dir_sync(tag: &str, base: Options) {
    let opts = Options {
        compact: false,
        ..ra_opts(&base)
    };
    let d = dir(&format!("ra-pubfail-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS / 2);
    db.flush().unwrap();
    db.settle().unwrap();
    let old = std::fs::read(d.join("manifest")).unwrap();
    let names0 = ra_names(&d);
    os_round(&mut db, &mut model, "r1", OS_KEYS, OS_KEYS / 4);
    db.fail_next_seal_dir_sync();
    let first = db.flush();
    assert!(
        !db.seal_sync_fault_pending(),
        "{tag}: premise: the publish's directory fsync was failed"
    );
    let failed = std::fs::read(d.join("manifest")).unwrap();
    let failed_seq = os_seq_of(&failed);
    assert!(
        failed_seq > os_seq_of(&old),
        "{tag}: premise: the failed publish renamed its manifest into place"
    );
    let new: Vec<String> = ra_names(&d)
        .difference(&names0)
        .filter(|f| ra_entry(f))
        .cloned()
        .collect();
    assert!(
        !new.is_empty(),
        "{tag}: premise: the seal made entries only the failed fsync covered"
    );
    let wals: Vec<String> = ra_names(&d)
        .into_iter()
        .filter(|n| n.starts_with("wal-"))
        .collect();
    // More acknowledged commits, a flush, and a settle: the landing after
    // the failure, if any lands.
    os_round(&mut db, &mut model, "r2", 2 * OS_KEYS, OS_KEYS / 8);
    let second = db.flush();
    let settled = db.settle();
    let now = std::fs::read(d.join("manifest")).unwrap();
    let left = ra_names(&d);
    // The power cut.
    let cut = dir(&format!("ra-pubfail-{tag}-cut"));
    os_copy(&d, &cut);
    std::mem::forget(db);
    if os_seq_of(&now) == failed_seq {
        std::fs::write(cut.join("manifest"), &old).unwrap();
    }
    for f in &new {
        let _ = std::fs::remove_file(cut.join(f));
    }
    let what = format!(
        "{tag}: a power cut after a failed publish directory fsync (first flush {first:?}, \
         second {second:?}, settle {settled:?}; manifest seq {:?} after the failure, {:?} at \
         the cut; lost {new:?})",
        failed_seq,
        os_seq_of(&now)
    );
    let failures = os_check_all(&[(cut.clone(), what.clone())], &model, &opts);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(first.is_err(), "{what}: the flush answers the failure");
    assert!(
        second.is_err(),
        "{what}: the flush after the failure answers it"
    );
    assert_eq!(
        os_seq_of(&now),
        failed_seq,
        "{what}: no manifest was written after the failure"
    );
    let gone: Vec<&String> = wals.iter().filter(|w| !left.contains(*w)).collect();
    assert!(
        gone.is_empty(),
        "{what}: no log retired after the failure: {gone:?} went"
    );
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&cut);
}

#[test]
fn a_rename_publish_whose_directory_fsync_fails_names_nothing_durable_after() {
    a_failed_publish_dir_sync("plain", Options::default());
}

#[test]
fn a_rename_publish_whose_directory_fsync_fails_names_nothing_durable_after_under_a_cipher() {
    a_failed_publish_dir_sync("cipher", under_cipher());
}

/// (b) A rename publish that fails before its rename -- its temp name taken
/// by a directory, so the temp cannot be made, as a device refusing the
/// write would leave it -- stops the seal queue as one that fails after its
/// rename does: every flush after it answers the failure, with the temp
/// name free again, until a reopen, which finds every acknowledged commit.
fn a_failed_publish_temp(tag: &str, base: Options) {
    let opts = Options {
        compact: false,
        ..ra_opts(&base)
    };
    let d = dir(&format!("ra-pubtemp-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS / 2);
    db.flush().unwrap();
    db.settle().unwrap();
    let old = std::fs::read(d.join("manifest")).unwrap();
    os_round(&mut db, &mut model, "r1", OS_KEYS, OS_KEYS / 4);
    std::fs::create_dir(d.join("manifest.tmp")).unwrap();
    let first = db.flush();
    assert!(
        first.is_err(),
        "{tag}: premise: the publish failed at its temp"
    );
    std::fs::remove_dir(d.join("manifest.tmp")).unwrap();
    assert_eq!(
        std::fs::read(d.join("manifest")).unwrap(),
        old,
        "{tag}: premise: the failed publish renamed nothing"
    );
    for i in 0..100u32 {
        let k = format!("after-{i:04}").into_bytes();
        let v = format!("w-{i}").into_bytes();
        db.put(&k, &v);
        model.insert(k, v);
    }
    db.commit().unwrap();
    let second = db.flush();
    let settled = db.settle();
    assert!(
        second.is_err() && settled.is_err(),
        "{tag}: the flush and the settle after a publish that failed at its temp answer it \
         (flush {second:?}, settle {settled:?})"
    );
    assert_eq!(
        std::fs::read(d.join("manifest")).unwrap(),
        old,
        "{tag}: no manifest was written after the failure"
    );
    drop(db);
    let failures = os_check_all(
        &[(
            d.clone(),
            format!("{tag}: reopened after a publish failed at its temp"),
        )],
        &model,
        &opts,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_rename_publish_that_fails_before_its_rename_stops_the_seal_queue() {
    a_failed_publish_temp("plain", Options::default());
}

#[test]
fn a_rename_publish_that_fails_before_its_rename_stops_the_seal_queue_under_a_cipher() {
    a_failed_publish_temp("cipher", under_cipher());
}

/// (d) A store written under `manifest_pending`, killed with its newest
/// slot naming a seal's segments pending -- whole in the page cache, synced
/// by nobody -- and reopened under the rename shape: the open takes the
/// pending names, more commits are acknowledged, and a flush's publish then
/// names the segments durable and retires the log that held their writes.
/// A power cut leaves a pending segment torn unless an fsync covered it,
/// and the store opens with every acknowledged commit.
fn a_pending_store_reopened_under_the_rename_shape(tag: &str, base: Options) {
    a_pending_store_reopened_under_the_rename_shape_as(tag, base, true)
}

/// The same, the rename-shape reopen made with `manifest_pending` as
/// `reopen_pending` says: the pending names the open takes are the slot's,
/// whatever the reopening options ask.
fn a_pending_store_reopened_under_the_rename_shape_as(
    tag: &str,
    base: Options,
    reopen_pending: bool,
) {
    let d = dir(&format!("ra-pend-{tag}"));
    // No merge, so the flush after the reopen leaves the pending segment
    // standing beside its own piece rather than rewriting it into a
    // partition whose fsync would cover it.
    let opts = Options {
        manifest_pending: true,
        direct_ingest: false,
        compact: false,
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 10),
        l0_trigger: 1000,
        ..base
    };
    let cipher = opts.encryption.clone();
    let rename = Options {
        manifest_slots: false,
        manifest_pending: reopen_pending,
        encryption: cipher,
        ..opts.clone()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for k in (0..3000).rev() {
        db.put(key(k).as_bytes(), b"v");
        model.insert(key(k).into_bytes(), vec![b"v".to_vec()]);
    }
    db.commit().unwrap();
    let names0 = ra_names(&d);
    db.hold_seal_durable(true);
    db.seal().unwrap();
    let plain = matches!(opts.encryption, Encryption::None);
    let pending = || -> Vec<String> {
        let b = std::fs::read(&oo_by_age(&d)[0]).unwrap();
        if !b.starts_with(b"SUPDBMAN") {
            return Vec::new();
        }
        oo_names(&b)
            .1
            .into_iter()
            .filter(|(_, p)| *p)
            .map(|(n, _)| n)
            .collect()
    };
    let newest_seq = || {
        std::fs::read(&oo_by_age(&d)[0])
            .ok()
            .and_then(|b| oo_seq(&b))
    };
    let seq0 = newest_seq();
    if plain {
        wait_for("the seal's pending slot", || !pending().is_empty());
    } else {
        wait_for("the seal's pending slot", || newest_seq() > seq0);
    }
    // The kill: the seal thread parked before its fsync for good, and the
    // store's files as it left them copied, since a store under a cipher
    // stays open in this process while its writer is.
    std::mem::forget(db);
    let held = d;
    let d = dir(&format!("ra-pend-{tag}-killed"));
    os_copy(&held, &d);
    let made: Vec<String> = ra_names(&d)
        .difference(&names0)
        .filter(|f| ra_entry(f))
        .cloned()
        .collect();
    assert!(!made.is_empty(), "{tag}: premise: the seal made its files");
    for f in &made {
        oo_dirty(&d.join(f));
    }
    let from = Db::trace_dir_io(true).len();
    let mut db = Db::open(&d, rename.clone()).unwrap();
    let calls = Db::trace_dir_io(true).split_off(from);
    let synced: Vec<PathBuf> = calls
        .iter()
        .filter(|(what, p)| *what == "sync_file" && p.parent() == Some(d.as_path()))
        .map(|(_, p)| p.clone())
        .collect();
    oo_holds(&db, &model, &format!("{tag}: after the kill"));
    // The open's record of what it fsynced, held to the calls: the
    // directory and every pending file it synced.
    let word = db.open_synced_files();
    assert!(
        calls
            .iter()
            .any(|(what, p)| *what == "sync_dir" && p.starts_with(&d)),
        "{tag}: the rename open fsynced the directory"
    );
    for f in synced
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .chain(std::iter::once(".".to_string()))
    {
        assert!(
            word.contains(&f),
            "{tag}: the open's record holds its fsync of {f}: {word:?}"
        );
    }
    for i in 0..500u32 {
        let k = format!("after-{i:04}").into_bytes();
        db.put(&k, b"w");
        model.insert(k, vec![b"w".to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    std::mem::forget(db);
    let reopened = d;
    let d = dir(&format!("ra-pend-{tag}-cut"));
    os_copy(&reopened, &d);
    let synced: Vec<PathBuf> = synced
        .iter()
        .map(|p| d.join(p.file_name().unwrap()))
        .collect();
    // The power cut: a file the seal made that no fsync covered comes back
    // torn. Under a cipher the seal's envelope was written and synced
    // before the slot that named it pending, so nothing of it is torn.
    if plain {
        for f in &made {
            if !synced.contains(&d.join(f)) && d.join(f).exists() {
                oo_tear(&d.join(f), OoTear::Run);
            }
        }
    }
    let state = format!(
        "{tag}: a power cut after the rename-shape reopen of a pending store (the seal made \
         {made:?}; the open fsynced {synced:?})"
    );
    let db = Db::open(&d, rename)
        .unwrap_or_else(|e| panic!("{state}: the store refused its acknowledged commits: {e}"));
    oo_holds(&db, &model, &state);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&reopened);
    let _ = std::fs::remove_dir_all(&held);
}

#[test]
fn a_pending_store_reopened_under_the_rename_shape_survives_a_power_cut() {
    a_pending_store_reopened_under_the_rename_shape("plain", Options::default());
}

#[test]
fn a_pending_store_reopened_under_the_rename_shape_without_the_option_survives_a_power_cut() {
    a_pending_store_reopened_under_the_rename_shape_as("plain-off", Options::default(), false);
}

#[test]
fn a_pending_store_reopened_under_the_rename_shape_without_the_option_under_a_cipher() {
    a_pending_store_reopened_under_the_rename_shape_as("cipher-off", under_cipher(), false);
}

/// Under a cipher a pending seal's envelope is written and synced before
/// the slot that names it (`manifest_image`), so the reopen has nothing to
/// make durable; the same states hold.
#[test]
fn a_pending_store_reopened_under_the_rename_shape_survives_a_power_cut_under_a_cipher() {
    a_pending_store_reopened_under_the_rename_shape("cipher", under_cipher());
}

/// (e) The other way: a store under the slots killed with a drain's slot
/// owed its fdatasync, reopened under the rename shape, commits
/// acknowledged, then a power cut that keeps of each slot file what an
/// fsync covered -- the open's own, read off the calls -- and the device's
/// bytes otherwise; then the reopened store's rename publishes, each step's
/// crash states built.
fn a_slots_store_reopened_under_the_rename_shape(tag: &str, base: Options) {
    let opts = os_opts(&base);
    let rename = ra_opts(&base);
    let (d, mut db, mut model, t) = os_start(&format!("ra-from-slots-{tag}"), &opts);
    for r in 1..=6u32 {
        os_round(&mut db, &mut model, &format!("r{r}"), r * OS_KEYS, OS_KEYS);
        OsTracker::arm(&t, true);
        db.flush().unwrap();
        OsTracker::arm(&t, false);
        db.settle().unwrap();
        if !t.lock().unwrap().kills.is_empty() {
            break;
        }
    }
    db.watch_slot_writes(None);
    drop(db);
    let mut t = OsTracker::taken(t);
    assert!(
        !t.kills.is_empty(),
        "{tag}: premise: a kill with a drain's slot owed was built"
    );
    let (kill, device, _) = t.kills.remove(0);
    let from = Db::trace_dir_io(true).len();
    let mut db = Db::open(&kill, rename.clone()).expect("the kill state opens");
    let synced: Vec<PathBuf> = Db::trace_dir_io(true)
        .split_off(from)
        .into_iter()
        .filter(|(what, _)| *what == "sync_slot")
        .map(|(_, p)| p)
        .collect();
    db.hold_seal_landing(true);
    db.hold_merge_landing(true);
    for i in 0..300u32 {
        let k = format!("after-{i:04}").into_bytes();
        let v = format!("w-{i}").into_bytes();
        db.put(&k, &v);
        model.insert(k, v);
    }
    db.commit().unwrap();
    let cut = kill.with_file_name(format!(
        "{}-cut",
        kill.file_name().unwrap().to_str().unwrap()
    ));
    os_copy(&kill, &cut);
    // The reopened store's publishes watched from the manifest the tracker
    // reads, before the landings held since the copy are let go.
    let root2 = kill.with_file_name(format!(
        "{}-renamed",
        kill.file_name().unwrap().to_str().unwrap()
    ));
    let t2 = RaTracker::new(&kill, &root2);
    db.watch_slot_writes(Some(ra_watcher(t2.clone())));
    db.hold_seal_landing(false);
    db.hold_merge_landing(false);
    for (j, s) in OS_SLOTS.iter().enumerate() {
        if !synced.contains(&kill.join(s)) {
            std::fs::write(cut.join(s), &device[j]).unwrap();
        }
    }
    let failures = os_check_all(
        &[(
            cut,
            format!("{tag}: a power cut after the rename-shape reopen of an owed slot"),
        )],
        &model,
        &rename,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // The reopened store's first rename publishes, at every step.
    os_round(&mut db, &mut model, "after-reopen", 60 * OS_KEYS, 2000);
    RaTracker::arm(&t2, true);
    db.flush().unwrap();
    RaTracker::arm(&t2, false);
    let pubs = RaTracker::take(&t2);
    db.settle().unwrap();
    db.watch_slot_writes(None);
    drop(db);
    assert!(
        !pubs.is_empty(),
        "{tag}: premise: the reopened store published"
    );
    let mut states = Vec::new();
    for (n, p) in pubs.iter().enumerate() {
        states.extend(ra_states(p, &root2, n));
    }
    let failures = os_check_all(&states, &model, &rename);
    assert!(
        failures.is_empty(),
        "{tag}: {} of {} crash states lost acknowledged commits:\n{}",
        failures.len(),
        states.len(),
        failures.join("\n")
    );
    for (p, _) in &t.crashes {
        let _ = std::fs::remove_dir_all(p);
    }
    for (p, _, _) in &t.kills {
        let _ = std::fs::remove_dir_all(p);
    }
    let _ = std::fs::remove_dir_all(&kill);
    let _ = std::fs::remove_dir_all(&root2);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&t.root);
}

#[test]
fn a_slots_store_killed_over_an_owed_slot_reopens_under_the_rename_shape() {
    a_slots_store_reopened_under_the_rename_shape("plain", Options::default());
}

#[test]
fn a_slots_store_killed_over_an_owed_slot_reopens_under_the_rename_shape_under_a_cipher() {
    a_slots_store_reopened_under_the_rename_shape("cipher", under_cipher());
}

/// (f) The open's refusals under the rename shape say which file they read:
/// `manifest` for a store born under the shape, and `manifest.alt` where a
/// store moved from the slots has its newest manifest there; and a store
/// whose only manifest is gone beside partitions is refused as one.
#[test]
fn an_open_under_the_rename_shape_refuses_by_the_manifest_file_it_read() {
    let rename = ra_opts(&Options::default());
    let slots = os_opts(&Options::default());
    // Born under the rename shape.
    let d = dir("ra-refuse-born");
    let mut db = Db::create(&d, rename.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
    db.flush().unwrap();
    db.settle().unwrap();
    drop(db);
    let named = oo_names(&std::fs::read(d.join("manifest")).unwrap()).1;
    let seg = &named.first().expect("a segment named").0;
    let kept = std::fs::read(d.join(seg)).unwrap();
    std::fs::remove_file(d.join(seg)).unwrap();
    oo_refused(
        &d,
        &rename,
        &format!("the manifest slot manifest names {seg}, which is missing"),
        "born under the rename shape",
    );
    std::fs::write(d.join(seg), &kept).unwrap();
    // Its only manifest gone, beside partitions.
    let m = std::fs::read(d.join("manifest")).unwrap();
    std::fs::write(d.join("manifest"), b"").unwrap();
    oo_refused(
        &d,
        &rename,
        "no manifest slot can be read",
        "the rename shape's manifest emptied",
    );
    std::fs::write(d.join("manifest"), &m).unwrap();
    drop(Db::open(&d, rename.clone()).expect("the store opens again"));
    let _ = std::fs::remove_dir_all(&d);
    // Moved from the slots, its newest manifest in `manifest.alt`.
    let d = dir("ra-refuse-moved");
    let mut db = Db::create(&d, slots.clone()).unwrap();
    let mut r = 0u32;
    loop {
        os_round(
            &mut db,
            &mut model,
            &format!("m{r}"),
            r * OS_KEYS,
            OS_KEYS / 4,
        );
        db.flush().unwrap();
        db.settle().unwrap();
        r += 1;
        let [newest, _] = oo_by_age(&d);
        if newest.ends_with("manifest.alt") {
            break;
        }
        assert!(r < 20, "premise: the newest slot came to be manifest.alt");
    }
    drop(db);
    let named = oo_names(&std::fs::read(d.join("manifest.alt")).unwrap()).1;
    let seg = &named.first().expect("a segment named").0;
    std::fs::remove_file(d.join(seg)).unwrap();
    oo_refused(
        &d,
        &rename,
        &format!("the manifest slot manifest.alt names {seg}, which is missing"),
        "moved from the slots",
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// (a), read off the calls: a rename publish fsyncs the directory after its
/// rename and before the flush it lands for is answered, so the names it
/// made are on the device before anything retires behind them. Read where
/// nothing else fsyncs the directory between the publish's rename and the
/// flush's answer: no merges, and no log rotation inside the flush.
fn a_rename_publish_syncs_its_directory(tag: &str, base: Options) {
    let opts = Options {
        compact: false,
        ..ra_opts(&base)
    };
    let d = dir(&format!("ra-pubsync-{tag}"));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = BTreeMap::new();
    os_round(&mut db, &mut model, "r0", 0, OS_KEYS / 4);
    db.flush().unwrap();
    db.settle().unwrap();
    type At = std::sync::Arc<std::sync::Mutex<Vec<usize>>>;
    let at: At = Default::default();
    let watch: supdb::db::SlotWatch = {
        let at = at.clone();
        std::sync::Arc::new(move |_: &supdb::db::SlotWrite| {
            let n = Db::trace_dir_io(true).len();
            at.lock().unwrap_or_else(|e| e.into_inner()).push(n);
        })
    };
    db.watch_slot_writes(Some(watch));
    for r in 1..=3u32 {
        os_round(
            &mut db,
            &mut model,
            &format!("r{r}"),
            r * OS_KEYS,
            OS_KEYS / 8,
        );
        let before = at.lock().unwrap().len();
        db.flush().unwrap();
        let marks: Vec<usize> = at.lock().unwrap()[before..].to_vec();
        assert!(
            !marks.is_empty(),
            "{tag}: round {r}: premise: the flush published"
        );
        let trace = Db::trace_dir_io(true);
        for (i, &m) in marks.iter().enumerate() {
            let end = marks.get(i + 1).copied().unwrap_or(trace.len());
            assert!(
                trace[m..end]
                    .iter()
                    .any(|(what, p)| *what == "sync_dir" && p.starts_with(&d)),
                "{tag}: round {r}: publish {i} of the flush fsynced the directory after its \
                 rename and before the next publish or the flush's answer"
            );
        }
        db.settle().unwrap();
    }
    db.watch_slot_writes(None);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_rename_publish_fsyncs_the_directory_before_its_flush_is_answered() {
    a_rename_publish_syncs_its_directory("plain", Options::default());
}

#[test]
fn a_rename_publish_fsyncs_the_directory_before_its_flush_is_answered_under_a_cipher() {
    a_rename_publish_syncs_its_directory("cipher", under_cipher());
}

/// (e) The rename open's removal of the slot file a life under the slots
/// left fails (`Options::fail_open_alt_removal`), as a device error would:
/// the open answers it, since a store that went on with `manifest.alt`
/// there would leave a later open under the slots nothing to make and no
/// directory to fsync before it removes what this life's renames stopped
/// naming. Nothing else is removed before it: a spare and an envelope's
/// temp name left in the directory, each a file the open removes once it
/// goes on, are there after the failure. Where the slot it took was
/// `manifest` nothing has changed; where it took `manifest.alt`, that is
/// written as `manifest` first and nothing else has changed. Either way
/// the store opens after, under either shape, with every write.
fn a_rename_open_whose_alt_removal_fails(tag: &str, base: Options) {
    let slots = os_opts(&base);
    let rename = ra_opts(&base);
    let failing = Options {
        fail_open_alt_removal: true,
        ..rename.clone()
    };
    for alt in [false, true] {
        let d = dir(&format!("ra-altrm-{tag}-{alt}"));
        let mut db = Db::create(&d, slots.clone()).unwrap();
        let mut model = BTreeMap::new();
        os_round(&mut db, &mut model, "r0", 0, OS_KEYS);
        db.flush().unwrap();
        db.settle().unwrap();
        ra_slots_life_ends_in(tag, &d, &mut db, &mut model, alt);
        drop(db);
        assert!(
            d.join("manifest.alt").exists(),
            "{tag}: premise: the slots life left manifest.alt"
        );
        // What an open that goes on removes: a spare past the pool
        // (`recycle_wal` is off) and an envelope's temp name, which the
        // cipher's sweep takes.
        let planted = ["spare-0000000000009999", "x-0000000000000000.z.tmp"];
        for p in planted {
            std::fs::write(d.join(p), vec![0u8; 4096]).unwrap();
        }
        let probe = dir(&format!("ra-altrm-{tag}-{alt}-probe"));
        os_copy(&d, &probe);
        drop(Db::open(&probe, rename.clone()).expect("the state opens"));
        let gone: Vec<&str> = planted
            .iter()
            .copied()
            .filter(|p| !probe.join(p).exists())
            .collect();
        assert!(
            !gone.is_empty(),
            "{tag} (alt {alt}): premise: an open that goes on removes a planted file"
        );
        let _ = std::fs::remove_dir_all(&probe);
        let found = os_files(&d);
        let e = match Db::open(&d, failing.clone()) {
            Ok(_) => panic!("{tag} (alt {alt}): the open answered no failure"),
            Err(e) => e.to_string(),
        };
        assert!(
            e.contains("a test failed the open's removal of manifest.alt"),
            "{tag} (alt {alt}): the open refused for another reason: {e}"
        );
        let changed = osl_changed(&found, &d);
        let allowed: &[&str] = if alt { &["manifest"] } else { &[] };
        assert!(
            changed.iter().all(|f| allowed.contains(&f.as_str())),
            "{tag} (alt {alt}): the open changed the directory before its failure: {changed:?}"
        );
        assert!(
            d.join("manifest.alt").exists(),
            "{tag} (alt {alt}): premise: the removal failed"
        );
        let mut failures = Vec::new();
        for (i, (o, what)) in [(&rename, "the rename shape"), (&slots, "the slots")]
            .into_iter()
            .enumerate()
        {
            let c = dir(&format!("ra-altrm-{tag}-{alt}-{i}"));
            os_copy(&d, &c);
            failures.extend(os_check_all(
                &[(
                    c.clone(),
                    format!("{tag} (alt {alt}): after a failed removal, opened under {what}"),
                )],
                &model,
                o,
            ));
            let _ = std::fs::remove_dir_all(&c);
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it() {
    a_rename_open_whose_alt_removal_fails("plain", Options::default());
}

#[test]
fn a_rename_open_whose_removal_of_the_slots_other_file_fails_answers_it_under_a_cipher() {
    a_rename_open_whose_alt_removal_fails("cipher", under_cipher());
}

// ---------------------------------------------------------------------
// due-log: a run's commit that its policy makes due syncs the log first
// wherever the log owed a sync when the run opened -- frames of a table
// frozen before the run, a file a rotation left loose, frames an open
// replayed -- so a power cut that keeps a run's commit the policy synced
// keeps every commit before it. Before that sync the run's helper thread
// and the kernel's writeback put the run's file on the device unordered
// with the log, which the helper test below holds as the window it is.
// ---------------------------------------------------------------------

/// What the log owes when the run opens: three commits of a hashed table
/// in the live file; commits enough of one to rotate it by size, the
/// closed file loose; or, under `Always` after a reopen, the frames a
/// buffered life left unsynced and the open replayed.
#[derive(Clone, Copy, Debug)]
enum LogOwes {
    Frames,
    Loose,
    Reopen,
}

/// The hashed table's keys of commit `c`: values a byte too long to
/// inline, so every write goes to the log and none opens a run.
fn dl_key(c: u32, k: u32) -> Vec<u8> {
    format!("a-{c}-{k:06}").into_bytes()
}

/// Commits of the run, ten keys each from `*next`, until one is due: the
/// one whose sync moves the run's synced bytes, which are none before the
/// run's first sync. Returns the commits it took.
fn dl_until_due(db: &mut Db, next: &mut u32, what: &str) -> u32 {
    let before = db.run_synced_bytes().unwrap_or(0);
    for n in 1..=64 {
        sr_commits(db, *next, *next + 10, 10);
        *next += 10;
        let synced = db.run_synced_bytes().expect("the run is open");
        if synced != before {
            return n;
        }
    }
    panic!("{what}: no commit of the run was due")
}

/// A power cut of `db`, whose run is open and whose log syncs were traced
/// in `lives`: its files copied from `d` into `crash`, the run's file cut
/// to what its fdatasyncs covered -- the writer's and its helper
/// thread's -- every log file to what its own did.
fn dl_power_cut(db: &Db, d: &Path, crash: &Path, lives: &[&[(PathBuf, Option<u64>)]]) {
    use std::os::unix::fs::MetadataExt;
    let synced = db.run_device_synced().expect("the run is open").0;
    let ino = (open_run(d)).then(|| run_ino(d));
    copy_store(d, crash);
    if let Some(ino) = ino {
        for e in std::fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            if e.metadata().map(|m| m.ino()).ok() == Some(ino) {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(crash.join(e.file_name()))
                    .unwrap()
                    .set_len(synced)
                    .unwrap();
            }
        }
    }
    let traced = db.wal_syncs_traced();
    let mut all: Vec<&[(PathBuf, Option<u64>)]> = lives.to_vec();
    all.push(&traced);
    power_cut_logs(crash, &all, WAL_HEAD, &[]);
}

/// What `Db::wal_syncs_traced` records of a life.
type Traced = Vec<(PathBuf, Option<u64>)>;

/// The log made to owe as `owes` says, a hashed table frozen and held
/// before its fsyncs so no manifest names it, then the run's commits until
/// one is due, then a power cut: every key written before that commit,
/// the log's and the run's, reads back once. With `fail_log_sync` the
/// run's second commit is the due one and its log sync fails: the cut
/// keeps nothing of the run unless it keeps the log before it.
fn a_due_commit_beside_a_log_that_owes(
    tag: &str,
    arm: Options,
    owes: LogOwes,
    fail_log_sync: bool,
) {
    let d = dir(&format!("dur-duelog-{tag}"));
    let crash = dir(&format!("dur-duelog-{tag}-crash"));
    let big = vec![b'x'; arm.inline_bytes + 1];
    let (mut db, opts, one, logged): (Db, Options, Traced, Vec<Vec<u8>>) = match owes {
        LogOwes::Frames => {
            let opts = sr_opts(SyncPolicy::EveryN(5), arm);
            let mut db = Db::create(&d, opts.clone()).unwrap();
            db.trace_wal_syncs(true);
            let mut logged = Vec::new();
            for c in 0..3u32 {
                for k in 0..50u32 {
                    db.append(&dl_key(c, k), &big);
                    logged.push(dl_key(c, k));
                }
                db.commit().unwrap();
            }
            (db, opts, Vec::new(), logged)
        }
        LogOwes::Loose => {
            // The commits a rotation by size takes, counted on a store
            // of the same writes whose policy never syncs; the store
            // itself syncs every three commits past that many, so none
            // of them is due and the run's third commit is.
            let shape = |sync| Options {
                sync,
                seal_rotates_wal: false,
                seal_bytes: 64 << 10,
                ..sr_opts(SyncPolicy::Always, arm.clone())
            };
            let val = "x".repeat(100);
            let put_until_rotated = |db: &mut Db| {
                let mut i = 0u32;
                while db.seal_waits().wal_rotations == 0 {
                    put_commit(db, &format!("a{i:07}"), &val).unwrap();
                    i += 1;
                    assert!(i < 10_000, "{tag}: the log never rotated");
                }
                i
            };
            let probe = dir(&format!("dur-duelog-{tag}-probe"));
            let n = put_until_rotated(&mut Db::create(&probe, shape(buffered().sync)).unwrap());
            let _ = std::fs::remove_dir_all(&probe);
            let opts = shape(SyncPolicy::EveryN(n + 3));
            let mut db = Db::create(&d, opts.clone()).unwrap();
            db.trace_wal_syncs(true);
            assert_eq!(
                put_until_rotated(&mut db),
                n,
                "{tag}: the same writes rotate alike"
            );
            assert!(
                db.seal_waits().rotated_unsynced > 0,
                "{tag}: the premise, a loose rotation"
            );
            assert_eq!(db.levels(), (0, 0), "{tag}: the premise, nothing sealed");
            let logged = (0..n).map(|i| format!("a{i:07}").into_bytes()).collect();
            (db, opts, Vec::new(), logged)
        }
        LogOwes::Reopen => {
            let one = life_one(&d, buffered());
            let opts = sr_opts(SyncPolicy::Always, arm);
            let mut db = life_two(&d, opts.clone());
            db.trace_wal_syncs(true);
            let logged = (0..100)
                .flat_map(|i| [bkey('a', i), bkey('b', i)])
                .collect();
            (db, opts, one, logged)
        }
    };
    assert!(db.wal_unsynced(), "{tag}: the premise, the log owes a sync");
    db.hold_seal_durable(true);
    let mut next = 0u32;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        assert!(
            db.wal_unsynced(),
            "{tag}: the premise, the seal synced nothing of the log"
        );
        if fail_log_sync {
            // The run's first commit is not due under `EveryN(5)`; the
            // second is, and its log sync fails.
            sr_commits(&mut db, 0, 10, 10);
            next = 10;
            db.fail_next_wal_sync();
            let r = sr_commits_answer(&mut db, 10, 20);
            next = 20;
            match r {
                Ok(()) => panic!("{tag}: the due commit answered Ok over a failed log sync"),
                Err(e) => assert!(
                    e.to_string().contains("injected"),
                    "{tag}: the due commit answered another error: {e}"
                ),
            }
        } else {
            let _ = dl_until_due(&mut db, &mut next, tag);
        }
        dl_power_cut(&db, &d, &crash, &[&one]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
    let log_missing = logged
        .iter()
        .filter(|k| read_vec(&opened, k).is_empty())
        .count();
    let run_here = (0..next)
        .filter(|&k| read_vec(&opened, &sr_key(k)) == vec![sr_val(k)])
        .count();
    if fail_log_sync {
        assert!(
            log_missing == 0 || run_here == 0,
            "{tag}: the due commit whose log sync failed left {run_here} of the run's {next} keys \
             on the device and lost {log_missing} of the log's {} keys before them",
            logged.len()
        );
    } else {
        assert!(
            log_missing == 0,
            "{tag}: the due commit kept {run_here} of the run's {next} keys and lost {log_missing} \
             of the {} keys committed to the log before the run",
            logged.len()
        );
        assert_eq!(
            run_here, next as usize,
            "{tag}: the run's keys up to its due commit read back"
        );
    }
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// Keys `from..to` appended to the run in one commit, its answer returned.
fn sr_commits_answer(db: &mut Db, from: u32, to: u32) -> std::io::Result<()> {
    for k in from..to {
        db.append(&sr_key(k), &sr_val(k));
    }
    db.commit()
}

#[test]
fn a_runs_due_commit_syncs_the_log_frames_of_a_table_frozen_before_it() {
    for (tag, arm) in run_arms() {
        a_due_commit_beside_a_log_that_owes(&format!("frames-{tag}"), arm, LogOwes::Frames, false);
    }
}

#[test]
fn a_runs_due_commit_syncs_a_log_file_a_rotation_left_loose_before_it() {
    a_due_commit_beside_a_log_that_owes("loose", Options::default(), LogOwes::Loose, false);
}

#[test]
fn a_runs_commit_under_always_after_a_reopen_syncs_the_replayed_log_first() {
    for (tag, arm) in run_arms() {
        a_due_commit_beside_a_log_that_owes(&format!("reopen-{tag}"), arm, LogOwes::Reopen, false);
    }
}

/// The log's sync comes before the run's: where it fails, the due commit
/// answers the failure and nothing of the run is on the device.
#[test]
fn a_runs_due_commit_whose_log_sync_fails_leaves_the_run_unsynced() {
    a_due_commit_beside_a_log_that_owes("fail", Options::default(), LogOwes::Frames, true);
}

/// A run opened over a log that owes nothing -- synced before the run
/// opens, or after it by a `sync` before its first due commit: the due
/// commits sync the run's file and nothing of the log, under `EveryN` and
/// under `Always`.
#[test]
fn a_runs_due_commit_over_a_log_that_owes_nothing_syncs_no_log() {
    for (p, sync, after) in [
        ("everyn", SyncPolicy::EveryN(5), false),
        ("always", SyncPolicy::Always, false),
        ("everyn-synced-after", SyncPolicy::EveryN(5), true),
    ] {
        let d = dir(&format!("dur-duelog-clean-{p}"));
        let opts = sr_opts(sync, Options::default());
        let mut db = Db::create(&d, opts).unwrap();
        // A table of the log first, synced by a `sync` before the run
        // opens, or with `after` by one once it has.
        let big = vec![b'x'; Options::default().inline_bytes + 1];
        for k in 0..50u32 {
            db.append(&dl_key(0, k), &big);
        }
        db.commit().unwrap();
        if !after {
            db.sync().unwrap();
        }
        db.hold_seal_durable(true);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.seal().unwrap();
            let mut next = 0u32;
            if after {
                assert!(db.wal_unsynced(), "{p}: the premise, the log owes a sync");
                sr_commits(&mut db, 0, 10, 10);
                next = 10;
                assert_eq!(db.run_synced_bytes(), Some(0), "{p}: the premise, not due");
                db.sync().unwrap();
            }
            assert!(!db.wal_unsynced(), "{p}: the premise, the log owes nothing");
            db.trace_wal_syncs(true);
            let opened = db.wal_syncs_traced().len();
            let _ = dl_until_due(&mut db, &mut next, p);
            // The policy's count starts over at a due commit: the next
            // is due `n` commits on, never at once.
            let every = match sync {
                SyncPolicy::EveryN(n) => n,
                SyncPolicy::Always => 1,
            };
            assert_eq!(
                dl_until_due(&mut db, &mut next, p),
                every,
                "{p}: the run's due commits are the policy's"
            );
            let traced = db.wal_syncs_traced();
            assert_eq!(
                traced.len(),
                opened,
                "{p}: a due commit of the run synced the log, which owed nothing: {traced:?}"
            );
        }));
        db.hold_seal_durable(false);
        if let Err(p) = caught {
            std::panic::resume_unwind(p);
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Under a cipher a run's commits are frames of the log, and a due one
/// syncs the log once, behind them (`Db::log_run`): the frames of the
/// table frozen before the run are covered with it, and nothing syncs the
/// log a second time ahead of the run's own frames.
#[test]
fn a_runs_due_commit_under_a_cipher_syncs_the_log_once_behind_its_frames() {
    let d = dir("dur-duelog-cipher");
    let crash = dir("dur-duelog-cipher-crash");
    let opts = sr_opts(SyncPolicy::EveryN(5), under_cipher());
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let big = vec![b'x'; opts.inline_bytes + 1];
    let mut logged = Vec::new();
    for c in 0..3u32 {
        for k in 0..50u32 {
            db.append(&dl_key(c, k), &big);
            logged.push(dl_key(c, k));
        }
        db.commit().unwrap();
    }
    db.hold_seal_durable(true);
    let mut next = 0u32;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        assert!(db.wal_unsynced(), "the premise, the log owes a sync");
        sr_commits(&mut db, 0, 10, 10);
        next = 10;
        let before = db.wal_syncs_traced().len();
        sr_commits(&mut db, 10, 20, 10);
        next = 20;
        let traced = db.wal_syncs_traced();
        let live = db.wal_durable().0;
        let syncs: Vec<_> = traced[before..]
            .iter()
            .filter(|(p, ev)| *p == live && ev.is_some())
            .collect();
        assert_eq!(
            syncs.len(),
            1,
            "the due commit syncs the live log once: {:?}",
            &traced[before..]
        );
        assert!(!db.wal_unsynced(), "the due commit left the log owing");
        let (_, synced, written) = db.wal_durable();
        assert_eq!(synced, written, "the log's every frame is synced");
        copy_store(&d, &crash);
        power_cut_logs(&crash, &[&traced], WAL_HEAD, &[]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the cut opens: {e}"));
    let log_missing = logged
        .iter()
        .filter(|k| read_vec(&opened, k).is_empty())
        .count();
    assert_eq!(log_missing, 0, "the log's keys read back after the cut");
    assert!(sr_missing(&opened, next, "the run's keys").is_none());
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A run long enough for its writer's helper thread to sync it
/// (`SegmentOptions::sync_ahead`, here 64 KiB) before any commit of it is
/// due, beside a hashed table's frames no sync covered: the helper's
/// fdatasyncs are not the policy's, and a power cut that keeps what they
/// covered keeps run commits after the log's older ones and loses those.
/// That is the window the docs name as the exception under `SyncPolicy`
/// and `ROADMAP.md` keeps open; this test holds it, so that whatever
/// closes it restates this half. The run's first due commit then syncs
/// the log before the run, and a power cut after it keeps every commit.
#[test]
fn a_runs_commits_its_helper_synced_before_a_due_one_outlive_older_log_frames_until_it() {
    let d = dir("dur-duelog-helper");
    let crash = dir("dur-duelog-helper-crash");
    let crash2 = dir("dur-duelog-helper-crash2");
    let every = 1000;
    let opts = Options {
        segment: SegmentOptions {
            sync_ahead: 64 << 10,
            write_piece: 4096,
            ..SegmentOptions::default()
        },
        ..sr_opts(SyncPolicy::EveryN(every), Options::default())
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let big = vec![b'x'; opts.inline_bytes + 1];
    let mut logged = Vec::new();
    for c in 0..3u32 {
        for k in 0..50u32 {
            db.append(&dl_key(c, k), &big);
            logged.push(dl_key(c, k));
        }
        db.commit().unwrap();
    }
    db.hold_seal_durable(true);
    let mut next = 0u32;
    let mut first = 0u32;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        assert!(db.wal_unsynced(), "the premise, the log owes a sync");
        // Commits of twenty keys, none due, until the file is four times
        // the helper's stride; then the helper's first fdatasync waited
        // for by its count, never by a clock.
        let mut commits = 0u32;
        while db.run_written_and_held().map_or(0, |(w, _)| w) < 4 * (64 << 10) {
            sr_commits(&mut db, next, next + 20, 20);
            next += 20;
            commits += 1;
            assert!(commits < every / 2, "the run reached four strides");
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while db.run_device_synced().expect("the run is open").1 == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the helper made no fdatasync"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            db.run_synced_bytes(),
            Some(0),
            "the premise, no commit of the run was due"
        );
        assert!(db.wal_unsynced(), "the premise, the log still owes a sync");
        first = next;
        dl_power_cut(&db, &d, &crash, &[]);
        // Then on to the run's first due commit, and a power cut there.
        let _ = dl_until_due_within(&mut db, &mut next, every + 1);
        dl_power_cut(&db, &d, &crash2, &[]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let count = |at: &Path, run_to: u32| {
        let opened = Db::open(at, opts.clone()).unwrap_or_else(|e| panic!("the cut opens: {e}"));
        let log_missing = logged
            .iter()
            .filter(|k| read_vec(&opened, k).is_empty())
            .count();
        let run_here = (0..run_to)
            .filter(|&k| read_vec(&opened, &sr_key(k)) == vec![sr_val(k)])
            .count();
        (log_missing, run_here)
    };
    let (log_missing, run_here) = count(&crash, first);
    assert!(
        run_here > 0 && log_missing == logged.len(),
        "the window: a power cut after the helper's fdatasync keeps run commits ({run_here} of \
         {first} keys) and loses the log's older ones ({log_missing} of {}) -- if it no longer \
         does, the window is closed: restate this half, the docs and ROADMAP.md",
        logged.len()
    );
    let (log_missing, run_here) = count(&crash2, next);
    assert_eq!(
        log_missing, 0,
        "the run's first due commit, after the helper's syncs, kept {run_here} of the run's {next} \
         keys and lost {log_missing} of the log's {} keys before them",
        logged.len()
    );
    assert_eq!(
        run_here, next as usize,
        "the run's keys up to its due commit read back"
    );
    for at in [&d, &crash, &crash2] {
        let _ = std::fs::remove_dir_all(at);
    }
}

/// `dl_until_due` with a bound of `max` commits, for a policy that makes
/// one due further on than its sixty-four.
fn dl_until_due_within(db: &mut Db, next: &mut u32, max: u32) -> u32 {
    let before = db.run_synced_bytes().unwrap_or(0);
    for n in 1..=max {
        sr_commits(db, *next, *next + 20, 20);
        *next += 20;
        if db.run_synced_bytes().expect("the run is open") != before {
            return n;
        }
    }
    panic!("no commit of the run was due in {max}")
}

/// A due commit whose log sync could not open a loose file -- an error of
/// the open, not of a sync, so nothing keeps it -- answers it and syncs
/// nothing of the run, and the run's next due commit syncs the log first
/// again: the note is cleared only once the log is synced, and a power cut
/// after that commit keeps every commit before it.
#[test]
fn a_runs_due_commit_whose_log_sync_could_not_open_a_loose_file_syncs_it_at_the_next() {
    let d = dir("dur-duelog-unopened");
    let crash = dir("dur-duelog-unopened-crash");
    let shape = |sync| Options {
        sync,
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        ..sr_opts(SyncPolicy::Always, Options::default())
    };
    let val = "x".repeat(100);
    let put_until_rotated = |db: &mut Db| {
        let mut i = 0u32;
        while db.seal_waits().wal_rotations == 0 {
            put_commit(db, &format!("a{i:07}"), &val).unwrap();
            i += 1;
            assert!(i < 10_000, "the log never rotated");
        }
        i
    };
    let probe = dir("dur-duelog-unopened-probe");
    let n = put_until_rotated(&mut Db::create(&probe, shape(buffered().sync)).unwrap());
    let _ = std::fs::remove_dir_all(&probe);
    let opts = shape(SyncPolicy::EveryN(n + 3));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    assert_eq!(
        put_until_rotated(&mut db),
        n,
        "the same writes rotate alike"
    );
    assert!(
        db.seal_waits().rotated_unsynced > 0,
        "the premise, a loose rotation"
    );
    let logged: Vec<Vec<u8>> = (0..n).map(|i| format!("a{i:07}").into_bytes()).collect();
    let live = db.wal_durable().0;
    let loose: Vec<PathBuf> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("wal-"))
                && std::fs::canonicalize(p).ok() != std::fs::canonicalize(&live).ok()
        })
        .collect();
    assert_eq!(
        loose.len(),
        1,
        "the premise, one closed log file: {loose:?}"
    );
    let loose = &loose[0];
    let aside = loose.with_extension("aside");
    db.hold_seal_durable(true);
    let mut next = 0u32;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        // The run's first two commits are not due; the third is, and the
        // loose file's name is a link to itself while it runs.
        sr_commits(&mut db, 0, 20, 10);
        next = 20;
        assert_eq!(db.run_synced_bytes(), Some(0), "the premise, not due yet");
        std::fs::rename(loose, &aside).unwrap();
        std::os::unix::fs::symlink(loose, loose).unwrap();
        let r = sr_commits_answer(&mut db, 20, 30);
        next = 30;
        std::fs::remove_file(loose).unwrap();
        std::fs::rename(&aside, loose).unwrap();
        match r {
            Ok(()) => panic!("the due commit answered Ok where the loose file would not open"),
            Err(e) => assert_ne!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "the premise, an error of the open that is not an absence: {e}"
            ),
        }
        assert_eq!(
            db.run_synced_bytes(),
            Some(0),
            "the due commit whose log sync failed synced the run"
        );
        let synced_loose = |db: &Db| {
            db.wal_syncs_traced()
                .iter()
                .filter(|(p, _)| std::fs::canonicalize(p).ok() == std::fs::canonicalize(loose).ok())
                .count()
        };
        assert_eq!(synced_loose(&db), 0, "the premise, the loose file unsynced");
        // The policy's count did not start over: the next commit is due.
        sr_commits_answer(&mut db, 30, 40).unwrap();
        next = 40;
        assert_ne!(db.run_synced_bytes(), Some(0), "the next commit was due");
        assert_eq!(
            synced_loose(&db),
            1,
            "the next due commit synced the loose file before the run"
        );
        dl_power_cut(&db, &d, &crash, &[]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the cut opens: {e}"));
    let log_missing = logged
        .iter()
        .filter(|k| read_vec(&opened, k).is_empty())
        .count();
    assert_eq!(
        log_missing,
        0,
        "the run's next due commit lost {log_missing} of the {} keys the loose file held",
        logged.len()
    );
    assert!(sr_missing(&opened, next, "the run's keys").is_none());
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// How a call with nothing new for the run writes a batch its file owes
/// again (`Db::run_again`): a commit of nothing, or a `sync`.
#[derive(Clone, Copy, Debug)]
enum DlAgain {
    Commit,
    Sync,
}

/// A run opened beside a hashed table's frames no sync covered, whose
/// first commit -- not due under `EveryN(5)`, a batch longer than a write
/// piece -- the device refused and the commit answered `Ok`, owing the
/// batch to the run's file; then a call with nothing new for the run,
/// which writes the batch again and syncs it whatever the policy. That
/// sync is the run's first and stands in for the policy's, so it syncs the
/// log first, as a due commit does: a power cut after it keeps every
/// commit before the run's, and with `fail_log` the log's sync fails, the
/// call answers it and nothing of the run is on the device.
fn a_run_written_again_beside_a_log_that_owes(
    tag: &str,
    arm: Options,
    how: DlAgain,
    fail_log: bool,
) {
    let d = dir(&format!("dur-duelog-again-{tag}"));
    let crash = dir(&format!("dur-duelog-again-{tag}-crash"));
    let opts = Options {
        segment: SegmentOptions {
            write_piece: 4096,
            ..arm.segment.clone()
        },
        ..sr_opts(SyncPolicy::EveryN(5), arm)
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let big = vec![b'x'; opts.inline_bytes + 1];
    let mut logged = Vec::new();
    for c in 0..3u32 {
        for k in 0..50u32 {
            db.append(&dl_key(c, k), &big);
            logged.push(dl_key(c, k));
        }
        db.commit().unwrap();
    }
    db.hold_seal_durable(true);
    let next = 50u32;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        assert!(db.wal_unsynced(), "{tag}: the premise, the log owes a sync");
        db.fail_next_run_write();
        sr_commits_answer(&mut db, 0, next)
            .unwrap_or_else(|e| panic!("{tag}: the premise, a refused piece is owed: {e}"));
        assert_eq!(
            db.run_synced_bytes(),
            Some(0),
            "{tag}: the premise, the run's first commit was not due"
        );
        assert!(db.wal_unsynced(), "{tag}: the premise, the log still owes");
        let again = |db: &mut Db| match how {
            DlAgain::Commit => db.commit(),
            DlAgain::Sync => db.sync(),
        };
        if fail_log {
            // The log's sync, the first thing the call does, fails: the
            // call and the one after it answer the failure, and neither
            // syncs the run's file.
            db.fail_next_wal_sync();
            for call in 0..2 {
                match again(&mut db) {
                    Ok(()) => panic!("{tag}: call {call} answered Ok over a failed log sync"),
                    Err(e) => assert!(
                        e.to_string().contains("the log's fdatasync failed"),
                        "{tag}: call {call} answered another error: {e}"
                    ),
                }
                assert_eq!(
                    db.run_synced_bytes(),
                    Some(0),
                    "{tag}: call {call}, whose log sync failed, synced the run's file"
                );
            }
        } else {
            // The write's failure is held for the next sync of the run's
            // file, which answers it and writes nothing; the call after
            // that writes the batch again.
            match again(&mut db) {
                Ok(()) => panic!("{tag}: the premise, the next call answers the held write"),
                Err(e) => assert!(
                    e.to_string().contains("the run's write failed"),
                    "{tag}: the premise, the held write's answer: {e}"
                ),
            }
            assert_eq!(
                db.run_synced_bytes(),
                Some(0),
                "{tag}: the premise, nothing synced"
            );
            again(&mut db)
                .unwrap_or_else(|e| panic!("{tag}: the call with nothing new answers: {e}"));
            assert_ne!(
                db.run_synced_bytes(),
                Some(0),
                "{tag}: the premise, the call wrote the owed batch again and synced it"
            );
        }
        dl_power_cut(&db, &d, &crash, &[]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("{tag}: the cut opens: {e}"));
    let log_missing = logged
        .iter()
        .filter(|k| read_vec(&opened, k).is_empty())
        .count();
    let run_here = (0..next)
        .filter(|&k| read_vec(&opened, &sr_key(k)) == vec![sr_val(k)])
        .count();
    if fail_log {
        assert!(
            run_here == 0 || log_missing == 0,
            "{tag}: the call whose log sync failed left {run_here} of the run's {next} keys on \
             the device and lost {log_missing} of the log's {} keys before them",
            logged.len()
        );
    } else {
        assert_eq!(
            log_missing,
            0,
            "{tag}: the batch written again kept {run_here} of the run's {next} keys and lost \
             {log_missing} of the {} keys committed to the log before the run",
            logged.len()
        );
        assert_eq!(
            run_here, next as usize,
            "{tag}: the run's batch written again reads back"
        );
    }
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

#[test]
fn a_runs_owed_batch_written_again_by_a_commit_of_nothing_syncs_the_log_first() {
    for (tag, arm) in run_arms() {
        a_run_written_again_beside_a_log_that_owes(
            &format!("commit-{tag}"),
            arm,
            DlAgain::Commit,
            false,
        );
    }
}

#[test]
fn a_runs_owed_batch_written_again_by_a_sync_syncs_the_log_first() {
    a_run_written_again_beside_a_log_that_owes("sync", Options::default(), DlAgain::Sync, false);
}

/// The log's sync comes before the run's there too: where it fails, the
/// call answers the failure and syncs nothing of the run, whether it is a
/// commit of nothing or a `sync`.
#[test]
fn a_runs_owed_batch_written_again_whose_log_sync_fails_leaves_the_run_unsynced() {
    for (tag, how) in [
        ("fail-commit", DlAgain::Commit),
        ("fail-sync", DlAgain::Sync),
    ] {
        a_run_written_again_beside_a_log_that_owes(tag, Options::default(), how, true);
    }
}

/// A run that closes before any commit of it is due, beside a hashed
/// table's frames no sync covered and that table's seal held between its
/// phases (`hold_seal_durable`), so no manifest names its segment: the
/// flush that closes the run syncs the run's records on the writer's
/// thread (`Options::close_presync`) and the run's seal job would sync the
/// whole file on its own, neither ordered behind the log. A power cut that
/// keeps what the presync covered -- the records on the file when the run
/// closed -- and the log as its own syncs left it keeps run commits after
/// the log's older ones and loses those. That is a way the docs name under
/// `SyncPolicy` beside the helper thread's and the kernel's writeback, and
/// `ROADMAP.md` keeps open; this test holds it, so that whatever closes it
/// restates this test. The copy is taken while every seal is held between
/// its phases, so nothing lands inside it.
#[test]
fn a_runs_commits_its_close_synced_before_a_due_one_outlive_older_log_frames() {
    use std::os::unix::fs::MetadataExt;
    let d = dir("dur-duelog-close");
    let crash = dir("dur-duelog-close-crash");
    let opts = Options {
        segment: SegmentOptions {
            write_piece: 4096,
            ..SegmentOptions::default()
        },
        ..sr_opts(SyncPolicy::EveryN(10), Options::default())
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let big = vec![b'x'; opts.inline_bytes + 1];
    let mut logged = Vec::new();
    for c in 0..3u32 {
        for k in 0..50u32 {
            db.append(&dl_key(c, k), &big);
            logged.push(dl_key(c, k));
        }
        db.commit().unwrap();
    }
    db.hold_seal_durable(true);
    db.seal().unwrap();
    assert!(db.wal_unsynced(), "the premise, the log owes a sync");
    // Two commits of the run, neither due.
    sr_commits(&mut db, 0, 100, 50);
    let next = 100u32;
    assert_eq!(
        db.run_synced_bytes(),
        Some(0),
        "the premise, no commit of the run was due"
    );
    let (upto, _) = db.run_written_and_held().expect("the run is open");
    assert!(upto > 0, "the premise, the run's records are on its file");
    let ino = run_ino(&d);
    let tmp = run_tmp_name(&d);
    let traced = db.wal_syncs_traced();
    let h = db.reader().unwrap();
    let (dd, cc) = (d.clone(), crash.clone());
    let lifter = std::thread::spawn(move || {
        // Released on the lifter's own failure as well, so the flush the
        // hold keeps waiting goes on to the test's failure.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // The run's seal job links its file under a piece's name before
            // its readable signal; from there the run is closed and its
            // records presynced or about to be, and the hold keeps every
            // landing out of the copy.
            let linked = |at: &Path| {
                std::fs::read_dir(at).unwrap().any(|e| {
                    let e = e.unwrap();
                    e.file_name() != tmp && e.metadata().map(|m| m.ino()).ok() == Some(ino)
                })
            };
            wait_for("the closed run's link", || linked(&dd));
            copy_store(&dd, &cc);
            // The presync's extent of the run's file under its temp name; the
            // link's entry is the directory's, which no fsync covered.
            for e in std::fs::read_dir(&dd).unwrap() {
                let e = e.unwrap();
                if e.metadata().map(|m| m.ino()).ok() != Some(ino) {
                    continue;
                }
                let at = cc.join(e.file_name());
                if e.file_name() == tmp {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&at)
                        .unwrap()
                        .set_len(upto)
                        .unwrap();
                } else {
                    std::fs::remove_file(&at).unwrap();
                }
            }
        }));
        h.hold_seal_durable_from_handle(false);
        if let Err(p) = caught {
            std::panic::resume_unwind(p);
        }
    });
    let flushed = db.flush();
    lifter.join().expect("the lifter");
    flushed.unwrap();
    assert_eq!(
        db.wal_syncs_traced().len(),
        traced.len(),
        "the premise, the flush synced nothing of the log before the copy"
    );
    power_cut_logs(&crash, &[&traced], WAL_HEAD, &[]);
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the cut opens: {e}"));
    let log_missing = logged
        .iter()
        .filter(|k| read_vec(&opened, k).is_empty())
        .count();
    let run_here = (0..next)
        .filter(|&k| read_vec(&opened, &sr_key(k)) == vec![sr_val(k)])
        .count();
    assert!(
        run_here > 0 && log_missing == logged.len(),
        "the window: a power cut after the closing run's presync keeps run commits ({run_here} of \
         {next} keys) and loses the log's older ones ({log_missing} of {}) -- if it no longer \
         does, the window is closed: restate this test, the docs and ROADMAP.md",
        logged.len()
    );
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// The sync of a batch the run's file owes, where a `sync` since the run
/// opened paid what the log owed then: it asks the log again and syncs
/// nothing of it, as a due commit does.
#[test]
fn a_runs_owed_batch_written_again_over_a_log_a_sync_paid_syncs_no_log() {
    let d = dir("dur-duelog-again-paid");
    let opts = Options {
        segment: SegmentOptions {
            write_piece: 4096,
            ..SegmentOptions::default()
        },
        ..sr_opts(SyncPolicy::EveryN(5), Options::default())
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let big = vec![b'x'; opts.inline_bytes + 1];
    for k in 0..50u32 {
        db.append(&dl_key(0, k), &big);
    }
    db.commit().unwrap();
    db.hold_seal_durable(true);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        assert!(db.wal_unsynced(), "the premise, the log owes a sync");
        sr_commits(&mut db, 0, 50, 50);
        db.sync().unwrap();
        assert!(!db.wal_unsynced(), "the premise, the sync paid the log");
        db.fail_next_run_write();
        sr_commits_answer(&mut db, 50, 100)
            .unwrap_or_else(|e| panic!("the premise, a refused piece is owed: {e}"));
        let synced = db.run_synced_bytes();
        let before = db.wal_syncs_traced().len();
        match db.commit() {
            Ok(()) => panic!("the premise, the next call answers the held write"),
            Err(e) => assert!(
                e.to_string().contains("the run's write failed"),
                "the premise, the held write's answer: {e}"
            ),
        }
        db.commit()
            .unwrap_or_else(|e| panic!("the call with nothing new answers: {e}"));
        assert_ne!(
            db.run_synced_bytes(),
            synced,
            "the premise, the call wrote the owed batch again and synced it"
        );
        let traced = db.wal_syncs_traced();
        assert_eq!(
            traced.len(),
            before,
            "the batch written again synced the log, which owed nothing: {:?}",
            &traced[before..]
        );
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Under a cipher a run's commits are frames of the log, and a run opened
/// beside a file a rotation left loose notes nothing: its due commit syncs
/// the loose file and the live one behind the run's frames (`Db::log_run`),
/// and a power cut after it keeps every commit before it.
#[test]
fn a_runs_due_commit_under_a_cipher_beside_a_loose_log_file_syncs_it_behind_its_frames() {
    a_due_commit_beside_a_log_that_owes("loose-cipher", under_cipher(), LogOwes::Loose, false);
}

/// The closed log files of `d`: every `wal-` name but the live one's.
fn dl_closed_logs(d: &Path, db: &Db) -> Vec<PathBuf> {
    let live = std::fs::canonicalize(db.wal_durable().0).ok();
    std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("wal-"))
                && std::fs::canonicalize(p).ok() != live
        })
        .collect()
}

/// Each of `files` made a link to itself, so an open of it fails with an
/// error that is not an absence; `dl_mend` puts them back.
fn dl_break(files: &[PathBuf]) {
    for f in files {
        std::fs::rename(f, f.with_extension("aside")).unwrap();
        std::os::unix::fs::symlink(f, f).unwrap();
    }
}

fn dl_mend(files: &[PathBuf]) {
    for f in files {
        std::fs::remove_file(f).unwrap();
        std::fs::rename(f.with_extension("aside"), f).unwrap();
    }
}

/// How many syncs of `files` the store's trace holds.
fn dl_synced_of(db: &Db, files: &[PathBuf]) -> usize {
    let names: Vec<_> = files
        .iter()
        .filter_map(|f| std::fs::canonicalize(f).ok())
        .collect();
    db.wal_syncs_traced()
        .iter()
        .filter(|(p, _)| {
            std::fs::canonicalize(p)
                .ok()
                .is_some_and(|c| names.contains(&c))
        })
        .count()
}

/// The store `Db::open` makes under `Always` of a buffered life that
/// rotated its log by size and was killed: the older file loose, its frames
/// replayed and unsynced. Returns the store, its options, life one's syncs
/// and the keys life one committed.
fn dl_reopened_loose(d: &Path) -> (Db, Options, Traced, Vec<Vec<u8>>) {
    let shape = |sync| Options {
        sync,
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        ..sr_opts(SyncPolicy::Always, Options::default())
    };
    let val = "x".repeat(100);
    let mut db = Db::create(&made(d), shape(buffered().sync)).unwrap();
    db.trace_wal_syncs(true);
    let mut i = 0u32;
    while db.seal_waits().wal_rotations == 0 {
        put_commit(&mut db, &format!("a{i:07}"), &val).unwrap();
        i += 1;
        assert!(i < 10_000, "life one: the log never rotated");
    }
    assert!(
        db.seal_waits().rotated_unsynced > 0,
        "life one: the premise, a loose rotation"
    );
    assert_eq!(db.levels(), (0, 0), "life one: the premise, nothing sealed");
    let one = db.wal_syncs_traced();
    killed(db, d);
    let opts = shape(SyncPolicy::Always);
    let mut db = Db::open(d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    let logged = (0..i).map(|i| format!("a{i:07}").into_bytes()).collect();
    (db, opts, one, logged)
}

/// Under `Always` after a reopen of a buffered life whose log rotated, a
/// run's first commit is due and syncs the log first; where that sync
/// cannot open the loose file -- an error of the open, which nothing keeps
/// -- the commit answers it with nothing of the run synced, and its batch,
/// marked committed and visible, is owed by the run's file: a commit with
/// nothing new after it writes and syncs the batch, the log first, or
/// answers an error -- never `Ok` with nothing synced -- and a power cut
/// after an `Ok` keeps every key of the batch and of the log before it.
#[test]
fn a_runs_due_commit_under_always_whose_log_sync_could_not_open_a_loose_file_owes_its_batch() {
    let d = dir("dur-duelog-unopened-always");
    let crash = dir("dur-duelog-unopened-always-crash");
    let (mut db, opts, one, logged) = dl_reopened_loose(&d);
    let loose = dl_closed_logs(&d, &db);
    assert!(!loose.is_empty(), "the premise, a closed log file");
    assert!(db.wal_unsynced(), "the premise, the log owes a sync");
    db.hold_seal_durable(true);
    let mut answered_ok = false;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        dl_break(&loose);
        let r = sr_commits_answer(&mut db, 0, 10);
        dl_mend(&loose);
        match r {
            Ok(()) => panic!("the due commit answered Ok where the loose file would not open"),
            Err(e) => assert_ne!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "the premise, an error of the open that is not an absence: {e}"
            ),
        }
        assert_eq!(
            db.run_synced_bytes(),
            Some(0),
            "the due commit whose log sync failed synced the run"
        );
        assert_eq!(
            dl_synced_of(&db, &loose),
            0,
            "the premise, the loose file unsynced"
        );
        match db.commit() {
            Err(_) => {}
            Ok(()) => {
                answered_ok = true;
                assert_ne!(
                    db.run_synced_bytes(),
                    Some(0),
                    "a commit of nothing answered Ok under Always over the batch its due commit \
                     failed to sync, with nothing of the run synced"
                );
                assert!(
                    dl_synced_of(&db, &loose) > 0,
                    "a commit of nothing synced the run's batch and not the loose file before it"
                );
            }
        }
        dl_power_cut(&db, &d, &crash, &[&one]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the cut opens: {e}"));
    if answered_ok {
        let log_missing = logged
            .iter()
            .filter(|k| read_vec(&opened, k).is_empty())
            .count();
        assert_eq!(
            log_missing,
            0,
            "the Ok over the batch lost {log_missing} of the {} keys the log held before it",
            logged.len()
        );
        if let Some(m) = sr_missing(
            &opened,
            10,
            "the batch the commit of nothing answered Ok over",
        ) {
            panic!("{m}");
        }
    }
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// A run opened beside a file a rotation left loose -- the live file holds
/// no frame -- whose first commit, not due, the device refused a piece of:
/// the batch is owed, and the call with nothing new that writes it again
/// syncs the log first. Where that sync cannot open the loose file, the
/// call answers it, and the next call syncs the loose file before it
/// syncs anything of the run: the note stays until the log is synced, and
/// a power cut after the batch is written again keeps every commit before
/// it.
#[test]
fn a_runs_owed_batch_written_again_whose_log_sync_could_not_open_a_loose_file_syncs_it_at_the_next()
{
    let d = dir("dur-duelog-again-unopened");
    let crash = dir("dur-duelog-again-unopened-crash");
    let shape = |sync| Options {
        sync,
        seal_rotates_wal: false,
        seal_bytes: 64 << 10,
        segment: SegmentOptions {
            write_piece: 4096,
            ..SegmentOptions::default()
        },
        ..sr_opts(SyncPolicy::Always, Options::default())
    };
    let val = "x".repeat(100);
    let put_until_rotated = |db: &mut Db| {
        let mut i = 0u32;
        while db.seal_waits().wal_rotations == 0 {
            put_commit(db, &format!("a{i:07}"), &val).unwrap();
            i += 1;
            assert!(i < 10_000, "the log never rotated");
        }
        i
    };
    let probe = dir("dur-duelog-again-unopened-probe");
    let n = put_until_rotated(&mut Db::create(&probe, shape(buffered().sync)).unwrap());
    let _ = std::fs::remove_dir_all(&probe);
    let opts = shape(SyncPolicy::EveryN(n + 3));
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.trace_wal_syncs(true);
    assert_eq!(
        put_until_rotated(&mut db),
        n,
        "the same writes rotate alike"
    );
    assert!(
        db.seal_waits().rotated_unsynced > 0,
        "the premise, a loose rotation"
    );
    let logged: Vec<Vec<u8>> = (0..n).map(|i| format!("a{i:07}").into_bytes()).collect();
    let loose = dl_closed_logs(&d, &db);
    assert_eq!(
        loose.len(),
        1,
        "the premise, one closed log file: {loose:?}"
    );
    db.hold_seal_durable(true);
    let next = 50u32;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        db.fail_next_run_write();
        sr_commits_answer(&mut db, 0, next)
            .unwrap_or_else(|e| panic!("the premise, a refused piece is owed: {e}"));
        assert_eq!(
            db.run_synced_bytes(),
            Some(0),
            "the premise, the run's first commit was not due"
        );
        dl_break(&loose);
        let r = db.commit();
        dl_mend(&loose);
        match r {
            Ok(()) => panic!("the call answered Ok where the loose file would not open"),
            Err(e) => assert_ne!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "the premise, an error of the open that is not an absence: {e}"
            ),
        }
        assert_eq!(
            db.run_synced_bytes(),
            Some(0),
            "the call whose log sync failed synced the run"
        );
        assert_eq!(
            dl_synced_of(&db, &loose),
            0,
            "the premise, the loose file unsynced"
        );
        // The held write's answer, and then the batch written again.
        match db.commit() {
            Ok(()) => panic!("the premise, the next call answers the held write"),
            Err(e) => assert!(
                e.to_string().contains("the run's write failed"),
                "the premise, the held write's answer: {e}"
            ),
        }
        assert_eq!(
            dl_synced_of(&db, &loose),
            1,
            "the next call synced the loose file before the run"
        );
        db.commit()
            .unwrap_or_else(|e| panic!("the call with nothing new answers: {e}"));
        assert_ne!(
            db.run_synced_bytes(),
            Some(0),
            "the premise, the call wrote the owed batch again and synced it"
        );
        dl_power_cut(&db, &d, &crash, &[]);
    }));
    db.hold_seal_durable(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the cut opens: {e}"));
    let log_missing = logged
        .iter()
        .filter(|k| read_vec(&opened, k).is_empty())
        .count();
    assert_eq!(
        log_missing,
        0,
        "the batch written again lost {log_missing} of the {} keys the loose file held",
        logged.len()
    );
    if let Some(m) = sr_missing(&opened, next, "the run's batch written again") {
        panic!("{m}");
    }
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

/// Run A, under `EveryN`, closed into a seal by `Db::seal` before any
/// commit of it was due, its seal held before it writes anything
/// (`hold_seal_landing`), so its commits are on its file and synced by
/// nothing; then run B above it, to its first due commit. B's note is of
/// the log, which owes nothing here, and says nothing of A's file, so the
/// due commit syncs B's file alone, and a power cut after it keeps B's
/// commits and loses A's older ones: not a prefix. The docs name it beside
/// the close's presync, and `ROADMAP.md` keeps it open; this test holds
/// the window, so that whatever closes it restates this test. The copy is
/// taken while every seal is held before its write, so nothing lands
/// inside it.
#[test]
fn a_runs_due_commit_after_a_run_closed_into_a_held_seal_outlives_that_runs_commits() {
    use std::os::unix::fs::MetadataExt;
    let d = dir("dur-duelog-closed-run");
    let crash = dir("dur-duelog-closed-run-crash");
    let opts = Options {
        segment: SegmentOptions {
            write_piece: 4096,
            ..SegmentOptions::default()
        },
        ..sr_opts(SyncPolicy::EveryN(5), Options::default())
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    db.sync().unwrap();
    db.trace_wal_syncs(true);
    // Run A: two commits of batches longer than a write piece, neither
    // due, so their pieces are on A's file and synced by nothing.
    let a_keys = 100u32;
    sr_commits(&mut db, 0, a_keys, 50);
    assert_eq!(db.run_synced_bytes(), Some(0), "the premise, A not due");
    let (a_written, _) = db.run_written_and_held().expect("A is open");
    assert!(a_written > 0, "the premise, A's records are on its file");
    let a_ino = run_ino(&d);
    let a_tmp = d.join(run_tmp_name(&d));
    let a_len = std::fs::metadata(&a_tmp).unwrap().len();
    let a_synced = db.run_device_synced().expect("A is open").0;
    assert!(!db.wal_unsynced(), "the premise, the log owes nothing");
    db.hold_seal_landing(true);
    let mut next = a_keys;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        db.seal().unwrap();
        // Run B: commits from A's greatest up, until one is due.
        let _ = dl_until_due(&mut db, &mut next, "run B");
        let inos: Vec<u64> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("direct-") && n.ends_with(".tmp")
            })
            .map(|e| e.metadata().unwrap().ino())
            .collect();
        assert!(
            inos.contains(&a_ino),
            "the premise, A's temp name stays until a manifest names it"
        );
        let b_ino = *inos
            .iter()
            .find(|&&i| i != a_ino)
            .expect("the premise, B is a run of its own");
        assert_eq!(
            std::fs::metadata(&a_tmp).unwrap().len(),
            a_len,
            "the premise, A's seal held before it writes anything: the cut below is a crash \
             state only while nothing has synced A's file"
        );
        let b_synced = db.run_device_synced().expect("B is open").0;
        assert!(b_synced > 0, "the premise, B's due commit synced its file");
        let traced = db.wal_syncs_traced();
        copy_store(&d, &crash);
        // Each run's file, under every name it has, cut to what its syncs
        // covered; the log to what its own did.
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let ino = e.metadata().map(|m| m.ino()).ok();
            let keep = if ino == Some(a_ino) {
                a_synced
            } else if ino == Some(b_ino) {
                b_synced
            } else {
                continue;
            };
            std::fs::OpenOptions::new()
                .write(true)
                .open(crash.join(e.file_name()))
                .unwrap()
                .set_len(keep)
                .unwrap();
        }
        power_cut_logs(&crash, &[&traced], WAL_HEAD, &[]);
    }));
    db.hold_seal_landing(false);
    if let Err(p) = caught {
        std::panic::resume_unwind(p);
    }
    drop(db);
    let opened = Db::open(&crash, opts).unwrap_or_else(|e| panic!("the cut opens: {e}"));
    let a_here = (0..a_keys)
        .filter(|&k| read_vec(&opened, &sr_key(k)) == vec![sr_val(k)])
        .count();
    let b_here = (a_keys..next)
        .filter(|&k| read_vec(&opened, &sr_key(k)) == vec![sr_val(k)])
        .count();
    assert!(
        b_here == (next - a_keys) as usize && a_here == 0,
        "the window: a power cut after run B's due commit keeps B's commits ({b_here} of {}) and \
         loses run A's older ones ({a_here} of {a_keys} kept) -- if it no longer does, the window \
         is closed: restate this test, the docs and ROADMAP.md",
        next - a_keys
    );
    drop(opened);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&crash);
}

// ---------------------------------------------------------------------
// helper-race: the helper thread behind a segment's writer syncs the file
// on an open file description of its own, so a writeback error its
// fdatasync meets is reported to the writer's own fdatasync as well, and
// a commit whose sync returns in the instant before the helper stores its
// error is not acknowledged over it. And the helper's failed sync at a
// seal's finish and at a merge's is the landing's failure.

/// Lets a held helper go when the test ends, on its own failure as well:
/// a writer's drop joins its helper.
struct ReleaseHelper(std::sync::Arc<HelperFault>);

impl Drop for ReleaseHelper {
    fn drop(&mut self) {
        self.0.hold_store(false);
    }
}

fn helper_seam(armed: bool, hold: bool) -> std::sync::Arc<HelperFault> {
    let f = std::sync::Arc::new(HelperFault::default());
    f.arm(armed);
    f.hold_store(hold);
    f
}

/// The trace "1 B B Y": a batch written, the helper's fdatasync and the
/// commit's begun, the device's error reported to the helper's, and the
/// commit's returning before the helper stores the error. The commit is
/// the failure's, and every call after it keeps it.
fn a_run_commit_beside_a_helper_sync_that_met_an_error(tag: &str, base: Options) {
    let d = dir(&format!("dur-hrace-{tag}"));
    let fault = helper_seam(true, true);
    let opts = Options {
        sync: SyncPolicy::EveryN(2),
        segment: SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        },
        ..base
    };
    let mut db = Db::create(&d, opts).unwrap();
    let _release = ReleaseHelper(fault.clone());
    // Ascending into an empty store: a direct run.
    for k in 0..3000u32 {
        db.append(&hkey(k), &hval(k));
    }
    db.commit().unwrap(); // the policy's first of two: no sync
    wait_for(
        "the helper's failed fdatasync, its error not yet stored",
        || fault.in_window(),
    );
    assert!(db.run_synced_bytes().is_some(), "the run is open");
    db.append(&hkey(3000), &hval(3000));
    let due = db.commit();
    fault.hold_store(false);
    assert!(
        due.is_err(),
        "{tag}: the policy's sync, returning while the helper held a writeback error it had \
         not stored, is not answered Ok"
    );
    wait_count_fault(&fault, "the helper's error stored");
    kept(db.commit(), "a commit of nothing after");
    kept(db.sync(), "a sync after");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

fn wait_count_fault(f: &HelperFault, what: &str) {
    wait_for(what, || f.failed() >= 1);
}

#[test]
fn a_run_commit_beside_a_helper_sync_that_met_an_error_answers_it() {
    a_run_commit_beside_a_helper_sync_that_met_an_error("plain", Options::default());
}

/// Under a cipher the run's file is a memory file, which has no writeback
/// to fail and whose commits are frames of the log; the helper syncs it on
/// a description of its own all the same, and a seam's error there is the
/// commit's.
#[test]
fn a_run_commit_beside_a_helper_sync_that_met_an_error_answers_it_under_a_cipher() {
    a_run_commit_beside_a_helper_sync_that_met_an_error("cipher", under_cipher());
}

/// The same at the writer: a marked segment's `sync` returning while its
/// helper holds a failed fdatasync's error unstored answers the error,
/// whether the seam was armed before the writer was made or after.
#[test]
fn a_marked_segments_sync_beside_its_helpers_failed_sync_answers_it() {
    let d = dir("dur-hrace-writer");
    for armed_at_create in [true, false] {
        let fault = helper_seam(armed_at_create, true);
        let opts = SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        };
        let mut w =
            SegmentWriter::create(&d.join(format!("run-{armed_at_create}.sup")), &opts).unwrap();
        fault.arm(true);
        let release = ReleaseHelper(fault.clone());
        w.set_inline_max(256);
        w.set_marks(true);
        for k in 0..3000u32 {
            w.begin(&hkey(k)).unwrap();
            w.value(&hval(k));
            w.end_with(false).unwrap();
        }
        w.mark().unwrap();
        wait_for(
            "the helper's failed fdatasync, its error not yet stored",
            || fault.in_window(),
        );
        let first = w.sync();
        drop(release);
        assert!(
        first.is_err(),
        "armed at create {armed_at_create}: a sync returning while the helper held a writeback \
         error it had not stored is not answered Ok"
    );
        assert!(w.sync().is_err(), "nor is the sync after it");
        drop(w);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The helper's own open of the file failing -- no `/proc` -- leaves the
/// writer with no helper: the segment is written, synced by the writer and
/// finished whole, and no helper sync is made to fail.
///
/// Both kinds of writer: a marked one, which hashes its row's pieces on
/// its own thread whatever the helper does, and an unmarked one, which
/// would hand its row over to the helper and keeps it instead.
///
/// And at two strides: at the smaller the helper would be spawned before
/// the section's first whole piece, and at the larger with the writer's
/// row holding several of them, which the fallback keeps where it would
/// have handed them over.
#[test]
fn a_writer_whose_helper_cannot_open_its_file_writes_without_one() {
    let d = dir("dur-hrace-noproc");
    for (marked, ahead) in [(true, 16384), (false, 16384), (true, 98304), (false, 98304)] {
        let fault = helper_seam(true, false);
        fault.refuse_reopen(true);
        let opts = SegmentOptions {
            write_piece: 4096,
            sync_ahead: ahead,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        };
        let path = d.join(format!("seg-{marked}-{ahead}.sup"));
        let mut w = SegmentWriter::create(&path, &opts).unwrap();
        w.set_inline_max(256);
        w.set_marks(marked);
        for k in 0..3000u32 {
            w.begin(&hkey(k)).unwrap();
            w.value(&hval(k));
            w.end_with(false).unwrap();
        }
        if marked {
            w.mark().unwrap();
            w.sync().expect("the writer's sync, with no helper");
        }
        w.finish(1).expect("the finish, with no helper");
        assert_eq!(
            fault.failed(),
            0,
            "marked {marked}: no helper ran to fail a sync"
        );
        assert_eq!(
            fault.refused(),
            1,
            "marked {marked}: the writer asks for the helper's open once, and goes on without one"
        );
        // The open verifies the key section's checksum row.
        let blob = supdb::Blob::open(supdb::bytes::MmapBytes::open(&path).unwrap())
            .unwrap_or_else(|e| panic!("marked {marked}: the segment opens whole: {e}"));
        assert_eq!(blob.count(&hkey(0)).unwrap(), 1);
        assert_eq!(blob.count(&hkey(2999)).unwrap(), 1);
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A helper that begins only once the finish has asked it to stop takes
/// the sync that is due at the stop, as an armed helper does whatever its
/// timing, and the finish answers that sync's failure: the stop is the
/// path the seal's and the merge's tests reach whenever their writer
/// outruns the helper's first turn.
#[test]
fn a_finish_answers_a_helper_sync_taken_at_the_stop() {
    let d = dir("dur-hrace-atstop");
    let fault = helper_seam(true, false);
    fault.start_after_stop(true);
    let opts = SegmentOptions {
        write_piece: 4096,
        sync_ahead: 16384,
        helper_fault: Some(fault.clone()),
        ..Default::default()
    };
    let path = d.join("seg.sup");
    let mut w = SegmentWriter::create(&path, &opts).unwrap();
    w.set_inline_max(256);
    for k in 0..3000u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    assert_eq!(
        fault.failed(),
        0,
        "the helper has not begun before the stop"
    );
    let r = w.finish(1);
    assert!(
        r.is_err(),
        "the finish answers the helper's sync that failed at the stop"
    );
    assert_eq!(
        fault.failed(),
        1,
        "the helper took the one sync due at the stop"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Two test seams on one writer: a helper's writeback seam, which sets
/// the writer to ask at every sync whether a helper's error reaches it, and
/// a failed fdatasync armed on the same writer. The armed failure is the
/// sync's answer, whether or not a helper sync has failed; the store's
/// run the same (`Db::fail_next_run_sync` beside `SegmentOptions::helper_fault`).
#[test]
fn a_sync_failed_by_a_test_beside_a_helper_seam_answers_the_failure() {
    let d = dir("dur-hrace-twoseams");
    for helper_failed in [false, true] {
        let fault = helper_seam(helper_failed, false);
        let opts = SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        };
        let mut w =
            SegmentWriter::create(&d.join(format!("run-{helper_failed}.sup")), &opts).unwrap();
        w.set_inline_max(256);
        w.set_marks(true);
        for k in 0..100u32 {
            w.begin(&hkey(k)).unwrap();
            w.value(&hval(k));
            w.end_with(false).unwrap();
        }
        w.mark().unwrap();
        assert_eq!(fault.failed(), 0, "no helper runs below its stride");
        w.fail_next_sync();
        assert!(
            w.sync().is_err(),
            "helper armed {helper_failed}: the sync a test failed is answered the failure"
        );
        drop(w);
    }
    let fault = helper_seam(false, false);
    let opts = Options {
        sync: SyncPolicy::Always,
        segment: SegmentOptions {
            helper_fault: Some(fault.clone()),
            ..Default::default()
        },
        ..Options::default()
    };
    let st = d.join("store");
    let mut db = Db::create(&st, opts).unwrap();
    db.append(&hkey(0), &hval(0));
    db.commit().unwrap();
    assert!(db.run_synced_bytes().is_some(), "the run is open");
    db.fail_next_run_sync();
    db.append(&hkey(1), &hval(1));
    assert!(
        db.commit().is_err(),
        "the run's commit whose fdatasync a test failed, beside a helper seam, answers it"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A helper's sync that failed is not tried again: the range it was for
/// counts as the helper's, its failure kept as every later sync's and the
/// finish's answer, so a sync falls due again only over what lands after
/// it -- at the stop as well, where an armed helper takes a sync that is
/// due. A retry would be answered `Ok` by the kernel over pages it marked
/// clean; here a helper that retried would retry at its stop for good, so
/// the finish is held to returning.
#[test]
fn a_helpers_failed_sync_is_not_tried_again() {
    let d = dir("dur-hrace-noretry");
    let fault = helper_seam(true, false);
    let opts = SegmentOptions {
        write_piece: 4096,
        sync_ahead: 16384,
        helper_fault: Some(fault.clone()),
        ..Default::default()
    };
    let mut w = SegmentWriter::create(&d.join("seg.sup"), &opts).unwrap();
    w.set_inline_max(256);
    w.set_marks(true);
    for k in 0..3000u32 {
        w.begin(&hkey(k)).unwrap();
        w.value(&hval(k));
        w.end_with(false).unwrap();
    }
    w.mark().unwrap();
    wait_count_fault(&fault, "the helper's due sync, failed");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = w.finish(1);
        let _ = tx.send(r.is_err());
    });
    let err = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the finish returns: the helper takes no sync at its stop over a failed range");
    assert!(err, "the finish answers the helper's failed sync");
    let _ = std::fs::remove_dir_all(&d);
}

/// The descriptors this process holds open on `path`.
fn fds_on(path: &Path) -> Vec<i32> {
    let want = std::fs::canonicalize(path).unwrap();
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc/self/fd").unwrap().flatten() {
        let Ok(fd) = e.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if std::fs::read_link(e.path()).is_ok_and(|t| t == want) {
            out.push(fd);
        }
    }
    out
}

/// A descriptor's open flags, as `/proc/self/fdinfo` reads them.
fn fd_flags(fd: i32) -> u32 {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}")).unwrap();
    let line = info
        .lines()
        .find_map(|l| l.strip_prefix("flags:"))
        .expect("a flags line");
    u32::from_str_radix(line.trim(), 8).unwrap()
}

/// The open flags of `path` opened read-only by the standard library
/// (`O_RDONLY | O_CLOEXEC`), as `/proc/self/fdinfo` reads them with what
/// the kernel adds to every open (`O_LARGEFILE`): what the helper's own
/// open of the file reads as, flag for flag.
fn open_flags(path: &Path) -> u32 {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::File::open(path).unwrap();
    let flags = fd_flags(f.as_raw_fd());
    assert_eq!(
        flags & (libc::O_ACCMODE | libc::O_CLOEXEC) as u32,
        (libc::O_RDONLY | libc::O_CLOEXEC) as u32,
        "the reference open is read-only and closed on exec"
    );
    flags
}

/// Whether two of this process's descriptors are one open file
/// description.
fn one_description(a: i32, b: i32) -> bool {
    let pid = std::process::id() as libc::pid_t;
    // SAFETY: compares two of this process's descriptors; no memory.
    let r = unsafe { libc::syscall(libc::SYS_kcmp, pid, pid, 0, a, b) };
    assert!(r >= 0, "kcmp: {}", std::io::Error::last_os_error());
    r == 0
}

/// The helper's description is the writer's file opened again, readable,
/// closed on exec, and another description than the writer's, whatever
/// the width of the writer's descriptor's number, which names the file
/// the helper opens (`/proc/self/fd/<n>`); and the writer's sync answers
/// the error the helper met there before the helper stores it. Each width
/// is built by holding descriptors open below it until the writer's file
/// is given a number of that width, never a palindrome, so a number read
/// backwards is another number.
#[test]
fn a_helpers_description_is_its_own_at_every_width_of_the_writers_descriptor() {
    use std::os::unix::io::AsRawFd;
    let d = dir("dur-hrace-width");
    let mut held: Vec<std::fs::File> = Vec::new();
    for width in [2u32, 3, 4] {
        let (lo, hi) = (10i32.pow(width - 1), 10i32.pow(width));
        let palindrome = |n: i32| {
            let s = n.to_string();
            s.chars().rev().collect::<String>() == s
        };
        let fault = helper_seam(true, true);
        let opts = SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        };
        let path = d.join(format!("run-{width}.sup"));
        let mut built = None;
        for _ in 0..1000 {
            while held.last().is_none_or(|f| f.as_raw_fd() < lo) {
                held.push(std::fs::File::open("/dev/null").unwrap());
            }
            let w = SegmentWriter::create(&path, &opts).unwrap();
            let fds = fds_on(&path);
            assert_eq!(fds.len(), 1, "the writer alone holds its new file: {fds:?}");
            if (lo..hi).contains(&fds[0]) && !palindrome(fds[0]) {
                built = Some((w, fds[0]));
                break;
            }
            drop(w);
            let _ = std::fs::remove_file(&path);
            // The number just given back is held, so the next is another.
            held.push(std::fs::File::open("/dev/null").unwrap());
        }
        let (mut w, wfd) =
            built.unwrap_or_else(|| panic!("a writer's descriptor of {width} digits"));
        // Dropped before the writer, whose drop joins the held helper.
        let release = ReleaseHelper(fault.clone());
        w.set_inline_max(256);
        w.set_marks(true);
        for k in 0..3000u32 {
            w.begin(&hkey(k)).unwrap();
            w.value(&hval(k));
            w.end_with(false).unwrap();
        }
        w.mark().unwrap();
        wait_for(
            &format!("width {width}: the helper's failed fdatasync, its error not yet stored"),
            || fault.in_window(),
        );
        let fds = fds_on(&path);
        let others: Vec<i32> = fds
            .iter()
            .copied()
            .filter(|&fd| !one_description(fd, wfd))
            .collect();
        assert!(
            !others.is_empty(),
            "width {width}: the helper holds a description of the writer's file of its own              (writer's fd {wfd}, every fd on the file {fds:?})"
        );
        for &fd in &fds {
            assert!(
                fd_flags(fd) & libc::O_CLOEXEC as u32 != 0,
                "width {width}: fd {fd} on the segment's file is closed on exec"
            );
        }
        let want = open_flags(&path);
        for &fd in &others {
            assert_eq!(
                fd_flags(fd),
                want,
                "width {width}: the helper's fd {fd} is the file opened read-only and closed on \
                 exec, and nothing else"
            );
        }
        let first = w.sync();
        drop(release);
        assert!(
            first.is_err(),
            "width {width}: a sync returning while the helper held a writeback error it had not \
             stored is not answered Ok"
        );
        drop(w);
    }
    drop(held);
    let _ = std::fs::remove_dir_all(&d);
}

/// A writer with no test's seam -- every store's writer -- gives its
/// helper the file opened again: a description of the helper's own,
/// read-only and closed on exec, held while the helper runs and closed
/// with it at the finish. Both kinds of writer, as the helper's row is
/// handed over by one and not the other.
#[test]
fn a_helper_with_no_seam_syncs_on_a_description_of_its_own() {
    let d = dir("dur-hrace-noseam");
    for marked in [true, false] {
        let opts = SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            ..Default::default()
        };
        let path = d.join(format!("seg-{marked}.sup"));
        let mut w = SegmentWriter::create(&path, &opts).unwrap();
        let fds = fds_on(&path);
        assert_eq!(
            fds.len(),
            1,
            "marked {marked}: the writer alone holds its new file: {fds:?}"
        );
        let wfd = fds[0];
        w.set_inline_max(256);
        w.set_marks(marked);
        for k in 0..3000u32 {
            w.begin(&hkey(k)).unwrap();
            w.value(&hval(k));
            w.end_with(false).unwrap();
        }
        // A marked writer's records go to the file at its mark.
        if marked {
            w.mark().unwrap();
        }
        let fds = fds_on(&path);
        let others: Vec<i32> = fds.iter().copied().filter(|&fd| fd != wfd).collect();
        assert_eq!(
            others.len(),
            1,
            "marked {marked}: a helper runs and holds one descriptor on the file \
             (writer's fd {wfd}, every fd on the file {fds:?})"
        );
        let h = others[0];
        assert!(
            !one_description(h, wfd),
            "marked {marked}: the helper's fd {h} is a description of its own, not the writer's"
        );
        assert_eq!(
            fd_flags(h),
            open_flags(&path),
            "marked {marked}: the helper's fd {h} is the file opened read-only and closed on \
             exec, and nothing else"
        );
        assert!(
            fd_flags(h) & libc::O_CLOEXEC as u32 != 0,
            "marked {marked}: the helper's fd {h} is closed on exec"
        );
        if marked {
            w.sync().expect("the writer's sync");
        }
        w.finish(1).expect("the finish");
        assert!(
            fds_on(&path).is_empty(),
            "marked {marked}: the helper's description is closed with it: {:?}",
            fds_on(&path)
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A seal whose output's helper fdatasync failed: the finish joins the
/// helper and takes its error before the segment is renamed into place, so
/// the seal fails before its readable landing. No segment is on disk under
/// a live name, no manifest names one or covers the writes, and the flush
/// answers the failure; the reopen replays the log.
#[test]
fn a_seal_whose_helper_sync_failed_is_named_by_no_manifest() {
    let d = dir("dur-hrace-seal");
    let fault = helper_seam(true, false);
    let opts = Options {
        direct_ingest: false,
        segment: SegmentOptions {
            write_piece: 4096,
            sync_ahead: 16384,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        },
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model = SsModel::new();
    for k in (0..3000u32).rev() {
        let v = String::from_utf8(hval(k)).unwrap();
        ss_put(&mut db, &mut model, &hkey(k), &v);
    }
    db.commit().unwrap();
    let flushed = db.flush();
    let settled = db.settle();
    assert!(fault.failed() >= 1, "the seal's helper met its failure");
    assert!(
        seg_names(&d).is_empty(),
        "no segment of the failed seal is under a live name: {:?}",
        seg_names(&d)
    );
    let (_, covered, names) = ss_newest(&d).expect("a manifest");
    assert!(names.is_empty(), "a manifest names {names:?}");
    assert_eq!(
        covered, 0,
        "a manifest covers the writes of the failed seal"
    );
    ss_check(&db, &model, "after the failure");
    for (what, r) in [("the flush", flushed), ("a settle after it", settled)] {
        match r {
            Ok(()) => panic!("{what}: answered Ok over the seal's failed helper sync"),
            Err(e) => assert!(
                e.to_string().contains("Input/output error"),
                "{what}: answered with an error that is not the helper's: {e}"
            ),
        }
    }
    assert!(db.seals_wedged(), "the seal's failure stops the queue");
    drop(db);
    let reopen = Options {
        segment: SegmentOptions::default(),
        ..opts
    };
    let db = Db::open(&d, reopen).expect("the store opens");
    ss_check(&db, &model, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A merge whose output's helper fdatasync failed: the finish takes the
/// helper's error before the piece is renamed into place, the merge lands
/// nothing, the store's segments and its manifest are as they were, and
/// the commit after it answers the failure.
#[test]
fn a_merge_whose_helper_sync_failed_lands_nothing() {
    let d = dir("dur-hrace-merge");
    let fault = helper_seam(false, false);
    let base = Options {
        segment: SegmentOptions {
            write_piece: 64 << 10,
            sync_ahead: 256 << 10,
            helper_fault: Some(fault.clone()),
            ..Default::default()
        },
        ..Options::default()
    };
    let mut model = SsModel::new();
    let armed = fault.clone();
    let (mut db, opts) = ss_store_with_a_merge_due(&d, base, &mut model, move |_| armed.arm(true));
    wait_for("the merge's end", || !db.in_flight().1);
    assert!(fault.failed() >= 1, "the merge's helper met its failure");
    let e = ss_commit_answer(&mut db);
    assert!(
        e.to_string().contains("Input/output error"),
        "the commit after the merge answers its helper's failure: {e}"
    );
    let on_disk = seg_names(&d);
    let mut named = ss_names(&d);
    named.sort();
    let mut disk = on_disk.clone();
    disk.sort();
    assert_eq!(
        disk, named,
        "a segment of the failed merge is under a live name"
    );
    assert_eq!(
        db.levels(),
        (1, 2),
        "the partition and its two pieces stand"
    );
    ss_check(&db, &model, "after the failure");
    fault.arm(false);
    drop(db);
    let db = Db::open(&d, opts).expect("the store opens");
    ss_check(&db, &model, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------
// drain-order: a drain's run job, which lands durable only at the
// writer's ask after its presync (`SealJob::awaits_writer`), is named
// durable by the flush that closed the run, before the flush answers; and
// the presync's test seams, one byte loaded once, hold and fail apart.

/// A flush that closes an ordered run hands its seal a job that waits for
/// the writer's word, and the word is the flush's own ask: when the flush
/// answers `Ok`, the newest slot names the run's segment durable, with
/// nothing after the flush to land it. `inline` runs the segment work on
/// the writer's thread (`Options::publish_in_background` off).
fn do_drain_lands_its_run(tag: &str, inline: bool) {
    let d = dir(&format!("dur-do-lands-{tag}"));
    let opts = Options {
        publish_in_background: !inline,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    for k in 0..3000u32 {
        db.append(&ss_key(k), &ss_val(k));
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    let (_, _, names) = ss_newest(&d).expect("a manifest");
    assert!(
        !names.is_empty() && names.iter().all(|(_, pending)| !pending),
        "{tag}: the flush answered with its run's segment not named durable: {names:?}"
    );
    let on_disk = seg_names(&d);
    for (n, _) in &names {
        assert!(
            on_disk.contains(n),
            "{tag}: the slot names {n}, not in {on_disk:?}"
        );
    }
    for k in [0u32, 1499, 2999] {
        assert_eq!(read_vec(&db, &ss_key(k)), vec![ss_val(k)], "{tag}");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_drain_names_its_runs_segment_durable_before_it_answers() {
    do_drain_lands_its_run("background", false);
}

#[test]
fn a_drain_names_its_runs_segment_durable_before_it_answers_inline() {
    do_drain_lands_its_run("inline", true);
}

/// Under a cipher the run's job awaits the writer as in the clear; the
/// slot is sealed, so the flush's work is read back by a reopen.
#[test]
fn a_drain_lands_its_run_under_a_cipher() {
    let d = dir("dur-do-lands-cipher");
    let opts = under_cipher();
    let mut db = Db::create(&d, opts.clone()).unwrap();
    for k in 0..3000u32 {
        db.append(&ss_key(k), &ss_val(k));
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    assert_ne!(db.levels(), (0, 0), "premise: the run's segment is live");
    db.settle().unwrap();
    drop(db);
    let db = Db::open(&d, opts).unwrap();
    for k in [0u32, 1499, 2999] {
        assert_eq!(read_vec(&db, &ss_key(k)), vec![ss_val(k)], "reopened");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A store for the presync seams' tests: an ordered load under a policy
/// that leaves the run's records to the presync, written in small pieces.
fn ps_opts(inline: bool) -> Options {
    Options {
        sync: SyncPolicy::EveryN(1_000_000),
        close_finish_inline: inline,
        segment: SegmentOptions {
            write_piece: 4096,
            ..Default::default()
        },
        ..Options::default()
    }
}

/// An ordered run of three thousand keys from `from`, committed in
/// batches of three hundred.
fn ps_run(db: &mut Db, from: u32) {
    for k in from..from + 3000 {
        db.append(&ss_key(k), &ss_val(k));
        if k % 300 == 299 {
            db.commit().unwrap();
        }
    }
}

/// A watchdog over a flush held at its presync: should the flush not
/// have answered within `secs`, it lifts the hold itself, so a test whose
/// lift never comes fails at its own assertion instead of hanging, and
/// answers whether it had to.
fn ps_watchdog(
    h: Reader,
    flushed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    secs: u64,
) -> std::thread::JoinHandle<bool> {
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        while !flushed.load(std::sync::atomic::Ordering::Acquire) {
            if t.elapsed() > std::time::Duration::from_secs(secs) {
                h.hold_presync(false);
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    })
}

/// The presync's seams share one byte: a hold with no failure armed waits
/// and then syncs, and the flush answers `Ok` with nothing injected; a
/// failure armed and a hold lifted before the flush fails it without a
/// wait. Each seam's bit is taken apart from the other's.
#[test]
fn the_presync_seams_hold_and_fail_apart() {
    let d = dir("dur-do-seams");
    let mut db = Db::create(&d, ps_opts(false)).unwrap();
    ps_run(&mut db, 0);
    let h = db.reader().unwrap();
    db.hold_presync(true);
    let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flushed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = ps_watchdog(db.reader().unwrap(), flushed.clone(), 30);
    let (seen, done) = (held.clone(), flushed.clone());
    let lifter = std::thread::spawn(move || {
        // The run's segment published while the writer waits at its
        // presync, and the flush still not answered a while after: the
        // hold holds it there until it is lifted. A presync that did not
        // wait has answered by then, and the lift comes after.
        let t = std::time::Instant::now();
        while h.levels() == (0, 0) {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "the run's segment was never published"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let waited = !done.load(std::sync::atomic::Ordering::Acquire);
        seen.store(waited, std::sync::atomic::Ordering::Release);
        h.hold_presync(false);
    });
    db.flush().unwrap();
    flushed.store(true, std::sync::atomic::Ordering::Release);
    lifter.join().unwrap();
    assert!(
        !watchdog.join().unwrap(),
        "the hold was lifted by the watchdog, not by the test's lift"
    );
    assert!(
        held.load(std::sync::atomic::Ordering::Acquire),
        "the flush answered with the presync's hold still on"
    );
    assert_eq!(
        db.failed_presync_extent(),
        None,
        "a hold armed alone injected a failure"
    );
    // A failure armed alone: the next flush's presync fails, and no hold
    // left from the first keeps it waiting.
    ps_run(&mut db, 3000);
    db.fail_next_presync();
    let e = db
        .flush()
        .expect_err("the flush answers its armed presync failure");
    assert!(
        e.to_string()
            .contains("the records' sync failed (injected)"),
        "the flush answers the injected presync: {e}"
    );
    assert!(
        db.failed_presync_extent().is_some(),
        "the failure was injected"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A failure armed and then a hold: the hold arms beside the failure
/// without clearing it, the writer waits before it takes the failure --
/// nothing injected while it is held -- and the failure fires once the
/// hold is lifted.
#[test]
fn a_held_presync_takes_the_failure_armed_before_the_hold_after_its_lift() {
    let d = dir("dur-do-seams-both");
    let mut db = Db::create(&d, ps_opts(false)).unwrap();
    ps_run(&mut db, 0);
    db.fail_next_presync();
    db.hold_presync(true);
    let h = db.reader().unwrap();
    let flushed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = ps_watchdog(db.reader().unwrap(), flushed.clone(), 30);
    let done = flushed.clone();
    let lifter = std::thread::spawn(move || {
        let t = std::time::Instant::now();
        while h.levels() == (0, 0) {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "the run's segment was never published"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let answered = done.load(std::sync::atomic::Ordering::Acquire);
        let injected = h.failed_presync_extent();
        h.hold_presync(false);
        (answered, injected)
    });
    let r = db.flush();
    flushed.store(true, std::sync::atomic::Ordering::Release);
    let (answered, injected) = lifter.join().unwrap();
    assert!(
        !watchdog.join().unwrap(),
        "the hold was lifted by the watchdog, not by the test's lift"
    );
    assert!(!answered, "the flush answered with the presync's hold on");
    assert_eq!(
        injected, None,
        "the armed failure was taken while the presync was held"
    );
    let e = r.expect_err("the failure armed before the hold fires after the lift");
    assert!(
        e.to_string()
            .contains("the records' sync failed (injected)"),
        "the flush answers the injected presync: {e}"
    );
    assert!(
        db.failed_presync_extent().is_some(),
        "the failure was injected"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// On the arm that finishes a run's close inline, the presync runs on a
/// thread of its own, which the hold does not hold: with the hold on and
/// a failure armed, the flush answers the failure with nobody lifting it.
#[test]
fn the_inline_arms_presync_is_not_held() {
    let d = dir("dur-do-seams-inline");
    let mut db = Db::create(&d, ps_opts(true)).unwrap();
    ps_run(&mut db, 0);
    db.hold_presync(true);
    db.fail_next_presync();
    let flushed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = ps_watchdog(db.reader().unwrap(), flushed.clone(), 20);
    let r = db.flush();
    flushed.store(true, std::sync::atomic::Ordering::Release);
    assert!(
        !watchdog.join().unwrap(),
        "the inline arm's presync waited for the hold"
    );
    let e = r.expect_err("the flush answers its armed presync failure");
    assert!(
        e.to_string()
            .contains("the records' sync failed (injected)"),
        "the flush answers the injected presync: {e}"
    );
    db.hold_presync(false);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// An injected presync failure is taken once. A store whose queue the
/// failure stopped still closes a run while its frozen list has room, and
/// presyncs it: with nothing armed again that presync syncs and the first
/// failure's record stands; armed again, the next one fails on the new
/// run's file, which says the store still presyncs there.
fn ps_failure_taken_once(tag: &str, inline: bool) {
    let d = dir(&format!("dur-do-seams-once-{tag}"));
    let mut db = Db::create(&d, ps_opts(inline)).unwrap();
    let more = |db: &mut Db, from: u32| {
        for k in from..from + 3000 {
            db.append(&ss_key(k), &ss_val(k));
            if k % 300 == 299 {
                let _ = db.commit();
            }
        }
    };
    ps_run(&mut db, 0);
    db.fail_next_presync();
    db.flush()
        .expect_err("the flush answers its armed presync failure");
    let first = db
        .failed_presync_extent()
        .expect("the failure was injected");
    more(&mut db, 3000);
    db.flush()
        .expect_err("a store whose seal queue stopped answers the stop");
    assert_eq!(
        db.failed_presync_extent(),
        Some(first),
        "a failure armed once failed a second presync"
    );
    more(&mut db, 6000);
    db.fail_next_presync();
    db.flush()
        .expect_err("a store whose seal queue stopped answers the stop");
    let third = db
        .failed_presync_extent()
        .expect("the failure was injected");
    assert_ne!(
        third.0, first.0,
        "the store closed no later run, so nothing was presynced after the first failure"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_injected_presync_failure_is_taken_once() {
    ps_failure_taken_once("writer", false);
}

#[test]
fn an_injected_presync_failure_is_taken_once_inline() {
    ps_failure_taken_once("inline", true);
}
