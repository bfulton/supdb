//! A store that is closed gives back what it allocated.
//!
//! Its own test binary, because it counts every allocation the process
//! makes: a global allocator that keeps the live byte count, and one
//! test, so no other test's allocations land in the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use supdb::{Db, Options};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: every call is passed to the system allocator unchanged; the
// counter is a side table that never touches the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            LIVE.fetch_add(new as isize - l.size() as isize, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// One store's life with the shape of a suite pass: a load, the writer
/// scanning (which builds the forms it keeps), handles that come and go
/// -- the writer publishes its forms only while one is live -- and
/// commits between them that dirty the blocks nobody is published for.
/// The next handle's arrival publishes those blocks over the forms the
/// last one was given, which retires them, and the store closes with
/// them retired and not yet swept, as a pass's closing threaded scans
/// leave it.
fn a_store_life(name: &str) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        seal_bytes: 32 << 20,
        partition_bytes: Some(64 << 20),
        scan_block_cache: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    let scan_all = |r: &supdb::Reader| {
        for s in (0..keys).step_by(500) {
            r.scan(&key(s), 100, |_, _| {}).unwrap();
        }
    };
    for k in 0..keys {
        db.append(&key(k), &[7u8; 100]);
    }
    db.commit().unwrap();
    scan_all(&db);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for round in 1..=3u64 {
        let h = db.reader().unwrap();
        scan_all(&h);
        drop(h);
        for _ in 0..4 {
            for _ in 0..500 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                db.put(&key((x % keys as u64) as u32), &[round as u8; 100]);
            }
            db.commit().unwrap();
            scan_all(&db);
        }
    }
    let h = db.reader().unwrap();
    scan_all(&h);
    drop(h);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// One store's life under `adaptive_shape`, whose `sync` hands the live
/// table to a seal without freezing it: an ordered load and its tail
/// handed, a handle reading across the landing, then shuffled updates and
/// a second hand-off of a hashed table beside them, and the store closed
/// with whatever the queue of seals still holds -- a seal in flight is
/// joined at the close and landed by no one, and its job owns the table
/// it wrote.
fn a_handed_life(name: &str) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        adaptive_shape: true,
        seal_bytes: 1 << 20,
        partition_bytes: Some(64 << 20),
        scan_block_cache: true,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    let scan_all = |r: &supdb::Reader| {
        for s in (0..keys).step_by(500) {
            r.scan(&key(s), 100, |_, _| {}).unwrap();
        }
    };
    for k in 0..keys {
        db.append(&key(k), &[7u8; 100]);
        if k % 2000 == 1999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.sync().unwrap();
    let h = db.reader().unwrap();
    scan_all(&h);
    drop(h);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..2000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        db.put(&key((x % keys as u64) as u32), &[2u8; 100]);
    }
    db.commit().unwrap();
    db.sync().unwrap();
    let h = db.reader().unwrap();
    scan_all(&h);
    drop(h);
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_closed_store_gives_back_what_it_allocated() {
    // The first life allocates what lives for the process -- the test
    // harness's buffers, lazily built statics -- so the count starts after.
    a_store_life("warm");
    a_handed_life("warm-handed");
    let before = LIVE.load(Ordering::SeqCst);
    let lives = 4;
    for i in 0..lives {
        a_store_life(&format!("life-{i}"));
        a_handed_life(&format!("handed-{i}"));
    }
    let after = LIVE.load(Ordering::SeqCst);
    let kept = after - before;
    assert!(
        kept < 64 << 10,
        "{} closed stores kept {kept} bytes allocated ({} a store)",
        2 * lives,
        kept / (2 * lives)
    );
}
