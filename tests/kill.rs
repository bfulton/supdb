//! A writer killed by the kernel mid-stream: every batch whose commit it
//! acknowledged is in the store it left, the one batch it may have been
//! inside is whole or absent, and nothing beyond it exists. This is what
//! "durable" is held to -- process death, with the device's word taken for
//! what it synced; power loss, where a device may lie about its sync, needs
//! a rig that pulls the plug and is out of scope here (`docs/engine.md`).
//!
//! The test runs itself as the child: the test binary, told through the
//! environment where the store is, opens it durable (`SyncPolicy::Always`,
//! the default), commits batches of a hundred keys without end and prints
//! each batch's number once its commit has returned. The parent reads the
//! stream, kills the child with SIGKILL after a random count of
//! acknowledgements, drains what the child printed before it died, reopens
//! the store and checks it against the stream. Ten rounds a run.

// A child killed with SIGKILL and its store read back: run on Linux
// alone, so everything here is, the helpers with the tests. They were not,
// and on another platform every one of them was code nothing used, which
// the lint refuses.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use supdb::{Db, Options, SyncPolicy};

mod common;
use common::vf;

const ENV: &str = "SUPDB_KILL_CHILD";
const BATCH: u32 = 100;
const ROUNDS: u32 = 10;
const TEST: &str = "a_killed_writers_acknowledged_batches_are_all_there";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-kill-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(batch: u32, j: u32) -> Vec<u8> {
    format!("b{batch:08}-{j:04}").into_bytes()
}

/// The child: a durable store, batches committed without end, each
/// acknowledged on stdout once its commit has returned. Killed from
/// outside; never returns.
fn child(dir: &Path) -> ! {
    let mut db = Db::create(dir, Options::default()).unwrap();
    let out = std::io::stdout();
    let mut i = 0u32;
    loop {
        for j in 0..BATCH {
            db.put(&key(i, j), &i.to_le_bytes());
        }
        db.commit().unwrap();
        let mut o = out.lock();
        writeln!(o, "{i}").unwrap();
        o.flush().unwrap();
        i += 1;
    }
}

/// The values a key holds in the reopened store.
fn values(db: &Db, k: &[u8]) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    db.read_all(k, vf(&mut |v| got.push(v.to_vec()))).unwrap();
    got
}

#[test]
fn a_killed_writers_acknowledged_batches_are_all_there() {
    if let Ok(d) = std::env::var(ENV) {
        child(Path::new(&d));
    }
    let exe = std::env::current_exe().unwrap();
    let mut seed = 0x5EED_u64;
    for round in 0..ROUNDS {
        let d = dir(&format!("{round}"));
        let mut c = Command::new(&exe)
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(ENV, &d)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(c.stdout.take().unwrap()).lines();
        // Killed after a random count of acknowledgements, five to sixty, so
        // the kill falls at a different point of a commit each round. The
        // harness prints its own lines on the same stream; a batch's
        // acknowledgement is a bare number.
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let want = 5 + (seed >> 33) % 56;
        let mut acked = 0u64;
        let mut last: Option<u32> = None;
        while acked < want {
            match lines.next() {
                Some(Ok(l)) => {
                    if let Ok(n) = l.trim().parse::<u32>() {
                        last = Some(n);
                        acked += 1;
                    }
                }
                _ => panic!("round {round}: the child ended before {want} acknowledgements"),
            }
        }
        c.kill().unwrap();
        let _ = c.wait();
        // What the child acknowledged before it died, to the last line it
        // got out: the parent's count was where it stopped reading, not
        // where the child stopped committing.
        for l in lines.map_while(Result::ok) {
            if let Ok(n) = l.trim().parse::<u32>() {
                last = Some(n);
            }
        }
        let last = last.expect("no batch acknowledged");
        let db = Db::open(&d, Options::default()).unwrap();
        for i in 0..=last {
            for j in 0..BATCH {
                assert_eq!(
                    values(&db, &key(i, j)),
                    vec![i.to_le_bytes().to_vec()],
                    "round {round}: batch {i}, acknowledged (last {last}), is not there as written"
                );
            }
        }
        // The batch the child may have been inside: whole or absent.
        let present = (0..BATCH)
            .filter(|&j| !values(&db, &key(last + 1, j)).is_empty())
            .count();
        assert!(
            present == 0 || present == BATCH as usize,
            "round {round}: batch {} is half there, {present} of {BATCH} keys",
            last + 1
        );
        // And nothing beyond it: the child acknowledges a batch before it
        // begins the next, and every acknowledgement it got out was read.
        for i in last + 2..last + 4 {
            for j in 0..BATCH {
                assert!(
                    values(&db, &key(i, j)).is_empty(),
                    "round {round}: batch {i}, never begun, has keys (last acknowledged {last})"
                );
            }
        }
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }
}

const BUFFERED_ENV: &str = "SUPDB_KILL_BUFFERED_CHILD";
const BUFFERED_TEST: &str = "a_killed_buffered_writer_loses_nothing_it_acknowledged";
/// The buffered child's batch and value: the suite's load, a thousand keys
/// of a hundred bytes, so that a run of ordered ingest closes after about
/// three hundred batches under the default seal and a megabyte, the
/// writer's piece, is about eight.
const BUFFERED_BATCH: u32 = 1_000;
const BUFFERED_VALUE: [u8; 100] = [0x5a; 100];

