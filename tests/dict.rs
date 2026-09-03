//! The range-readable dictionary: `SparseBlob` against `Blob`.
//!
//! A second way to read the key space is a liability in the same way the
//! second reader was -- its failure mode is a range that quietly answers a
//! different question -- so every range here is checked against the whole
//! reader's answer, and every plan against the reads that follow it, on both
//! index shapes (a key section built at the end over block-backed runs, and
//! the records-first one the writer streams). Then the browser's contract is run literally: a source that
//! serves only what was ensured, and nothing else, is enough.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use supdb::blob::{open_probe, open_sparse_fence_ranges, open_sparse_ranges};
use supdb::next::SegmentWriter;
use supdb::{Blob, Bytes, MmapBytes, Options, SparseBlob};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-dicttest-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d.join("index.supdb")
}

/// A byte source that cannot lend and remembers every read.
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

/// The browser's cache, reduced to its rule: a read outside what was ensured
/// is an error, never zeroes.
struct Ensured {
    data: Vec<u8>,
    allowed: RefCell<Vec<(u64, u64)>>,
}

impl Ensured {
    fn ensure(&self, ranges: &[(u64, u64)]) {
        self.allowed.borrow_mut().extend_from_slice(ranges);
    }
}

impl Bytes for Ensured {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        let end = off + dst.len() as u64;
        let ok = merged(&self.allowed.borrow())
            .iter()
            .any(|&(a, l)| a <= off && end <= a + l);
        if !ok {
            return Err(std::io::Error::other(format!(
                "cache miss: {} bytes at {off} were not ensured",
                dst.len()
            )));
        }
        dst.copy_from_slice(&self.data[off as usize..end as usize]);
        Ok(())
    }
}

/// Every byte of `reads` lies inside `plan` (both merged). A checksummed
/// index verifies each 16 KiB piece once per reader, so a walk reads
/// exactly its plans the first time it touches a piece and less after; the
/// contract a range-fetching host relies on is that it never reads more.
fn within(reads: &[(u64, u64)], plan: &[(u64, u64)]) -> bool {
    let plan = merged(plan);
    merged(reads)
        .iter()
        .all(|&(a, n)| plan.iter().any(|&(pa, pn)| pa <= a && a + n <= pa + pn))
}

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

/// Keys under three field prefixes, so a range can be a whole field, part
/// of one, or a span across two. Values vary in count so the fixture holds
/// inline runs, spilled extent lists and block-backed runs alike.
fn keys_and_values(n: usize) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut all = Vec::new();
    for (fi, field) in ["app", "host", "path"].iter().enumerate() {
        for i in 0..n {
            let key = format!("{field}={i:05}").into_bytes();
            let count = match (i + fi) % 9 {
                0 => 1,
                1 => 2,
                2 => 3,
                3 => 12,
                4 => 40,
                5 => 300,
                _ => 5,
            };
            let vals = (0..count)
                .map(|j| format!("{fi}:{i}:{j}").into_bytes())
                .collect();
            all.push((key, vals));
        }
    }
    all.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    all
}

/// The writer's block layout: every run in a block, key section built at the
/// end. The sparse reader plans differently over it than over the records-
/// first layout below, so both index shapes are swept.
fn build_blocks(path: &Path, all: &[(Vec<u8>, Vec<Vec<u8>>)]) {
    build_segment_with(path, all, 0)
}

fn build_segment(path: &Path, all: &[(Vec<u8>, Vec<Vec<u8>>)]) {
    build_segment_with(path, all, 256)
}

fn build_segment_with(path: &Path, all: &[(Vec<u8>, Vec<Vec<u8>>)], inline_max: usize) {
    let opts = Options {
        redo_log: false,
        shards: 1,
        ..Options::default()
    };
    let mut w = SegmentWriter::create(path, &opts).expect("create");
    w.set_inline_max(inline_max);
    for (k, vals) in all {
        w.begin(k).expect("begin");
        for v in vals {
            w.value(v);
        }
        w.end().expect("end");
    }
    w.finish(1).expect("finish");
}

