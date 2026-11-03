//! The form's LZ4 decoder priced against liblz4's, on the form's own pages.
//!
//! A form read's largest fixed cost is the decode of the page's group
//! (`lz4_flex::block::decompress_into`, which the engine links; the bounded
//! profile put it at a third of a point read's samples and more of a scan's).
//! This builds a store under the forced form (`supdb-nocachezpages`) in the
//! suite's shape -- `size` keys in key order, values of a hundred bytes at
//! the suite's compressibility, `Batch` flushes of a thousand and the `sync`
//! the suite's load ends in -- reads every compressed group of every page of
//! its form files back, decodes each with the engine's decoder and with
//! liblz4's `LZ4_decompress_safe` (the `lz4` crate), requires the two to
//! agree byte for byte with the group's `ulen`, and times each over the
//! groups:
//!
//! - `cycle`: every group in turn, the decoder's input streaming through the
//!   cache as a memo miss's does, one time a round, the decoders alternating
//!   their order round by round so a drift in the host falls on both;
//! - `hot`: one group decoded over and over, the input in L1, the decoder's
//!   own instruction cost with nothing of the bytes' placement in it.
//!
//! Prints each decoder's median ns a group and ns a decoded byte over the
//! rounds, and the C decoder's share of the engine's. For an instruction
//! count, `--count flex` or `--count c` decodes every group once with that
//! decoder and exits, for `valgrind --tool=callgrind` with
//! `--toggle-collect=zdecode::decode_all_flex` (or `_c`).
//!
//! Then the lever the count pointed at, priced here before it was built and
//! kept to price the cut again: a point read decodes one group of a page
//! for one record. `--split k,...` cuts each group as read at record
//! boundaries into `k` runs of about equal bytes, compresses each run on its
//! own with the engine's encoder (the group recompressed whole is the
//! check: its bytes are the page's payload's for it), and reports what the
//! cut costs in bytes (the compressed bytes over the decoded, against the
//! file's own) and what it saves in decode: one run a group, the point
//! read's decode, and all `k` runs, the scan's, each cycling the groups as
//! above. The form as shipped cuts a page at two kibibytes of records
//! (`Options::zpages_group_bytes`); a store built with the target at zero
//! (one group a page) is what the split was first priced on.
//!
//! And the compressor's side of the decode: `--hc LEVEL` recompresses every
//! group with liblz4's high-compression mode at that level (a longer match
//! search, fewer tokens for the decoder to parse, the same block format)
//! and prices the cycling decode of those payloads beside their bytes, since
//! a cold decode's cost is the decoder's branches on tokens it has not
//! learned, and fewer tokens is the one lever on that the form's writer
//! holds. The split probe is skipped under it unless asked for.
//!
//! `--rhc ATTEMPTS:SEQCOST:SKIP` recompresses every group the same way with
//! the engine's own encoder (`supdb::db::zlz4`), the hash chain walked
//! `ATTEMPTS` positions deep, each sequence priced `SEQCOST` bytes over its
//! own and the inside of a match of `SKIP` or more not searched, and prices it as `--hc` does, with the encode's own time a
//! group beside it. The summary counts the sequences a group's payload
//! holds, the tokens a decoder parses, whichever encoder wrote it.
//!
//!     cargo run --release --features lz4-c --example zdecode -- [--size N] [--rounds N] [--split k,...] [--pages N] [--count flex|c] [--hc LEVEL] [--rhc ATTEMPTS:SEQCOST:SKIP]
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;
use supdb::db::zpages::{lz4_decoder, parse_page, Lz4Decoder, Trailer, ZPAGE, ZTRAILER};
use supdb::flatindex::record_key_next;
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, Payload, Rng};

/// A compressed group's payload, the length it decodes to and the records
/// in it.
struct Page {
    payload: Vec<u8>,
    ulen: usize,
    count: usize,
    /// The payload and the rest of its page after it, which the engine's own
    /// decoder may read into (`zlz4::decompress_in`); a recompressed payload
    /// is given sixty-four zeros instead.
    padded: Vec<u8>,
}

