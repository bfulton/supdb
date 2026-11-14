//! The runs of ordered ingest held to the log across a crash.
//!
//! A run keeps its records in a file of its own and the WAL never sees
//! them, so the store's history lies in several files: the log's, and one
//! for each run not yet a named segment. What a process's death leaves is
//! all of them whole, since every commit is written where it goes. What a
//! machine's crash may leave, under a policy that does not sync every
//! commit, is any of them cut back to what a sync had covered, and which
//! ones is the kernel's choice and not the order they were written in. The
//! store that reopens holds a prefix of what was committed all the same:
//! `SyncPolicy`'s words are that an unsynced tail is lost whole and never
//! served in part.
//!
//! No test can pull the plug, so each one builds the state a lost page
//! leaves: the writer is stopped as a crash stops it (`Db::crash`) with its
//! seals held, so that every run is still under its temp name as the
//! writer left it, and then a file is cut or taken away by hand before the
//! store is opened again.

use std::path::{Path, PathBuf};

use supdb::{Db, Options, SyncPolicy};

mod common;
use common::vf;

const BATCH: u64 = 1_000;
const VALUE: [u8; 100] = [0x5a; 100];

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-runlog-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(i: u64) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

/// A store that syncs nothing at a commit and closes a run every megabyte
/// or so, eight batches of these.
fn buffered() -> Options {
    Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        seal_bytes: 1 << 20,
        ..Options::default()
    }
}

/// The options of a store that is read and then stopped as a crash stops
/// it: its segment work inline, since `Db::crash` leaves a store's threads
/// running, and a thread that shapes what the reads touched would go on
/// publishing beside the store opened next.
fn inline(o: Options) -> Options {
    Options {
        publish_in_background: false,
        ..o
    }
}

fn values(db: &Db, k: &[u8]) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    db.read_all(k, vf(&mut |v| got.push(v.to_vec()))).unwrap();
    got
}

/// Batch `b` appended in key order and committed: keys `b * BATCH` on.
fn ordered_batch(db: &mut Db, b: u64) {
    for j in 0..BATCH {
        db.append(&key(b * BATCH + j), &VALUE);
    }
    db.commit().unwrap();
}

/// Whether batch `b` is in the store: every key with its value, or none.
fn holds(db: &Db, b: u64) -> bool {
    let n = (0..BATCH)
        .filter(|&j| {
            let v = values(db, &key(b * BATCH + j));
            assert!(
                v.is_empty() || v == vec![VALUE.to_vec()],
                "batch {b}, key {j}: not the value written"
            );
            !v.is_empty()
        })
        .count() as u64;
    assert!(n == 0 || n == BATCH, "batch {b} is there in part, {n} keys");
    n == BATCH
}

fn run_file(d: &Path, id: u64) -> PathBuf {
    d.join(format!("direct-{id:08}.tmp"))
}

fn names(d: &Path, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    v.sort();
    v
}

/// Ordered batches committed with every seal held until a second run has
/// `more` batches on its file behind the first run's close; the count of
/// batches committed and the first batch of the second run.
fn two_runs(db: &mut Db, d: &Path, more: u64) -> (u64, u64) {
    db.hold_seal_landing(true);
    let mut b = 0;
    while !run_file(d, 1).exists() {
        assert!(b < 64, "no second run after {b} batches");
        ordered_batch(db, b);
        b += 1;
    }
    let second = b - 1;
    for _ in 0..more {
        ordered_batch(db, b);
        b += 1;
    }
    assert!(run_file(d, 0).exists(), "the first run's seal was held");
    (b, second)
}

