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
    Db, Encryption, Options, Reader, SegmentOptions, SegmentWriter, StoreCipher, SyncPolicy,
};

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
