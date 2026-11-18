//! Codecs beside LZ4, priced on the form's own groups.
//!
//! A scan through the compact form decodes the groups it crosses, and out
//! of core that decode is what the form pays where LMDB, which compresses
//! nothing, pays its page faults instead. This builds a store under the
//! forced form (`supdb-nocachezpages`) in the suite's shape -- `size` keys
//! in key order, values of a hundred bytes at the suite's compressibility,
//! `Batch` flushes of a thousand and the `sync` the suite's load ends in --
//! decodes every compressed group of its form files back to the records
//! the group holds, and compresses each group's records again with each
//! codec, group by group, as the form would store them:
//!
//! - `lz4`: the form's own payload, decoded by the engine's own decoder
//!   (`supdb::db::zlz4`), the shape it ships;
//! - `lz4flex`: the shipped encoder (`lz4_flex`'s greedy block encoder)
//!   run again over the records, for its encode time; its bytes are
//!   checked to be the form's own;
//! - `ownlazy`: the engine's own encoder at the effort it is built with
//!   (`supdb::db::zlz4::Encoder`, `Options::zpages_parse`), the same block
//!   format, decoded by the engine's decoder;
//! - `lz4hc3` .. `lz4hc12`: liblz4's high-compression mode at those levels,
//!   and `lz4hc12f` at level 12 with its parser told to favour the decode's
//!   speed (`LZ4_favorDecompressionSpeed`), the same format and decoder;
//! - `snappy`: the raw Snappy block format;
//! - `lzav`, `lzavhi`: LZAV 7.1 (`vendor/lzav`, upstream's header), its
//!   default and its higher-ratio compressor, one raw block a group;
//! - `zstd-3`, `zstd1`, `zstd3`: zstd at levels -3, 1 and 3, one frame a
//!   group;
//! - `zstd1d`, `zstd3d`: the same with a dictionary trained on a sample of
//!   the groups, the shape a store would keep one dictionary a form file in.
//!
//! Every codec's decode of every group is checked against the records byte
//! for byte before anything is timed. Each is then timed decoding every
//! group in turn (`cycle`), the input streaming through the cache as a
//! memo miss's does, one pass a round, the codecs' order rotated round by
//! round so a drift in the host falls on all of them; the median ns a group
//! over the rounds is printed beside the bytes each stores against the
//! records' own, the encode's ns a group over one pass, and for the LZ4
//! block format the sequences a group, the tokens its decode parses.
//!
//! `cargo run --release --features codecs --example zcodec -- --size 300000`
//! in `bench/`.

use std::hint::black_box;
use std::path::Path;
use std::time::Instant;
use supdb::db::zpages::{lz4_decoder, parse_page, Lz4Decoder, Trailer, ZPAGE, ZTRAILER};
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, Payload, Rng};

/// LZAV 7.1's entry points, compiled from `vendor/lzav` by `build.rs`.
mod lzav {
    use std::os::raw::{c_int, c_void};
    extern "C" {
        fn supdb_lzav_bound(n: c_int) -> c_int;
        fn supdb_lzav_bound_hi(n: c_int) -> c_int;
        fn supdb_lzav_compress(src: *const c_void, dst: *mut c_void, n: c_int, cap: c_int)
            -> c_int;
        fn supdb_lzav_compress_hi(
            src: *const c_void,
            dst: *mut c_void,
            n: c_int,
            cap: c_int,
        ) -> c_int;
        fn supdb_lzav_decompress(
            src: *const c_void,
            dst: *mut c_void,
            n: c_int,
            len: c_int,
        ) -> c_int;
    }

    /// `src` as one LZAV block, by the higher-ratio compressor where `hi`.
    pub fn compress(src: &[u8], hi: bool) -> Vec<u8> {
        let n = src.len() as c_int;
        // SAFETY: `dst` holds the bound LZAV names for `n` bytes, and both
        // buffers are the lengths passed.
        unsafe {
            let cap = if hi {
                supdb_lzav_bound_hi(n)
            } else {
                supdb_lzav_bound(n)
            };
            let mut dst = vec![0u8; cap as usize];
            let p = dst.as_mut_ptr() as *mut c_void;
            let w = if hi {
                supdb_lzav_compress_hi(src.as_ptr() as *const c_void, p, n, cap)
            } else {
                supdb_lzav_compress(src.as_ptr() as *const c_void, p, n, cap)
            };
            assert!(w > 0, "LZAV compresses");
            dst.truncate(w as usize);
            dst
        }
    }

