//! What a scan's walk of a copied block costs by the block's layout, with no
//! engine in it: the block cache holds a block dense with unsealed keys as
//! one merged copy in three buffers -- sixteen bytes of entry a record,
//! the keys, and the values each behind a length -- and a scan of fifty
//! from a key somewhere in the store starts three streams in a block it
//! has not touched lately and three more in the next. This walks the same
//! records laid out that way and as one buffer of fixed-size records, key
//! and value side by side, which a block can be where its keys are of one
//! length and each has one value of one length.
//!
//! The walk is the engine's in shape and no more: a table of blocks, a
//! start at a random entry of a random block, `len` records to a callback
//! that copies the value into a sink as the suite's harness does, into the
//! next block where the first runs out, with the asks the engine makes
//! ahead of each block or none. Blocks are allocated in a shuffled order,
//! so neither layout's blocks lie in the table's order in memory.
//!
//!     blockwalk [--blocks N] [--scans N] [--len N] [--rounds N]
//!
//! Prints nanoseconds a scan, the median of the rounds, for each layout
//! and ask; `--blocks` sets the working set (sixty-four records of 116
//! bytes a block: 500 is 4 MB, 5,000 is 40 MB, 50,000 is 400 MB).
//! A scan walks its start block and the one after it and no third, so a
//! `--len` over sixty-four is cut short where it starts late in a block,
//! alike for every arm.
use std::hint::black_box;
use std::time::Instant;

const BLOCK: usize = 64;
const KEY: usize = 16;
const VALUE: usize = 100;
const RUN: usize = 4 + VALUE;
const STRIDE: usize = KEY + VALUE;

/// The copy as the block cache holds it: an entry a record naming its key
/// and its run, the keys, and the runs, each value behind its length.
struct Soa {
    ents: Vec<[u32; 4]>,
    keys: Vec<u8>,
    vals: Vec<u8>,
}

