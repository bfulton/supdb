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
use supdb::{Blob, Options, Reader, Store};

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

fn write_bulk(path: &Path, data: &[(Vec<u8>, Vec<Vec<u8>>)], o: Options) {
    let mut w = SegmentWriter::create(path, &o).expect("create");
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
        b.blocks() > 20,
        "the data should span many blocks, got {}",
        b.blocks()
    );
    agree(&a, &b, &data);
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
    write_bulk(&dir.join("bulk.sup"), &data, opts());
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
    write_bulk(&path, &data, opts());
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
