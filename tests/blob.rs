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
//! identically to one that can. That is the browser's shape, and it is
//! the seam where a difference would be the reader's rather than the format's.
//! `tests/dict.rs` holds `SparseBlob` to this reader over every range.
//!
//! It also pins the property a correctness test would otherwise let rot: that
//! the native path stays zero-copy.

use std::path::{Path, PathBuf};
use supdb::bytes::{Bytes, MmapBytes, VecBytes};
use supdb::SegmentWriter;
use supdb::{Blob, BlobOptions, SegmentOptions};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-blobtest-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d.join("segment.supdb")
}

/// A key-multivalue segment shaped like a term index: a few thousand
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
    build_opts(path, keys, compress, inline_max, false)
}

fn build_opts(
    path: &Path,
    keys: usize,
    compress: bool,
    inline_max: usize,
    compact: bool,
) -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let opts = SegmentOptions {
        compact_records: compact,
        ..SegmentOptions::default()
    };
    let mut w = SegmentWriter::create(path, &opts).expect("create");
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
        // The values, in order.
        let mut got: Vec<Vec<u8>> = Vec::new();
        let n = blob.read_all(key, |v| got.push(v.to_vec())).expect("blob");
        assert_eq!(&got, vals, "values of {}", String::from_utf8_lossy(key));
        assert_eq!(n, vals.len() as u64, "read_all returns the value count");

        // The count, without materialising anything.
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
    assert!(blob.zero_copy(), "the native path must not copy");
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

/// The copies a `Piecewise` has lent, by offset and length.
type Lent = std::collections::HashMap<(u64, usize), Box<[u64]>>;

/// A source that lends by the piece (`Bytes::piecewise`), and lends exactly
/// what it is asked for: each `slice_at` answers with a copy of the range in
/// memory of its own, at the range's alignment in the file, with nothing of
/// the file on either side of it. A reader that asked for less than it then
/// read finds zeroes or the end of a slice, never the bytes that would have
/// been there in a mapping, so a reader that agrees with the mapped one
/// through this source has asked for every byte it read. It counts what it
/// was asked for, since a reader that asked for its sections whole would
/// agree as well. It checks nothing of what it lends unless it is made
/// `checked`, where it answers `authenticated` as a paged mapping does.
struct Piecewise {
    bytes: Vec<u8>,
    piece: u64,
    checks: bool,
    /// A byte no range that holds it is lent past (`Piecewise::refusing`),
    /// as a source refuses a range it cannot read.
    refuse: Option<u64>,
    /// What has been lent, by the range asked for: a range asked for again
    /// is lent from the copy made for it the first time, since every copy
    /// stands for as long as the source does and a walk asks for the same
    /// windows many times over.
    lent: std::sync::Mutex<Lent>,
    asked: std::sync::atomic::AtomicU64,
    /// The longest range asked for in one `slice_at`.
    longest: std::sync::atomic::AtomicU64,
}

impl Piecewise {
    fn new(bytes: Vec<u8>, piece: u64) -> Piecewise {
        Piecewise {
            bytes,
            piece,
            checks: false,
            refuse: None,
            lent: std::sync::Mutex::new(std::collections::HashMap::new()),
            asked: std::sync::atomic::AtomicU64::new(0),
            longest: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// One that says it has checked what it lends (`Bytes::authenticated`).
    fn checked(bytes: Vec<u8>, piece: u64) -> Piecewise {
        Piecewise {
            checks: true,
            ..Piecewise::new(bytes, piece)
        }
    }

    /// One that checks nothing and lends no range that holds byte `at`.
    fn refusing(bytes: Vec<u8>, piece: u64, at: u64) -> Piecewise {
        Piecewise {
            refuse: Some(at),
            ..Piecewise::new(bytes, piece)
        }
    }

    fn asked(&self) -> u64 {
        self.asked.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn longest(&self) -> u64 {
        self.longest.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Bytes for Piecewise {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        Copying(self.bytes.clone()).read_at(off, dst)
    }
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.len() || self.refuse.is_some_and(|r| off <= r && r < end) {
            return None;
        }
        self.asked
            .fetch_add(len as u64, std::sync::atomic::Ordering::Relaxed);
        self.longest
            .fetch_max(len as u64, std::sync::atomic::Ordering::Relaxed);
        let phase = (off % 8) as usize;
        let mut lent = self.lent.lock().unwrap();
        let buf = lent.entry((off, len)).or_insert_with(|| {
            let mut buf = vec![0u64; (phase + len).div_ceil(8) + 1].into_boxed_slice();
            // SAFETY: `phase + len` bytes fit the buffer, and the source
            // range is inside `bytes`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.bytes.as_ptr().add(off as usize),
                    buf.as_mut_ptr().cast::<u8>().add(phase),
                    len,
                );
            }
            buf
        });
        // SAFETY: the buffer is kept in `lent` for this source's life,
        // boxed, so it does not move when the map grows, and it is never
        // written again: the slice lent from it stands as long as `self`.
        Some(unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>().add(phase), len) })
    }
    fn piecewise(&self) -> bool {
        true
    }
    fn authenticated(&self) -> bool {
        self.checks
    }
    fn piece_end(&self, off: u64) -> u64 {
        ((off / self.piece + 1) * self.piece).min(self.len())
    }
}

