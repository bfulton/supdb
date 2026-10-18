//! A probe of the compact-form thesis, outside the engine: the suite's records in 4 KiB pages, raw or as LZ4
//! groups that each fill one page, read at random under a memory bound, with an optional file touched once per
//! read to stand for the engine's resident hash. It measures major faults and the time a read takes.
//!
//! ```text
//! zprobe build DIR N [HASH_MB]                 write DIR/raw.bin, z.bin, their fences and DIR/hash.bin, print sizes
//! zprobe read  DIR N LAYOUT HASH_MB READS PASSES    LAYOUT is raw or z; run it inside the bound (bound.sh)
//! ```
//!
//! `BALLAST_MB` in the environment holds that much anonymous memory, the engine's own beside its cache.
//! The records are fixed-size (a sixteen-digit key and a hundred-byte value, half of it a repeated phrase, the
//! suite's `Payload`), so a read finds its record by binary search inside a page and nothing here stands for a
//! variable-length record, a tombstone, a multi-value key or the unsealed overlay: the probe is the physics of
//! the page cache under a bound, not a model of the engine. Its notes and the results are in `prereg.md`.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::time::Instant;

const PAGE: usize = 4096;
const HDR: usize = 8;
const KEY: usize = 16;
const VAL: usize = 100;
const REC: usize = KEY + VAL;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn key_into(n: u64, buf: &mut [u8]) {
    let mut v = n;
    for b in buf[..KEY].iter_mut().rev() {
        *b = b'0' + (v % 10) as u8;
        v /= 10;
    }
}

/// The suite's `Payload::new(100, 0.5, 0xE1)` pool: half a repeated phrase, half entropy, handed out as
/// windows at random offsets.
fn pool() -> Vec<u8> {
    let target = (8usize << 20).max(VAL * 16);
    let mut rng = Rng::new(0xE1);
    let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
    let mut p = Vec::with_capacity(target + VAL);
    while p.len() < target + VAL {
        let start = p.len();
        for i in 0..VAL / 2 {
            p.push(motif[(start + i) % motif.len()]);
        }
        for _ in VAL / 2..VAL {
            p.push((rng.next() & 0xff) as u8);
        }
    }
    p
}

/// The first rank and the first key of every page, which is all a read needs to find its page.
#[derive(Default)]
struct Fence {
    first_rank: Vec<u32>,
    first_key: Vec<[u8; KEY]>,
}

impl Fence {
    fn push(&mut self, rank: u64, key: &[u8]) {
        self.first_rank.push(rank as u32);
        let mut k = [0u8; KEY];
        k.copy_from_slice(&key[..KEY]);
        self.first_key.push(k);
    }

    fn save(&self, path: &str) {
        let mut b = Vec::new();
        for (r, k) in self.first_rank.iter().zip(self.first_key.iter()) {
            b.extend_from_slice(&r.to_le_bytes());
            b.extend_from_slice(k);
        }
        let mut f = File::create(path).expect("create fence");
        f.write_all(&b).expect("write fence");
        f.sync_all().expect("sync fence");
    }
}

fn drop_cache(path: &str) {
    if let Ok(f) = OpenOptions::new().read(true).open(path) {
        // SAFETY: advice over a descriptor this function owns for the call.
        unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    }
}

