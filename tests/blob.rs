//! `blob::Blob` over a segment, against what was written.
//!
//! `Blob` is one of three read paths -- itself, `SparseBlob`, and the JS
//! reader over the same wasm -- and a read path is a liability unless
//! something forces it to agree, because the failure mode is not a crash but
//! a browser quietly answering a different question from the server. So every
//! test here writes a segment through the writer that ships and requires the
//! reader to return the same keys, the same values in the same order, and the
//! same counts.
//!
//! The cross-path check that matters most is here rather than against a
//! second reader: a source that *cannot* lend its bytes must answer
//! identically to one that can (R2.1). That is the browser's shape, and it is
//! the seam where a difference would be the reader's rather than the format's.
//! `tests/dict.rs` holds `SparseBlob` to this reader over every range.
//!
//! It also pins the property a correctness test would otherwise let rot: that
//! the native path stays zero-copy (R2.3).

use std::path::{Path, PathBuf};
use supdb::bytes::{Bytes, MmapBytes, VecBytes};
use supdb::next::SegmentWriter;
use supdb::{Blob, Options};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-blobtest-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d.join("segment.supdb")
}

/// A key-multivalue segment shaped like a logshed day index: a few thousand
/// keys, wildly uneven run lengths, values grouped by key.
///
/// The run lengths straddle `inline_max`, so the fixture covers a run that
/// lives in its index record, one that spills to a block, and one large
/// enough to span several -- the three shapes `with_extent` has to plan for.
fn build(path: &Path, keys: usize, compress: bool) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    build_with(path, keys, compress, 256)
}

fn build_with(
    path: &Path,
    keys: usize,
    compress: bool,
    inline_max: usize,
) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut w = SegmentWriter::create(path, &Options::default()).expect("create");
    w.set_compress(compress);
    w.set_inline_max(inline_max);
    let mut want = Vec::new();
    // Zero-padded, so ascending index order is the byte order the writer
    // requires.
    for k in 0..keys {
        let key = format!("term={k:08}").into_bytes();
        let n = match k % 7 {
            0 => 1,
            1 => 2,
            2 => 17,
            3 => 200,
            4 => 1500,
            5 => 3,
            _ => 40,
        };
        let mut vals = Vec::with_capacity(n);
        w.begin(&key).expect("begin");
        for i in 0..n {
            let v = format!("{k}:{i}").into_bytes();
            w.value(&v);
            vals.push(v);
        }
        w.end().expect("end");
        want.push((key, vals));
    }
    w.finish(1).expect("finish");
    want
}

/// Every value of every key, read through `Blob`, compared against what was
/// written.
fn reads_what_was_written<B: Bytes>(blob: &Blob<B>, want: &[(Vec<u8>, Vec<Vec<u8>>)]) {
    reads_what_was_written_opts(blob, want, true)
}

/// `check_stored` is for fixtures whose stored bytes follow from the values
/// alone; a key holding a fixed run beside a prefixed one does not.
fn reads_what_was_written_opts<B: Bytes>(
    blob: &Blob<B>,
    want: &[(Vec<u8>, Vec<Vec<u8>>)],
    check_stored: bool,
) {
    assert_eq!(blob.keys(), want.len(), "key count");

    for (key, vals) in want {
        // R4.2 -- the values, in order.
        let mut got: Vec<Vec<u8>> = Vec::new();
        let n = blob.read_all(key, |v| got.push(v.to_vec())).expect("blob");
        assert_eq!(&got, vals, "values of {}", String::from_utf8_lossy(key));
        assert_eq!(n, vals.len() as u64, "read_all returns the value count");

        // R4.3 -- the count, without materialising anything.
        assert_eq!(
            blob.count(key).expect("count"),
            vals.len() as u64,
            "count of {}",
            String::from_utf8_lossy(key)
        );
        // O(extents), on the quantity the format does record. Since format
        // v6 that depends on the run's encoding: values that all share one
        // width are stored back to back with no prefixes (Ext::FIXED), so
        // the stored bytes are the payload alone; a mixed run pays a
        // one-byte varint prefix per value, every value here being short.
        let uniform = !vals.is_empty()
            && !vals[0].is_empty()
            && vals.iter().all(|v| v.len() == vals[0].len());
        let stored: u64 = vals
            .iter()
            .map(|v| v.len() as u64 + if uniform { 0 } else { 1 })
            .sum();
        if check_stored {
            assert_eq!(
                blob.stored_bytes(key),
                stored,
                "stored bytes of {}",
                String::from_utf8_lossy(key)
            );
        }
    }

    // A key that is not there is zero, not an error and not a panic.
    assert_eq!(blob.count(b"term=nosuch").unwrap(), 0);
    assert_eq!(blob.stored_bytes(b"term=nosuch"), 0);
    assert_eq!(blob.read_all(b"term=nosuch", |_| {}).unwrap(), 0);
}

