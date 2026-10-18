//! What the z form's layout makes of the suite's records: the real `fill_group`
//! over the record images of partitions the engine wrote, for the four key
//! shapes a store may have and three payload compressibilities.
//!
//! ```text
//! cargo run --release --example zsize -- [--keys N] [--shapes seq,rdec,rbytes,uuid]
//!                                        [--comp 0.5,0,1] [--rounds R] [--dir PATH] [--no-time]
//! cargo run --release --example zsize --features lz4-safe -- ...      # the cross-check codec
//! ```
//!
//! A cell is a key shape and a payload: N keys with the suite's hundred-byte
//! values (`Payload`, the compressibility given) loaded through `Db` the way
//! the suite loads -- ordered keys in order, the others in the order drawn --
//! and flushed into partitions. For every record of every partition the image
//! the directory points at is compared with the record `stream_record` makes of
//! what `exts_at_full` reads back, byte for byte; the images then go through
//! `fill_group` a page at a time with the previous page's count as the hint,
//! and every page it builds goes through `verify_page`, the check a reader's
//! first decode makes.
//!
//! Printed: the size of what was built (records and bytes a page, fill,
//! compress calls a page); the time of what a read does with a page, on this
//! machine and hot unless the column says cold (decode, the walk that finds a
//! key, the walk that checks a group, the CRC, the whole first decode) and what
//! `fill_group` itself takes a page; and the occupancy of the cache the bound
//! leaves at ten million keys, with the share of a build that `fill_group`
//! is at that size. Pages are
//! built from the records and nothing else, so what is printed is a function
//! of the images, the layout and the codec: the codec is printed, and the
//! digest of the pages is, so that a build with the other codec can be held to
//! the same bytes.
//!
//! Exit status 2 if the suite's shape (sequential keys, half-compressible
//! values) comes out under 36 records a page, the point under which the form
//! cannot match LMDB's reads at the cell it was designed for; 3 if the decoder
//! that ran is not the one the build says it linked.
use std::collections::HashSet;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::db::zpages::{
    bind32, decode_group, fill_group, group_seek, lz4_decoder, page_crc, parse_page, verify_page,
    walk_group, FenceEntry, Fill, FillScratch, FillStats, Group, GroupBuf, Images, Lz4Decoder,
    SegImages, ZCAP, ZPAGE, ZTRAILER,
};
use supdb::flatindex::{self, record_key, record_key_next, FlatIndex};
use supdb::{Blob, Bytes, Db, MmapBytes, Options};
use supdb_bench::workload::{db_key_into, Payload, Rng};

/// The cache a 1 GiB bound leaves at the suite's out-of-core cell, in bytes:
/// the figure the design's occupancy arithmetic uses.
const CACHE_B: f64 = 937e6;
/// The cell's key count.
const CELL_KEYS: f64 = 10_000_000.0;
/// The filter's bits a key and the block it is set in, as `BlockedBloom` has
/// them: 10 bits a key, 512 bits to a block.
const FILTER_BITS_PER_KEY: f64 = 10.0;
/// What every page of the example is bound by: the hash of an identity.
fn bind() -> u32 {
    bind32(1, 2, 3, 4)
}
/// The pages timed per cell.
const SAMPLE: usize = 400;
/// The lock this build resolved, which names the codec's version.
const LOCK: &str = include_str!("../Cargo.lock");