/// The window the suite's recovery workload could not reach: a run closed
/// with nothing synced, the writer went on into the next, and the machine
/// went down holding the second run's pages and not the first run's last.
/// The open took every run it found, each to its own last commit marker,
/// and served the second run's batches without the ones before them.
#[test]
fn a_run_cut_short_ends_the_history_before_the_run_after_it() {
    let d = dir("short");
    let mut db = Db::create(&d, buffered()).unwrap();
    let (n, second) = two_runs(&mut db, &d, 3);
    db.crash();
    // The first run's end, lost: a quarter of a megabyte, two batches.
    let first = run_file(&d, 0);
    let len = std::fs::metadata(&first).unwrap().len();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&first)
        .unwrap();
    f.set_len(len - (256 << 10)).unwrap();
    drop(f);

    let check = |db: &Db| -> u64 {
        let kept = (0..n).take_while(|&b| holds(db, b)).count() as u64;
        assert!(
            kept > 0 && kept < second,
            "the cut took batches of the first run: {kept} kept of {second}"
        );
        for b in kept..n + 1 {
            assert!(
                !holds(db, b),
                "batch {b} is there and batch {kept}, committed before it, is not"
            );
        }
        kept
    };
    let db = Db::open(&d, inline(buffered())).unwrap();
    let kept = check(&db);
    assert!(
        names(&d, "direct-").is_empty(),
        "a run the history never reached is still there: {:?}",
        names(&d, "direct-")
    );
    // The store goes on from where its history ends, and what it writes
    // next stands there: the lost batches again, a crash, and all of it.
    let mut db = db;
    for b in kept..n {
        ordered_batch(&mut db, b);
    }
    db.crash();
    let db = Db::open(&d, buffered()).unwrap();
    for b in 0..n {
        assert!(holds(&db, b), "batch {b}, written again, is not there");
    }
    db.close().unwrap();
    let db = Db::open(&d, buffered()).unwrap();
    for b in 0..n {
        assert!(holds(&db, b), "batch {b} is gone after a close");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A run that closed unsynced, frames in the log after it, and a crash
/// that kept the frames and not the run's file at all: young enough that
/// its name had not reached the directory. The frames stand past the run's
/// sequence numbers, in a file whose header says a run was left unsynced
/// before it, so the log ends there and the update the frames carry is not
/// served over a store without the keys it updated.
#[test]
fn frames_behind_a_run_that_is_gone_are_not_served() {
    let d = dir("gone");
    let mut db = Db::create(&d, buffered()).unwrap();
    db.hold_seal_landing(true);
    for b in 0..3 {
        ordered_batch(&mut db, b);
    }
    // A key below the run's greatest: the run closes and the write goes to
    // the log, with two more behind it.
    for i in 0..3u64 {
        db.put(&key(i), b"after the run");
        db.commit().unwrap();
    }
    assert_eq!(
        db.runs_unsynced(),
        (None, 1),
        "the run closed with commits no sync had covered"
    );
    db.crash();
    std::fs::remove_file(run_file(&d, 0)).unwrap();

    let db = Db::open(&d, buffered()).unwrap();
    for i in 0..3 * BATCH {
        assert!(
            values(&db, &key(i)).is_empty(),
            "key {i} is served and the run that held it is gone"
        );
    }
    // And the store takes writes where its history ends.
    let mut db = db;
    ordered_batch(&mut db, 0);
    db.close().unwrap();
    let db = Db::open(&d, buffered()).unwrap();
    assert!(holds(&db, 0));
    assert_eq!(values(&db, &key(BATCH)), Vec::<Vec<u8>>::new());
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// The other order: frames in the log, unsynced, then a run, and a crash
/// that kept the run's file and not the log's tail. The run stands past
/// the frames' sequence numbers, the log does not reach its start, and it
/// is not taken.
#[test]
fn a_run_behind_frames_that_are_gone_is_not_served() {
    let d = dir("tail");
    let mut db = Db::create(&d, buffered()).unwrap();
    db.hold_seal_landing(true);
    // Frames: three keys, greatest first, so none of them opens a run.
    for i in [900u64, 800, 700] {
        db.put(&key(10 * BATCH + i), b"in the log");
        db.commit().unwrap();
    }
    // The table sealed, so the next key above the store's greatest finds
    // it empty and opens a run.
    db.seal().unwrap();
    for b in 11..14 {
        ordered_batch(&mut db, b);
    }
    assert!(
        run_file(&d, 1).exists() || run_file(&d, 0).exists(),
        "a run opened"
    );
    let (wal, _, written) = db.wal_durable();
    assert!(written > 8, "the log holds the three frames");
    db.crash();
    // The log's unsynced tail, lost: everything behind its header.
    let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
    f.set_len(8).unwrap();
    drop(f);

    let db = Db::open(&d, buffered()).unwrap();
    for b in 11..14 {
        assert!(
            !holds(&db, b),
            "batch {b} of the run is served and the frames before it are gone"
        );
    }
    for i in [900u64, 800, 700] {
        assert!(values(&db, &key(10 * BATCH + i)).is_empty());
    }
    assert!(names(&d, "direct-").is_empty(), "the run is still there");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// Nothing lost: frames, a run, frames behind it, a second run, and a
/// writer stopped as a kill stops it, under each policy. Everything is
/// there at the open; at the open after a second crash with nothing
/// written between, when the runs are pieces and their numbers a gap in
/// the log that only the store's own note fills; after more writes and a
/// third; and after a close, when the manifest covers all of it and the
/// notes are gone.
#[test]
fn runs_and_frames_come_back_whole_and_stay_so() {
    for sync in [
        SyncPolicy::EveryN(u32::MAX),
        SyncPolicy::Always,
        SyncPolicy::EveryN(3),
    ] {
        let opts = || Options {
            sync,
            seal_bytes: 1 << 20,
            ..Options::default()
        };
        let d = dir(&format!("whole-{sync:?}").replace(['(', ')'], "-"));
        let mut db = Db::create(&d, opts()).unwrap();
        db.hold_seal_landing(true);
        // Frames, sealed; a run of three batches; an update that closes
        // it, in the log; then keys above the store's greatest again.
        for i in [900u64, 800, 700] {
            db.put(&key(i), b"first frames");
            db.commit().unwrap();
        }
        db.seal().unwrap();
        for b in 1..4 {
            ordered_batch(&mut db, b);
        }
        db.put(&key(900), b"second frames");
        db.commit().unwrap();
        db.put(&key(4 * BATCH + 5), b"second frames");
        db.commit().unwrap();
        db.crash();

        let check = |db: &Db, what: &str| {
            for b in 1..4 {
                assert!(holds(db, b), "{sync:?}, {what}: batch {b} of the run");
            }
            assert_eq!(
                values(db, &key(900)),
                vec![b"second frames".to_vec()],
                "{sync:?}, {what}: the update behind the run"
            );
            assert_eq!(values(db, &key(800)), vec![b"first frames".to_vec()]);
            assert_eq!(values(db, &key(700)), vec![b"first frames".to_vec()]);
            assert_eq!(
                values(db, &key(4 * BATCH + 5)),
                vec![b"second frames".to_vec()],
                "{sync:?}, {what}: the last commit"
            );
        };
        let db = Db::open(&d, inline(opts())).unwrap();
        check(&db, "the first open");
        assert!(names(&d, "direct-").is_empty(), "{sync:?}: a run was left");
        assert_eq!(
            names(&d, "cov-").len(),
            1,
            "{sync:?}: the run's numbers are noted while the manifest covers less"
        );
        db.crash();
        let mut db = Db::open(&d, inline(opts())).unwrap();
        check(&db, "the second open");
        db.put(&key(5), b"third frames");
        db.commit().unwrap();
        db.crash();
        let db = Db::open(&d, opts()).unwrap();
        check(&db, "the third open");
        assert_eq!(values(&db, &key(5)), vec![b"third frames".to_vec()]);
        db.close().unwrap();
        let db = Db::open(&d, opts()).unwrap();
        check(&db, "after a close");
        assert_eq!(values(&db, &key(5)), vec![b"third frames".to_vec()]);
        assert!(
            names(&d, "cov-").is_empty(),
            "{sync:?}: a note outlived the manifest that covers its range: {:?}",
            names(&d, "cov-")
        );
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A run every commit of which was synced, frames behind it, and the run's
/// file gone: not a tail a crash may take, since the policy promised every
/// one of those commits, so the open refuses as it refuses a log with a
/// record missing, and serves nothing from behind the hole.
#[test]
fn a_synced_run_that_is_gone_is_a_record_lost() {
    let d = dir("damage");
    let mut db = Db::create(&d, Options::default()).unwrap();
    db.hold_seal_landing(true);
    for b in 0..2 {
        ordered_batch(&mut db, b);
    }
    db.put(&key(7), b"behind the run");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 0), "every commit was synced");
    db.crash();
    std::fs::remove_file(run_file(&d, 0)).unwrap();
    let e = match Db::open(&d, Options::default()) {
        Ok(_) => panic!("a store with a synced run missing opened"),
        Err(e) => e.to_string(),
    };
    assert!(
        e.contains("sequence gap"),
        "refused for another reason: {e}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// What a sync by policy acknowledges is every commit so far, wherever it
/// was written: a run that closed with commits unsynced is synced by the
/// policy's next sync, though that sync is of the log, and `sync` covers
/// the run in progress, which holds none of its records in the log.
#[test]
fn a_sync_covers_the_runs_as_well_as_the_log() {
    let d = dir("sync");
    let mut db = Db::create(
        &d,
        Options {
            sync: SyncPolicy::EveryN(4),
            // The run in progress stays where it is at a `sync`.
            adaptive_shape: false,
            ..Options::default()
        },
    )
    .unwrap();
    db.hold_seal_landing(true);
    // Two commits into a run, a third that closes it into the log: three
    // of the policy's four, none synced.
    ordered_batch(&mut db, 0);
    ordered_batch(&mut db, 1);
    assert_eq!(db.runs_unsynced(), (Some(true), 0));
    db.put(&key(0), b"closes the run");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 1));
    let before = db.seal_waits().loose_synced;
    // The fourth: the policy's sync, of the log and of the run before it.
    db.put(&key(1), b"the fourth commit");
    db.commit().unwrap();
    assert_eq!(
        db.runs_unsynced(),
        (None, 0),
        "the policy's sync left a run loose"
    );
    assert_eq!(db.seal_waits().loose_synced, before + 1);
    let (_, synced, written) = db.wal_durable();
    assert_eq!(synced, written, "the policy's sync left frames unsynced");

    // The other order: a frame in the log, unsynced, a run behind it, and
    // the policy's fourth commit falling in the run. The sync is of the
    // run's file, and of the log before it.
    db.put(&key(2), b"a frame before the run");
    db.commit().unwrap();
    let (_, synced, written) = db.wal_durable();
    assert!(synced < written, "the frame is unsynced");
    db.seal().unwrap();
    ordered_batch(&mut db, 2);
    ordered_batch(&mut db, 3);
    assert_eq!(db.runs_unsynced().0, Some(true));
    ordered_batch(&mut db, 4);
    assert_eq!(
        db.runs_unsynced().0,
        Some(false),
        "the policy's sync left the run in progress unsynced"
    );
    let (_, synced, written) = db.wal_durable();
    assert_eq!(
        synced, written,
        "the policy's sync, made in a run, left the frames before the run unsynced"
    );

    // A run in progress with a commit unsynced, and a `sync`.
    ordered_batch(&mut db, 5);
    assert_eq!(db.runs_unsynced().0, Some(true));
    db.sync().unwrap();
    assert_eq!(
        db.runs_unsynced().0,
        Some(false),
        "`sync` returned over a run with commits no sync had covered"
    );
    db.crash();
    let _ = std::fs::remove_dir_all(&d);
}

/// A run's temp name is behind a directory barrier before a commit of the
/// run is acknowledged as synced: a sync of the file promises the file and
/// not the entry that names it, and a run whose name a machine's crash took
/// is every commit of it gone. One barrier a run, at the first sync that
/// covers it, on each path that makes one: a commit under a policy that
/// syncs every commit, the policy's sync falling in the run, the policy's
/// sync falling after the run has closed, and `sync`. No test can lose a
/// name, so this one counts what each acknowledgement left unpromised and
/// the barriers the writer made.
#[test]
fn a_runs_name_is_behind_a_barrier_before_its_commits_are_acknowledged() {
    // Every commit synced: the run's first commit carries the barrier, and
    // no commit after it.
    let d = dir("name-always");
    let mut db = Db::create(
        &d,
        Options {
            sync: SyncPolicy::Always,
            adaptive_shape: false,
            ..Options::default()
        },
    )
    .unwrap();
    db.hold_seal_landing(true);
    assert_eq!(db.seal_waits().run_names, 0);
    ordered_batch(&mut db, 0);
    assert_eq!(
        db.run_names_unsynced(),
        (Some(false), 0),
        "a synced commit was acknowledged into a run whose name nothing promises"
    );
    assert_eq!(db.seal_waits().run_names, 1);
    ordered_batch(&mut db, 1);
    ordered_batch(&mut db, 2);
    assert_eq!(
        db.seal_waits().run_names,
        1,
        "one barrier a run, not one a commit"
    );
    // The run closes into the log, synced and named: nothing is owed.
    db.put(&key(0), b"closes the run");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 0));
    assert_eq!(db.run_names_unsynced(), (None, 0));
    assert_eq!(db.seal_waits().run_names, 1);
    // The next run is a new name.
    db.seal().unwrap();
    ordered_batch(&mut db, 3);
    assert_eq!(db.run_names_unsynced(), (Some(false), 0));
    assert_eq!(db.seal_waits().run_names, 2);
    db.crash();
    let _ = std::fs::remove_dir_all(&d);

    // A sync every fourth commit.
    let d = dir("name-everyn");
    let mut db = Db::create(
        &d,
        Options {
            sync: SyncPolicy::EveryN(4),
            adaptive_shape: false,
            ..Options::default()
        },
    )
    .unwrap();
    db.hold_seal_landing(true);
    // The policy's sync falling in the run.
    ordered_batch(&mut db, 0);
    ordered_batch(&mut db, 1);
    ordered_batch(&mut db, 2);
    assert_eq!(
        db.run_names_unsynced(),
        (Some(true), 0),
        "nothing is acknowledged as synced yet, and nothing is owed"
    );
    assert_eq!(db.seal_waits().run_names, 0);
    ordered_batch(&mut db, 3);
    assert_eq!(db.runs_unsynced(), (Some(false), 0));
    assert_eq!(
        db.run_names_unsynced(),
        (Some(false), 0),
        "the policy's sync covered the run's file and not its name"
    );
    assert_eq!(db.seal_waits().run_names, 1);
    // That run closes named, with two commits the policy has not synced:
    // loose for its file, and its name is owed nothing more.
    ordered_batch(&mut db, 4);
    ordered_batch(&mut db, 5);
    db.put(&key(0), b"closes the run");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 1));
    assert_eq!(db.run_names_unsynced(), (None, 0));
    db.put(&key(1), b"the policy's fourth");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 0));
    assert_eq!(db.seal_waits().run_names, 1, "a name promised twice");

    // The policy's sync falling after a run closed that no sync had
    // reached, the live log file holding frames: the frames after the run
    // go to a new file (`WAL_LOOSE_RUN`), whose name is owed a barrier of
    // its own, and that one barrier is the run's name's too.
    db.seal().unwrap();
    ordered_batch(&mut db, 6);
    ordered_batch(&mut db, 7);
    let rotations = db.seal_waits().wal_rotations;
    db.put(&key(2), b"closes the run");
    db.commit().unwrap();
    assert_eq!(db.seal_waits().wal_rotations, rotations + 1);
    assert_eq!(db.runs_unsynced(), (None, 1));
    assert_eq!(db.run_names_unsynced(), (None, 1), "the run closed unnamed");
    db.put(&key(3), b"the policy's fourth");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 0));
    assert_eq!(db.run_names_unsynced(), (None, 0));
    assert_eq!(
        db.seal_waits().run_names,
        1,
        "the rotation's barrier is of the directory, and the run's name was in it"
    );
    db.crash();
    let _ = std::fs::remove_dir_all(&d);

    // The same close where the live log file holds no frame: the file's
    // header is marked in place, no file is begun, and the run's name is
    // the policy's sync's to promise.
    let d = dir("name-inplace");
    let mut db = Db::create(
        &d,
        Options {
            sync: SyncPolicy::EveryN(4),
            adaptive_shape: false,
            ..Options::default()
        },
    )
    .unwrap();
    db.hold_seal_landing(true);
    ordered_batch(&mut db, 0);
    ordered_batch(&mut db, 1);
    let rotations = db.seal_waits().wal_rotations;
    db.put(&key(0), b"closes the run");
    db.commit().unwrap();
    assert_eq!(db.seal_waits().wal_rotations, rotations);
    assert_eq!(db.run_names_unsynced(), (None, 1));
    assert_eq!(db.seal_waits().run_names, 0);
    db.put(&key(1), b"the policy's fourth");
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 0));
    assert_eq!(
        db.seal_waits().run_names,
        1,
        "the policy's sync covered a closed run's file and not its name"
    );
    db.crash();
    let _ = std::fs::remove_dir_all(&d);

    // `sync` over a run the policy has synced nothing of.
    let d = dir("name-sync");
    let mut db = Db::create(
        &d,
        Options {
            adaptive_shape: false,
            ..buffered()
        },
    )
    .unwrap();
    db.hold_seal_landing(true);
    ordered_batch(&mut db, 0);
    assert_eq!(db.run_names_unsynced(), (Some(true), 0));
    db.sync().unwrap();
    assert_eq!(db.runs_unsynced(), (Some(false), 0));
    assert_eq!(
        db.run_names_unsynced(),
        (Some(false), 0),
        "`sync` returned over a run whose name nothing promises"
    );
    assert_eq!(db.seal_waits().run_names, 1);
    db.sync().unwrap();
    assert_eq!(db.seal_waits().run_names, 1);
    db.crash();
    let _ = std::fs::remove_dir_all(&d);

    // A store whose commits never sync owes the name to nothing: its
    // flush is answered behind the landing, whose own directory barrier
    // carries every name in it.
    let d = dir("name-never");
    let mut db = Db::create(&d, buffered()).unwrap();
    for b in 0..4 {
        ordered_batch(&mut db, b);
    }
    db.flush().unwrap();
    assert_eq!(db.seal_waits().run_names, 0);
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

