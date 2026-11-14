//! A store under a key: its directory encrypted by the kernel
//! (`Options::dir_key`, `supdb::dirkey`).
//!
//! Two halves, since a test cannot make a filesystem. The first runs
//! wherever the tests do and holds the refusals: a store given a key where
//! the directory cannot be encrypted writes nothing, and a key given to a
//! store in the clear changes nothing. The second needs a directory on a
//! filesystem that can encrypt and the file that filesystem is in, to read
//! the device's own bytes: `SUPDB_FSCRYPT_DIR` and `SUPDB_FSCRYPT_IMAGE`,
//! which `scripts/fscrypt.sh up` makes. Those tests are ignored by default
//! and run by `scripts/check.sh fscrypt`, which names each one it expects
//! to have run; called without the two variables they fail, they do not
//! pass.
//!
//! Each test has a key of its own: a key is the filesystem's once it is
//! handed over, and a test that gave up another's would lock its store
//! under it.

use std::path::{Path, PathBuf};
use supdb::dirkey::{self, DirKey, DirState};
use supdb::{Db, Options};

mod common;
use common::{kv, vf};

fn key(n: u8) -> DirKey {
    DirKey::new(std::array::from_fn(|i| {
        n.wrapping_mul(31).wrapping_add(i as u8 * 7)
    }))
}

fn under(key: &DirKey) -> Options {
    Options {
        dir_key: Some(key.clone()),
        ..small()
    }
}

