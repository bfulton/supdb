//! `blob::Blob` against `store::Reader`, on the same file.
//!
//! `Blob` is a second read path. A second read path is a liability unless
//! something forces it to agree with the first one, because the failure mode
//! is not a crash -- it is a browser quietly answering a different question
//! from the server. So every test here opens a store written by `store.rs` and
//! requires the two readers to return the same keys, the same values in the
//! same order, and the same counts.
//!
//! It also pins the two properties that a correctness test would otherwise let
//! rot: that the native path stays zero-copy (R2.3), and that a source which
//! *cannot* lend its bytes answers identically to one that can (R2.1), which
//! is the only thing standing between this and the browser.

use std::path::{Path, PathBuf};
use supdb::bytes::{Bytes, MmapBytes, VecBytes};
use supdb::{Blob, Options, Reader, Store};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-blobtest-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d.join("store.supdb")
}

/// A key-multivalue store shaped like a logshed day index: a few thousand
/// keys, wildly uneven run lengths, values grouped by key.
fn build(path: &Path, keys: usize, opts: Options) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let store = Store::create(path, opts).expect("create");
    let mut want = Vec::new();
    for k in 0..keys {
        let key = format!("term={k:08}").into_bytes();
        // Run lengths spanning one value to a few thousand, so the fixture
        // covers an inline extent, a spilled extent list and a solo block.
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
        for i in 0..n {
            let v = format!("{k}:{i}").into_bytes();
            store.append(&key, &v).expect("append");
            vals.push(v);
        }
        want.push((key, vals));
    }
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");
    want
}

/// Every value of every key, read through `Blob`, compared against what was
/// written and against what `Reader` says.
fn agrees_with_reader<B: Bytes>(blob: &Blob<B>, path: &Path, want: &[(Vec<u8>, Vec<Vec<u8>>)]) {
    agrees_with_reader_opts(blob, path, want, true)
}

/// `check_stored` is for fixtures sealed in one run per key, where the
/// stored bytes follow from the values alone; a key sealed in several runs
/// may hold a fixed run beside a prefixed one and its stored bytes depend
/// on how the store cut them.
fn agrees_with_reader_opts<B: Bytes>(
    blob: &Blob<B>,
    path: &Path,
    want: &[(Vec<u8>, Vec<Vec<u8>>)],
    check_stored: bool,
) {
    let r = Reader::open(path).expect("reader open");
    assert_eq!(blob.keys(), r.keys(), "key count");
    assert_eq!(blob.version(), r.version(), "checkpoint identity");

    for (key, vals) in want {
        // R4.2 -- the values, in order.
        let mut got: Vec<Vec<u8>> = Vec::new();
        let n = blob.read_all(key, |v| got.push(v.to_vec())).expect("blob");
        assert_eq!(&got, vals, "values of {}", String::from_utf8_lossy(key));
        assert_eq!(n, vals.len() as u64, "read_all returns the value count");

        let mut from_reader: Vec<Vec<u8>> = Vec::new();
        r.read_all(key, |v| from_reader.push(v.to_vec()))
            .expect("reader");
        assert_eq!(got, from_reader, "the two readers disagree");

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
            assert_eq!(blob.stored_bytes(key), stored, "stored bytes of {}", String::from_utf8_lossy(key));
        }
    }

    // A key that is not there is zero, not an error and not a panic.
    assert_eq!(blob.count(b"term=nosuch").unwrap(), 0);
    assert_eq!(blob.stored_bytes(b"term=nosuch"), 0);
    assert_eq!(blob.read_all(b"term=nosuch", |_| {}).unwrap(), 0);
}

#[test]
fn blob_and_reader_agree_on_a_real_store() {
    let path = scratch("agree");
    let want = build(&path, 500, Options::default());
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    assert!(blob.zero_copy(), "R2.3: the native path must not copy");
    agrees_with_reader(&blob, &path, &want);
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
    let want = build(&path, 300, Options::default());
    let raw = std::fs::read(&path).unwrap();
    let blob = Blob::open(Copying(raw)).expect("blob open over a copying source");
    assert!(
        !blob.zero_copy(),
        "this source has nothing to lend, and the test is worthless if it does"
    );
    agrees_with_reader(&blob, &path, &want);
}

#[test]
fn compressed_blocks_read_the_same_as_plain_ones() {
    // Compression is off by default since f12; a file written with it on is
    // still a file this reader may be handed, and it takes the chunked and
    // solo arms of `with_extent` that the default never reaches.
    let path = scratch("compressed");
    let want = build(
        &path,
        200,
        Options {
            compress: true,
            ..Default::default()
        },
    );
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    agrees_with_reader(&blob, &path, &want);
}