/// The barrier's failure is the commit's: a commit whose run's name could
/// not be promised is not acknowledged, the next sync owes the barrier
/// still, and a run that closes owing it is the next sync's as a run that
/// closed unsynced is.
#[test]
fn a_barrier_that_fails_is_owed_until_it_is_made() {
    let d = dir("name-fails");
    let mut db = Db::create(
        &d,
        inline(Options {
            sync: SyncPolicy::Always,
            adaptive_shape: false,
            ..Options::default()
        }),
    )
    .unwrap();
    db.fail_next_dir_barrier();
    for j in 0..BATCH {
        db.append(&key(j), &VALUE);
    }
    let e = db
        .commit()
        .expect_err("the barrier failed and the commit was acknowledged");
    assert!(e.to_string().contains("injected"), "{e}");
    assert_eq!(db.run_names_unsynced(), (Some(true), 0));
    assert_eq!(db.seal_waits().run_names, 0);
    // The next commit of the run makes it.
    ordered_batch(&mut db, 1);
    assert_eq!(db.run_names_unsynced(), (Some(false), 0));
    assert_eq!(db.seal_waits().run_names, 1);
    db.flush().unwrap();

    // A run whose one commit's barrier failed, closed by a write into the
    // log: its file is synced and its name is not, and the commit into
    // the log does not return before the name is promised. Its seal is
    // held, so the run is under its temp name when the commit looks.
    db.hold_seal_landing(true);
    db.fail_next_dir_barrier();
    for j in 0..BATCH {
        db.append(&key(2 * BATCH + j), &VALUE);
    }
    db.commit()
        .expect_err("the barrier failed and the commit was acknowledged");
    assert_eq!(db.runs_unsynced(), (Some(false), 0));
    assert_eq!(db.run_names_unsynced(), (Some(true), 0));
    db.put(&key(0), b"closes the run");
    assert_eq!(
        db.run_names_unsynced(),
        (None, 1),
        "a run closed with its name unpromised and nothing owes it"
    );
    db.commit().unwrap();
    assert_eq!(db.runs_unsynced(), (None, 0));
    assert_eq!(db.run_names_unsynced(), (None, 0));
    assert_eq!(db.seal_waits().run_names, 2);
    db.crash();

    let db = Db::open(&d, Options::default()).unwrap();
    assert_eq!(values(&db, &key(0)), vec![b"closes the run".to_vec()]);
    assert_eq!(values(&db, &key(1)), vec![VALUE.to_vec()]);
    for b in 1..3 {
        assert!(holds(&db, b), "batch {b} is gone");
    }
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// A run's file from before runs took sequence numbers names no start, and
/// is taken as those were, wherever it stands: a store a release from
/// before was killed over opens with what that release would have found.
#[test]
fn a_run_that_names_no_start_is_taken_as_it_stands() {
    let d = dir("legacy");
    let mut db = Db::create(&d, buffered()).unwrap();
    db.hold_seal_landing(true);
    for b in 0..3 {
        ordered_batch(&mut db, b);
    }
    db.crash();
    // The note out of the superblock page, as a file from before has it.
    use std::os::unix::fs::FileExt;
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(run_file(&d, 0))
        .unwrap();
    f.write_all_at(&[0u8; 24], 2048).unwrap();
    drop(f);
    let db = Db::open(&d, buffered()).unwrap();
    for b in 0..3 {
        assert!(holds(&db, b), "batch {b} of a run from before");
    }
    assert!(
        names(&d, "cov-").is_empty(),
        "a run with no numbers was noted"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------- the oracle --

/// The stream the oracle's writer commits: a function of a seed and of the
/// commits before, so that the parent makes the same stream again and asks
/// which prefix of it the reopened store is. Runs of ordered ingest, writes
/// into the log over the keys already there, a batch that begins as a run
/// and ends in the log, and seals between them, so that a store is killed
/// with runs open, runs closed and unsealed, runs landed, and frames on
/// every side of them.
struct Stream {
    rng: u64,
    /// Keys `0..next` have been written; the next run starts here.
    next: u64,
}

enum Op {
    Append(Vec<u8>, Vec<u8>),
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

impl Stream {
    fn new(seed: u64) -> Stream {
        Stream {
            rng: seed | 1,
            next: 0,
        }
    }

    fn draw(&mut self, n: u64) -> u64 {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 33) % n
    }

    fn value(&mut self, commit: u64, j: u64) -> Vec<u8> {
        let mut v = format!("c{commit}-{j}-").into_bytes();
        let pad = 40 + self.draw(80) as usize;
        v.resize(v.len() + pad, b'.');
        v
    }

    fn ordered(&mut self, commit: u64, ops: &mut Vec<Op>) {
        let n = 1 + self.draw(300);
        for j in 0..n {
            let v = self.value(commit, j);
            ops.push(Op::Append(key(self.next + j), v));
        }
        self.next += n;
    }

    fn logged(&mut self, commit: u64, ops: &mut Vec<Op>) {
        if self.next == 0 {
            return self.ordered(commit, ops);
        }
        for j in 0..1 + self.draw(8) {
            let k = key(self.draw(self.next));
            let v = self.value(commit, 1000 + j);
            ops.push(match self.draw(5) {
                0 => Op::Delete(k),
                1 => Op::Append(k, v),
                _ => Op::Put(k, v),
            });
        }
    }

    /// Commit `commit`'s writes, and whether a seal follows it.
    fn commit(&mut self, commit: u64) -> (Vec<Op>, bool) {
        let mut ops = Vec::new();
        match self.draw(10) {
            0..=4 => self.ordered(commit, &mut ops),
            5..=7 => self.logged(commit, &mut ops),
            8 => {
                // A run's batch that ends in the log: the run's committed
                // records close as a segment and the batch's own go to the
                // log behind them.
                self.ordered(commit, &mut ops);
                self.logged(commit, &mut ops);
            }
            _ => {
                self.logged(commit, &mut ops);
                return (ops, true);
            }
        }
        (ops, false)
    }
}

type Model = std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

fn apply(m: &mut Model, ops: &[Op]) {
    for op in ops {
        match op {
            Op::Append(k, v) => m.entry(k.clone()).or_default().push(v.clone()),
            Op::Put(k, v) => {
                m.insert(k.clone(), vec![v.clone()]);
            }
            Op::Delete(k) => {
                m.remove(k);
            }
        }
    }
}

const ORACLE_ENV: &str = "SUPDB_RUNLOG_CHILD";
const ORACLE_TEST: &str = "a_torn_store_is_a_prefix_of_what_was_committed";

fn oracle_options(sync: SyncPolicy) -> Options {
    Options {
        sync,
        // Small seals: a run closes, a table seals and a merge starts
        // every few commits.
        seal_bytes: 96 << 10,
        ..Options::default()
    }
}

/// The oracle's writer: the stream committed without end, each commit
/// acknowledged on stdout. Killed from outside; never returns.
fn oracle_child(spec: &str) -> ! {
    use std::io::Write;
    let (seed, rest) = spec.split_once(':').unwrap();
    let (sync, d) = rest.split_once(':').unwrap();
    let sync = match sync {
        "always" => SyncPolicy::Always,
        _ => SyncPolicy::EveryN(u32::MAX),
    };
    let mut db = Db::create(Path::new(d), oracle_options(sync)).unwrap();
    let mut s = Stream::new(seed.parse().unwrap());
    let out = std::io::stdout();
    for i in 0.. {
        let (ops, seal) = s.commit(i);
        for op in &ops {
            match op {
                Op::Append(k, v) => db.append(k, v),
                Op::Put(k, v) => db.put(k, v),
                Op::Delete(k) => db.delete(k),
            }
        }
        db.commit().unwrap();
        if matches!(sync, SyncPolicy::Always) {
            // What a commit under this policy acknowledges is everything
            // so far, on whatever path the stream took to it: no run's
            // file with a commit unsynced, and no run's temp name that a
            // crash could take.
            let (run, loose) = db.runs_unsynced();
            let (name, names) = db.run_names_unsynced();
            if run == Some(true) || loose != 0 || name == Some(true) || names != 0 {
                let mut o = out.lock();
                writeln!(
                    o,
                    "commit {i} left unsynced: run {run:?}, loose runs {loose}, \
                     the run's name {name:?}, loose runs' names {names}"
                )
                .unwrap();
                o.flush().unwrap();
                std::process::exit(1);
            }
        }
        let mut o = out.lock();
        writeln!(o, "{i}").unwrap();
        o.flush().unwrap();
        drop(o);
        if seal {
            db.seal().unwrap();
        }
    }
    unreachable!()
}

/// What a machine's crash may do to the files of a store whose writer
/// synced nothing at its commits: every log -- the WAL's files, and each
/// run not yet a named segment -- cut back to some length, since nothing
/// promised the device any of it, and a run's file gone altogether, since
/// nothing promised its name either. A run's temp name that shares its
/// file with a finished segment is left alone: that file was synced whole
/// before anything named it. What the choices were comes back for a
/// failure's message.
#[cfg(target_os = "linux")]
fn tear(d: &Path, rng: &mut Stream) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let mut did = Vec::new();
    for name in names(d, "wal-").into_iter().chain(names(d, "direct-")) {
        let p = d.join(&name);
        let md = std::fs::metadata(&p).unwrap();
        let run = name.starts_with("direct-");
        if run && md.nlink() > 1 {
            continue;
        }
        match rng.draw(if run { 5 } else { 4 }) {
            0 | 1 => {}
            4 => {
                std::fs::remove_file(&p).unwrap();
                did.push(format!("{name} gone"));
            }
            _ => {
                let to = rng.draw(md.len() + 1);
                let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
                f.set_len(to).unwrap();
                did.push(format!("{name} cut from {} to {to}", md.len()));
            }
        }
    }
    did
}

/// The property the tests above hold one state at a time, held over
/// whatever states a killed writer leaves: the store that reopens is the
/// stream's first `p` commits for some `p`, every key with its values in
/// their order and no key beyond them. With nothing torn, `p` is every
/// commit acknowledged, under either policy -- a process's death takes
/// nothing. With the logs torn as a machine's crash may tear them under a
/// policy that syncs no commit, `p` is anything, and it is still a prefix.
#[cfg(target_os = "linux")]
#[test]
fn a_torn_store_is_a_prefix_of_what_was_committed() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    if let Ok(spec) = std::env::var(ORACLE_ENV) {
        oracle_child(&spec);
    }
    let exe = std::env::current_exe().unwrap();
    let mut pick = Stream::new(0x0BAD_5EED);
    for round in 0..24u64 {
        let seed = 1000 + round;
        // One round in four under a policy that syncs every commit, and
        // one in four of the others with nothing torn: those lose nothing.
        let always = round % 4 == 3;
        let torn = !always && round % 4 != 0;
        let d = dir(&format!("oracle-{round}"));
        let sync = if always { "always" } else { "never" };
        let mut c = Command::new(&exe)
            .args(["--exact", ORACLE_TEST, "--nocapture", "--test-threads=1"])
            .env(ORACLE_ENV, format!("{seed}:{sync}:{}", d.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(c.stdout.take().unwrap()).lines();
        let want = 20 + pick.draw(if always { 60 } else { 400 });
        let mut last: Option<u64> = None;
        let mut acked = 0;
        let mut said = String::new();
        while acked < want {
            match lines.next() {
                Some(Ok(l)) => {
                    if let Ok(n) = l.trim().parse::<u64>() {
                        last = Some(n);
                        acked += 1;
                    } else if l.starts_with("commit ") {
                        said = l;
                    }
                }
                _ => panic!("round {round}: the writer ended before {want} commits: {said}"),
            }
        }
        c.kill().unwrap();
        let _ = c.wait();
        for l in lines.map_while(Result::ok) {
            if let Ok(n) = l.trim().parse::<u64>() {
                last = Some(n);
            }
        }
        let last = last.expect("no commit acknowledged");
        let did = if torn {
            tear(&d, &mut pick)
        } else {
            Vec::new()
        };
        let what = format!(
            "round {round}, seed {seed}, {sync}, last acknowledged {last}, torn: {did:?}, store {}",
            d.display()
        );

        let db = match Db::open(&d, oracle_options(SyncPolicy::EveryN(u32::MAX))) {
            Ok(db) => db,
            Err(e) => panic!("{what}: the store does not open: {e}"),
        };
        let mut got = Model::new();
        db.scan(
            b"",
            usize::MAX,
            common::kv(&mut |k, v| got.entry(k.to_vec()).or_default().push(v.to_vec())),
        )
        .unwrap();
        // The prefix it is: the stream made again, a commit at a time. The
        // writer may have been inside the commit after the last it
        // acknowledged, and that one may be there whole.
        let mut s = Stream::new(seed);
        let mut model = Model::new();
        let mut prefix = None;
        for p in 0..=last + 2 {
            if model == got {
                prefix = Some(p);
            }
            let (ops, _) = s.commit(p);
            apply(&mut model, &ops);
        }
        let Some(p) = prefix else {
            panic!(
                "{what}: the store, {} keys, is no prefix of the commits",
                got.len()
            );
        };
        if !torn {
            assert!(
                p > last,
                "{what}: nothing was torn and the store is the first {p} commits"
            );
        }
        // The point reads agree with the scan on a sample of it.
        for (k, v) in got.iter().step_by(1 + got.len() / 64) {
            assert_eq!(
                &values(&db, k),
                v,
                "{what}: a point read of a key the scan holds"
            );
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}
