//! What the resident count reads, what a trim returns of it, and the trim
//! thread that counts by it.
//!
//! A test binary of its own because the count is the process's: the
//! anonymous bytes resident beyond the allocator's use, which any thread's
//! allocation from free chunks already resident lowers. In the `db` binary,
//! beside two hundred tests that allocate and free, a dead arena of
//! thirty-two megabytes read as nine more between the two readings of one
//! run of the gate, the other tests' allocations taking the rest; and the
//! trim thread's test there waited four hundred looks for a second trim on
//! a hosted ARM runner, reading 211 MB resident beyond use, the second dead
//! arena never a stride above the floor. Its tests share one lock, since
//! each reads the count the other moves.

use std::collections::BTreeMap;
use std::path::PathBuf;

use supdb::{Db, Options};

mod common;
use common::{dead_arena, kv};

/// Taken by every test here: each reads the process's count, and a dead
/// arena or a trim of one test's moves the other's readings. Through
/// `into_inner`, so that a failure holds nobody.
static COUNT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn count_lock() -> std::sync::MutexGuard<'static, ()> {
    COUNT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What the resident count reads (`Options::trim_by_resident`) and what a
/// trim returns of it: a dead arena of thirty-two megabytes reads as that
/// much more resident beyond use, and a trim of every arena gives it back.
/// Each difference is held to half the arena and not to its bytes, since
/// the process's own threads allocate beside it.
#[test]
fn the_resident_count_reads_a_dead_arenas_pages_and_a_trim_returns_them() {
    if !Db::trim_supported() {
        return;
    }
    let _count = count_lock();
    // The process's resident free pages returned first, so the dead arena's
    // pieces come from pages that have to be faulted in: in the gate's whole
    // `db` run the process held 118 MB resident and free from the other
    // tests' dead arenas, the pieces came from pages those had left
    // resident, and the count rose 3 MB for an arena of 32.
    Db::trim_now();
    let before =
        Db::resident_beyond_use().expect("the resident count reads where the free count does");
    let keeper = dead_arena(32 << 20);
    let freed = Db::resident_beyond_use().unwrap();
    assert!(
        freed >= before + (16 << 20),
        "a dead arena of 32 MB read {before} -> {freed} resident beyond use"
    );
    Db::trim_now();
    let trimmed = Db::resident_beyond_use().unwrap();
    assert!(
        freed - trimmed.min(freed) >= 16 << 20,
        "a trim returned {freed} -> {trimmed} of the dead arena's pages"
    );
    drop(keeper);
}

/// A store whose upkeep thread's passes file into the block forms, as the
/// `db` binary's burst helper makes it (`burst_into_forms_with` there):
/// three thousand keys flushed into several partitions, every eighth
/// rewritten and scanned across, which forms the blocks, then five of
/// every eight rewritten, a commit, and a scan of one entry from a block
/// that holds a form. Each key holds one value, the last written, which
/// `written` keeps for the check at the test's end.
fn burst_store(name: &str, tune: impl FnOnce(&mut Options)) -> (Db, BTreeMap<String, String>) {
    let mut opts = Options {
        seal_bytes: 1 << 20,
        partition_bytes: Some(2 << 10),
        l0_trigger: 64,
        scan_block_cache: true,
        scan_cache_ahead: false,
        scan_cache_bytes: 8 << 10,
        forms_settle_backlog_pct: 0,
        commit_forms: true,
        upkeep: supdb::Upkeep::Inline,
        forms_to_fit: true,
        ..common::options()
    };
    tune(&mut opts);
    let mut db = Db::create(&dir(name), opts).unwrap();
    let mut written = BTreeMap::new();
    let key = |k: u32| format!("key-{k:05}");
    let mut put = |db: &mut Db, k: String, v: &str, replace: bool| {
        if replace {
            db.delete(k.as_bytes());
        }
        db.append(k.as_bytes(), v.as_bytes());
        written.insert(k, v.to_string());
    };
    for k in 0..3000u32 {
        put(&mut db, key(k), "v0-0123456789", false);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(db.levels().0 > 1, "several partitions");
    let mut sink = 0usize;
    for k in (0..3000u32).step_by(8) {
        put(&mut db, key(k), "v1-0123456789", true);
    }
    db.commit().unwrap();
    db.scan(
        key(0).as_bytes(),
        usize::MAX,
        kv(&mut |_k, v| sink += v.len()),
    )
    .unwrap();
    for k in (0..3000u32).filter(|k| k % 8 < 5) {
        put(&mut db, key(k), "v2-0123456789", true);
    }
    db.commit().unwrap();
    let held = (0..3000u32)
        .step_by(64)
        .find(|&k| {
            db.block_state(key(k).as_bytes())
                .is_some_and(|s| s.4 != "unbuilt")
        })
        .expect("a block holds a form");
    db.scan(key(held).as_bytes(), 1, kv(&mut |_k, v| sink += v.len()))
        .unwrap();
    std::hint::black_box(sink);
    (db, written)
}

/// The trim thread counting by the resident bytes (`Options::trim_by_resident`)
/// trims again when a dead arena's pages come back resident, which the free
/// count never sees, since the pages come back under chunks already free. A
/// store with the look at every pass end and a stride of eight megabytes,
/// beside a dead arena of thirty-two: its first look trims, the floor
/// starting at zero; a thread then re-touches the arena and frees it, and a
/// pass's look trims again. The positive alone, since what the allocator
/// holds is the whole process's; the free count's standing still is
/// `trim_due`'s arithmetic over the census's readings. The store answers
/// every key it was written, through a scan, at the end.
#[test]
fn the_trim_thread_counting_by_resident_bytes_trims_a_dead_arena_again() {
    if !Db::trim_supported() {
        return;
    }
    let _count = count_lock();
    let keeper = dead_arena(32 << 20);
    let (mut db, mut written) = burst_store("trim-resident", |o| {
        o.upkeep = supdb::Upkeep::Background(1);
        o.upkeep_inline_bursts = false;
        o.forms_trim_bytes = 8 << 20;
        o.upkeep_trim_look = true;
        o.trim_by_resident = true;
    });
    let mut rounds = 0u32;
    while db.forms_trims() == 0 {
        assert!(
            rounds < 400,
            "no first trim: {} looks, held {:?}",
            db.forms_trim_looks(),
            db.trim_held_bytes()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        rounds += 1;
    }
    let first = db.forms_trims();
    let keeper2 = dead_arena(32 << 20);
    let mut rounds = 0u32;
    while db.forms_trims() <= first {
        assert!(
            rounds < 400,
            "no second trim: {} trims, {} looks, held {:?}",
            db.forms_trims(),
            db.forms_trim_looks(),
            db.trim_held_bytes()
        );
        let k = format!("key-{:05}", 3000 + rounds);
        db.append(k.as_bytes(), b"v3-0123456789");
        written.insert(k, "v3-0123456789".to_string());
        db.commit().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        rounds += 1;
    }
    let mut read: BTreeMap<String, Vec<String>> = BTreeMap::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            read.entry(String::from_utf8(k.to_vec()).unwrap())
                .or_default()
                .push(String::from_utf8(v.to_vec()).unwrap())
        }),
    )
    .unwrap();
    let want: BTreeMap<String, Vec<String>> =
        written.into_iter().map(|(k, v)| (k, vec![v])).collect();
    assert!(read == want, "the store answers every key it was written");
    drop((keeper, keeper2));
}
