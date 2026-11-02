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

/// A store whose table holds tens of megabytes maps that much
/// (`Options::blocks_mapped`); at its close the blocks are unmapped and
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
        ..Options::default()
    };
    assert!(opts.blocks_mapped, "the default maps");
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
    let anon_open = anon_resident();
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
}
