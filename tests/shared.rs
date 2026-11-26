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
    // The reader has read a value: the writers' end waits for it before it
    // stops the reader, whose first pass a loaded host may start after the
    // writes are done -- one run in several hundred, with nothing wrong.
    let read_one = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        let mut handles = handles.into_iter();
        let reader = handles.next().unwrap();
        let watcher = {
            let (stop, read_one) = (&stop, &read_one);
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
                                    read_one.store(true, std::sync::atomic::Ordering::Release);
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
        while !read_one.load(std::sync::atomic::Ordering::Acquire) && !watcher.is_finished() {
            std::thread::yield_now();
        }
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
    // The seals the freezes made land on the segment work's thread, up to
    // the frozen list's three of them still in flight as the writes end;
    // counted before they had, the freezes read short one run in hundreds.
    db.settle().unwrap();
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
        // A scan lists every live key once, each with the values its point
        // read gives: a scan reads the live table by entry number, which a
        // table of records names in each record.
        let mut scanned: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
        db.scan(
            b"",
            usize::MAX,
            kv(&mut |k, v| match scanned.last_mut() {
                Some((last, vs)) if last.as_slice() == k => vs.push(v.to_vec()),
                _ => scanned.push((k.to_vec(), vec![v.to_vec()])),
            }),
        )
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
        let keys: Vec<Vec<u8>> = scanned.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys.len(), want.len(), "{when}: the scan's keys");
        assert_eq!(keys, want, "{when}: the scan's keys");
        for (k, vs) in &scanned {
            assert_eq!(
                vs,
                &values(db, k),
                "{when}: the scan's values of {}",
                String::from_utf8_lossy(k)
            );
        }
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

