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

use supdb::{Db, Encryption, Options, Reader, StoreCipher, SyncPolicy};

mod common;
use common::{seg_names, vf, Gcm};

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