    /// The block `src` into `out`, which it must fill exactly; its length.
    pub fn decompress(src: &[u8], out: &mut [u8]) -> Option<usize> {
        // SAFETY: LZAV reads `src.len()` bytes and writes at most
        // `out.len()`, the lengths passed.
        let r = unsafe {
            supdb_lzav_decompress(
                src.as_ptr() as *const c_void,
                out.as_mut_ptr() as *mut c_void,
                src.len() as c_int,
                out.len() as c_int,
            )
        };
        (r >= 0).then_some(r as usize)
    }
}

/// liblz4's streaming high-compression state, linked through `lz4-sys`;
/// the parser's speed flag is in its static API, which the crate does not
/// bind.
mod hc {
    use std::os::raw::{c_char, c_int, c_void};
    extern "C" {
        pub fn LZ4_createStreamHC() -> *mut c_void;
        pub fn LZ4_freeStreamHC(s: *mut c_void) -> c_int;
        pub fn LZ4_resetStreamHC_fast(s: *mut c_void, level: c_int);
        pub fn LZ4_favorDecompressionSpeed(s: *mut c_void, favor: c_int);
        pub fn LZ4_compress_HC_continue(
            s: *mut c_void,
            src: *const c_char,
            dst: *mut c_char,
            src_size: c_int,
            dst_cap: c_int,
        ) -> c_int;
    }

    /// Each input as one LZ4 block at `level`, no block a dictionary for
    /// the next.
    pub fn compress_all(inputs: &[&[u8]], level: i32, favor: bool) -> Vec<Vec<u8>> {
        // SAFETY: the stream is made and freed here and used by this thread
        // alone; every call is handed buffers of the lengths it is told.
        unsafe {
            let s = LZ4_createStreamHC();
            assert!(!s.is_null(), "an HC stream");
            let out = inputs
                .iter()
                .map(|src| {
                    LZ4_resetStreamHC_fast(s, level);
                    LZ4_favorDecompressionSpeed(s, favor as c_int);
                    let mut dst = vec![0u8; supdb::db::zlz4::max_output(src.len())];
                    let n = LZ4_compress_HC_continue(
                        s,
                        src.as_ptr() as *const c_char,
                        dst.as_mut_ptr() as *mut c_char,
                        src.len() as c_int,
                        dst.len() as c_int,
                    );
                    assert!(n > 0, "liblz4's HC compressor");
                    dst.truncate(n as usize);
                    dst
                })
                .collect();
            LZ4_freeStreamHC(s);
            out
        }
    }
}

/// One group's records, as the engine decodes them, and the form's own
/// payload for it with the rest of its page after it, which the engine's
/// decoder may read into.
struct Group {
    raw: Vec<u8>,
    padded: Vec<u8>,
    len: usize,
}