fn build(dir: &str, n: u64, hash_mb: usize) {
    let pool = pool();
    let span = pool.len() - VAL;
    let mut raw = File::create(format!("{dir}/raw.bin")).expect("create raw");
    let mut z = File::create(format!("{dir}/z.bin")).expect("create z");
    let mut fence_raw = Fence::default();
    let mut fence_z = Fence::default();
    // The raw layout: as many whole records as a page holds after its header.
    let per_raw = (PAGE - HDR) / REC;
    {
        let mut vr = Rng::new(7);
        let mut page = vec![0u8; PAGE];
        let mut r = 0u64;
        while r < n {
            let c = per_raw.min((n - r) as usize);
            page.fill(0);
            page[0..4].copy_from_slice(&(r as u32).to_le_bytes());
            page[4..6].copy_from_slice(&(c as u16).to_le_bytes());
            let mut first = [0u8; KEY];
            key_into(r, &mut first);
            fence_raw.push(r, &first);
            for i in 0..c {
                let at = HDR + i * REC;
                key_into(r + i as u64, &mut page[at..at + KEY]);
                let off = (vr.next() as usize) % span;
                page[at + KEY..at + REC].copy_from_slice(&pool[off..off + VAL]);
            }
            raw.write_all(&page).expect("write raw");
            r += c as u64;
        }
    }
    // The z layout: groups of records that each compress into one page. A guess at the count is grown
    // while it fits and shrunk while it does not, so a page costs about three compressions.
    let cap = PAGE - HDR;
    let mut vr = Rng::new(7);
    let mut recs: Vec<u8> = Vec::with_capacity(REC * 200);
    let mut cbuf = vec![0u8; lz4_flex::block::get_maximum_output_size(REC * 200)];
    let (mut gen_rank, mut rank_next, mut pages, mut sum_c, mut guess) =
        (0u64, 0u64, 0u64, 0u64, 60usize);
    while rank_next < n {
        let want = (guess + 8).min((n - rank_next) as usize);
        while recs.len() < want * REC {
            let at = recs.len();
            recs.resize(at + REC, 0);
            key_into(gen_rank, &mut recs[at..at + KEY]);
            let off = (vr.next() as usize) % span;
            recs[at + KEY..at + REC].copy_from_slice(&pool[off..off + VAL]);
            gen_rank += 1;
        }
        let avail = recs.len() / REC;
        let mut c = guess.min(avail);
        let mut clen =
            lz4_flex::block::compress_into(&recs[..c * REC], &mut cbuf).expect("compress");
        if clen <= cap {
            while c < avail {
                let more = lz4_flex::block::compress_into(&recs[..(c + 1) * REC], &mut cbuf)
                    .expect("compress");
                if more > cap {
                    clen = lz4_flex::block::compress_into(&recs[..c * REC], &mut cbuf)
                        .expect("compress");
                    break;
                }
                c += 1;
                clen = more;
            }
        } else {
            while c > 1 && clen > cap {
                c -= 1;
                clen =
                    lz4_flex::block::compress_into(&recs[..c * REC], &mut cbuf).expect("compress");
            }
        }
        assert!(clen <= cap, "a single record does not fit a page");
        let mut page = vec![0u8; PAGE];
        page[0..4].copy_from_slice(&(rank_next as u32).to_le_bytes());
        page[4..6].copy_from_slice(&(c as u16).to_le_bytes());
        page[6..8].copy_from_slice(&(clen as u16).to_le_bytes());
        page[HDR..HDR + clen].copy_from_slice(&cbuf[..clen]);
        fence_z.push(rank_next, &recs[..KEY]);
        z.write_all(&page).expect("write z");
        pages += 1;
        sum_c += c as u64;
        guess = c;
        recs.drain(..c * REC);
        rank_next += c as u64;
    }
    raw.sync_all().expect("sync raw");
    z.sync_all().expect("sync z");
    fence_raw.save(&format!("{dir}/raw.fence"));
    fence_z.save(&format!("{dir}/z.fence"));
    // A file that stands for the engine's resident hash.
    if hash_mb > 0 {
        let mut f = File::create(format!("{dir}/hash.bin")).expect("create hash");
        let mut r = Rng::new(99);
        let chunk: Vec<u8> = (0..(1 << 20)).map(|_| (r.next() & 0xff) as u8).collect();
        for _ in 0..hash_mb {
            f.write_all(&chunk).expect("write hash");
        }
        f.sync_all().expect("sync hash");
    }
    for name in ["raw.bin", "z.bin", "hash.bin"] {
        drop_cache(&format!("{dir}/{name}"));
    }
    let raw_pages = fence_raw.first_rank.len();
    println!(
        "built n={n}: raw {raw_pages} pages ({:.1} MB, {:.1} B/key); z {pages} pages ({:.1} MB, {:.1} B/key, {:.1} records/page)",
        (raw_pages * PAGE) as f64 / 1e6,
        (raw_pages * PAGE) as f64 / n as f64,
        (pages as usize * PAGE) as f64 / 1e6,
        (pages as usize * PAGE) as f64 / n as f64,
        sum_c as f64 / pages as f64
    );
}

fn load_fence(dir: &str, name: &str) -> Fence {
    let b = std::fs::read(format!("{dir}/{name}.fence")).expect("read fence");
    let mut f = Fence::default();
    for e in b.chunks_exact(4 + KEY) {
        f.first_rank
            .push(u32::from_le_bytes(e[..4].try_into().expect("four bytes")));
        let mut k = [0u8; KEY];
        k.copy_from_slice(&e[4..]);
        f.first_key.push(k);
    }
    f
}

