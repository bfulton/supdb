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
use common::{kv, vf};

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
        ..common::options()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    let scan_all = |r: &supdb::Reader| {
        for s in (0..keys).step_by(500) {
            r.scan(&key(s), 100, kv(&mut |_, _| {})).unwrap();
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
        ..common::options()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    let scan_all = |r: &supdb::Reader| {
        for s in (0..keys).step_by(500) {
            r.scan(&key(s), 100, kv(&mut |_, _| {})).unwrap();
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
        ..common::options()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    let scan_all = |r: &supdb::Reader| {
        for s in (0..keys).step_by(500) {
            r.scan(&key(s), 100, kv(&mut |_, _| {})).unwrap();
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
        // The page flipped below is met by the check a page takes at its
        // first touch, which a stride file this process built is trusted
        // past (`Options::zpages_stride_trust_built`): under the stride
        // switch, whose keys and values here the stride form holds, the
        // check is asked for, so the flip reaches the retire list.
        zpages_stride_trust_built: false,
        // The form files are found, flipped and counted in the process's
        // mappings by their names in the directory: a store of files
        // (`common::files`; `a_one_file_life` is the store in one file).
        ..common::files(common::options())
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
            r.read_all(&key, vf(&mut |_| {})).unwrap();
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
        db.read_all(&key(k), vf(&mut |_| {})).unwrap();
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

/// A store under the stride form: its partitions' stride files, reopened so
/// that each is a file found on disk and checked page by page as the reads
/// touch them; a handle pinned at a snapshot; a value page flipped in place
/// under the mappings once the open has attached them, which a read meets
/// at the page's first touch and the settle takes out of its live cell into
/// the retire list -- the path a merge never reaches, the form file
/// outliving the segment that held it -- and builds again. The store closes
/// with the pin held, so nothing sweeps: the handle's drop frees what the
/// list held, and nothing of the directory stays mapped after it.
fn a_stride_life(name: &str) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        zpages: supdb::ZPages::Always,
        zpages_stride: supdb::ZStride::Forced,
        zpages_min_bytes: 0,
        scan_block_cache: false,
        seal_bytes: 1 << 20,
        partition_bytes: Some(256 << 10),
        // Its stride files by their names, as `a_zpages_life`'s.
        ..common::files(common::options())
    };
    let keys = 12_000u32;
    let key = |k: u32| format!("k{k:06}").into_bytes();
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
    {
        let mut db = Db::create(&d, opts.clone()).unwrap();
        for k in 0..keys {
            db.append(&key(k), &value(k));
            if k % 2000 == 1999 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.zpages_settle().unwrap();
        let c = db.zpage_counts();
        assert!(
            c.stride_files > 1 && c.stride_files == c.form_files,
            "{c:?}"
        );
        db.close().unwrap();
    }
    let mut db = Db::open(&d, opts.clone()).unwrap();
    let read_all = |r: &supdb::Reader| {
        for k in 0..keys {
            let mut n = 0;
            r.read_all(
                &key(k),
                vf(&mut |v| {
                    assert_eq!(v, value(k).as_slice());
                    n += 1;
                }),
            )
            .unwrap();
            assert!(n <= 1);
        }
    };
    let pinned = db.reader().unwrap();
    pinned.snapshot();
    // The largest stride file's second value page. The open attached every
    // file before this, so what meets the flip is a read's first touch of
    // the page and never the attach's sample.
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
    let (len, file) = files.pop().expect("a stride file");
    assert!(len > 20 * ZPAGE as u64, "a stride file of {len} bytes");
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file)
            .unwrap();
        let at = ZPAGE as u64 + 100;
        let mut byte = [0u8; 1];
        f.read_at(&mut byte, at).unwrap();
        byte[0] ^= 0x40;
        f.write_at(&byte, at).unwrap();
    }
    let before = db.zpage_counts();
    read_all(&db);
    let c = db.zpage_counts();
    assert_eq!((c.damaged, c.bad_pages), (1, 1), "the flipped page: {c:?}");
    assert!(
        c.stride_page_checks > 0,
        "pages found on disk, checked: {c:?}"
    );
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!(
        (c.rebuilds - before.rebuilds, c.damaged, c.stride_files),
        (1, 0, before.stride_files),
        "retired into the list and built again: {c:?}"
    );
    read_all(&db);
    read_all(&pinned);
    drop(db);
    // The retired file by its own mapping: the retire renamed it to a dead
    // name (`zfile::zpg_dead_name`) and the settle's sweep unlinked that, so
    // the pinned mapping shows the dead name deleted. The pinned state maps
    // the rebuilt files too, under their names, and those alone are no sign
    // of the retired one.
    #[cfg(target_os = "linux")]
    {
        let maps = common::zpg_mappings(&d);
        assert!(
            maps.iter()
                .any(|p| p.contains("/zpg-dead-") && p.ends_with(" (deleted)")),
            "the pin holds the retired stride file, under its dead name, past the store's close: {maps:?}"
        );
    }
    drop(pinned);
    #[cfg(target_os = "linux")]
    assert!(
        common::zpg_mappings(&d).is_empty(),
        "no mapping of the store's stride files survives the last handle: {:?}",
        common::zpg_mappings(&d)
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// The mappings this process holds of the data file of the store in `d`
/// (`Options::one_file`), as `/proc/self/maps` names them.
#[cfg(target_os = "linux")]
fn data_file_mappings(d: &std::path::Path) -> usize {
    let name = d.join("store.one");
    let name = name.to_string_lossy();
    std::fs::read_to_string("/proc/self/maps")
        .unwrap()
        .lines()
        .filter(|l| l.contains(&*name))
        .count()
}

/// A store kept in one file (`Options::one_file`) with its form forced:
/// seals, merges that retire segments, their indexes and their form files
/// into room the next ones are written over, a handle pinned at a snapshot
/// across a round of them, and the store closed with the pin still held.
/// The files the pin reads are let go by the handle's drop, the table
/// with the last of them, and nothing of the data file stays mapped after
/// it: the table, an entry a file and the hold a mapping keeps on its
/// file are what this life adds to the count.
fn a_one_file_life(name: &str) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        one_file: true,
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        scan_block_cache: false,
        seal_bytes: 256 << 10,
        partition_bytes: Some(256 << 10),
        l0_trigger: 2,
        ..common::options()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let keys = 12_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    fn read_some(r: &supdb::Reader, keys: u32) {
        for k in (0..keys).step_by(97) {
            let key = format!("key-{k:08}").into_bytes();
            r.read_all(&key, vf(&mut |_| {})).unwrap();
            r.count(&key).unwrap();
        }
    }
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut round = |db: &mut Db, tag: u8| {
        for _ in 0..keys {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            db.put(&key((x % keys as u64) as u32), &[tag; 100]);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.zpages_settle().unwrap();
    };
    for tag in 0..3u8 {
        round(&mut db, tag);
        read_some(&db, keys);
    }
    let h = db.reader().unwrap();
    std::thread::spawn(move || read_some(&h, keys))
        .join()
        .unwrap();
    let c = db.one_counts().expect("a store in one file");
    assert!(c.files > 2 && c.reused > 0, "{c:?}");
    assert!(db.zpage_counts().reads > 0 && db.zpage_counts().form_files > 1);
    // The pin, and a round whose merges retire what it reads.
    let pinned = db.reader().unwrap();
    pinned.snapshot();
    read_some(&pinned, keys);
    round(&mut db, 9);
    read_some(&db, keys);
    read_some(&pinned, keys);
    assert!(
        db.one_counts().unwrap().files > db.segments() as u64,
        "the pin holds files the merges retired: {:?}",
        db.one_counts()
    );
    drop(db);
    #[cfg(target_os = "linux")]
    assert!(
        data_file_mappings(&d) > 0,
        "the pin holds its files past the store's close"
    );
    drop(pinned);
    #[cfg(target_os = "linux")]
    assert_eq!(
        data_file_mappings(&d),
        0,
        "no mapping of the data file survives the last handle"
    );
    let db = Db::open(&d, opts).unwrap();
    assert!(db.one_counts().is_some());
    read_some(&db, keys);
    drop(db);
    #[cfg(target_os = "linux")]
    assert_eq!(
        data_file_mappings(&d),
        0,
        "no mapping of the data file survives its close"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A store's blocks come from the kernel and the allocator the count
/// below sees nothing of them, so they are counted on their own
/// (`Db::mapped_bytes`): zero past every store's close.
/// One store's life that shares its live table on demand
/// (`Options::share_on_write`): scans that build the writer's block
/// tables, a handle whose writes install a shared table over a table with
/// writes in it -- the tables dropped at the flip, the frozen table
/// sealed beside the shared one -- the writer's own writes and scans
/// beside the handle, the handle gone and the shared table left once its
/// rent is paid, the block cache built again over what landed, and a
/// second handle's shared table live at the drop, with `left` a table
/// frozen and still on its way to a segment beside it.
fn a_sharing_life(name: &str, left: bool) {
    let d = std::env::temp_dir().join(format!("supdb-next-leak-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        sync: supdb::SyncPolicy::Explicit,
        held_frames: false,
        share_on_write: true,
        seal_bytes: 1 << 20,
        seal_max_pct: 0,
        partition_bytes: Some(256 << 10),
        scan_block_cache: true,
        ..common::options()
    };
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 20_000u32;
    let key = |k: u32| format!("key-{k:08}").into_bytes();
    let scan_all = |r: &supdb::Reader| {
        for s in (0..keys).step_by(500) {
            r.scan(&key(s), 100, kv(&mut |_, _| {})).unwrap();
        }
    };
    for k in 0..keys {
        db.append(&key(k), &[7u8; 100]);
    }
    db.flush().unwrap();
    db.settle().unwrap();
    for k in (0..keys).step_by(9) {
        db.put(&key(k), &[8u8; 100]);
    }
    db.commit().unwrap();
    scan_all(&db);
    for round in 0..2u32 {
        let h = db.reader().unwrap();
        for k in (round..keys).step_by(5) {
            while !h.put(&key(k), &[round as u8; 100]).unwrap() {
                db.commit().unwrap();
            }
            if k % 400 < 5 {
                db.put(&key(k + 1), &[9u8; 100]);
                db.commit().unwrap();
            }
        }
        assert!(db.sharing(), "{name}: the handle's writes shared no table");
        scan_all(&db);
        scan_all(&h);
        drop(h);
        if round == 1 {
            break;
        }
        // The rent, and the store back to the one writer's table.
        for i in 0..keys * 4 {
            if !db.live_shared() {
                break;
            }
            db.put(&key(i % keys), &[10u8; 100]);
            if i % 100 == 99 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.settle().unwrap();
        assert!(!db.sharing(), "{name}: the store stayed shared");
        scan_all(&db);
    }
    if left {
        // A shared table frozen, its seal still running, as the store goes.
        db.commit().unwrap();
        db.seal().unwrap();
    }
    drop(db);
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
    a_stride_life("warm-stride");
    a_one_file_life("warm-onefile");
    a_sharing_life("warm-sharing", false);
    a_sharing_life("warm-sharing-left", true);
    assert_eq!(
        Db::mapped_bytes(),
        0,
        "mapped blocks past the warm stores' close"
    );
    let before = LIVE.load(Ordering::SeqCst);
    let lives = 4;
    let kinds = 8;
    for i in 0..lives {
        a_store_life(&format!("life-{i}"));
        a_handed_life(&format!("handed-{i}"));
        a_three_frozen_life(&format!("three-{i}"));
        a_zpages_life(&format!("zpages-{i}"));
        a_stride_life(&format!("stride-{i}"));
        a_one_file_life(&format!("onefile-{i}"));
        a_sharing_life(&format!("sharing-{i}"), false);
        a_sharing_life(&format!("sharing-left-{i}"), true);
    }
    let after = LIVE.load(Ordering::SeqCst);
    let kept = after - before;
    assert!(
        kept < 64 << 10,
        "{} closed stores kept {kept} bytes allocated ({} a store)",
        kinds * lives,
        kept / (kinds * lives)
    );
    assert_eq!(
        Db::mapped_bytes(),
        0,
        "{} closed stores kept blocks mapped",
        kinds * lives
    );
}