/// Every compressed group of every page of every form file under `dir`, in
/// file order.
fn form_pages(dir: &Path) -> Vec<Page> {
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
    let mut pages = Vec::new();
    let (mut stored, mut npages) = (0usize, 0usize);
    for f in &files {
        let bytes = std::fs::read(f).expect("a form file reads");
        let t = Trailer::decode(&bytes[bytes.len() - ZTRAILER..]).expect("a trailer");
        let bind = t.bind32();
        for i in 0..t.n_pages as usize {
            let page = &bytes[i * ZPAGE..(i + 1) * ZPAGE];
            let view = parse_page(page, bind).expect("a page we built");
            npages += 1;
            for j in 0..view.groups() {
                let g = view.group(j).expect("a group of the page");
                if g.stored {
                    stored += 1;
                    continue;
                }
                let (rest, len) = view.payload_and_rest(j).expect("its payload");
                pages.push(Page {
                    payload: rest[..len].to_vec(),
                    ulen: g.ulen as usize,
                    count: usize::from(g.count),
                    padded: rest.to_vec(),
                });
            }
        }
    }
    println!(
        "{} form files, {npages} pages, {} compressed groups, {} stored groups skipped",
        files.len(),
        pages.len(),
        stored
    );
    pages
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

/// The engine's decoder over one page, as `decode_group` calls it.
#[inline(always)]
fn flex(p: &Page, out: &mut [u8]) -> usize {
    lz4_flex::block::decompress_into(&p.padded[..p.payload.len()], &mut out[..p.ulen])
        .expect("the engine's decoder")
}

/// liblz4's `LZ4_decompress_safe` over one page, the size known as the
/// engine knows it from the header, no size prefix.
#[inline(always)]
fn c(p: &Page, out: &mut [u8]) -> usize {
    lz4::block::decompress_to_buffer(
        &p.padded[..p.payload.len()],
        Some(p.ulen as i32),
        &mut out[..p.ulen],
    )
    .expect("liblz4's decoder")
}

/// The engine's own decoder (`supdb::db::zlz4`) over one page, into an
/// output of exactly the decoded length: the shape `lz4_flex` and liblz4
/// are given, where the last 64 bytes go the careful way.
#[inline(always)]
fn ours(p: &Page, out: &mut [u8]) -> usize {
    supdb::db::zlz4::decompress_into(&p.padded[..p.payload.len()], &mut out[..p.ulen])
        .expect("the engine's own decoder")
}

/// The same into an output with the slack the fast loop runs to the end
/// in (`zlz4::DECODE_SLACK`), the shape the engine can give it.
#[inline(always)]
fn ours_roomy(p: &Page, out: &mut [u8]) -> usize {
    supdb::db::zlz4::decompress_into(
        &p.padded[..p.payload.len()],
        &mut out[..p.ulen + supdb::db::zlz4::DECODE_SLACK],
    )
    .expect("the engine's own decoder")
}

/// The engine's own decoder as the engine calls it: the payload with the
/// rest of its page after it and the output with its slack.
#[inline(always)]
fn ours_paged(p: &Page, out: &mut [u8]) -> usize {
    supdb::db::zlz4::decompress_in(
        &p.padded,
        p.payload.len(),
        &mut out[..p.ulen + supdb::db::zlz4::DECODE_SLACK],
    )
    .expect("the engine's own decoder")
}

/// Every page once through the engine's own decoder, exact and paged.
#[inline(never)]
fn decode_all_ours(pages: &[Page], out: &mut [u8]) -> usize {
    let mut n = 0;
    for p in pages {
        n += ours(black_box(p), out);
    }
    black_box(n)
}

#[inline(never)]
fn decode_all_roomy(pages: &[Page], out: &mut [u8]) -> usize {
    let mut n = 0;
    for p in pages {
        n += ours_paged(black_box(p), out);
    }
    black_box(n)
}

/// Every page once through the engine's decoder; the callgrind toggle.
#[inline(never)]
fn decode_all_flex(pages: &[Page], out: &mut [u8]) -> usize {
    let mut n = 0;
    for p in pages {
        n += flex(black_box(p), out);
    }
    black_box(n)
}

/// Every page once through liblz4's; the callgrind toggle.
#[inline(never)]
fn decode_all_c(pages: &[Page], out: &mut [u8]) -> usize {
    let mut n = 0;
    for p in pages {
        n += c(black_box(p), out);
    }
    black_box(n)
}

/// A page's group cut into runs of records, each compressed on its own:
/// the runs' compressed bytes laid end to end, and for each run where it
/// starts, its compressed length and its decoded length.
struct Runs {
    data: Vec<u8>,
    runs: Vec<(usize, usize, usize)>,
}

/// Every page's group cut into `k` runs at record boundaries, as near to
/// equal bytes as the records allow, and each run compressed with the
/// engine's encoder. Returns the runs and the bytes they would store: a
/// run the encoder expands is stored as it is, as the fill stores a page.
fn cut_runs(pages: &[Page], k: usize) -> (Vec<Runs>, usize) {
    let ulen_max = pages.iter().map(|p| p.ulen).max().unwrap();
    let mut g = vec![0u8; ulen_max];
    let mut tmp = vec![0u8; lz4_flex::block::get_maximum_output_size(ulen_max)];
    let mut stored = 0usize;
    let mut out = Vec::with_capacity(pages.len());
    for p in pages {
        let n = flex(p, &mut g);
        assert_eq!(n, p.ulen);
        let group = &g[..p.ulen];
        // The record boundaries, from the walk the engine's seek makes.
        let mut bounds = Vec::with_capacity(p.count + 1);
        let mut off = 0;
        for _ in 0..p.count {
            bounds.push(off);
            let (_, next) = record_key_next(group, off).expect("a whole group");
            off = next;
        }
        assert_eq!(off, p.ulen, "the records fill the group");
        bounds.push(p.ulen);
        // The cuts: for each target byte the first boundary at or past it,
        // each strictly after the last so no run is empty.
        let mut cuts = vec![0usize];
        for j in 1..k {
            let target = j * p.ulen / k;
            let at = bounds.partition_point(|&b| b < target);
            let b = bounds[at.min(bounds.len() - 1)];
            if b > *cuts.last().unwrap() && b < p.ulen {
                cuts.push(b);
            }
        }
        cuts.push(p.ulen);
        let mut r = Runs {
            data: Vec::new(),
            runs: Vec::new(),
        };
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let n = lz4_flex::block::compress_into(&group[a..b], &mut tmp).expect("compresses");
            stored += n.min(b - a);
            r.runs.push((r.data.len(), n, b - a));
            r.data.extend_from_slice(&tmp[..n]);
        }
        out.push(r);
    }
    (out, stored)
}

