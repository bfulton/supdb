//! A job's mapped buffers are kept between jobs while the store has room
//! and given back when it reads short of memory (`Options::jobs_pooled`),
//! and the store they write is the same store.
//!
//! Its own test binary, with one test, because what it reads is the
//! process's: the pool's counts and bytes, and the bytes mapped and not
//! yet unmapped, which every store in the process moves.

use std::collections::BTreeMap;
use std::sync::Arc;
use supdb::{Db, Options, ZInject};

mod common;
use common::{kv, without_the_clock};

fn key(k: u32) -> Vec<u8> {
    format!("key-{k:012}").into_bytes()
}

const KEYS: u32 = 60_000;

/// A run of ordered ingest of `KEYS` keys from `from`, closed by a flush:
/// its close composes a trailer, one of the jobs' arrays.
fn run(db: &mut Db, from: u32, fill: u8) {
    for k in from..from + KEYS {
        db.append(&key(k), &[fill; 100]);
        if k % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
}

/// Every key below `KEYS` rewritten in scattered order and flushed: the
/// seal sorts the table and writes its piece, and the merge of the piece
/// into the partition is joined by the settle. Every array the jobs have.
fn rewrite(db: &mut Db, fill: u8) {
    for i in 0..KEYS {
        let k = (i as u64 * 7919 % KEYS as u64) as u32;
        db.put(&key(k), &[fill; 90]);
        if i % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
}

fn files(d: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for e in std::fs::read_dir(d).unwrap() {
        let p = e.unwrap().path();
        let n = p.file_name().unwrap().to_str().unwrap().to_string();
        if n.ends_with(".sup") {
            files.insert(n, without_the_clock(&std::fs::read(&p).unwrap()));
        } else if n.ends_with(".oidx") {
            files.insert(n, std::fs::read(&p).unwrap());
        }
    }
    files
}

fn scan(db: &Db) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut read = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| read.push((k.to_vec(), v.to_vec()))),
    )
    .unwrap();
    read
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "supdb-next-jobspooled-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The store's options: one seal a flush and none between, so the jobs
/// are the test's; the budget the seam's; and the look at the store
/// against it the pool's own, the forms' fit, which shares it, off.
fn options(pooled: bool, inject: &Arc<ZInject>) -> Options {
    Options {
        jobs_mapped: pooled,
        jobs_pooled: pooled,
        jobs_mapped_when_short: false,
        forms_fit_when_short: false,
        zpages: supdb::ZPages::Off,
        zpages_inject: Some(inject.clone()),
        seal_bytes: 1 << 30,
        seal_max_pct: 0,
        ..common::options()
    }
}