/// Every answer of one reader held to another's, rank by rank and key by
/// key: the keys, the extents, the seeks, the scans and the counts.
fn agree<A: Bytes, B: Bytes>(a: &Blob<A>, b: &Blob<B>) {
    assert_eq!(a.keys(), b.keys());
    assert_eq!(a.blocks(), b.blocks());
    assert_eq!(a.index_bytes(), b.index_bytes());
    assert_eq!(a.version(), b.version());
    assert_eq!(a.payload_bytes(), b.payload_bytes());
    for rank in 0..a.keys() + 2 {
        assert_eq!(a.key_at(rank), b.key_at(rank), "key at {rank}");
        let (x, y) = (a.exts_at_full(rank), b.exts_at_full(rank));
        assert_eq!(x.is_some(), y.is_some(), "record at {rank}");
        if let (Some((ka, ea, ta)), Some((kb, eb, tb))) = (x, y) {
            assert_eq!(ka, kb);
            assert_eq!(ea.iter().collect::<Vec<_>>(), eb.iter().collect::<Vec<_>>());
            assert_eq!(ta, tb, "inline tail at {rank}");
        }
        let (mut va, mut vb) = (Vec::new(), Vec::new());
        let na = a.values_at(rank, |v| va.push(v.to_vec())).unwrap();
        let nb = b.values_at(rank, |v| vb.push(v.to_vec())).unwrap();
        assert_eq!((na, va), (nb, vb), "values at {rank}");
    }
    let mut probes: Vec<Vec<u8>> = (0..a.keys())
        .step_by(7)
        .filter_map(|r| a.key_at(r).map(<[u8]>::to_vec))
        .collect();
    probes.push(Vec::new());
    probes.push(b"term=".to_vec());
    probes.push(b"term=00000003x".to_vec());
    probes.push(b"zzzz".to_vec());
    for key in &probes {
        assert_eq!(a.seek(key), b.seek(key), "seek");
        assert_eq!(
            a.lookup(key).map(|e| e.iter().copied().collect::<Vec<_>>()),
            b.lookup(key).map(|e| e.iter().copied().collect::<Vec<_>>()),
            "lookup"
        );
        for limit in [0, 1, 5, 64, usize::MAX] {
            let (mut sa, mut sb) = (Vec::new(), Vec::new());
            let na = a
                .scan(key, limit, |k, v| sa.push((k.to_vec(), v.to_vec())))
                .unwrap();
            let nb = b
                .scan(key, limit, |k, v| sb.push((k.to_vec(), v.to_vec())))
                .unwrap();
            assert_eq!((na, sa), (nb, sb), "scan of {limit}");
            let (mut ca, mut cb) = (Vec::new(), Vec::new());
            a.scan_counts(key, limit, |k, n| {
                ca.push((k.to_vec(), n));
                true
            })
            .unwrap();
            b.scan_counts(key, limit, |k, n| {
                cb.push((k.to_vec(), n));
                true
            })
            .unwrap();
            assert_eq!(ca, cb, "counts over {limit}");
            let (mut fa, mut fb) = (Vec::new(), Vec::new());
            a.scan_counts_fixed(key, limit, 4, |k, n| {
                fa.push((k.to_vec(), n));
                true
            })
            .unwrap();
            b.scan_counts_fixed(key, limit, 4, |k, n| {
                fb.push((k.to_vec(), n));
                true
            })
            .unwrap();
            assert_eq!(fa, fb, "fixed counts over {limit}");
        }
    }
}

/// A source that lends by the piece answers as the mapping does, having
/// been asked for every byte it was read by -- and for little: a reader
/// that borrowed its key section whole at each lookup would agree on every
/// answer and ask for the section a lookup. The shapes are the ones a
/// record takes (full and compact, values inline, in plain blocks and in
/// compressed ones), and the pieces are small enough that a record crosses
/// one in most of them and large enough in the last that none does.
#[test]
fn a_source_that_lends_by_the_piece_answers_the_same() {
    for (name, compress, compact) in [
        ("plain", false, false),
        ("compact", false, true),
        ("compressed", true, false),
    ] {
        let path = scratch(&format!("piecewise-{name}"));
        let want = build_opts(&path, 600, compress, 256, compact);
        let raw = std::fs::read(&path).unwrap();
        let mapped = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
        for piece in [24, 64, 4080, 1 << 16] {
            // A source that has checked what it lends, as a paged mapping
            // has: one that has not has its row verified at the open,
            // which asks for the section a piece at a time
            // (`a_source_by_the_piece_that_checks_nothing_has_its_row_verified_at_open`).
            let blob =
                Blob::open(Piecewise::checked(raw.clone(), piece)).expect("open by the piece");
            assert!(blob.zero_copy(), "{name}: a piece lent is not a copy");
            let at_open = blob_source(&blob).asked();
            assert!(
                at_open < 4096,
                "{name}, pieces of {piece}: an open asked for {at_open} bytes of a section of {}",
                blob.index_bytes()
            );
            reads_what_was_written(&blob, &want);
            agree(&mapped, &blob);
        }
        // What a point read asks for: a slot or two, and the record's
        // window, which is the piece the record lies in.
        let blob = Blob::open(Piecewise::new(raw.clone(), 256)).expect("open by the piece");
        let before = blob_source(&blob).asked();
        let reads = 50;
        for (key, _) in want.iter().step_by(want.len() / reads).take(reads) {
            assert!(blob.lookup(key).is_some());
        }
        let per = (blob_source(&blob).asked() - before) / reads as u64;
        assert!(
            per < 1024,
            "{name}: a lookup asked for {per} bytes of a section of {}",
            blob.index_bytes()
        );
    }
}

/// The source a reader was opened over.
fn blob_source<B: Bytes>(blob: &Blob<B>) -> &B {
    blob.source()
}