/// Handles on three threads put values of one length over a set of keys in
/// common, round after round -- each put after a key's first in a table
/// overwriting its value where it lies (`Options::shared_in_place`) -- while
/// a handle on a fourth reads every key and scans the store, under a table
/// small enough to freeze about once a round. Every value a read or a scan
/// meets is one put's whole, each a run of one byte, which a read of bytes a
/// writer was overwriting would break; the writers made overwrites in the
/// thousands; and each key reads one whole value after the flush and after
/// a reopen.
#[test]
fn handles_overwrite_in_place_beside_reads_and_scans_and_every_value_is_whole() {
    const THREADS: usize = 3;
    const KEYS: usize = 2000;
    const ROUNDS: usize = 30;
    let _w = Watchdog::arm("the in-place test waited a minute: a freeze never let go");
    let d = scratch("inplace");
    let db = Db::create(&d, options(256 << 10)).unwrap();
    let handles: Vec<supdb::Reader> = (0..THREADS + 1).map(|_| db.reader().unwrap()).collect();
    let db = Arc::new(Mutex::new(db));
    let stop = std::sync::atomic::AtomicBool::new(false);
    // The reader has read a value: the writers' end waits for it before it
    // stops the reader, whose first pass a loaded host may start after the
    // writes are done -- one run in several hundred, with nothing wrong.
    let read_one = std::sync::atomic::AtomicBool::new(false);
    let key = |i: usize| format!("inplace-{i:05}").into_bytes();
    let whole = |v: &[u8]| v.len() == 64 && v.iter().all(|&b| b == v[0]);
    let rewrites: u64 = std::thread::scope(|s| {
        let mut handles = handles.into_iter();
        let reader = handles.next().unwrap();
        let watcher = {
            let (stop, read_one) = (&stop, &read_one);
            s.spawn(move || {
                let mut seen = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for i in 0..KEYS {
                        reader
                            .read_all(
                                &key(i),
                                vf(&mut |v| {
                                    assert!(whole(v), "a read met a value torn: {v:?}");
                                    seen += 1;
                                    read_one.store(true, std::sync::atomic::Ordering::Release);
                                }),
                            )
                            .unwrap();
                    }
                    reader
                        .scan(
                            b"",
                            usize::MAX,
                            kv(&mut |k, v| {
                                assert!(
                                    whole(v),
                                    "a scan met {}'s value torn",
                                    String::from_utf8_lossy(k)
                                );
                                seen += 1;
                            }),
                        )
                        .unwrap();
                }
                seen
            })
        };
        let writers: Vec<_> = handles
            .enumerate()
            .map(|(t, r)| {
                let db = &db;
                s.spawn(move || {
                    for round in 0..ROUNDS {
                        for i in 0..KEYS {
                            let v = [(1 + (t * 31 + round * 7 + i) % 250) as u8; 64];
                            write(db, || r.put(&key(i), &v));
                        }
                    }
                    r.rewrites_made()
                })
            })
            .collect();
        let made: Vec<_> = writers.into_iter().map(|h| h.join()).collect();
        while !read_one.load(std::sync::atomic::Ordering::Acquire) && !watcher.is_finished() {
            std::thread::yield_now();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let seen = watcher.join();
        let made: u64 = made
            .into_iter()
            .map(|m| m.unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .sum();
        match seen {
            Ok(n) => assert!(n > 0, "the reader read nothing"),
            Err(p) => std::panic::resume_unwind(p),
        }
        made
    });
    let puts = (THREADS * ROUNDS * KEYS) as u64;
    assert!(
        rewrites > puts / 2,
        "{rewrites} of {puts} puts overwrote in place: the test did not take the path"
    );
    let mut db = Arc::try_unwrap(db).ok().unwrap().into_inner().unwrap();
    let hold = |db: &Db, when: &str| {
        for i in 0..KEYS {
            let v = values(db, &key(i));
            assert!(v.len() == 1 && whole(&v[0]), "{when}: key {i}: {v:?}");
        }
    };
    hold(&db, "before the flush");
    db.flush().unwrap();
    hold(&db, "after the flush");
    db.close().unwrap();
    let db = Db::open(&d, options(256 << 10)).unwrap();
    hold(&db, "after a reopen");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A table its freeze has closed takes no write: a handle's put started
/// once the freeze has closed and settled the table, and before it has
/// published it frozen, is answered as a full table's is, with nothing
/// written and nothing overwritten, and made again after the flush it
/// lands in the next table. A frozen table is then one nothing writes, as
/// the one writer's is: a write that landed in it after its publish was
/// one a scan made before it had sorted the table without.
#[test]
fn a_put_into_a_table_its_freeze_closed_is_refused() {
    // A table that overwrites its puts where they lie, and one that lays
    // every write down: each is closed to every write.
    for in_place in [true, false] {
        a_closed_table_refuses(in_place);
    }
}

fn a_closed_table_refuses(in_place: bool) {
    let _w = Watchdog::arm("the closed-table test waited a minute");
    let d = scratch(if in_place { "closed" } else { "closed-laid" });
    let mut db = Db::create(
        &d,
        Options {
            shared_in_place: in_place,
            ..options(1 << 20)
        },
    )
    .unwrap();
    let r = db.reader().unwrap();
    assert!(r.put(b"k", &[1u8; 64]).unwrap());
    assert!(r.put(b"k", &[2u8; 64]).unwrap());
    assert_eq!(
        r.rewrites_made(),
        u64::from(in_place),
        "a put of its length overwrote in place where the table does"
    );
    db.hold_shared_closed(true);
    let r = std::thread::scope(|s| {
        let put = s.spawn(move || {
            while !r.shared_closed_holding() {
                std::thread::yield_now();
            }
            let before = r.rewrites_made();
            // Every kind of write, each refused.
            let mut read = 0;
            let refused = [
                r.put(b"k", &[3u8; 64]).unwrap(),
                r.put(b"new", b"v").unwrap(),
                r.append(b"k", b"more").unwrap(),
                r.delete(b"k").unwrap(),
                r.rmw(b"k", &[4u8; 64], vf(&mut |_| read += 1)).unwrap(),
            ];
            assert_eq!(refused, [false; 5], "a closed table took a write");
            assert_eq!(read, 0, "a refused read-modify-write read its key");
            assert_eq!(r.rewrites_made(), before, "a closed table was overwritten");
            assert_eq!(handle_values(&r, b"k"), vec![vec![2u8; 64]]);
            r.release_shared_closed();
            r
        });
        db.flush().unwrap();
        put.join().unwrap()
    });
    db.hold_shared_closed(false);
    assert_eq!(
        values(&db, b"k"),
        vec![vec![2u8; 64]],
        "a refused write landed"
    );
    assert!(values(&db, b"new").is_empty());
    assert!(
        r.put(b"k", &[3u8; 64]).unwrap(),
        "the next table took no write"
    );
    assert_eq!(values(&db, b"k"), vec![vec![3u8; 64]]);
    drop(r);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A freeze settles its table only once an overwrite in flight is through:
/// a handle's overwrite held inside its record's lock, the freeze held in
/// the grace that waits it out, and the live table reads not settled there
/// -- a table settled before its overwrites are through is read as one
/// that never rewrites while a value of it is being written.
#[test]
fn a_freeze_settles_its_table_only_once_an_overwrite_in_flight_is_through() {
    let _w = Watchdog::arm("the settling test waited a minute");
    let d = scratch("settles");
    let mut db = Db::create(&d, options(1 << 20)).unwrap();
    let w = db.reader().unwrap();
    let g = db.reader().unwrap();
    let h = db.reader().unwrap();
    assert!(w.put(b"k", &[1u8; 64]).unwrap());
    g.hold_rewrites(true);
    db.hold_shared_grace(true);
    let settled_in_grace = std::thread::scope(|s| {
        s.spawn(move || {
            assert!(w.put(b"k", &[2u8; 64]).unwrap());
            assert_eq!(w.rewrites_made(), 1, "the held put overwrote in place");
        });
        let gate = s.spawn(move || {
            while !g.rewrite_holding() {
                std::thread::yield_now();
            }
            while !g.shared_grace_holding() {
                std::thread::yield_now();
            }
            let settled = g.live_settled();
            g.release_shared_grace();
            g.hold_rewrites(false);
            settled
        });
        // The flush only once the put is held inside its record's lock: a
        // freeze that closed the table first left the put nothing to
        // overwrite and the gate waiting for a hold that never came, which a
        // put started late under load did.
        while !h.rewrite_holding() {
            std::thread::yield_now();
        }
        db.flush().unwrap();
        gate.join().unwrap()
    });
    db.hold_shared_grace(false);
    assert!(
        !settled_in_grace,
        "the table read settled with an overwrite in flight"
    );
    assert_eq!(values(&db, b"k"), vec![vec![2u8; 64]]);
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
        // The handle inside its hold before the flush freezes: a sleep
        // that bet the handle would be there by its end lost to a thread
        // started late under load.
        while db.shared_writes_held() == 0 {
            std::thread::yield_now();
        }
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
/// A handle's get, put and read-modify-write of a key the live table holds
/// in one put's shape go through the record's address
/// (`Options::shared_lean`), and every answer is the layered path's: the
/// store with the option off answers each the same, overwrites as many in
/// place, and counts none of them lean. A key the live table has no record
/// of is read below it -- in the frozen table a held seal keeps, and in the
/// partition -- as the layered read reads it, the lean probe having said
/// the live table does not hold it.
#[test]
fn a_handles_lean_path_answers_what_the_layered_one_does() {
    let _w = Watchdog::arm("the lean path's test waited a minute");
    let mut answers = Vec::new();
    for lean in [true, false] {
        let d = scratch(if lean { "lean" } else { "nolean" });
        let mut db = Db::create(
            &d,
            Options {
                shared_lean: lean,
                ..options(1 << 20)
            },
        )
        .unwrap();
        db.put(b"part", b"p1");
        db.put(b"frozen", b"f0");
        db.put(b"base", b"b0");
        db.commit().unwrap();
        db.flush().unwrap();
        let r = db.reader().unwrap();
        let rmw = |k: &[u8], v: &[u8]| {
            let mut g = Vec::new();
            assert!(r.rmw(k, v, |x| g.push(x.to_vec())).unwrap());
            g
        };
        // A key in a frozen table, its seal's landing held.
        assert!(r.put(b"frozen", &[9u8; 64]).unwrap());
        db.hold_seal_landing(true);
        db.seal().unwrap();
        assert_eq!(db.frozen_tables(), 1, "the frozen table");
        let mut got: Vec<Vec<Vec<u8>>> = Vec::new();
        let ops0 = r.shared_lean_ops();
        let probes0 = r.placed_live_probes();
        got.push(handle_values(&r, b"frozen"));
        got.push(handle_values(&r, b"part"));
        assert_eq!(
            r.shared_lean_ops(),
            ops0,
            "reads the live table does not end"
        );
        // The read below the table probes the table again only where the
        // lean probe did not say it lacks the key. The placed read is the
        // one that counts, and under the stride switch a form file serves
        // the partition and the general read takes the key
        // (`common::stride_switch`).
        let layered = if common::stride_switch() { 0 } else { 2 };
        assert_eq!(
            r.placed_live_probes() - probes0,
            if lean { 0 } else { layered },
            "lean: {lean}: the live table probed again"
        );
        assert!(r.put(b"k", &[1u8; 64]).unwrap());
        got.push(handle_values(&r, b"k"));
        assert!(r.put(b"k", &[2u8; 64]).unwrap());
        got.push(handle_values(&r, b"k"));
        got.push(rmw(b"k", &[3u8; 64]));
        got.push(handle_values(&r, b"k"));
        got.push(rmw(b"part", b"p2"));
        got.push(handle_values(&r, b"part"));
        assert!(r.delete(b"k").unwrap());
        got.push(handle_values(&r, b"k"));
        // A chain the lean path leaves to the layered read -- an append
        // over the partition's value, no tombstone under it -- read with
        // the partition below it, not as a key the table lacks.
        assert!(r.append(b"base", b"b1").unwrap());
        got.push(handle_values(&r, b"base"));
        // The reads of `k` and of `part` after its put, the put of `k`'s
        // length and its read-modify-write.
        assert_eq!(
            r.shared_lean_ops() - ops0,
            if lean { 7 } else { 0 },
            "lean: {lean}"
        );
        got.push(vec![r.rewrites_made().to_be_bytes().to_vec()]);
        drop(r);
        db.hold_seal_landing(false);
        db.flush().unwrap();
        for k in [&b"frozen"[..], b"part", b"k"] {
            got.push(values(&db, k));
        }
        answers.push(got);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
    assert_eq!(
        answers[0], answers[1],
        "the lean path's answers and the layered one's"
    );
    let a = &answers[0];
    assert_eq!(a[0], vec![vec![9u8; 64]], "the frozen table's value");
    assert_eq!(a[1], vec![b"p1".to_vec()], "the partition's");
    assert_eq!(a[4], vec![vec![2u8; 64]], "the value the rmw replaced");
    assert_eq!(
        a[6],
        vec![b"p1".to_vec()],
        "the rmw's read of the partition"
    );
    assert_eq!(a[8], Vec::<Vec<u8>>::new(), "a deleted key");
    assert_eq!(
        a[9],
        vec![b"b0".to_vec(), b"b1".to_vec()],
        "an append over the partition's value"
    );
    assert_eq!(
        a[10],
        vec![2u64.to_be_bytes().to_vec()],
        "two overwrites in place"
    );
}

/// A store of several partitions with no piece and no frozen table, its
/// keys of several lengths, some past a word, loaded and drained, for the
/// reads below the shared table; the store, its keys, and keys it lacks
/// that fall between and beyond them.
fn partitioned(name: &str, o: Options) -> (PathBuf, Db, Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let d = scratch(name);
    let mut db = Db::create(
        &d,
        Options {
            partition_bytes: Some(1024),
            ..o
        },
    )
    .unwrap();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for i in 0..300 {
        keys.push(format!("pk-{i:04}").into_bytes());
        if i % 3 == 0 {
            keys.push(format!("pk-{i:04}-and-a-tail-past-a-word").into_bytes());
        }
    }
    keys.sort();
    for k in &keys {
        db.put(k, k);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let absent: Vec<Vec<u8>> = (0..300)
        .step_by(7)
        .map(|i| format!("pk-{i:04}x").into_bytes())
        .chain([
            b"pk".to_vec(),
            b"pk-9999".to_vec(),
            b"zz".to_vec(),
            Vec::new(),
        ])
        .collect();
    (d, db, keys, absent)
}

/// A key the shared table's lean probe finds no record of is read from
/// its partition alone where the store has no piece and no frozen table
/// (`Options::point_lone_absent`), and the partition is placed by the
/// fences' words (`Options::point_fence_words`): every answer is the one
/// the placed read gives and the one the search by bytes gives, over keys
/// at, between and past the fences, and the reads are counted by the
/// read that answered them. A key the table holds is the lean probe's,
/// and a store with a frozen table is the placed read's.
#[test]
fn a_key_the_live_table_lacks_is_read_from_its_partition_alone() {
    let _w = Watchdog::arm("the lone read's test waited a minute");
    let mut answers = Vec::new();
    for (lone, words) in [(true, true), (false, true), (true, false)] {
        let (d, mut db, keys, absent) = partitioned(
            &format!("lone-{lone}-{words}"),
            Options {
                point_lone_absent: lone,
                point_fence_words: words,
                ..options(1 << 20)
            },
        );
        assert!(db.segments() > 4, "partitions: {}", db.segments());
        let r = db.reader().unwrap();
        let mut got: Vec<Vec<Vec<u8>>> = Vec::new();
        let (lone0, layer0) = r.point_lean_reads();
        for k in keys.iter().chain(&absent) {
            got.push(handle_values(&r, k));
        }
        let (lone1, layer1) = r.point_lean_reads();
        // Under the stride switch a form file serves the partitions and
        // the general read takes every key (`common::stride_switch`).
        if !common::stride_switch() {
            let n = (keys.len() + absent.len()) as u64;
            assert_eq!(
                (lone1 - lone0, layer1 - layer0),
                if lone { (n, 0) } else { (0, n) },
                "lone {lone}, words {words}: the reads by the read that answered"
            );
        }
        // A key the table holds, and then a store with a frozen table.
        assert!(r.put(&keys[5], b"live").unwrap());
        let ops0 = r.shared_lean_ops();
        got.push(handle_values(&r, &keys[5]));
        assert_eq!(
            r.shared_lean_ops() - ops0,
            1,
            "the table's key is the lean probe's"
        );
        assert!(r.put(&keys[7], b"frozen").unwrap());
        db.hold_seal_landing(true);
        db.seal().unwrap();
        assert_eq!(db.frozen_tables(), 1, "the frozen table");
        let (lone2, _) = r.point_lean_reads();
        got.push(handle_values(&r, &keys[7]));
        got.push(handle_values(&r, &keys[9]));
        assert_eq!(
            r.point_lean_reads().0,
            lone2,
            "a frozen table is the placed read's"
        );
        drop(r);
        db.hold_seal_landing(false);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        answers.push(got);
    }
    assert_eq!(answers[0], answers[1], "the lone read and the placed one");
    assert_eq!(answers[0], answers[2], "the search by words and by bytes");
    let a = &answers[0];
    assert!(
        a.iter().filter(|v| v.is_empty()).count() >= 4,
        "the keys the store lacks read absent"
    );
    let n = a.len();
    assert_eq!(a[n - 3], vec![b"live".to_vec()], "the table's value");
    assert_eq!(
        a[n - 2],
        vec![b"frozen".to_vec()],
        "the frozen table's value"
    );
}

/// A handle whose read fell below the shared table asks for its next key's
/// partition slot before it probes the table, and hands the key's word
/// and partition to the read below (`Options::shared_fall_ahead`): a run
/// of reads below the table asks at every read after its first, a read
/// the table ends stops the asks, and every key, in whichever partition,
/// reads back. Under `shared_fall_always` every read asks; with the
/// option off none does, and the answers are the same.
#[test]
fn a_handle_whose_read_fell_below_the_table_asks_for_the_next_partition_first() {
    let _w = Watchdog::arm("the fall-ahead test waited a minute");
    let mut answers = Vec::new();
    for (fall, always) in [(true, false), (true, true), (false, false)] {
        let (d, db, keys, absent) = partitioned(
            &format!("fall-{fall}-{always}"),
            Options {
                shared_fall_ahead: fall,
                shared_fall_always: always,
                ..options(1 << 20)
            },
        );
        let r = db.reader().unwrap();
        assert!(r.put(b"hot", b"h").unwrap());
        let mut got: Vec<Vec<Vec<u8>>> = Vec::new();
        let asks = |want: u64, what: &str| {
            // Under the stride switch a form file serves the partitions,
            // whose slots the read does not ask for
            // (`common::stride_switch`).
            if !common::stride_switch() {
                assert_eq!(
                    r.shared_fall_asks(),
                    want,
                    "fall {fall}, always {always}: {what}"
                );
            }
        };
        got.push(handle_values(&r, b"hot"));
        asks(
            u64::from(always),
            "a read the table ends asks nothing after it",
        );
        got.push(handle_values(&r, &keys[0]));
        asks(2 * u64::from(always), "the first read below the table");
        for k in keys.iter().chain(&absent) {
            got.push(handle_values(&r, k));
        }
        let n = (keys.len() + absent.len()) as u64;
        let after = if fall { n } else { 0 } + if always { 2 } else { 0 };
        asks(after, "every read after one that fell");
        got.push(handle_values(&r, b"hot"));
        got.push(handle_values(&r, &keys[1]));
        let after = after + if fall { 1 } else { 0 } + u64::from(always);
        asks(after, "the read after the table's");
        // A read the table's chain leaves to the read below -- an append
        // over the partition's value -- found a record, and asks nothing
        // after it.
        assert!(r.append(&keys[3], b"more").unwrap());
        got.push(handle_values(&r, &keys[2]));
        got.push(handle_values(&r, &keys[3]));
        got.push(handle_values(&r, &keys[4]));
        let after = after + 2 * u64::from(fall) + u64::from(always);
        asks(after, "the read after a chain the table does not end");
        drop(r);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        answers.push(got);
    }
    assert_eq!(answers[0], answers[2], "the asks and none");
    assert_eq!(answers[1], answers[2], "an ask at every read and none");
    for (i, v) in answers[0][2..].iter().enumerate().take(5) {
        assert!(!v.is_empty(), "key {i} reads back");
    }
}

/// A handle's read-modify-write of a key the shared table has no record of
/// reads the key below the table in the pin its lean probe was made in,
/// with the key's partition asked for ahead of that probe where the last
/// read fell below (`Options::shared_rmw_absent`): over keys at, between
/// and past the fences, each first touch is given the partition's value
/// and puts its own, a second touch is the lean overwrite's, and after one
/// the next first touch asks for nothing ahead. The answers are the ones
/// the read-modify-write that probes the table again gives.
#[test]
fn a_handles_rmw_of_a_key_the_table_lacks_reads_it_below_in_one_pin() {
    let _w = Watchdog::arm("the rmw-below test waited a minute");
    let mut answers = Vec::new();
    for absent in [true, false] {
        let (d, db, keys, missing) = partitioned(
            &format!("rmwbelow-{absent}"),
            Options {
                shared_rmw_absent: absent,
                ..options(1 << 20)
            },
        );
        let r = db.reader().unwrap();
        let rmw = |k: &[u8], v: &[u8]| {
            let mut g = Vec::new();
            assert!(r.rmw(k, v, |x| g.push(x.to_vec())).unwrap());
            g
        };
        // Under the stride switch a form file serves the partitions, whose
        // slots the read does not ask for (`common::stride_switch`).
        let asks = |want: u64, what: &str| {
            if !common::stride_switch() {
                assert_eq!(r.shared_fall_asks(), want, "absent {absent}: {what}");
            }
        };
        let mut got: Vec<Vec<Vec<u8>>> = Vec::new();
        let (lone0, _) = r.point_lean_reads();
        for k in keys.iter().chain(&missing) {
            got.push(rmw(k, &[7u8; 16]));
        }
        let n = (keys.len() + missing.len()) as u64;
        // Each first touch read the partition alone, the table known not to
        // hold the key (`Options::point_lone_absent`), in either shape.
        if !common::stride_switch() {
            assert_eq!(
                r.point_lean_reads().0 - lone0,
                n,
                "absent {absent}: the reads below"
            );
        }
        assert_eq!(
            r.shared_rmw_below(),
            if absent { n } else { 0 },
            "absent {absent}: the first touches read below in their pin"
        );
        // Every first touch but the first asks, after one that fell, both
        // where the read-modify-write asks and where its read does.
        asks(n - 1, "the first touches after the first");
        // A second touch is the lean overwrite's. Where the read-modify-write
        // asks ahead itself, the first second touch after a first touch
        // asks, and its overwrite ends the run of reads below: the get after
        // it asks nothing. Where it does not, the get asks, the mark left by
        // the read before the overwrite.
        let ops0 = r.shared_lean_ops();
        got.push(rmw(&keys[0], &[8u8; 16]));
        asks(n - 1 + u64::from(absent), "a second touch after a first");
        let mut fresh = b"pk-0000".to_vec();
        fresh.extend_from_slice(b"-new");
        got.push(handle_values(&r, &fresh));
        asks(n, "the get after a second touch");
        for k in keys.iter().skip(1).take(19) {
            got.push(rmw(k, &[8u8; 16]));
        }
        assert_eq!(
            r.shared_lean_ops() - ops0,
            20,
            "a second touch is the lean overwrite's"
        );
        got.push(rmw(&fresh, &[9u8; 16]));
        for k in keys.iter().chain(&missing).chain([&fresh]) {
            got.push(handle_values(&r, k));
        }
        if absent {
            for (k, g) in keys.iter().zip(&got) {
                assert_eq!(g, &vec![k.clone()], "the partition's value of {k:?}");
            }
            for g in &got[keys.len()..keys.len() + missing.len()] {
                assert!(g.is_empty(), "a key the store lacks");
            }
        }
        drop(r);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        answers.push(got);
    }
    assert_eq!(answers[0], answers[1], "one pin and three probes");
}

/// A handle's gets and puts read no length of the live table it shares,
/// whose inserts leave the length to its readers
/// (`Options::shared_len_lazy`): after a handle's inserts and its gets of
/// them, the length is where the inserts left it, and the handle's scan
/// carries it over every entry written and lists every key. With the
/// option off the inserts carry it themselves.
#[test]
fn a_handles_point_operations_leave_the_shared_length_and_a_scan_carries_it() {
    for lazy in [true, false] {
        let d = scratch(if lazy { "lazylen" } else { "eagerlen" });
        let db = Db::create(
            &d,
            Options {
                shared_len_lazy: lazy,
                ..options(1 << 20)
            },
        )
        .unwrap();
        let r = db.reader().unwrap();
        for i in 0..50 {
            assert!(r.put(&common_key(i), b"v").unwrap());
        }
        for i in 0..50 {
            assert_eq!(handle_values(&r, &common_key(i)), vec![b"v".to_vec()]);
        }
        assert_eq!(
            r.shared_len_carried(),
            if lazy { 0 } else { 50 },
            "lazy {lazy}: the length after the inserts and the gets"
        );
        let mut keys = Vec::new();
        r.scan(b"common-", 100, kv(&mut |k, _| keys.push(k.to_vec())))
            .unwrap();
        assert_eq!(
            keys,
            (0..50).map(common_key).collect::<Vec<_>>(),
            "lazy {lazy}"
        );
        assert_eq!(
            r.shared_len_carried(),
            50,
            "lazy {lazy}: carried by the scan"
        );
        drop(r);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

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