/// The store against its budget as the segment work's look reads it,
/// once a second: waited for by the word the look sets, which it sets
/// once it has held the pool off or let it go.
fn wait_short(db: &Db, short: bool) {
    let t = std::time::Instant::now();
    while db.forms_short() != short {
        assert!(t.elapsed().as_secs() < 30, "the store reads short: {short}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// One store's life of a run and a rewrite of it, with memory to spare
/// throughout: what it leaves on disk by name, what a scan of it read,
/// and the pool's counts after the run's close and after the rewrite.
type Life = (
    BTreeMap<String, Vec<u8>>,
    Vec<(Vec<u8>, Vec<u8>)>,
    [[usize; 4]; 2],
);

fn life(name: &str, pooled: bool) -> Life {
    let d = dir(name);
    let inject = Arc::new(ZInject::new());
    inject.set_budget(1 << 40);
    let mut db = Db::create(&d, options(pooled, &inject)).unwrap();
    run(&mut db, 0, b'a');
    let c1 = Db::job_pool_counts();
    rewrite(&mut db, b'b');
    let c2 = Db::job_pool_counts();
    let read = scan(&db);
    db.close().unwrap();
    let written = files(&d);
    let _ = std::fs::remove_dir_all(&d);
    (written, read, [c1, c2])
}

/// While the store has room, a job's mapped buffers are kept at its end
/// and the next job's arrays of the same kind take them: the rewrite's
/// seal and merge take what the run's close and each other gave back.
/// The look that finds the store short of its budget empties the pool
/// and holds it off: the jobs after it map their arrays and unmap them,
/// nothing kept and nothing taken. Room again, and the jobs' buffers are
/// kept again. A store closed while it holds the pool off lets it go. A
/// close leaves nothing kept and nothing mapped. And the files a store
/// writes through pooled buffers, whatever those held before, are the
/// files the same writes make through the allocator's.
#[test]
fn a_jobs_buffers_are_kept_while_there_is_room_and_given_back_when_there_is_not() {
    let mapped_before = Db::mapped_bytes();
    let counts = Db::job_pool_counts;
    assert_eq!(counts(), [0, 0, 0, 0], "a pool nobody has used");

    // The allocator's first: the pool is nobody's.
    let (heap_files, heap_read, heap_counts) = life("heap", false);
    assert_eq!(heap_counts, [[0; 4]; 2], "off, nothing kept or taken");
    assert_eq!(counts(), [0, 0, 0, 0]);
    assert_eq!(Db::mapped_bytes(), mapped_before, "the heap store closed");
    assert_eq!(heap_read.len(), KEYS as usize);
    assert!(heap_read.iter().all(|(_, v)| v == &[b'b'; 90]));

    // A store that reads short from its first look, and is closed so:
    // nothing of its jobs' kept, and its hold let go at the close, which
    // the stores after this one show by keeping.
    {
        let d = dir("closed-short");
        let inject = Arc::new(ZInject::new());
        inject.set_budget(1 << 20);
        let mut db = Db::create(&d, options(true, &inject)).unwrap();
        run(&mut db, 0, b'a');
        wait_short(&db, true);
        let before = counts();
        assert_eq!(before[3], 0, "emptied by the look that found it short");
        rewrite(&mut db, b'b');
        let after = counts();
        assert_eq!(
            (after[0], after[1], after[3]),
            (before[0], before[1], 0),
            "short, nothing taken and nothing kept: {before:?} {after:?}"
        );
        assert!(after[2] > before[2], "each buffer unmapped as it came back");
        assert_eq!(scan(&db), heap_read, "closed-short: the same store read");
        db.close().unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(Db::mapped_bytes(), mapped_before, "the short store closed");
    }

    // The pooled life, memory to spare: kept at the run's close, taken
    // and kept by the rewrite's jobs, nothing kept past the close.
    let c0 = counts();
    let (pool_files, pool_read, [c1, c2]) = life("pooled", true);
    assert!(
        c1[1] > c0[1] && c1[3] > 0,
        "kept at the run's close: {c0:?} {c1:?}"
    );
    assert!(
        c2[0] > c1[0],
        "the rewrite's jobs took what was kept: {c1:?} {c2:?}"
    );
    assert!(c2[1] > c1[1] && c2[3] > 0, "and gave back: {c2:?}");
    let c3 = counts();
    assert_eq!(c3[3], 0, "nothing kept past the close");
    assert!(c3[2] > c2[2], "the close unmapped what was kept");
    assert_eq!(
        Db::mapped_bytes(),
        mapped_before,
        "nothing mapped past the close"
    );
    assert_eq!(pool_read, heap_read, "the same store read back");
    assert_eq!(
        pool_files.keys().collect::<Vec<_>>(),
        heap_files.keys().collect::<Vec<_>>(),
        "the same files by name"
    );
    assert!(heap_files.keys().any(|n| n.starts_with("par-")));
    for (name, bytes) in &heap_files {
        assert!(
            pool_files[name] == *bytes,
            "{name}: written through pooled buffers as through the allocator's"
        );
    }

    // Room, short, room: one store through all three.
    let d = dir("turns");
    let inject = Arc::new(ZInject::new());
    inject.set_budget(1 << 40);
    let mut db = Db::create(&d, options(true, &inject)).unwrap();
    run(&mut db, 0, b'a');
    rewrite(&mut db, b'b');
    let roomy = counts();
    assert!(roomy[3] > 0, "room: buffers kept: {roomy:?}");
    inject.set_budget(1 << 20);
    wait_short(&db, true);
    let held = counts();
    assert_eq!(held[3], 0, "short: the pool emptied");
    assert!(held[2] > roomy[2], "by unmapping what it kept: {held:?}");
    assert_eq!((held[0], held[1]), (roomy[0], roomy[1]));
    rewrite(&mut db, b'c');
    run(&mut db, KEYS, b'd');
    let still = counts();
    assert_eq!(
        (still[0], still[1], still[3]),
        (held[0], held[1], 0),
        "short: nothing taken and nothing kept: {held:?} {still:?}"
    );
    assert!(still[2] > held[2], "each job's buffers unmapped at its end");
    inject.set_budget(1 << 40);
    wait_short(&db, false);
    rewrite(&mut db, b'e');
    let again = counts();
    assert!(
        again[1] > still[1] && again[3] > 0,
        "room again: kept again: {still:?} {again:?}"
    );
    // And taken again, by every array a seal and a merge have: the
    // sort's three, the writers' four, the builders' two, the merge's
    // key list and the finishes' trailers.
    let takes_before = Db::job_pool_takes();
    rewrite(&mut db, b'f');
    assert!(counts()[0] > again[0], "and taken again");
    let takes = Db::job_pool_takes();
    assert_eq!(takes.len(), 12, "the arrays the jobs have");
    for (&(site, after), &(_, before)) in takes.iter().zip(&takes_before) {
        assert!(
            after > before,
            "room: no {site} array took its buffer from the pool: {takes:?}"
        );
    }
    let read = scan(&db);
    assert_eq!(read.len(), 2 * KEYS as usize);
    for (i, (k, v)) in read.iter().enumerate() {
        assert_eq!(k, &key(i as u32));
        if (i as u32) < KEYS {
            assert!(v == &[b'f'; 90], "key {i}: its last rewrite");
        } else {
            assert!(v == &[b'd'; 100], "key {i}: the second run's");
        }
    }
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(counts()[3], 0, "nothing kept past the close");
    assert_eq!(
        Db::mapped_bytes(),
        mapped_before,
        "nothing mapped past the close"
    );
}
