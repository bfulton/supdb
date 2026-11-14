//! An encrypted segment against the plain one, under a real cipher.
//!
//! `src/crypt.rs` holds the envelope's own arithmetic to its unit tests,
//! under a stand-in cipher. What is held here is the rest: that the readers
//! answer through an envelope exactly as they answer without one, on a
//! source that lends and on one that cannot; that AES-GCM fits the hook as
//! the hook is drawn; and that the envelope `encrypt_with_id` writes is, byte
//! for byte, the one on record in `tests/data/crypt-v1.bin`, which is the
//! same file `web/crypt.test.mjs` requires the browser's reader to read. The
//! layout is written down in Rust and in JavaScript, and that file is what
//! keeps the two from drifting.

use ring::aead::{Aad, LessSafeKey, Nonce, Tag, UnboundKey, AES_256_GCM};
use std::path::{Path, PathBuf};
use supdb::crypt::{self, Aead, EncryptOptions, Encrypted, Header, ReadOptions, NONCE, TAG};
use supdb::{Blob, Bytes, MmapBytes, SegmentOptions, SegmentWriter, SparseBlob, VecBytes};

mod common;
use common::{kv, vf};

/// AES-256-GCM, `ring`'s, as the hook takes it: the whole of what a caller
/// writes.
struct Gcm(LessSafeKey);

impl Gcm {
    fn new(key: &[u8; 32]) -> Gcm {
        Gcm(LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key).expect("a 32-byte key"),
        ))
    }
}

impl Aead for Gcm {
    fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG] {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        let tag = self
            .0
            .seal_in_place_separate_tag(nonce, Aad::from(aad), buf)
            .expect("a chunk is far under the cipher's limit");
        tag.as_ref().try_into().expect("a sixteen-byte tag")
    }
    fn decrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8], tag: &[u8; TAG]) -> bool {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        self.0
            .open_in_place_separate_tag(nonce, Aad::from(aad), Tag::from(*tag), buf, 0..)
            .is_ok()
    }
}

const KEY: [u8; 32] = [7; 32];

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-crypttest-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

type Written = Vec<(Vec<u8>, Vec<Vec<u8>>)>;

/// A segment whose runs straddle the inline bound: values in the index
/// record, in one block, and across several.
fn build(path: &Path, keys: usize, compress: bool) -> Written {
    let mut w = SegmentWriter::create(path, &SegmentOptions::default()).expect("create");
    w.set_compress(compress);
    let mut want = Vec::new();
    for k in 0..keys {
        let key = format!("term={k:08}").into_bytes();
        let n = [1, 2, 17, 200, 1500, 3, 40][k % 7];
        w.begin(&key).expect("begin");
        let vals: Vec<Vec<u8>> = (0..n).map(|i| format!("{k}:{i}").into_bytes()).collect();
        for v in &vals {
            w.value(v);
        }
        w.end().expect("end");
        want.push((key, vals));
    }
    w.finish(1).expect("finish");
    want
}

/// The envelope of the file at `plain`, written beside it.
fn encrypt_file(plain: &Path, key: &[u8; 32], opts: &EncryptOptions) -> (PathBuf, Header) {
    let out = plain.with_extension("enc");
    let mut f = std::fs::File::create(&out).unwrap();
    let src = MmapBytes::open(plain).unwrap();
    let hdr = crypt::encrypt(&src, &mut f, &Gcm::new(key), opts).unwrap();
    (out, hdr)
}

/// A source with no memory to lend: the shape of a file handle.
struct Copying(Vec<u8>);

impl Bytes for Copying {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, dst: &mut [u8]) -> std::io::Result<()> {
        supdb::SliceBytes(&self.0).read_at(off, dst)
    }
}

/// What a whole walk of a reader answers: every key with each value, and
/// every key with its count.
type Walked = (Vec<(Vec<u8>, Vec<u8>)>, Vec<(Vec<u8>, u64)>);