fn lz4_version() -> &'static str {
    let at = LOCK.find("name = \"lz4_flex\"").unwrap_or(0);
    LOCK[at..]
        .lines()
        .nth(1)
        .and_then(|l| l.strip_prefix("version = \""))
        .and_then(|l| l.strip_suffix('"'))
        .unwrap_or("?")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// Sixteen decimal digits of a counter: the suite's keys.
    Seq,
    /// Sixteen random decimal digits.
    RandDec,
    /// Sixteen random bytes.
    RandBytes,
    /// A random version-4 UUID as thirty-six characters of text.
    Uuid,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Shape::Seq => "seq-dec16",
            Shape::RandDec => "rand-dec16",
            Shape::RandBytes => "rand-bytes16",
            Shape::Uuid => "uuid-text36",
        }
    }

    fn parse(s: &str) -> Option<Shape> {
        Some(match s {
            "seq" => Shape::Seq,
            "rdec" => Shape::RandDec,
            "rbytes" => Shape::RandBytes,
            "uuid" => Shape::Uuid,
            _ => return None,
        })
    }

    /// `n` distinct keys in the order they arrive.
    fn keys(self, n: usize, seed: u64) -> Vec<Vec<u8>> {
        let mut rng = Rng::new(seed);
        if self == Shape::Seq {
            return (0..n as u64)
                .map(|i| {
                    let mut b = [0u8; 16];
                    db_key_into(i, &mut b);
                    b.to_vec()
                })
                .collect();
        }
        let mut seen = HashSet::with_capacity(n);
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let k: Vec<u8> = match self {
                Shape::RandDec => {
                    let mut b = [0u8; 16];
                    db_key_into(rng.below(10_000_000_000_000_000), &mut b);
                    b.to_vec()
                }
                Shape::RandBytes => (0..16).map(|_| (rng.next() >> 40) as u8).collect(),
                _ => {
                    let (a, b) = (rng.next(), rng.next());
                    format!(
                        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
                        (a >> 32) as u32,
                        (a >> 16) as u16,
                        a as u16 & 0xfff,
                        8 + (b >> 62),
                        (b >> 48) as u16 & 0xfff,
                        b & 0xffff_ffff_ffff
                    )
                    .into_bytes()
                }
            };
            if seen.insert(k.clone()) {
                out.push(k);
            }
        }
        out
    }
}

/// What one cell built and measured.
struct Cell {
    shape: Shape,
    comp: f64,
    keys: usize,
    partitions: usize,
    /// Bytes of record images, and of the record region between them.
    image_bytes: u64,
    gap_bytes: u64,
    key_bytes: u64,
    pages: Vec<[u8; ZPAGE]>,
    /// Rank within its partition, count, and first key of each page.
    meta: Vec<(u32, u32, Vec<u8>)>,
    groups: Vec<Group>,
    declined: u64,
    stats: FillStats,
    /// Nanoseconds spent inside `fill_group`: the compress calls, and the
    /// decode and compare of each page it built.
    fill_ns: u64,
    digest: u64,
    /// grow_rounds, records a page, fill, calls a page.
    sweep: Vec<(u32, f64, f64, f64)>,
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Load `keys` through the engine as the suite does and return the segment
/// files it left.
fn load(dir: &Path, keys: &[Vec<u8>], payload: &Payload) -> Vec<PathBuf> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("scratch directory");
    let mut db = Db::create(dir, Options::default()).expect("create");
    let mut vr = Rng::new(0xE2);
    for chunk in keys.chunks(1000) {
        let items: Vec<(&[u8], &[u8])> = chunk
            .iter()
            .map(|k| (k.as_slice(), payload.get(&mut vr)))
            .collect();
        db.append_batch(&items);
        db.commit().expect("commit");
    }
    db.flush().expect("flush");
    db.close().expect("close");
    let mut segs: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read the store")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sup"))
        .collect();
    segs.sort();
    segs
}