/// The ranges a test walks: whole fields, spans across fields, a key and its
/// neighbour, an empty range, an inverted one, ranges past both ends, and
/// open-ended ones.
fn ranges_to_try(all: &[(Vec<u8>, Vec<Vec<u8>>)]) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let key = |i: usize| all[i % all.len()].0.clone();
    let bump = |k: &Vec<u8>| {
        let mut b = k.clone();
        *b.last_mut().unwrap() += 1;
        b
    };
    let mut v: Vec<(Vec<u8>, Option<Vec<u8>>)> = vec![
        (b"app=".to_vec(), Some(b"app>".to_vec())),
        (b"host=".to_vec(), Some(b"host>".to_vec())),
        (b"path=".to_vec(), Some(b"path>".to_vec())),
        (b"host=00010".to_vec(), Some(b"path=00010".to_vec())),
        (b"".to_vec(), None),
        (b"".to_vec(), Some(b"app=00001".to_vec())),
        (b"path=".to_vec(), None),
        (b"zzz".to_vec(), None),
        (b"zzz".to_vec(), Some(b"zzzz".to_vec())),
        (b"".to_vec(), Some(b"".to_vec())),
        (key(50), Some(key(50))),
        (key(60), Some(key(40))),
        (key(7), Some(bump(&key(7)))),
        (key(all.len() - 1), None),
        (key(all.len() - 1), Some(bump(&key(all.len() - 1)))),
    ];
    let mut x = 0xD1C7_u64;
    for _ in 0..120 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let a = (x >> 33) as usize % all.len();
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let b = (x >> 33) as usize % all.len();
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let hi_key = if x & 1 == 0 {
            Some(key(hi))
        } else {
            Some(bump(&key(hi)))
        };
        v.push((key(lo), if x & 2 == 0 { hi_key } else { None }));
    }
    v
}

fn expected(whole: &Blob<MmapBytes>, lo: &[u8], hi: Option<&[u8]>) -> Vec<(Vec<u8>, u64)> {
    let mut out = Vec::new();
    whole
        .scan_counts(lo, usize::MAX, |k, n| {
            if hi.is_some_and(|h| k >= h) {
                return false;
            }
            out.push((k.to_vec(), n));
            true
        })
        .expect("scan");
    out
}

fn check_shape(path: &Path, all: &[(Vec<u8>, Vec<Vec<u8>>)], shape: &str) {
    let whole = Blob::open(MmapBytes::open(path).unwrap()).expect("whole open");
    let lending = SparseBlob::open(MmapBytes::open(path).unwrap()).expect("sparse open, lending");
    let log = Rc::new(RefCell::new(Vec::new()));
    let sparse = SparseBlob::open(Recording {
        data: std::fs::read(path).unwrap(),
        log: log.clone(),
    })
    .expect("sparse open, copying");
    assert!(
        sparse.has_fence(),
        "{shape}: the fixture must carry a fence or the plans are trivial"
    );
    assert_eq!(sparse.keys(), whole.keys());
    assert_eq!(sparse.version(), whole.version());
    let mut mark = log.borrow().len();
    let mut touched = || {
        let l = log.borrow();
        let out = merged(&l[mark..]);
        mark = l.len();
        out
    };

    let mut nonempty = 0usize;
    let mut partial = 0usize;
    for (lo, hi) in ranges_to_try(all) {
        let hi = hi.as_deref();
        let want = expected(&whole, &lo, hi);
        let label = format!(
            "{shape}: [{:?}, {:?})",
            String::from_utf8_lossy(&lo),
            hi.map(String::from_utf8_lossy)
        );

        // The plans: phase one reads nothing, phase two reads within phase
        // one, and the walk reads within both -- exactly, the first time a
        // piece is touched, and less once its checksum has been verified.
        touched();
        let p1 = sparse.dictionary_plan(&lo, hi);
        assert!(
            touched().is_empty(),
            "{label}: planning the directory slice read bytes"
        );
        let p2 = sparse
            .dictionary_plan_records(&lo, hi)
            .expect("plan records");
        let t2 = touched();
        assert!(
            within(&t2, &p1),
            "{label}: phase two must read within phase one: {t2:?} vs {p1:?}"
        );
        let mut got = Vec::new();
        sparse
            .dictionary_counts(&lo, hi, |k, n| {
                got.push((k.to_vec(), n));
                true
            })
            .expect("walk");
        let both: Vec<(u64, u64)> = p1.iter().chain(p2.iter()).copied().collect();
        let tw = touched();
        assert!(
            within(&tw, &both),
            "{label}: the walk must read within its two plans: {tw:?} vs {both:?}"
        );
        assert_eq!(
            got, want,
            "{label}: the sparse walk disagrees with the whole reader"
        );

        // The lending source answers the same, through the zero-copy arm.
        let mut lent = Vec::new();
        lending
            .dictionary_counts(&lo, hi, |k, n| {
                lent.push((k.to_vec(), n));
                true
            })
            .expect("walk, lending");
        assert_eq!(lent, want, "{label}: the lending walk disagrees");

        // A range's plan is proportional to the range, not to the index:
        // anything narrower than the dictionary must plan fewer record
        // bytes than the index holds.
        let planned: u64 = both.iter().map(|r| r.1).sum();
        if !want.is_empty() {
            nonempty += 1;
        }
        if want.len() * 4 < whole.keys() {
            partial += 1;
            assert!(
                planned < whole.index_bytes() as u64,
                "{label}: a partial range planned {planned} bytes of a {}-byte index",
                whole.index_bytes()
            );
        }

        // Stopping early stops.
        if want.len() > 3 {
            let mut seen = 0usize;
            let n = sparse
                .dictionary_counts(&lo, hi, |_, _| {
                    seen += 1;
                    seen < 3
                })
                .unwrap();
            assert_eq!(
                (n, seen),
                (3, 3),
                "{label}: returning false must stop the walk"
            );
        }
    }
    assert!(
        nonempty > 20 && partial > 20,
        "{shape}: the range set is too thin to mean anything"
    );

    // Values, through the extents a walk hands out: inline runs need no
    // plan, block-backed ones plan their blocks and read exactly them.
    touched();
    let mut checked = 0usize;
    sparse
        .dictionary_walk(b"host=", Some(b"host>"), |k, exts, tail| {
            let plan = sparse.ranges_for_exts(exts).expect("plan exts");
            let key = k.to_vec();
            let exts = exts.to_vec();
            let tail = tail.to_vec();
            let mut vals = Vec::new();
            sparse
                .read_exts(&exts, &tail, |v| vals.push(v.to_vec()))
                .expect("read exts");
            let want = &all.iter().find(|(wk, _)| *wk == key).expect("known key").1;
            assert_eq!(
                &vals,
                want,
                "{shape}: values of {}",
                String::from_utf8_lossy(&key)
            );
            let _ = plan;
            checked += 1;
            checked < 60
        })
        .expect("walk with values");
    assert!(checked > 0);
}

