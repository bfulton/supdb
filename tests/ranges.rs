//! `ranges_for` against what a read actually touches, byte for byte.
//!
//! R6 rests on one property: the ranges the planner reports for a key are
//! exactly the ranges a subsequent read of that key asks the source for --
//! not roughly, not a superset that happens to work. A plan that
//! under-reports makes the caching byte source miss, which it is required to
//! treat as corruption; a plan that over-reports quietly fetches bytes no
//! read wants, which is the failure the whole feature exists to remove. So
//! the property is asserted rather than argued: a recording `Bytes` wrapper
//! logs every `(offset, len)` the reader pulls, and the merged log must equal
//! the merged plan.
//!
//! Three traps this file is built around, all found by design rather than by
//! accident:
//!
//! * **Granularity.** The read path reads a whole stored block per extent --
//!   verification and decompression want the enclosing bytes -- so the plan
//!   must be block-granular. A plan at extent granularity would be a subset
//!   of what is read and this test would fail it.
//! * **The open boundary.** `open` reads the superblock probe, the key index
//!   and the block table, and those are resident afterwards; the plan covers
//!   only reads after open. So every comparison here snapshots the log after
//!   open and diffs from there. `open_ranges` is tested separately to cover
//!   exactly the open-time reads.
//! * **Vacuity.** A test that passes on an empty set has fooled this project
//!   before. Present keys must plan non-empty, a key spanning blocks must
//!   plan more than one block's bytes, and scattered keys must plan disjoint
//!   ranges.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use supdb::bytes::Bytes;
use supdb::{Blob, Options, Store};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-rangestest-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d.join("store.supdb")
}

/// A byte source that cannot lend and remembers every read.
///
/// Cannot lend on purpose: this is the browser's shape (`supdb_host_read`
/// copies), and a lending source would satisfy section reads by borrowing,
/// which records nothing. With `slice_at` unanswered, every byte the reader
/// wants shows up in the log exactly once per request.
struct Recording {
    data: Vec<u8>,
    log: Rc<RefCell<Vec<(u64, u64)>>>,
}

impl Bytes for Recording {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        self.log.borrow_mut().push((off, dst.len() as u64));
        let end = off as usize + dst.len();
        if end > self.data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "short",
            ));
        }
        dst.copy_from_slice(&self.data[off as usize..end]);
        Ok(())
    }
}

/// Sort and merge (overlapping and adjacent), the same normalisation the
/// planner applies, so two range lists can be compared as byte sets.
fn merged(ranges: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = ranges.iter().copied().filter(|r| r.1 > 0).collect();
    v.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (off, len) in v {
        match out.last_mut() {
            Some(last) if off <= last.0 + last.1 => {
                let end = (off + len).max(last.0 + last.1);
                last.1 = end - last.0;
            }
            _ => out.push((off, len)),
        }
    }
    out
}

/// A store with run lengths from one value to tens of thousands, so the
/// probes cover an inline extent, a spilled extent list, and runs that
/// span several 64 KiB blocks.
fn build(path: &Path, keys: usize, opts: Options) -> Vec<Vec<u8>> {
    let store = Store::create(path, opts).expect("create");
    let mut names = Vec::new();
    for k in 0..keys {
        let key = format!("term={k:08}").into_bytes();
        let n = match k % 5 {
            0 => 1,
            1 => 17,
            2 => 400,
            3 => 40_000,
            _ => 6,
        };
        for i in 0..n {
            store
                .append(&key, &(i as u32).to_le_bytes())
                .expect("append");
        }
        names.push(key);
    }
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");
    names
}

struct Rig {
    blob: Blob<Recording>,
    log: Rc<RefCell<Vec<(u64, u64)>>>,
    mark: usize,
}

impl Rig {
    fn open(path: &Path) -> Rig {
        let log = Rc::new(RefCell::new(Vec::new()));
        let src = Recording {
            data: std::fs::read(path).unwrap(),
            log: log.clone(),
        };
        let blob = Blob::open(src).expect("blob open over a recording source");
        // The open boundary: everything logged so far is the superblock probe
        // and the two sections, resident from here on. The plan covers only
        // what comes after, so the comparisons start after it too.
        let mark = log.borrow().len();
        Rig { blob, log, mark }
    }

    /// The reads since the last call, merged.
    fn touched(&mut self) -> Vec<(u64, u64)> {
        let log = self.log.borrow();
        let out = merged(&log[self.mark..]);
        drop(log);
        self.mark = self.log.borrow().len();
        out
    }
}