fn buffered_key(i: u32) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

fn buffered_options() -> Options {
    Options {
        sync: SyncPolicy::EveryN(u32::MAX),
        ..Options::default()
    }
}

/// The buffered child: keys in order, so every batch goes to a run of
/// ordered ingest and none to the log, committed under a policy that syncs
/// nothing at a commit, each acknowledged on stdout. Every seal is held
/// before its thread does anything, so a run that closes is still as the
/// writer left it when the kill comes, however long after.
fn buffered_child(dir: &Path) -> ! {
    let mut db = Db::create(dir, buffered_options()).unwrap();
    db.hold_seal_landing(true);
    let out = std::io::stdout();
    let mut i = 0u32;
    loop {
        for j in 0..BUFFERED_BATCH {
            db.append(&buffered_key(i * BUFFERED_BATCH + j), &BUFFERED_VALUE);
        }
        db.commit().unwrap();
        let mut o = out.lock();
        writeln!(o, "{i}").unwrap();
        o.flush().unwrap();
        i += 1;
    }
}

/// A writer that syncs nothing at its commits promises the device nothing,
/// and promises the kernel everything: a commit that returned is written
/// where it goes under every policy (`SyncPolicy`), so the death of the
/// process takes none of them, whichever of the log and a run's file they
/// went to.
///
/// Two things were found here, both by the benchmark's recovery workload.
/// A run of ordered ingest went to its file a write piece at a time, so up
/// to a megabyte of acknowledged batches was in the writer's buffer when a
/// kill came: two to eight batches of the suite's load gone where LMDB and
/// RocksDB without sync lost none. And a run that closed was handed to its
/// seal with that last piece still in the buffer, for the seal's thread to
/// write with the trailer, while the writer went on into the next run,
/// whose first whole piece reached its own file first: a kill between the
/// two left batch 290 there and 287 to 289 not. Every commit writes its
/// batch now. Here the seal's thread is held, so the second window is as
/// wide as the test needs, and the kill comes once the second run has
/// pieces on its file.
#[test]
fn a_killed_buffered_writer_loses_nothing_it_acknowledged() {
    if let Ok(d) = std::env::var(BUFFERED_ENV) {
        buffered_child(Path::new(&d));
    }
    let exe = std::env::current_exe().unwrap();
    let d = dir("buffered");
    let mut c = Command::new(&exe)
        .args(["--exact", BUFFERED_TEST, "--nocapture", "--test-threads=1"])
        .env(BUFFERED_ENV, &d)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(c.stdout.take().unwrap()).lines();
    // Killed once a second run has written two megabytes behind the first
    // one's close: the files say when, since where a run closes is the
    // seal threshold's to decide.
    let second = d.join("direct-00000001.tmp");
    let mut last: Option<u32> = None;
    let t = std::time::Instant::now();
    while std::fs::metadata(&second).map_or(0, |m| m.len()) < 2 << 20 {
        assert!(
            t.elapsed().as_secs() < 120,
            "the child never wrote a second run (last acknowledged {last:?})"
        );
        match lines.next() {
            Some(Ok(l)) => {
                if let Ok(n) = l.trim().parse::<u32>() {
                    last = Some(n);
                }
            }
            _ => panic!("the child ended before its second run (last acknowledged {last:?})"),
        }
    }
    c.kill().unwrap();
    let _ = c.wait();
    for l in lines.map_while(Result::ok) {
        if let Ok(n) = l.trim().parse::<u32>() {
            last = Some(n);
        }
    }
    let last = last.expect("no batch acknowledged");
    // The premise: the first run closed and was not finished -- it is
    // still under its temp name, with no segment beside it -- and the
    // second holds records.
    let first = d.join("direct-00000000.tmp");
    assert!(first.exists(), "the first run's file is gone");
    let segments = std::fs::read_dir(&d)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".sup")
        })
        .count();
    assert_eq!(segments, 0, "a seal landed, and every one was held");

    let db = Db::open(&d, buffered_options()).unwrap();
    // Each batch by every key: whole, or absent.
    let held = |i: u32| -> Option<bool> {
        let n = (0..BUFFERED_BATCH)
            .filter(|&j| {
                let v = values(&db, &buffered_key(i * BUFFERED_BATCH + j));
                assert!(
                    v.is_empty() || v == vec![BUFFERED_VALUE.to_vec()],
                    "batch {i}, key {j}: not the value written"
                );
                !v.is_empty()
            })
            .count() as u32;
        match n {
            0 => Some(false),
            BUFFERED_BATCH => Some(true),
            _ => None,
        }
    };
    let mut first_lost: Option<u32> = None;
    for i in 0..=last + 1 {
        match (held(i), first_lost) {
            (None, _) => panic!("batch {i} is there in part (last acknowledged {last})"),
            (Some(true), Some(lost)) => panic!(
                "batch {i} is there and batch {lost}, acknowledged before it, is not \
                 (last acknowledged {last})"
            ),
            (Some(false), None) => first_lost = Some(i),
            _ => {}
        }
    }
    // Nothing acknowledged is gone: the first batch not there, if any, is
    // the one the child was inside when it died.
    assert!(
        first_lost.is_none_or(|lost| lost > last),
        "batch {} was acknowledged and is not there (last acknowledged {last})",
        first_lost.unwrap_or(0)
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}
