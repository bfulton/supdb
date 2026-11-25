//! The live table every handle writes (`Options::shared_writers`).
//!
//! Handles on several threads put, append and delete through one live
//! table at once, beside the writer, with no lock between them. These tests
//! hold the store to every write it took: across the freezes a full table
//! makes, through the seal that reads a frozen table while handles still
//! write, and across a reopen of what the flush made durable.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use supdb::{Db, Options, SyncPolicy, Upkeep};

mod common;
use common::{kv, vf, Watchdog};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-shared-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A store whose live table every handle writes, with a table small enough
/// that a test's writes fill it many times over.
fn options(seal_bytes: usize) -> Options {
    Options {
        sync: SyncPolicy::Explicit,
        held_frames: false,
        scan_block_cache: false,
        upkeep: Upkeep::Inline,
        direct_ingest: false,
        shared_writers: true,
        seal_bytes,
        seal_max_pct: 0,
        ..common::options()
    }
}

/// Every value of `k`, oldest first, through `db`.
fn values(db: &Db, k: &[u8]) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    db.read_all(k, vf(&mut |v| got.push(v.to_vec())))
        .unwrap_or_else(|e| panic!("read of {:?}: {e}", String::from_utf8_lossy(k)));
    got
}

fn own_key(t: usize, i: usize) -> Vec<u8> {
    format!("own-{t}-{i:06}").into_bytes()
}

fn common_key(i: usize) -> Vec<u8> {
    format!("common-{i:05}").into_bytes()
}

/// Every value of `k`, oldest first, through the handle `r`.
fn handle_values(r: &supdb::Reader, k: &[u8]) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    r.read_all(k, vf(&mut |v| got.push(v.to_vec())))
        .unwrap_or_else(|e| panic!("read of {:?}: {e}", String::from_utf8_lossy(k)));
    got
}

/// What thread `t`'s writes leave on its own key `i`: two puts, an append
/// on every third key, and a delete ending every tenth.
fn own_want(t: usize, i: usize) -> Vec<Vec<u8>> {
    if i.is_multiple_of(10) {
        return Vec::new();
    }
    let mut v = vec![format!("{t}-{i}-second").into_bytes()];
    if i.is_multiple_of(3) {
        v.push(format!("{t}-{i}-appended").into_bytes());
    }
    v
}

/// A handle's write made, the table frozen by the writer's commit and the
/// write made again wherever a full table refused it.
fn write(db: &Mutex<Db>, mut once: impl FnMut() -> std::io::Result<bool>) {
    loop {
        if once().unwrap() {
            return;
        }
        db.lock().unwrap().commit().unwrap();
    }
}