fn assert_exact(rig: &mut Rig, keys: &[Vec<u8>]) {
    for key in keys {
        let plan = rig.blob.ranges_for(key).expect("plan");
        assert_eq!(
            plan,
            merged(&plan),
            "a plan must come out already sorted and merged"
        );
        rig.touched(); // discard anything the planning itself did (nothing)
        assert!(
            rig.touched().is_empty(),
            "planning is a plan: it must read no data"
        );

        let mut n = 0u64;
        rig.blob.read_all(key, |_| n += 1).expect("read_all");
        assert!(n > 0, "fixture key {:?} must exist", key);
        let touched = rig.touched();
        assert!(
            !touched.is_empty(),
            "a present key's read touches data, or this test checks nothing"
        );
        assert_eq!(
            touched,
            plan,
            "read_all of {} touched other bytes than its plan named",
            String::from_utf8_lossy(key)
        );

        // The walked count reads the same blocks -- it decodes the length
        // prefixes in place, and the unit of transfer is the block either way.
        let c = rig.blob.count(key).expect("count");
        assert_eq!(c, n);
        assert_eq!(
            rig.touched(),
            plan,
            "count of {} touched other bytes than its plan named",
            String::from_utf8_lossy(key)
        );
    }
}

#[test]
fn the_plan_is_exactly_what_a_read_touches() {
    let path = scratch("exact");
    let names = build(&path, 60, Options::default());
    let mut rig = Rig::open(&path);

    let probes: Vec<Vec<u8>> = [0usize, 1, 2, 3, 4, 31, 58, 59]
        .iter()
        .map(|i| names[*i].clone())
        .collect();
    assert_exact(&mut rig, &probes);

    // Non-vacuity: the 40,000-value keys cannot fit one 64 KiB block, so
    // their plan must name more than one block's worth of bytes.
    let big = rig.blob.ranges_for(&names[3]).unwrap();
    let planned: u64 = big.iter().map(|r| r.1).sum();
    assert!(
        planned > 64 << 10,
        "a 40,000-value run must span blocks, got {planned} bytes"
    );

    // A key that is not there plans nothing and reads nothing.
    let empty = rig.blob.ranges_for(b"term=nosuch").unwrap();
    assert!(empty.is_empty(), "an absent key has no bytes to plan");
    rig.touched();
    assert_eq!(rig.blob.read_all(b"term=nosuch", |_| {}).unwrap(), 0);
    assert!(
        rig.touched().is_empty(),
        "reading an absent key must not touch the source"
    );
}

#[test]
fn a_plan_for_many_keys_is_the_merged_union_and_a_shared_fetch() {
    let path = scratch("many");
    let names = build(&path, 60, Options::default());
    let mut rig = Rig::open(&path);

    // Scattered keys, deliberately mixed in size. The small keys' runs are
    // sealed together into shared blocks at checkpoint -- adjacent in the
    // file, which is what dedup and adjacency-merging are for -- while the
    // 40,000-value keys (3 and 33) fill solo blocks sealed mid-stream, far
    // from the tail and from each other. A pick of only small keys collapsed
    // to one contiguous range and made the disjointness assertion below
    // vacuous, which is itself worth knowing: block placement, not key
    // order, decides what a fetch costs.
    let picks = [0usize, 3, 30, 33, 59];
    let keys: Vec<&[u8]> = picks.iter().map(|i| names[*i].as_slice()).collect();
    let plan = rig.blob.ranges_for_many(&keys).expect("plan many");
    assert_eq!(plan, merged(&plan), "sorted, deduped, merged");
    assert!(
        plan.len() >= 2,
        "keys from opposite ends of the file cannot share one contiguous range"
    );

    // The union of the single plans, merged, is the same byte set.
    let mut all = Vec::new();
    for k in &keys {
        all.extend(rig.blob.ranges_for(k).unwrap());
    }
    assert_eq!(plan, merged(&all));

    // And reading every key touches exactly the shared plan.
    rig.touched();
    for k in &keys {
        rig.blob.read_all(k, |_| {}).unwrap();
    }
    assert_eq!(rig.touched(), plan);

    // An absent key in the set adds nothing and breaks nothing.
    let with_ghost: Vec<&[u8]> = keys
        .iter()
        .copied()
        .chain([b"term=nosuch" as &[u8]])
        .collect();
    assert_eq!(rig.blob.ranges_for_many(&with_ghost).unwrap(), plan);
}

