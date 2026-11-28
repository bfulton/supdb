//! A store under a cipher the caller supplies (`Encryption::Cipher`,
//! `supdb::db::vault`), under a real one.
//!
//! `src/db/vault.rs` holds the layer's own pieces to its unit tests, under a
//! stand-in cipher: the memory files, the envelopes, the sealed manifest and
//! the sealed log. What is held here is the store: that it reads as a plain
//! store reads, that nothing it was given is in its directory in the clear,
//! names included, that it refuses what is not it, that a byte changed
//! anywhere is refused or is the log's end, and that what a crash leaves
//! opens.
//!
//! Linux only, as the store under a cipher is: elsewhere the binary holds
//! no test.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use supdb::{Db, Encryption, Options, StoreCipher, Upkeep};

mod common;
use common::{kv, vf, Gcm};

fn cipher(n: u8) -> StoreCipher {
    StoreCipher::new(Gcm::new(&[n; 32]), Gcm::new).key_id(u64::from(n))
}

fn under(n: u8) -> Options {
    Options {
        encryption: Encryption::Cipher(cipher(n)),
        ..small()
    }
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

/// One at a time, for the tests that hold a cache to its budget and a
/// slack: what the cache holds past its budget is what its readers read
/// while the thread that gives back was off its core, and four of these
/// at once on four cores are each other's reason for that. Alone the
/// larger store's peak sat under its bound in every run, and with three
/// more of its kind beside it, 3% over in one.
fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static QUIET: std::sync::Mutex<()> = std::sync::Mutex::new(());
    QUIET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-cipher-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The names in `dir` itself: what a search of the device reads, and what
/// a copy of the directory is made of.
fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
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

/// The store's files whose names `pick` takes, each with the file of the
/// directory its bytes lie in and its range there: itself from its start
/// in a store of files, and a range of the data file in a store kept in
/// one, which has to be open for the asking. For a test that changes a
/// byte of one of them.
fn places(dir: &Path, pick: impl Fn(&str) -> bool) -> Vec<(String, String, u64, u64)> {
    names(dir)
        .into_iter()
        .filter(|n| pick(n))
        .map(|n| {
            let (file, at, len) = common::store_place(dir.join(&n));
            let file = file.file_name().unwrap().to_string_lossy().into_owned();
            (n, file, at, len)
        })
        .collect()
}

const MARK: &str = "IN-THE-CLEAR";

/// What every key begins with: short, so that the keys differ inside the
/// sixteen bytes a partition's fence is cut to and a plain store's
/// partitions are named by them.
const KEY_MARK: &str = "k-CLR-";

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

/// A value of one width for every key: what the stride form takes, with
/// the numbered keys `k` renders.
fn w(i: u32) -> Vec<u8> {
    format!("value {i:08}, {MARK}, one width").into_bytes()
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

/// Every file in `dir` that holds `needle`, by name.
fn files_holding(dir: &Path, needle: &[u8]) -> Vec<String> {
    dir_names(dir)
        .into_iter()
        .filter(|n| {
            let bytes = std::fs::read(dir.join(n)).unwrap();
            bytes.windows(needle.len()).any(|w| w == needle)
        })
        .collect()
}

/// A store under a cipher reads as itself, before and after it is
/// reopened, and nothing it was given is in its directory: not a key, not a
/// value, and not in a file's name, where a plain store's partitions carry
/// their fences. The search has a control, the same store in the clear,
/// where it has to find all three.
#[test]
fn a_store_under_a_cipher_reads_as_itself_and_is_ciphertext_in_its_directory() {
    let n = 30_000;
    let dir = scratch("reads");
    {
        let mut db = Db::create(&dir, under(1)).unwrap();
        fill(&mut db, n);
        holds(&db, n);
        let c = db.cipher_counts().unwrap();
        assert!(c.persisted > 2, "segments reached the directory: {c:?}");
        // Kept as the tests' switch says: in the store's data file, with
        // envelopes there to count, or as files of the directory.
        assert_eq!(
            db.one_counts().is_some_and(|c| c.files > 2),
            common::one_file_switch(),
            "{:?}",
            db.one_counts()
        );
        assert_eq!(
            c.cache.fills, 0,
            "and none was read back from it: the store fits its cache: {c:?}"
        );
    }
    for needle in [MARK, KEY_MARK] {
        assert!(
            files_holding(&dir, needle.as_bytes()).is_empty(),
            "{needle} in {:?}",
            files_holding(&dir, needle.as_bytes())
        );
    }
    let fence: String = KEY_MARK.bytes().map(|b| format!("{b:02x}")).collect();
    for name in names(&dir) {
        let known = name == "supdb.cipher"
            // The store lock: empty, and named alike in every store.
            || name == "LOCK"
            || name == "manifest"
            || name == "manifest.alt"
            // The data file of a store kept in one (`Options::one_file`),
            // which holds the envelopes there.
            || (name == "store.one" && common::one_file_switch())
            || (name.starts_with("wal-") && name[4..].bytes().all(|b| b.is_ascii_digit()))
            // A retired log kept to be the next one (`Options::recycle_wal`):
            // a sealed log under a number, as a live one is.
            || (name.starts_with("spare-") && name[6..].bytes().all(|b| b.is_ascii_digit()))
            || (name.starts_with("x-")
                && name[2..].trim_end_matches(".o").len() == 32
                && name[2..]
                    .trim_end_matches(".o")
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit()));
        assert!(known, "a file named {name}");
        assert!(!name.contains(&fence), "{name} carries a fence");
    }
    {
        let db = Db::open(&dir, under(1)).unwrap();
        holds(&db, n);
        let c = db.cipher_counts().unwrap();
        assert!(c.cache.fills > 0, "what was read was decrypted: {c:?}");
        assert_eq!(c.swept, 0, "a clean close leaves nothing to sweep");
    }
    // The control: in the clear, the same search finds the keys and the
    // values in the files and a fence in a name.
    let plain = scratch("reads-plain");
    {
        // A store of files, whose partitions' names are the directory's.
        let mut db = Db::create(&plain, common::files(small())).unwrap();
        fill(&mut db, n);
    }
    for needle in [MARK, KEY_MARK] {
        assert!(!files_holding(&plain, needle.as_bytes()).is_empty());
    }
    assert!(
        names(&plain).iter().any(|name| name.contains(&fence)),
        "no fence {fence} in {:?}",
        names(&plain)
    );
}

/// A directory's files, each by name with its bytes.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    dir_names(dir)
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(dir.join(&n)).unwrap();
            (n, bytes)
        })
        .collect()
}

/// `dir` made to hold exactly `files`.
fn restore(dir: &Path, files: &[(String, Vec<u8>)]) {
    for n in dir_names(dir) {
        std::fs::remove_file(dir.join(n)).unwrap();
    }
    for (n, bytes) in files {
        std::fs::write(dir.join(n), bytes).unwrap();
    }
}

/// A store under a cipher opens under that cipher and no other way, and a
/// cipher opens no store that is not under it; each refusal leaves the
/// directory as it found it, byte for byte, and the store opens after
/// them all.
#[test]
fn a_store_under_a_cipher_opens_under_no_other() {
    let n = 4_000;
    let dir = scratch("refusals");
    {
        let mut db = Db::create(&dir, under(1)).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&dir);
    let refused = |r: std::io::Result<Db>, what: &str| {
        let e = r.err().unwrap_or_else(|| panic!("{what} was not refused"));
        assert_eq!(snapshot(&dir), before, "{what} changed the directory");
        e.to_string()
    };
    let e = refused(Db::open(&dir, small()), "an open with no cipher");
    assert!(e.contains("cipher"), "{e}");
    let e = refused(Db::open(&dir, under(2)), "an open under another key");
    assert!(e.contains("key is wrong"), "{e}");
    refused(Db::create(&dir, small()), "a plain store made over it");
    refused(Db::create(&dir, under(1)), "a second store made over it");
    assert_eq!(supdb::cipher_key_id(&dir).unwrap(), Some(1));
    holds(&Db::open(&dir, under(1)).unwrap(), n);

    // And the other way: a cipher given to a store in the clear.
    let plain = scratch("refusals-plain");
    {
        let mut db = Db::create(&plain, small()).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&plain);
    let e = Db::open(&plain, under(1))
        .err()
        .expect("a cipher on a plain store")
        .to_string();
    assert!(e.contains("in the clear"), "{e}");
    assert!(Db::create(&plain, under(1)).is_err());
    assert_eq!(snapshot(&plain), before);
    assert_eq!(supdb::cipher_key_id(&plain).unwrap(), None);
    holds(&Db::open(&plain, small()).unwrap(), n);
}

/// An unpinned writer is refused under a cipher, before the directory
/// is touched. `writer_pins` off is sound only while nothing frees what
/// the writer reads, which is why it is refused beside a second
/// publisher; under a cipher the store's cache gives a file's pages back
/// once no pinned reader could hold them, and a writer that pins nothing
/// is a reader it does not wait for. The refusal is held to its words
/// and to a directory left as it was: one that was not there is not
/// made, and a store is opened under its pinned writer after an open
/// that was refused.
#[test]
fn an_unpinned_writer_is_refused_under_a_cipher() {
    let unpinned = |n: u8| Options {
        writer_pins: false,
        publish_in_background: false,
        snapshot_keeper: false,
        ..under(n)
    };
    // The same options in the clear are a store: the arm the suite prices.
    let plain = scratch("unpinned-plain");
    let mut db = Db::create(
        &plain,
        Options {
            encryption: Encryption::None,
            ..unpinned(1)
        },
    )
    .expect("an unpinned writer that is the one publisher, in the clear");
    fill(&mut db, 600);
    holds(&db, 600);
    drop(db);

    let dir = scratch("unpinned");
    let e = Db::create(&dir, unpinned(1))
        .err()
        .expect("an unpinned writer under a cipher was not refused at the creation");
    let e = e.to_string();
    assert!(e.contains("writer_pins") && e.contains("cipher"), "{e}");
    assert!(!dir.exists(), "the refused creation made the directory");

    let n = 2_000;
    {
        let mut db = Db::create(&dir, under(1)).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&dir);
    let e = Db::open(&dir, unpinned(1))
        .err()
        .expect("an unpinned writer under a cipher was not refused at the open");
    let e = e.to_string();
    assert!(e.contains("writer_pins") && e.contains("cipher"), "{e}");
    assert_eq!(
        snapshot(&dir),
        before,
        "the refused open changed the directory"
    );
    holds(&Db::open(&dir, under(1)).unwrap(), n);
}