/// Handles on four threads each write keys of their own -- a put, a second
/// put, an append on every third, a delete ending every tenth -- and all of
/// them put one set of keys in common, while a fifth thread reads, under a
/// table small enough to freeze many times over. Each thread reads every
/// write of its own keys back through its handle as soon as it is made,
/// while the others make theirs. Every write lands: each thread's keys read
/// back what its writes leave, each common key one whole value some thread
/// put last, a scan lists every live key once, and all of it reads the
/// same after the flush and after a reopen.
#[test]
fn handles_on_threads_write_one_live_table_and_every_write_lands() {
    const THREADS: usize = 4;
    const OWN: usize = 3000;
    const COMMON: usize = 500;
    // A freeze that waits for a writer out of its table for ever -- a slot
    // given back read as a pin below the epoch -- hangs the writes behind
    // it, so the test says so rather than hangs.
    let _w = Watchdog::arm("the shared writers' test waited a minute: a freeze never let go");
    let d = scratch("threads");
    let db = Db::create(&d, options(256 << 10)).unwrap();
    let handles: Vec<supdb::Reader> = (0..THREADS + 1).map(|_| db.reader().unwrap()).collect();
    let seals0 = db.seal_waits().joins;
    let db = Arc::new(Mutex::new(db));
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        let mut handles = handles.into_iter();
        let reader = handles.next().unwrap();
        let watcher = {
            let stop = &stop;
            s.spawn(move || {
                let mut seen = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for i in 0..COMMON {
                        reader
                            .read_all(
                                &common_key(i),
                                vf(&mut |v| {
                                    let s = std::str::from_utf8(v).expect("a value of one write");
                                    assert!(s.starts_with("common-"), "a value torn: {s:?}");
                                    seen += 1;
                                }),
                            )
                            .unwrap();
                    }
                }
                seen
            })
        };
        let writers: Vec<_> = handles
            .enumerate()
            .map(|(t, r)| {
                let db = &db;
                s.spawn(move || {
                    for i in 0..OWN {
                        let k = own_key(t, i);
                        let first = format!("{t}-{i}-first").into_bytes();
                        let second = format!("{t}-{i}-second").into_bytes();
                        write(db, || r.put(&k, &first));
                        assert_eq!(
                            handle_values(&r, &k),
                            vec![first.clone()],
                            "{t}-{i}: its put"
                        );
                        write(db, || r.put(&k, &second));
                        assert_eq!(
                            handle_values(&r, &k),
                            vec![second.clone()],
                            "{t}-{i}: its second put"
                        );
                        if i.is_multiple_of(3) {
                            let app = format!("{t}-{i}-appended").into_bytes();
                            write(db, || r.append(&k, &app));
                            assert_eq!(
                                handle_values(&r, &k),
                                vec![second.clone(), app],
                                "{t}-{i}: its append"
                            );
                        }
                        if i.is_multiple_of(10) {
                            write(db, || r.delete(&k));
                            assert!(handle_values(&r, &k).is_empty(), "{t}-{i}: its delete");
                        }
                        let c = common_key((i * 7 + t) % COMMON);
                        write(db, || r.put(&c, format!("common-{t}-{i}").as_bytes()));
                    }
                })
            })
            .collect();
        let done: Vec<_> = writers.into_iter().map(|h| h.join()).collect();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let seen = watcher.join();
        for d in done {
            if let Err(p) = d {
                std::panic::resume_unwind(p);
            }
        }
        match seen {
            Ok(n) => assert!(n > 0, "the reader read nothing"),
            Err(p) => std::panic::resume_unwind(p),
        }
    });
    let mut db = Arc::try_unwrap(db).ok().unwrap().into_inner().unwrap();
    assert!(
        db.seal_waits().joins - seals0 >= 4,
        "the writes froze the table {} times, too few to cross a freeze",
        db.seal_waits().joins - seals0
    );
    let hold = |db: &Db, when: &str| {
        for t in 0..THREADS {
            for i in 0..OWN {
                assert_eq!(
                    values(db, &own_key(t, i)),
                    own_want(t, i),
                    "{when}: thread {t} key {i}"
                );
            }
        }
        for i in 0..COMMON {
            let v = values(db, &common_key(i));
            assert_eq!(v.len(), 1, "{when}: common key {i}: {v:?}");
            assert!(v[0].starts_with(b"common-"), "{when}: common key {i}");
        }
        let mut keys = Vec::new();
        db.scan(b"", usize::MAX, kv(&mut |k, _| keys.push(k.to_vec())))
            .unwrap();
        let mut want: Vec<Vec<u8>> = (0..THREADS)
            .flat_map(|t| {
                (0..OWN)
                    .filter(|i| !i.is_multiple_of(10))
                    .map(move |i| own_key(t, i))
            })
            .chain((0..COMMON).map(common_key))
            .collect();
        want.sort();
        keys.dedup();
        assert_eq!(keys.len(), want.len(), "{when}: the scan's keys");
        assert_eq!(keys, want, "{when}: the scan's keys");
    };
    hold(&db, "before the flush");
    db.flush().unwrap();
    db.settle().unwrap();
    hold(&db, "after the flush");
    db.close().unwrap();
    let db = Db::open(&d, options(256 << 10)).unwrap();
    hold(&db, "after a reopen");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A freeze waits out the handle still writing the table it replaces: a