#[test]
fn blob_reads_back_every_key_of_a_segment() {
    let path = scratch("agree");
    let want = build(&path, 500, false);
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    assert!(blob.zero_copy(), "R2.3: the native path must not copy");
    reads_what_was_written(&blob, &want);
}

/// The same file, read through a source with no memory behind it.
///
/// This is the browser's shape: an OPFS handle answers `read_at` and cannot
/// answer `slice_at`. If the two disagree, the difference is in the seam
/// rather than in the format, which is exactly what this file exists to catch.
struct Copying(Vec<u8>);

impl Bytes for Copying {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        let end = off as usize + dst.len();
        if end > self.0.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "short",
            ));
        }
        dst.copy_from_slice(&self.0[off as usize..end]);
        Ok(())
    }
}

#[test]
fn a_source_that_cannot_lend_answers_the_same() {
    let path = scratch("copying");
    let want = build(&path, 300, false);
    let raw = std::fs::read(&path).unwrap();
    let blob = Blob::open(Copying(raw)).expect("blob open over a copying source");
    assert!(
        !blob.zero_copy(),
        "this source has nothing to lend, and the test is worthless if it does"
    );
    reads_what_was_written(&blob, &want);
}

#[test]
fn compressed_blocks_read_the_same_as_plain_ones() {
    // A compressed segment takes the chunked and solo arms of `with_extent`
    // that a plain one never reaches, and its inline runs stay uncompressed
    // in the key section either way (R7.4).
    let path = scratch("compressed");
    let want = build(&path, 200, true);
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    reads_what_was_written(&blob, &want);
}

#[test]
fn scanning_walks_the_dictionary_in_key_order_with_counts() {
    let path = scratch("scan");
    let want = build(&path, 400, false);
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");

    // R4.4 -- from a prefix, in order, with each key's count.
    let mut seen: Vec<(Vec<u8>, u64)> = Vec::new();
    blob.scan_counts(b"term=", usize::MAX, |k, n| {
        seen.push((k.to_vec(), n));
        true
    })
    .expect("scan");
    assert_eq!(seen.len(), want.len());
    assert!(
        seen.windows(2).all(|w| w[0].0 < w[1].0),
        "a scan must be ordered"
    );
    for (k, n) in &seen {
        let expect = want.iter().find(|(wk, _)| wk == k).expect("known key");
        assert_eq!(*n, expect.1.len() as u64, "scan count matches read_all");
    }

    // Seeking into the middle starts where it says it does.
    let from = want[123].0.clone();
    let mut first = Vec::new();
    blob.scan_counts(&from, 1, |k, _| {
        first = k.to_vec();
        true
    })
    .unwrap();
    assert_eq!(first, from);

    // A limit is a limit.
    let mut n = 0;
    blob.scan_counts(b"term=", 10, |_, _| {
        n += 1;
        true
    })
    .unwrap();
    assert_eq!(n, 10);

    // Returning false stops the walk.
    let mut n = 0;
    blob.scan_counts(b"term=", usize::MAX, |_, _| {
        n += 1;
        n < 5
    })
    .unwrap();
    assert_eq!(n, 5);

    // The O(extents) form must agree key for key. The fixture's values are
    // `k:i` strings of varying length, so most keys are *not* fixed width and
    // must come back as None rather than as a wrong number -- which is the
    // half of this that matters.
    let mut fixed: Vec<(Vec<u8>, Option<u64>)> = Vec::new();
    blob.scan_counts_fixed(b"term=", usize::MAX, 3, |k, n| {
        fixed.push((k.to_vec(), n));
        true
    })
    .expect("scan fixed");
    assert_eq!(fixed.len(), seen.len(), "both scans visit the same keys");
    for ((k, got), (wk, want)) in fixed.iter().zip(seen.iter()) {
        assert_eq!(k, wk, "the two scans must walk in the same order");
        if let Some(got) = got {
            assert_eq!(got, want, "a count it claims must be the right one");
        }
    }
    assert!(
        fixed.iter().any(|(_, n)| n.is_none()),
        "this fixture has variable-width values, so the guard must fire somewhere"
    );

    // The full scan visits the same keys and every value under them.
    let mut pairs = 0usize;
    blob.scan(b"term=", usize::MAX, |_, _| pairs += 1).unwrap();
    let total: usize = want.iter().map(|(_, v)| v.len()).sum();
    assert_eq!(pairs, total);
}