/// A file mapped read-only with the kernel's readahead off, as the engine maps a segment for point reads.
fn map(path: &str) -> (*const u8, usize) {
    let f = File::open(path).expect("open");
    let len = f.metadata().expect("stat").len() as usize;
    // SAFETY: a private read-only mapping of a file this process does not write; the mapping outlives every use.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        )
    };
    assert!(p != libc::MAP_FAILED, "mmap failed");
    // SAFETY: advice over the mapping just made.
    unsafe { libc::madvise(p, len, libc::MADV_RANDOM) };
    (p as *const u8, len)
}

fn majflt() -> u64 {
    // SAFETY: getrusage fills a zeroed struct of the right type.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    unsafe { libc::getrusage(libc::RUSAGE_THREAD, &mut ru) };
    ru.ru_majflt as u64
}

fn read(dir: &str, n: u64, layout: &str, hash_mb: usize, reads: u64, passes: u32) {
    let fence = load_fence(dir, layout);
    let (base, len) = map(&format!("{dir}/{layout}.bin"));
    let hash = (hash_mb > 0).then(|| map(&format!("{dir}/hash.bin")));
    let mut out = vec![0u8; PAGE * 4];
    // Anonymous memory standing for what the engine holds beside its cache (about 90-130 MB at this rung).
    let ballast_mb: usize = std::env::var("BALLAST_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut ballast = vec![0u8; ballast_mb << 20];
    for i in (0..ballast.len()).step_by(PAGE) {
        ballast[i] = 1;
    }
    let mut rng = Rng::new(0x5EAD);
    let mut sink = 0u64;
    println!(
        "layout {layout} n {n} file {:.1} MB hash {hash_mb} MB ballast {ballast_mb} MB fence {} pages",
        len as f64 / 1e6,
        fence.first_rank.len()
    );
    for pass in 0..passes {
        let f0 = majflt();
        let t = Instant::now();
        for _ in 0..reads {
            let r = rng.next() % n;
            let mut kb = [0u8; KEY];
            key_into(r, &mut kb);
            if let Some((hp, hl)) = hash {
                let off = ((rng.next() as usize) % (hl / PAGE)) * PAGE;
                // SAFETY: the offset is a whole page inside the mapping.
                sink ^= u64::from(unsafe { *hp.add(off) });
            }
            let p = fence.first_key.partition_point(|k| k[..] <= kb[..]) - 1;
            // SAFETY: page `p` is inside the mapping, which is a whole number of pages.
            let page = unsafe { std::slice::from_raw_parts(base.add(p * PAGE), PAGE) };
            let count = u16::from_le_bytes([page[4], page[5]]) as usize;
            let recs: &[u8] = if layout == "z" {
                let clen = u16::from_le_bytes([page[6], page[7]]) as usize;
                let got = lz4_flex::block::decompress_into(
                    &page[HDR..HDR + clen],
                    &mut out[..count * REC],
                )
                .expect("decompress");
                assert_eq!(got, count * REC);
                &out[..count * REC]
            } else {
                &page[HDR..HDR + count * REC]
            };
            // Binary search over the page's fixed records.
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if recs[mid * REC..mid * REC + KEY] < kb[..] {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            assert!(
                lo < count && recs[lo * REC..lo * REC + KEY] == kb[..],
                "key {r} not found"
            );
            sink ^= u64::from(recs[lo * REC + KEY]);
        }
        let secs = t.elapsed().as_secs_f64();
        let faults = majflt() - f0;
        println!(
            "pass {pass}: {:>9.0} reads/s  {:.3} us/read  {:.3} major faults/read",
            reads as f64 / secs,
            secs * 1e6 / reads as f64,
            faults as f64 / reads as f64
        );
    }
    std::hint::black_box((sink, &ballast));
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let num = |i: usize| -> u64 {
        a.get(i)
            .and_then(|s| s.parse().ok())
            .expect("a number argument")
    };
    match a.get(1).map(String::as_str) {
        Some("build") => build(
            &a[2],
            num(3),
            a.get(4).map_or(0, |s| s.parse().expect("HASH_MB")),
        ),
        Some("read") => read(&a[2], num(3), &a[4], num(5) as usize, num(6), num(7) as u32),
        _ => eprintln!(
            "usage: zprobe build DIR N [HASH_MB] | zprobe read DIR N raw|z HASH_MB READS PASSES"
        ),
    }
}
