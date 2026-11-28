//! A closed store's blocks go back to the kernel.
//!
//! Its own test binary, with one test, because what it reads is the
//! process's: the anonymous bytes resident, which every other test's
//! stores move, and the count of the engine's mapped blocks, which every
//! store in the process maps into. Linux only, for the kernel's count of
//! the anonymous bytes (`RssAnon`); elsewhere the binary holds no test.

#![cfg(target_os = "linux")]

use supdb::{Db, Options};

mod common;
use common::kv;

/// The anonymous bytes this process has resident, from the kernel.
fn anon_resident() -> u64 {
    let text = std::fs::read_to_string("/proc/self/status").unwrap();
    supdb::db::zselect::parse_status_rss_anon(&text).expect("RssAnon in /proc/self/status")
}

/// A store whose table holds tens of megabytes maps that much; at its close the blocks are unmapped and
/// the process's anonymous bytes fall by them, where the allocator would
/// have kept the heap's. The write is a batch of appends and one commit,
/// a seal never reached (`seal_bytes` above the table), so the table lives
/// until the close and nothing but the close frees it.
#[test]
fn a_closed_stores_blocks_are_not_resident() {
    let d = std::env::temp_dir().join(format!("supdb-next-mapped-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        seal_bytes: 1 << 30,
        ..common::options()
    };
    let mapped_before = Db::mapped_bytes();
    let anon_before = anon_resident();
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 400_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    for k in 0..keys {
        db.append(&key(k), &[7u8; 100]);
    }
    db.commit().unwrap();
    db.scan(&key(0), 10, kv(&mut |_, _| {})).unwrap();
    let mapped_open = Db::mapped_bytes() - mapped_before;
    let resident_open = Db::mapped_resident_bytes();
    let anon_open = anon_resident();
    // The kernel's count of the blocks' resident pages: no more than the
    // blocks, and most of them once written through.
    assert!(
        resident_open <= Db::mapped_bytes() && resident_open >= 40 << 20,
        "the blocks' resident pages {resident_open} of {} mapped",
        Db::mapped_bytes()
    );
    // The keys and values alone are 46 MB, in blocks of four megabytes;
    // the slabs and the index are more.
    assert!(
        mapped_open >= 48 << 20,
        "a store of {keys} keys maps {mapped_open} bytes"
    );
    assert!(
        anon_open >= anon_before + (40 << 20),
        "the mapped blocks are resident once written: {anon_before} -> {anon_open}"
    );
    drop(db);
    let mapped_closed = Db::mapped_bytes() - mapped_before;
    let anon_closed = anon_resident();
    assert_eq!(mapped_closed, 0, "the closed store's blocks are unmapped");
    assert!(
        anon_closed + (32 << 20) <= anon_open,
        "the closed store's blocks are not resident: {anon_open} -> {anon_closed}"
    );
    let _ = std::fs::remove_dir_all(&d);
    a_closed_store_under_a_cipher_holds_no_memory_file();
}

/// The memory files of this process, as its descriptors and its mappings
/// name them.
fn memory_files() -> (usize, usize) {
    let fds = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|t| t.to_string_lossy().starts_with("/memfd:supdb"))
        .count();
    let maps = std::fs::read_to_string("/proc/self/maps")
        .unwrap()
        .lines()
        .filter(|l| l.contains("/memfd:supdb"))
        .count();
    (fds, maps)
}

/// A store under a cipher reads its files through mappings of memory
/// files (`Encryption::Cipher`, `db::pager`), which are the process's until
/// their last descriptor and their last mapping go. An open store, with
/// everything it wrote in the directory as its envelope, maps its files
/// and holds a descriptor for none of them; a closed one has neither.
/// Called from the one test, since the count is the process's.
fn a_closed_store_under_a_cipher_holds_no_memory_file() {
    let d = std::env::temp_dir().join(format!("supdb-next-mapped-cipher-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(memory_files(), (0, 0));
    let cipher = supdb::StoreCipher::new(common::Gcm::new(&[9; 32]), common::Gcm::new);
    let opts = || Options {
        encryption: supdb::Encryption::Cipher(cipher.clone()),
        seal_bytes: 256 << 10,
        ..Options::default()
    };
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    for round in 0..2 {
        let mut db = if round == 0 {
            Db::create(&d, opts()).unwrap()
        } else {
            Db::open(&d, opts()).unwrap()
        };
        for k in 0..30_000u32 {
            db.put(&key(k), &[7u8; 100]);
            if k % 1000 == 999 {
                db.commit().unwrap();
            }
        }
        db.flush().unwrap();
        db.settle().unwrap();
        let handle = db.reader().unwrap();
        let (fds, maps) = memory_files();
        assert!(
            fds == 0 && maps > 0,
            "an open store's files: {fds} held, {maps} mapped"
        );
        drop(db);
        // A handle outlives the store it was claimed from, and the
        // mappings it reads through with it.
        drop(handle);
        assert_eq!(memory_files(), (0, 0), "round {round}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
