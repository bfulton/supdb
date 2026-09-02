//! The bulk segment writer against the general one.
//!
//! `next::SegmentWriter` is a second writer of the store format, and a second
//! writer fails the way a second reader does: not by crashing but by producing
//! a file that opens and answers differently. So the same data is written
//! through `Store` and through the writer, and both files are read through
//! `Blob` -- and the writer's file through `store::Reader` too -- and required
//! to agree on every key, every value, every count, and the scan order.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use supdb::bytes::MmapBytes;
use supdb::next::SegmentWriter;
use supdb::{Blob, Options, Reader, SparseBlob, Store};

/// `block::CHECKSUMS` is process-wide -- `Store::create` and the writer both
/// set it from `Options::checksums` -- and the test harness runs tests
/// concurrently, so a checksums-off test flips the switch under a
/// checksums-on one and every block it reads mismatches. Each test holds
/// this for its duration.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-segwriter-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Keys of uneven length, values of uneven length and count, a few values
/// larger than a whole block so a run has to become a block by itself, and a
/// first key whose first value is long enough to corrupt in the middle.
/// Sorted by key, as both writers require of a seal.
fn varlen(keys: usize, seed: u64) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut r = seed;
    let mut out: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::with_capacity(keys + 1);
    out.push((b"\x00first".to_vec(), vec![vec![0xA5u8; 50]]));
    for _ in 0..keys {
        let klen = 1 + (splitmix(&mut r) % 40) as usize;
        let key: Vec<u8> = (0..klen)
            .map(|_| b'a' + (splitmix(&mut r) % 26) as u8)
            .collect();
        let n = 1 + (splitmix(&mut r) % 12) as usize;
        let vals = (0..n)
            .map(|_| {
                let roll = splitmix(&mut r) % 100;
                let vlen = match roll {
                    0 => 9_000,
                    1..=5 => 0,
                    _ => (splitmix(&mut r) % 300) as usize,
                };
                (0..vlen)
                    .map(|_| splitmix(&mut r) as u8)
                    .collect::<Vec<u8>>()
            })
            .collect();
        out.push((key, vals));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Every value the same width, so `count_fixed` has something to claim.
fn fixed(keys: usize, width: usize, seed: u64) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut r = seed;
    let mut out: Vec<(Vec<u8>, Vec<Vec<u8>>)> = (0..keys)
        .map(|i| {
            let n = 1 + (splitmix(&mut r) % 40) as usize;
            let vals = (0..n)
                .map(|_| {
                    (0..width)
                        .map(|_| splitmix(&mut r) as u8)
                        .collect::<Vec<u8>>()
                })
                .collect();
            (format!("term={i:08}").into_bytes(), vals)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn opts() -> Options {
    Options {
        // Small blocks, so a few thousand keys span many of them and the
        // 9,000-byte values overflow one.
        block_size: 4096,
        checksums: true,
        ..Options::default()
    }
}

fn write_store(path: &Path, data: &[(Vec<u8>, Vec<Vec<u8>>)], o: Options) {
    let store = Store::create(path, o).expect("create");
    for (k, vals) in data {
        for v in vals {
            store.append(k, v).expect("append");
        }
    }
    store.checkpoint().expect("checkpoint");
    store.close().expect("close");
}

/// Runs up to this many bytes go inline in the index record; the fixture's
/// 9,000-byte values stay in blocks, so one store exercises both paths.
const INLINE: usize = 256;

fn write_bulk(path: &Path, data: &[(Vec<u8>, Vec<Vec<u8>>)], o: Options) {
    write_bulk_with(path, data, o, INLINE)
}

fn write_bulk_with(path: &Path, data: &[(Vec<u8>, Vec<Vec<u8>>)], o: Options, inline: usize) {
    let mut w = SegmentWriter::create(path, &o).expect("create");
    w.set_inline_max(inline);
    for (k, vals) in data {
        w.begin(k).expect("begin");
        for v in vals {
            w.value(v);
        }
        w.end().expect("end");
    }
    assert_eq!(w.keys(), data.len());
    w.finish(1).expect("finish");
}

fn open(path: &Path) -> Blob<MmapBytes> {
    Blob::open(MmapBytes::open(path).expect("map")).expect("open")
}

/// Everything a reader can ask, asked of both and compared.
fn agree(a: &Blob<MmapBytes>, b: &Blob<MmapBytes>, data: &[(Vec<u8>, Vec<Vec<u8>>)]) {
    assert_eq!(a.keys(), data.len(), "store key count");
    assert_eq!(b.keys(), data.len(), "bulk key count");
    assert!(
        b.zero_copy(),
        "the bulk segment must still be read without copying"
    );
    for (rank, (key, vals)) in data.iter().enumerate() {
        let mut ga = Vec::new();
        let na = a
            .read_all(key, |v| ga.push(v.to_vec()))
            .expect("store read_all");
        let mut gb = Vec::new();
        let nb = b
            .read_all(key, |v| gb.push(v.to_vec()))
            .expect("bulk read_all");
        assert_eq!(na, vals.len() as u64, "store value count for {key:?}");
        assert_eq!(nb, na, "value count differs for {key:?}");
        assert_eq!(&ga, vals, "store values for {key:?}");
        assert_eq!(gb, ga, "values differ for {key:?}");
        assert_eq!(b.count(key).expect("count"), a.count(key).expect("count"));
        let mut cat = Vec::new();
        let nc = b.read_concat(key, &mut cat).expect("read_concat");
        assert_eq!(nc, vals.len() as u64);
        assert_eq!(
            cat,
            vals.concat(),
            "read_concat is the values back to back for {key:?}"
        );
        assert_eq!(
            b.stored_bytes(key),
            a.stored_bytes(key),
            "stored bytes for {key:?}"
        );
        assert_eq!(b.key_at(rank), Some(key.as_slice()), "rank {rank}");
        assert_eq!(
            a.key_at(rank),
            Some(key.as_slice()),
            "rank {rank} in the store"
        );
        assert_eq!(b.seek(key), rank);
        let mut by_rank = Vec::new();
        b.values_at(rank, |v| by_rank.push(v.to_vec()))
            .expect("values_at");
        assert_eq!(&by_rank, vals, "values by rank for {key:?}");
    }
    assert_eq!(a.read_all(b"no such key", |_| {}).unwrap(), 0);
    assert_eq!(b.read_all(b"no such key", |_| {}).unwrap(), 0);
    assert_eq!(b.count(b"no such key").unwrap(), 0);

    let mut sa = Vec::new();
    a.scan(b"", usize::MAX, |k, v| sa.push((k.to_vec(), v.to_vec())))
        .expect("store scan");
    let mut sb = Vec::new();
    b.scan(b"", usize::MAX, |k, v| sb.push((k.to_vec(), v.to_vec())))
        .expect("bulk scan");
    let want: Vec<(Vec<u8>, Vec<u8>)> = data
        .iter()
        .flat_map(|(k, vals)| vals.iter().map(move |v| (k.clone(), v.clone())))
        .collect();
    assert_eq!(sa.len(), want.len(), "store scan length");
    assert_eq!(sa, want, "store scan order");
    assert_eq!(sb, want, "bulk scan order");
}

#[test]
fn bulk_segment_reads_identically_to_a_store_written_one() {
    let _serial = serial();
    let dir = scratch("varlen");
    let data = varlen(3_000, 7);
    write_store(&dir.join("store.sup"), &data, opts());
    write_bulk(&dir.join("bulk.sup"), &data, opts());
    let a = open(&dir.join("store.sup"));
    let b = open(&dir.join("bulk.sup"));
    assert!(
        b.blocks() > 0,
        "the long runs still need blocks, got {}",
        b.blocks()
    );
    agree(&a, &b, &data);
    // The writer's other layout -- every run in a block, section built at
    // the end, the order `Store` writes -- must agree with both as well:
    // two layouts of one format that answered differently would be the
    // second-reader failure mode with a second writer.
    write_bulk_with(&dir.join("blocks.sup"), &data, opts(), 0);
    let c = open(&dir.join("blocks.sup"));
    agree(&a, &c, &data);
    assert!(c
        .lookup(&data[0].0)
        .expect("present")
        .iter()
        .all(|e| !e.is_inline()));
    // Which runs went inline is decided by their byte length, and an inline
    // run plans no fetch: the extent names no block and the read touches
    // the record alone.
    let mut inline = 0usize;
    for (key, vals) in &data {
        // The run's stored size decides: format v6 stores a uniform-width
        // run without prefixes, a mixed one with a varint prefix per value.
        let uniform = !vals.is_empty()
            && vals
                .iter()
                .all(|v| v.len() == vals[0].len() && !v.is_empty());
        let run: usize = vals
            .iter()
            .map(|v| v.len() + if uniform { 0 } else { varint_len(v.len()) })
            .sum();
        let exts = b.lookup(key).expect("present");
        assert_eq!(exts.len(), 1, "one run per key in a bulk segment");
        if run <= INLINE {
            assert!(
                exts[0].is_inline(),
                "a {run}-byte run must be inline for {key:?}"
            );
            assert!(
                b.ranges_for(key).expect("plan").is_empty(),
                "an inline run fetches nothing"
            );
            inline += 1;
        } else {
            assert!(
                !exts[0].is_inline(),
                "a {run}-byte run must be in a block for {key:?}"
            );
            assert!(!b.ranges_for(key).expect("plan").is_empty());
        }
        assert!(
            a.lookup(key)
                .expect("present")
                .iter()
                .all(|e| !e.is_inline()),
            "Store never inlines"
        );
    }
    // Both paths well exercised: the fixture's uneven run lengths put a
    // minority of keys under the threshold and the rest in blocks.
    assert!(
        inline > data.len() / 10,
        "too few inline runs to test: {inline}"
    );
    assert!(
        data.len() - inline > data.len() / 10,
        "too few block runs to test: {inline} inline"
    );
}

fn varint_len(n: usize) -> usize {
    let mut n = n as u64;
    let mut l = 1;
    while n >= 0x80 {
        n >>= 7;
        l += 1;
    }
    l
}

#[test]
fn bulk_segment_agrees_with_checksums_off_too() {
    let _serial = serial();
    let dir = scratch("nocrc");
    let data = varlen(1_000, 11);
    let o = Options {
        checksums: false,
        ..opts()
    };
    write_store(&dir.join("store.sup"), &data, o.clone());
    write_bulk(&dir.join("bulk.sup"), &data, o);
    agree(
        &open(&dir.join("store.sup")),
        &open(&dir.join("bulk.sup")),
        &data,
    );
}

#[test]
fn fixed_width_counts_come_from_the_extent_alone() {
    let _serial = serial();
    let dir = scratch("fixed");
    let data = fixed(2_000, 4, 3);
    write_store(&dir.join("store.sup"), &data, opts());
    write_bulk(&dir.join("bulk.sup"), &data, opts());
    let a = open(&dir.join("store.sup"));
    let b = open(&dir.join("bulk.sup"));
    agree(&a, &b, &data);
    for (key, vals) in &data {
        assert_eq!(
            b.count_fixed(key, 4),
            Some(vals.len() as u64),
            "count_fixed for {key:?}"
        );
        assert_eq!(b.count_fixed(key, 4), a.count_fixed(key, 4));
        // The wrong stride must be refused, not answered.
        assert_eq!(b.count_fixed(key, 3), a.count_fixed(key, 3));
    }
    let mut seen = 0usize;
    b.scan_counts_fixed(b"term=", usize::MAX, 4, |k, n| {
        assert_eq!(
            n,
            Some(data[seen].1.len() as u64),
            "scan_counts_fixed for {k:?}"
        );
        seen += 1;
        true
    })
    .expect("scan_counts_fixed");
    assert_eq!(seen, data.len());
}

#[test]
fn the_mapped_reader_opens_a_bulk_segment() {
    let _serial = serial();
    let dir = scratch("reader");
    let data = varlen(1_500, 5);
    // Block-backed only: `store::Reader` is the old engine's reader and does
    // not serve inline runs -- a next-engine segment is read through `Blob`.
    write_bulk_with(&dir.join("bulk.sup"), &data, opts(), 0);
    let r = Reader::open(&dir.join("bulk.sup")).expect("Reader::open");
    for (key, vals) in &data {
        let mut got = Vec::new();
        r.read_all(key, |v| got.push(v.to_vec()))
            .expect("reader read_all");
        assert_eq!(&got, vals, "Reader values for {key:?}");
    }
}

#[test]
fn the_checksum_recorded_is_the_one_the_reader_checks() {
    let _serial = serial();
    let dir = scratch("crc");
    let data = varlen(200, 9);
    let path = dir.join("bulk.sup");
    // Block-backed, because the byte this test flips is in the first block
    // and the first key's 50-byte value would otherwise be inline in the
    // index record, where the block checksum does not reach.
    write_bulk_with(&path, &data, opts(), 0);
    // Intact first.
    let b = open(&path);
    assert_eq!(b.read_all(b"\x00first", |_| {}).expect("intact"), 1);
    drop(b);
    // The first key is the first record of the first block, which starts
    // right after the header region; flip a byte inside its 50-byte value.
    let mut bytes = std::fs::read(&path).expect("read");
    let at = 4096 + 1 + 20;
    bytes[at] ^= 0xFF;
    std::fs::write(&path, &bytes).expect("write");
    let b = open(&path);
    let e = b
        .read_all(b"\x00first", |_| {})
        .expect_err("a corrupted block must not be served");
    assert!(
        e.to_string().contains("checksum"),
        "expected a checksum error, got: {e}"
    );
}

#[test]
fn keys_out_of_order_are_refused() {
    let _serial = serial();
    let dir = scratch("order");
    let mut w = SegmentWriter::create(&dir.join("bulk.sup"), &opts()).expect("create");
    w.begin(b"b").unwrap();
    w.value(b"1");
    w.end().unwrap();
    assert!(w.begin(b"a").is_err(), "a smaller key must be refused");
    assert!(w.begin(b"b").is_err(), "a duplicate key must be refused");
    w.begin(b"c").unwrap();
    assert!(
        w.begin(b"d").is_err(),
        "begin while a key is open must be refused"
    );
    w.end().unwrap();
    w.finish(1).unwrap();
    let b = open(&dir.join("bulk.sup"));
    assert_eq!(b.keys(), 2);
    assert_eq!(
        b.count(b"c").unwrap(),
        0,
        "a key closed with no values has none"
    );
}

#[test]
fn a_run_of_one_width_is_stored_without_prefixes_and_reads_the_same() {
    // Format v6: a run whose values share a width carries Ext::FIXED and no
    // per-value length prefix; a mixed run keeps the prefixes. Both must
    // read identically through every path, and the fixed one must be
    // smaller by exactly the prefixes.
    let _g = serial();
    let dir = std::env::temp_dir().join("supdb-segwriter-fixed");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("seg.sup");
    let o = opts();
    let mut w = SegmentWriter::create(&path, &o).expect("create");
    w.set_inline_max(0);
    // 1,000 four-byte values: fixed.
    w.begin(b"fixed").unwrap();
    for i in 0u32..1000 {
        w.value(&i.to_be_bytes());
    }
    w.end().unwrap();
    // Mixed widths: prefixed.
    w.begin(b"mixed").unwrap();
    for i in 0u32..1000 {
        w.value(format!("{i}").as_bytes());
    }
    w.end().unwrap();
    // One value: fixed too (a single width).
    w.begin(b"one").unwrap();
    w.value(b"solo");
    w.end().unwrap();
    w.finish(1).unwrap();

    let blob = Blob::open(MmapBytes::open(&path).unwrap()).unwrap();
    let ef = blob.lookup(b"fixed").unwrap();
    assert_eq!(ef.len(), 1);
    assert!(ef[0].is_fixed(), "a uniform run must be flagged fixed");
    assert_eq!(ef[0].fixed_width(), Some(4));
    assert_eq!(ef[0].len, 4000, "no prefixes: 1,000 x 4 bytes");
    assert_eq!(ef[0].records(), 1000);
    assert_eq!(ef[0].last, 3996);
    let em = blob.lookup(b"mixed").unwrap();
    assert!(!em[0].is_fixed(), "a mixed run stays prefixed");
    let eo = blob.lookup(b"one").unwrap();
    assert!(eo[0].is_fixed());

    let mut got = Vec::new();
    blob.read_all(b"fixed", |v| got.push(v.to_vec())).unwrap();
    assert_eq!(got.len(), 1000);
    for (i, v) in got.iter().enumerate() {
        assert_eq!(v.as_slice(), (i as u32).to_be_bytes());
    }
    let mut got = Vec::new();
    blob.read_all(b"mixed", |v| got.push(v.to_vec())).unwrap();
    assert_eq!(got.len(), 1000);
    assert_eq!(got[999], b"999".to_vec());
    // Counts: exact from the flag, and a wrong width is refused outright.
    assert_eq!(blob.count(b"fixed").unwrap(), 1000);
    assert_eq!(blob.count_fixed(b"fixed", 4), Some(1000));
    assert_eq!(
        blob.count_fixed(b"fixed", 2),
        None,
        "a fixed run of 4 is not a run of 2"
    );
    assert_eq!(blob.count_fixed(b"one", 4), Some(1));
    // The scan agrees.
    let mut pairs = 0usize;
    blob.scan(b"", usize::MAX, |_, _| pairs += 1).unwrap();
    assert_eq!(pairs, 2001);
    // read_concat of a fixed run is the bytes back to back.
    let mut out = Vec::new();
    assert_eq!(blob.read_concat(b"fixed", &mut out).unwrap(), 1000);
    assert_eq!(out.len(), 4000);
    assert_eq!(&out[..8], &[0, 0, 0, 0, 0, 0, 0, 1]);
    // And the in-place intersection over fixed runs matches the naive one.
    let mut w2 = SegmentWriter::create(&dir.join("seg2.sup"), &o).expect("create");
    w2.set_inline_max(0);
    w2.begin(b"a").unwrap();
    for i in (0u32..3000).step_by(3) {
        w2.value(&i.to_be_bytes());
    }
    w2.end().unwrap();
    w2.begin(b"b").unwrap();
    for i in (0u32..3000).step_by(5) {
        w2.value(&i.to_be_bytes());
    }
    w2.end().unwrap();
    w2.finish(1).unwrap();
    let b2 = Blob::open(MmapBytes::open(&dir.join("seg2.sup")).unwrap()).unwrap();
    let common = (0u32..3000).filter(|i| i % 15 == 0).count() as u64;
    assert_eq!(b2.intersect_fixed(b"a", b"b", 4).unwrap(), common);
    assert_eq!(b2.intersect_fixed(b"a", b"missing", 4).unwrap(), 0);
}

/// Every byte of a segment's key index is covered by its checksum row, so a
/// flip anywhere in the section fails the open rather than changing an
/// answer. Format v6 made this the difference between an error and a quiet
/// misread: a flipped FIXED bit re-decodes a run under the other encoding
/// (indexsum-plan.md, P64.1).
#[test]
fn every_flip_in_the_key_section_fails_the_open() {
    let _g = serial();
    let dir = scratch("segwriter-indexsum");
    let path = dir.join("seg.sup");
    let data = fixed(1500, 4, 0x1D5);
    write_bulk(&path, &data, opts());
    let clean = std::fs::read(&path).unwrap();
    let (key_off, key_len) = {
        let b = open(&path);
        assert!(
            b.index_checksummed(),
            "a segment's index carries a checksum row"
        );
        (b.index_offset() as usize, b.index_bytes())
    };
    assert!(
        key_len > 4096,
        "the fixture's index spans several pieces: {key_len}"
    );
    let mut tried = 0usize;
    for at in (key_off..key_off + key_len).step_by(7) {
        let mut bytes = clean.clone();
        bytes[at] ^= 0x40;
        std::fs::write(&path, &bytes).unwrap();
        tried += 1;
        assert!(
            Blob::open(MmapBytes::open(&path).unwrap()).is_err(),
            "a flip at section offset {} opened cleanly",
            at - key_off
        );
    }
    assert!(tried > 500);
    // And with verification off the same file opens, which is the arm f64
    // prices the check against.
    let mut bytes = clean.clone();
    bytes[key_off + key_len / 2] ^= 0x40;
    std::fs::write(&path, &bytes).unwrap();
    let opts = supdb::BlobOptions {
        verify_checksums: true,
        verify_index: false,
        ..Default::default()
    };
    assert!(Blob::open_with(MmapBytes::open(&path).unwrap(), opts).is_ok());
    std::fs::write(&path, &clean).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A segment's superblock page carries an extension -- a copy of the key
/// header and every offset a sparse open needs -- and, with a head reserve,
/// the block table and a copy of the fence right after the page. A source
/// that serves only the first probe then opens the sparse reader with no
/// second round trip, and it agrees with the whole reader (waves-plan.md,
/// P7.1).
#[test]
fn a_head_reserve_opens_the_sparse_reader_from_the_probe_alone() {
    use std::cell::RefCell;
    struct Ensured {
        data: Vec<u8>,
        allowed: RefCell<Vec<(u64, u64)>>,
    }
    impl supdb::Bytes for Ensured {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
            let end = off + dst.len() as u64;
            let ok = self
                .allowed
                .borrow()
                .iter()
                .any(|&(a, l)| a <= off && end <= a + l);
            if !ok {
                return Err(std::io::Error::other(format!(
                    "read outside the probe: {off}+{}",
                    dst.len()
                )));
            }
            dst.copy_from_slice(&self.data[off as usize..end as usize]);
            Ok(())
        }
    }
    let _g = serial();
    let dir = scratch("segwriter-reserve");
    let path = dir.join("seg.sup");
    let data = fixed(1200, 4, 0x7E5);
    let reserve = 128 << 10;
    {
        let mut w = SegmentWriter::create(&path, &opts()).expect("create");
        w.set_inline_max(INLINE);
        w.set_head_reserve(reserve);
        for (k, vals) in &data {
            w.begin(k).expect("begin");
            for v in vals {
                w.value(v);
            }
            w.end().expect("end");
        }
        w.finish(1).expect("finish");
    }
    let whole = open(&path);
    assert!(whole.index_checksummed());
    let bytes = std::fs::read(&path).unwrap();

    // The first plan, made from the page alone, lies inside the probe.
    let probe = 4096 + reserve as u64;
    let head = bytes[..4096].to_vec();
    let p1 = supdb::blob::open_sparse_ranges(&head, bytes.len() as u64).unwrap();
    for &(o, l) in &p1 {
        assert!(
            o + l <= probe,
            "plan {o}+{l} reaches past the {probe}-byte probe"
        );
    }
    let src = Ensured {
        data: bytes.clone(),
        allowed: RefCell::new(vec![(0, probe)]),
    };
    let sparse = SparseBlob::open(src).expect("open from the probe alone");
    assert!(sparse.opened_from_extension());
    assert_eq!(sparse.keys(), data.len());
    assert!(sparse.has_fence());

    // And the dictionary agrees with the whole reader once its plans are
    // ensured, through the same source.
    let lo = data[300].0.clone();
    let hi = data[340].0.clone();
    let d = sparse.dictionary_plan(&lo, Some(&hi));
    sparse
        .source()
        .allowed
        .borrow_mut()
        .extend(d.iter().copied());
    let r = sparse
        .dictionary_plan_records(&lo, Some(&hi))
        .expect("records plan");
    sparse
        .source()
        .allowed
        .borrow_mut()
        .extend(r.iter().copied());
    let mut got = Vec::new();
    sparse
        .dictionary_counts(&lo, Some(&hi), |k, n| {
            got.push((k.to_vec(), n));
            true
        })
        .expect("walk");
    let mut want = Vec::new();
    whole
        .scan_counts(&lo, 40, |k, n| {
            want.push((k.to_vec(), n));
            true
        })
        .expect("scan counts");
    assert_eq!(got, want);

    // Without a reserve the extension still plans the open in one dependent
    // read after the probe: the second plan is empty.
    let path2 = dir.join("seg2.sup");
    write_bulk(&path2, &data, opts());
    let bytes2 = std::fs::read(&path2).unwrap();
    let head2 = bytes2[..4096].to_vec();
    let p1 = supdb::blob::open_sparse_ranges(&head2, bytes2.len() as u64).unwrap();
    let hdr_off = {
        let b = open(&path2);
        b.index_offset() as usize
    };
    let p2 = supdb::blob::open_sparse_fence_ranges(
        &head2,
        bytes2.len() as u64,
        &bytes2[hdr_off..hdr_off + 192],
    )
    .unwrap();
    assert!(
        p2.is_empty(),
        "with an extension the second plan is empty: {p2:?}"
    );
    let src = Ensured {
        data: bytes2,
        allowed: RefCell::new(p1),
    };
    let sparse = SparseBlob::open(src).expect("open over the first plan");
    assert!(sparse.opened_from_extension());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Values with structure, so compression has something to find: four-byte
/// posting *deltas*, which is what logshed stores and what LZ4 halves --
/// small numbers, so three bytes in four are zero and the matches are long.
/// Absolute ordinals do not compress, which the first version of this test
/// discovered by shrinking nothing.
fn postings(keys: usize, seed: u64) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let mut r = seed;
    let mut out: Vec<(Vec<u8>, Vec<Vec<u8>>)> = (0..keys)
        .map(|i| {
            let n = match splitmix(&mut r) % 20 {
                0 => 4_000,
                1..=3 => 1,
                _ => 1 + (splitmix(&mut r) % 60) as usize,
            };
            let vals = (0..n)
                .map(|_| (1 + (splitmix(&mut r) % 250) as u32).to_le_bytes().to_vec())
                .collect();
            (format!("term={i:08}").into_bytes(), vals)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A compressed segment answers exactly what an uncompressed one does, on
/// every key, and its inline runs are untouched because they live in the key
/// section rather than in a block (segcompress-plan.md, P4.2).
#[test]
fn a_compressed_segment_agrees_with_an_uncompressed_one() {
    let _g = serial();
    let dir = scratch("segwriter-compress");
    let data = postings(900, 0xC0FFEE);
    // 64 KiB blocks, so a block passes `block::CHUNK` and takes the chunked
    // path rather than being compressed whole.
    let o = Options {
        block_size: 64 << 10,
        checksums: true,
        ..Options::default()
    };
    let plain = dir.join("plain.sup");
    let packed = dir.join("packed.sup");
    for (path, compress) in [(&plain, false), (&packed, true)] {
        let mut w = SegmentWriter::create(path, &o).expect("create");
        w.set_inline_max(INLINE);
        w.set_compress(compress);
        for (k, vals) in &data {
            w.begin(k).expect("begin");
            for v in vals {
                w.value(v);
            }
            w.end().expect("end");
        }
        w.finish(1).expect("finish");
    }
    let a = open(&plain);
    let b = open(&packed);
    agree(&a, &b, &data);

    // The compression is real, and it did not come out of the key section:
    // the index is the same size and the file is smaller.
    let (sa, sb) = (
        std::fs::metadata(&plain).unwrap().len(),
        std::fs::metadata(&packed).unwrap().len(),
    );
    assert_eq!(
        a.index_bytes(),
        b.index_bytes(),
        "the key section is untouched"
    );
    assert!(sb < sa, "compressed {sb} against plain {sa}");

    // An uncompressed segment's blocks now carry per-chunk checksums, so a
    // run read plans the chunks it spans rather than the block; a compressed
    // one carries them inside its own chunk directory instead.
    let key = data
        .iter()
        .find(|(k, _)| {
            a.lookup(k)
                .is_some_and(|e| e.iter().any(|x| !x.is_inline()))
        })
        .expect("a key whose run went to a block")
        .0
        .clone();
    let plan = a.ranges_for(&key).expect("plan");
    let whole: u64 = a
        .lookup(&key)
        .unwrap()
        .iter()
        .filter(|e| !e.is_inline())
        .map(|_| o.block_size as u64)
        .sum();
    let planned: u64 = plan.iter().map(|r| r.1).sum();
    assert!(
        planned <= whole,
        "chunk plan {planned} against block plan {whole}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