/// Seals of a quarter megabyte, so that a test's few megabytes leave a
/// log, pieces, merged partitions and their indexes behind.
fn small() -> Options {
    Options {
        seal_bytes: 256 << 10,
        ..Options::default()
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-dirkey-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

const MARK: &str = "IN-THE-CLEAR";

fn k(i: u32) -> Vec<u8> {
    format!("key-{MARK}-{i:08}").into_bytes()
}

fn v(i: u32, round: u32) -> Vec<u8> {
    format!(
        "value {i} of round {round}, {MARK}, {}",
        "x".repeat((i % 50) as usize)
    )
    .into_bytes()
}

/// A few megabytes through every way the store writes: keys in order, which
/// go straight into a segment; the same keys again out of order, through
/// the log; and enough of both for seals and the merges behind them.
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

// ------------------------------------------------------------- anywhere --

/// A store given a key is encrypted or is not made: where the directory
/// cannot be encrypted, nothing is written into it.
#[test]
fn a_store_given_a_key_where_none_can_be_set_writes_nothing() {
    let dir = scratch("nowhere");
    match Db::create(&dir, under(&key(1))) {
        Err(e) => {
            assert!(e.to_string().contains("the store at"), "{e}");
            assert_eq!(names(&dir), Vec::<String>::new(), "a refused create wrote");
            assert_eq!(dirkey::state(&dir).unwrap(), DirState::Plain);
        }
        // The temporary directory is on a filesystem that can: then the
        // store is under its key, which is the other thing it may be.
        Ok(db) => {
            assert_eq!(dirkey::state(&dir).unwrap(), DirState::Unlocked);
            drop(db);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key given to a store in the clear is refused before anything is read
/// or written: the store would go on in the clear under a caller who
/// believes otherwise.
#[test]
fn a_key_given_to_a_store_in_the_clear_is_refused_and_changes_nothing() {
    let dir = scratch("clear");
    let mut db = Db::create(&dir, small()).unwrap();
    fill(&mut db, 2_000);
    db.close().unwrap();
    assert_eq!(dirkey::state(&dir).unwrap(), DirState::Plain);
    let before = names(&dir);

    let e = Db::open(&dir, under(&key(2)))
        .err()
        .expect("opened under a key");
    assert!(e.to_string().contains("the store at"), "{e}");
    assert_eq!(names(&dir), before, "a refused open changed the directory");

    let db = Db::open(&dir, small()).unwrap();
    holds(&db, 2_000);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

// --------------------------------------- on a filesystem that can encrypt --

fn encrypting() -> (PathBuf, PathBuf) {
    let var = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| {
            panic!("{name} is not set: these tests run through `scripts/check.sh fscrypt`")
        })
    };
    (
        var("SUPDB_FSCRYPT_DIR").into(),
        var("SUPDB_FSCRYPT_IMAGE").into(),
    )
}

fn on_the_filesystem(name: &str) -> PathBuf {
    let d = encrypting()
        .0
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Everything the filesystem holds, written through to its file.
fn written_through(dir: &Path) {
    use std::os::fd::AsRawFd;
    let d = std::fs::File::open(dir).unwrap();
    // SAFETY: a sync of the filesystem a descriptor this function owns is on.
    assert_eq!(unsafe { libc::syncfs(d.as_raw_fd()) }, 0);
}

/// Which of `needles` the filesystem's own bytes hold, as the device has
/// them. Read a megabyte at a time, each piece with the tail of the one
/// before it, and a piece of nothing but zeroes, which is most of a
/// filesystem in a sparse file, is not searched.
fn on_the_device(needles: &[&[u8]]) -> Vec<bool> {
    use std::io::Read;
    let longest = needles.iter().map(|n| n.len()).max().unwrap();
    let mut image = std::fs::File::open(encrypting().1).expect("the filesystem's file");
    let mut found = vec![false; needles.len()];
    let mut buf = vec![0u8; longest + (1 << 20)];
    let mut tail = 0usize;
    loop {
        let n = image.read(&mut buf[tail..]).unwrap();
        if n == 0 {
            break;
        }
        let have = tail + n;
        if buf[..have].iter().any(|&b| b != 0) {
            for (needle, hit) in needles.iter().zip(found.iter_mut()) {
                *hit = *hit || buf[..have].windows(needle.len()).any(|w| w == *needle);
            }
        }
        tail = have.min(longest - 1);
        buf.copy_within(have - tail..have, 0);
    }
    found
}

/// A store under a key: it reads as itself, and the device holds none of
/// its keys or its values. Beside it, a file written in the clear on the
/// same filesystem, which the same search of the device does find: the
/// search can see.
#[test]
#[ignore = "needs a filesystem that can encrypt: scripts/check.sh fscrypt"]
fn a_store_under_a_key_is_ciphertext_on_the_device_and_reads_as_itself() {
    let (dir, key) = (on_the_filesystem("store"), key(11));
    let n = 20_000;
    let mut db = Db::create(&dir, under(&key)).expect("a store under a key");
    assert_eq!(dirkey::state(&dir).unwrap(), DirState::Unlocked);
    fill(&mut db, n);
    holds(&db, n);
    db.close().unwrap();

    // Every kind of file the store keeps is there, by its own name.
    let files = names(&dir);
    for kind in ["wal-", "manifest", "par-"] {
        assert!(
            files.iter().any(|f| f.starts_with(kind)),
            "no {kind} file in {files:?}"
        );
    }

    // Open again under the key, and with none while the kernel holds it.
    let db = Db::open(&dir, under(&key)).expect("open under the key");
    holds(&db, n);
    drop(db);
    let db = Db::open(&dir, small()).expect("open with the key already held");
    holds(&db, n);
    drop(db);

    let clear = encrypting()
        .0
        .join(format!("clear-{}.txt", std::process::id()));
    std::fs::write(
        &clear,
        format!("a file {MARK}-AND-FOUND, beside the store").repeat(200),
    )
    .unwrap();
    written_through(&dir);
    let (control, a_key, a_value) = (
        format!("{MARK}-AND-FOUND"),
        format!("key-{MARK}-"),
        format!(", {MARK}, "),
    );
    // Its keys and its values, which are this test's own words. Not its
    // files' names: every store's are the same, so one store in the clear
    // anywhere on the filesystem puts them on the device, and the names are
    // held by the locked store's listing instead.
    let found = on_the_device(&[control.as_bytes(), a_key.as_bytes(), a_value.as_bytes()]);
    assert!(
        found[0],
        "the control: a file in the clear is not found on the device"
    );
    assert!(!found[1], "a key in the clear");
    assert!(!found[2], "a value in the clear");
    let _ = std::fs::remove_file(&clear);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key given up: the store is refused by name, its files' names are not
/// its own, a wrong key is refused and leaves it locked, and its own key
/// opens it as it was.
#[test]
#[ignore = "needs a filesystem that can encrypt: scripts/check.sh fscrypt"]
fn a_locked_store_is_refused_by_name_and_opens_under_its_key() {
    let (dir, key, other) = (on_the_filesystem("locked"), key(12), key(13));
    let n = 6_000;
    let mut db = Db::create(&dir, under(&key)).unwrap();
    fill(&mut db, n);
    db.close().unwrap();
    let unlocked = names(&dir);

    // A file under the key still open keeps the kernel from forgetting it.
    let held = std::fs::File::open(dir.join(&unlocked[0])).unwrap();
    assert!(
        !dirkey::lock(&dir, &key).unwrap(),
        "given up under an open file"
    );
    drop(held);
    assert!(dirkey::lock(&dir, &key).unwrap(), "not given up");
    assert_eq!(dirkey::state(&dir).unwrap(), DirState::Locked);
    assert!(
        dirkey::lock(&dir, &key).unwrap(),
        "given up twice is given up"
    );

    let locked = names(&dir);
    assert_eq!(locked.len(), unlocked.len());
    assert!(
        locked.iter().all(|f| !unlocked.contains(f)),
        "a name readable: {locked:?}"
    );

    let e = Db::open(&dir, small()).err().expect("opened with no key");
    assert!(e.to_string().contains("its key is not held"), "{e}");
    let e = Db::open(&dir, under(&other))
        .err()
        .expect("opened under another key");
    assert!(e.to_string().contains("another key"), "{e}");
    assert_eq!(
        dirkey::state(&dir).unwrap(),
        DirState::Locked,
        "a wrong key unlocked it"
    );
    assert_eq!(names(&dir), locked, "a refused open changed the directory");

    let mut db = Db::open(&dir, under(&key)).expect("open under its key");
    assert_eq!(names(&dir), unlocked);
    holds(&db, n);
    // And it goes on as a store: more writes, another seal, another open.
    for i in n..n + 2_000 {
        db.append(&k(i), &v(i, 0));
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.close().unwrap();
    let db = Db::open(&dir, under(&key)).unwrap();
    let mut seen = 0u32;
    db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
        .unwrap();
    assert_eq!(seen, n + 2_000);
    drop(db);
    assert!(dirkey::lock(&dir, &key).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A directory already under a key takes a store under that key and
/// refuses one under another: a directory made inside an encrypted one is
/// under its key from birth.
#[test]
#[ignore = "needs a filesystem that can encrypt: scripts/check.sh fscrypt"]
fn a_directory_under_one_key_refuses_a_store_under_another() {
    let (outer, key, other) = (on_the_filesystem("outer"), key(14), key(15));
    let db = Db::create(&outer, under(&key)).unwrap();
    drop(db);

    let inner = outer.join("inner");
    let e = Db::create(&inner, under(&other))
        .err()
        .expect("made under another key");
    assert!(e.to_string().contains("another policy"), "{e}");
    assert_eq!(
        names(&inner),
        Vec::<String>::new(),
        "a refused create wrote"
    );
    // The other key was handed to the kernel to be told apart, and was
    // given back: it unlocks nothing here and is not left held.
    assert!(dirkey::lock(&outer, &other).unwrap());

    let mut db = Db::create(&inner, under(&key)).expect("the same key again");
    fill(&mut db, 1_000);
    holds(&db, 1_000);
    drop(db);
    let _ = std::fs::remove_dir_all(&outer);
    assert!(dirkey::lock(&encrypting().0, &key).unwrap());
}