/// The stored records of every partition checked against the writer's
/// encoder, then run through `fill_group`; the cell's numbers.
fn analyze(segs: &[PathBuf], want: usize, shape: Shape, comp: f64, sweep: bool) -> Cell {
    let mut cell = Cell {
        shape,
        comp,
        keys: 0,
        partitions: segs.len(),
        image_bytes: 0,
        gap_bytes: 0,
        key_bytes: 0,
        pages: Vec::new(),
        meta: Vec::new(),
        groups: Vec::new(),
        declined: 0,
        stats: FillStats::default(),
        fill_ns: 0,
        digest: 0xcbf2_9ce4_8422_2325,
        sweep: Vec::new(),
    };
    let mut scratch = FillScratch::new();
    let mut vbuf = GroupBuf::new();
    for (pi, path) in segs.iter().enumerate() {
        let src = MmapBytes::open(path).expect("map");
        let blob = Blob::open(MmapBytes::open(path).expect("map")).expect("open");
        let off = blob.index_offset();
        let sec = src
            .slice_at(off, (src.len() - off) as usize)
            .expect("the key section");
        let idx = FlatIndex::parse(sec).expect("a flat key section");
        let (recs, dir) = idx.regions(sec).expect("the regions");
        let n = blob.keys();
        let images = SegImages::new(recs, dir, n);
        // The stored record is the record the writer's encoder makes of what
        // reads back.
        let mut end = None::<usize>;
        for r in 0..n {
            let stored = images.image(r);
            let (key, exts, tail) = blob.exts_at_full(r).expect("a rank reads back");
            let mut made = Vec::with_capacity(stored.len());
            flatindex::stream_record(&mut made, key, &exts, tail).expect("streamable");
            assert!(
                stored == &made[..],
                "{}: rank {r}: the stored record is not what stream_record makes",
                path.display()
            );
            let at = idx.record_off(dir, r).expect("a directory word");
            if let Some(e) = end {
                assert!(at >= e, "records ascend with rank");
                cell.gap_bytes += (at - e) as u64;
            }
            end = Some(at + stored.len());
            cell.image_bytes += stored.len() as u64;
            cell.key_bytes += key.len() as u64;
        }
        cell.keys += n;
        // The pages.
        let (mut rank, mut hint) = (0usize, 0usize);
        while rank < n {
            let mut page = [0u8; ZPAGE];
            let t = Instant::now();
            let made = fill_group(&images, rank, hint, bind(), &mut scratch, &mut page);
            cell.fill_ns += t.elapsed().as_nanos() as u64;
            match made {
                Fill::Page(g) => {
                    let key = record_key(images.image(rank), 0).expect("a key").to_vec();
                    let entry = FenceEntry {
                        index: cell.pages.len(),
                        first_rank: rank as u32,
                        count: g.count as u32,
                        page: cell.pages.len() as u32,
                        key: &key,
                    };
                    verify_page(&page, bind(), &entry, &mut vbuf)
                        .unwrap_or_else(|e| panic!("partition {pi} rank {rank}: {e}"));
                    cell.digest = fnv(cell.digest, &page);
                    cell.meta.push((rank as u32, g.count as u32, key));
                    cell.pages.push(page);
                    cell.groups.push(g);
                    rank += g.count;
                    hint = g.count;
                }
                Fill::NoPage(_) => {
                    cell.declined += 1;
                    rank += 1;
                }
                other => panic!("partition {pi} rank {rank}: {other:?}"),
            }
        }
        // The knob, over the first partition.
        if sweep && pi == 0 {
            for rounds in 0..=3u32 {
                let mut s = FillScratch::new();
                s.grow_rounds = rounds;
                let (mut rank, mut hint) = (0usize, 60usize);
                while rank < n {
                    let mut page = [0u8; ZPAGE];
                    match fill_group(&images, rank, hint, bind(), &mut s, &mut page) {
                        Fill::Page(g) => {
                            rank += g.count;
                            hint = g.count;
                        }
                        Fill::NoPage(_) => rank += 1,
                        other => panic!("sweep rank {rank}: {other:?}"),
                    }
                }
                let p = s.stats.pages as f64;
                cell.sweep.push((
                    rounds,
                    s.stats.records as f64 / p,
                    s.stats.payload_bytes as f64 / (p * ZCAP as f64),
                    s.stats.calls as f64 / p,
                ));
            }
        }
    }
    assert_eq!(cell.keys, want, "every key is in exactly one partition");
    cell.stats = scratch.stats;
    cell
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    v[v.len() / 2]
}

/// Mean nanoseconds of `reps` calls.
fn ns_of<F: FnMut()>(reps: u32, mut f: F) -> f64 {
    let t = Instant::now();
    for _ in 0..reps {
        f();
    }
    t.elapsed().as_nanos() as f64 / f64::from(reps)
}

/// Drop a page's lines from every level of the cache, so the decode that
/// follows reads them from memory as a read of a page that was just faulted
/// in does. Only where the instruction exists.
fn evict(page: &[u8; ZPAGE]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{_mm_clflush, _mm_mfence};
        for line in page.chunks(64) {
            // SAFETY: a flush of a line inside a live slice.
            unsafe { _mm_clflush(line.as_ptr()) };
        }
        // SAFETY: a fence.
        unsafe { _mm_mfence() };
        true
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = page;
        false
    }
}

/// Microseconds a page, medians over the sampled pages (in a round) and then
/// over the rounds.
struct Times {
    decode: f64,
    decode_range: (f64, f64),
    cold: Option<f64>,
    find: f64,
    walk: f64,
    parse: f64,
    crc: f64,
    verify: f64,
    stored_pages: usize,
    sampled: usize,
}

