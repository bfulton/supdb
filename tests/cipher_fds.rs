//! A store under a cipher larger than its cache, read as itself within
//! it, in a binary of its own: the test holds the process's descriptors
//! to the store's (`/proc/self/fd`), a count of the whole process, and in
//! `tests/cipher.rs` beside other tests' stores it counted theirs -- an
//! envelope of another test's store, and memory files no path tells apart
//! (`CLAUDE.md`: a test that counts a process-wide effect gets a binary of
//! its own).
//!
//! Linux only, as the store under a cipher is: elsewhere the binary holds
//! no test.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use supdb::{Db, Encryption, Options, StoreCipher};

mod common;
use common::{kv, vf, Gcm};

const MARK: &str = "IN-THE-CLEAR";

/// What every key begins with, as in `tests/cipher.rs`.
const KEY_MARK: &str = "k-CLR-";

fn cipher(n: u8) -> StoreCipher {
    StoreCipher::new(Gcm::new(&[n; 32]), Gcm::new).key_id(u64::from(n))
}

/// The engine's options as the tests' switches have them: the store's
/// envelopes files of its data file under `SUPDB_TEST_ONEFILE=1`
/// (`Options::one_file`), and files of the directory otherwise.
fn base() -> Options {
    Options {
        one_file: common::one_file_switch(),
        ..Options::default()
    }
}