/// handle's write held between taking the state and writing its table, a
/// flush that freezes and seals the table meanwhile, and the write is in
/// what the flush made durable -- read back after the flush and after a
/// reopen -- where a seal that read the table before the write landed would
/// have left it in a table nothing reads.
#[test]
fn a_freeze_waits_out_a_write_holding_the_table_it_replaces() {
    let _w = Watchdog::arm("the grace test waited a minute: a freeze never let go");
    let d = scratch("grace");
    let mut db = Db::create(&d, options(256 << 10)).unwrap();
    db.put(b"before", b"0");
    db.commit().unwrap();
    let r = db.reader().unwrap();
    db.hold_shared_writes(200_000);
    std::thread::scope(|s| {
        let held = s.spawn(move || {
            assert!(r.put(b"held", b"landed").unwrap());
            drop(r);
        });
        // The handle is inside its hold by the time the flush freezes.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let t = std::time::Instant::now();
        db.flush().unwrap();
        assert!(
            t.elapsed() >= std::time::Duration::from_millis(100),
            "the flush did not wait for the held write ({:?})",
            t.elapsed()
        );
        held.join().unwrap();
    });
    db.hold_shared_writes(0);
    db.settle().unwrap();
    assert_eq!(values(&db, b"held"), vec![b"landed".to_vec()]);
    db.close().unwrap();
    let db = Db::open(&d, options(256 << 10)).unwrap();
    assert_eq!(
        values(&db, b"held"),
        vec![b"landed".to_vec()],
        "after a reopen"
    );
    assert_eq!(values(&db, b"before"), vec![b"0".to_vec()]);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A freeze lets go of a handle dropped inside its grace: the freeze has
/// read the list of writers with the handle on it, the handle drops -- its
/// slot given back, which reads zero -- before the freeze reads the slot,
/// and the flush ends, with the handle's write in what it made durable. A
/// slot given back read as a pin below the epoch waited for ever, and only
/// a drop in that window shows it, so the grace is held there
/// (`Db::hold_shared_grace`) until the handle has gone.
#[test]
fn a_freeze_lets_go_of_a_writer_dropped_inside_its_grace() {
    let _w = Watchdog::arm("a freeze waited a minute on a writer that had dropped");
    let d = scratch("dropped");
    let mut db = Db::create(&d, options(256 << 10)).unwrap();
    let r = db.reader().unwrap();
    assert!(r.put(b"dropped", b"written").unwrap());
    let gate = db.reader().unwrap();
    db.hold_shared_grace(true);
    std::thread::scope(|s| {
        s.spawn(move || {
            while !gate.shared_grace_holding() {
                std::thread::yield_now();
            }
            drop(r);
            gate.release_shared_grace();
        });
        db.flush().unwrap();
    });
    db.hold_shared_grace(false);
    assert_eq!(values(&db, b"dropped"), vec![b"written".to_vec()]);
    db.close().unwrap();
    let db = Db::open(&d, options(256 << 10)).unwrap();
    assert_eq!(
        values(&db, b"dropped"),
        vec![b"written".to_vec()],
        "after a reopen"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A handle's read-modify-write reads what the key held and puts its value:
/// over a key only the partition holds, over a key the live table ends
/// with a put, and over one whose live chain is an append on the
/// partition's value, which the read takes whole, oldest first.
#[test]
fn a_handles_rmw_reads_what_the_key_held_and_puts_its_value() {
    let d = scratch("rmw");
    let mut db = Db::create(&d, options(1 << 20)).unwrap();
    db.put(b"loaded", b"base");
    db.put(b"appended", b"base");
    db.commit().unwrap();
    db.flush().unwrap();
    let r = db.reader().unwrap();
    let rmw = |k: &[u8], v: &[u8]| {
        let mut got = Vec::new();
        assert!(r.rmw(k, v, |x| got.push(x.to_vec())).unwrap());
        got
    };
    assert_eq!(
        rmw(b"loaded", b"one"),
        vec![b"base".to_vec()],
        "the partition's value"
    );
    assert_eq!(
        rmw(b"loaded", b"two"),
        vec![b"one".to_vec()],
        "the live table's"
    );
    assert_eq!(handle_values(&r, b"loaded"), vec![b"two".to_vec()]);
    assert!(r.append(b"appended", b"more").unwrap());
    assert_eq!(
        rmw(b"appended", b"replaced"),
        vec![b"base".to_vec(), b"more".to_vec()],
        "an append's chain reads the partition's value under it"
    );
    assert_eq!(handle_values(&r, b"appended"), vec![b"replaced".to_vec()]);
    assert_eq!(rmw(b"absent", b"first"), Vec::<Vec<u8>>::new());
    assert_eq!(handle_values(&r, b"absent"), vec![b"first".to_vec()]);
    drop(r);
    db.flush().unwrap();
    assert_eq!(values(&db, b"loaded"), vec![b"two".to_vec()]);
    assert_eq!(values(&db, b"appended"), vec![b"replaced".to_vec()]);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Handles on threads read-modify-write one key the live table holds,
/// each putting a value of its own: every value but the last is read by
/// exactly one of them, and the last by none -- each read is of the value
/// its put replaced, which is what the one probe under the key's stripe
/// buys and a read and a put made apart do not.
#[test]
fn rmws_on_threads_each_replace_the_value_they_read() {
    const THREADS: usize = 3;
    const EACH: usize = 20_000;
    let _w = Watchdog::arm("the rmw test waited a minute");
    let d = scratch("rmwrace");
    let mut db = Db::create(&d, options(64 << 20)).unwrap();
    let handles: Vec<supdb::Reader> = (0..THREADS).map(|_| db.reader().unwrap()).collect();
    assert!(handles[0].put(b"hot", b"start").unwrap());
    let go = std::sync::Barrier::new(THREADS);
    let go = &go;
    let seen: Vec<Vec<(Vec<u8>, Vec<u8>)>> = std::thread::scope(|s| {
        let threads: Vec<_> = handles
            .into_iter()
            .enumerate()
            .map(|(t, r)| {
                s.spawn(move || {
                    go.wait();
                    let mut mine = Vec::with_capacity(EACH);
                    for i in 0..EACH {
                        let v = format!("{t}-{i}").into_bytes();
                        let mut read = Vec::new();
                        assert!(
                            r.rmw(b"hot", &v, |x| read.push(x.to_vec())).unwrap(),
                            "the table filled"
                        );
                        assert_eq!(read.len(), 1, "one value read: {read:?}");
                        mine.push((read.pop().unwrap(), v));
                    }
                    mine
                })
            })
            .collect();
        threads.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut read = std::collections::HashMap::<Vec<u8>, usize>::new();
    let mut written = std::collections::HashSet::new();
    for (r, w) in seen.into_iter().flatten() {
        *read.entry(r).or_default() += 1;
        written.insert(w);
    }
    let last = values(&db, b"hot");
    assert_eq!(last.len(), 1);
    assert_eq!(
        read.get(b"start".as_slice()),
        Some(&1),
        "the first value read once"
    );
    for w in &written {
        let n = read.get(w).copied().unwrap_or(0);
        if *w == last[0] {
            assert_eq!(n, 0, "the last value read by an rmw");
        } else {
            assert_eq!(n, 1, "{}: read by {n} rmws", String::from_utf8_lossy(w));
        }
    }
    db.flush().unwrap();
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A store opened without the option refuses a handle's write, and one
/// with it refuses a handle that holds a snapshot, whose pin a freeze would
/// wait on for as long as the snapshot is held.
#[test]
fn a_handle_writes_only_where_the_store_shares_its_table() {
    let d = scratch("refused");
    let db = Db::create(
        &d,
        Options {
            sync: SyncPolicy::Explicit,
            ..common::options()
        },
    )
    .unwrap();
    let r = db.reader().unwrap();
    assert!(r.put(b"k", b"v").is_err());
    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let d = scratch("snapshot");
    let db = Db::create(&d, options(256 << 10)).unwrap();
    let r = db.reader().unwrap();
    r.snapshot();
    assert!(r.put(b"k", b"v").is_err());
    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}
