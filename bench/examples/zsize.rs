//! What the z form's layout makes of the suite's records: the real `fill_page`
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
//! `fill_page` a page at a time, each page cut into groups at the engine's
//! target (`ZGROUP_BYTES`), and every page it builds goes through
//! `verify_page`, the check a reader's first decode makes.
//!
//! Printed: the size of what was built (records, groups and bytes a page,
//! fill, compress calls a page); the time of what a read does with a page,
//! on this machine and hot unless the column says cold (the decode of one
//! group, the walk that finds a key in it, the walk that checks it, the
//! page's CRC, the whole first decode of a page) and what `fill_page` itself
//! takes a page; and the occupancy of the cache the bound leaves at ten
//! million keys, with the time `fill_page` takes at that size.
//! Pages are built from the records and nothing else, so what is printed is a
//! function of the images, the layout and the codec: the codec is printed, and
//! the digest of the pages is, so that a build with the other codec can be held
//! to the same bytes.
//!
//! Exit status 2 if the suite's shape (sequential keys, half-compressible
//! values) comes out under `STOP_RPP` records a page, counted over the pages a
//! partition builds once it is under way: not its first, which has no
//! predecessor to size from, and not its last, which holds what is left. 4 if
//! the run has no such cell, or no such pages, and so did not evaluate the rule
//! (it says so, and a run that exits 0 did evaluate it and it held). 3 if the
//! codec that ran is not the one the build says it linked, or its encoder
//! makes other bytes than the sizes were taken with.
use std::collections::HashSet;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::db::zpages::{
    bind32, decode_group, fill_page, filter_bytes, group_seek, lz4_decoder, lz4_encoder_pin,
    page_crc, parse_page, verify_page, walk_group, FenceEntry, Fill, FillScratch, FillStats,
    Filled, GroupBuf, Images, Lz4Decoder, SegImages, LZ4_ENCODER_PIN, ZCAP, ZPAGE, ZTRAILER,
};
use supdb::flatindex::{self, record_key, record_key_next, FlatIndex};
use supdb::{Blob, Bytes, Db, MmapBytes, Options};
use supdb_bench::workload::{db_key_into, Payload, Rng};

/// The page cache the cell's 1 GiB bound left, in bytes: the limit less the
/// process's own anonymous memory, as the suite's out-of-core rung printed it
/// (`docs/engine.md`, "Out of core": 0.94 GB of cache at ten million keys).
/// The occupancy printed is a model against this figure and not a
/// measurement of any cache; a different bound, or a different resident set
/// in the process, moves it.
const CACHE_B: f64 = 937e6;
/// Records a page on the suite's shape under which the form cannot beat the
/// comparator the cell is read against, and the example exits 2.
///
/// The comparator is LMDB with readahead off, the fastest the cell is read
/// (`docs/engine.md`, "Out of core": 82 thousand reads a second on one
/// thread, 12.2 us a read). A read of the form costs its work in memory,
/// 4.0 us as estimated when the threshold was set (the raw read's fixed
/// 0.7 us, a page's decode 2.8, the walk to the key 0.5), and a major fault,
/// 41.5 us (a read's time less its in-memory work, over its faults, for the
/// two arms there: 40 and 43), for the share `phi` of reads whose page is not
/// resident: `4.0 + 41.5 phi <= 12.2` gives `phi <= 0.198`. A read is uniform
/// over the pages, so `phi = 1 - (cache - resident part) / pages` and the
/// pages may fill `(937 - 17) MB / (1 - 0.198)` = 1.147 GB: 280 thousand
/// pages of ten million keys, 35.7 records a page, rounded up here to 36. The
/// estimate of 4.0 us is the one the rule was registered with and is not
/// replaced by the decode timed below: the threshold that stops the work is
/// not moved by the work's own measurements.
const STOP_RPP: f64 = 36.0;
/// The cell's key count.
const CELL_KEYS: f64 = 10_000_000.0;
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
    groups: Vec<Filled>,
    /// Records and pages over the pages a partition builds once it is under
    /// way: all but its first, whose call has no predecessor to size from
    /// and holds about half a page, and its last, which holds what is left.
    /// The stop rule is evaluated over these; the table prints all pages.
    steady: (u64, u64),
    declined: u64,
    stats: FillStats,
    /// Nanoseconds spent inside `fill_page`: the compress calls, and the
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