/// The same records as one buffer, key then value, at one stride.
struct Aos {
    recs: Vec<u8>,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[inline]
fn lines(ptr: *const u8, bytes: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
        let lead = ptr as usize & 63;
        let base = ptr.wrapping_sub(lead);
        let mut off = 0usize;
        while off < bytes + lead {
            // SAFETY: a prefetch faults on no address.
            unsafe { _mm_prefetch(base.wrapping_add(off) as *const i8, _MM_HINT_T0) };
            off += 64;
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (ptr, bytes);
}

/// The asks ahead of a block's walk.
#[derive(Clone, Copy, PartialEq)]
enum Ask {
    /// None: the walk's own loads and the hardware's prefetcher.
    None,
    /// The engine's: every entry and key of the block and the first
    /// kilobyte of its values, wherever the walk starts.
    Whole,
    /// The lines the walk reads: from its first entry, as many as it takes.
    Span,
}

impl Soa {
    fn ask(&self, ask: Ask, i: usize, n: usize) {
        match ask {
            Ask::None => {}
            Ask::Whole => {
                lines(self.ents.as_ptr() as *const u8, self.ents.len() * 16);
                lines(self.keys.as_ptr(), self.keys.len());
                lines(self.vals.as_ptr(), self.vals.len().min(1024));
            }
            Ask::Span => {
                let m = n.min(BLOCK - i);
                lines(self.ents[i..].as_ptr() as *const u8, m * 16);
                lines(self.keys[i * KEY..].as_ptr(), m * KEY);
                lines(self.vals[i * RUN..].as_ptr(), (m * RUN).min(1024));
            }
        }
    }

    #[inline]
    fn walk(&self, i: usize, n: usize, f: &mut impl FnMut(&[u8], &[u8])) -> usize {
        let mut seen = 0;
        for e in &self.ents[i..] {
            if seen >= n {
                break;
            }
            let k = &self.keys[e[0] as usize..(e[0] + e[1]) as usize];
            let run = &self.vals[e[2] as usize..(e[2] + e[3]) as usize];
            let mut p = 0usize;
            while p < run.len() {
                let l = u32::from_le_bytes(run[p..p + 4].try_into().unwrap()) as usize;
                f(k, &run[p + 4..p + 4 + l]);
                p += 4 + l;
            }
            seen += 1;
        }
        seen
    }
}

impl Aos {
    fn ask(&self, ask: Ask, i: usize, n: usize) {
        match ask {
            Ask::None => {}
            // The same count of lines as the three buffers' whole ask, from
            // the block's first record: what "whole" is of one buffer.
            Ask::Whole => lines(self.recs.as_ptr(), 48 * 64),
            Ask::Span => {
                let m = n.min(BLOCK - i);
                lines(
                    self.recs[i * STRIDE..].as_ptr(),
                    (m * STRIDE).min(1024 + 512),
                );
            }
        }
    }

    #[inline]
    fn walk(&self, i: usize, n: usize, f: &mut impl FnMut(&[u8], &[u8])) -> usize {
        let mut seen = 0;
        for r in self.recs[i * STRIDE..].as_chunks::<STRIDE>().0 {
            if seen >= n {
                break;
            }
            f(&r[..KEY], &r[KEY..]);
            seen += 1;
        }
        seen
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str, d: usize| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let blocks = arg("--blocks", 5_000);
    let scans = arg("--scans", 200_000);
    let len = arg("--len", 50);
    let rounds = arg("--rounds", 7);

    // The blocks' contents, and an order to allocate them in that is not
    // the table's.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut order: Vec<usize> = (0..blocks).collect();
    for i in (1..blocks).rev() {
        order.swap(i, rng.below(i + 1));
    }
    let mut soa: Vec<Option<Box<Soa>>> = (0..blocks).map(|_| None).collect();
    let mut aos: Vec<Option<Box<Aos>>> = (0..blocks).map(|_| None).collect();
    for &b in &order {
        let mut s = Soa {
            ents: Vec::with_capacity(BLOCK),
            keys: Vec::with_capacity(BLOCK * KEY),
            vals: Vec::with_capacity(BLOCK * RUN),
        };
        let mut a = Aos {
            recs: Vec::with_capacity(BLOCK * STRIDE),
        };
        for j in 0..BLOCK {
            let key = format!("{:016}", b * BLOCK + j);
            let mut value = [0u8; VALUE];
            for (x, v) in value.iter_mut().enumerate() {
                *v = (rng.next() >> (x % 8)) as u8;
            }
            s.ents
                .push([(j * KEY) as u32, KEY as u32, (j * RUN) as u32, RUN as u32]);
            s.keys.extend_from_slice(key.as_bytes());
            s.vals.extend_from_slice(&(VALUE as u32).to_le_bytes());
            s.vals.extend_from_slice(&value);
            a.recs.extend_from_slice(key.as_bytes());
            a.recs.extend_from_slice(&value);
        }
        soa[b] = Some(Box::new(s));
        aos[b] = Some(Box::new(a));
    }
    let soa: Vec<Box<Soa>> = soa.into_iter().map(|b| b.unwrap()).collect();
    let aos: Vec<Box<Aos>> = aos.into_iter().map(|b| b.unwrap()).collect();

    // The scans' starts, the same for every arm of a round.
    let starts: Vec<(u32, u8)> = (0..scans)
        .map(|_| (rng.below(blocks - 1) as u32, rng.below(BLOCK) as u8))
        .collect();

    let mut sink = [0u8; VALUE];
    let mut sum = 0u64;
    let arms: [(&str, bool, Ask); 6] = [
        ("three buffers, no ask", false, Ask::None),
        ("three buffers, whole ", false, Ask::Whole),
        ("three buffers, span  ", false, Ask::Span),
        ("one buffer,    no ask", true, Ask::None),
        ("one buffer,    whole ", true, Ask::Whole),
        ("one buffer,    span  ", true, Ask::Span),
    ];
    let mut times: Vec<Vec<f64>> = arms.iter().map(|_| Vec::new()).collect();
    for round in 0..rounds {
        // The arms in an order that turns with the round, so none stands
        // first more often.
        for k in 0..arms.len() {
            let a = (k + round) % arms.len();
            let (_, one, ask) = arms[a];
            let t = Instant::now();
            for &(b, i) in &starts {
                let (b, i) = (b as usize, i as usize);
                let mut f = |k: &[u8], v: &[u8]| {
                    sink.copy_from_slice(v);
                    sum = sum.wrapping_add(u64::from(k[KEY - 1]) + u64::from(sink[0]));
                };
                let first = len.min(BLOCK - i);
                if one {
                    let blk = &aos[b];
                    blk.ask(ask, i, len);
                    if first < len {
                        aos[b + 1].ask(ask, 0, len - first);
                    }
                    let seen = blk.walk(i, len, &mut f);
                    if seen < len {
                        aos[b + 1].walk(0, len - seen, &mut f);
                    }
                } else {
                    let blk = &soa[b];
                    blk.ask(ask, i, len);
                    if first < len {
                        soa[b + 1].ask(ask, 0, len - first);
                    }
                    let seen = blk.walk(i, len, &mut f);
                    if seen < len {
                        soa[b + 1].walk(0, len - seen, &mut f);
                    }
                }
            }
            times[a].push(t.elapsed().as_secs_f64() * 1e9 / scans as f64);
        }
    }
    black_box(sum);
    println!(
        "{blocks} blocks ({} MB of records), {scans} scans of {len}, median of {rounds} rounds:",
        (blocks * BLOCK * STRIDE) >> 20
    );
    for (a, (name, _, _)) in arms.iter().enumerate() {
        let m = median(&mut times[a]);
        println!(
            "  {name}  {m:7.1} ns a scan  {:5.2} ns an entry",
            m / len as f64
        );
    }
}