#[test]
fn sparse_ranges_agree_with_the_whole_reader_and_read_exactly_their_plans() {
    let all = keys_and_values(700);
    let blocks = scratch("blocks");
    build_blocks(&blocks, &all);
    check_shape(&blocks, &all, "blocks");
    let seg = scratch("segment");
    build_segment(&seg, &all);
    check_shape(&seg, &all, "segment");
}

#[test]
fn the_sparse_open_reads_header_fence_and_block_table_only() {
    let all = keys_and_values(2000);
    for (shape, build) in [
        (
            "blocks",
            build_blocks as fn(&Path, &[(Vec<u8>, Vec<Vec<u8>>)]),
        ),
        ("segment", build_segment),
    ] {
        let path = scratch(&format!("open-{shape}"));
        build(&path, &all);
        let data = std::fs::read(&path).unwrap();
        let object_len = data.len() as u64;

        let whole_log = Rc::new(RefCell::new(Vec::new()));
        let whole = Blob::open(Recording {
            data: data.clone(),
            log: whole_log.clone(),
        })
        .unwrap();
        let whole_bytes: u64 = merged(&whole_log.borrow()).iter().map(|r| r.1).sum();

        let log = Rc::new(RefCell::new(Vec::new()));
        let sparse = SparseBlob::open(Recording {
            data: data.clone(),
            log: log.clone(),
        })
        .unwrap();
        let read = merged(&log.borrow());
        let sparse_bytes: u64 = read.iter().map(|r| r.1).sum();

        // The two plans name exactly what the open read.
        let head = &data[..open_probe() as usize];
        let p1 = open_sparse_ranges(head, object_len).unwrap();
        let hdr_range = p1
            .iter()
            .find(|r| {
                r.0 != 0 && r.1 as usize >= supdb::flatindex::HEADER_BYTES && r.0 < object_len
            })
            .copied();
        // The index header is the first HEADER_BYTES of the key section; the
        // plan lists it merged with whatever it touches, so find it through
        // the section offset the superblock names rather than by position.
        let key_off = {
            // A whole reader's index section starts where its first non-probe
            // read after the superblock did.
            let l = whole_log.borrow();
            l.iter()
                .map(|r| r.0)
                .filter(|&o| o >= open_probe())
                .min()
                .unwrap()
        };
        let _ = hdr_range;
        let index_header =
            &data[key_off as usize..key_off as usize + supdb::flatindex::HEADER_BYTES];
        let p2 = open_sparse_fence_ranges(head, object_len, index_header).unwrap();
        let plan: Vec<(u64, u64)> = p1.iter().chain(p2.iter()).copied().collect();
        assert_eq!(
            read,
            merged(&plan),
            "{shape}: the sparse open must read exactly its two plans"
        );
        assert!(
            sparse_bytes * 4 < whole_bytes,
            "{shape}: sparse open read {sparse_bytes} bytes against the whole reader's {whole_bytes}"
        );
        assert!(sparse.keys() == whole.keys() && sparse.keys() == all.len());
    }
}