/// A work directory, removed when it is dropped.
struct Work(PathBuf);

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
/// encoder, then run through `fill_page`; the cell's numbers.
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
        steady: (0, 0),
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
        let mut rank = 0usize;
        let mut counts: Vec<usize> = Vec::new();
        while rank < n {
            let mut page = [0u8; ZPAGE];
            let t = Instant::now();
            let made = fill_page(&images, rank, bind(), &mut scratch, &mut page);
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
                    counts.push(g.count);
                    rank += g.count;
                }
                Fill::NoPage(_) => {
                    cell.declined += 1;
                    rank += 1;
                }
                other => panic!("partition {pi} rank {rank}: {other:?}"),
            }
        }
        // The first page of a partition is built with no hint and the last
        // holds what is left, so a count over every page of a run of a few
        // pages reads far under the steady state, and a stop rule evaluated
        // over it stops a run that has not yet got going.
        if counts.len() > 2 {
            let mid = &counts[1..counts.len() - 1];
            cell.steady.0 += mid.iter().sum::<usize>() as u64;
            cell.steady.1 += mid.len() as u64;
        }
        // The one-group shape's knob, over the first partition.
        if sweep && pi == 0 {
            for rounds in 0..=3u32 {
                let mut s = FillScratch::new();
                s.group_bytes = 0;
                s.grow_rounds = rounds;
                s.hint = 60;
                let mut rank = 0usize;
                while rank < n {
                    let mut page = [0u8; ZPAGE];
                    match fill_page(&images, rank, bind(), &mut s, &mut page) {
                        Fill::Page(g) => rank += g.count,
                        Fill::NoPage(_) => rank += 1,
                        other => panic!("sweep rank {rank}: {other:?}"),
                    }
                }
                let p = s.stats.pages as f64;
                cell.sweep.push((
                    rounds,
                    s.stats.records as f64 / p,
                    s.stats.used_bytes as f64 / (p * ZCAP as f64),
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
///
/// From the line that holds the page's first byte to the line that holds its
/// last: the pages sit in a `Vec<[u8; ZPAGE]>`, whose alignment is a byte's,
/// so a page is not promised to begin on a line, and one that does not spans
/// a line more than its size, which a flush of sixty-four chunks from its
/// start leaves hot.
fn evict(page: &[u8; ZPAGE]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{_mm_clflush, _mm_mfence};
        let first = page.as_ptr();
        let end = first.wrapping_add(page.len());
        let mut line = first.wrapping_sub(first as usize % 64);
        while line < end {
            // SAFETY: a flush of the line that holds a byte of a live slice;
            // a line lies within one page of the address space, so the bytes
            // of it that are a neighbour's are mapped and only made cold.
            unsafe { _mm_clflush(line) };
            line = line.wrapping_add(64);
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
    stored_groups: usize,
    groups_sampled: usize,
}

fn time_cell(cell: &Cell, rounds: usize) -> Times {
    let total = cell.pages.len();
    let take = SAMPLE.min(total);
    let picks: Vec<usize> = (0..take).map(|i| i * total / take).collect();
    let high = [0xFFu8; 64];
    let mut rows: Vec<[f64; 7]> = Vec::new();
    let (mut stored_groups, mut groups_sampled) = (0, 0);
    for _ in 0..rounds {
        let (mut decode, mut cold, mut find, mut walk, mut parse, mut crc, mut verify) =
            (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut buf = GroupBuf::new();
        let mut vbuf = GroupBuf::new();
        let mut cold_ok = true;
        stored_groups = 0;
        groups_sampled = 0;
        for &pi in &picks {
            let page = &cell.pages[pi];
            let view = parse_page(page, bind()).expect("a page we built");
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
            // Each group of the page on its own: the decode a point read
            // pays, the walks over it.
            for j in 0..view.groups() {
                let g = view.group(j).expect("a group of the page");
                groups_sampled += 1;
                if g.stored {
                    stored_groups += 1;
                } else {
                    decode.push(ns_of(100, || {
                        let _ = black_box(decode_group(&view, j, &mut buf).map(|g| g.len()));
                    }));
                    cold_ok &= evict(page);
                    let t = Instant::now();
                    black_box(decode_group(&view, j, &mut buf).map(|g| g.len()).ok());
                    cold.push(t.elapsed().as_nanos() as f64);
                }
                let group = decode_group(&view, j, &mut buf).expect("decodes");
                let n = usize::from(g.count);
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
                            black_box(group_seek(black_box(group), n, k).is_ok());
                        }
                    }) / n as f64,
                );
                walk.push(ns_of(100, || {
                    black_box(group_seek(black_box(group), n, &high).is_ok());
                }));
                parse.push(ns_of(100, || {
                    black_box(walk_group(black_box(group), n, None, None).is_ok());
                }));
            }
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
        stored_groups,
        groups_sampled,
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
    let filter = filter_bytes(CELL_KEYS as usize) as f64;
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
    // The encoder this build links, by what it makes of the input the sizes
    // were pinned on. The suite is its own workspace with its own lock, so
    // the engine's tests say nothing of the codec this binary carries.
    let pin = lz4_encoder_pin();
    println!("encoder pin: {pin:?} (bytes in, bytes out, crc of them); the pin the sizes were taken with: {LZ4_ENCODER_PIN:?}");
    if pin != LZ4_ENCODER_PIN {
        eprintln!(
            "the encoder this build links makes other bytes than the sizes were taken with: \
             what is printed below is not comparable with them until the sizing is taken again \
             and the pin (supdb::db::zpages::LZ4_ENCODER_PIN) moved"
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
            // Named so that the cleanup the repository documents
            // (`supdb-next-*` under the temp directory) takes it, and removed
            // when it goes out of scope, a failed assertion mid-cell included:
            // a store of tens of megabytes left on a disk with a fixed
            // allowance is what a check that fails must not cost.
            let work = Work(dir.join(format!(
                "supdb-next-zsize-{}-{}-{comp}",
                std::process::id(),
                shape.name()
            )));
            let segs = load(&work.0, &ks, &payload);
            let sweep = shape == Shape::Seq && (comp - 0.5).abs() < 1e-9;
            let cell = analyze(&segs, keys, shape, comp, sweep);
            drop(work);
            let times = timing.then(|| time_cell(&cell, rounds));
            eprintln!("done: {} comp {comp}", shape.name());
            cells.push((cell, times));
        }
    }

    println!(
        "SIZE (what fill_page built; every page decoded group by group, compared and verified)"
    );
    println!(
        "{:<13} {:>4} {:>6} {:>5} {:>7} {:>8} {:>6} {:>8} {:>7} {:>6} {:>7} {:>6} {:>7} {:>6} {:>7} {:>18}",
        "shape", "comp", "parts", "rec_B", "pages", "rec/page", "grp/pg", "grpB/pg", "payB/pg", "B/key", "fill", "stored", "nopage", "calls", "turns", "digest"
    );
    for (c, _) in &cells {
        let p = c.pages.len() as f64;
        println!(
            "{:<13} {:>4} {:>6} {:>5.1} {:>7} {:>8.2} {:>6.2} {:>8.1} {:>7.1} {:>6.2} {:>7.4} {:>5.1}% {:>7} {:>6.3} {:>7.3} {:>18x}",
            c.shape.name(),
            c.comp,
            c.partitions,
            c.image_bytes as f64 / c.keys as f64,
            c.pages.len(),
            c.keys as f64 / p,
            c.stats.groups as f64 / p,
            c.stats.group_bytes as f64 / p,
            c.stats.payload_bytes as f64 / p,
            ZPAGE as f64 * p / c.keys as f64,
            c.stats.used_bytes as f64 / (p * ZCAP as f64),
            100.0 * c.stats.stored as f64 / c.stats.groups.max(1) as f64,
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
            "\nTIME, microseconds a group (crc and verify a page), medians over sampled pages' groups and rounds (hot unless cold)"
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
            "fill_page",
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
                t.stored_groups,
                t.groups_sampled,
            );
        }
        println!("  decode: one group's LZ4 block into the buffer, what a point read pays (stored groups are lent in place and not in the median); cold: the page's lines flushed first, the buffer it decodes into warm,");
        println!("  one call timed by a pair of clock reads that is in the figure; the fault that brings a page in is not, so cold is the decode's share of a first read, a quantity of its own and not a second reading of the hot column;");
        println!("  find: the walk to one key, averaged over the group's keys; walk: the walk past every record of the group; parse: the structural walk (parse_record);");
        println!("  crc: the page's checksum; verify: a reader's whole first decode of a page, every group; stored/n: stored groups over the groups sampled;");
        println!("  fill_page: the builder's call for the page, its compress calls and the decode and compare of the page it made, once, in the load's pass");
    }

    println!(
        "\nOCCUPANCY at {CELL_KEYS:.0} keys, against {:.0} MB of cache",
        CACHE_B / 1e6
    );
    println!(
        "{:<13} {:>4} {:>11} {:>10} {:>12} {:>10} {:>14}",
        "shape", "comp", "pages", "pages_GB", "resident_MB", "occupancy", "fill_page_s"
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
    println!("  fill_page_s: the builder's fill_page calls at this many pages, one thread: the compress and the decode-compare of the build, not its read of the raw segment, the filter, the CRCs or the writes");

    let suite = cells
        .iter()
        .find(|(c, _)| c.shape == Shape::Seq && c.comp == 0.5)
        .map(|(c, _)| c);
    if let Some(c) = suite {
        if !c.sweep.is_empty() {
            println!("\nGROWTH ROUNDS (the suite's shape, first partition, the page as one group, first hint 60)");
            for (r, rpp, fill, calls) in &c.sweep {
                println!("  grow_rounds {r}: {rpp:.2} records a page, fill {fill:.4}, {calls:.3} compress calls a page");
            }
        }
    }
    // The stop rule says what it did on every run: a gate that speaks only
    // when it fires cannot be told from one that did not run.
    let unevaluated = |why: &str| -> ! {
        println!("\nSTOP RULE: not evaluated: {why}");
        eprintln!("the stop rule was not evaluated: {why}");
        std::process::exit(4);
    };
    let Some(c) = suite else {
        unevaluated("the suite's shape (seq-dec16 at compressibility 0.5) is not in this run")
    };
    let (records, pages) = c.steady;
    if pages == 0 {
        unevaluated(
            "the suite's shape built no page between a partition's first and last, so there is \
             no steady state to count (more keys)",
        );
    }
    let rpp = records as f64 / pages as f64;
    if rpp < STOP_RPP {
        println!("\nSTOP: {rpp:.2} records a page on the suite's shape over {pages} page(s) of its steady state is under {STOP_RPP}");
        std::process::exit(2);
    }
    println!("\nstop rule held: {rpp:.2} records a page on the suite's shape over {pages} page(s) of its steady state, not under {STOP_RPP}");
}