/// A key section's checksum row is the one check of its records a reader
/// makes, and the open is where a reader that holds the section whole makes
/// it. A source that lends by the piece is not asked for its section whole
/// at the open, which is what its kind is for; where it has checked what it
/// lends itself (`Bytes::authenticated`, a paged mapping's tag over every
/// chunk) the row is skipped, and where it has not the row is verified
/// there all the same, a piece an ask: a flipped byte of the records with
/// the block checksums whole fails that open, as it fails a mapped file's,
/// and opens over a source that says it checked it.
#[test]
fn a_source_by_the_piece_that_checks_nothing_has_its_row_verified_at_open() {
    let path = scratch("piecewise-unchecked-row");
    build_opts(&path, 3000, false, 256, false);
    let clean = std::fs::read(&path).unwrap();
    let mapped = MmapBytes::open(&path).unwrap();
    assert!(!mapped.authenticated(), "a mapped file checks nothing");
    let empty = path.with_file_name("empty");
    std::fs::write(&empty, b"").unwrap();
    assert!(
        !MmapBytes::open(&empty).unwrap().authenticated(),
        "an empty mapped file checks nothing either"
    );
    assert!(!VecBytes(Vec::new()).authenticated());
    assert!(!Piecewise::new(Vec::new(), 64).authenticated());
    assert!(Advised::new(Piecewise::checked(Vec::new(), 64)).authenticated());
    let (key_off, key_len) = {
        let b = Blob::open(mapped).expect("blob open");
        assert!(b.index_checksummed(), "a segment's index carries a row");
        (b.index_offset() as usize, b.index_bytes())
    };
    let piece = 1usize << supdb::flatindex::PIECE_SHIFT;
    assert!(
        key_len > 4 * piece,
        "the fixture's key section spans several pieces: {key_len}"
    );
    // The pieces are cut at the file's offsets, not the section's: a
    // section that began on a piece would read the same either way.
    assert_ne!(
        key_off % piece,
        0,
        "the key section starts off a piece boundary"
    );
    let crc_off = supdb::flatindex::Header::parse(&clean[key_off..key_off + key_len])
        .expect("the section's header")
        .crc_off;
    assert!(crc_off > 0 && crc_off < key_len);
    let off_verify = BlobOptions {
        verify_index: false,
        ..BlobOptions::default()
    };
    let mut tried = 0;
    for i in 1..13 {
        let at = key_off + i * key_len / 13;
        let mut bytes = clean.clone();
        bytes[at] ^= 0x40;
        tried += 1;
        let whole = refusal(Blob::open(VecBytes(bytes.clone())));
        assert!(
            whole.is_some(),
            "a flip at section offset {} opened whole",
            at - key_off
        );
        assert_eq!(
            refusal(Blob::open(Piecewise::new(bytes.clone(), 4080))),
            whole,
            "a flip at section offset {} opened over a source by the piece that checks nothing, \
             or was refused for another piece",
            at - key_off
        );
        assert!(
            Blob::open(Piecewise::checked(bytes.clone(), 4080)).is_ok(),
            "a flip at section offset {} was checked over a source that checked it",
            at - key_off
        );
        // A wrapper says what the source it wraps says.
        assert!(
            Blob::open(Advised::new(Piecewise::checked(bytes.clone(), 4080))).is_ok(),
            "a flip at section offset {} was checked through a wrapper of a source that checked it",
            at - key_off
        );
        // With the row's verification off, nothing checks it.
        assert!(
            Blob::open_with(VecBytes(bytes.clone()), off_verify).is_ok()
                && Blob::open_with(Piecewise::new(bytes, 4080), off_verify).is_ok(),
            "a flip at section offset {} was checked with the row's verification off",
            at - key_off
        );
    }
    assert_eq!(tried, 12);
    // Every piece's compare is reached, the first and the last above all:
    // a byte of each, its last, flipped, the whole reader's refusal naming
    // that piece and the reader by the piece's the same. The first piece is
    // short, the section not starting on a piece, and its last byte lies
    // past the header, so the refusal is the row's; the last piece's last
    // byte is the one before the row.
    let shift = supdb::flatindex::Header::parse(&clean[key_off..key_off + key_len])
        .expect("the section's header")
        .piece_shift;
    let cut: Vec<(usize, usize)> =
        supdb::flatindex::pieces(crc_off, shift, key_off as u64).collect();
    assert!(cut.len() > 4, "the fixture spans several pieces");
    assert!(
        cut[0].1 < piece && cut[0].1 > supdb::flatindex::HEADER_BYTES,
        "the first piece is short and holds bytes past the header: {:?}",
        cut[0]
    );
    assert_eq!(
        cut[cut.len() - 1].1,
        crc_off,
        "the last piece ends at the row"
    );
    for (p, &(_, end)) in cut.iter().enumerate() {
        let mut bytes = clean.clone();
        bytes[key_off + end - 1] ^= 0x40;
        let whole = refusal(Blob::open(VecBytes(bytes.clone())));
        assert!(
            whole
                .as_deref()
                .is_some_and(|m| m.ends_with(&format!("mismatch in piece {p}"))),
            "a flip at the end of piece {p} is refused by the whole reader for it: {whole:?}"
        );
        assert_eq!(
            refusal(Blob::open(Piecewise::new(bytes, 4080))),
            whole,
            "a flip at the end of piece {p} of {} opened over a source by the piece \
             that checks nothing, or was refused for another piece",
            cut.len()
        );
    }
    // A piece or the row the source will not lend fails the open, and is
    // not asked for at an open that verifies nothing.
    // The piece a byte of the section lies in, cut at the file's offsets.
    let first = piece - key_off % piece;
    let piece_of = |x: usize| {
        if x < first {
            0
        } else {
            1 + (x - first) / piece
        }
    };
    for (what, at, names) in [
        (
            "a piece of the records",
            key_off + key_len / 2,
            Some(piece_of(key_len / 2)),
        ),
        ("the row", key_off + crc_off + 1, None),
    ] {
        let said = refusal(Blob::open(Piecewise::refusing(
            clean.clone(),
            4080,
            at as u64,
        )));
        assert!(said.is_some(), "{what} not lent opened");
        if let Some(p) = names {
            assert!(
                said.as_deref()
                    .is_some_and(|m| m.ends_with(&format!("piece {p}"))),
                "{what} not lent: {said:?}, not piece {p}"
            );
        }
        assert!(
            Blob::open_with(
                Piecewise::refusing(clean.clone(), 4080, at as u64),
                off_verify
            )
            .is_ok(),
            "{what}: the refusal is the verification's"
        );
    }
    // The clean file opens over both, and the one that checks nothing is
    // asked for every byte of its section, no more than a piece an ask.
    let unchecked = Blob::open(Piecewise::new(clean.clone(), 4080)).expect("clean open");
    let (asked, longest) = (
        blob_source(&unchecked).asked(),
        blob_source(&unchecked).longest(),
    );
    assert!(
        asked as usize >= key_len - 4 * key_len.div_ceil(piece) && longest as usize <= piece,
        "the row's verification asked for {asked} bytes of a section of {key_len}, \
         {longest} at most in one ask"
    );
    let checked = Blob::open(Piecewise::checked(clean.clone(), 4080)).expect("clean open");
    assert!(blob_source(&checked).asked() < 4096);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// What an open's refusal said, or `None` where it opened.
fn refusal<B: Bytes, E: std::fmt::Display>(open: Result<Blob<B>, E>) -> Option<String> {
    open.err().map(|e| e.to_string())
}

/// The row's word is held whole: a change to a piece whose CRC32C differs
/// from the row's word in its top byte alone is refused over a source by
/// the piece that checks nothing, as over a whole one. A flip of one byte
/// changes every byte of the CRC but by a chance of one in sixteen
/// million, so the flips of the test above cannot tell a compare of three
/// bytes from one of four; this change is forged for it, from the CRC's
/// linearity: a set of bits of one piece whose deltas cancel in the low
/// three bytes.
#[test]
fn a_piece_whose_crc_moves_in_its_top_byte_alone_is_refused_by_the_piece() {
    let path = scratch("piecewise-top-byte");
    build_opts(&path, 3000, false, 256, false);
    let clean = std::fs::read(&path).unwrap();
    let (key_off, key_len) = {
        let b = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
        (b.index_offset() as usize, b.index_bytes())
    };
    let hdr = supdb::flatindex::Header::parse(&clean[key_off..key_off + key_len])
        .expect("the section's header");
    let pieces: Vec<(usize, usize)> =
        supdb::flatindex::pieces(hdr.crc_off, hdr.piece_shift, key_off as u64).collect();
    assert!(pieces.len() > 4, "the fixture spans several pieces");
    let (a, b) = pieces[pieces.len() / 2];
    // The CRC's change for one bit flipped at byte `p` of the piece, `bit`
    // of it: the register's linear part from that byte to the piece's end.
    let len = b - a;
    let delta = |p: usize, bit: u32| {
        let mut c = 1u32 << bit;
        for _ in 0..8 * (len - p) {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0x82F6_3B78
            } else {
                c >> 1
            };
        }
        c
    };
    // Gaussian elimination over the low 24 bits of the last eight bytes'
    // sixty-four bits: the first combination that cancels there and not
    // in the top byte.
    let mut basis: Vec<(u32, u64)> = Vec::new();
    let mut found = None;
    for j in 0..64u32 {
        let (p, bit) = (len - 8 + (j / 8) as usize, j % 8);
        let d = delta(p, bit);
        let (mut low, mut mask) = (d & 0xFF_FFFF, 1u64 << j);
        for &(bl, bm) in &basis {
            if low ^ bl < low {
                low ^= bl;
                mask ^= bm;
            }
        }
        if low == 0 {
            let full = (0..64)
                .filter(|k| mask >> k & 1 != 0)
                .fold(0, |x, k| x ^ delta(len - 8 + (k / 8) as usize, k % 8));
            if full >> 24 != 0 {
                found = Some(mask);
                break;
            }
            continue;
        }
        basis.push((low, mask));
        basis.sort_by_key(|x| std::cmp::Reverse(x.0));
    }
    let mask = found.expect("a combination of bits that cancels in the low three bytes");
    let mut bytes = clean.clone();
    for k in (0..64).filter(|k| mask >> k & 1 != 0) {
        bytes[key_off + a + len - 8 + (k / 8) as usize] ^= 1 << (k % 8);
    }
    let crc = |s: &[u8]| {
        let mut c = !0u32;
        for &x in s {
            c ^= u32::from(x);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0x82F6_3B78
                } else {
                    c >> 1
                };
            }
        }
        !c
    };
    let moved = crc(&clean[key_off + a..key_off + b]) ^ crc(&bytes[key_off + a..key_off + b]);
    assert!(
        moved & 0xFF_FFFF == 0 && moved != 0,
        "the forged change moves the piece's CRC in its top byte alone: {moved:#010x}"
    );
    let whole = refusal(Blob::open(VecBytes(bytes.clone())));
    assert!(
        whole
            .as_deref()
            .is_some_and(|m| m.ends_with(&format!("piece {}", pieces.len() / 2))),
        "the whole reader refuses the forged piece: {whole:?}"
    );
    assert_eq!(
        refusal(Blob::open(Piecewise::new(bytes, 4080))),
        whole,
        "the forged piece opened over a source by the piece that checks nothing"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// An advice a reader gave its source, by its kind and its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Advice {
    Willneed(u64, usize),
    Dontneed(u64, usize),
}