/// The evictor of a store's cache ends with the store, at its close and
/// at a test's crash (`Db::crash`), as a dead process's does. Left running
/// at a crash, each crashed store's evictor read the room every twenty
/// milliseconds for the rest of the process, a `mincore` over every mapped
/// block alive in it, and at the end of a test binary under
/// `SUPDB_TEST_ONEFILE` fifty-eight of them held the process's map lock
/// between them while the one store still open crawled through its
/// `mmap`s. Read off the thread's task in `/proc/self/task`, which goes a
/// moment after the join that ends it.
#[test]
fn a_stores_close_and_its_crash_each_end_its_caches_evictor() {
    let evictor = |db: &Db| {
        let t = std::time::Instant::now();
        loop {
            let tid = db.cipher_counts().unwrap().cache.evictor_tid;
            if tid != 0 {
                let task = PathBuf::from(format!("/proc/self/task/{tid}"));
                assert!(task.exists(), "the premise: the evictor {tid} runs");
                return task;
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(60),
                "the evictor never made its first look"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    let gone = |task: &Path, how: &str| {
        let t = std::time::Instant::now();
        while task.exists() {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(10),
                "the evictor ({}) runs on past the store's {how}",
                task.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    for how in ["close", "crash"] {
        let dir = scratch(&format!("evictor-{how}"));
        let mut db = Db::create(&dir, under(1)).unwrap();
        fill(&mut db, 200);
        let task = evictor(&db);
        if how == "close" {
            db.close().unwrap();
        } else {
            db.crash();
        }
        gone(&task, how);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn mix(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn digest_pair(h: u64, key: &[u8], value: &[u8]) -> u64 {
    let h = mix(mix(h, &(key.len() as u32).to_le_bytes()), key);
    mix(mix(h, &(value.len() as u32).to_le_bytes()), value)
}

/// Everything a store holds, as one number.
fn digest_of(db: &Db) -> u64 {
    read_digest(db).unwrap()
}

/// `digest_of` for a store whose files may not read.
fn read_digest(db: &Db) -> std::io::Result<u64> {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |key, value| h = digest_pair(h, key, value)),
    )?;
    Ok(h)
}

fn digest_of_model(model: &BTreeMap<Vec<u8>, Vec<u8>>) -> u64 {
    model
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, (key, value)| {
            digest_pair(h, key, value)
        })
}

/// Seals and partitions of sixteen kilobytes, the segment work on the
/// writer's thread: a store of a hundred kilobytes with several segments
/// and a log, small enough to open hundreds of times.
fn tiny(n: u8) -> Options {
    Options {
        encryption: Encryption::Cipher(cipher(n)),
        seal_bytes: 16 << 10,
        partition_bytes: Some(16 << 10),
        l0_trigger: 2,
        ..base()
    }
}

/// A store with a history, left as a crash leaves it: sixty commits of
/// puts over seven hundred keys, flushed into segments, then forty small
/// commits that stay under a seal and so are the log's alone when the
/// store is dropped. What the store held after each commit, as a digest:
/// every state it ever acknowledged.
fn with_a_history(dir: &Path, n: u8) -> Vec<u64> {
    let mut db = Db::create(dir, tiny(n)).unwrap();
    let mut model = BTreeMap::new();
    let mut history = vec![digest_of_model(&model)];
    let mut commit = |db: &mut Db, c: u32, puts: u32| {
        for j in 0..puts {
            let i = (c * 37 + j * 101) % 700;
            db.put(&k(i), &v(i, c));
            model.insert(k(i), v(i, c));
        }
        db.commit().unwrap();
        history.push(digest_of_model(&model));
    };
    for c in 0..60 {
        commit(&mut db, c, 30);
    }
    db.flush().unwrap();
    db.settle().unwrap();
    for c in 60..100 {
        commit(&mut db, c, 2);
    }
    assert_eq!(digest_of(&db), *history.last().unwrap());
    drop(db);
    history
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

/// A store several times the size of the plaintext it may hold reads as
/// itself: every key, a scan of the whole of it in one call, reads from
/// four threads at once beside a writer, and all of it again after a
/// reopen. What it holds of itself stays at its cache's size, give or take
/// what the reads in flight are lent, through all of it; an open decrypts
/// what an open reads and not the store; and the store holds a descriptor
/// for none of its files.
#[test]
fn a_store_larger_than_its_cache_reads_as_itself_within_it() {
    let _quiet = one_at_a_time();
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

/// A snapshot under a cipher holds a state and nothing of the cache. Its
/// handle is pinned for the snapshot's life, and the cache of plaintext
/// gives nothing back that a pinned reader could have been lent, so the
/// handle says at each read from where it holds anything and at each
/// read's end that it holds nothing: with a snapshot standing, unread and
/// then between its reads, the store read through by its writer turns the
/// cache over within its budget. The snapshot reads the store as it was,
/// by the key and in one scan of the whole of it, several times its
/// cache; released, the handle reads the store as it is.
#[test]
fn a_snapshot_under_a_cipher_holds_its_state_and_none_of_the_cache() {
    let _quiet = one_at_a_time();
    let n = 100_000;
    let cache: u64 = 2 << 20;
    let slack: u64 = 8 << 20;
    let dir = scratch("snapshot");
    let mut db = Db::create(
        &dir,
        Options {
            encryption: Encryption::Cipher(cipher(11).cache_bytes(cache)),
            ..small()
        },
    )
    .unwrap();
    fill(&mut db, n);
    let size = stored_bytes(&dir);
    assert!(
        size > 4 * cache,
        "the store is {size} bytes, not several times its cache of {cache}"
    );
    // The store read through by the writer, and the cache back within its
    // budget after: it gave back since `before`, and never held more than
    // a read in flight is lent beyond the budget.
    let through = |db: &Db, what: &str| {
        let before = db.cipher_counts().unwrap().cache.evicted_units;
        let mut seen = 0u32;
        db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
            .unwrap();
        assert_eq!(seen, n, "{what}");
        let t = std::time::Instant::now();
        let c = loop {
            let c = db.cipher_counts().unwrap().cache;
            if c.held <= c.budget {
                break c;
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "{what}: the cache stayed over its budget with nothing reading: {c:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(
            c.evicted_units > before,
            "{what}: nothing was given back: {c:?}"
        );
        assert!(
            c.peak < cache + slack,
            "{what}: the cache held {} bytes of a budget of {cache}: {c:?}",
            c.peak
        );
    };
    let was = |i: u32| v(i, u32::from((n - 1 - i).is_multiple_of(3)));
    let snap = db.reader().unwrap();
    snap.snapshot();
    // Every other key rewritten past the snapshot, and the rewrite merged.
    for i in (0..n).step_by(2) {
        db.put(&k(i), &v(i, 5));
        if i % 1000 == 0 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    through(&db, "with a snapshot standing unread");
    // The snapshot's reads: the store as it was.
    for i in (0..n).step_by(11) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        snap.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        assert_eq!(got, [was(i)], "key {i} through the snapshot");
    }
    let mut seen = 0u32;
    let mut newer = 0u32;
    snap.scan(
        b"",
        usize::MAX,
        kv(&mut |key, val| {
            let i: u32 = std::str::from_utf8(&key[KEY_MARK.len()..])
                .unwrap()
                .parse()
                .unwrap();
            newer += u32::from(val != was(i).as_slice());
            seen += 1;
        }),
    )
    .unwrap();
    assert_eq!(
        (seen, newer),
        (n, 0),
        "the snapshot's scan of the whole store"
    );
    through(&db, "with a snapshot standing between its reads");
    // Released: the store as it is, through the same handle.
    snap.release();
    for i in (0..n).step_by(13) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        snap.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        let want = if i % 2 == 0 { v(i, 5) } else { was(i) };
        assert_eq!(got, [want], "key {i} after the release");
    }
    let mut seen = 0u32;
    snap.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
        .unwrap();
    assert_eq!(seen, n);
    through(&db, "after the release");
}

/// A caller's read waits a little for a cache far past its budget, and
/// then reads. The cache gives nothing back that a reader pinned before
/// could have been lent, so one reader parked inside its scan's callback
/// holds the cache open, and reads from another handle fill it past any
/// line: each of those waits a breath at its start, holding nothing, for
/// the cache to come back, and goes on when it has not -- the wait is a
/// brake and not a bound, since the reader it waits for may be the
/// caller's own. Let go, the parked scan ends and the cache comes back to
/// its budget.
#[test]
fn a_read_waits_a_breath_for_a_cache_far_past_its_budget() {
    let _quiet = one_at_a_time();
    let n = 150_000;
    let cache: u64 = 2 << 20;
    let dir = scratch("swamped");
    let mut db = Db::create(
        &dir,
        Options {
            encryption: Encryption::Cipher(cipher(13).cache_bytes(cache)),
            ..small()
        },
    )
    .unwrap();
    fill(&mut db, n);
    assert!(stored_bytes(&dir) > 6 * cache);
    let parked = db.reader().unwrap();
    let (inside, entered) = std::sync::mpsc::channel::<()>();
    let (release, go) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut first = true;
            parked
                .scan(
                    &k(0),
                    2,
                    kv(&mut |_k, _v| {
                        if std::mem::take(&mut first) {
                            inside.send(()).unwrap();
                            // Until it is let go, or the test has failed
                            // and dropped its end.
                            let _ = go.recv();
                        }
                    }),
                )
                .unwrap();
        });
        // Moved in, so that a failure below lets the parked reader go.
        let release = release;
        entered.recv().unwrap();
        let before = db.cipher_counts().unwrap().cache;
        let mut read = 0u32;
        for i in (0..n).step_by(3) {
            let mut got: Vec<Vec<u8>> = Vec::new();
            db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
                .unwrap();
            let round = u32::from((n - 1 - i).is_multiple_of(3));
            assert_eq!(got, [v(i, round)], "key {i}");
            read += 1;
            if db.cipher_counts().unwrap().cache.breaths >= before.breaths + 3 {
                break;
            }
        }
        let c = db.cipher_counts().unwrap().cache;
        assert!(
            c.breaths > before.breaths,
            "{read} reads beside a parked reader, and none waited: {c:?}"
        );
        assert!(
            c.held > c.budget,
            "the parked reader held nothing open: {c:?}"
        );
        release.send(()).unwrap();
    });
    let t = std::time::Instant::now();
    loop {
        let c = db.cipher_counts().unwrap().cache;
        if c.held <= c.budget {
            break;
        }
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "the cache stayed over its budget with nothing reading: {c:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The block cache's builders build through a cache the store does not
/// fit. A burst over every block of a store several times its cache is
/// filed into the block cache's forms by the store's own thread, block by
/// block, with a rest between two so that the cache may give back what the
/// blocks before were read from. The builders gave their forms up at the
/// first look that found the cache well past its budget, at first, with
/// no rest, and a pass reads every block it builds under the one pin it
/// holds: every block of such a burst was left to the scan that first
/// walked it, built through the cache's misses.
#[test]
fn the_block_caches_builders_build_through_a_cache_the_store_does_not_fit() {
    builders_build_through_a_small_cache(Upkeep::default(), "builders", 120_000, 0, 8 << 20);
}

/// And with values of two kilobytes, where sixty-four keys are an eighth
/// of a megabyte: the fill rests at every block under a cipher, where a
/// plain store's looks are sixty-four blocks apart, since sixty-four of
/// these blocks built between two rests are eight megabytes the cache
/// could not give back, four times this one.
#[test]
fn the_builders_rest_at_every_block_of_a_store_of_wide_values() {
    builders_build_through_a_small_cache(
        Upkeep::default(),
        "builders-wide",
        12_000,
        2_000,
        4 << 20,
    );
}

/// The same burst filed by the writer itself (`Upkeep::Inline`): the
/// commit builds every block of the store inside its one operation, and
/// rests between two as the store's thread does, since a commit that read
/// the store under its one pin is the store held in the cache.
#[test]
fn the_writers_own_fill_builds_through_a_cache_the_store_does_not_fit() {
    builders_build_through_a_small_cache(Upkeep::Inline, "builders-inline", 120_000, 0, 8 << 20);
}

/// `n` keys, each value `pad` bytes longer than `v`'s, and the cache held
/// to its budget and `slack`.
fn builders_build_through_a_small_cache(
    upkeep: Upkeep,
    name: &str,
    n: u32,
    pad: usize,
    slack: u64,
) {
    let _quiet = one_at_a_time();
    let inline = matches!(upkeep, Upkeep::Inline);
    let cache: u64 = 2 << 20;
    let dir = scratch(name);
    let v = |i: u32, round: u32| {
        let mut x = v(i, round);
        x.resize(x.len() + pad, b'w');
        x
    };
    // `sealing` off is a memtable that takes the whole burst: no seal at a
    // share of the store, and none for a store being read.
    let opts = |sealing: bool| Options {
        encryption: Encryption::Cipher(cipher(12).cache_bytes(cache)),
        seal_bytes: if sealing { 256 << 10 } else { 64 << 20 },
        seal_max_pct: if sealing { small().seal_max_pct } else { 0 },
        seal_idle: sealing && small().seal_idle,
        upkeep,
        // No builder ahead: a burst's commit starts one, and the writer's
        // fill and the store's thread then leave its blocks to it, where
        // these are the rests of the writer and of the store's thread
        // under test. The builder ahead's own look under a cipher stops
        // once the cache is crowded (`Reader::ahead_look`).
        scan_cache_ahead_min_blocks: usize::MAX,
        ..small()
    };
    let size = {
        let mut db = Db::create(&dir, opts(true)).unwrap();
        for i in 0..n {
            db.append(&k(i), &v(i, 0));
            if i % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        stored_bytes(&dir)
    };
    assert!(
        size > 4 * cache,
        "the store is {size} bytes, not several times its cache of {cache}"
    );
    // Opened again with a memtable the burst stays in: the store's files
    // are a quarter megabyte each, small beside the cache, and none is
    // written from here on, so what the cache holds is what was read.
    let mut db = Db::open(&dir, opts(false)).unwrap();
    // Scans, so that the store is one that is scanned: the forms are kept
    // for readers who are there.
    for from in (0..n).step_by(n as usize / 50) {
        let mut seen = 0u32;
        db.scan(&k(from), 100, kv(&mut |_k, _v| seen += 1)).unwrap();
        assert_eq!(seen, 100.min(n - from));
    }
    // The burst: a value added to every key, in one commit, so that filing
    // it is one pass over every block of the store, each built from its
    // records and the new values. A key rewritten would not do: its old
    // record is masked, and its block is built without being read.
    for i in 0..n {
        db.append(&k(i), &v(i, 2));
    }
    let before = db.blocks_built().1;
    // The peak held below is the burst's. The scans above are a caller's
    // reads, which wait for the evictor only at the cache's second line,
    // half the budget over it, and on a busy machine the evictor's first
    // pass came after they had filled the cache past the first line,
    // which is the line the store's own builders rest at and the one this
    // test holds them to: one run of the whole file in thirteen failed on
    // the scans' peak with the builders under the line.
    db.restart_cipher_peak();
    db.commit().unwrap();
    db.settle().unwrap();
    let ev = db.forms_events();
    let blocks = u64::from(n) / 64;
    let c = db.cipher_counts().unwrap().cache;
    if inline {
        let built = db.blocks_built().1 - before;
        assert!(
            built >= blocks * 9 / 10,
            "the commit built {built} forms for a burst over {blocks} blocks: {c:?}"
        );
    } else {
        assert!(
            ev[0] + ev[10] >= blocks * 9 / 10,
            "the store's own threads built {} forms and {} ahead for a burst over {blocks} blocks: {c:?}",
            ev[0],
            ev[10]
        );
    }
    assert!(c.evicted_units > 0, "nothing was given back: {c:?}");
    assert!(
        c.peak < cache + slack,
        "the cache held {} bytes of a budget of {cache}: {c:?}",
        c.peak
    );
    // And what the forms answer is the store: every key with both its
    // values, by scans of a hundred keys.
    let mut seen = 0u32;
    for from in (0..n).step_by(100) {
        let mut at = from;
        let mut second = false;
        db.scan(
            &k(from),
            100,
            kv(&mut |key, val| {
                assert_eq!(key, k(at).as_slice());
                let round = if second { 2 } else { 0 };
                assert_eq!(val, v(at, round).as_slice(), "key {at}");
                if second {
                    at += 1;
                    seen += 1;
                }
                second = !second;
            }),
        )
        .unwrap();
    }
    assert_eq!(seen, n);
}

/// A byte changed anywhere in the directory is refused, or is where the
/// log ends: what the store answers afterwards is a state it
/// acknowledged, the last or one before it, and never anything else.
/// A store's files are decrypted where they are read, so a changed byte
/// is refused in one of two places: at the open, where it is in what an
/// open reads -- the marker, a manifest, an envelope's header -- and at
/// the read that first wants it, which answers with an error and not with
/// what is left of the store, as every read after it does. Every file is
/// walked, and each of the four outcomes has to occur: a store refused at
/// its open, a store that refuses to be read, a store that lost its log's
/// tail, and a store whole where the byte was in a file nothing durable
/// rests on.
#[test]
fn a_changed_byte_is_refused_or_is_where_the_log_ends() {
    let dir = scratch("tamper");
    let history = with_a_history(&dir, 3);
    let pristine = snapshot(&dir);
    let (mut refused, mut unread, mut earlier, mut whole) = (0, 0, 0, 0);
    for (name, bytes) in &pristine {
        let stride = (bytes.len() / 24).max(1);
        let mut ats: Vec<usize> = (0..bytes.len()).step_by(stride).collect();
        if name == "store.one" {
            // The data file of a store kept in one: every envelope behind
            // a head of a megabyte, so most of two dozen steps over the
            // whole fall in the head. What lies past it is walked at steps
            // of its own, which fall in the envelopes' headers, their
            // bodies and the room between them as the steps above fall in
            // a file an envelope.
            let head = 1usize << 20;
            let fine = (bytes.len().saturating_sub(head) / 120).max(1);
            ats.extend((head..bytes.len()).step_by(fine));
        }
        for at in ats {
            restore(&dir, &pristine);
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            std::fs::write(dir.join(name), &bad).unwrap();
            let db = match Db::open(&dir, tiny(3)) {
                Err(_) => {
                    refused += 1;
                    continue;
                }
                Ok(db) => db,
            };
            match read_digest(&db) {
                Err(e) => {
                    unread += 1;
                    assert!(
                        e.to_string().contains("did not read"),
                        "{name}, byte {at}: refused as {e}"
                    );
                    // And from then on: no read of it answers.
                    assert!(
                        db.read_all(&k(1), vf(&mut |_| ())).is_err(),
                        "{name}, byte {at}: a read answered after one had failed"
                    );
                    assert!(read_digest(&db).is_err());
                }
                Ok(d) => {
                    assert!(
                        history.contains(&d),
                        "{name}, byte {at}: read as a state the store never held"
                    );
                    if d == *history.last().unwrap() {
                        whole += 1;
                    } else {
                        earlier += 1;
                    }
                }
            }
        }
    }
    assert!(
        refused > 0 && unread > 0 && earlier > 0 && whole > 0,
        "refused {refused}, unread {unread}, earlier {earlier}, whole {whole}"
    );
    // And untouched, it opens whole.
    restore(&dir, &pristine);
    assert_eq!(
        digest_of(&Db::open(&dir, tiny(3)).unwrap()),
        *history.last().unwrap()
    );
}

/// The log cut at any byte, as a power loss cuts it, opens: as the commits
/// whose records are whole before the cut, each a state the store
/// acknowledged, later for a later cut, and the whole store for the whole
/// log.
#[test]
fn a_log_cut_anywhere_opens_as_the_commits_before_the_cut() {
    let dir = scratch("cut");
    let history = with_a_history(&dir, 4);
    let pristine = snapshot(&dir);
    let (log, bytes) = pristine
        .iter()
        .rfind(|(n, _)| n.starts_with("wal-"))
        .expect("a log");
    let mut last = 0;
    let mut states = std::collections::BTreeSet::new();
    for cut in (0..bytes.len()).step_by(7).chain([bytes.len()]) {
        restore(&dir, &pristine);
        std::fs::write(dir.join(log), &bytes[..cut]).unwrap();
        let db = Db::open(&dir, tiny(4)).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
        let d = digest_of(&db);
        let at = history
            .iter()
            .position(|&h| h == d)
            .unwrap_or_else(|| panic!("cut at {cut}: a state the store never held"));
        assert!(at >= last, "cut at {cut}: state {at} after state {last}");
        last = at;
        states.insert(at);
    }
    assert_eq!(last, history.len() - 1, "the whole log is the whole store");
    assert!(states.len() > 3, "the cuts fell in {} states", states.len());
}

/// What a crash leaves in the directory beside the store: an envelope no
/// manifest names is removed at the open, and the store is whole without
/// it; a named one missing is refused.
#[test]
fn an_envelope_nobody_names_is_swept_and_one_missing_is_refused() {
    let n = 4_000;
    let dir = scratch("orphan");
    // A store of files: the envelopes here are copied, torn and removed by
    // their paths. What a store kept in one file does with room no
    // manifest names, and with a place its manifest names that is not
    // there, is `tests/onefile.rs`'s.
    let files = || common::files(under(5));
    {
        let mut db = Db::create(&dir, files()).unwrap();
        fill(&mut db, n);
    }
    let segment = names(&dir)
        .into_iter()
        .find(|name| name.starts_with("x-") && !name.ends_with(".o"))
        .expect("an envelope");
    let orphan = format!("x-{}", "ab".repeat(16));
    std::fs::copy(dir.join(&segment), dir.join(&orphan)).unwrap();
    std::fs::write(dir.join(format!("{orphan}.o")), b"torn").unwrap();
    {
        let db = Db::open(&dir, files()).unwrap();
        holds(&db, n);
        assert_eq!(db.cipher_counts().unwrap().swept, 2);
    }
    assert!(!dir.join(&orphan).exists());
    assert!(!dir.join(format!("{orphan}.o")).exists());
    // An ordered index missing is composed again from its segment, as a
    // plain store's is, and written back with the next manifest.
    let index = format!("{segment}.o");
    std::fs::remove_file(dir.join(&index)).unwrap();
    {
        let mut db = Db::open(&dir, files()).unwrap();
        holds(&db, n);
        assert!(db.ord_rebuilt() > 0);
        db.put(&k(0), &v(0, 1));
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
    }
    std::fs::remove_file(dir.join(&segment)).ok();
    if !dir.join(&segment).exists() {
        // The flush above may have merged the segment away; whichever
        // envelope the store rests on now, one of them gone is refused.
        let any = names(&dir)
            .into_iter()
            .find(|name| name.starts_with("x-") && !name.ends_with(".o"));
        if let Some(any) = any {
            std::fs::remove_file(dir.join(any)).unwrap();
        }
    }
    assert!(Db::open(&dir, files()).is_err());
}

/// A fall back past a damaged slot under a cipher, where the slot passed
/// over had retired the log file holding writes only it named: the slot
/// before needs that file (`Manifest::needs_log`), and the open refuses it
/// with every file as found, where it swept the envelope the damaged slot
/// named and answered the writes' older values. Put back, the slot opens as
/// the store.
#[test]
fn a_fall_back_under_a_cipher_past_a_slot_whose_log_retired_is_refused() {
    let dir = scratch("fallback-log");
    let opts = || Options {
        seal_rotates_wal: true,
        recycle_wal: false,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        direct_ingest: false,
        ..under(9)
    };
    let mut db = Db::create(&dir, opts()).unwrap();
    for i in 0..2000u32 {
        db.put(&k((i * 7919) % 2000), &v(0, 0));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    for i in (0..2000u32).step_by(7) {
        db.put(&k(i), &v(i, 1));
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    // Dropped, as a crash is here, the seal's landing the last thing
    // written: a close would flush, and the merge it lands would stand in
    // both slots' place.
    drop(db);
    // The slot written last is the newest: the one the seal of the
    // rewrites landed in.
    let slot = |n: &str| dir.join(n);
    let age = |n: &str| std::fs::metadata(slot(n)).unwrap().modified().unwrap();
    let newest = if age("manifest") >= age("manifest.alt") {
        "manifest"
    } else {
        "manifest.alt"
    };
    let saved = std::fs::read(slot(newest)).unwrap();
    let mut bad = saved.clone();
    let at = bad.len() / 2;
    bad[at] ^= 0x5a;
    std::fs::write(slot(newest), &bad).unwrap();
    let before = names(&dir);
    let e = match Db::open(&dir, opts()) {
        Ok(db) => {
            let mut got = Vec::new();
            db.read_all(&k(7), vf(&mut |x| got.push(x.to_vec())))
                .unwrap();
            panic!("the open fell back past the slot whose log retired and answered Ok: names {before:?} newest {newest} key7 {got:?}")
        }
        Err(e) => e.to_string(),
    };
    assert!(e.contains("needs the log file"), "the refusal's words: {e}");
    assert_eq!(
        names(&dir),
        before,
        "a refused open leaves the directory as it found it"
    );
    std::fs::write(slot(newest), &saved).unwrap();
    let db = Db::open(&dir, opts()).unwrap();
    for i in (0..2000u32).step_by(7) {
        let mut got = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        assert_eq!(got.last(), Some(&v(i, 1)), "the rewrite of {i}, repaired");
    }
}

/// A store dropped without its close, its memory gone with it, opens from
/// its directory as everything it had committed; and a copy of the
/// directory taken while the store is open opens beside it, under the same
/// cipher, as the store at that moment.
#[test]
fn a_crash_and_a_copy_open_as_what_was_committed() {
    let n = 12_000;
    let dir = scratch("crash");
    {
        let mut db = Db::create(&dir, under(6)).unwrap();
        for i in 0..n {
            db.append(&k(i), &v(i, 0));
            if i % 250 == 249 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        for i in (0..n).rev().step_by(3) {
            db.put(&k(i), &v(i, 1));
            if i % 250 < 3 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        // No flush, no settle, no close: seals in flight, the tail in the
        // log, and every memory file gone at the drop.
    }
    // And again before anything is written: what the first open replayed
    // out of the log is in memory alone, so the log it replayed has to be
    // there for the second.
    holds(&Db::open(&dir, under(6)).unwrap(), n);
    let mut db = Db::open(&dir, under(6)).unwrap();
    holds(&db, n);
    db.flush().unwrap();
    db.settle().unwrap();
    let copy = scratch("crash-copy");
    std::fs::create_dir_all(&copy).unwrap();
    restore(&copy, &snapshot(&dir));
    let twin = Db::open(&copy, under(6)).unwrap();
    holds(&twin, n);
    holds(&db, n);
}

/// A store under a cipher recycles its log's files (`Options::recycle_wal`):
/// a file a durable segment has retired is kept under a spare's name and
/// renamed to be the next log, written in place over its last life's
/// records. Those are sealed under the last life's key and the new file's
/// header holds another, so the first of them fails its tag, and that is
/// where the new log ends: nothing of a file's last life is read back.
/// The spare is in the directory, holds nothing in the clear, and is the
/// file the next rotation's log is, by its inode. Across crashes the store
/// holds what it committed, each value once; closed, it leaves no spare.
#[test]
fn a_recycled_log_under_a_cipher_holds_what_was_committed_and_no_more() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch("recycle");
    let opts = || Options {
        recycle_wal: true,
        seal_bytes: 64 << 10,
        l0_trigger: 2,
        ..under(14)
    };
    // A log or spare file by its name and its identity, the inode with the
    // time it was made: a rename keeps both, and an inode number the
    // filesystem gives again to a new file has a new birth.
    let files = || -> Vec<(String, (u64, Option<std::time::SystemTime>))> {
        names(&dir)
            .into_iter()
            .filter(|n| n.starts_with("wal-") || n.starts_with("spare-"))
            // A file the segment work retired or renamed since the
            // listing is not there to ask.
            .filter_map(|n| {
                let m = std::fs::metadata(dir.join(&n)).ok()?;
                Some((n, (m.ino(), m.created().ok())))
            })
            .collect()
    };
    let key = |i: u32| format!("{KEY_MARK}{i:05}").into_bytes();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    let holds_model = |db: &Db, model: &BTreeMap<Vec<u8>, Vec<Vec<u8>>>, when: &str| {
        for (k, want) in model {
            let mut got: Vec<Vec<u8>> = Vec::new();
            db.read_all(k, vf(&mut |x| got.push(x.to_vec()))).unwrap();
            assert_eq!(&got, want, "{when}");
        }
    };
    let mut db = Db::create(&dir, opts()).unwrap();
    // A round's writes and their commit. Out of order, so that every write
    // goes through the log, and enough of them for a few rounds to take the
    // log past its size and rotate it.
    let write_round = |db: &mut Db, model: &mut BTreeMap<Vec<u8>, Vec<Vec<u8>>>, round: u32| {
        for i in 0u32..400 {
            let k = key((i * 7 + round) % 500);
            let v = format!("{MARK} round {round} write {i}").into_bytes();
            db.append(&k, &v);
            model.entry(k).or_default().push(v);
        }
        db.commit().unwrap();
    };
    let mut round = 0u32;
    while round < 16 {
        write_round(&mut db, &mut model, round);
        if round % 4 == 3 {
            drop(db);
            db = Db::open(&dir, opts()).unwrap();
            holds_model(&db, &model, &format!("after the crash in round {round}"));
        }
        round += 1;
    }
    // Rounds until the log has rotated: the live file is another.
    let mut rotate = |db: &mut Db, model: &mut BTreeMap<Vec<u8>, Vec<Vec<u8>>>| {
        let (before, _, _) = db.wal_durable();
        let from = round;
        while db.wal_durable().0 == before {
            write_round(db, model, round);
            round += 1;
            assert!(round < from + 64, "the log never rotated");
        }
    };
    // Whether a rotation of the rounds above found a spare waiting was the
    // segment work's timing: a retired log is offered as the spare on that
    // thread, behind a seal's durable landing, and a rotation that comes
    // first makes its file. So the spare is held here, not waited for. A
    // rotation leaves a closed log; the flush's durable landing retires it,
    // and with no spare waiting it is kept as one, which the settle joins;
    // the rotation after that takes it. A log pre-written ahead of its tail
    // (`Options::wal_ahead_bytes`) is made with no spare, so the first is
    // always a retired file.
    rotate(&mut db, &mut model);
    db.flush().unwrap();
    db.settle().unwrap();
    let (spare_was, spare_id) = files()
        .into_iter()
        .find(|(n, _)| n.starts_with("spare-"))
        .unwrap_or_else(|| panic!("a retired log is kept as the spare: {:?}", names(&dir)));
    rotate(&mut db, &mut model);
    // The file that was the spare is the log now: recycled, not made.
    let (live, _, _) = db.wal_durable();
    let m = std::fs::metadata(&live).unwrap();
    assert_eq!(
        (m.ino(), m.created().ok()),
        spare_id,
        "the rotation made {live:?} where {spare_was} was waiting: {:?}",
        names(&dir)
    );
    db.flush().unwrap();
    db.settle().unwrap();
    // The live log's last life lies behind its header, longer than what
    // this life goes on to write: two commits, a crash, and none of the
    // old records read back as a value twice.
    for (k, v) in [("new-a", "A"), ("new-b", "B")] {
        db.append(k.as_bytes(), v.as_bytes());
        model
            .entry(k.as_bytes().to_vec())
            .or_default()
            .push(v.as_bytes().to_vec());
        db.commit().unwrap();
    }
    for needle in [MARK, KEY_MARK] {
        assert!(
            files_holding(&dir, needle.as_bytes()).is_empty(),
            "{needle} in the clear in {:?}",
            files_holding(&dir, needle.as_bytes())
        );
    }
    drop(db);
    let db = Db::open(&dir, opts()).unwrap();
    holds_model(&db, &model, "after the crash behind a recycled log");
    drop(db);
    // A crash at a rotation, as it leaves the directory: the spare under
    // the next log's name, and its new header not on the device, so the
    // header there is its last life's, a key wrapped for another file.
    // A store that recycles reads that as a log with nothing in it; one
    // that does not has no such file to meet and refuses it.
    let live: u64 = names(&dir)
        .iter()
        .filter_map(|n| n.strip_prefix("wal-")?.parse().ok())
        .max()
        .unwrap();
    let spare_name = names(&dir)
        .into_iter()
        .find(|n| n.starts_with("spare-"))
        .expect("a spare while the store was open");
    let next = dir.join(format!("wal-{:08}", live + 1));
    std::fs::rename(dir.join(&spare_name), &next).unwrap();
    let kept = std::fs::read(&next).unwrap();
    let refused = Db::open(
        &dir,
        Options {
            recycle_wal: false,
            ..opts()
        },
    )
    .err()
    .expect("a log under another file's key, in a store that recycles none");
    assert!(
        refused.to_string().contains("not this store's log"),
        "{refused}"
    );
    // And a file there that is no sealed log's at all is nobody's last
    // life: refused by a store that recycles too.
    let mut strange = kept.clone();
    strange[0] ^= 0x55;
    std::fs::write(&next, &strange).unwrap();
    assert!(
        Db::open(&dir, opts()).is_err(),
        "a log file that is not a sealed log's opened"
    );
    std::fs::write(&next, &kept).unwrap();
    let mut db = Db::open(&dir, opts()).unwrap();
    holds_model(&db, &model, "after the crash at a rotation");
    db.append(b"new-c", b"C");
    model
        .entry(b"new-c".to_vec())
        .or_default()
        .push(b"C".to_vec());
    db.commit().unwrap();
    drop(db);
    let db = Db::open(&dir, opts()).unwrap();
    holds_model(&db, &model, "and after a commit into the log begun there");
    db.close().unwrap();
    assert!(
        !files().iter().any(|(n, _)| n.starts_with("spare-")),
        "a closed store left a spare: {:?}",
        names(&dir)
    );
    holds_model(&Db::open(&dir, opts()).unwrap(), &model, "reopened");
}

/// Ordered ingest under a cipher (`Options::direct_ingest`): keys above
/// the store's greatest go to an ordered run, as a plain store's do, and
/// the run's file is a memory file, so each commit of it is sealed into
/// the log as well, which is what a crash leaves. A store dropped in the
/// middle of a run opens with every commit of the run and nothing of the
/// batch that was not committed; a run left for a key out of order, and a
/// run closed by a flush, read as themselves; and nothing of a run is in
/// the directory in the clear, its file's name included.
#[test]
fn an_ordered_run_under_a_cipher_keeps_its_commits_through_the_log() {
    let n = 24_000u32;
    let dir = scratch("runs");
    let opts = || under(15);
    let check = |db: &Db, upto: u32, what: &str| {
        let mut at = 0u32;
        db.scan(
            b"",
            usize::MAX,
            kv(&mut |key, val| {
                assert_eq!(key, k(at).as_slice(), "{what}");
                assert_eq!(val, v(at, 0).as_slice(), "{what}: key {at}");
                at += 1;
            }),
        )
        .unwrap();
        assert_eq!(at, upto, "{what}");
        for i in (0..upto).step_by(97) {
            let mut got: Vec<Vec<u8>> = Vec::new();
            db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
                .unwrap();
            assert_eq!(got, [v(i, 0)], "{what}: key {i}");
        }
    };
    {
        let mut db = Db::create(&dir, opts()).unwrap();
        for i in 0..n / 2 {
            db.append(&k(i), &v(i, 0));
            if i % 250 == 249 {
                db.commit().unwrap();
            }
            if i == 249 {
                // In the log, and synced there, as the policy says.
                let (_, synced, written) = db.wal_durable();
                assert!(
                    written > 250 * 60,
                    "the run's first commit is not in the log"
                );
                assert_eq!(synced, written, "a run's commit left the log unsynced");
            }
        }
        let (runs, logged) = db.direct_runs();
        assert!(runs > 0, "no ordered run was opened under the cipher");
        assert!(
            logged >= u64::from(n / 2 / 250) - 1,
            "{logged} commits of {runs} runs went to the log"
        );
        check(&db, n / 2, "a run in progress");
        // A batch appended and not committed, and the store gone.
        for i in n / 2..n / 2 + 100 {
            db.append(&k(i), &v(i, 0));
        }
    }
    assert!(
        names(&dir).iter().all(|f| !f.contains("direct")),
        "a run's own file in the directory: {:?}",
        names(&dir)
    );
    {
        let mut db = Db::open(&dir, opts()).unwrap();
        check(&db, n / 2, "after the crash in the middle of a run");
        // The rest in order, a run again, left for a key out of order
        // with a batch in hand, and the store flushed.
        for i in n / 2..n {
            db.append(&k(i), &v(i, 0));
            if i % 250 == 249 {
                db.commit().unwrap();
                // The log rotates under a run as under any commits: a
                // file of it passes the size a seal is cut at by a
                // commit, and no more.
                let (_, _, written) = db.wal_durable();
                assert!(
                    written < 2 * (256 << 10),
                    "a log file of {written} bytes under a run"
                );
            }
            if i == n - 100 {
                assert!(db.direct_runs().0 > 0, "no run after the reopen");
            }
        }
        db.append(b"a key below every other", b"left the run");
        db.commit().unwrap();
        db.delete(b"a key below every other");
        db.commit().unwrap();
        check(&db, n, "a run left for a key out of order");
        db.flush().unwrap();
        db.settle().unwrap();
        check(&db, n, "flushed");
        // The log rotates under a run as under any commits, and what a
        // closed run's segment covers retires: several runs' worth of
        // records went through it, and it holds a file or two.
        let logs: u64 = names(&dir)
            .iter()
            .filter(|f| f.starts_with("wal-"))
            .map(|f| std::fs::metadata(dir.join(f)).unwrap().len())
            .sum();
        assert!(
            logs < 3 * (256 << 10),
            "{logs} bytes of log after an ordered load of {n} keys"
        );
    }
    for name in names(&dir) {
        let known = name == "supdb.cipher"
            // The store lock: empty, and named alike in every store.
            || name == "LOCK"
            || name.starts_with("manifest")
            || name.starts_with("wal-")
            || name.starts_with("spare-")
            || (name == "store.one" && common::one_file_switch())
            || name.starts_with("x-");
        assert!(known, "a file named {name}");
    }
    for needle in [MARK, KEY_MARK] {
        assert!(files_holding(&dir, needle.as_bytes()).is_empty());
    }
    check(&Db::open(&dir, opts()).unwrap(), n, "reopened");
}

/// A run that closes is made durable where it closes, on the thread that
/// finishes its segment, as a plain run's segment is synced there. Left
/// to the manifest, which writes the envelope of whatever it names that
/// has none, the cipher over a run's segment was the segment work's to
/// do, on the thread every landing goes through. A store's first run
/// closes under the name it keeps, so nothing renames it on the way.
#[test]
fn a_closed_runs_segment_is_sealed_where_it_closes() {
    let dir = scratch("run-close");
    let mut db = Db::create(&dir, under(17)).unwrap();
    for i in 0..2_000u32 {
        db.append(&k(i), &v(i, 0));
        if i % 250 == 249 {
            db.commit().unwrap();
        }
    }
    assert_eq!(db.direct_runs().0, 1, "one run");
    db.flush().unwrap();
    db.settle().unwrap();
    let c = db.cipher_counts().unwrap();
    assert!(c.persisted > 0, "the run's segment has no envelope: {c:?}");
    assert_eq!(
        c.late, 0,
        "the run's envelope was left to the manifest that names it: {c:?}"
    );
    let mut seen = 0u32;
    db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
        .unwrap();
    assert_eq!(seen, 2_000);
}

/// An ordered run's table reads the records it has committed through a
/// mapping of the run's own file, and under a cipher that file is memory
/// the store's cache takes for its view once the run has closed and has
/// its envelope. The cache gives back by taking pages out of that memory,
/// so while a table still maps the run its file is not the cache's to
/// give back: a snapshot taken over a run reads every key of it after the
/// run has closed and the store has been read through a cache a fraction
/// of its size, and once the snapshot has gone the run's file is given
/// back as any is.
#[test]
fn a_runs_table_reads_its_run_while_the_cache_turns_over() {
    let _quiet = one_at_a_time();
    let n = 120_000u32;
    let run = 40_000u32;
    let cache: u64 = 2 << 20;
    let dir = scratch("run-map");
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(16).cache_bytes(cache)),
        // The run is one table: no seal at a share of the store.
        seal_bytes: 64 << 20,
        seal_max_pct: 0,
        seal_idle: false,
        direct_map_bytes: 64 << 10,
        ..small()
    };
    {
        // A store several times the cache under the run, made with small
        // seals so that it is partitions.
        let mut db = Db::create(
            &dir,
            Options {
                encryption: Encryption::Cipher(cipher(16).cache_bytes(cache)),
                ..small()
            },
        )
        .unwrap();
        fill(&mut db, n);
    }
    let mut db = Db::open(&dir, opts()).unwrap();
    assert!(stored_bytes(&dir) > 4 * cache);
    let top = |i: u32| format!("{KEY_MARK}zz-{i:08}").into_bytes();
    for i in 0..run {
        db.append(&top(i), &v(i, 3));
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    assert!(
        db.direct_runs().0 > 0,
        "the keys above the store's took no run"
    );
    assert!(
        db.stream_recycled() > 0,
        "the run's table gave no block up, and reads nothing through the run's file"
    );
    let snap = db.reader().unwrap();
    snap.snapshot();
    // The run closed and durable, its table held by the snapshot alone.
    db.flush().unwrap();
    db.settle().unwrap();
    // The store under the run, read through: every seventh key of it and
    // a scan of all of it, the run's keys with it.
    let under_it = |db: &Db| {
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
        assert_eq!(seen, n + run);
    };
    let through = |h: &supdb::Reader, what: &str| {
        for i in (0..run).step_by(7) {
            let mut got: Vec<Vec<u8>> = Vec::new();
            h.read_all(&top(i), vf(&mut |x| got.push(x.to_vec())))
                .unwrap();
            assert_eq!(got, [v(i, 3)], "{what}: key {i} of the run");
        }
    };
    for round in 0..3 {
        // The store under the run read through, several times the cache.
        let before = db.cipher_counts().unwrap().cache.evicted_units;
        under_it(&db);
        assert!(
            db.cipher_counts().unwrap().cache.evicted_units > before,
            "round {round}: nothing was given back"
        );
        through(
            &snap,
            &format!("round {round}, through the snapshot's table"),
        );
    }
    snap.release();
    drop(snap);
    // The store keeps a table it has replaced until its next publish
    // (`State::replaced`), so the run's table goes with one: a write, a
    // flush. Nothing maps the run then, and its file is given back with
    // the rest.
    db.put(&top(run), b"the next publish");
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    for i in (0..n).step_by(7) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        assert_eq!(got.len(), 1, "key {i}");
    }
    let t = std::time::Instant::now();
    loop {
        let c = db.cipher_counts().unwrap().cache;
        if c.held <= c.budget {
            break;
        }
        assert!(
            t.elapsed() < std::time::Duration::from_secs(20),
            "the cache stayed over its budget with nothing reading: {c:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for i in (0..run).step_by(11) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&top(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        assert_eq!(got, [v(i, 3)], "key {i} of the run, from its segment");
    }
}

/// The compact form under a cipher (`ZPages::Always`): every partition is
/// given its form file, built from records read through the store's cache
/// by the piece; reads are answered through it; it is in the directory as
/// an envelope beside its segment's and as nothing else; and the next
/// open finds it there and builds none. Ordered, so that the run is under
/// it too, with a crash before the first settle.
#[test]
fn the_compact_form_under_a_cipher_is_built_read_and_found_again() {
    let n = 12_000;
    let dir = scratch("forced");
    let asked = || Options {
        direct_ingest: true,
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        ..under(7)
    };
    {
        let mut db = Db::create(&dir, asked()).unwrap();
        // Ordered, committed, and dropped.
        for i in 0..n {
            db.append(&k(i), &v(i, 0));
            if i % 250 == 249 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
    }
    let (partitions, all) = {
        let mut db = Db::open(&dir, asked()).unwrap();
        let mut seen = 0u32;
        db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
            .unwrap();
        assert_eq!(seen, n, "what was committed before the crash");
        for i in (0..n).rev().step_by(3) {
            db.put(&k(i), &v(i, 1));
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        db.zpages_settle().unwrap();
        let forms = db.zpage_form_files();
        assert!(!forms.is_empty(), "no form file under a cipher");
        assert!(
            forms.iter().all(|f| !f.3),
            "a form file out of service: {forms:?}"
        );
        let before = db.zpage_counts();
        assert!(before.build_starts > 0, "no build begun");
        assert_eq!(before.build_failed, 0, "a build failed");
        holds(&db, n);
        let after = db.zpage_counts();
        assert!(
            after.reads > before.reads,
            "no read was answered through the form"
        );
        assert_eq!(after.fallbacks, before.fallbacks, "a read fell back");
        (forms.len(), names(&dir))
    };
    // Among the store's files: an envelope a form file, beside its
    // segment's, and no name or byte of the store's in the clear.
    let forms: Vec<&String> = all.iter().filter(|n| n.ends_with(".z")).collect();
    assert_eq!(forms.len(), partitions, "an envelope a form file: {all:?}");
    for z in &forms {
        let seg = z.strip_suffix(".z").unwrap();
        assert!(
            all.iter().any(|n| n == seg),
            "{z} beside no segment's envelope"
        );
    }
    for name in &all {
        let known = name == "supdb.cipher"
            // The store lock: empty, and named alike in every store.
            || name == "LOCK"
            || name.starts_with("manifest")
            || name.starts_with("wal-")
            || name.starts_with("spare-")
            || (name == "store.one" && common::one_file_switch())
            || name.starts_with("x-");
        assert!(known, "a file named {name}");
    }
    for needle in [MARK, KEY_MARK] {
        assert!(files_holding(&dir, needle.as_bytes()).is_empty());
    }
    // What a crash in the middle of a form file's envelope leaves: the
    // temp name it is written under, which the next open removes.
    let torn = format!("{}.tmp", forms[0]);
    std::fs::write(dir.join(&torn), b"half an envelope").unwrap();
    // The next open attaches what is there.
    let mut db = Db::open(&dir, asked()).unwrap();
    assert!(
        !dir.join(&torn).exists(),
        "a torn envelope outlived the open"
    );
    db.zpages_settle().unwrap();
    let counts = db.zpage_counts();
    assert_eq!(counts.build_starts, 0, "a form file built again");
    assert_eq!(
        db.zpage_form_files().len(),
        partitions,
        "the form files the last open left"
    );
    holds(&db, n);
    assert!(db.zpage_counts().reads > counts.reads);
    assert_eq!(db.zpage_counts().fallbacks, counts.fallbacks);
}

/// The compact form of a store several times its cache: every form file
/// is built through the cache, the builder's reads resting so that the
/// cache gives back what the pages before were read from; the store's
/// reads are answered through the forms; and what the store holds of
/// itself stays at its cache's size and a slack through the builds and the
/// reads, and through a reopen that attaches every form file.
#[test]
fn the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit() {
    let _quiet = one_at_a_time();
    // Twice. With the builds left to the form's own settle, every one of
    // them falls between the two readings of the cache, and what they gave
    // back is counted exactly. With the builds begun as the partitions land
    // (the default), they run beside the fill's seals and merges, and how
    // many are left for the settle is the timing's: a third had run by the
    // first reading in every run looked at, and all of them in one run of a
    // gate's stride group, which read "the builds gave nothing back" off a
    // cache that had given back 690 units before that reading.
    for eager in [false, true] {
        a_form_larger_than_its_cache(eager);
    }
}

fn a_form_larger_than_its_cache(eager: bool) {
    let n = 150_000;
    let cache: u64 = 2 << 20;
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(12).cache_bytes(cache)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        zpages_eager: eager,
        ..small()
    };
    let dir = scratch(if eager {
        "formlarger-eager"
    } else {
        "formlarger"
    });
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
    let (forms, form_bytes) = {
        let mut db = Db::create(&dir, opts()).unwrap();
        fill(&mut db, n);
        db.flush().unwrap();
        db.settle().unwrap();
        let before = settled(&db, "after the fill");
        let started = db.zpage_counts().build_starts;
        db.zpages_settle().unwrap();
        let forms = db.zpage_form_files();
        assert!(
            forms.len() >= 8,
            "the store's partitions have {} form files",
            forms.len()
        );
        assert!(forms.iter().all(|f| !f.3), "a form file out of service");
        let z = db.zpage_counts();
        assert_eq!(z.build_failed, 0, "a build failed");
        let c = settled(&db, "after the builds");
        // Every partition was read whole by its build, the store several
        // times over its cache: read under one hold with no rest, the
        // builds hold the store.
        if eager {
            // Begun at the first landing: held to having been made, and the
            // cache to having turned over since the store's birth.
            assert!(
                z.builds >= forms.len() as u64,
                "{} builds for {} form files",
                z.builds,
                forms.len()
            );
            assert!(c.evicted_units > 0, "the cache gave nothing back: {c:?}");
        } else {
            assert_eq!(
                started, 0,
                "a build before the form's settle, with the builds left to it"
            );
            assert!(
                c.evicted_units > before.evicted_units,
                "the builds gave nothing back: {c:?}"
            );
        }
        assert!(
            c.peak < cache + slack,
            "the cache held {} bytes of a budget of {cache} through the builds: {c:?}",
            c.peak
        );
        holds(&db, n);
        let after = db.zpage_counts();
        assert!(after.reads > z.reads, "no read through a form");
        assert!(after.scans > z.scans, "no scan through a form");
        assert_eq!(after.fallbacks, z.fallbacks, "a read fell back");
        let c = settled(&db, "after the reads");
        assert!(
            c.peak < cache + slack,
            "the cache held {} bytes of a budget of {cache} through the reads: {c:?}",
            c.peak
        );
        // No form file is left in its writer's memory: each has its
        // envelope, and is the cache's to give back.
        assert_eq!(
            db.cipher_counts().unwrap().files,
            0,
            "a file with no envelope"
        );
        let bytes: u64 = names(&dir)
            .iter()
            .filter(|n| n.ends_with(".z"))
            .map(|n| len_of(&dir, n))
            .sum();
        (forms.len(), bytes)
    };
    assert!(
        form_bytes > 2 * cache,
        "the form files are {form_bytes} bytes, not more than their cache of {cache}"
    );
    // Reopened, the form files attached by the open: what it decrypts is
    // by the file, a handful of chunks a segment and an index and the
    // attach's reads a form file, and not by the byte.
    let mut db = Db::open(&dir, opts()).unwrap();
    let opened = db.cipher_counts().unwrap().cache;
    assert_eq!(db.zpage_form_files().len(), forms);
    let others = names(&dir)
        .iter()
        .filter(|n| n.starts_with("x-") && !n.ends_with(".z"))
        .count() as u64;
    assert!(
        opened.fills <= 6 * others + 40 * forms as u64,
        "the open decrypted {} chunks of {others} files and {forms} form files: {opened:?}",
        opened.fills
    );
    db.zpages_settle().unwrap();
    assert_eq!(db.zpage_counts().build_starts, 0, "a form file built again");
    holds(&db, n);
    let c = settled(&db, "after the reopen");
    assert!(
        c.peak < cache + slack,
        "the cache held {} bytes of a budget of {cache} after the reopen: {c:?}",
        c.peak
    );
}

/// A form file's build rests as it goes: a partition several times its
/// cache is built within the cache and a slack, where a build that read
/// under one hold with no rest held the partition. The partition is one
/// ordered run's, twice the bound and more, and it is built by a store
/// reopened: a file being written is in memory whole until it has its
/// envelope, so a store's peak through its own load is a segment's size
/// whatever its builds do, and a reopened store's peak is its builds'.
#[test]
fn a_form_build_holds_its_cache_and_not_its_partition() {
    form_build_rests("formrests", false);
}

/// The same of the stride form, whose walk reads a kept segment's records
/// by the piece and reads its own file back through the store's cache: a
/// partition of one width of values, several times the cache, built
/// within it, the file a stride file and read by its arithmetic after.
#[test]
fn a_stride_build_holds_its_cache_and_not_its_partition() {
    form_build_rests("striderests", true);
}

/// A store whose cipher names no size for its cache follows its room as
/// the room moves under it: it holds what the room allows, gives back with
/// nobody reading when the room falls and reads right through what is
/// left, and holds more when the room rises. A cache whose size is named
/// reads no room at all.
#[test]
fn a_store_whose_cache_follows_its_room_holds_what_the_room_allows() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let _quiet = one_at_a_time();
    let n = 150_000;
    let room = Arc::new(AtomicU64::new(4 << 20));
    let opts = |room: &Arc<AtomicU64>| {
        let room = room.clone();
        Options {
            encryption: Encryption::Cipher(
                cipher(52).cache_room(move |_| Some(room.load(Ordering::SeqCst))),
            ),
            ..small()
        }
    };
    let dir = scratch("follows");
    let slack: u64 = 8 << 20;
    // What the cache may hold past its budget and the slack: what was read
    // into it while its evictor could not give back -- off its core, by the
    // kernel's count of the thread's time waiting to run, or waiting out
    // the readers' pins, by the pager's own -- at the rate the store read
    // into it over its life. The bound is soft by design: a reader past the
    // swamp line waits a breath and reads on (`Pager::breathe_swamped`), so
    // a fixed slack read a busy machine's scheduling, and failed beside
    // four busy loops with the evictor off its core for most of a scan.
    let overshoot = |c: &supdb::PagerCounts, born: std::time::Instant| -> (u64, u64, u64) {
        let delay_ns =
            std::fs::read_to_string(format!("/proc/self/task/{}/schedstat", c.evictor_tid))
                .ok()
                .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
                .unwrap_or(0);
        let off_ns = delay_ns.saturating_add(c.waited_us.saturating_mul(1000));
        let wall_ns = (born.elapsed().as_nanos() as u64).max(1);
        let read_in = c.evicted_bytes.saturating_add(c.held);
        let allowed =
            (u128::from(read_in) * u128::from(off_ns.min(wall_ns)) / u128::from(wall_ns)) as u64;
        (allowed, off_ns, wall_ns)
    };

    let until = |db: &Db, what: &str, done: &dyn Fn(&supdb::PagerCounts) -> bool| {
        let t = std::time::Instant::now();
        loop {
            let c = db.cipher_counts().unwrap().cache;
            if done(&c) {
                return c;
            }
            assert!(
                t.elapsed() < std::time::Duration::from_secs(20),
                "never: {what}: {c:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let mut db = Db::create(&dir, opts(&room)).unwrap();
    let born = std::time::Instant::now();
    fill(&mut db, n);
    db.flush().unwrap();
    db.settle().unwrap();
    let size = stored_bytes(&dir);
    assert!(
        size > 4 * (4 << 20),
        "the store is {size} bytes, not several times its room"
    );
    // Held to the same bound before the reads: a cache that never gives
    // back makes every read below breathe its whole breath, and twenty
    // thousand of them ran for minutes before the bound was read.
    let c = db.cipher_counts().unwrap().cache;
    let (allowed, off_ns, wall_ns) = overshoot(&c, born);
    assert!(
        c.peak < (4 << 20) + slack + allowed,
        "held {} through the load, of a room of four mebibytes, a slack of {slack} and {allowed} \
         read while the evictor was off for {off_ns} ns of {wall_ns}: {c:?}",
        c.peak
    );
    holds(&db, n);
    let c = until(&db, "under the first room", &|c| c.held <= c.budget);
    assert!(c.rooms > 0, "the room was never read: {c:?}");
    assert!(c.budget <= 4 << 20, "a budget past the room: {c:?}");
    let (allowed, off_ns, wall_ns) = overshoot(&c, born);
    assert!(
        c.peak < (4 << 20) + slack + allowed,
        "held {} of a room of four mebibytes, a slack of {slack} and {allowed} read while the \
         evictor was off for {off_ns} ns of {wall_ns}: {c:?}",
        c.peak
    );
    // The room falls to a half: given back with nobody reading, and the
    // store reads as itself through what is left.
    room.store(2 << 20, Ordering::SeqCst);
    let c = until(&db, "under the lower room", &|c| {
        c.budget <= 2 << 20 && c.held <= c.budget
    });
    assert!(c.held <= 2 << 20, "{c:?}");
    holds(&db, n);
    // And rises: the store holds more than the lower room let it.
    room.store(10 << 20, Ordering::SeqCst);
    until(&db, "the budget up with the room", &|c| c.budget > 5 << 20);
    holds(&db, n);
    let c = until(&db, "under the higher room", &|c| c.held <= c.budget);
    assert!(
        c.held > 3 << 20,
        "no more held under a room of ten mebibytes: {c:?}"
    );
    db.close().unwrap();
    // A size the cipher names is the budget, whatever a room says.
    let named = {
        let room = room.clone();
        Options {
            encryption: Encryption::Cipher(
                cipher(52)
                    .cache_bytes(3 << 20)
                    .cache_room(move |_| Some(room.load(Ordering::SeqCst))),
            ),
            ..small()
        }
    };
    let db = Db::open(&dir, named).unwrap();
    holds(&db, n);
    let c = db.cipher_counts().unwrap().cache;
    assert_eq!((c.budget, c.rooms), (3 << 20, 0), "{c:?}");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A walk through a form file under a cipher asks for the pages it will
/// cross as it begins, the scans' prefetch off as shipped
/// (`Options::zpages_scan_prefetch`): each is bytes of an envelope mapped
/// for reads far apart, read from the device by itself where its chunk is
/// decrypted, so a walk that asked for nothing waited once a page. In
/// either body; a walk of a few keys, inside its page, asks for nothing;
/// and the same store in the clear leaves its walks to the option.
#[test]
fn a_walk_through_a_form_file_under_a_cipher_asks_for_the_pages_it_will_cross() {
    for (stride, name) in [(true, "walkasks-stride"), (false, "walkasks-block")] {
        for clear in [false, true] {
            let dir = scratch(&format!("{name}-{clear}"));
            let n = 20_000u32;
            let opts = Options {
                encryption: if clear {
                    Encryption::None
                } else {
                    Encryption::Cipher(cipher(41))
                },
                zpages: supdb::ZPages::Always,
                zpages_min_bytes: 0,
                ..base()
            };
            assert!(!opts.zpages_scan_prefetch, "the option as shipped");
            let mut db = Db::create(&dir, opts).unwrap();
            for i in 0..n {
                db.append(&k(i), &if stride { w(i) } else { v(i, 0) });
                if i % 500 == 499 {
                    db.commit().unwrap();
                }
            }
            db.commit().unwrap();
            db.flush().unwrap();
            db.settle().unwrap();
            db.zpages_settle().unwrap();
            let z = db.zpage_counts();
            assert_eq!(
                z.stride_files > 0,
                stride,
                "{name}: the body the values call for: {z:?}"
            );
            // Short walks: inside a page, nothing to ask for.
            let before = db.zpage_counts();
            for i in (0..n).step_by(997) {
                let mut seen = 0;
                db.scan(&k(i), 2, kv(&mut |_, _| seen += 1)).unwrap();
                assert!(seen >= 1);
            }
            let short = db.zpage_counts();
            assert!(
                short.scans > before.scans,
                "{name}: no walk through the form"
            );
            // Long ones: several pages each.
            let mut walks = 0u64;
            for i in (0..n - 1000).step_by(997) {
                let mut seen = 0u32;
                db.scan(
                    &k(i),
                    600,
                    kv(&mut |key, _| {
                        assert_eq!(key, &k(i + seen)[..], "{name}: the walk's order");
                        seen += 1;
                    }),
                )
                .unwrap();
                assert_eq!(seen, 600, "{name}");
                walks += 1;
            }
            let long = db.zpage_counts();
            assert_eq!(long.fallbacks, 0, "{name}: {long:?}");
            if clear {
                assert_eq!(
                    long.prefetches, 0,
                    "{name}: a mapped file's walk is the option's: {long:?}"
                );
            } else {
                assert!(
                    short.prefetches - before.prefetches <= 2,
                    "{name}: walks of two keys asked ahead: {short:?}"
                );
                assert!(
                    long.prefetches - short.prefetches >= walks,
                    "{name}: {walks} walks of several pages asked {} times: {long:?}",
                    long.prefetches - short.prefetches
                );
            }
            drop(db);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

fn form_build_rests(name: &str, stride: bool) {
    let _quiet = one_at_a_time();
    let n = 250_000;
    let cache: u64 = 2 << 20;
    let slack: u64 = 8 << 20;
    let val = |i: u32| if stride { w(i) } else { v(i, 0) };
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(18).cache_bytes(cache)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        // Built by the form's own settle, after the reopen, and not as
        // the partition lands.
        zpages_eager: false,
        ..base()
    };
    let dir = scratch(name);
    let largest = {
        let mut db = Db::create(&dir, opts()).unwrap();
        for i in 0..n {
            db.append(&k(i), &val(i));
            if i % 500 == 499 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        assert_eq!(
            db.zpage_counts().build_starts,
            0,
            "a build before the reopen"
        );
        names(&dir)
            .iter()
            .filter(|n| n.starts_with("x-") && !n.ends_with(".z"))
            .map(|n| len_of(&dir, n))
            .max()
            .unwrap()
    };
    assert!(
        largest > 2 * (cache + slack),
        "the largest segment is {largest} bytes: a build that held it whole would pass the bound"
    );
    let mut db = Db::open(&dir, opts()).unwrap();
    db.zpages_settle().unwrap();
    let z = db.zpage_counts();
    assert!(
        z.build_starts >= 1 && z.build_failed == 0,
        "{} builds, {} failed",
        z.build_starts,
        z.build_failed
    );
    let forms = db.zpage_form_files();
    assert!(
        !forms.is_empty() && forms.iter().all(|f| !f.3),
        "the form files: {forms:?}"
    );
    // The body the values' widths call for: the stride form for one width,
    // every partition of it, and the block form for the widths `v` gives.
    assert_eq!(
        (z.stride_files, z.stride_builds > 0),
        if stride {
            (z.partitions, true)
        } else {
            (0, false)
        },
        "the bodies built: {z:?}"
    );
    // A file being written is in memory whole until it has its envelope,
    // the build's own output among them, which the cache counts and cannot
    // give back: a stride file is its partition's values as they are, the
    // better part of the segment, where the block form's is a fraction, so
    // the stride build's line is drawn past its output. A build that held
    // its segment would hold the segment beside it.
    let out: u64 = if stride {
        names(&dir)
            .iter()
            .filter(|n| n.ends_with(".z"))
            .map(|n| len_of(&dir, n))
            .sum()
    } else {
        0
    };
    assert!(
        largest > out + cache + slack,
        "premise: a segment of {largest} bytes held whole is past the line with an output of {out}"
    );
    let c = db.cipher_counts().unwrap().cache;
    assert!(
        c.peak < out + cache + slack,
        "the cache held {} bytes of a budget of {cache} through the build of a segment of {largest}, its output {out}: {c:?}",
        c.peak
    );
    for i in (0..n).step_by(997) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        assert_eq!(got, [val(i)], "key {i}");
    }
    let after = db.zpage_counts();
    assert!(after.reads > z.reads, "no read through the form");
    // A page's check is made the first time the page is read and kept: a
    // stride file under a cipher keeps it apart from the bit that would
    // keep a read from asking for the page.
    assert!(
        after.verified_pages > z.verified_pages && after.verified_pages <= after.pages,
        "the pages the reads checked, kept: {} of {}, {} before",
        after.verified_pages,
        after.pages,
        z.verified_pages
    );
}

/// The attach of a form file under a cipher decrypts what an attach reads
/// of it -- its trailer, its resident part, the sampled pages -- and not
/// the file: one partition of several megabytes, reopened.
#[test]
fn an_attach_decrypts_what_it_reads_of_a_form_file_and_not_the_file() {
    let n = 60_000;
    let dir = scratch("attach");
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(13)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        seal_bytes: 16 << 20,
        partition_bytes: Some(16 << 20),
        ..base()
    };
    let chunks = {
        let mut db = Db::create(&dir, opts()).unwrap();
        fill(&mut db, n);
        db.flush().unwrap();
        db.settle().unwrap();
        db.zpages_settle().unwrap();
        assert_eq!(db.zpage_form_files().len(), 1, "one partition, one form");
        let form = names(&dir)
            .into_iter()
            .find(|n| n.ends_with(".z"))
            .expect("a form file's envelope");
        len_of(&dir, &form) / 4096
    };
    assert!(chunks > 200, "a form file of {chunks} chunks");
    let db = Db::open(&dir, opts()).unwrap();
    assert_eq!(db.zpage_form_files().len(), 1);
    let fills = db.cipher_counts().unwrap().cache.fills;
    assert!(
        fills > 0 && fills < chunks / 3,
        "the open decrypted {fills} chunks beside a form file of {chunks}"
    );
    holds(&db, n);
    assert!(db.zpage_counts().reads > 0, "no read through the form");
}

/// A form file is built through a cache its partition does not fit: one
/// partition several times the cache and its slack, its form built by an
/// open that finds none, the builder resting as it goes so that the cache
/// gives back the records the pages before were built from. Read under
/// one hold with no rest, the build holds the partition.
#[test]
fn a_form_file_is_built_through_a_cache_its_partition_does_not_fit() {
    let _quiet = one_at_a_time();
    let n = 150_000;
    let cache: u64 = 2 << 20;
    let slack: u64 = 8 << 20;
    let dir = scratch("formbuild");
    let opts = |zpages| Options {
        encryption: Encryption::Cipher(cipher(14).cache_bytes(cache)),
        zpages,
        zpages_min_bytes: 0,
        seal_bytes: 64 << 20,
        partition_bytes: Some(64 << 20),
        ..base()
    };
    let size = {
        let mut db = Db::create(&dir, opts(supdb::ZPages::Off)).unwrap();
        fill(&mut db, n);
        db.flush().unwrap();
        db.settle().unwrap();
        stored_bytes(&dir)
    };
    assert!(
        size > cache + slack + (2 << 20),
        "the partition is {size} bytes, within the cache of {cache} and its slack"
    );
    let mut db = Db::open(&dir, opts(supdb::ZPages::Always)).unwrap();
    assert!(
        db.zpage_form_files().is_empty(),
        "a form file before a build"
    );
    db.zpages_settle().unwrap();
    assert_eq!(db.zpage_form_files().len(), 1, "one partition, one form");
    assert_eq!(db.zpage_counts().build_failed, 0);
    let c = db.cipher_counts().unwrap().cache;
    assert!(c.evicted_units > 0, "the build gave nothing back: {c:?}");
    assert!(
        c.peak < cache + slack,
        "the cache held {} bytes of a budget of {cache} through the build: {c:?}",
        c.peak
    );
    holds(&db, n);
    assert!(db.zpage_counts().reads > 0, "no read through the form");
    assert_eq!(db.zpage_counts().fallbacks, 0);
    // The build asked for its records ahead of its walk, as a plain
    // store's does: an envelope is mapped for reads far apart, and nothing
    // else reads ahead of a walk of it.
    assert!(
        db.zpage_counts().build_ahead_bytes > 0,
        "the build asked for nothing ahead"
    );
}

/// A build that drops the records behind its walk drops them where a
/// store under a cipher has them: the kernel's cache of the segment's
/// envelope, which the build has just read whole. The same partition built
/// with the drop and without it, by the share of the envelope's pages the
/// page cache holds afterwards.
#[test]
fn a_build_drops_the_ciphertext_behind_its_walk_where_it_is_told_to() {
    let _quiet = one_at_a_time();
    let n = 100_000;
    let cached = |drop_behind: bool| {
        let dir = scratch(if drop_behind {
            "dropbehind"
        } else {
            "keepbehind"
        });
        let opts = |zpages| Options {
            encryption: Encryption::Cipher(cipher(17)),
            zpages,
            zpages_min_bytes: 0,
            zpages_drop_behind: drop_behind,
            zpages_drop_raw: false,
            seal_bytes: 64 << 20,
            partition_bytes: Some(64 << 20),
            ..base()
        };
        {
            let mut db = Db::create(&dir, opts(supdb::ZPages::Off)).unwrap();
            fill(&mut db, n);
            db.flush().unwrap();
            db.settle().unwrap();
        }
        let mut db = Db::open(&dir, opts(supdb::ZPages::Always)).unwrap();
        let segment = names(&dir)
            .into_iter()
            .filter(|name| name.starts_with("x-") && !name.contains('.'))
            .max_by_key(|name| len_of(&dir, name))
            .expect("a segment's envelope");
        let len = len_of(&dir, &segment) as usize;
        db.zpages_settle().unwrap();
        assert_eq!(db.zpage_form_files().len(), 1);
        let pages = common::cached_pages(dir.join(&segment), 0, len);
        pages as f64 / len.div_ceil(4096) as f64
    };
    let (kept, dropped) = (cached(false), cached(true));
    assert!(kept > 0.8, "a build that drops nothing left {kept} cached");
    assert!(
        dropped < 0.4,
        "a build that drops behind its walk left {dropped} cached"
    );
}

/// A form file goes with its segment whether or not this open of the store
/// knew of it: built and sealed by one open, left alone by the next, which
/// runs without the compact form and rewrites every partition, it is not
/// left in the directory beside nothing.
#[test]
fn a_form_file_goes_with_its_segment_whether_or_not_this_open_knew_it() {
    let n = 12_000;
    let dir = scratch("formgone");
    let opts = |zpages| Options {
        zpages,
        zpages_min_bytes: 0,
        ..under(15)
    };
    let beside = |dir: &Path| {
        let all = names(dir);
        let forms: Vec<String> = all.iter().filter(|n| n.ends_with(".z")).cloned().collect();
        for z in &forms {
            let seg = z.strip_suffix(".z").unwrap();
            assert!(all.iter().any(|n| n == seg), "{z} beside no segment");
        }
        forms
    };
    // Each listing is read with the store open, where a store kept in one
    // file has its envelopes' names to give.
    let before = {
        let mut db = Db::create(&dir, opts(supdb::ZPages::Always)).unwrap();
        fill(&mut db, n);
        db.flush().unwrap();
        db.settle().unwrap();
        db.zpages_settle().unwrap();
        beside(&dir)
    };
    assert!(before.len() > 2, "form files to lose: {before:?}");
    let after = {
        let mut db = Db::open(&dir, opts(supdb::ZPages::Off)).unwrap();
        // Unknown to this open, and still there: a store that ran with the
        // option may run with it again.
        assert_eq!(beside(&dir), before);
        for i in 0..n {
            db.put(&k(i), &v(i, 5));
            if i % 500 == 499 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        let mut seen = 0u32;
        db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
            .unwrap();
        assert_eq!(seen, n);
        let after = beside(&dir);
        assert!(
            before.iter().filter(|z| after.contains(z)).count() < before.len(),
            "no partition was rewritten: {after:?}"
        );
        after
    };
    // And closed and opened again, nothing of them came back.
    let db = Db::open(&dir, opts(supdb::ZPages::Off)).unwrap();
    assert_eq!(beside(&dir), after);
    drop(db);
}

/// A byte changed in a form file is never read as data: the store reads
/// as itself, where the byte was in what an attach refuses the file for
/// and the partition is read raw, or refuses to be read, where the byte
/// was in a page a read wanted.
#[test]
fn a_changed_byte_in_a_form_file_takes_the_form_out_of_service_and_not_the_store() {
    form_file_damage("tamperz", false);
}

/// The same of a stride file, whose pages are asked for a page at a time
/// where a read's check of the page is made: a chunk that fails its tag
/// is a page the file does not lend, the page and the file leave service
/// there, and the read is the raw form's.
#[test]
fn a_changed_byte_in_a_stride_file_takes_the_form_out_of_service_and_not_the_store() {
    form_file_damage("tamperstride", true);
}

fn form_file_damage(name: &str, stride: bool) {
    let dir = scratch(name);
    let opts = || Options {
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        ..tiny(16)
    };
    // The form files with where their bytes lie, read while the store is
    // open: a file each in a store of files, a range of the data file in
    // a store kept in one.
    let (want, forms) = {
        let mut db = Db::create(&dir, opts()).unwrap();
        if stride {
            // Numbered keys once each, in order, a value of one width:
            // every partition a run the stride form holds.
            for i in 0..700u32 {
                db.append(&k(i), &w(i));
                if i % 30 == 29 {
                    db.commit().unwrap();
                }
            }
            db.commit().unwrap();
        } else {
            for c in 0..40u32 {
                for j in 0..30 {
                    let i = (c * 37 + j * 101) % 700;
                    db.put(&k(i), &v(i, c));
                }
                db.commit().unwrap();
            }
        }
        db.flush().unwrap();
        db.settle().unwrap();
        db.zpages_settle().unwrap();
        assert!(!db.zpage_form_files().is_empty(), "no form file to change");
        let z = db.zpage_counts();
        assert_eq!(
            z.stride_files > 0,
            stride,
            "premise: the body under test is the one built: {z:?}"
        );
        (digest_of(&db), places(&dir, |n| n.ends_with(".z")))
    };
    let pristine = snapshot(&dir);
    let bytes_of = |file: &str| -> &Vec<u8> {
        &pristine
            .iter()
            .find(|(n, _)| n == file)
            .unwrap_or_else(|| panic!("{file} is no file of the directory"))
            .1
    };
    // A form file is the store's to make again: a byte changed anywhere in
    // its envelope is the form's failure alone. The store opens, every read
    // answers the store it was -- through the raw form where the file left
    // service -- and nothing poisons the store, as for a plain store's.
    let (mut out_of_service, mut whole) = (0, 0);
    assert!(!forms.is_empty(), "no form file's envelope to change");
    for (name, file, base, len) in &forms {
        let (base, len) = (*base as usize, *len as usize);
        let stride = (len / 24).max(1);
        for at in (0..len).step_by(stride) {
            restore(&dir, &pristine);
            let mut bad = bytes_of(file).clone();
            bad[base + at] ^= 1;
            std::fs::write(dir.join(file), &bad).unwrap();
            let db = Db::open(&dir, opts()).unwrap_or_else(|e| {
                panic!("{name}, byte {at}: a form file's damage refused the store: {e}")
            });
            let d = read_digest(&db).unwrap_or_else(|e| {
                panic!("{name}, byte {at}: a form file's damage poisoned the store: {e}")
            });
            assert_eq!(d, want, "{name}, byte {at}: read as another store");
            // Out of service at its attach, which reads the file's chunks
            // and rejects it, or at a page a read found failing.
            let c = db.zpage_counts();
            if c.rejected_damage > 0 || c.damaged > 0 {
                out_of_service += 1;
            } else {
                whole += 1;
            }
        }
    }
    assert!(
        out_of_service > 0,
        "no flip took a form file out of service: whole {whole}"
    );
    // The same in service: the file attached whole, then changed in place
    // under a cache too small to keep its plaintext, so a chunk given back
    // is decrypted again from the changed envelope and fails there. The
    // read that meets it takes the file out of service (`ZFile::page_bytes`)
    // and answers through the raw form.
    restore(&dir, &pristine);
    let small = || Options {
        encryption: Encryption::Cipher(cipher(16).cache_bytes(16 << 10)),
        ..opts()
    };
    let db = Db::open(&dir, small()).unwrap();
    let keys: Vec<u32> = (0..700).collect();
    for &i in &keys {
        let mut got = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
    }
    let z = db.zpage_counts();
    assert!(
        z.reads > 0 && (z.stride_reads > 0) == stride,
        "premise: reads through the form, the body under test's: {z:?}"
    );
    // Every form file changed, a byte a chunk: which of them the store
    // serves is the controller's, and a file it does not is read by nobody.
    for (_, file, base, len) in &forms {
        use std::os::unix::fs::FileExt;
        let (base, len) = (*base as usize, *len as usize);
        let mut bad = bytes_of(file)[base..base + len].to_vec();
        for at in (4096..bad.len()).step_by(4096) {
            bad[at] ^= 1;
        }
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(file))
            .unwrap();
        f.write_all_at(&bad, base as u64).unwrap();
        f.sync_all().unwrap();
    }
    // Rounds of reads until one meets a chunk the evictor gave back, which
    // is its thread's to do in its own time: every round answers whole.
    let t = std::time::Instant::now();
    let mut round = 0;
    while db.zpage_counts().damaged == 0 && t.elapsed().as_secs() < 60 {
        for &i in &keys {
            let mut got = Vec::new();
            db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
                .unwrap_or_else(|e| panic!("round {round}, key {i}: the store refused: {e}"));
        }
        round += 1;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        read_digest(&db).unwrap(),
        want,
        "read as itself through the damage"
    );
    assert!(
        db.zpage_counts().damaged > 0,
        "a chunk that failed in service took its form file out: {:?}",
        db.zpage_counts()
    );
    drop(db);
    restore(&dir, &pristine);
    assert_eq!(digest_of(&Db::open(&dir, opts()).unwrap()), want);
}

/// The caller's key rotated under a store with data in it
/// (`supdb::cipher_rewrap`): the store opens under the new key and holds
/// what it held, the old key opens nothing, and no file but the marker
/// changed.
#[test]
fn a_store_handed_to_another_key_opens_under_that_one() {
    let n = 4_000;
    let dir = scratch("rewrap");
    {
        let mut db = Db::create(&dir, under(8)).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&dir);
    supdb::cipher_rewrap(&dir, &cipher(8), &cipher(9)).unwrap();
    let after = snapshot(&dir);
    for ((name, was), (_, is)) in before.iter().zip(&after) {
        assert_eq!(was == is, name != "supdb.cipher", "{name}");
    }
    assert_eq!(supdb::cipher_key_id(&dir).unwrap(), Some(9));
    assert!(Db::open(&dir, under(8)).is_err());
    assert_eq!(snapshot(&dir), after, "a refused open changes nothing");
    holds(&Db::open(&dir, under(9)).unwrap(), n);
}

/// A merge under a cipher that reads its inputs beside the store's cache
/// (`StoreCipher::merges_beside_cache`, `pager::Stream`) decrypts them into a
/// buffer of its own and fills nothing of them into the cache, where the
/// same merge through the cache fills every chunk of them; and the store
/// reads as itself after either. The settle that folds the fifth round's
/// piece is the merge; what else it reads (the seal's landing, the
/// indexes) is the same in both.
#[test]
fn a_merge_reading_its_inputs_beside_the_cache_fills_nothing_of_them() {
    let _quiet = one_at_a_time();
    let build = |name: &str, stream: bool| -> (u64, u64) {
        let d = scratch(name);
        let mut db = Db::create(
            &d,
            Options {
                encryption: Encryption::Cipher(
                    cipher(12).cache_bytes(1 << 20).merges_beside_cache(stream),
                ),
                seal_bytes: 4 << 20,
                partition_bytes: None,
                l0_trigger: 1,
                ..base()
            },
        )
        .unwrap();
        let value = |round: u32, k: u32| format!("value-{round}-{k}-{}", "z".repeat(60));
        for round in 0u32..4 {
            for k in 0u32..24_000 {
                db.append(
                    format!("key-{:06}", k * 4 + round).as_bytes(),
                    value(round, k).as_bytes(),
                );
            }
            db.commit().unwrap();
            db.flush().unwrap();
        }
        db.settle().unwrap();
        let before = db.cipher_counts().unwrap().cache;
        for k in 0u32..96_000 {
            db.append(format!("key-{k:06}").as_bytes(), value(9, k).as_bytes());
        }
        db.commit().unwrap();
        db.settle().unwrap();
        let after = db.cipher_counts().unwrap().cache;
        let (_, pieces) = db.levels();
        assert_eq!(
            pieces, 0,
            "{name}: the fifth round's piece was not folded in"
        );
        for k in 0u32..96_000 {
            let mut got = Vec::new();
            db.read_all(
                format!("key-{k:06}").as_bytes(),
                vf(&mut |x| got.push(x.to_vec())),
            )
            .unwrap();
            let (round, i) = (k % 4, k / 4);
            assert_eq!(
                got,
                vec![value(round, i).into_bytes(), value(9, k).into_bytes()],
                "{name}: key {k}"
            );
        }
        assert!(db.cipher_counts().is_some());
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        (after.fills - before.fills, after.streamed - before.streamed)
    };
    let (fills_on, streamed_on) = build("mergestream-on", true);
    let (fills_off, streamed_off) = build("mergestream-off", false);
    eprintln!("on: fills {fills_on} streamed {streamed_on}; off: fills {fills_off} streamed {streamed_off}");
    assert_eq!(streamed_off, 0, "nothing streamed with the option off");
    assert!(streamed_on > 0, "nothing streamed with the option on");
    assert!(
        fills_on < fills_off,
        "the merge beside the cache filled {fills_on} chunks, through it {fills_off}"
    );
}

/// A merge under a cipher asks ahead for what stands behind its cold
/// inputs (`Options::merge_read_ahead_cipher`, on as shipped), as one over
/// a plain store's files does where it is asked to: the record region's
/// place is read from the section's header however the reader holds the
/// section, where it was a span of the mapping's lent bytes, which a file
/// read through the store's cache has none of, so every window was empty
/// and nothing was asked. The inputs are made cold by a cache a twentieth
/// of them, which gives their chunks back, and by the drop of their
/// ciphertext from the kernel's cache. The window is the cipher's own
/// option: the plain store's (`merge_read_ahead`) set beside a cipher's of
/// zero asks nothing.
#[test]
fn a_merge_under_a_cipher_asks_ahead_for_its_cold_inputs() {
    let _quiet = one_at_a_time();
    let build = |name: &str, window: Option<usize>, plain: usize| -> (u64, u64, u64) {
        let d = scratch(name);
        let mut db = Db::create(
            &d,
            Options {
                encryption: Encryption::Cipher(cipher(11).cache_bytes(1 << 20)),
                seal_bytes: 4 << 20,
                partition_bytes: None,
                l0_trigger: 1,
                merge_read_ahead: plain,
                merge_read_ahead_cipher: window
                    .unwrap_or(Options::default().merge_read_ahead_cipher),
                ..base()
            },
        )
        .unwrap();
        let value = |round: u32, k: u32| format!("value-{round}-{k}-{}", "z".repeat(60));
        for round in 0u32..4 {
            for k in 0u32..24_000 {
                db.append(
                    format!("key-{:06}", k * 4 + round).as_bytes(),
                    value(round, k).as_bytes(),
                );
            }
            db.commit().unwrap();
            db.flush().unwrap();
        }
        db.settle().unwrap();
        let before = db.merge_ahead();
        let dropped = db.drop_record_pages();
        for k in 0u32..96_000 {
            db.append(format!("key-{k:06}").as_bytes(), value(9, k).as_bytes());
        }
        db.commit().unwrap();
        db.settle().unwrap();
        let (_, pieces) = db.levels();
        assert_eq!(pieces, 0, "the fifth round's piece was not folded in");
        let after = db.merge_ahead();
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
        (dropped, after.0 - before.0, after.1 - before.1)
    };
    for (name, window) in [
        ("mergeahead-cold", Some(1 << 20)),
        ("mergeahead-default", None),
    ] {
        let (dropped, asks, bytes) = build(name, window, 0);
        assert!(
            dropped > 6 << 20,
            "{name}: dropped {dropped} bytes of records"
        );
        assert!(
            asks > 0 && bytes > 0,
            "{name}: cold inputs: {asks} asks of {bytes} bytes"
        );
    }
    let (_, asks, bytes) = build("mergeahead-off", Some(0), 4 << 20);
    assert_eq!(
        (asks, bytes),
        (0, 0),
        "the cipher's option off asked, the plain store's set beside it"
    );
}

/// The envelopes of a store's segments advised as a plain store's mappings
/// are, by the kernel's own word (`rr` in `VmFlags`): random after a point
/// read, read around a fault after a scan long enough to read ahead for
/// (`Options::scan_readahead_bytes`), and so for a segment that lands after
/// such a scan. An envelope opens advised random (`pager::Back::open`), so a
/// segment landed in that phase was read a page a fault until the next
/// switch where the plain store's was read a window a fault; and a long
/// scan under a cipher is walked a thousand keys at a time
/// (`Reader::scan_rested`), each walk advised by its own span, which left a
/// scan of the whole store advised random.
#[cfg(target_os = "linux")]
#[test]
fn a_segment_landed_after_a_scan_has_its_envelope_advised_as_a_plain_ones() {
    let dir = scratch("advice-landed");
    let mut db = Db::create(&dir, under(14)).unwrap();
    fill(&mut db, 20_000);
    // The segments' envelopes mapped now, each with the kernel's advice:
    // `x-` and a token, the ordered indexes' and the form files' carrying
    // a suffix beside it.
    let envelopes = || -> Vec<(String, bool)> {
        if common::one_file_switch() {
            // In a store kept in one file an envelope is a range of the
            // data file, and its mapping one of that range: each mapping
            // of the data file is the segment's whose place it begins in.
            let places: Vec<(u64, u64, String)> = names(&dir)
                .into_iter()
                .filter(|n| n.starts_with("x-") && n.len() == 34)
                .map(|n| {
                    let (_, at, len) = common::store_place(dir.join(&n));
                    (at, len, n)
                })
                .collect();
            return common::mappings_at(&dir.join("store.one").to_string_lossy())
                .into_iter()
                .filter_map(|(_, off, rr)| {
                    places
                        .iter()
                        .find(|(at, len, _)| (*at..*at + *len).contains(&off))
                        .map(|(_, _, n)| (n.clone(), rr))
                })
                .collect();
        }
        common::mappings_advised_random(&dir.to_string_lossy())
            .into_iter()
            .filter(|(p, _)| {
                let n = p.rsplit('/').next().unwrap_or("");
                n.starts_with("x-") && n.len() == 34
            })
            .collect()
    };
    db.read_all(&k(1), vf(&mut |_| {})).unwrap();
    let read = envelopes();
    assert!(
        !read.is_empty() && read.iter().all(|(_, rr)| *rr),
        "after a point read, every envelope random: {read:?}"
    );
    // A scan whose span is past the line at which the store reads ahead
    // (`Options::scan_readahead_bytes`): a short one leaves the advice
    // random, as a point read does.
    db.scan(b"", usize::MAX, kv(&mut |_k, _v| {})).unwrap();
    let scanned = envelopes();
    assert!(
        scanned.iter().all(|(_, rr)| !*rr),
        "after a scan, every envelope read around a fault: {scanned:?}"
    );
    let before: std::collections::BTreeSet<String> =
        scanned.iter().map(|(p, _)| p.clone()).collect();
    // Keys written again, which a seal takes, and its segment lands
    // readable as a memory file before its envelope is written: keys above
    // every key would go to an ordered run, whose envelope is written as
    // the run closes, before it lands.
    for i in (0..20_000).step_by(2) {
        db.put(&k(i), &v(i, 2));
        if i % 1000 == 998 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let landed = envelopes();
    assert!(
        landed.iter().any(|(p, _)| !before.contains(p)),
        "no segment's envelope came after the scan: {landed:?}"
    );
    assert!(
        landed.iter().all(|(_, rr)| !*rr),
        "a segment landed after the scan is read as the others are: {landed:?}"
    );
    db.read_all(&k(2), vf(&mut |_| {})).unwrap();
    let again = envelopes();
    assert!(
        again.iter().all(|(_, rr)| *rr),
        "after a point read again, every envelope random: {again:?}"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An open refused beside a live store writes nothing in its directory:
/// the marker's temp name, which a creation's crash or a rewrap leaves, is
/// removed by an open that holds the store, at its sweep, and not by one
/// refused as a second open -- under a cipher the store's registry of its
/// open vaults refuses that one before it reaches the lock, by its words.
#[test]
fn a_refused_second_open_leaves_the_markers_temp_name_and_the_open_after_clears_it() {
    let d = scratch("second-open-marker-tmp");
    let mut db = Db::create(&d, under(3)).unwrap();
    db.append(b"k", b"v");
    db.commit().unwrap();
    let tmp = d.join("supdb.cipher.tmp");
    std::fs::write(&tmp, b"a rewrap's half-written marker").unwrap();
    let e = Db::open(&d, under(3)).err().expect("a second open refused");
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists, "{e}");
    assert!(
        e.to_string().contains("open in this process already"),
        "{e}"
    );
    assert!(
        tmp.exists(),
        "a refused open removed the marker's temp name"
    );
    db.close().unwrap();
    assert!(tmp.exists(), "the close removed the marker's temp name");
    let db = Db::open(&d, under(3)).unwrap();
    assert!(
        !tmp.exists(),
        "the open that holds the store left the temp name"
    );
    assert_eq!(db.count(b"k").unwrap(), 1);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}
