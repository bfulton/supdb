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

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use supdb::{Db, Options};

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

#[cfg(target_os = "linux")]
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