/// A source that answers as the one it wraps and records every advice it
/// is given (`advise_willneed`, `advise_dontneed_range`), so that two
/// readers over the same bytes, one that lends whole and one by the piece,
/// can be held to asking for the same ranges. It says the bytes of one
/// range are resident (`resident_at`), none by default, so that an ask is
/// held to being skipped over resident pages and only there.
struct Advised<B> {
    inner: B,
    said: std::sync::Mutex<Vec<Advice>>,
    resident: std::sync::Mutex<(u64, u64)>,
}

impl<B> Advised<B> {
    fn new(inner: B) -> Advised<B> {
        Advised {
            inner,
            said: std::sync::Mutex::new(Vec::new()),
            resident: std::sync::Mutex::new((0, 0)),
        }
    }

    /// The bytes `a..b` said to be resident from now on.
    fn resident(&self, a: u64, b: u64) {
        *self.resident.lock().unwrap() = (a, b);
    }

    fn take(&self) -> Vec<Advice> {
        std::mem::take(&mut *self.said.lock().unwrap())
    }
}

impl<B: Bytes> Bytes for Advised<B> {
    fn len(&self) -> u64 {
        self.inner.len()
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_at(off, dst)
    }
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        self.inner.slice_at(off, len)
    }
    fn piecewise(&self) -> bool {
        self.inner.piecewise()
    }
    fn authenticated(&self) -> bool {
        self.inner.authenticated()
    }
    fn piece_end(&self, off: u64) -> u64 {
        self.inner.piece_end(off)
    }
    fn advise_willneed(&self, off: u64, len: usize) -> usize {
        self.said.lock().unwrap().push(Advice::Willneed(off, len));
        len
    }
    fn advise_dontneed_range(&self, off: u64, len: usize) {
        self.said.lock().unwrap().push(Advice::Dontneed(off, len));
    }
    fn resident_at(&self, off: u64) -> Option<bool> {
        let (a, b) = *self.resident.lock().unwrap();
        (a <= off && off < b).then_some(true)
    }
}

