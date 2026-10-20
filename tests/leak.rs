//! A store that is closed gives back what it allocated.
//!
//! Its own test binary, because it counts every allocation the process
//! makes: a global allocator that keeps the live byte count, and one
//! test, so no other test's allocations land in the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use supdb::db::zpages::ZPAGE;
use supdb::{Db, Options};

mod common;

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

/// One store's life with three seals in flight at its close, each held
/// before it writes: the state holds three frozen tables, each with its
/// own snapshot set on it and named as a base by the live snapshots the
/// writer and a handle built over them. The close lifts the hold and
/// joins the seals, and every table and snapshot goes with the store.
fn a_three_frozen_life(name: &str) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        seal_bytes: 1 << 30,
        partition_bytes: Some(64 << 10),
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
    db.flush().unwrap();
    db.hold_seal_landing(true);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for round in 1..=4u64 {
        for _ in 0..1000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            db.put(&key((x % keys as u64) as u32), &[round as u8; 100]);
        }
        db.commit().unwrap();
        scan_all(&db);
        let h = db.reader().unwrap();
        scan_all(&h);
        drop(h);
        if round < 4 {
            db.seal().unwrap();
        }
    }
    assert_eq!(db.frozen_tables(), 3, "three frozen at the close");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

/// One store's life under the second form, forced: a load into partitions,
/// each given its form file, reads and counts from them on the writer and on
/// a handle's thread (whose buffer for a decoded group goes with the
/// thread), updates spread over every range and a drain, which merge the
/// partitions again and take their form files with them, a handle pinned at
/// a snapshot across such a round, which keeps the form files it reads mapped
/// after their retirement and past the store's close, and a reopen that
/// attaches what the merges left. The form files are mappings, which the
/// allocator does not count; what it counts is everything else a form file
/// is: the cell, the fence, the filter, the bitmaps, the trailer, and the
/// retire list's entries. The close frees them with their segments, and
/// what a pin held goes with the pin; on Linux the process's mappings say
/// so, read from the kernel after each.
fn a_zpages_life(name: &str) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        // The nocache policy, the form's and the arm's: nothing here scans.
        scan_block_cache: false,
        seal_bytes: 1 << 20,
        partition_bytes: Some(256 << 10),
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    // Values of random bytes, so that a partition's form file is stored pages
    // and more than the sixteen an attach samples; a value of one byte
    // repeated made a form file of ten.
    let value = |k: u32| -> Vec<u8> {
        let mut x = (u64::from(k) + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (0..100)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    };
    for k in 0..keys {
        db.append(&key(k), &value(k));
        if k % 2000 == 1999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.zpages_settle().unwrap();
    fn read_some(r: &supdb::Reader, keys: u32) {
        for k in (0..keys).step_by(97) {
            let key = format!("key-{k:08}").into_bytes();
            r.read_all(&key, |_| {}).unwrap();
            r.count(&key).unwrap();
        }
    }
    read_some(&db, keys);
    let h = db.reader().unwrap();
    std::thread::spawn(move || read_some(&h, keys))
        .join()
        .unwrap();
    assert!(db.zpage_counts().reads > 0 && db.zpage_counts().form_files > 1);
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for round in 1..=3u64 {
        for _ in 0..1500 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            db.put(&key((x % keys as u64) as u32), &[round as u8; 100]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.zpages_settle().unwrap();
        read_some(&db, keys);
    }
    // A handle pinned at a snapshot across two retirements: a round whose
    // merges retire the form files with their segments, and a form file taken
    // out of a live segment's cell after a page of it failed a check, which
    // waits in the retire list for the pin. The store is closed with the
    // pin still held, so neither is swept: the handle's drop is what frees
    // them, and nothing of the directory stays mapped after it.
    let pinned = db.reader().unwrap();
    pinned.snapshot();
    read_some(&pinned, keys);
    #[cfg(target_os = "linux")]
    let mapped_before = common::zpg_mappings(&d).len();
    for _ in 0..1500 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        db.put(&key((x % keys as u64) as u32), &[4u8; 100]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.zpages_settle().unwrap();
    #[cfg(target_os = "linux")]
    {
        let live = db.zpage_counts().form_files as usize;
        let mapped = common::zpg_mappings(&d).len();
        assert!(
            mapped > live && mapped >= mapped_before,
            "{mapped} mapped, {live} live, {mapped_before} before: the pin holds what the merges retired"
        );
    }
    // A page no attach samples -- the second, of a form file of more than
    // sixteen pages, just built and read by nobody -- flipped in place
    // under the mapping: the reads meet the check, and the settle takes the
    // form file out of its cell into the retire list and builds another.
    let mut files: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(&d)
        .unwrap()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("zpg-") && n.ends_with(".zpg")
        })
        .map(|e| (e.metadata().unwrap().len(), e.path()))
        .collect();
    files.sort();
    let (len, file) = files.pop().expect("a form file");
    assert!(len > 20 * ZPAGE as u64, "a form file of {len} bytes");
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file)
            .unwrap();
        let mut byte = [0u8; 1];
        f.read_at(&mut byte, ZPAGE as u64 + 100).unwrap();
        byte[0] ^= 0x40;
        f.write_at(&byte, ZPAGE as u64 + 100).unwrap();
    }
    let before = db.zpage_counts();
    for k in 0..keys {
        db.read_all(&key(k), |_| {}).unwrap();
    }
    let c = db.zpage_counts();
    assert_eq!((c.damaged, c.bad_pages), (1, 1), "the flipped page: {c:?}");
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!(
        (c.rebuilds - before.rebuilds, c.damaged, c.form_files),
        (1, 0, before.form_files),
        "retired into the list and built again: {c:?}"
    );
    read_some(&db, keys);
    read_some(&pinned, keys);
    drop(db);
    #[cfg(target_os = "linux")]
    assert!(
        !common::zpg_mappings(&d).is_empty(),
        "the pin holds its form files past the store's close"
    );
    // Dropped without a release: no operation sweeps after the pin, and
    // what the handle held goes with it.
    drop(pinned);
    #[cfg(target_os = "linux")]
    assert!(
        common::zpg_mappings(&d).is_empty(),
        "no mapping of the store's form files survives the last handle: {:?}",
        common::zpg_mappings(&d)
    );
    let db = Db::open(&d, opts).unwrap();
    assert!(db.zpage_counts().attached > 0);
    read_some(&db, keys);
    drop(db);
    #[cfg(target_os = "linux")]
    assert!(
        common::zpg_mappings(&d).is_empty(),
        "no mapping of the store's form files survives its close"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_closed_store_gives_back_what_it_allocated() {
    // The first life allocates what lives for the process -- the test
    // harness's buffers, lazily built statics, the buffer a thread that
    // reads from a form file decodes into -- so the count starts after.
    a_store_life("warm");
    a_handed_life("warm-handed");
    a_three_frozen_life("warm-three");
    a_zpages_life("warm-zpages");
    let before = LIVE.load(Ordering::SeqCst);
    let lives = 4;
    for i in 0..lives {
        a_store_life(&format!("life-{i}"));
        a_handed_life(&format!("handed-{i}"));
        a_three_frozen_life(&format!("three-{i}"));
        a_zpages_life(&format!("zpages-{i}"));
    }
    let after = LIVE.load(Ordering::SeqCst);
    let kept = after - before;
    assert!(
        kept < 64 << 10,
        "{} closed stores kept {kept} bytes allocated ({} a store)",
        4 * lives,
        kept / (4 * lives)
    );
}