/// Every compressed group of every page of every form file under `dir`.
fn groups(dir: &Path) -> Vec<Group> {
    let mut files: Vec<_> = walk(dir)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("zpg-") && n.ends_with(".zpg"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no form file under {}", dir.display());
    let mut out = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).expect("a form file reads");
        let t = Trailer::decode(&bytes[bytes.len() - ZTRAILER..]).expect("a trailer");
        let bind = t.bind32();
        for i in 0..t.n_pages as usize {
            let page = &bytes[i * ZPAGE..(i + 1) * ZPAGE];
            let view = parse_page(page, bind).expect("a page we built");
            for j in 0..view.groups() {
                let g = view.group(j).expect("a group of the page");
                if g.stored {
                    continue;
                }
                let (rest, len) = view.payload_and_rest(j).expect("its payload");
                let mut raw = vec![0u8; g.ulen as usize];
                let n = supdb::db::zlz4::decompress_into(&rest[..len], &mut raw)
                    .expect("the engine's decoder");
                assert_eq!(n, raw.len());
                out.push(Group {
                    raw,
                    padded: rest.to_vec(),
                    len,
                });
            }
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).expect("the store's directory reads") {
        let p = e.expect("an entry").path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// A codec's stored bytes for every group, and how it decodes one.
struct Coded {
    name: &'static str,
    /// Each group's stored bytes, with sixty-four bytes of slack after them
    /// for a decoder that reads ahead.
    payloads: Vec<Vec<u8>>,
    lens: Vec<usize>,
    kind: Kind,
    /// The encode's ns a group, over one pass; `None` for the form's own.
    enc_ns: Option<f64>,
}

enum Kind {
    Lz4,
    Snappy,
    Lzav,
    Zstd(Option<Vec<u8>>),
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let mut size: u64 = 300_000;
    let mut rounds: usize = 15;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |what: &str| args.next().unwrap_or_else(|| panic!("{a} needs {what}"));
        match a.as_str() {
            "--size" => size = val("a count").parse().expect("a count"),
            "--rounds" => rounds = val("a count").parse().expect("a count"),
            other => panic!("unknown argument {other}"),
        }
    }
    if lz4_decoder() != Lz4Decoder::Unsafe {
        eprintln!("the engine's decoder in this build is not the one it ships");
        std::process::exit(3);
    }
    let root = std::env::temp_dir().join(format!("supdb-zcodec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    {
        let payload = Payload::new(100, 0.5, 0xE1);
        let mut e = engines::open("supdb-nocachezpages", &root, 1).unwrap();
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        let mut kb = [0u8; 16];
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        e.sync().unwrap();
    }
    let gs = groups(&root);
    let _ = std::fs::remove_dir_all(&root);
    let raw_bytes: usize = gs.iter().map(|g| g.raw.len()).sum();
    let max = gs.iter().map(|g| g.raw.len()).max().unwrap();
    // Through the crate, so its liblz4 is linked for the HC calls as well.
    println!("liblz4 {}", lz4::version());
    println!(
        "{size} keys: {} compressed groups, {:.1} MB of records, {:.0} B a group",
        gs.len(),
        raw_bytes as f64 / 1e6,
        raw_bytes as f64 / gs.len() as f64
    );
    let slack = |mut v: Vec<u8>| {
        v.extend([0u8; 64]);
        v
    };
    // A dictionary a codec trains on every sixteenth group: the store would
    // keep one a form file, trained at its build.
    let sample: Vec<&[u8]> = gs.iter().step_by(16).map(|g| g.raw.as_slice()).collect();
    let dict = zstd::dict::from_samples(&sample, 16 << 10).expect("a dictionary trains");
    let raws: Vec<&[u8]> = gs.iter().map(|g| g.raw.as_slice()).collect();
    let n_groups = gs.len() as f64;
    // A codec's payloads, timed over one pass of its encoder.
    let timed = |f: &mut dyn FnMut() -> Vec<Vec<u8>>| -> (Vec<Vec<u8>>, f64) {
        let t = Instant::now();
        let p = f();
        (p, t.elapsed().as_nanos() as f64 / n_groups)
    };
    let made = |name: &'static str, (p, ns): (Vec<Vec<u8>>, f64), kind: Kind| Coded {
        name,
        lens: p.iter().map(|v| v.len()).collect(),
        payloads: p.into_iter().map(slack).collect(),
        kind,
        enc_ns: Some(ns),
    };
    let mut coded: Vec<Coded> = vec![Coded {
        name: "lz4",
        payloads: gs.iter().map(|g| g.padded.clone()).collect(),
        lens: gs.iter().map(|g| g.len).collect(),
        kind: Kind::Lz4,
        enc_ns: None,
    }];
    {
        let p = timed(&mut || {
            raws.iter()
                .map(|r| {
                    let mut out = vec![0u8; lz4_flex::block::get_maximum_output_size(r.len())];
                    let n = lz4_flex::block::compress_into(r, &mut out).expect("lz4_flex");
                    out.truncate(n);
                    out
                })
                .collect()
        });
        let same = p.0.iter().zip(&gs).all(|(v, g)| v[..] == g.padded[..g.len]);
        println!("lz4flex's bytes are the form's own: {same}");
        coded.push(made("lz4flex", p, Kind::Lz4));
    }
    {
        let mut enc = supdb::db::zlz4::Encoder::new();
        let e = supdb::db::zlz4::Effort::SHIPPED;
        let p = timed(&mut || {
            raws.iter()
                .map(|r| {
                    let mut out = vec![0u8; supdb::db::zlz4::max_output(r.len())];
                    let n = enc
                        .compress_into(r, &mut out, e)
                        .expect("the engine's encoder");
                    out.truncate(n);
                    out
                })
                .collect()
        });
        coded.push(made("ownlazy", p, Kind::Lz4));
    }
    for (name, level, favor) in [
        ("lz4hc3", 3, false),
        ("lz4hc6", 6, false),
        ("lz4hc9", 9, false),
        ("lz4hc12", 12, false),
        ("lz4hc12f", 12, true),
    ] {
        let p = timed(&mut || hc::compress_all(&raws, level, favor));
        coded.push(made(name, p, Kind::Lz4));
    }
    {
        let mut enc = snap::raw::Encoder::new();
        let p = timed(&mut || {
            raws.iter()
                .map(|r| enc.compress_vec(r).expect("snappy compresses"))
                .collect()
        });
        coded.push(made("snappy", p, Kind::Snappy));
    }
    for (name, hi) in [("lzav", false), ("lzavhi", true)] {
        let p = timed(&mut || raws.iter().map(|r| lzav::compress(r, hi)).collect());
        coded.push(made(name, p, Kind::Lzav));
    }
    for (name, level, d) in [
        ("zstd-3", -3, None),
        ("zstd1", 1, None),
        ("zstd3", 3, None),
        ("zstd1d", 1, Some(dict.as_slice())),
        ("zstd3d", 3, Some(dict.as_slice())),
    ] {
        let mut c = match d {
            Some(d) => zstd::bulk::Compressor::with_dictionary(level, d).unwrap(),
            None => zstd::bulk::Compressor::new(level).unwrap(),
        };
        let p = timed(&mut || {
            raws.iter()
                .map(|r| c.compress(r).expect("zstd compresses"))
                .collect()
        });
        coded.push(made(name, p, Kind::Zstd(d.map(|d| d.to_vec()))));
    }
    // Each codec's decoder state, made once as a reader's would be.
    let mut out = vec![0u8; max + 64];
    let mut snap_dec = snap::raw::Decoder::new();
    let mut zdecs: Vec<Option<zstd::bulk::Decompressor>> = coded
        .iter()
        .map(|c| match &c.kind {
            Kind::Zstd(Some(d)) => Some(zstd::bulk::Decompressor::with_dictionary(d).unwrap()),
            Kind::Zstd(None) => Some(zstd::bulk::Decompressor::new().unwrap()),
            _ => None,
        })
        .collect();
    let mut decode = |ci: usize, c: &Coded, i: usize, out: &mut [u8]| -> usize {
        let src = &c.payloads[i][..c.lens[i]];
        let n = gs[i].raw.len();
        match &c.kind {
            Kind::Lz4 => supdb::db::zlz4::decompress_into(src, &mut out[..n]).expect("lz4"),
            Kind::Snappy => snap_dec.decompress(src, &mut out[..n]).expect("snappy"),
            Kind::Lzav => lzav::decompress(src, &mut out[..n]).expect("lzav"),
            Kind::Zstd(_) => zdecs[ci]
                .as_mut()
                .unwrap()
                .decompress_to_buffer(src, &mut out[..n])
                .expect("zstd"),
        }
    };
    // Checked whole before anything is timed.
    for (ci, c) in coded.iter().enumerate() {
        for (i, g) in gs.iter().enumerate() {
            let n = decode(ci, c, i, &mut out);
            assert_eq!(&out[..n], g.raw.as_slice(), "{} group {i}", c.name);
        }
    }
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); coded.len()];
    for r in 0..rounds {
        for k in 0..coded.len() {
            let ci = (k + r) % coded.len();
            let c = &coded[ci];
            let t = Instant::now();
            let mut sum = 0usize;
            for i in 0..gs.len() {
                sum += decode(ci, c, i, &mut out);
            }
            black_box(sum);
            times[ci].push(t.elapsed().as_nanos() as f64 / gs.len() as f64);
        }
    }
    println!(
        "{:<9} {:>10} {:>8} {:>12} {:>8} {:>10} {:>9}",
        "codec", "stored_MB", "ratio", "ns_a_group", "vs_lz4", "enc_ns", "seqs"
    );
    let base = median(&mut times[0].clone());
    for (ci, c) in coded.iter().enumerate() {
        let stored: usize = c.lens.iter().sum();
        let ns = median(&mut times[ci]);
        let enc = c.enc_ns.map_or("-".to_string(), |e| format!("{e:.0}"));
        let seqs = match c.kind {
            Kind::Lz4 => {
                let total: usize = (0..gs.len())
                    .map(|i| {
                        supdb::db::zlz4::sequences(&c.payloads[i][..c.lens[i]])
                            .expect("an LZ4 block")
                    })
                    .sum();
                format!("{:.1}", total as f64 / n_groups)
            }
            _ => "-".to_string(),
        };
        println!(
            "{:<9} {:>10.2} {:>8.3} {:>12.0} {:>7.2}x {:>10} {:>9}",
            c.name,
            stored as f64 / 1e6,
            stored as f64 / raw_bytes as f64,
            ns,
            ns / base,
            enc,
            seqs
        );
    }
}