/// What a reader answers and advises for the ranks of a segment: every
/// record's offset, the record region's place, and the asks and drops of
/// a fixed set of calls, each with the advice it gave.
type AdviceTrace = (
    Vec<Option<u64>>,
    Option<(u64, usize)>,
    Vec<(usize, Vec<Advice>)>,
);

fn advice_trace<B: Bytes>(blob: &Blob<Advised<B>>) -> AdviceTrace {
    let keys = blob.keys();
    let offsets: Vec<Option<u64>> = (0..keys + 2).map(|r| blob.record_offset(r)).collect();
    let place = blob.record_region_place();
    let src = blob_source(blob);
    let _ = src.take();
    let mut calls = Vec::new();
    for (rank, n, cap) in [
        (0, keys, usize::MAX),
        (0, 1, usize::MAX),
        (keys / 2, 10, 64),
        (keys / 3, 40, 1 << 20),
        (keys - 1, 5, 1 << 20),
        (keys, 3, 1 << 20),
        (keys / 2, 4, 0),
    ] {
        let got = blob.willneed_ranks(rank, n, cap);
        calls.push((got, src.take()));
    }
    for (rank, n) in [(0, keys), (0, 0), (keys / 3, 20), (keys - 1, 9), (keys, 1)] {
        let got = blob.willneed_dir(rank, n);
        calls.push((got, src.take()));
    }
    if let (Some(a), Some(b)) = (offsets[1], offsets[keys / 2]) {
        blob.drop_records(a, b);
    }
    calls.push((0, src.take()));
    (offsets, place, calls)
}

/// The advice a reader asks of its source for a span of ranks or of
/// directory words, and the file offset of a record, answer the same for
/// a source that lends by the piece as for one that lends whole, and the
/// same offsets as the file itself says: the record's key at its offset,
/// and the directory word the ask for a rank's word starts at naming that
/// record. A reader by the piece once answered nothing to all of them, so
/// that every ask ahead and every drop behind of a store under a cipher
/// was a call that did nothing.
#[test]
fn the_advice_by_rank_answers_the_same_for_a_source_by_the_piece() {
    for (name, compact) in [("plain", false), ("compact", true)] {
        let path = scratch(&format!("advice-{name}"));
        let want = build_opts(&path, 600, false, 256, compact);
        let raw = std::fs::read(&path).unwrap();
        let keys = want.len();
        let mapped = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
        let whole = Blob::open(Advised::new(VecBytes(raw.clone()))).expect("open whole");
        assert!(!blob_source(&whole).piecewise() && whole.zero_copy());
        let whole_trace = advice_trace(&whole);
        // A reader that copied its section at the open answers as the one
        // that lends it.
        let copied = Blob::open(Advised::new(Copying(raw.clone()))).expect("open copying");
        assert!(!blob_source(&copied).piecewise() && !copied.zero_copy());
        let copied_trace = advice_trace(&copied);
        holds_to_the_file(
            &format!("{name}, copied"),
            &copied,
            &copied_trace,
            &raw,
            &want,
            &mapped,
        );
        assert_eq!(copied_trace, whole_trace, "{name}: the copying reader");
        for piece in [24, 64, 4080] {
            let blob = Blob::open(Advised::new(Piecewise::new(raw.clone(), piece)))
                .expect("open by the piece");
            assert!(blob_source(&blob).piecewise());
            let before = blob_source(&blob).inner.asked();
            let trace = advice_trace(&blob);
            let asked = blob_source(&blob).inner.asked() - before;
            let (offsets, place, calls) = &trace;
            // What the calls read they asked the source for by the piece:
            // a few words of the directory and the header a call, never the
            // key section whole, which a reader that borrowed it whole would
            // ask for at every call.
            let region = place.map_or(0, |(_, l)| l as u64);
            assert!(
                asked < region / 4,
                "{name}, pieces of {piece}: asked for {asked} bytes, the region is {region}"
            );
            for (r, off) in offsets.iter().enumerate() {
                assert_eq!(
                    off.is_some(),
                    r < keys,
                    "{name}, pieces of {piece}: record offset at {r}"
                );
            }
            assert!(place.is_some(), "{name}, pieces of {piece}: region place");
            for (i, (got, said)) in calls.iter().enumerate().take(4) {
                assert!(
                    *got > 0 && !said.is_empty(),
                    "{name}, pieces of {piece}: ask {i} answered {got} with {said:?}"
                );
            }
            holds_to_the_file(
                &format!("{name}, pieces of {piece}"),
                &blob,
                &trace,
                &raw,
                &want,
                &mapped,
            );
            assert_eq!(
                trace, whole_trace,
                "{name}, pieces of {piece}: the piece reader against the whole one"
            );
        }
        holds_to_the_file(name, &whole, &whole_trace, &raw, &want, &mapped);
    }
}

