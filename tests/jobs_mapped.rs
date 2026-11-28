//! A job's large arrays are the engine's own mappings, unmapped with the
//! job.
//!
//! Its own test binary, with one test, because what it reads is the
//! process's: the count of the buffers that became mappings and the bytes
//! mapped and not yet unmapped, which every store in the process moves.

use std::collections::BTreeMap;
use supdb::{Db, Options};

mod common;
use common::{kv, without_the_clock};

fn key(k: u32) -> Vec<u8> {
    format!("key-{k:012}").into_bytes()
}

/// One store's life with every kind of job in it: a run of ordered ingest
/// closed by a flush, a table of scattered updates sealed by the next --
/// its seal sorts the table -- and the merge of that piece into the
/// partition the run became, joined by the settle. What it leaves on
/// disk, by name, how many of its jobs' arrays became mappings, by which
/// array, and what a scan of it read.
type Life = (
    BTreeMap<String, Vec<u8>>,
    Vec<(&'static str, usize)>,
    Vec<(Vec<u8>, usize)>,
);

fn life(name: &str) -> Life {
    let d = std::env::temp_dir().join(format!(
        "supdb-next-jobsmapped-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // The files are listed by name: a store of files (`common::files`).
    let opts = common::files(common::options());
    let opts = Options {
        // One seal a flush and none between, so the jobs are the test's
        // and the table the second flush seals holds every key: neither
        // the byte threshold nor the cap at a share of the store, which
        // sealed the updates a seventh at a time, each table's order too
        // small to be a mapping.
        seal_bytes: 1 << 30,
        seal_max_pct: 0,
        ..opts
    };
    let maps_before = Db::job_buffers_mapped();
    let mut db = Db::create(&d, opts).unwrap();
    let keys = 60_000u32;
    for k in 0..keys {
        db.append(&key(k), &[b'a'; 100]);
        if k % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // Scattered: every key at a stride coprime to the count, so the table
    // is a hash table and the seal sorts it.
    for i in 0..keys {
        let k = (i as u64 * 7919 % keys as u64) as u32;
        db.put(&key(k), &[b'b'; 90]);
        if i % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let mut read: Vec<(Vec<u8>, usize)> = Vec::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| read.push((k.to_vec(), v.len()))),
    )
    .unwrap();
    db.close().unwrap();
    let made: Vec<(&'static str, usize)> = Db::job_buffers_mapped()
        .into_iter()
        .zip(maps_before)
        .map(|((site, after), (_, before))| (site, after - before))
        .collect();
    let mut files = BTreeMap::new();
    for e in std::fs::read_dir(&d).unwrap() {
        let p = e.unwrap().path();
        let n = p.file_name().unwrap().to_str().unwrap().to_string();
        if n.ends_with(".sup") {
            files.insert(n, without_the_clock(&std::fs::read(&p).unwrap()));
        } else if n.ends_with(".oidx") {
            files.insert(n, std::fs::read(&p).unwrap());
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    (files, made, read)
}

/// The seal's sort, the writers' per-key arrays, the ordered index
/// builders' keys, the merge's key list and the finishes' trailers are
/// mappings once they are large -- counted as each becomes one -- every
/// key reads back with its last value, and nothing stays mapped past the
/// store's close; and a run a crash left, which the open finishes, has its
/// trailer composed in a mapping too.
#[test]
fn a_jobs_large_arrays_are_mapped_and_go_with_the_job() {
    let mapped_before = Db::mapped_bytes();
    let (files, made, read) = life("mapped");
    // Every array the store maps, each large at sixty thousand keys: the
    // seal's sort and its order, the writer's four and the builder's two
    // for the piece it writes and for the merge's output, the merge's key
    // list, and a trailer at every finish.
    assert_eq!(made.len(), 12, "the sites the jobs map");
    for &(site, n) in &made {
        assert!(n > 0, "no {site} array became a mapping: {made:?}");
    }
    assert_eq!(
        Db::mapped_bytes(),
        mapped_before,
        "every mapping a job made went with the job"
    );
    assert_eq!(read.len(), 60_000);
    assert!(
        read.iter().all(|(_, n)| *n == 90),
        "every key reads its last value"
    );
    assert!(files.keys().any(|n| n.starts_with("par-")));
    // Last, since a crashed store's blocks are never unmapped.
    let d = std::env::temp_dir().join(format!(
        "supdb-next-jobsmapped-crash-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = || Options {
        zpages: supdb::ZPages::Off,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts()).unwrap();
    for k in 0..60_000u32 {
        db.append(&key(k), &[b'a'; 100]);
        if k % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.crash();
    let trailers = || {
        Db::job_buffers_mapped()
            .iter()
            .find(|(s, _)| *s == "trailer")
            .unwrap()
            .1
    };
    let before = trailers();
    let db = Db::open(&d, opts()).unwrap();
    assert!(
        trailers() > before,
        "the trailer of the run the open finished"
    );
    let mut n = 0usize;
    db.scan(b"", usize::MAX, kv(&mut |_, _| n += 1)).unwrap();
    assert_eq!(n, 60_000, "every committed key recovered");
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}