/// Damage anywhere in the object must produce an error or a wrong-but-bounded
/// answer -- never a panic in the calling process, which for a library
/// embedded in a browser means the worker dying mid-query.
#[test]
fn damaged_objects_do_not_panic_the_caller() {
    let path = scratch("damage");
    build(&path, 120, false);
    let clean = std::fs::read(&path).unwrap();

    let mut hit = 0;
    for seed in 0..600u64 {
        let mut bytes = clean.clone();
        // xorshift, so the damage is spread over the whole object rather than
        // clustered where a modulo would put it.
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for _ in 0..3 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let at = (x as usize) % bytes.len();
            bytes[at] ^= 0xff;
        }
        let Ok(blob) = Blob::open(VecBytes(bytes)) else {
            hit += 1;
            continue;
        };
        // Whatever it answers, it must answer rather than abort.
        let _ = blob.count(b"term=00000005");
        let _ = blob.read_all(b"term=00000005", |_| {});
        let _ = blob.scan_counts(b"term=", 32, |_, _| true);
        let _ = blob.stored_bytes(b"term=00000005");
        let _ = blob.count_fixed(b"term=00000005", 4);
        hit += 1;
    }
    assert_eq!(hit, 600, "every trial must complete one way or the other");
}

/// R4.3, the part that is a real O(extents) count.
///
/// A posting list of fixed-width values -- which is what logshed writes, four
/// bytes of line ordinal -- has a count that falls out of the extent list with
/// no block touched. This checks the arithmetic against the walk on a segment
/// whose keys span one extent and many, and checks that a schema which is
/// *not* fixed width is refused rather than answered wrongly.
///
/// It also pins a property of the writer that the reader's shape depends on:
/// one `end()` emits one extent, and a run too large for a block takes a
/// block to itself so its values stay contiguous. Every extent list in a
/// segment therefore has length one, however long the run -- 20,000 four-byte
/// postings here against a 4 KiB block. `count_fixed` still sums per extent,
/// because a key read through `Db` has a run in each segment holding it, and
/// because a writer that started fragmenting runs would change what a point
/// read costs; this is where that change would surface.
#[test]
fn a_fixed_width_schema_counts_without_touching_a_block() {
    let path = scratch("fixed");
    // Small blocks, so the largest run is many times a block: it must still
    // arrive as one extent.
    let opts = Options {
        block_size: 4096,
        ..Options::default()
    };
    let mut w = SegmentWriter::create(&path, &opts).expect("create");
    let mut want: Vec<(Vec<u8>, u64)> = Vec::new();
    for k in 0..400u32 {
        let key = format!("term={k:08}").into_bytes();
        // Deliberately across the extent boundary: 20,000 four-byte postings
        // do not fit one 4 KiB block.
        let n = [1u32, 5, 900, 20_000][(k % 4) as usize];
        w.begin(&key).expect("begin");
        for i in 0..n {
            w.value(&i.to_le_bytes());
        }
        w.end().expect("end");
        want.push((key, n as u64));
    }
    // One key whose values are not all the same width, to prove the guard.
    // It sorts after every `term=` key, so the writer's byte order holds.
    w.begin(b"zmixed").expect("begin");
    w.value(b"aaaa");
    w.value(b"bbbbbbbbbb");
    w.end().expect("end");
    w.finish(1).expect("finish");

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    for (key, n) in &want {
        assert_eq!(blob.count(key).unwrap(), *n, "the walk is the authority");
        assert_eq!(
            blob.count_fixed(key, 4),
            Some(*n),
            "count_fixed on {}",
            String::from_utf8_lossy(key)
        );
        assert_eq!(
            blob.lookup(key).map_or(0, |e| e.len()),
            1,
            "one run, one extent, whatever the block size: {}",
            String::from_utf8_lossy(key)
        );
    }
    // A run of two different widths declines rather than guesses.
    assert_eq!(blob.count(b"zmixed").unwrap(), 2);
    assert_eq!(blob.count_fixed(b"zmixed", 4), None);
    assert_eq!(blob.count_fixed(b"zmixed", 10), None);
}