/// Seals and partitions of a quarter megabyte, so that a test's few
/// megabytes leave a log, pieces, several merged partitions -- whose names
/// carry their fences -- and their indexes behind.
fn small() -> Options {
    Options {
        seal_bytes: 256 << 10,
        partition_bytes: Some(256 << 10),
        l0_trigger: 2,
        ..base()
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-cipher-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn k(i: u32) -> Vec<u8> {
    format!("{KEY_MARK}{i:08}").into_bytes()
}

fn v(i: u32, round: u32) -> Vec<u8> {
    format!(
        "value {i} of round {round}, {MARK}, {}",
        "x".repeat((i % 50) as usize)
    )
    .into_bytes()
}

/// A few megabytes through every way the store writes: keys in order, the
/// same keys again out of order, and enough of both for seals and the
/// merges behind them.
fn fill(db: &mut Db, n: u32) {
    for i in 0..n {
        db.append(&k(i), &v(i, 0));
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    for i in (0..n).rev().step_by(3) {
        db.put(&k(i), &v(i, 1));
        if i % 500 < 3 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
}

/// What `fill` left, read back: every key's value, and a scan of them all.
fn holds(db: &Db, n: u32) {
    for i in (0..n).step_by(7) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        let round = u32::from((n - 1 - i).is_multiple_of(3));
        assert_eq!(got, [v(i, round)], "key {i}");
    }
    let mut seen = 0u32;
    db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
        .unwrap();
    assert_eq!(seen, n);
}

/// The plaintext a directory's files hold under their envelopes: what a
/// store there would have to hold to hold itself whole.
fn stored_bytes(dir: &Path) -> u64 {
    names(dir)
        .iter()
        .filter(|n| n.starts_with("x-"))
        .map(|n| len_of(dir, n))
        .sum()
}

/// The names of the store's files wherever it keeps them: the directory's
/// own and, for a store kept in one file that is open there, the
/// envelopes its data file holds (`common::store_names`). A closed store
/// kept in one file has the directory's alone, its envelopes' names being
/// in its sealed manifest, so a test that reads them reads them while the
/// store is open.
fn names(dir: &Path) -> Vec<String> {
    common::store_names(dir)
}

/// The length of the store's file `name`, wherever it lies.
fn len_of(dir: &Path, name: &str) -> u64 {
    common::store_len(dir.join(name))
}

/// A store several times the size of the plaintext it may hold reads as
/// itself: every key, a scan of the whole of it in one call, reads from
/// four threads at once beside a writer, and all of it again after a
/// reopen. What it holds of itself stays at its cache's size, give or take
/// what the reads in flight are lent, through all of it; an open decrypts
/// what an open reads and not the store; and the store holds a descriptor
/// for none of its files.
#[test]
fn a_store_larger_than_its_cache_reads_as_itself_within_it() {
    let n = 150_000;
    let cache: u64 = 2 << 20;
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(9).cache_bytes(cache)),
        ..small()
    };
    let dir = scratch("larger");
    // What a read in flight may hold beyond the budget: a megabyte is what
    // the cache gives back at a time, and a scan or a merge rests every
    // thousand keys or so.
    let slack: u64 = 8 << 20;
    let settled = |db: &Db, what: &str| {
        let t = std::time::Instant::now();
        loop {
            let c = db.cipher_counts().unwrap().cache;
            if c.held <= c.budget {
                return c;
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "{what}: the cache stayed over its budget with nothing reading: {c:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    {
        let mut db = Db::create(&dir, opts()).unwrap();
        fill(&mut db, n);
        let size = stored_bytes(&dir);
        assert!(
            size > 6 * cache,
            "the store is {size} bytes, not several times its cache of {cache}"
        );
        holds(&db, n);
        let c = settled(&db, "after the fill");
        assert_eq!(c.budget, cache);
        assert!(c.evicted_units > 0, "nothing was given back: {c:?}");
        assert!(
            c.peak < cache + slack,
            "the cache held {} bytes of a budget of {cache}: {c:?}",
            c.peak
        );
        // Reads from four threads, each through a handle of its own, far
        // apart in the store, beside a writer rewriting keys and flushing.
        let handles: Vec<_> = (0..4).map(|_| db.reader().unwrap()).collect();
        std::thread::scope(|scope| {
            for (t, h) in handles.into_iter().enumerate() {
                scope.spawn(move || {
                    let mut s = 0x9e37_79b9u64 ^ ((t as u64) << 20) | 1;
                    for _ in 0..4_000 {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        // The keys the writer below leaves alone.
                        let i = (s % u64::from(n / 2)) as u32 * 2 + 1;
                        let mut got: Vec<Vec<u8>> = Vec::new();
                        h.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
                            .unwrap();
                        let round = u32::from((n - 1 - i).is_multiple_of(3));
                        assert_eq!(got, [v(i, round)], "thread {t}, key {i}");
                        if s.is_multiple_of(97) {
                            let mut seen = 0u32;
                            h.scan(&k(i), 3000, kv(&mut |_k, _v| seen += 1)).unwrap();
                            assert!(seen > 0);
                        }
                    }
                });
            }
            for round in 0..3u32 {
                for i in (0..n).step_by(2).skip(round as usize).step_by(40) {
                    db.put(&k(i), &v(i, 7 + round));
                }
                db.commit().unwrap();
                db.flush().unwrap();
            }
        });
        db.settle().unwrap();
        let c = settled(&db, "after the threads");
        // Under four readers, a writer and the merges behind it the bound
        // is the line a caller's read waits at, six megabytes past a
        // budget this small, and what each of them reads between two
        // looks at it: held to the eight megabytes one reader is, the
        // peak sat within a megabyte of the bound and went half a
        // megabyte over it in one run of the file in twenty.
        assert!(
            c.peak < cache + slack + (2 << 20),
            "the cache held {} bytes of a budget of {cache} under four readers: {c:?}",
            c.peak
        );
        // No descriptor of the process is a file of the store's: a memory
        // file's is closed when the file has its envelope, and an
        // envelope's when it is mapped.
        let fds: Vec<String> = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| p.contains("memfd:supdb") || p.contains("/x-"))
            .collect();
        assert!(
            fds.len() <= 4,
            "the store holds descriptors for its files: {fds:?}"
        );
    }
    // Reopened: what the open decrypts is what an open reads of each file
    // -- a header, the first key and the last, the block table's and the
    // index's first words, a handful of chunks whatever the file's length
    // -- and not the store. The partitions here are a quarter megabyte,
    // sixty chunks, so the bound is by the file and not by the byte.
    let db = Db::open(&dir, opts()).unwrap();
    let c = db.cipher_counts().unwrap().cache;
    let files = names(&dir).iter().filter(|n| n.starts_with("x-")).count() as u64;
    assert!(
        c.fills <= 6 * files,
        "the open decrypted {} chunks of {files} files: {c:?}",
        c.fills
    );
    assert!(
        c.fills * 4080 < stored_bytes(&dir) / 4,
        "the open decrypted {} chunks of a store of {} bytes: {c:?}",
        c.fills,
        stored_bytes(&dir)
    );
    for i in (1..n).step_by(2).step_by(13) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        let round = u32::from((n - 1 - i).is_multiple_of(3));
        assert_eq!(got, [v(i, round)], "key {i} after the reopen");
    }
    let mut seen = 0u32;
    db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
        .unwrap();
    assert_eq!(seen, n);
    let c = settled(&db, "after the reopen");
    assert!(c.evicted_units > 0, "{c:?}");
    assert!(
        c.peak < cache + slack,
        "the cache held {} bytes of a budget of {cache} after the reopen: {c:?}",
        c.peak
    );
}
