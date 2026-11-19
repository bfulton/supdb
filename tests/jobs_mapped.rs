//! A job's large arrays are the engine's own mappings where the option
//! says so, and the store they write is the same store.
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

fn life(name: &str, mapped: bool) -> Life {
    let d = std::env::temp_dir().join(format!(
        "supdb-next-jobsmapped-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let opts = Options {
        jobs_mapped: mapped,
        // One seal a flush and none between, so the jobs are the test's
        // and the table the second flush seals holds every key: neither
        // the byte threshold nor the cap at a share of the store, which
        // sealed the updates a seventh at a time, each table's order too
        // small to be a mapping.
        seal_bytes: 1 << 30,
        seal_max_pct: 0,
        ..common::options()
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

/// Under `Options::jobs_mapped` the seal's sort, the writers' per-key
/// arrays, the ordered index builders' keys, the merge's key list and the
/// finishes' trailers are mappings once they are large -- counted as each
/// becomes one -- and none where the option is off; the segments and the
/// ordered indexes the two stores leave are the same bytes, every key
/// reads back with its last value, and nothing stays mapped past either
/// store's close.
#[test]
fn a_jobs_large_arrays_are_mapped_and_the_store_is_the_same() {
    let mapped_before = Db::mapped_bytes();
    let (heap_files, heap_made, heap_read) = life("heap", false);
    assert!(
        heap_made.iter().all(|&(_, n)| n == 0),
        "off, no job's array is a mapping: {heap_made:?}"
    );
    assert_eq!(Db::mapped_bytes(), mapped_before, "the heap store closed");
    let (map_files, map_made, map_read) = life("mapped", true);
    // Every array the option names, each large at sixty thousand keys:
    // the seal's sort and its order, the writer's four and the builder's
    // two for the piece it writes and for the merge's output, the
    // merge's key list, and a trailer at every finish.
    assert_eq!(map_made.len(), 12, "the sites the option names");
    for &(site, n) in &map_made {
        assert!(n > 0, "on, no {site} array became a mapping: {map_made:?}");
    }
    assert_eq!(
        Db::mapped_bytes(),
        mapped_before,
        "every mapping a job made went with the job"
    );
    assert_eq!(heap_read.len(), 60_000);
    assert!(
        heap_read.iter().all(|(_, n)| *n == 90),
        "every key reads its last value"
    );
    assert_eq!(heap_read, map_read, "the same keys and values read back");
    assert_eq!(
        heap_files.keys().collect::<Vec<_>>(),
        map_files.keys().collect::<Vec<_>>(),
        "the same files"
    );
    assert!(heap_files.keys().any(|n| n.starts_with("par-")));
    for (name, bytes) in &heap_files {
        let other = &map_files[name];
        let first = if bytes.len() != other.len() {
            Some(bytes.len().min(other.len()))
        } else {
            bytes.iter().zip(other).position(|(a, b)| a != b)
        };
        assert!(
            first.is_none(),
            "{name} differs between the two, first at {first:?}"
        );
    }
}