/// A reader's answers held to the file they are about: each record's key
/// at its offset, the offsets a mapped reader answers, the region's place
/// its span, every ask over ranks inside the region and each directory ask
/// at the word that names its rank's record.
fn holds_to_the_file<B: Bytes>(
    name: &str,
    blob: &Blob<Advised<B>>,
    trace: &AdviceTrace,
    raw: &[u8],
    want: &[(Vec<u8>, Vec<Vec<u8>>)],
    mapped: &Blob<MmapBytes>,
) {
    let keys = want.len();
    // The offsets are the file's: each record's key lies at its offset,
    // and they are the ones a mapped reader answers.
    let (offsets, place, calls) = trace;
    for (r, (key, _)) in want.iter().enumerate() {
        let off = offsets[r].expect("a record's offset") as usize;
        let near = &raw[off..(off + key.len() + 16).min(raw.len())];
        assert!(
            near.windows(key.len()).any(|w| w == key.as_slice()),
            "{name}: record {r}'s key is not at its offset {off}"
        );
        assert_eq!(offsets[r], mapped.record_offset(r), "{name}: mapped at {r}");
    }
    let span = mapped.record_region_span().expect("the region's span");
    assert_eq!(
        place.map(|(o, l)| (o, l as u64)),
        Some(span),
        "{name}: the region's place is its span"
    );
    assert_eq!(
        Some(span.0),
        offsets[0],
        "{name}: the region starts at rank 0"
    );
    // Every ask over ranks lies in the record region, and asks for
    // something; past the last rank there is nothing to ask for.
    for (got, said) in &calls[..7] {
        for a in said {
            let Advice::Willneed(o, l) = *a else {
                panic!("{name}: an ask over ranks gave {a:?}");
            };
            assert!(
                o >= span.0 && o + l as u64 <= span.0 + span.1 && l == *got && l > 0,
                "{name}: the ask {a:?} against the region {span:?}"
            );
        }
    }
    assert_eq!(
        calls[5],
        (0, Vec::new()),
        "{name}: an ask past the last rank"
    );
    assert_eq!(calls[6], (0, Vec::new()), "{name}: an ask of no bytes");
    for (got, said) in &calls[7..12] {
        for a in said {
            assert!(
                matches!(*a, Advice::Willneed(_, l) if l > 0 && l == *got),
                "{name}: a directory ask {a:?}"
            );
        }
    }
    // The ask for a rank's directory word starts at the word that names
    // the rank's record, read out of the file.
    let word = |o: u64| u32::from_le_bytes(raw[o as usize..o as usize + 4].try_into().unwrap());
    let dir_at = |rank: usize| match blob.willneed_dir(rank, 1) {
        0 => panic!("{name}: no ask for the directory word of rank {rank}"),
        _ => match blob_source(blob).take().as_slice() {
            [Advice::Willneed(o, 8)] => *o,
            other => panic!("{name}: the directory ask at {rank} was {other:?}"),
        },
    };
    let d0 = dir_at(0);
    for r in [1, keys / 3, keys - 2] {
        let d = dir_at(r);
        assert_eq!(d, d0 + 4 * r as u64, "{name}: directory word of {r}");
        assert_eq!(
            (word(d) - word(d0)) as u64,
            offsets[r].unwrap() - offsets[0].unwrap(),
            "{name}: the word at {d} names record {r}"
        );
    }
    // The asks over ranks start at the first rank's record and the drop
    // runs between the two offsets it was given.
    assert_eq!(
        calls[1].1.first(),
        Some(&Advice::Willneed(offsets[0].unwrap(), calls[1].0)),
        "{name}: the ask for rank 0"
    );
    let (a, b) = (offsets[1].unwrap(), offsets[keys / 2].unwrap());
    assert_eq!(
        calls.last().unwrap().1,
        vec![Advice::Dontneed(a, (b - a) as usize)],
        "{name}: the drop"
    );
    // An ask whose first page is resident is skipped, by the residency of
    // the offset the ask would start at and of no other.
    let src = blob_source(blob);
    let o0 = offsets[0].unwrap();
    src.resident(o0, o0 + 1);
    assert_eq!(
        blob.willneed_ranks(0, 4, usize::MAX),
        0,
        "{name}: rank 0 resident"
    );
    assert!(
        blob.willneed_ranks(keys / 2, 4, usize::MAX) > 0,
        "{name}: rank {}",
        keys / 2
    );
    src.resident(d0, d0 + 1);
    assert_eq!(blob.willneed_dir(0, 1), 0, "{name}: word 0 resident");
    assert!(blob.willneed_dir(1, 1) > 0, "{name}: word 1 not resident");
    assert!(
        blob.willneed_ranks(0, 4, usize::MAX) > 0,
        "{name}: rank 0 again"
    );
    src.resident(0, 0);
    let _ = src.take();
}

/// Compact records, through a source that lends and one that copies: the
/// same answers as each other and as what was written, and the lending one
/// still copies nothing. The fixture's runs straddle the inline bound, so
/// the segment holds compact records, full ones for runs in blocks, and
/// both kinds of fixed and prefixed run.
#[test]
fn compact_records_read_the_same_through_both_sources() {
    let path = scratch("compact-agree");
    let want = build_opts(&path, 700, false, 256, true);
    let full = scratch("compact-agree-full");
    build_opts(&full, 700, false, 256, false);
    let (a, b) = (
        std::fs::metadata(&path).unwrap().len(),
        std::fs::metadata(&full).unwrap().len(),
    );
    assert!(
        a < b,
        "compact records make the file smaller: {a} against {b}"
    );
    let lending = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
    assert!(lending.zero_copy(), "the native path must not copy");
    reads_what_was_written(&lending, &want);
    let copying = Blob::open(Copying(std::fs::read(&path).unwrap())).expect("copying open");
    assert!(!copying.zero_copy());
    reads_what_was_written(&copying, &want);
}

