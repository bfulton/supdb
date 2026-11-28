//! The live table shared on demand (`Options::share_on_write`).
//!
//! A store opens with the one writer's table and everything that goes
//! with it. A handle's write asks for a shared table, the writer's next
//! commit installs one, and the store is read as a shared one until the
//! writer has left the table and the shared tables have landed. These
//! tests hold the store to every write across each of those changes, and
//! to the form on either side of them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use supdb::{Db, Options, SyncPolicy};

mod common;
use common::{kv, vf, Watchdog};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-share-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The held log with no frames, which is where a store shares, and
/// everything else as the engine ships it: the block cache, the upkeep
/// thread and ordered ingest, each of which a shared table stands down.
fn options() -> Options {
    Options {
        sync: SyncPolicy::Explicit,
        held_frames: false,
        share_on_write: true,
        partition_bytes: Some(4 << 10),
        ..common::options()
    }
}

type Model = BTreeMap<Vec<u8>, Vec<u8>>;

fn key(i: usize) -> Vec<u8> {
    format!("key-{i:06}").into_bytes()
}

/// Every value of `k`, oldest first, through the writer.
fn values(db: &Db, k: &[u8]) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    db.read_all(k, vf(&mut |v| got.push(v.to_vec()))).unwrap();
    got
}

/// Every value of `k`, oldest first, through the handle `r`.
fn handle_values(r: &supdb::Reader, k: &[u8]) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    r.read_all(k, vf(&mut |v| got.push(v.to_vec()))).unwrap();
    got
}

/// The store through the writer and through a fresh handle, by point read
/// and by scan, held to `model`: every key's one value, and a scan that
/// lists every key once with it.
fn hold(db: &Db, model: &Model, when: &str) {
    let r = db.reader().unwrap();
    for (k, v) in model {
        assert_eq!(
            values(db, k),
            vec![v.clone()],
            "{when}: the writer's read of {k:?}"
        );
        assert_eq!(
            handle_values(&r, k),
            vec![v.clone()],
            "{when}: a handle's read of {k:?}"
        );
    }
    let want: Vec<(Vec<u8>, Vec<u8>)> = model.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut got = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))),
    )
    .unwrap();
    assert_eq!(got.len(), want.len(), "{when}: the writer's scan");
    assert_eq!(got, want, "{when}: the writer's scan");
    let mut got = Vec::new();
    r.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))),
    )
    .unwrap();
    assert_eq!(got.len(), want.len(), "{when}: a handle's scan");
    assert_eq!(got, want, "{when}: a handle's scan");
    // And from the middle, a window as a mix's scan takes one.
    if let Some(from) = model.keys().nth(model.len() / 2) {
        let want: Vec<_> = model
            .range(from.clone()..)
            .take(50)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut got = Vec::new();
        r.scan(from, 50, kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))))
            .unwrap();
        assert_eq!(got, want, "{when}: a handle's window");
    }
}

/// A handle's write made, the writer's commit asked wherever it was
/// refused, as a caller's remedy for a full table is.
fn write(db: &mut Db, mut once: impl FnMut() -> std::io::Result<bool>) {
    for _ in 0..8 {
        if once().unwrap() {
            return;
        }
        db.commit().unwrap();
    }
    panic!("a handle's write refused past every commit");
}

/// A store loaded in key order and written out of it, scanned so that its
/// block cache holds forms, and with writes unsealed in its live table.
fn loaded(name: &str, o: Options) -> (PathBuf, Db, Model) {
    let d = scratch(name);
    let mut db = Db::create(&d, o).unwrap();
    let mut model = Model::new();
    for i in 0..3000 {
        let (k, v) = (key(i * 2), format!("loaded-{i}").into_bytes());
        db.append(&k, &v);
        model.insert(k, v);
        if i % 100 == 99 {
            db.commit().unwrap();
        }
    }
    db.flush().unwrap();
    db.settle().unwrap();
    for i in (0..3000).step_by(7) {
        let (k, v) = (key(i * 2), format!("put-{i}").into_bytes());
        db.put(&k, &v);
        model.insert(k, v);
    }
    db.commit().unwrap();
    hold(&db, &model, "loaded");
    (d, db, model)
}