/// The case that made `count_fixed` check `Ext::last` as well as divisibility.
///
/// Seventeen values of varying length whose bytes happen to divide exactly by
/// a stride of four: divisibility alone answers 23, confidently and wrongly.
/// `Ext::last` is the offset of the final record, which the format already
/// stores so that reading the newest value is O(1), and for 23 records of
/// stride 4 it would have to be 88 where it is actually 87. Two independent
/// quantities have to agree before a count is claimed.
///
/// Run against both encodings of the same run. A run this size lives in its
/// index record by default, and the guard has to hold there as well as in a
/// block -- an inline extent names `Ext::INLINE` rather than a block id, and
/// nothing else about the arithmetic changes, which is exactly the kind of
/// "nothing else changes" worth a test.
#[test]
fn a_count_from_the_extent_list_checks_two_quantities_not_one() {
    // "16:0".."16:16": ten values of four bytes and seven of five, each with
    // a one-byte length prefix, so 10*5 + 7*6 = 92 stored bytes -- exactly
    // 23 strides of 4.
    let key = b"term=00000016";
    for (name, inline_max) in [("guard-inline", 256), ("guard-block", 0)] {
        let path = scratch(name);
        let mut w = SegmentWriter::create(&path, &Options::default()).expect("create");
        w.set_inline_max(inline_max);
        w.begin(key).expect("begin");
        for i in 0..17 {
            w.value(format!("16:{i}").as_bytes());
        }
        w.end().expect("end");
        w.finish(1).expect("finish");

        let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
        assert_eq!(blob.count(key).unwrap(), 17, "the walk is the authority");
        assert_eq!(blob.stored_bytes(key) % 4, 0, "the trap needs divisibility");
        assert_eq!(
            blob.count_fixed(key, 3),
            None,
            "divisibility alone would have answered {} ({name})",
            blob.stored_bytes(key) / 4
        );
    }

    // The same key really is fixed width at its own width, and is answered.
    let path = scratch("guard-ok");
    let mut w = SegmentWriter::create(&path, &Options::default()).expect("create");
    w.begin(key).expect("begin");
    for i in 0..17u32 {
        w.value(&i.to_le_bytes());
    }
    w.end().expect("end");
    w.finish(1).expect("finish");
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    assert_eq!(blob.count_fixed(key, 4), Some(17));
}