#[test]
fn compressed_blocks_read_the_same_as_plain_ones() {
    // A compressed segment takes the chunked and solo arms of `with_extent`
    // that a plain one never reaches, and its inline runs stay uncompressed
    // in the key section either way.
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

    // From a prefix, in order, with each key's count.
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

/// The part of counting that is a real O(extents) count.
///
/// A posting list of fixed-width values -- four bytes of ordinal -- has a
/// count that falls out of the extent list with
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
    let opts = SegmentOptions {
        block_size: 4096,
        ..SegmentOptions::default()
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
        let mut w = SegmentWriter::create(&path, &SegmentOptions::default()).expect("create");
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
    let mut w = SegmentWriter::create(&path, &SegmentOptions::default()).expect("create");
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
    // is inline plans no fetch at all and could not be damaged this
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
    // The plan is the 4 KiB chunks the run spans, starting at a chunk
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
    let mut w = SegmentWriter::create(&path, &SegmentOptions::default()).expect("create");
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

#[test]
fn a_walk_over_a_compressed_block_decompresses_each_chunk_once() {
    // `dec_buf` is a reused buffer, and for a long time that was all it was:
    // `with_run` decompressed the chunks covering one extent, handed them
    // out, and kept no record of what the buffer held. Extents are laid down
    // in key order and many share a chunk, so a walk decoded the same chunk
    // once per key -- about forty times over where a key holds one hundred
    // byte value and chunks are four kibibytes.
    //
    // Nothing is wrong with the bytes either way, which is why no correctness
    // check ever saw this and why the assertion below counts work instead.
    let path = scratch("dec-reuse");
    // inline_max 0 keeps every run in a block. An inline run lives in the key
    // section and is never compressed, so a fixture that inlines measures
    // nothing at all.
    let want = build_with(&path, 400, true, 0);
    let blob = Blob::open(MmapBytes::open(&path).expect("map")).expect("open");

    for (key, runs) in &want {
        let mut got: Vec<Vec<u8>> = Vec::new();
        blob.read_all(key, |v| got.push(v.to_vec())).expect("read");
        assert_eq!(&got, runs, "value bytes differ for {key:?}");
    }

    // One `with_run` per key, so without reuse this is exactly the key count
    // -- measured at 400 of 400. With it, 71. Half the keys is the line: it
    // cannot be reached by decoding per key, and it is not so tight that a
    // change in the fixture's run lengths trips it.
    let keys = want.len() as u64;
    let decodes = blob.decodes();
    assert!(keys >= 400, "{keys} keys: this proves nothing");
    assert!(
        decodes * 2 < keys,
        "{decodes} decompressions for {keys} keys: the walk is decoding the \
         same chunk once per key again"
    );
}

/// The walk by the piece over the common record -- values of one width,
/// inline -- answers as the mapping's for scans that cross several of its
/// windows of ranks: each window moves the rank by what it walked, and a
/// rank moved by the count walked since the scan began, or since anything
/// but the window's start, hands out the records of the wrong ranks from
/// the second window on. The shapes of `a_source_that_lends_by_the_piece_
/// answers_the_same` are records of mixed lengths, which that walk hands to
/// the general one at the first record, so they never reach a second window.
#[test]
fn a_walk_by_the_piece_across_its_windows_answers_as_the_mapped_one() {
    for compact in [false, true] {
        let path = scratch(&format!("piecewise-windows-{compact}"));
        let opts = SegmentOptions {
            compact_records: compact,
            ..SegmentOptions::default()
        };
        let mut w = SegmentWriter::create(&path, &opts).expect("create");
        w.set_inline_max(256);
        let mut want = Vec::new();
        for k in 0..300usize {
            let key = format!("term={k:08}").into_bytes();
            let n = if k % 5 == 0 { 3 } else { 1 };
            w.begin(&key).expect("begin");
            let vals: Vec<Vec<u8>> = (0..n).map(|i| format!("{k:06}.{i}").into_bytes()).collect();
            for v in &vals {
                w.value(v);
            }
            w.end().expect("end");
            want.push((key, vals));
        }
        w.finish(1).expect("finish");
        let raw = std::fs::read(&path).unwrap();
        let mapped = Blob::open(MmapBytes::open(&path).unwrap()).expect("blob open");
        for piece in [64, 4080, 1 << 16] {
            let blob = Blob::open(Piecewise::new(raw.clone(), piece)).expect("open by the piece");
            reads_what_was_written(&blob, &want);
            agree(&mapped, &blob);
            for from in [0usize, 1, 63, 64, 65, 130] {
                for limit in [65usize, 128, 200, usize::MAX] {
                    let (mut sa, mut sb) = (Vec::new(), Vec::new());
                    let na = mapped
                        .scan_at(from, limit, |k, v| sa.push((k.to_vec(), v.to_vec())))
                        .unwrap();
                    let nb = blob
                        .scan_at(from, limit, |k, v| sb.push((k.to_vec(), v.to_vec())))
                        .unwrap();
                    assert_eq!(
                        (na, sa),
                        (nb, sb),
                        "compact {compact}, pieces of {piece}: a scan of {limit} from rank {from}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_paced_walk_hands_out_what_the_plain_walk_does() {
    // `scan_at_paced` asks for the records ahead of its walk and is
    // otherwise `scan_at`: held to it at every seventh start, over the
    // record shapes the fast loop takes and the ones it leaves to the
    // general loop, at leads short of a record, longer than one, and past
    // the record region's end.
    for compact in [false, true] {
        let path = scratch(if compact { "paced-compact" } else { "paced" });
        build_opts(&path, 300, false, 256, compact);
        let blob = Blob::open(MmapBytes::open(&path).expect("map")).expect("open");
        let keys = blob.keys();
        assert!(keys > 100);
        for from in (0..=keys).step_by(7) {
            for limit in [1usize, 5, 100, usize::MAX] {
                let mut plain = Vec::new();
                let n = blob
                    .scan_at(from, limit, |k, v| plain.push((k.to_vec(), v.to_vec())))
                    .expect("scan");
                for lead in [0usize, 64, 1000, 1 << 20] {
                    let mut paced = Vec::new();
                    let m = blob
                        .scan_at_paced(from, limit, lead, |k, v| {
                            paced.push((k.to_vec(), v.to_vec()))
                        })
                        .expect("paced");
                    assert_eq!(
                        (m, &paced),
                        (n, &plain),
                        "compact={compact} from={from} limit={limit} lead={lead}"
                    );
                }
            }
        }
    }
}

/// The walks over a reader that holds its key section whole --
/// `scan_at_paced` and a `run_walk`'s -- refuse a source that lends by the
/// piece before they walk anything, with `Unsupported`: that refusal is the
/// engine's word that a segment is to be walked by the piece, a window of
/// records an ask, and a walk that took such a source a record at a time
/// would answer the same and ask for every record on its own. `scan_at`
/// walks it.
#[test]
fn the_whole_walks_refuse_a_source_that_lends_by_the_piece() {
    let path = scratch("whole-walks-refuse");
    build_opts(&path, 300, false, 256, false);
    let raw = std::fs::read(&path).unwrap();
    let blob = Blob::open(Piecewise::new(raw, 4080)).expect("open by the piece");
    let keys = blob.keys();
    assert!(keys > 100);
    let mut seen = 0usize;
    let mut count = |_: &[u8], _: &[u8]| seen += 1;
    let cb: &mut dyn FnMut(&[u8], &[u8]) = &mut count;
    for lead in [0usize, 64] {
        let e = blob
            .scan_at_paced(0, keys, lead, &mut *cb)
            .expect_err("a paced walk over a source by the piece is refused");
        assert_eq!(e.kind(), std::io::ErrorKind::Unsupported, "lead={lead}");
    }
    let e = blob
        .run_walk()
        .scan_at(0, keys, &mut *cb)
        .expect_err("a run walk over a source by the piece is refused");
    assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
    let walked = blob.scan_at(0, keys, &mut *cb).expect("scan_at walks it");
    assert_eq!(walked, keys);
    let after = seen;
    let mut seen = 0usize;
    let mut count = |_: &[u8], _: &[u8]| seen += 1;
    let cb: &mut dyn FnMut(&[u8], &[u8]) = &mut count;
    let _ = blob.scan_at_paced(0, keys, 0, &mut *cb);
    let _ = blob.run_walk().scan_at(0, keys, &mut *cb);
    assert!(after > 0, "`scan_at` handed out the records");
    assert_eq!(seen, 0, "a refused walk handed nothing out");
}

/// A source that lends whole until it is told to stop, and then lends
/// nothing and is not one that lends by the piece: a reader over it has
/// its key section neither whole nor by the piece.
struct StopsLending {
    bytes: Vec<u8>,
    lends: std::sync::atomic::AtomicBool,
}

impl Bytes for StopsLending {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        Copying(self.bytes.clone()).read_at(off, dst)
    }
    fn slice_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        if !self.lends.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let end = off.checked_add(len as u64)?;
        self.bytes.get(off as usize..usize::try_from(end).ok()?)
    }
}

/// A seek over a reader whose key section cannot be read, whole or by the
/// piece, answers the end: no key is at or after the query, and a scan
/// from there walks nothing.
#[test]
fn a_seek_over_a_section_that_does_not_read_answers_the_end() {
    let path = scratch("seek-unread");
    let want = build_opts(&path, 300, false, 256, false);
    let src = StopsLending {
        bytes: std::fs::read(&path).unwrap(),
        lends: std::sync::atomic::AtomicBool::new(true),
    };
    let blob = Blob::open(src).expect("open while it lends");
    let keys = blob.keys();
    assert_eq!(blob.seek(&want[0].0), 0, "the first key, while it lends");
    blob.source()
        .lends
        .store(false, std::sync::atomic::Ordering::Relaxed);
    for (key, _) in want.iter().step_by(37) {
        assert_eq!(blob.seek(key), keys, "a seek with nothing to read");
    }
    assert_eq!(blob.seek(b""), keys);
}

#[test]
fn a_scan_hands_out_the_bytes_read_all_does() {
    // Every scan test before this one counted: pairs, or values a key. A
    // fast path that walks the record region and returns the right NUMBER
    // of entries with the wrong bytes passes all of them. This holds the
    // scan's (key, value) stream to `read_all` key by key, over the mixed
    // fixture -- runs that live in the record, runs that spill to a block,
    // runs of one value and of fifteen hundred -- so a walk that takes a
    // short cut for the common shape has to come back correctly for the
    // rest, mid-scan, in both directions.
    for compress in [false, true] {
        let path = scratch(if compress {
            "scan-bytes-lz4"
        } else {
            "scan-bytes"
        });
        let want = build(&path, 400, compress);
        let blob = Blob::open(MmapBytes::open(&path).expect("map")).expect("open");

        let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        blob.scan(b"", usize::MAX, |k, v| got.push((k.to_vec(), v.to_vec())))
            .expect("scan");

        // Group the stream by key, keeping order.
        let mut keys: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
        for (k, v) in got {
            match keys.last_mut() {
                Some((last, vals)) if *last == k => vals.push(v),
                _ => keys.push((k, vec![v])),
            }
        }
        assert_eq!(
            keys.len(),
            want.len(),
            "compress={compress}: the scan visited {} keys of {}",
            keys.len(),
            want.len()
        );
        for ((k, vals), (wk, wvals)) in keys.iter().zip(&want) {
            assert_eq!(k, wk, "compress={compress}: keys out of order or missing");
            assert_eq!(vals, wvals, "compress={compress}: values differ for {k:?}");
            let mut via_read = Vec::new();
            blob.read_all(k, |v| via_read.push(v.to_vec()))
                .expect("read_all");
            assert_eq!(
                vals, &via_read,
                "compress={compress}: scan and read_all disagree for {k:?}"
            );
        }

        // And a bounded scan from the middle: the limit counts keys, the
        // start is exact, and the bytes are still the right ones.
        let from = &want[150].0;
        let mut seen: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let n = blob
            .scan(from, 20, |k, v| seen.push((k.to_vec(), v.to_vec())))
            .expect("scan");
        assert_eq!(n, 20, "compress={compress}: a limit of 20 keys");
        let mut at = 150usize;
        let mut i = 0usize;
        while i < seen.len() {
            let (wk, wvals) = &want[at];
            let run: Vec<Vec<u8>> = seen[i..i + wvals.len()]
                .iter()
                .map(|(_, v)| v.clone())
                .collect();
            assert!(seen[i..i + wvals.len()].iter().all(|(k, _)| k == wk));
            assert_eq!(&run, wvals, "compress={compress}: bytes differ at key {at}");
            i += wvals.len();
            at += 1;
        }
        assert_eq!(
            at, 170,
            "compress={compress}: the bounded scan covered keys 150..170"
        );
    }
}