/// The decode of run `j` of a page by the engine's decoder.
#[inline(always)]
fn flex_run(r: &Runs, j: usize, out: &mut [u8]) -> usize {
    let (at, clen, ulen) = r.runs[j];
    lz4_flex::block::decompress_into(&r.data[at..at + clen], &mut out[..ulen]).expect("decodes")
}

/// The same by liblz4's.
#[inline(always)]
fn c_run(r: &Runs, j: usize, out: &mut [u8]) -> usize {
    let (at, clen, ulen) = r.runs[j];
    lz4::block::decompress_to_buffer(&r.data[at..at + clen], Some(ulen as i32), &mut out[..ulen])
        .expect("decodes")
}

/// One run a page, cycling the pages: the point read's decode under the
/// cut, the run chosen by the page so every position is read.
#[inline(never)]
fn decode_one_run(runs: &[Runs], out: &mut [u8], c_dec: bool) -> usize {
    let mut n = 0;
    for (i, r) in runs.iter().enumerate() {
        let j = i % r.runs.len();
        n += if c_dec {
            c_run(black_box(r), j, out)
        } else {
            flex_run(black_box(r), j, out)
        };
    }
    black_box(n)
}

/// Every run of every page, cycling the pages: the scan's decode under the
/// cut.
#[inline(never)]
fn decode_all_runs(runs: &[Runs], out: &mut [u8], c_dec: bool) -> usize {
    let mut n = 0;
    for r in runs {
        for j in 0..r.runs.len() {
            n += if c_dec {
                c_run(black_box(r), j, out)
            } else {
                flex_run(black_box(r), j, out)
            };
        }
    }
    black_box(n)
}