/// Every key's values and count, and the whole scan, through `blob`.
fn answers<B: Bytes>(blob: &Blob<B>, want: &Written) -> Walked {
    assert_eq!(blob.keys(), want.len());
    for (key, vals) in want {
        let mut got: Vec<Vec<u8>> = Vec::new();
        let n = blob
            .read_all(key, vf(&mut |v| got.push(v.to_vec())))
            .expect("read_all");
        assert_eq!(&got, vals, "values of {}", String::from_utf8_lossy(key));
        assert_eq!(n, vals.len() as u64);
        assert_eq!(blob.count(key).expect("count"), vals.len() as u64);
    }
    assert_eq!(blob.count(b"term=nosuch").unwrap(), 0);
    let mut scanned = Vec::new();
    blob.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| scanned.push((k.to_vec(), v.to_vec()))),
    )
    .expect("scan");
    let mut counts = Vec::new();
    blob.scan_counts(b"", usize::MAX, |k, n| {
        counts.push((k.to_vec(), n));
        true
    })
    .expect("scan_counts");
    (scanned, counts)
}

#[test]
fn an_encrypted_segment_answers_as_the_plain_one_does() {
    for compress in [false, true] {
        let dir = scratch(if compress { "agree-lz4" } else { "agree" });
        let path = dir.join("segment.supdb");
        let want = build(&path, 400, compress);
        let plain = Blob::open(MmapBytes::open(&path).unwrap()).expect("plain open");
        assert!(
            plain.zero_copy(),
            "the plain native path still copies nothing"
        );
        let truth = answers(&plain, &want);
        assert_eq!(truth.1.len(), want.len());

        // The default chunk, one smaller than a block's own chunks and one
        // larger than a block.
        for shift in [crypt::DEFAULT_CHUNK_SHIFT, 9, 17] {
            let opts = EncryptOptions {
                chunk_shift: shift,
                key_id: 3,
            };
            let (enc, hdr) = encrypt_file(&path, &KEY, &opts);
            let stored = std::fs::read(&enc).unwrap();
            assert_eq!(stored.len() as u64, hdr.stored_len());
            assert_eq!(hdr.plain_len(), std::fs::metadata(&path).unwrap().len());

            // A reader that keeps what it decrypts lends it, and the index
            // is borrowed out of it as it is out of a mapping.
            let kept = Encrypted::open(MmapBytes::open(&enc).unwrap(), Gcm::new(&KEY)).unwrap();
            let blob = Blob::open(kept).expect("open through a mapping");
            assert!(blob.zero_copy(), "a kept envelope lends its index");
            assert_eq!(answers(&blob, &want), truth, "kept, shift {shift}");
            assert_eq!(
                answers(&blob, &want),
                truth,
                "kept and read again, shift {shift}"
            );
            assert_eq!(blob.version(), plain.version());
            assert_eq!(blob.index_checksummed(), plain.index_checksummed());

            // One that keeps nothing lends nothing, through a mapping and
            // through a source that cannot lend either.
            let nothing = ReadOptions {
                keep_bytes: 0,
                ..ReadOptions::default()
            };
            let stored_map = MmapBytes::open(&enc).unwrap();
            let bounded = Encrypted::open_with(stored_map, Gcm::new(&KEY), &nothing).unwrap();
            let blob = Blob::open(bounded).expect("open keeping nothing");
            assert!(!blob.zero_copy(), "nothing kept is nothing to lend");
            assert_eq!(answers(&blob, &want), truth, "mapped, shift {shift}");
            let copying = Encrypted::open_with(Copying(stored.clone()), Gcm::new(&KEY), &nothing);
            let blob = Blob::open(copying.unwrap()).expect("open through a copying source");
            assert_eq!(answers(&blob, &want), truth, "copied, shift {shift}");

            // And one that keeps a quarter of the object reads the rest.
            let some = ReadOptions {
                keep_bytes: hdr.plain_len() / 4,
                ..ReadOptions::default()
            };
            let part = Encrypted::open_with(Copying(stored), Gcm::new(&KEY), &some).unwrap();
            let blob = Blob::open(part).expect("open keeping a part");
            assert_eq!(
                answers(&blob, &want),
                truth,
                "a quarter kept, shift {shift}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_sparse_reader_walks_an_encrypted_dictionary() {
    let dir = scratch("sparse");
    let path = dir.join("segment.supdb");
    let want = build(&path, 600, false);
    let (enc, _) = encrypt_file(&path, &KEY, &EncryptOptions::default());
    let sparse =
        SparseBlob::open(Encrypted::open(MmapBytes::open(&enc).unwrap(), Gcm::new(&KEY)).unwrap())
            .expect("sparse open");
    assert_eq!(sparse.keys(), want.len());
    for (lo, hi) in [
        (0usize, None),
        (100, Some(350)),
        (599, None),
        (250, Some(251)),
    ] {
        let lo_key = &want[lo].0;
        let hi_key = hi.map(|h| want[h].0.as_slice());
        let mut got = Vec::new();
        sparse
            .dictionary_counts(lo_key, hi_key, |k, n| {
                got.push((k.to_vec(), n));
                true
            })
            .expect("walk");
        let truth: Vec<(Vec<u8>, u64)> = want[lo..hi.unwrap_or(want.len())]
            .iter()
            .map(|(k, v)| (k.clone(), v.len() as u64))
            .collect();
        assert_eq!(got, truth, "{lo}..{hi:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// One reader, four threads: the remembered chunk is each thread's own.
#[test]
fn threads_share_one_encrypted_reader() {
    let dir = scratch("threads");
    let path = dir.join("segment.supdb");
    let want = build(&path, 300, false);
    let (enc, _) = encrypt_file(&path, &KEY, &EncryptOptions::default());
    let blob = Blob::open(Encrypted::open(MmapBytes::open(&enc).unwrap(), Gcm::new(&KEY)).unwrap())
        .unwrap();
    std::thread::scope(|s| {
        for t in 0..4 {
            let (blob, want) = (&blob, &want);
            s.spawn(move || {
                for (key, vals) in want.iter().skip(t).step_by(3) {
                    let mut got: Vec<Vec<u8>> = Vec::new();
                    blob.read_all(key, vf(&mut |v| got.push(v.to_vec())))
                        .unwrap();
                    assert_eq!(&got, vals);
                }
            });
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// Neither reader takes the other's object for its own: an envelope is not
/// a segment, a segment is not an envelope, and each says so at open.
#[test]
fn an_envelope_and_a_segment_are_not_mistaken_for_each_other() {
    let dir = scratch("mistaken");
    let path = dir.join("segment.supdb");
    build(&path, 50, false);
    let (enc, _) = encrypt_file(&path, &KEY, &EncryptOptions::default());
    assert!(Blob::open(MmapBytes::open(&enc).unwrap()).is_err());
    assert!(Encrypted::open(MmapBytes::open(&path).unwrap(), Gcm::new(&KEY)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_wrong_key_opens_nothing_and_the_header_reads_without_one() {
    let dir = scratch("wrongkey");
    let path = dir.join("segment.supdb");
    build(&path, 50, false);
    let opts = EncryptOptions {
        key_id: 0xfeed,
        ..EncryptOptions::default()
    };
    let (enc, hdr) = encrypt_file(&path, &KEY, &opts);
    let stored = || MmapBytes::open(&enc).unwrap();
    // Which key, asked of the object before any key is tried.
    assert_eq!(Header::read(&stored()).unwrap(), hdr);
    assert_eq!(hdr.key_id(), 0xfeed);
    assert!(
        Encrypted::open(stored(), Gcm::new(&KEY)).is_ok(),
        "the control"
    );
    let mut other = KEY;
    other[31] ^= 1;
    assert!(Encrypted::open(stored(), Gcm::new(&other)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A byte changed in the stored object is an error from the envelope, and
/// through the reader it is an error or the right answer and never another
/// answer.
#[test]
fn a_changed_byte_is_an_error_and_never_another_answer() {
    let dir = scratch("changed");
    let path = dir.join("segment.supdb");
    let want = build(&path, 120, false);
    let (enc, _) = encrypt_file(&path, &KEY, &EncryptOptions::default());
    let stored = std::fs::read(&enc).unwrap();
    let whole = |bytes: Vec<u8>| -> std::io::Result<Vec<u8>> {
        let e = Encrypted::open(VecBytes(bytes), Gcm::new(&KEY))?;
        let mut v = vec![0u8; e.len() as usize];
        e.read_at(0, &mut v)?;
        Ok(v)
    };
    assert_eq!(
        whole(stored.clone()).unwrap(),
        std::fs::read(&path).unwrap(),
        "the control"
    );

    let (mut refused_at_open, mut refused_at_read) = (0, 0);
    for at in (0..stored.len()).step_by(stored.len() / 61) {
        let mut bad = stored.clone();
        bad[at] ^= 0x10;
        assert!(
            whole(bad.clone()).is_err(),
            "byte {at} read as if unchanged"
        );
        let Ok(src) = Encrypted::open(VecBytes(bad), Gcm::new(&KEY)) else {
            refused_at_open += 1;
            continue;
        };
        let Ok(blob) = Blob::open(src) else {
            refused_at_open += 1;
            continue;
        };
        for (key, vals) in &want {
            let mut got: Vec<Vec<u8>> = Vec::new();
            match blob.read_all(key, vf(&mut |v| got.push(v.to_vec()))) {
                Ok(_) => assert_eq!(&got, vals, "byte {at} changed an answer"),
                Err(_) => refused_at_read += 1,
            }
        }
    }
    assert!(refused_at_open > 0, "no change reached the open");
    assert!(refused_at_read > 0, "no change reached a read");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------- the envelope on record --

/// What `tests/data/crypt-v1.bin` holds, as `web/crypt.test.mjs` also has
/// it: 1200 bytes under AES-256-GCM, in stored chunks of 512.
fn on_record() -> (Vec<u8>, [u8; 32], EncryptOptions, [u8; 16]) {
    let plain = (0..1200usize).map(|i| (i * 7 + i / 251) as u8).collect();
    let key = std::array::from_fn(|i| i as u8);
    let opts = EncryptOptions {
        chunk_shift: 9,
        key_id: 0x0102_0304_0506_0708,
    };
    (plain, key, opts, std::array::from_fn(|i| 0xa0 + i as u8))
}

fn record_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/crypt-v1.bin")
}

fn envelope_now() -> Vec<u8> {
    let (plain, key, opts, id) = on_record();
    let mut out = Vec::new();
    crypt::encrypt_with_id(
        &supdb::SliceBytes(&plain),
        &mut out,
        &Gcm::new(&key),
        &opts,
        id,
    )
    .unwrap();
    out
}

#[test]
fn the_envelope_written_is_the_one_on_record() {
    let recorded = std::fs::read(record_path()).expect("tests/data/crypt-v1.bin");
    assert_eq!(recorded.len(), 4096 + 1200 + 3 * 16);
    assert!(
        envelope_now() == recorded,
        "the envelope's layout moved: a reader of version 1, the browser's included, no longer \
         reads what this writes"
    );
    let (plain, key, ..) = on_record();
    let e = Encrypted::open(VecBytes(recorded), Gcm::new(&key)).unwrap();
    let mut got = vec![0u8; plain.len()];
    e.read_at(0, &mut got).unwrap();
    assert_eq!(got, plain);
    assert_eq!(e.header().key_id(), 0x0102_0304_0506_0708);
}

/// Writes the record. Run by hand, once a version: `cargo test --test crypt
/// -- --ignored`. A layout change is a new version and a new file beside
/// this one, never this file written again.
#[test]
#[ignore = "writes tests/data/crypt-v1.bin"]
fn write_the_envelope_on_record() {
    std::fs::create_dir_all(record_path().parent().unwrap()).unwrap();
    std::fs::write(record_path(), envelope_now()).unwrap();
}