#[test]
fn scanning_walks_the_dictionary_in_key_order_with_counts() {
    let path = scratch("scan");
    let want = build(&path, 400, Options::default());
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
    build(&path, 120, Options::default());
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
/// no block touched. This checks the arithmetic against the walk on a store
/// whose keys span one extent and many, and checks that a schema which is
/// *not* fixed width is refused rather than answered wrongly.
#[test]
fn a_fixed_width_schema_counts_without_touching_a_block() {
    let path = scratch("fixed");
    let store = Store::create(&path, Options::default()).expect("create");
    let mut want: Vec<(Vec<u8>, u64)> = Vec::new();
    for k in 0..400u32 {
        let key = format!("term={k:08}").into_bytes();
        // Deliberately across the extent boundary: a few thousand four-byte
        // postings do not fit in one 64 KiB block.
        let n = [1u32, 5, 900, 20_000][(k % 4) as usize];
        for i in 0..n {
            store.append(&key, &i.to_le_bytes()).expect("append");
        }
        want.push((key, n as u64));
    }
    // One key whose values are not all the same width, to prove the guard.
    store.append(b"mixed", b"aaaa").unwrap();
    store.append(b"mixed", b"bbbbbbbbbb").unwrap();
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    for (key, n) in &want {
        assert_eq!(blob.count(key).unwrap(), *n, "walked count");
        assert_eq!(
            blob.count_fixed(key, 4),
            Some(*n),
            "O(extents) count of {}",
            String::from_utf8_lossy(key)
        );
    }
    // 4 + 1 and 10 + 1 is 15 bytes, which is not a multiple of 5, so the
    // guard fires. It is a necessary condition, not a sufficient one, and the
    // doc comment says so.
    assert_eq!(blob.count(b"mixed").unwrap(), 2);
    assert_eq!(blob.count_fixed(b"mixed", 4), None);

    // The dictionary scan in its O(extents) form, which is what a breakdown
    // panel calls. Every fixed-width key must come back with the count the
    // walk gives, and the one mixed key must come back as None.
    let mut rows: Vec<(Vec<u8>, Option<u64>)> = Vec::new();
    blob.scan_counts_fixed(b"", usize::MAX, 4, |k, n| {
        rows.push((k.to_vec(), n));
        true
    })
    .expect("scan fixed");
    assert_eq!(rows.len(), want.len() + 1, "every key, plus `mixed`");
    for (key, n) in &want {
        let (_, got) = rows.iter().find(|(k, _)| k == key).expect("known key");
        assert_eq!(*got, Some(*n), "{}", String::from_utf8_lossy(key));
    }
    assert_eq!(
        rows.iter().find(|(k, _)| k == b"mixed").map(|(_, n)| *n),
        Some(None),
        "the mixed key must decline rather than guess"
    );
}

/// The case that made `count_fixed` check `Ext::last` as well as divisibility.
///
/// Seventeen values of varying length whose bytes happen to divide exactly by
/// a stride of four: divisibility alone answers 23, confidently and wrongly.
/// `Ext::last` is the offset of the final record, which the format already
/// stores so that reading the newest value is O(1), and for 23 records of
/// stride 4 it would have to be 88 where it is actually 87. Two independent
/// quantities have to agree before a count is claimed.
#[test]
fn a_count_from_the_extent_list_checks_two_quantities_not_one() {
    let path = scratch("guard");
    let store = Store::create(&path, Options::default()).expect("create");
    // "2:0".."2:16" -- three bytes for the first ten, four for the rest, so
    // 10*(1+3) + 7*(1+4) = 75 bytes, which is not divisible by 4. Pad to the
    // shape that fooled the old check: the fixture in `build` produces it at
    // k=2, so reproduce that key exactly.
    // "16:0".."16:16": ten values of four bytes and seven of five, each with
    // a one-byte length prefix, so 10*5 + 7*6 = 92 stored bytes -- exactly
    // 23 strides of 4. This is key 16 of the fixture `build` writes, and it
    // is where the old check was found to be wrong.
    let key = b"term=00000016";
    for i in 0..17 {
        store
            .append(key, format!("16:{i}").as_bytes())
            .expect("append");
    }
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    assert_eq!(blob.count(key).unwrap(), 17, "the walk is the authority");
    // 92 stored bytes over a stride of 4 is 23, and 23 is not 17.
    assert_eq!(blob.stored_bytes(key) % 4, 0, "the trap needs divisibility");
    assert_eq!(
        blob.count_fixed(key, 3),
        None,
        "divisibility alone would have answered {}",
        blob.stored_bytes(key) / 4
    );
    // The same key really is fixed width at its own width, and is answered.
    let path2 = scratch("guard-ok");
    let store = Store::create(&path2, Options::default()).expect("create");
    for i in 0..17u32 {
        store.append(key, &i.to_le_bytes()).expect("append");
    }
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");
    let blob = Blob::open(MmapBytes::open(&path2).unwrap()).expect("blob open");
    assert_eq!(blob.count_fixed(key, 4), Some(17));
}

/// A store consolidated by the deferred policy reads identically through
/// both paths, and its fragmented runs keep `count_fixed`'s contract.
///
/// `Options::defer_merge` leaves a key holding a geometric ladder of extents
/// where the inline policy leaves one, so this is the store shape with the
/// most extents per key either reader will ever see. Two things must hold.
/// First, `Blob` and `Reader` agree on every key, value, order and count --
/// the standing bar for a second read path. Second, `count_fixed` checks its
/// two quantities *per extent*, so a fragmented fixed-width run must still
/// answer exactly, and a fragmented mixed-width run must still decline: a
/// merge that concatenated fragments but miscomputed the merged extent's
/// `last` would break the first, and a policy that produced a coincidental
/// stride-multiple would break the second.
///
/// The fragmentation is asserted, not assumed: interleaved appends against a
/// tiny buffer must leave at least one key spilled across several extents,
/// or the test has quietly stopped covering the shape it exists for.
#[test]
fn a_deferred_consolidated_store_reads_the_same_and_counts_exactly() {
    let path = scratch("deferred");
    let store = Store::create(
        &path,
        Options {
            defer_merge: true,
            buffer_bytes: 1 << 16,
            ..Default::default()
        },
    )
    .expect("create");
    // Interleaved fixed-width appends: every key's run is broken by every
    // other key's, which is the fragmenting shape, and four-byte values are
    // logshed's posting schema -- the caller count_fixed exists for.
    let keys = 60u32;
    let depth = 300u32;
    for i in 0..depth {
        for k in 0..keys {
            let key = format!("term={k:08}").into_bytes();
            store.append(&key, &i.to_le_bytes()).expect("append");
        }
    }
    // One mixed-width key, fragmented the same way, to prove the guard still
    // fires on a multi-extent run.
    for i in 0..40u32 {
        let v = if i % 3 == 0 {
            vec![b'x'; 4]
        } else {
            vec![b'y'; 10]
        };
        store.append(b"mixed", &v).expect("append");
    }
    store.checkpoint().expect("checkpoint");
    let stats = store.close().expect("close");
    assert!(stats.merges > 0, "nothing merged, so nothing was deferred");

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    assert!(blob.zero_copy(), "the native path must not start copying");
    let r = Reader::open(&path).expect("reader open");
    assert_eq!(blob.keys(), r.keys(), "key count");
    assert_eq!(blob.version(), r.version(), "checkpoint identity");

    let mut spilled = 0usize;
    for k in 0..keys {
        let key = format!("term={k:08}").into_bytes();
        let exts = blob.lookup(&key).expect("every key is in the index");
        spilled += usize::from(exts.len() > 1);

        // Both readers, same values, same order.
        let mut got: Vec<Vec<u8>> = Vec::new();
        blob.read_all(&key, |v| got.push(v.to_vec())).expect("blob");
        let want: Vec<Vec<u8>> = (0..depth).map(|i| i.to_le_bytes().to_vec()).collect();
        assert_eq!(got, want, "values of {}", String::from_utf8_lossy(&key));
        let mut from_reader: Vec<Vec<u8>> = Vec::new();
        r.read_all(&key, |v| from_reader.push(v.to_vec()))
            .expect("reader");
        assert_eq!(got, from_reader, "the two readers disagree");

        // The O(extents) count must agree with the walk on every fragment
        // ladder the policy produced.
        assert_eq!(blob.count(&key).unwrap(), depth as u64, "walked count");
        assert_eq!(
            blob.count_fixed(&key, 4),
            Some(depth as u64),
            "count_fixed on the fragmented run of {}",
            String::from_utf8_lossy(&key)
        );
    }
    assert!(
        spilled > 0,
        "no key was left multi-extent, so the fragmented shape was never under test"
    );
    // The mixed-width key declines rather than guesses, however it fragmented.
    assert_eq!(blob.count(b"mixed").unwrap(), 40);
    assert_eq!(blob.count_fixed(b"mixed", 4), None);
    assert_eq!(blob.count_fixed(b"mixed", 10), None);
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
    let want = build(&path, 120, Options::default());
    // k=2 holds 17 values: one extent, so its block range is unambiguous.
    let key = want[2].0.clone();
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
    let at = (ranges[0].0 + e.off as u64 + e.len as u64 / 2) as usize;
    // A key that lives in a different block: verification granularity is the
    // block, so a neighbour in the same one would rightly fail too.
    let (other, other_vals) = want
        .iter()
        .find(|(k, _)| blob.ranges_for(k).unwrap() != ranges)
        .expect("the fixture spans more than one block")
        .clone();
    drop(blob);

    let mut bytes = clean;
    bytes[at] ^= 0xff;
    let blob = Blob::open(VecBytes(bytes)).expect("damage inside a block does not stop the open");

    let err = blob
        .read_all(&key, |_| {})
        .expect_err("a checksum mismatch must fail the read, not empty it");
    assert!(err.to_string().contains("checksum"), "unhelpful error: {err}");
    // The count does not touch the block: since format v5 it is read out of
    // the extent record, so damage inside the block cannot reach it and it
    // must still answer rather than fail.
    assert_eq!(
        blob.count(&key).expect("a count comes from the index"),
        want[2].1.len() as u64
    );
    // The *second* read of the same block is its own regression: the first
    // version of `Blob::verify` marked a chunk verified before comparing its
    // checksum, so one failed read poisoned the bitmap and the next read
    // served the corrupt bytes as already-verified.
    let err = blob
        .read_all(&key, |_| {})
        .expect_err("the failure must repeat, not report once and go quiet");
    assert!(err.to_string().contains("checksum"), "unhelpful error: {err}");

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
    build(&path, 50, Options::default());
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
fn the_store_seals_uniform_runs_fixed_and_both_readers_agree() {
    // Format v6 through the original store: a key whose pending values all
    // share a width is sealed without prefixes and flagged; a mixed key is
    // not; a later append of another width to the fixed key is read back in
    // order whether or not consolidation has merged the two runs; and the
    // two readers agree on every key throughout.
    let path = scratch("fixed-store");
    let store = Store::create(&path, Options::default()).expect("create");
    let mut want: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
    let mut a = Vec::new();
    for i in 0u32..2000 {
        let v = i.to_be_bytes().to_vec();
        store.append(b"fixed", &v).unwrap();
        a.push(v);
    }
    want.push((b"fixed".to_vec(), a));
    let mut b = Vec::new();
    for i in 0u32..300 {
        let v = format!("{i}").into_bytes();
        store.append(b"mixed", &v).unwrap();
        b.push(v);
    }
    want.push((b"mixed".to_vec(), b));
    store.checkpoint().unwrap();
    store.close().unwrap();

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).unwrap();
    let ef = blob.lookup(b"fixed").unwrap();
    assert!(ef.iter().all(|e| e.is_fixed()), "a uniform run is sealed fixed: {ef:?}");
    assert_eq!(ef.iter().map(|e| e.len as u64).sum::<u64>(), 8000, "no prefixes");
    assert_eq!(blob.count_fixed(b"fixed", 4), Some(2000));
    assert_eq!(blob.stored_bytes(b"fixed"), 8000);
    let em = blob.lookup(b"mixed").unwrap();
    assert!(em.iter().all(|e| !e.is_fixed()));
    agrees_with_reader(&blob, &path, &want);
    drop(blob);

    // Reopen, append a wider value: the key is no longer uniform.
    let store = Store::open(&path, Options::default()).expect("open");
    store.append(b"fixed", b"wider").unwrap();
    want[0].1.push(b"wider".to_vec());
    store.checkpoint().unwrap();
    store.close().unwrap();
    let blob = Blob::open(MmapBytes::open(&path).unwrap()).unwrap();
    assert_eq!(blob.count(b"fixed").unwrap(), 2001);
    assert_eq!(blob.count_fixed(b"fixed", 4), None, "the run is not all fours any more");
    // Two fixed runs (8,000 + 5 bytes) if the store kept them apart, one
    // prefixed run (2,001 prefixes more) if it consolidated them; either is
    // right and both readers must agree on the values.
    let stored = blob.stored_bytes(b"fixed");
    assert!(stored == 8005 || stored == 10006, "stored bytes {stored}");
    agrees_with_reader_opts(&blob, &path, &want, false);
}