fn time_cell(cell: &Cell, rounds: usize) -> Times {
    let total = cell.pages.len();
    let take = SAMPLE.min(total);
    let picks: Vec<usize> = (0..take).map(|i| i * total / take).collect();
    let high = [0xFFu8; 64];
    let mut rows: Vec<[f64; 7]> = Vec::new();
    let mut stored_pages = 0;
    for _ in 0..rounds {
        let (mut decode, mut cold, mut find, mut walk, mut parse, mut crc, mut verify) =
            (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut buf = GroupBuf::new();
        let mut vbuf = GroupBuf::new();
        let mut cold_ok = true;
        stored_pages = 0;
        for &pi in &picks {
            let page = &cell.pages[pi];
            let (h, payload) = parse_page(page, bind()).expect("a page we built");
            let (rank, count, key) = &cell.meta[pi];
            let entry = FenceEntry {
                index: pi,
                first_rank: *rank,
                count: *count,
                page: pi as u32,
                key,
            };
            crc.push(ns_of(200, || {
                black_box(page_crc(black_box(page), bind()));
            }));
            verify.push(ns_of(40, || {
                black_box(verify_page(black_box(page), bind(), &entry, &mut vbuf).is_ok());
            }));
            if h.stored {
                stored_pages += 1;
            } else {
                decode.push(ns_of(100, || {
                    let _ =
                        black_box(decode_group(&h, black_box(payload), &mut buf).map(|g| g.len()));
                }));
                cold_ok &= evict(page);
                let t = Instant::now();
                black_box(decode_group(&h, payload, &mut buf).map(|g| g.len()).ok());
                cold.push(t.elapsed().as_nanos() as f64);
            }
            let group = decode_group(&h, payload, &mut buf).expect("decodes");
            let n = h.count as usize;
            let mut keys: Vec<&[u8]> = Vec::with_capacity(n);
            let mut off = 0;
            for _ in 0..n {
                let (k, next) = record_key_next(group, off).expect("a whole group");
                keys.push(k);
                off = next;
            }
            find.push(
                ns_of(40, || {
                    for k in &keys {
                        black_box(group_seek(black_box(group), n, k));
                    }
                }) / n as f64,
            );
            walk.push(ns_of(100, || {
                black_box(group_seek(black_box(group), n, &high));
            }));
            parse.push(ns_of(100, || {
                black_box(walk_group(black_box(group), n, None).is_ok());
            }));
        }
        let med = |v: &mut [f64]| {
            if v.is_empty() {
                f64::NAN
            } else {
                median(v) / 1000.0
            }
        };
        rows.push([
            med(&mut decode),
            if cold_ok { med(&mut cold) } else { f64::NAN },
            med(&mut find),
            med(&mut walk),
            med(&mut parse),
            med(&mut crc),
            med(&mut verify),
        ]);
    }
    let across = |i: usize| {
        let mut v: Vec<f64> = rows.iter().map(|r| r[i]).filter(|x| !x.is_nan()).collect();
        if v.is_empty() {
            f64::NAN
        } else {
            median(&mut v)
        }
    };
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for r in &rows {
        lo = lo.min(r[0]);
        hi = hi.max(r[0]);
    }
    Times {
        decode: across(0),
        decode_range: (lo, hi),
        cold: Some(across(1)).filter(|c| !c.is_nan()),
        find: across(2),
        walk: across(3),
        parse: across(4),
        crc: across(5),
        verify: across(6),
        stored_pages,
        sampled: take,
    }
}

/// The cell at ten million keys: pages, and with the fence entries and the
/// filter the resident part, against the cache the bound leaves.
fn occupancy(cell: &Cell) -> (f64, f64, f64, f64) {
    let rpp = cell.keys as f64 / cell.pages.len() as f64;
    let pages = (CELL_KEYS / rpp).ceil();
    let first_key =
        cell.meta.iter().map(|m| m.2.len()).sum::<usize>() as f64 / cell.meta.len() as f64;
    let fence = pages * (12.0 + first_key) + 4.0;
    let filter = (CELL_KEYS * FILTER_BITS_PER_KEY / 512.0).ceil() * 64.0;
    let resident = fence + filter + ZTRAILER as f64;
    let pages_bytes = pages * ZPAGE as f64;
    (
        pages,
        pages_bytes,
        resident,
        (pages_bytes + resident) / CACHE_B,
    )
}

fn main() {
    let mut keys = 200_000usize;
    let mut shapes = vec![Shape::Seq, Shape::RandDec, Shape::RandBytes, Shape::Uuid];
    let mut comps = vec![0.5f64, 0.0, 1.0];
    let mut rounds = 3usize;
    let mut dir = std::env::temp_dir();
    let mut timing = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |what: &str| args.next().unwrap_or_else(|| panic!("{a} needs {what}"));
        match a.as_str() {
            "--keys" => keys = val("a count").parse().expect("a count"),
            "--rounds" => rounds = val("a count").parse().expect("a count"),
            "--dir" => dir = PathBuf::from(val("a path")),
            "--shapes" => {
                shapes = val("a list")
                    .split(',')
                    .map(|s| Shape::parse(s).unwrap_or_else(|| panic!("no shape {s}")))
                    .collect()
            }
            "--comp" => {
                comps = val("a list")
                    .split(',')
                    .map(|s| s.parse().expect("a fraction"))
                    .collect()
            }
            "--no-time" => timing = false,
            other => panic!("unknown argument {other}"),
        }
    }

    // The codec, found by running it, beside what the build says it linked.
    let decoder = lz4_decoder();
    let safe_feature = cfg!(feature = "lz4-safe");
    println!(
        "codec: lz4_flex {} (bench/Cargo.lock); decoder run: {decoder:?}; bench feature lz4-safe: {}; the encoders emit the same bytes (compare the digests)",
        lz4_version(),
        if safe_feature { "on" } else { "off" },
    );
    let want = if safe_feature {
        Lz4Decoder::Safe
    } else {
        Lz4Decoder::Unsafe
    };
    if decoder != want {
        eprintln!(
            "the decoder that ran is not the one this build was asked to link: expected {want:?}"
        );
        std::process::exit(3);
    }
    println!(
        "{keys} keys a cell, values of 100 bytes, partitions under {}, timing {} ({rounds} rounds of {SAMPLE} sampled pages)\n",
        dir.display(),
        if timing { "on" } else { "off" },
    );

    let mut cells: Vec<(Cell, Option<Times>)> = Vec::new();
    for &shape in &shapes {
        for &comp in &comps {
            let ks = shape.keys(keys, 7);
            let payload = Payload::new(100, comp, 0xE1);
            let work = dir.join(format!(
                "supdb-zsize-{}-{}-{comp}",
                std::process::id(),
                shape.name()
            ));
            let segs = load(&work, &ks, &payload);
            let sweep = shape == Shape::Seq && (comp - 0.5).abs() < 1e-9;
            let cell = analyze(&segs, keys, shape, comp, sweep);
            let _ = std::fs::remove_dir_all(&work);
            let times = timing.then(|| time_cell(&cell, rounds));
            eprintln!("done: {} comp {comp}", shape.name());
            cells.push((cell, times));
        }
    }

    println!("SIZE (what fill_group built; every page decoded, compared and verified)");
    println!(
        "{:<13} {:>4} {:>6} {:>5} {:>7} {:>8} {:>8} {:>7} {:>6} {:>7} {:>6} {:>7} {:>6} {:>7} {:>18}",
        "shape", "comp", "parts", "rec_B", "pages", "rec/page", "grpB/pg", "payB/pg", "B/key", "fill", "stored", "nopage", "calls", "turns", "digest"
    );
    for (c, _) in &cells {
        let p = c.pages.len() as f64;
        println!(
            "{:<13} {:>4} {:>6} {:>5.1} {:>7} {:>8.2} {:>8.1} {:>7.1} {:>6.2} {:>7.4} {:>5.1}% {:>7} {:>6.3} {:>7.3} {:>18x}",
            c.shape.name(),
            c.comp,
            c.partitions,
            c.image_bytes as f64 / c.keys as f64,
            c.pages.len(),
            c.keys as f64 / p,
            c.stats.group_bytes as f64 / p,
            c.stats.payload_bytes as f64 / p,
            ZPAGE as f64 * p / c.keys as f64,
            c.stats.payload_bytes as f64 / (p * ZCAP as f64),
            100.0 * c.stats.stored as f64 / p,
            c.declined,
            c.stats.calls as f64 / p,
            c.stats.turns as f64 / p,
            c.digest,
        );
    }
    println!(
        "\nstored images are byte-equal to stream_record's: checked for every record of every cell"
    );
    for (c, _) in &cells {
        if c.gap_bytes > 0 {
            println!(
                "  {} comp {}: {} bytes of the record region lie between records (commit markers of a direct run); the directory skips them",
                c.shape.name(),
                c.comp,
                c.gap_bytes
            );
        }
    }

    if timing {
        println!(
            "\nTIME, microseconds a page, medians over sampled pages and rounds (hot unless cold)"
        );
        println!(
            "{:<13} {:>4} {:>8} {:>16} {:>8} {:>8} {:>8} {:>8} {:>8} {:>7} {:>9} {:>9}",
            "shape",
            "comp",
            "decode",
            "(range of rounds)",
            "cold",
            "find",
            "walk",
            "parse",
            "crc",
            "verify",
            "fill_group",
            "stored/n"
        );
        for (c, t) in &cells {
            let t = t.as_ref().expect("timed");
            println!(
                "{:<13} {:>4} {:>8.3} {:>7.3}-{:<8.3} {:>8} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>7.3} {:>10.3} {:>4}/{:<4}",
                c.shape.name(),
                c.comp,
                t.decode,
                t.decode_range.0,
                t.decode_range.1,
                t.cold.map_or("n/a".to_string(), |v| format!("{v:.3}")),
                t.find,
                t.walk,
                t.parse,
                t.crc,
                t.verify,
                c.fill_ns as f64 / c.pages.len() as f64 / 1000.0,
                t.stored_pages,
                t.sampled,
            );
        }
        println!("  decode: the LZ4 block into the buffer (stored pages are lent in place and not in the median); cold: the page's lines flushed first;");
        println!("  find: the walk to one key, averaged over the page's keys; walk: the walk past every record; parse: the structural walk (parse_record);");
        println!("  crc: the page's checksum; verify: a reader's whole first decode of a page;");
        println!("  fill_group: the builder's call for the page, its compress calls and the decode and compare of the page it made, once, in the load's pass");
    }

    println!(
        "\nOCCUPANCY at {CELL_KEYS:.0} keys, against {:.0} MB of cache",
        CACHE_B / 1e6
    );
    println!(
        "{:<13} {:>4} {:>11} {:>10} {:>12} {:>10} {:>14}",
        "shape", "comp", "pages", "pages_GB", "resident_MB", "occupancy", "fill_group_s"
    );
    for (c, _) in &cells {
        let (pages, bytes, resident, occ) = occupancy(c);
        let fill = if timing {
            format!(
                "{:.1}",
                c.fill_ns as f64 / c.pages.len() as f64 * pages / 1e9
            )
        } else {
            "-".to_string()
        };
        println!(
            "{:<13} {:>4} {:>11.0} {:>10.3} {:>12.1} {:>9.1}% {:>14}",
            c.shape.name(),
            c.comp,
            pages,
            bytes / 1e9,
            resident / 1e6,
            100.0 * occ,
            fill
        );
    }
    println!("  fill_group_s: the builder's fill_group calls at this many pages, one thread: the compress and the decode-compare of the build, not its read of the raw segment, the filter, the CRCs or the writes");
    println!("  records a page at which the claims fall away: 35.7 (parity with LMDB's reads), 40.1 (1.5x), 39.3 (scan 2x; 43.1 with the heads resident), 45 (the cache is full)");

    if let Some((c, _)) = cells
        .iter()
        .find(|(c, _)| c.shape == Shape::Seq && c.comp == 0.5)
    {
        if !c.sweep.is_empty() {
            println!("\nGROWTH ROUNDS (the suite's shape, first partition, first hint 60)");
            for (r, rpp, fill, calls) in &c.sweep {
                println!("  grow_rounds {r}: {rpp:.2} records a page, fill {fill:.4}, {calls:.3} compress calls a page");
            }
        }
        let rpp = c.keys as f64 / c.pages.len() as f64;
        if rpp < 36.0 {
            println!("\nSTOP: {rpp:.2} records a page on the suite's shape is under 36");
            std::process::exit(2);
        }
    }
}