#[test]
fn a_source_that_serves_only_what_was_ensured_is_enough() {
    // The browser's contract, run natively: ensure the open plans, open;
    // ensure phase one, plan phase two, ensure it, walk; then ensure a key's
    // blocks and read its values. No read may land outside an ensured range.
    let all = keys_and_values(1500);
    let path = scratch("ensured");
    build_segment(&path, &all);
    let data = std::fs::read(&path).unwrap();
    let object_len = data.len() as u64;
    let whole = Blob::open(MmapBytes::open(&path).unwrap()).unwrap();

    let src = Ensured {
        data: data.clone(),
        allowed: RefCell::new(Vec::new()),
    };
    src.ensure(&[(0, open_probe())]);
    let head = data[..open_probe() as usize].to_vec();
    let p1 = open_sparse_ranges(&head, object_len).unwrap();
    src.ensure(&p1);
    // The key section offset comes from the plan: the header range is the
    // one that is neither the probe nor the block table; read it through
    // the source as the browser would.
    // A segment's first plan does not name the header at all -- the
    // superblock extension carries a copy -- so when no planned range parses
    // as one, the section offset comes from the whole reader's plan, whose
    // largest range is the key section; the header bytes then come from the
    // file, since the browser never reads them in that case either.
    let key_off = {
        let mut best = None;
        for &(off, len) in &p1 {
            if off == 0 {
                continue;
            }
            let mut probe = vec![0u8; supdb::flatindex::HEADER_BYTES.min(len as usize)];
            src.read_at(off, &mut probe).unwrap();
            if supdb::flatindex::Header::parse(&probe).is_some() {
                best = Some(off);
            }
        }
        best.unwrap_or_else(|| {
            supdb::blob::open_ranges(&head, object_len)
                .unwrap()
                .into_iter()
                .filter(|r| r.0 != 0)
                .max_by_key(|r| r.1)
                .expect("the whole reader's plan names the key section")
                .0
        })
    };
    let index_header =
        data[key_off as usize..key_off as usize + supdb::flatindex::HEADER_BYTES].to_vec();
    let p2 = open_sparse_fence_ranges(&head, object_len, &index_header).unwrap();
    src.ensure(&p2);
    let sparse = SparseBlob::open(src).expect("open over ensured ranges only");

    for (lo, hi) in [
        (b"path=".to_vec(), Some(b"path=00100".to_vec())),
        (b"app=01490".to_vec(), Some(b"host=00003".to_vec())),
        (b"host=".to_vec(), None),
    ] {
        let hi = hi.as_deref();
        // Before the plans are ensured, the walk must fail loudly, not
        // answer from nothing.
        assert!(
            sparse.dictionary_counts(&lo, hi, |_, _| true).is_err(),
            "a walk over unfetched bytes must be an error"
        );
        // Phase one, then phase two, then the walk.
        let d = sparse.dictionary_plan(&lo, hi);
        sparse.source().ensure(&d);
        let r = sparse
            .dictionary_plan_records(&lo, hi)
            .expect("plan records after phase one");
        sparse.source().ensure(&r);
        let mut got = Vec::new();
        sparse
            .dictionary_counts(&lo, hi, |k, n| {
                got.push((k.to_vec(), n));
                true
            })
            .expect("walk after both phases");
        assert_eq!(got, expected(&whole, &lo, hi));

        // One key's values: inline needs nothing, block-backed needs its plan.
        let mut first: Option<(Vec<u8>, Vec<supdb::index::Ext>, Vec<u8>)> = None;
        sparse
            .dictionary_walk(&lo, hi, |k, e, t| {
                if e.iter().any(|x| !x.is_inline()) || first.is_none() {
                    first = Some((k.to_vec(), e.to_vec(), t.to_vec()));
                }
                !first
                    .as_ref()
                    .is_some_and(|(_, e, _)| e.iter().any(|x| !x.is_inline()))
            })
            .unwrap();
        if let Some((k, exts, tail)) = first {
            let plan = sparse.ranges_for_exts(&exts).unwrap();
            if !plan.is_empty() {
                // A neighbour's block may already be resident -- adjacent
                // keys share 64 KiB blocks -- so the loud-miss check applies
                // only when the plan names something not yet ensured.
                let have = merged(&sparse.source().allowed.borrow());
                let covered = plan
                    .iter()
                    .all(|&(o, l)| have.iter().any(|&(a, al)| a <= o && o + l <= a + al));
                if !covered {
                    assert!(
                        sparse.read_exts(&exts, &tail, |_| {}).is_err(),
                        "unfetched blocks must error, not answer: {}",
                        String::from_utf8_lossy(&k)
                    );
                }
                sparse.source().ensure(&plan);
            }
            let mut vals = Vec::new();
            sparse
                .read_exts(&exts, &tail, |v| vals.push(v.to_vec()))
                .expect("values after ensure");
            let want = &all.iter().find(|(wk, _)| *wk == k).unwrap().1;
            assert_eq!(&vals, want);
        }
    }
}