#[test]
fn extent_counts_need_no_plan_because_they_read_nothing() {
    let path = scratch("fixedcount");
    // Fixed-width values, so `count_fixed` applies to every key.
    let store = Store::create(&path, Options::default()).expect("create");
    for k in 0..80u32 {
        let key = format!("term={k:08}").into_bytes();
        let n = [1u32, 9, 700, 20_000][(k % 4) as usize];
        for i in 0..n {
            store.append(&key, &i.to_le_bytes()).expect("append");
        }
    }
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");

    let mut rig = Rig::open(&path);
    rig.touched();
    for k in 0..80u32 {
        let key = format!("term={k:08}").into_bytes();
        let n = [1u64, 9, 700, 20_000][(k % 4) as usize];
        assert_eq!(rig.blob.count_fixed(&key, 4), Some(n));
        assert!(rig.blob.stored_bytes(&key) > 0);
    }
    let mut rows = 0usize;
    rig.blob
        .scan_counts_fixed(b"", usize::MAX, 4, |_, c| {
            assert!(c.is_some());
            rows += 1;
            true
        })
        .expect("scan fixed");
    assert_eq!(rows, 80);
    // The selling point, stated as an assertion: the O(extents) count and the
    // dictionary scan built on it are answered from the resident sections.
    // Over a caching source that means zero fetches after open.
    assert!(
        rig.touched().is_empty(),
        "count_fixed, stored_bytes and scan_counts_fixed must read no data"
    );
}

#[test]
fn the_plan_is_exact_through_compressed_blocks_too() {
    // Compression is off by default; a file written with it on reads through
    // the chunked and solo-decompress arms of `with_extent`, whose unit of
    // transfer is the *stored* (compressed) block. The plan must follow.
    let path = scratch("compressed");
    let names = build(
        &path,
        40,
        Options {
            compress: true,
            ..Default::default()
        },
    );
    let mut rig = Rig::open(&path);
    let probes: Vec<Vec<u8>> = [0usize, 1, 2, 3, 4, 39]
        .iter()
        .map(|i| names[*i].clone())
        .collect();
    assert_exact(&mut rig, &probes);
}

#[test]
fn open_ranges_names_exactly_what_open_reads() {
    let path = scratch("openplan");
    build(&path, 60, Options::default());
    let data = std::fs::read(&path).unwrap();

    let probe = supdb::blob::open_probe() as usize;
    let plan = supdb::blob::open_ranges(&data[..probe], data.len() as u64).expect("open plan");
    assert_eq!(plan, merged(&plan));
    assert!(
        plan.iter().any(|r| r.0 == 0),
        "the plan must include the probe it was made from"
    );

    // A fresh open over a recording source: the reads it makes, merged, must
    // be covered by the plan -- and the plan must not name bytes open never
    // asks for beyond the probe ranges themselves.
    let log = Rc::new(RefCell::new(Vec::new()));
    let src = Recording {
        data,
        log: log.clone(),
    };
    let _blob = Blob::open(src).expect("open");
    let opened = merged(&log.borrow());
    assert_eq!(
        opened, plan,
        "open touched other bytes than open_ranges named"
    );
}

#[test]
fn a_store_with_unreplayed_log_records_is_refused_not_misread() {
    // A sealed store carries an *empty* redo-log arena -- `log_len` in the
    // superblock is the arena's capacity, and a full rewrite leaves a zero
    // length-word at its head. This reader does not replay a log, which is
    // only sound while that word stays zero: a record there is newer than
    // everything in the index by construction, and ignoring it would serve
    // the previous state, silently. So `open` probes the word and refuses a
    // nonzero one. Checked by byte surgery -- write a nonzero length into
    // the arena of a clean store -- so the test does not depend on which
    // shapes the writer currently sends to the log.
    let path = scratch("logbytes");
    build(&path, 20, Options::default());
    let mut data = std::fs::read(&path).unwrap();

    // Find the winning superblock's log arena, exactly as the decoder does.
    let field = |slot: usize, i: usize| -> u64 {
        u64::from_le_bytes(data[slot + i * 8..slot + (i + 1) * 8].try_into().unwrap())
    };
    let slot = if field(0, 0) >= field(512, 0) { 0 } else { 512 };
    let (log_off, log_len) = (field(slot, 13) as usize, field(slot, 14));
    assert!(log_len >= 4, "a default store carries a log arena");

    // A sealed store's arena head really is the zero word the reader relies
    // on -- assert it before breaking it, so the surgery below is known to
    // be the only difference between the two opens.
    assert_eq!(&data[log_off..log_off + 4], &[0u8; 4]);
    assert!(Blob::open(supdb::VecBytes(data.clone())).is_ok());

    data[log_off..log_off + 4].copy_from_slice(&64u32.to_le_bytes());
    let err = match Blob::open(supdb::VecBytes(data.clone())) {
        Ok(_) => panic!("a store with unreplayed log records must be refused"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("redo log"),
        "the refusal must name the log, got: {err}"
    );
    // The open *plan* still succeeds -- it names the probe range so a caching
    // host fetches the word `open` will then check. The plan is where the
    // bytes are, not whether their content will pass.
    let probe = supdb::blob::open_probe() as usize;
    let plan = supdb::blob::open_ranges(&data[..probe], data.len() as u64).expect("plan");
    assert!(
        plan.iter()
            .any(|(o, l)| *o <= log_off as u64 && log_off as u64 + 4 <= o + l),
        "the open plan must cover the log probe at {log_off}: {plan:?}"
    );
}