/// The split priced for each `k`: the bytes the cut costs and the decode it
/// saves, cycling the pages, the decoders alternating round by round.
fn split_probe(pages: &[Page], ks: &[usize], rounds: usize) {
    let total_u: usize = pages.iter().map(|p| p.ulen).sum();
    let total_c: usize = pages.iter().map(|p| p.payload.len()).sum();
    let ulen_max = pages.iter().map(|p| p.ulen).max().unwrap();
    let mut out = vec![0u8; ulen_max];
    println!(
        "\nsplit: each group cut into k runs compressed apart; bytes as the compressed over the decoded (the file's own {:.3}); ns a decode cycling {} groups, median of {rounds} rounds",
        total_c as f64 / total_u as f64,
        pages.len()
    );
    println!(
        "{:>3} {:>7} {:>6} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "k", "bytes", "ratio", "run B", "one flex", "one C", "all flex", "all C"
    );
    for &k in ks {
        let (runs, stored) = cut_runs(pages, k);
        if k == 1 {
            assert_eq!(stored, total_c, "a group recompressed whole is its payload");
        }
        // The runs decode to the group: every run, against the page's own
        // decode.
        let mut g = vec![0u8; ulen_max];
        for (p, r) in pages.iter().zip(&runs) {
            let n = flex(p, &mut g);
            let mut off = 0;
            for j in 0..r.runs.len() {
                let m = flex_run(r, j, &mut out);
                assert!(
                    g[off..off + m] == out[..m],
                    "a run decodes to its slice of the group"
                );
                let m2 = c_run(r, j, &mut out);
                assert!(m2 == m && g[off..off + m] == out[..m]);
                off += m;
            }
            assert_eq!(off, n);
        }
        let nruns: usize = runs.iter().map(|r| r.runs.len()).sum();
        let (mut one_f, mut one_c, mut all_f, mut all_c) = (vec![], vec![], vec![], vec![]);
        for r in 0..rounds {
            let time = |v: &mut Vec<f64>, f: &mut dyn FnMut() -> usize, per: usize| {
                let t = Instant::now();
                let n = f();
                let d = t.elapsed().as_nanos() as f64;
                black_box(n);
                v.push(d / per as f64);
            };
            let n = pages.len();
            if r % 2 == 0 {
                time(
                    &mut one_f,
                    &mut || decode_one_run(&runs, &mut out, false),
                    n,
                );
                time(&mut one_c, &mut || decode_one_run(&runs, &mut out, true), n);
                time(
                    &mut all_f,
                    &mut || decode_all_runs(&runs, &mut out, false),
                    n,
                );
                time(
                    &mut all_c,
                    &mut || decode_all_runs(&runs, &mut out, true),
                    n,
                );
            } else {
                time(&mut one_c, &mut || decode_one_run(&runs, &mut out, true), n);
                time(
                    &mut one_f,
                    &mut || decode_one_run(&runs, &mut out, false),
                    n,
                );
                time(
                    &mut all_c,
                    &mut || decode_all_runs(&runs, &mut out, true),
                    n,
                );
                time(
                    &mut all_f,
                    &mut || decode_all_runs(&runs, &mut out, false),
                    n,
                );
            }
        }
        println!(
            "{:>3} {:>7.3} {:>6.3} {:>9.0} {:>9.0} {:>9.0} {:>9.0} {:>9.0}",
            k,
            stored as f64 / total_u as f64,
            stored as f64 / total_c as f64,
            total_u as f64 / nruns as f64,
            median(&mut one_f),
            median(&mut one_c),
            median(&mut all_f),
            median(&mut all_c),
        );
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// A payload recompressed given sixty-four zeros after it for the room a
/// decoder that reads ahead may use; one read from its page keeps the page.
fn pad(pages: &mut [Page]) {
    for p in pages.iter_mut() {
        if !p.padded.starts_with(&p.payload) {
            p.padded = p.payload.clone();
            p.padded.extend([0u8; 64]);
        }
    }
}

fn main() {
    let mut size: u64 = 300_000;
    let mut rounds: usize = 21;
    let mut count: Option<String> = None;
    let mut ks: Vec<usize> = vec![1, 2, 4, 8, 16];
    let mut split_given = false;
    let mut hc: Option<i32> = None;
    let mut rhc: Option<(u32, u32, u32, u32)> = None;
    let mut first: Option<usize> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |what: &str| args.next().unwrap_or_else(|| panic!("{a} needs {what}"));
        match a.as_str() {
            "--size" => size = val("a count").parse().expect("a count"),
            "--rounds" => rounds = val("a count").parse().expect("a count"),
            "--count" => count = Some(val("flex or c")),
            // The cycle over the first N pages only: what the cycling figure
            // is made of, the predictor or the cache level the pages sit in.
            "--pages" => first = Some(val("a count").parse().expect("a count")),
            "--hc" => hc = Some(val("a level").parse().expect("a level")),
            "--rhc" => {
                let v = val("ATTEMPTS:SEQCOST:SKIP[:LAZY]");
                let f: Vec<u32> = v.split(':').map(|x| x.parse().expect("a count")).collect();
                assert!(f.len() == 3 || f.len() == 4, "ATTEMPTS:SEQCOST:SKIP[:LAZY]");
                rhc = Some((f[0], f[1], f[2], f.get(3).copied().unwrap_or(0)));
            }
            "--split" => {
                split_given = true;
                ks = val("a list")
                    .split(',')
                    .map(|s| s.parse().expect("a count"))
                    .collect()
            }
            other => panic!("unknown argument {other}"),
        }
    }
    let decoder = lz4_decoder();
    if decoder != Lz4Decoder::Unsafe {
        eprintln!("the engine's decoder in this build is {decoder:?}, not the one it ships");
        std::process::exit(3);
    }
    let root = std::env::temp_dir().join(format!("supdb-zdecode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    {
        let payload = Payload::new(100, 0.5, 0xE1);
        let mut e = engines::open("supdb-nocachezpages", &root, 1).unwrap();
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        let mut kb = [0u8; 16];
        let t = Instant::now();
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        e.sync().unwrap();
        println!(
            "{size} keys loaded under {} in {:.2} s",
            e.name(),
            t.elapsed().as_secs_f64()
        );
    }
    let mut pages = form_pages(&root);
    let _ = std::fs::remove_dir_all(&root);
    if let Some(n) = first {
        pages.truncate(n);
    }
    pad(&mut pages);
    if let Some(level) = hc {
        // Every group recompressed by liblz4's high-compression mode from
        // the bytes the engine's decoder gives for it; a group it would
        // not shrink keeps the engine's payload, as the fill keeps a group
        // the encoder does not shrink.
        let before: usize = pages.iter().map(|p| p.payload.len()).sum();
        let mut g = vec![0u8; pages.iter().map(|p| p.ulen).max().unwrap()];
        let t = Instant::now();
        for p in pages.iter_mut() {
            let n = flex(p, &mut g);
            assert_eq!(n, p.ulen);
            let out = lz4::block::compress(
                &g[..p.ulen],
                Some(lz4::block::CompressionMode::HIGHCOMPRESSION(level)),
                false,
            )
            .expect("liblz4's HC compressor");
            if out.len() < p.payload.len() {
                p.payload = out;
            }
        }
        let after: usize = pages.iter().map(|p| p.payload.len()).sum();
        println!(
            "hc {level}: {} compressed bytes recompressed to {} ({:.3} of the engine's) in {:.2} s",
            before,
            after,
            after as f64 / before as f64,
            t.elapsed().as_secs_f64()
        );
        if !split_given {
            ks.clear();
        }
    }
    if let Some((attempts, seq_cost, skip_min, lazy)) = rhc {
        // The same recompression with the engine's own encoder, timed a
        // group: the decode into the scratch first, outside the clock.
        let before: usize = pages.iter().map(|p| p.payload.len()).sum();
        let ulen_max = pages.iter().map(|p| p.ulen).max().unwrap();
        let mut g = vec![0u8; ulen_max];
        let mut out = vec![0u8; supdb::db::zlz4::max_output(ulen_max)];
        let mut enc = supdb::db::zlz4::Encoder::new();
        let effort = supdb::db::zlz4::Effort {
            attempts,
            seq_cost,
            skip_min,
            lazy,
        };
        let mut enc_ns = 0u128;
        let mut flex_ns = 0u128;
        let mut flex_out = vec![0u8; lz4_flex::block::get_maximum_output_size(ulen_max)];
        for p in pages.iter_mut() {
            let n = flex(p, &mut g);
            assert_eq!(n, p.ulen);
            // The greedy encoder the fill has used, on the same group, for
            // the build's price of the change.
            let t = Instant::now();
            black_box(lz4_flex::block::compress_into(&g[..p.ulen], &mut flex_out).unwrap());
            flex_ns += t.elapsed().as_nanos();
            let t = Instant::now();
            let m = enc
                .compress_into(&g[..p.ulen], &mut out, effort)
                .expect("the engine's encoder");
            enc_ns += t.elapsed().as_nanos();
            if m < p.payload.len() {
                p.payload = out[..m].to_vec();
            }
        }
        let after: usize = pages.iter().map(|p| p.payload.len()).sum();
        println!(
            "rhc {attempts}:{seq_cost}:{skip_min}:{lazy}: {} compressed bytes recompressed to {} ({:.3} of the engine's), {:.2} us a group to encode, lz4_flex {:.2}",
            before,
            after,
            after as f64 / before as f64,
            enc_ns as f64 / 1e3 / pages.len() as f64,
            flex_ns as f64 / 1e3 / pages.len() as f64
        );
        if !split_given {
            ks.clear();
        }
    }
    pad(&mut pages);
    {
        let room = |p: &Page| p.padded.len() - p.payload.len();
        let short = pages.iter().filter(|p| room(p) < 64).count();
        println!(
            "groups with under 64 bytes of their page after them: {short} of {} ({:.1}%)",
            pages.len(),
            100.0 * short as f64 / pages.len() as f64
        );
    }
    let total_u: usize = pages.iter().map(|p| p.ulen).sum();
    let total_c: usize = pages.iter().map(|p| p.payload.len()).sum();
    let seqs: usize = pages
        .iter()
        .map(|p| supdb::db::zlz4::sequences(&p.payload).expect("a block"))
        .sum();
    // The sequences by the decoder's path: a run of fifteen or more, a match
    // of nineteen or more, and a match from under eight or sixteen back.
    let (mut long_lit, mut long_ml, mut near8, mut near16) = (0usize, 0usize, 0usize, 0usize);
    let (mut lit_h, mut ml_h) = ([0usize; 6], [0usize; 6]);
    let bucket = |n: usize| {
        [16usize, 32, 48, 64, 128]
            .iter()
            .position(|&b| n < b)
            .unwrap_or(5)
    };
    for p in &pages {
        let b = &p.payload;
        let mut i = 0;
        loop {
            let t = b[i];
            i += 1;
            let mut lit = (t >> 4) as usize;
            if lit == 15 {
                long_lit += 1;
                loop {
                    let x = b[i];
                    i += 1;
                    lit += x as usize;
                    if x != 255 {
                        break;
                    }
                }
            }
            if lit >= 15 {
                lit_h[bucket(lit)] += 1;
            }
            i += lit;
            if i >= b.len() {
                break;
            }
            let off = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
            i += 2;
            near8 += usize::from(off < 8);
            near16 += usize::from(off < 16);
            if t & 15 == 15 {
                long_ml += 1;
                let mut ml = 19usize;
                loop {
                    let x = b[i];
                    i += 1;
                    ml += x as usize;
                    if x != 255 {
                        break;
                    }
                }
                ml_h[bucket(ml)] += 1;
            }
        }
    }
    println!(
        "sequences by path: long runs {:.1}%, long matches {:.1}%, offsets under 8 {:.1}%, under 16 {:.1}%",
        100.0 * long_lit as f64 / seqs as f64,
        100.0 * long_ml as f64 / seqs as f64,
        100.0 * near8 as f64 / seqs as f64,
        100.0 * near16 as f64 / seqs as f64
    );
    println!("long runs by length under 16/32/48/64/128/more: {lit_h:?}; long matches: {ml_h:?}");
    println!(
        "{} compressed bytes to {} decoded, {:.2} a group decoded, ratio {:.3}, {:.1} sequences a group",
        total_c,
        total_u,
        total_u as f64 / pages.len() as f64,
        total_c as f64 / total_u as f64,
        seqs as f64 / pages.len() as f64
    );
    let ulen_max = pages.iter().map(|p| p.ulen).max().unwrap();
    let mut a = vec![0u8; ulen_max + supdb::db::zlz4::DECODE_SLACK];
    let mut b = vec![0u8; ulen_max + supdb::db::zlz4::DECODE_SLACK];

    // Agreement first: both decoders, every group, byte for byte and in
    // length.
    for (i, p) in pages.iter().enumerate() {
        let na = flex(p, &mut a);
        let nb = c(p, &mut b);
        assert_eq!((na, nb), (p.ulen, p.ulen), "page {i}: a decoder's length");
        assert!(
            a[..p.ulen] == b[..p.ulen],
            "page {i}: the decoders disagree"
        );
        for dec in [
            ours as fn(&Page, &mut [u8]) -> usize,
            ours_roomy,
            ours_paged,
        ] {
            assert_eq!(
                dec(p, &mut b),
                p.ulen,
                "page {i}: the engine's own decoder's length"
            );
            assert!(
                a[..p.ulen] == b[..p.ulen],
                "page {i}: the engine's own decoder disagrees"
            );
        }
    }
    println!("the decoders agree on every group");

    if let Some(which) = count {
        let n = match which.as_str() {
            "flex" => decode_all_flex(&pages, &mut a),
            "c" => decode_all_c(&pages, &mut a),
            "ours" => decode_all_ours(&pages, &mut a),
            "paged" => decode_all_roomy(&pages, &mut a),
            other => panic!("--count takes flex, c, ours or paged, not {other}"),
        };
        println!(
            "counted {which}: {n} bytes decoded over {} groups",
            pages.len()
        );
        return;
    }

    // Cycling: every page once a round, the order of the decoders swapped
    // each round.
    let (mut cyc_f, mut cyc_c) = (Vec::new(), Vec::new());
    let (mut cyc_o, mut cyc_r) = (Vec::new(), Vec::new());
    for r in 0..rounds {
        // The four decoders in an order rotated a place each round.
        for k in 0..4 {
            let which = (k + r) % 4;
            let t = Instant::now();
            let n = match which {
                0 => decode_all_flex(&pages, &mut a),
                1 => decode_all_c(&pages, &mut a),
                2 => decode_all_ours(&pages, &mut a),
                _ => decode_all_roomy(&pages, &mut a),
            };
            let d = t.elapsed().as_nanos() as f64 / pages.len() as f64;
            assert_eq!(n, total_u);
            [&mut cyc_f, &mut cyc_c, &mut cyc_o, &mut cyc_r][which].push(d);
        }
    }
    // Hot: a few pages spread over the file, each decoded many times on
    // its own; the median over the pages of the median over the rounds.
    let picks: Vec<usize> = (0..16).map(|i| i * pages.len() / 16).collect();
    let hot = |dec: &dyn Fn(&Page, &mut [u8]) -> usize, out: &mut [u8]| -> f64 {
        let mut per_page = Vec::new();
        for &pi in &picks {
            let p = &pages[pi];
            let mut rs = Vec::new();
            for _ in 0..rounds {
                let t = Instant::now();
                for _ in 0..200 {
                    black_box(dec(black_box(p), out));
                }
                rs.push(t.elapsed().as_nanos() as f64 / 200.0);
            }
            per_page.push(median(&mut rs));
        }
        median(&mut per_page)
    };
    let hot_f = hot(&flex, &mut a);
    let hot_c = hot(&c, &mut a);
    let hot_f2 = hot(&flex, &mut a);
    let hot_c2 = hot(&c, &mut a);
    let hot_o = hot(&ours, &mut a);
    let hot_r = hot(&ours_paged, &mut a);
    let bytes = total_u as f64 / pages.len() as f64;
    let (mf, mc) = (median(&mut cyc_f), median(&mut cyc_c));
    let (mo, mr) = (median(&mut cyc_o), median(&mut cyc_r));
    println!(
        "cycle, the engine's own decoder: exact {:.0} ns a group ({:.2} of lz4_flex, {:.2} of liblz4), paged {:.0} ({:.2}, {:.2}); hot exact {:.0}, paged {:.0}",
        mo,
        mo / mf,
        mo / mc,
        mr,
        mr / mf,
        mr / mc,
        hot_o,
        hot_r
    );
    println!(
        "cycle ({rounds} rounds over {} groups): lz4_flex {:.0} ns a group ({:.3} ns a byte), liblz4 {:.0} ns a group ({:.3} ns a byte); liblz4 at {:.2} of lz4_flex",
        pages.len(),
        mf,
        mf / bytes,
        mc,
        mc / bytes,
        mc / mf
    );
    println!(
        "hot (16 groups, 200 decodes a sample, {rounds} samples): lz4_flex {:.0} / {:.0} ns a group, liblz4 {:.0} / {:.0} ns a group; liblz4 at {:.2} of lz4_flex",
        hot_f,
        hot_f2,
        hot_c,
        hot_c2,
        (hot_c + hot_c2) / (hot_f + hot_f2)
    );
    if !ks.is_empty() {
        split_probe(&pages, &ks, rounds);
    }
}