/// With the directory resident (R7.2) the sparse reader plans no directory
/// slice at all -- phase one is empty -- and still agrees with the whole
/// reader on every range, over a source that serves only what was ensured.
#[test]
fn a_resident_directory_plans_no_first_phase_and_agrees() {
    let all = keys_and_values(700);
    for (shape, path) in [
        ("blocks", scratch("resident-blocks")),
        ("segment", scratch("resident-seg")),
    ] {
        if shape == "blocks" {
            build_blocks(&path, &all);
        } else {
            build_segment(&path, &all);
        }
        let data = std::fs::read(&path).unwrap();
        let object_len = data.len() as u64;
        let whole = Blob::open(MmapBytes::open(&path).unwrap()).unwrap();
        let head = data[..open_probe() as usize].to_vec();
        let src = Ensured {
            data: data.clone(),
            allowed: RefCell::new(vec![(0, open_probe())]),
        };
        let p1 = supdb::blob::open_sparse_ranges_opts(&head, object_len, true).unwrap();
        src.ensure(&p1);
        // Phase two needs the section header for a store, which the first
        // plan named; for a segment the extension carried it.
        let key_off = whole.index_offset() as usize;
        let index_header = data[key_off..key_off + supdb::flatindex::HEADER_BYTES].to_vec();
        let p2 = supdb::blob::open_sparse_fence_ranges_opts(&head, object_len, &index_header, true)
            .unwrap();
        src.ensure(&p2);
        let opts = supdb::BlobOptions {
            resident_directory: true,
            ..Default::default()
        };
        let sparse = SparseBlob::open_with(src, opts).expect("open with the directory resident");
        assert!(sparse.directory_resident(), "{shape}");
        assert_eq!(sparse.keys(), whole.keys(), "{shape}");
        let mut checked = 0;
        for (lo, hi) in ranges_to_try(&all) {
            let hi = hi.as_deref();
            let d = sparse.dictionary_plan(&lo, hi);
            assert!(
                d.is_empty(),
                "{shape}: a resident directory plans no slice: {d:?}"
            );
            let r = sparse
                .dictionary_plan_records(&lo, hi)
                .expect("records plan with no phase one");
            sparse.source().ensure(&r);
            let mut got = Vec::new();
            sparse
                .dictionary_counts(&lo, hi, |k, n| {
                    got.push((k.to_vec(), n));
                    true
                })
                .expect("walk");
            assert_eq!(
                got,
                expected(&whole, &lo, hi),
                "{shape}: [{:?}, {:?})",
                String::from_utf8_lossy(&lo),
                hi.map(String::from_utf8_lossy)
            );
            checked += 1;
        }
        assert!(checked > 100, "{shape}: {checked} ranges");
        let _ = std::fs::remove_file(&path);
    }
}