/// The whole of it on one thread. A handle's write to the one writer's
/// table is refused and asks; the commit after it installs a shared table,
/// and nothing before it does. The handle's writes and the writer's own
/// land in it. With the handle gone and the writer's work come to the
/// table's entries, a commit leaves the table, and the store reads through
/// its block cache again once the table has landed. Every read and scan,
/// through the writer and a handle, answers the model at each step, and
/// after a flush and a reopen.
#[test]
fn a_handles_write_shares_the_table_and_the_writer_takes_it_back() {
    let _w = Watchdog::arm("the sharing lifecycle waited a minute");
    let (d, mut db, mut model) = loaded("lifecycle", options());
    assert!(!db.sharing(), "a store opens with the one writer's table");
    assert_eq!(db.share_counts(), [0, 0, 0]);
    // A commit with nothing asked shares nothing.
    db.commit().unwrap();
    assert!(!db.sharing());

    let r = db.reader().unwrap();
    let (k, v) = (key(1), b"handle-first".to_vec());
    assert!(
        !r.put(&k, &v).unwrap(),
        "the one writer's table takes no handle's write"
    );
    assert!(
        !db.sharing(),
        "asking installs nothing: the writer's commit does"
    );
    assert!(values(&db, &k).is_empty(), "a refused write wrote nothing");
    db.commit().unwrap();
    assert!(
        db.sharing(),
        "the commit after a handle asked installs a shared table"
    );
    assert_eq!(db.share_counts(), [1, 0, 0]);
    assert!(r.put(&k, &v).unwrap(), "the shared table takes it");
    model.insert(k, v);
    hold(&db, &model, "shared, one handle's write");

    // The handle's writes and the writer's own, old keys and new.
    for i in 0..400 {
        let (k, v) = (key(i * 5 + 1), format!("handle-{i}").into_bytes());
        write(&mut db, || r.put(&k, &v));
        model.insert(k, v);
        let (k, v) = (key(i * 6), format!("writer-{i}").into_bytes());
        db.put(&k, &v);
        model.insert(k, v);
        if i % 50 == 49 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    assert!(db.sharing(), "a handle that writes holds the table shared");
    assert_eq!(
        db.share_counts()[1],
        0,
        "and the writer leaves none while it lives"
    );
    hold(&db, &model, "shared, both writing");

    // The handle gone: the writer's work pays the table's rent first.
    drop(r);
    db.commit().unwrap();
    assert!(
        db.sharing(),
        "a commit with no work of the writer's since leaves nothing"
    );
    assert_eq!(db.share_counts()[1], 0);
    for i in 0..4000 {
        let (k, v) = (key(i % 6000), format!("rent-{i}").into_bytes());
        db.put(&k, &v);
        model.insert(k, v);
        if i % 100 == 99 {
            db.commit().unwrap();
        }
        if db.share_counts()[1] > 0 {
            break;
        }
    }
    assert_eq!(
        db.share_counts()[1],
        1,
        "the writer left the table once its rent was paid"
    );
    assert_eq!(
        db.upkeep_abandon_state().1,
        0,
        "the shared table was kept for a look as it was left"
    );
    db.settle().unwrap();
    assert!(
        !db.sharing(),
        "and the store is the one writer's once the table has landed"
    );
    hold(&db, &model, "after the way back");
    // The block cache serves again: a handle's scans take the block path.
    let r = db.reader().unwrap();
    let (_, block0) = db.reader_scans();
    for i in 0..20 {
        r.scan(&key(i * 100), 50, kv(&mut |_, _| {})).unwrap();
    }
    assert!(
        db.reader_scans().1 > block0,
        "a handle's scans took the merge path with no shared table standing"
    );
    drop(r);
    db.flush().unwrap();
    db.settle().unwrap();
    hold(&db, &model, "after the flush");
    db.close().unwrap();
    let db = Db::open(&d, options()).unwrap();
    hold(&db, &model, "after a reopen");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A handle's write of a key, asking the writer behind `db` wherever it
/// is refused, from a thread that shares the writer with others.
fn write_locked(db: &Mutex<Db>, mut once: impl FnMut() -> std::io::Result<bool>) {
    for _ in 0..1000 {
        if once().unwrap() {
            return;
        }
        db.lock().unwrap().commit().unwrap();
    }
    panic!("a handle's write refused past every commit");
}

/// The writer's work until the live table is its own again: puts over
/// keys of its own, committed, as the rent asks of it -- or until one of
/// them fills the table, whose seal leaves it too. Every put committed
/// before it returns, since a handle reads the one writer's table to its
/// last commit.
fn pay_rent(db: &mut Db, model: &mut Model, tag: &str) {
    for i in 0..200_000 {
        if !db.live_shared() {
            db.commit().unwrap();
            return;
        }
        let (k, v) = (
            format!("writer-{:05}", i % 2000).into_bytes(),
            format!("{tag}-{i}").into_bytes(),
        );
        db.put(&k, &v);
        model.insert(k, v);
        if i % 100 == 99 {
            db.commit().unwrap();
        }
    }
    panic!(
        "the writer never left the shared table: {:?}",
        db.share_counts()
    );
}

/// While a shared table stands the store keeps nothing a handle's write
/// could not reach: the writer's block tables go at the flip, no block is
/// built by a scan or a commit, no builder is started, nothing is lent to
/// the upkeep thread, no table is kept for its look, and claiming a handle
/// sorts no snapshot for it. A scan made before a handle overwrites a key
/// and one made after both read what the key holds, which a form built by
/// the first would not.
#[test]
fn nothing_is_built_lent_or_kept_while_a_shared_table_stands() {
    stood_down("stood-down", options());
}

/// The same with a builder started ahead at every publish
/// (`Options::build_ahead_on_publish`), which reads the state's tables on
/// a thread of its own: none is started over a shared table.
#[test]
fn no_builder_is_started_ahead_over_a_shared_table() {
    stood_down(
        "stood-down-ahead",
        Options {
            build_ahead_on_publish: true,
            scan_cache_ahead_min_blocks: 0,
            forms_settle_backlog_pct: 0,
            commit_forms: true,
            l0_trigger: 64,
            ..options()
        },
    );
}

fn stood_down(name: &str, o: Options) {
    let _w = Watchdog::arm("the stood-down test waited a minute");
    // Under the stride switch the compact form is forced, a store read out
    // of core, where no builder is started ahead at all: the variant is
    // then the test above.
    let ahead = o.build_ahead_on_publish && !common::stride_switch();
    let (d, mut db, mut model) = loaded(name, o);
    if ahead {
        // A publish beside a handle the forms are for starts one, in the
        // store as it opens.
        let h = db.reader().unwrap();
        db.seal().unwrap();
        db.settle().unwrap();
        drop(h);
        assert!(
            db.forms_events()[2] > 0,
            "a seal of the loaded store started no builder: the test has none to see stood down"
        );
        db.scan(b"", usize::MAX, kv(&mut |_, _| {})).unwrap();
        for i in (0..3000).step_by(11) {
            let (k, v) = (key(i * 2), format!("again-{i}").into_bytes());
            db.put(&k, &v);
            model.insert(k, v);
        }
        db.commit().unwrap();
        db.scan(b"", usize::MAX, kv(&mut |_, _| {})).unwrap();
    }
    assert!(
        db.block_cache_bytes() > 0,
        "the loaded store's scans built no block: the test has nothing to see go"
    );
    let r = db.reader().unwrap();
    let (k, v) = (key(3), b"asks".to_vec());
    write(&mut db, || r.put(&k, &v));
    model.insert(k, v);
    assert!(db.sharing());
    assert_eq!(
        db.block_cache_bytes(),
        0,
        "the writer's tables stood through the flip"
    );
    assert_eq!(
        db.upkeep_abandon_state().1,
        0,
        "the table the flip froze was kept for a look that no table waits on"
    );
    let built = db.blocks_built();
    let events = db.forms_events();
    let upkeep = db.upkeep_counts();
    let land = db.zpage_counts().land_bound_walks;
    for round in 0..20 {
        for i in 0..40 {
            // Keys the store holds, overwritten where a scan has read them.
            let (k, v) = (
                key((round * 40 + i) * 2),
                format!("over-{round}-{i}").into_bytes(),
            );
            write(&mut db, || r.put(&k, &v));
            model.insert(k, v);
            let (k, v) = (key(i * 2 + 1), format!("own-{round}-{i}").into_bytes());
            db.put(&k, &v);
            model.insert(k, v);
        }
        db.commit().unwrap();
        let mut got = Vec::new();
        db.scan(
            &key(round * 80),
            80,
            kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))),
        )
        .unwrap();
        let want: Vec<_> = model
            .range(key(round * 80)..)
            .take(80)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(got, want, "round {round}: the writer's window");
        let mut got = Vec::new();
        r.scan(
            &key(round * 80),
            80,
            kv(&mut |k, v| got.push((k.to_vec(), v.to_vec()))),
        )
        .unwrap();
        assert_eq!(got, want, "round {round}: the handle's window");
    }
    // A seal beside the handle, near the scans above: its landing walks
    // no piece's bounds for tables that do not stand.
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.live_shared());
    assert_eq!(
        db.zpage_counts().land_bound_walks,
        land,
        "a landing walked a piece's bounds beside a shared table"
    );
    // A handle claimed over a shared table has nothing sorted for it: new
    // keys since the last scan, and no snapshot built or extended.
    for i in 0..50 {
        let (k, v) = (format!("new-{i:04}").into_bytes(), b"new".to_vec());
        write(&mut db, || r.put(&k, &v));
        model.insert(k, v);
    }
    db.commit().unwrap();
    let snaps = (db.snapshot_builds(), db.snapshot_extends());
    for _ in 0..5 {
        drop(db.reader().unwrap());
    }
    assert_eq!(
        (db.snapshot_builds(), db.snapshot_extends()),
        snaps,
        "a claim sorted the shared table"
    );
    assert!(db.sharing());
    assert_eq!(
        db.blocks_built(),
        built,
        "a block was built over a shared table"
    );
    let now = db.forms_events();
    assert_eq!(
        now[2], events[2],
        "a builder was started over a shared table"
    );
    assert_eq!(
        now[4], events[4],
        "a builder's forms were installed over one"
    );
    assert_eq!(
        (now[6], now[7]),
        (events[6], events[7]),
        "a commit filled forms over one"
    );
    let now = db.upkeep_counts();
    assert_eq!(
        (now[0], now[1]),
        (upkeep[0], upkeep[1]),
        "the upkeep was lent beside a shared table: passes and takes"
    );
    assert_eq!(db.block_cache_bytes(), 0);
    hold(&db, &model, "shared");
    drop(r);
    db.commit().unwrap();
    // The writer leaves the table, and until the table has landed the
    // store is read as it was beside it: a frozen shared table's writes
    // are in no log a form is kept current by.
    let built = db.blocks_built();
    db.hold_seal_landing(true);
    pay_rent(&mut db, &mut model, "rent");
    assert!(
        !db.live_shared() && db.sharing(),
        "the table left has landed under its hold"
    );
    for i in 0..20 {
        db.scan(&key(i * 200), 50, kv(&mut |_, _| {})).unwrap();
    }
    assert_eq!(
        db.blocks_built(),
        built,
        "a block was built over a shared table still on its way to a segment"
    );
    db.hold_seal_landing(false);
    db.settle().unwrap();
    assert!(!db.sharing());
    hold(&db, &model, "back");
    assert!(
        db.blocks_built() != built,
        "no block built once the table had landed"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A shared table nothing was written into is left without a seal: frozen
/// so that its writers are waited out, found empty and taken off the
/// frozen list, the store the one writer's at once.
#[test]
fn an_empty_shared_table_is_left_without_a_seal() {
    let _w = Watchdog::arm("the empty table's test waited a minute");
    let (d, mut db, model) = loaded("empty", options());
    db.flush().unwrap();
    db.settle().unwrap();
    let r = db.reader().unwrap();
    assert!(!r.put(&key(1), b"never").unwrap());
    db.commit().unwrap();
    assert!(db.sharing());
    assert_eq!(db.share_counts(), [1, 0, 0]);
    drop(r);
    let joins = db.seal_waits().joins;
    // The first look to find no handle starts the rent; the second finds
    // it paid, an empty table's being nothing.
    db.commit().unwrap();
    assert!(db.sharing(), "left at the first look to find no handle");
    db.commit().unwrap();
    assert_eq!(db.share_counts(), [1, 1, 0]);
    assert!(
        !db.sharing(),
        "an empty table left the frozen list at its freeze"
    );
    assert_eq!(
        db.seal_waits().joins,
        joins,
        "a seal was handed for an empty table"
    );
    hold(&db, &model, "left empty");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A handle whose write takes the shared table as the writer is leaving
/// it -- after the writer's look found no handle, before its freeze has
/// closed the table -- is waited for and its write kept: the table is
/// judged empty only once its writers are out, and one with a write in it
/// is sealed. A write that comes once the table is closed is refused, and
/// made again it asks for a table of its own.
#[test]
fn a_write_that_lands_as_the_writer_leaves_the_table_is_kept() {
    let _w = Watchdog::arm("the late write's test waited a minute");
    let (d, mut db, mut model) = loaded("late", options());
    db.flush().unwrap();
    db.settle().unwrap();
    let r = db.reader().unwrap();
    assert!(!r.put(&key(1), b"never").unwrap());
    db.commit().unwrap();
    assert!(db.sharing());
    drop(r);
    db.commit().unwrap();
    // The handles that will write late: made now, and on no list until
    // they write, so the writer's look finds no handle and leaves the table.
    let late = db.reader().unwrap();
    let later = db.reader().unwrap();
    let gate = db.reader().unwrap();
    db.hold_unshare(true);
    db.hold_shared_closed(true);
    // A handle's write waits between taking the table and writing it.
    db.hold_shared_writes(200_000);
    let (k, v) = (key(7), b"late".to_vec());
    let held = gate.shared_writes_held();
    let (late, later) = std::thread::scope(|s| {
        let leaving = s.spawn(|| db.commit().unwrap());
        while !gate.unshare_holding() {
            assert!(!leaving.is_finished(), "the commit left no table");
            std::thread::yield_now();
        }
        // The table is empty and open: the late write takes it and waits.
        let (lk, lv) = (k.clone(), v.clone());
        let writing = s.spawn(move || {
            assert!(late.put(&lk, &lv).unwrap(), "an open table refused a write");
            late
        });
        while gate.shared_writes_held() == held {
            assert!(!writing.is_finished(), "the late write did not wait");
            std::thread::yield_now();
        }
        // The writer closes the table and waits the write out: the write
        // returns, having landed, and the freeze holds with the table
        // closed and not yet published, where a write is refused.
        gate.release_unshare();
        let late = writing.join().unwrap();
        while !gate.shared_closed_holding() {
            std::thread::yield_now();
        }
        let refused = later.put(&key(9), b"later").unwrap();
        gate.release_shared_closed();
        leaving.join().unwrap();
        assert!(!refused, "a closed table took a write");
        (late, later)
    });
    db.hold_shared_writes(0);
    model.insert(k.clone(), v.clone());
    assert_eq!(db.share_counts()[1], 1);
    assert_eq!(
        handle_values(&late, &k),
        vec![v.clone()],
        "the late write went with a table taken for an empty one"
    );
    // The refused write made again: it meets the one writer's table and
    // asks, and the commit installs a table for it.
    let (k, v) = (key(9), b"later".to_vec());
    write(&mut db, || later.put(&k, &v));
    model.insert(k, v);
    assert!(db.live_shared());
    hold(&db, &model, "after the late writes");
    drop((late, later, gate));
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(!db.sharing());
    hold(&db, &model, "landed");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A seal makes the next table, so with no handle that writes it makes
/// the one writer's: a flush of a shared store leaves it the writer's
/// with no rent paid.
#[test]
fn a_seal_with_no_handle_writing_makes_the_next_table_the_writers() {
    let _w = Watchdog::arm("the seal's choice waited a minute");
    let (d, mut db, mut model) = loaded("seal", options());
    let r = db.reader().unwrap();
    for i in 0..300 {
        let (k, v) = (key(i * 3 + 1), format!("shared-{i}").into_bytes());
        write(&mut db, || r.put(&k, &v));
        model.insert(k, v);
    }
    assert!(db.sharing());
    // A seal beside a handle that writes keeps the table's kind.
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(
        db.sharing(),
        "a seal beside a writing handle left the shared form"
    );
    assert_eq!(db.share_counts(), [1, 0, 0]);
    let (k, v) = (key(5), b"after-the-seal".to_vec());
    assert!(
        r.put(&k, &v).unwrap(),
        "the table after the seal took no write"
    );
    model.insert(k, v);
    drop(r);
    db.flush().unwrap();
    db.settle().unwrap();
    assert_eq!(db.share_counts(), [1, 0, 1]);
    assert!(!db.sharing());
    hold(&db, &model, "flushed");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A handle asks while a run of ordered ingest is open with a batch
/// staged: the run is closed with every write of it, the shared table
/// installed, and the writer's keys above the store's greatest go into
/// the shared table beside the handle's, where a run would have taken the
/// live table's place.
#[test]
fn a_run_of_ordered_ingest_closes_for_a_handle_that_asks() {
    let _w = Watchdog::arm("the open run's test waited a minute");
    let d = scratch("run");
    let mut db = Db::create(&d, options()).unwrap();
    let mut model = Model::new();
    for i in 0..500 {
        let (k, v) = (key(i), format!("run-{i}").into_bytes());
        db.append(&k, &v);
        model.insert(k, v);
        if i % 100 == 99 && i < 400 {
            db.commit().unwrap();
        }
    }
    let r = db.reader().unwrap();
    assert!(!r.put(b"handle", b"first").unwrap());
    db.commit().unwrap();
    assert!(db.sharing());
    assert!(r.put(b"handle", b"first").unwrap());
    model.insert(b"handle".to_vec(), b"first".to_vec());
    hold(&db, &model, "the run closed");
    for i in 500..800 {
        let (k, v) = (key(i), format!("above-{i}").into_bytes());
        db.append(&k, &v);
        model.insert(k, v);
        assert!(r.put(b"handle", format!("beside-{i}").as_bytes()).unwrap());
        model.insert(b"handle".to_vec(), format!("beside-{i}").into_bytes());
    }
    db.commit().unwrap();
    assert!(db.sharing());
    hold(&db, &model, "written above the store");
    drop(r);
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(!db.sharing());
    hold(&db, &model, "flushed");
    db.close().unwrap();
    let db = Db::open(&d, options()).unwrap();
    hold(&db, &model, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The option acts only where a handle's write may go unlogged: under a
/// policy that logs, or with frames held, a handle's write is refused as
/// it is without the option.
#[test]
fn a_store_that_logs_its_writes_shares_nothing() {
    for (name, o) in [
        (
            "durable",
            Options {
                share_on_write: true,
                ..common::options()
            },
        ),
        (
            "frames",
            Options {
                sync: SyncPolicy::Explicit,
                share_on_write: true,
                ..common::options()
            },
        ),
        (
            "off",
            Options {
                share_on_write: false,
                ..options()
            },
        ),
    ] {
        let d = scratch(name);
        let mut db = Db::create(&d, o).unwrap();
        let r = db.reader().unwrap();
        assert!(
            r.put(b"k", b"v").is_err(),
            "{name}: a handle's write was taken"
        );
        db.commit().unwrap();
        assert!(!db.sharing(), "{name}");
        assert_eq!(db.share_counts(), [0, 0, 0], "{name}");
        drop(r);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Round after round: handles on three threads write keys of their own and
/// keys in common while a fourth reads and scans and the writer, behind
/// its lock, writes and scans beside them, under a table small enough to
/// freeze many times a round; then the handles go, the writer pays the
/// table's rent and the store is the one writer's again, with its block
/// cache, until the next round's first write. Each thread reads its own
/// writes back as it makes them; every value a read or a scan meets is
/// one write's whole; and after each round, shared and back, every key
/// reads what its last write left, by point read and by scan, through the
/// writer and a handle.
#[test]
fn handles_share_the_table_and_the_writer_takes_it_back_round_after_round() {
    const THREADS: usize = 3;
    const OWN: usize = 600;
    const COMMON: usize = 200;
    const ROUNDS: usize = 3;
    let _w = Watchdog::arm("the rounds of sharing waited a minute");
    let o = Options {
        seal_bytes: 96 << 10,
        seal_max_pct: 0,
        ..options()
    };
    let (d, db, mut model) = loaded("rounds", o.clone());
    let db = Mutex::new(db);
    let own_key = |t: usize, i: usize| format!("own-{t}-{i:05}").into_bytes();
    let common_key = |i: usize| format!("common-{i:05}").into_bytes();
    let mut common_written = false;
    for round in 0..ROUNDS {
        let handles: Vec<supdb::Reader> = {
            let db = db.lock().unwrap();
            (0..THREADS + 1).map(|_| db.reader().unwrap()).collect()
        };
        let shares0 = db.lock().unwrap().share_counts();
        assert!(!db.lock().unwrap().sharing(), "round {round} began shared");
        let stop = AtomicBool::new(false);
        let read_one = AtomicBool::new(false);
        std::thread::scope(|s| {
            // The reader stopped however this thread leaves the scope: a
            // failure here would otherwise wait on it for ever.
            let _stop = StopOnDrop(&stop);
            let mut handles = handles.into_iter();
            let reader = handles.next().unwrap();
            let watcher = {
                let (stop, read_one) = (&stop, &read_one);
                s.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        for i in 0..COMMON {
                            reader
                                .read_all(
                                    &common_key(i),
                                    vf(&mut |v| {
                                        assert!(v.starts_with(b"common-"), "a value torn: {v:?}");
                                        read_one.store(true, Ordering::Release);
                                    }),
                                )
                                .unwrap();
                        }
                        let mut last: Option<Vec<u8>> = None;
                        reader
                            .scan(
                                b"common-",
                                COMMON,
                                kv(&mut |k, v| {
                                    assert!(
                                        last.as_deref().is_none_or(|l| l < k),
                                        "a scan out of order or twice at {k:?}"
                                    );
                                    last = Some(k.to_vec());
                                    if k.starts_with(b"common-") {
                                        assert!(v.starts_with(b"common-"), "a scanned value torn");
                                    }
                                }),
                            )
                            .unwrap();
                    }
                })
            };
            let writers: Vec<_> = handles
                .enumerate()
                .map(|(t, r)| {
                    let db = &db;
                    s.spawn(move || {
                        for i in 0..OWN {
                            let k = own_key(t, i);
                            let first = format!("{round}-{t}-{i}-first").into_bytes();
                            let second = format!("{round}-{t}-{i}-second").into_bytes();
                            write_locked(db, || r.put(&k, &first));
                            assert_eq!(handle_values(&r, &k), vec![first.clone()], "its put");
                            write_locked(db, || r.put(&k, &second));
                            assert_eq!(handle_values(&r, &k), vec![second.clone()], "its second");
                            let c = common_key((i * 7 + t) % COMMON);
                            write_locked(db, || {
                                r.put(&c, format!("common-{round}-{t}-{i}").as_bytes())
                            });
                        }
                    })
                })
                .collect();
            // The writer beside them: its own keys, committed, and scans.
            let mut own = Vec::new();
            for i in 0..300 {
                let (k, v) = (
                    format!("writer-{:05}", i % 2000).into_bytes(),
                    format!("beside-{round}-{i}").into_bytes(),
                );
                let mut db = db.lock().unwrap();
                db.put(&k, &v);
                own.push((k, v));
                if i % 20 == 19 {
                    db.commit().unwrap();
                    db.scan(&key(i * 10), 30, kv(&mut |_, _| {})).unwrap();
                }
            }
            let done: Vec<_> = writers.into_iter().map(|h| h.join()).collect();
            while !read_one.load(Ordering::Acquire) && !watcher.is_finished() {
                std::thread::yield_now();
            }
            stop.store(true, Ordering::Relaxed);
            let seen = watcher.join();
            for d in done {
                if let Err(p) = d {
                    std::panic::resume_unwind(p);
                }
            }
            if let Err(p) = seen {
                std::panic::resume_unwind(p);
            }
            for (k, v) in own {
                model.insert(k, v);
            }
        });
        common_written = true;
        for t in 0..THREADS {
            for i in 0..OWN {
                model.insert(
                    own_key(t, i),
                    format!("{round}-{t}-{i}-second").into_bytes(),
                );
            }
        }
        let mut db = db.lock().unwrap();
        db.commit().unwrap();
        assert!(
            db.share_counts()[0] > shares0[0],
            "round {round}: no table installed for the handles"
        );
        // Shared still, or left already: the writer's commits beside the
        // last of the handles' writes may have found none of them alive.
        hold_with_common(
            &db,
            &model,
            COMMON,
            common_written,
            &format!("round {round}, written"),
        );
        pay_rent(&mut db, &mut model, &format!("rent-{round}"));
        db.settle().unwrap();
        assert!(!db.sharing(), "round {round}: the store stayed shared");
        hold_with_common(
            &db,
            &model,
            COMMON,
            common_written,
            &format!("round {round}, back"),
        );
    }
    let mut db = db.into_inner().unwrap();
    assert!(
        db.seal_waits().joins >= 8,
        "the rounds froze {} tables, too few to cross a freeze beside the handles",
        db.seal_waits().joins
    );
    db.flush().unwrap();
    db.settle().unwrap();
    hold_with_common(&db, &model, COMMON, common_written, "flushed");
    db.close().unwrap();
    let db = Db::open(&d, o).unwrap();
    hold_with_common(&db, &model, COMMON, common_written, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// `hold` for a store with keys several threads put in common, each of
/// which holds one whole value some thread put: the model's keys by point
/// read and by scan through the writer and a handle, the common keys one
/// value each, and nothing else in a scan.
fn hold_with_common(db: &Db, model: &Model, common: usize, written: bool, when: &str) {
    let r = db.reader().unwrap();
    for (k, v) in model {
        assert_eq!(
            values(db, k),
            vec![v.clone()],
            "{when}: the writer's read of {k:?}"
        );
        assert_eq!(
            handle_values(&r, k),
            vec![v.clone()],
            "{when}: a handle's read of {k:?}"
        );
    }
    let commons = if written { common } else { 0 };
    for i in 0..commons {
        let k = format!("common-{i:05}").into_bytes();
        for vs in [values(db, &k), handle_values(&r, &k)] {
            assert_eq!(vs.len(), 1, "{when}: common key {i}: {vs:?}");
            assert!(vs[0].starts_with(b"common-"), "{when}: common key {i}");
        }
    }
    for by in ["writer", "handle"] {
        let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut f = |k: &[u8], v: &[u8]| got.push((k.to_vec(), v.to_vec()));
        if by == "writer" {
            db.scan(b"", usize::MAX, kv(&mut f)).unwrap();
        } else {
            r.scan(b"", usize::MAX, kv(&mut f)).unwrap();
        }
        let (c, rest): (Vec<_>, Vec<_>) = got
            .into_iter()
            .partition(|(k, _)| k.starts_with(b"common-"));
        assert_eq!(
            c.len(),
            commons,
            "{when}: the {by}'s scan of the common keys"
        );
        assert!(
            c.iter().all(|(_, v)| v.starts_with(b"common-")),
            "{when}: the {by}'s scan read a common value torn"
        );
        let want: Vec<_> = model.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(rest.len(), want.len(), "{when}: the {by}'s scan");
        assert_eq!(rest, want, "{when}: the {by}'s scan");
    }
}

/// A handle's read-modify-write of a key asks as its put does: refused by
/// the one writer's table with nothing read or written, and after the
/// commit it reads what the key held, from the segments and then from the
/// shared table, and puts its value.
#[test]
fn a_handles_read_modify_write_asks_as_its_put_does() {
    let _w = Watchdog::arm("the read-modify-write's test waited a minute");
    let (d, mut db, mut model) = loaded("rmw", options());
    let r = db.reader().unwrap();
    let k = key(40);
    let was = model[&k].clone();
    let mut read = Vec::new();
    assert!(
        !r.rmw(&k, b"first", vf(&mut |v| read.push(v.to_vec())))
            .unwrap(),
        "the one writer's table took a handle's read-modify-write"
    );
    assert!(read.is_empty(), "a refused read-modify-write read the key");
    assert_eq!(values(&db, &k), vec![was.clone()], "and it wrote nothing");
    db.commit().unwrap();
    assert!(db.live_shared());
    assert!(r
        .rmw(&k, b"first", vf(&mut |v| read.push(v.to_vec())))
        .unwrap());
    assert_eq!(read, vec![was], "the first read what the store held");
    read.clear();
    assert!(r
        .rmw(&k, b"again", vf(&mut |v| read.push(v.to_vec())))
        .unwrap());
    assert_eq!(
        read,
        vec![b"first".to_vec()],
        "the second read the first's value"
    );
    model.insert(k, b"again".to_vec());
    hold(&db, &model, "after the read-modify-writes");
    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A handle that asked and was dropped before the writer's commit shares
/// nothing: no table installed, nothing sealed, and the writer's block
/// tables left as they were.
#[test]
fn a_handle_that_asked_and_went_shares_nothing() {
    let _w = Watchdog::arm("the gone handle's test waited a minute");
    let (d, mut db, model) = loaded("gone", options());
    let (joins, cache) = (db.seal_waits().joins, db.block_cache_bytes());
    assert!(cache > 0);
    let r = db.reader().unwrap();
    assert!(!r.put(&key(1), b"never").unwrap());
    drop(r);
    db.commit().unwrap();
    assert!(!db.sharing() && !db.live_shared());
    assert_eq!(db.share_counts(), [0, 0, 0]);
    assert_eq!(
        db.seal_waits().joins,
        joins,
        "a table was sealed for a handle that had gone"
    );
    assert_eq!(
        db.block_cache_bytes(),
        cache,
        "the writer's tables went for it"
    );
    hold(&db, &model, "nothing shared");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The rent of the shared form is the writer's own work with no handle
/// writing: its writes into the table and the entries its scans ask for,
/// counted from the first commit to find no handle, and started over by a
/// handle that writes. Scans alone pay it.
#[test]
fn the_rent_is_the_writers_work_beside_no_handle() {
    let _w = Watchdog::arm("the rent's test waited a minute");
    let (d, mut db, mut model) = loaded("rent", options());
    let r = db.reader().unwrap();
    for i in 0..500 {
        let (k, v) = (key(i * 4 + 1), format!("shared-{i}").into_bytes());
        write(&mut db, || r.put(&k, &v));
        model.insert(k, v);
    }
    // Work beside a handle that writes pays nothing, however much.
    for round in 0..5 {
        for i in 0..400 {
            let (k, v) = (key(i * 4 + 3), format!("beside-{round}-{i}").into_bytes());
            db.put(&k, &v);
            model.insert(k, v);
        }
        db.scan(b"", usize::MAX, kv(&mut |_, _| {})).unwrap();
        db.commit().unwrap();
    }
    assert!(
        db.live_shared(),
        "the writer left a table a handle was writing"
    );
    drop(r);
    // The first commit to find no handle starts the rent at nothing, the
    // work before it done beside one.
    db.commit().unwrap();
    assert!(db.live_shared(), "work beside a handle paid the rent");
    // Short of the table's entries, about nine hundred: nothing.
    for i in 0..100 {
        let (k, v) = (key(i * 4 + 3), format!("short-{i}").into_bytes());
        db.put(&k, &v);
        model.insert(k, v);
    }
    db.commit().unwrap();
    assert!(
        db.live_shared(),
        "a hundred writes paid for nine hundred entries"
    );
    // A handle that writes starts it over: the hundred are forgotten, and
    // so is the writer's work beside the handle, however much it is and
    // though the look before it found none.
    let r = db.reader().unwrap();
    let (k, v) = (key(5), b"again".to_vec());
    assert!(r.put(&k, &v).unwrap());
    model.insert(k, v);
    db.commit().unwrap();
    for i in 0..3000 {
        let (k, v) = (
            key(i % 1000 * 4 + 3),
            format!("beside-again-{i}").into_bytes(),
        );
        db.put(&k, &v);
        model.insert(k, v);
    }
    drop(r);
    db.commit().unwrap();
    assert!(
        db.live_shared(),
        "work beside a handle that came back paid the rent"
    );
    for _ in 0..8 {
        for i in 0..100 {
            let (k, v) = (key(i * 4 + 3), format!("over-{i}").into_bytes());
            db.put(&k, &v);
            model.insert(k, v);
        }
        db.commit().unwrap();
    }
    assert!(
        db.live_shared(),
        "eight hundred writes paid for more entries than that"
    );
    // And scans pay it: windows of fifty until their entries come to it.
    let mut scans = 0;
    while db.live_shared() {
        assert!(scans < 200, "the writer's scans never paid the rent");
        db.scan(&key(scans * 30), 50, kv(&mut |_, _| {})).unwrap();
        db.commit().unwrap();
        scans += 1;
    }
    assert!(
        scans >= 2,
        "the first window paid for a table of hundreds of entries"
    );
    assert_eq!(db.share_counts()[1], 1);
    db.settle().unwrap();
    hold(&db, &model, "left by scans");
    // A table shared again after a leave starts its rent as the first did:
    // the writer's work beside the handle that asked for it pays nothing.
    let r = db.reader().unwrap();
    let (k, v) = (key(9), b"second-table".to_vec());
    write(&mut db, || r.put(&k, &v));
    model.insert(k, v);
    assert!(db.live_shared());
    for i in 0..3000 {
        let (k, v) = (key(i % 1000 * 4 + 3), format!("second-{i}").into_bytes());
        db.put(&k, &v);
        model.insert(k, v);
    }
    drop(r);
    db.commit().unwrap();
    assert!(db.live_shared(), "a second table's rent began paid");
    hold(&db, &model, "shared again");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A shared table's piece reads newer than the pieces sealed before it:
/// the one writer's table a handle's first write froze holds a key's
/// values, the shared table after it the handle's put of the key, and the
/// store read after a close and a reopen answers the put. A shared table's
/// writes take no number of the store's sequence as they are made, and
/// the two tables sealed under one number, the older piece's values read
/// over the put.
#[test]
fn a_shared_tables_piece_reads_newer_than_the_piece_before_it() {
    let _w = Watchdog::arm("the pieces' order waited a minute");
    for flushed in [false, true] {
        let d = scratch(if flushed { "order-flushed" } else { "order" });
        let o = Options {
            // Pieces that stay pieces: nothing merges them before the read.
            l0_trigger: 64,
            ..options()
        };
        let mut db = Db::create(&d, o.clone()).unwrap();
        let mut model = Model::new();
        // Partitions first, so that what follows seals as pieces.
        for i in 0..2000 {
            let (k, v) = (key(i), format!("loaded-{i}").into_bytes());
            db.append(&k, &v);
            model.insert(k, v);
        }
        db.flush().unwrap();
        db.settle().unwrap();
        // The one writer's table: several values of each key, and a delete.
        for i in (0..2000).step_by(3) {
            db.put(&key(i), b"older-a");
            db.append(&key(i), b"older-b");
            model.insert(key(i), b"older-b".to_vec());
        }
        db.delete(&key(1));
        model.remove(&key(1));
        db.commit().unwrap();
        // A handle's puts over the same keys, in the shared table.
        let r = db.reader().unwrap();
        for i in (0..2000).step_by(6) {
            let v = format!("newer-{i}").into_bytes();
            write(&mut db, || r.put(&key(i), &v));
            model.insert(key(i), v);
        }
        write(&mut db, || r.put(&key(1), b"back"));
        model.insert(key(1), b"back".to_vec());
        drop(r);
        if flushed {
            db.flush().unwrap();
            db.settle().unwrap();
        }
        let appended: Vec<Vec<u8>> = (0..2000)
            .step_by(3)
            .filter(|i| i % 6 != 0)
            .map(key)
            .collect();
        let check = |db: &Db, when: &str| {
            for (k, v) in &model {
                let got = values(db, k);
                if appended.contains(k) {
                    assert_eq!(
                        got,
                        vec![b"older-a".to_vec(), b"older-b".to_vec()],
                        "{when}: {k:?}"
                    );
                } else {
                    assert_eq!(got, vec![v.clone()], "{when}: {k:?}");
                }
            }
        };
        check(&db, "before the close");
        db.close().unwrap();
        let db = Db::open(&d, o).unwrap();
        check(&db, "after the reopen");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A run of ordered ingest opens only above every key the store holds,
/// the keys handles wrote into a shared table among them: the writer's
/// keys above its own greatest and below a handle's go to its table as
/// any write does, and the store flushes, merges and reopens with every
/// key. Judged against the writer's own greatest alone, the run was a
/// piece open above over keys the shared table's pieces held beside it.
#[test]
fn a_run_of_ordered_ingest_opens_only_above_the_keys_handles_wrote() {
    let _w = Watchdog::arm("the run above a handle's keys waited a minute");
    for landed in [false, true] {
        let (d, mut db, mut model) =
            loaded(if landed { "above-landed" } else { "above" }, options());
        db.flush().unwrap();
        db.settle().unwrap();
        let r = db.reader().unwrap();
        // Far above the writer's greatest, key-005998.
        for i in 0..200 {
            let (k, v) = (
                format!("zz-{i:05}").into_bytes(),
                format!("handle-{i}").into_bytes(),
            );
            write(&mut db, || r.put(&k, &v));
            model.insert(k, v);
        }
        drop(r);
        // The shared table left at a seal: still on its way to a segment,
        // or landed, when the writer's ascending keys come.
        if landed {
            db.flush().unwrap();
            db.settle().unwrap();
        } else {
            db.hold_seal_landing(true);
            db.seal().unwrap();
        }
        assert!(!db.live_shared());
        // Above the writer's greatest, below the handle's, in key order.
        for i in 0..300 {
            let (k, v) = (
                format!("m-{i:05}").into_bytes(),
                format!("between-{i}").into_bytes(),
            );
            db.append(&k, &v);
            model.insert(k, v);
            assert!(
                db.runs_unsynced().0.is_none(),
                "a run of ordered ingest opened under keys a handle wrote"
            );
            if i % 50 == 49 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.hold_seal_landing(false);
        db.flush().unwrap();
        db.settle().unwrap();
        hold(&db, &model, "between the writer's keys and a handle's");
        // And above the handle's keys a run opens as it always did.
        for i in 0..300 {
            let (k, v) = (
                format!("zzz-{i:05}").into_bytes(),
                format!("above-{i}").into_bytes(),
            );
            db.append(&k, &v);
            model.insert(k, v);
        }
        db.commit().unwrap();
        assert!(
            db.runs_unsynced().0.is_some(),
            "no run opened above every key"
        );
        db.flush().unwrap();
        db.settle().unwrap();
        hold(&db, &model, "above them all");
        db.close().unwrap();
        let db = Db::open(&d, options()).unwrap();
        hold(&db, &model, "reopened");
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A crash keeps nothing of the history past its first loss: a shared
/// table whose seal never landed is lost with the process, and the run of
/// ordered ingest the writer began after it, whose commits reached its
/// file, is not served without it. The table takes a number of the store's
/// sequence at its freeze and counts as a commit no manifest covers, so
/// the run starts past what the store can vouch for and says so.
#[test]
fn a_run_begun_after_a_shared_table_is_not_served_without_it() {
    let _w = Watchdog::arm("the crash past a shared table waited a minute");
    let (d, mut db, mut model) = loaded("crash", options());
    db.flush().unwrap();
    db.settle().unwrap();
    let durable = model.clone();
    let r = db.reader().unwrap();
    for i in 0..200 {
        let (k, v) = (key(i * 2 + 1), format!("handle-{i}").into_bytes());
        write(&mut db, || r.put(&k, &v));
        model.insert(k, v);
    }
    drop(r);
    db.hold_seal_landing(true);
    db.seal().unwrap();
    assert!(!db.live_shared());
    for i in 0..300 {
        let (k, v) = (
            format!("zz-{i:05}").into_bytes(),
            format!("run-{i}").into_bytes(),
        );
        db.append(&k, &v);
        model.insert(k, v);
        if i % 50 == 49 {
            db.commit().unwrap();
        }
    }
    assert!(
        db.runs_unsynced().0.is_some(),
        "the writer's keys above the store opened no run: the test has no run to lose"
    );
    hold(&db, &model, "before the crash");
    db.crash();
    let db = Db::open(&d, options()).expect("the store opens past a shared table it lost");
    // The shared table's writes went with the process, and so did the run
    // begun after them: the store is what its last flush left.
    hold(&db, &durable, "after the crash");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The option acts only where nothing keeps a copy of a live table's
/// values that a handle's write would have to reach, and never beside the
/// option that shares every table: a store with the kept snapshot or the
/// hand-off of a live table refuses a handle's
/// write as it did, and a store opened to share every table goes on
/// sharing it with no handle writing.
#[test]
fn the_option_stands_down_beside_what_it_cannot_share_under() {
    for (name, o) in [
        (
            "kept",
            Options {
                forms_off_snapshot_kept: true,
                ..options()
            },
        ),
        (
            "shape",
            Options {
                adaptive_shape: true,
                ..options()
            },
        ),
        (
            "in-place",
            Options {
                put_in_place: true,
                scan_block_cache: false,
                upkeep: supdb::Upkeep::Inline,
                ..options()
            },
        ),
    ] {
        let d = scratch(name);
        let mut db = Db::create(&d, o).unwrap_or_else(|e| panic!("{name}: {e}"));
        let r = db.reader().unwrap();
        assert!(
            r.put(b"k", b"v").is_err(),
            "{name}: a handle's write was taken"
        );
        db.commit().unwrap();
        assert!(!db.sharing(), "{name}");
        drop(r);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
    // Every table shared by option: the choice is not the writer's to take
    // back, with no handle writing or with nothing in the table.
    let d = scratch("always");
    let mut db = Db::create(
        &d,
        Options {
            shared_writers: true,
            scan_block_cache: false,
            upkeep: supdb::Upkeep::Inline,
            direct_ingest: false,
            ..options()
        },
    )
    .unwrap();
    let r = db.reader().unwrap();
    assert!(r.put(b"k", b"v").unwrap());
    drop(r);
    for i in 0..50 {
        db.put(format!("w-{i}").as_bytes(), b"v");
        db.commit().unwrap();
    }
    db.seal().unwrap();
    db.settle().unwrap();
    db.commit().unwrap();
    db.commit().unwrap();
    assert!(
        db.live_shared(),
        "a store that shares every table left the shared form"
    );
    assert_eq!(db.share_counts(), [0, 0, 0]);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A handle claimed to write (`Db::writer`) writes at once: the claim
/// installs the shared table, where a handle from `reader` has its first
/// write refused until the writer's next commit, and a second claim
/// installs nothing more. With the handles gone and the rent paid the
/// writer leaves the table, and the next claim installs one again. Every
/// read answers the model at each step, and after a flush and a reopen.
#[test]
fn a_handle_claimed_to_write_writes_at_once() {
    let _w = Watchdog::arm("a claim to write waited a minute");
    let (d, mut db, mut model) = loaded("writer", options());
    assert!(!db.live_shared());
    let w = db.writer().unwrap();
    assert!(db.live_shared(), "the claim installs the table");
    assert_eq!(db.share_counts(), [1, 0, 0]);
    let (k, v) = (key(1), b"claimed-first".to_vec());
    assert!(
        w.put(&k, &v).unwrap(),
        "the first write of a handle claimed to write lands"
    );
    model.insert(k, v);
    let w2 = db.writer().unwrap();
    assert_eq!(db.share_counts(), [1, 0, 0], "one table for both");
    for i in 0..300 {
        let (k, v) = (key(i * 2 + 1), format!("second-{i}").into_bytes());
        assert!(w2.put(&k, &v).unwrap(), "write {i}");
        model.insert(k, v);
    }
    hold(&db, &model, "claimed");

    drop(w);
    drop(w2);
    pay_rent(&mut db, &mut model, "rent");
    assert!(!db.live_shared(), "the writer has the table back");
    hold(&db, &model, "left");
    let w = db.writer().unwrap();
    assert!(
        db.live_shared(),
        "a claim after the writer left installs again"
    );
    assert_eq!(db.share_counts()[0], 2);
    let (k, v) = (key(3), b"claimed-again".to_vec());
    assert!(w.put(&k, &v).unwrap());
    model.insert(k, v);
    hold(&db, &model, "claimed again");

    drop(w);
    db.flush().unwrap();
    db.settle().unwrap();
    hold(&db, &model, "flushed");
    drop(db);
    let db = Db::open(&d, options()).unwrap();
    hold(&db, &model, "reopened");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The claim commits what the writer has staged, as `commit` does: a
/// handle under the watermark reads the staged value from the claim on and
/// not before.
#[test]
fn a_claim_to_write_commits_what_is_staged() {
    let _w = Watchdog::arm("a claim over a staged batch waited a minute");
    let (d, mut db, mut model) = loaded("writer-staged", options());
    let h = db.reader().unwrap();
    let k = key(0);
    let before = model[&k].clone();
    db.put(&k, b"staged");
    assert_eq!(
        handle_values(&h, &k),
        vec![before],
        "a staged put is no handle's to read"
    );
    let w = db.writer().unwrap();
    model.insert(k.clone(), b"staged".to_vec());
    assert_eq!(
        handle_values(&h, &k),
        vec![b"staged".to_vec()],
        "the claim committed it"
    );
    assert!(w.put(&key(1), b"after").unwrap());
    model.insert(key(1), b"after".to_vec());
    hold(&db, &model, "claimed over a staged batch");
    drop((h, w));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// No handle is claimed to write where none may write: a store whose
/// writes are logged, which shares nothing, refuses the claim, and holds
/// no slot for it after.
#[test]
fn a_claim_to_write_is_refused_where_no_handle_may_write() {
    let d = scratch("writer-refused");
    let mut db = Db::create(
        &d,
        Options {
            held_frames: true,
            ..options()
        },
    )
    .unwrap();
    assert!(!db.shares());
    db.put(b"k", b"v");
    assert!(db.writer().is_err(), "a claim to write a logged store");
    // A refused claim's handle goes back: after more refusals than the
    // reader table has slots, as many handles are to be had as before.
    let room = |db: &Db| {
        std::iter::from_fn(|| db.reader().ok())
            .take(1 << 16)
            .collect::<Vec<_>>()
            .len()
    };
    let before = room(&db);
    assert!(
        before > 0 && before < 1 << 16,
        "the table's slots: {before}"
    );
    for _ in 0..2 * before {
        assert!(db.writer().is_err());
    }
    assert_eq!(room(&db), before, "a refused claim kept its slot");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The table the writer installs for its handles is made whole before it
/// is published (`Options::shared_populates`): its arena to its cap and
/// its slab, counted in bytes, at a claim and at a commit that finds a
/// handle asking alike. A table a freeze makes for the handles is not, and
/// with the option off none is. The writes land and read back either way.
#[test]
fn the_table_installed_for_the_handles_is_made_whole() {
    let _w = Watchdog::arm("a table made whole waited a minute");
    let cap = 6usize << 20;
    for (name, populates, claimed) in [
        ("whole-claim", true, true),
        ("whole-asked", true, false),
        ("whole-off", false, true),
    ] {
        let o = Options {
            shared_table_bytes: cap,
            shared_populates: populates,
            ..options()
        };
        let (d, mut db, mut model) = loaded(name, o);
        assert_eq!(db.share_populated(), 0, "{name}: nothing before a handle");
        let w = if claimed {
            db.writer().unwrap()
        } else {
            let r = db.reader().unwrap();
            assert!(!r.put(&key(1), b"asks").unwrap(), "{name}");
            db.commit().unwrap();
            r
        };
        assert!(db.live_shared(), "{name}");
        let made = db.share_populated();
        if populates {
            assert!(
                made >= cap as u64,
                "{name}: the table's arena to its cap, {made} bytes"
            );
        } else {
            assert_eq!(made, 0, "{name}: made as it is written");
        }
        // Values enough to fill the table, so that a put is refused and
        // the commit after it freezes the table under a fresh one.
        let big = vec![0x5Au8; 64 << 10];
        let before = db.share_counts();
        for i in 0..160 {
            let k = format!("fill-{i:04}").into_bytes();
            write(&mut db, || w.put(&k, &big));
            model.insert(k, big.clone());
        }
        assert!(db.live_shared(), "{name}: still the handles' table");
        assert_eq!(db.share_counts(), before, "{name}: by freezes, no install");
        assert_eq!(
            db.share_populated(),
            made,
            "{name}: a table a freeze makes is not made ahead"
        );
        hold(&db, &model, name);
        drop(w);
        db.flush().unwrap();
        db.settle().unwrap();
        hold(&db, &model, name);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A value of over an arena block put through a handle into a table made
/// whole is laid past the blocks made ahead, each of which is a block's
/// bytes: it reads back whole, beside the keys written before it and
/// after, through the freeze its range ends the table's room with, the
/// flush and a reopen. The same with the table made as it is written.
#[test]
fn a_value_over_an_arena_block_lands_whole_in_a_table_made_ahead() {
    let _w = Watchdog::arm("a value over a block waited a minute");
    for (name, populates) in [("over-block-whole", true), ("over-block-bare", false)] {
        let o = Options {
            shared_table_bytes: 6 << 20,
            shared_populates: populates,
            ..options()
        };
        let (d, mut db, mut model) = loaded(name, o.clone());
        let w = db.writer().unwrap();
        for i in 0..200 {
            let (k, v) = (key(i * 2 + 1), format!("before-{i}").into_bytes());
            write(&mut db, || w.put(&k, &v));
            model.insert(k, v);
        }
        let big: Vec<u8> = (0..(5usize << 20)).map(|i| (i % 251) as u8).collect();
        let k = b"key-over-a-block".to_vec();
        write(&mut db, || w.put(&k, &big));
        model.insert(k, big);
        for i in 200..400 {
            let (k, v) = (key(i * 2 + 1), format!("after-{i}").into_bytes());
            write(&mut db, || w.put(&k, &v));
            model.insert(k, v);
        }
        hold(&db, &model, name);
        drop(w);
        db.flush().unwrap();
        db.settle().unwrap();
        hold(&db, &model, name);
        drop(db);
        let db = Db::open(&d, o).unwrap();
        hold(&db, &model, name);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}