/// The under-return the downstream requirements document reported: corrupt a
/// byte *inside* a data block and the store still opens cleanly -- the header,
/// key index and block table are untouched -- so the only place the damage can
/// surface is the read itself. It must surface as an error. An empty or
/// partial answer here is the browser quietly answering a different question,
/// which is the single thing this index may never do.
///
/// The engine side already held: `verify` fails the checksum and `read_all`
/// propagates it. What swallowed it was the JS glue comparing a wasm `u32`
/// failure sentinel (arriving as a signed -1) against 4294967295; this test
/// pins the native half, `web/test/node.mjs` pins the JS half.
#[test]
fn a_corrupted_block_byte_fails_the_read_rather_than_under_returning() {
    let path = scratch("corrupt-block");
    let want = build(&path, 120, false);
    // k=4 holds 1,500 values: too many to live in its index record, so the
    // run is in a block and the damage has somewhere to land. A key whose run
    // is inline plans no fetch at all (R7.3) and could not be damaged this
    // way, which is the point of choosing deliberately.
    let key = want[4].0.clone();
    let clean = std::fs::read(&path).unwrap();

    // Find a byte inside the key's own extent -- not merely inside its block,
    // whose other chunks belong to other keys and are not verified by this
    // key's read. `ranges_for` names the extent's block as (off, stored), and
    // the extent's bytes start `e.off` into it.
    let blob = Blob::open(VecBytes(clean.clone())).expect("clean open");
    let exts = blob.lookup(&key).expect("key exists");
    assert_eq!(exts.len(), 1, "the fixture key must hold a single extent");
    let e = exts[0];
    let ranges = blob.ranges_for(&key).expect("plan");
    assert_eq!(ranges.len(), 1);
    // The plan is the 4 KiB chunks the run spans (R7.3), starting at a chunk
    // boundary at or before the run, so the run begins `e.off % 4096` into
    // it. Flip its second byte: inside the run, inside the first chunk.
    let at = (ranges[0].0 + e.off as u64 % 4096 + 1) as usize;
    // A key that lives in a different block: verification granularity is the
    // chunk, and a neighbour sharing this one's chunk would rightly fail too.
    let overlaps = |p: &[(u64, u64)]| {
        p.iter()
            .any(|&(o, l)| ranges.iter().any(|&(ro, rl)| o < ro + rl && ro < o + l))
    };
    let (other, other_vals) = want
        .iter()
        .find(|(k, _)| {
            let p = blob.ranges_for(k).unwrap();
            !p.is_empty() && !overlaps(&p)
        })
        .expect("the fixture spans more than one chunk")
        .clone();
    drop(blob);

    let mut bytes = clean;
    bytes[at] ^= 0xff;
    let blob = Blob::open(VecBytes(bytes)).expect("damage inside a block does not stop the open");

    let err = blob
        .read_all(&key, |_| {})
        .expect_err("a checksum mismatch must fail the read, not empty it");
    assert!(
        err.to_string().contains("checksum"),
        "unhelpful error: {err}"
    );
    // The count does not touch the block: since format v5 it is read out of
    // the extent record, so damage inside the block cannot reach it and it
    // must still answer rather than fail.
    assert_eq!(
        blob.count(&key).expect("a count comes from the index"),
        want[4].1.len() as u64
    );
    // The *second* read of the same block is its own regression: the first
    // version of `Blob::verify` marked a chunk verified before comparing its
    // checksum, so one failed read poisoned the bitmap and the next read
    // served the corrupt bytes as already-verified.
    let err = blob
        .read_all(&key, |_| {})
        .expect_err("the failure must repeat, not report once and go quiet");
    assert!(
        err.to_string().contains("checksum"),
        "unhelpful error: {err}"
    );

    // A key in another block still answers: the damage is one block's, not
    // the file's.
    let mut got = Vec::new();
    blob.read_all(&other, |v| got.push(v.to_vec()))
        .expect("undamaged key");
    assert_eq!(got, other_vals);
}

#[test]
fn a_truncated_object_is_refused_at_open() {
    let path = scratch("truncated");
    build(&path, 50, false);
    let clean = std::fs::read(&path).unwrap();
    let cut = clean[..clean.len() / 2].to_vec();
    let err = match Blob::open(VecBytes(cut)) {
        Ok(_) => panic!("a half file is not a checkpoint"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("truncated") || err.kind() == std::io::ErrorKind::InvalidData,
        "unhelpful error: {err}"
    );
}

#[test]
fn the_writer_stores_uniform_runs_without_prefixes() {
    // Format v6: a run whose values all share a width is stored back to back
    // with no varint prefixes and flagged `Ext::FIXED`; a mixed run is not.
    // The reader has to branch on the flag, and a run large enough to spill
    // to a block has to keep the flag across every extent it spans.
    let path = scratch("fixed-runs");
    let mut w = SegmentWriter::create(&path, &Options::default()).expect("create");
    let mut want: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
    let mut a = Vec::new();
    w.begin(b"fixed").expect("begin");
    for i in 0u32..2000 {
        let v = i.to_be_bytes().to_vec();
        w.value(&v);
        a.push(v);
    }
    w.end().expect("end");
    want.push((b"fixed".to_vec(), a));
    let mut b = Vec::new();
    w.begin(b"mixed").expect("begin");
    for i in 0u32..300 {
        let v = format!("{i}").into_bytes();
        w.value(&v);
        b.push(v);
    }
    w.end().expect("end");
    want.push((b"mixed".to_vec(), b));
    w.finish(1).expect("finish");

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).unwrap();
    let ef = blob.lookup(b"fixed").unwrap();
    assert!(
        ef.iter().all(|e| e.is_fixed()),
        "a uniform run is stored fixed: {ef:?}"
    );
    assert_eq!(
        ef.iter().map(|e| e.len as u64).sum::<u64>(),
        8000,
        "no prefixes"
    );
    assert_eq!(blob.count_fixed(b"fixed", 4), Some(2000));
    assert_eq!(blob.stored_bytes(b"fixed"), 8000);
    let em = blob.lookup(b"mixed").unwrap();
    assert!(em.iter().all(|e| !e.is_fixed()));
    assert_eq!(
        blob.count_fixed(b"mixed", 4),
        None,
        "a mixed run declines rather than guessing"
    );
    reads_what_was_written(&blob, &want);
}
