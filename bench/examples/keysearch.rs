//! What a point read costs where its rank is searched for and not computed:
//! the read path of a stride form whose keys are no run of numbers.
//!
//! The stride form holds a partition's values at their stride and finds a
//! key's rank by arithmetic, which needs the keys to be every number of a
//! run. Keys that are numbers with gaps between them -- random ids -- could
//! be held the same way with their numbers beside the values, eight bytes a
//! key in rank order, and a read that searches them: a sample of every
//! `--stride`th number, small enough to stay in the core's cache, then the
//! run of numbers between two samples, then the value. This probe prices
//! that read against the arithmetic one over the same values in the same
//! memory, in core, before anything of the form is built: `--keys` values
//! of `--width` bytes in one buffer, cut into `--parts` partitions as a
//! store is, and `--reads` reads of each shape, each copying its value out
//! as the suite's reads do (`consume`).
//!
//! Four reads, their order alternating round by round: `dense`, the rank
//! by arithmetic; `samples`, the rank by a binary search of the samples and
//! of one run, the run's lines asked for together; `buckets`, the rank
//! by a table of where each of a partition's equal spans of numbers begins,
//! `--bucket` keys a span, and a search of the one span, which for ids
//! spread evenly is a line or two and for any others is still exact; and
//! `records`, the same table over records of a number and its value laid
//! together at one stride, where the span searched is the page the value
//! is on, so the read waits on the table and on one place more; and
//! `guess`, the samples again, searched from the place a straight line
//! through the partition's first and last number puts the number at and no
//! further from it than the line is ever wrong for that partition, which
//! the build measures -- a few samples for ids spread evenly, and never
//! more than the binary search for any others. The
//! numbers the reads search for are made before the clock, since the
//! arithmetic read parses its key too. Each read's draw is made to wait on
//! the value before it, so the reads are in series as an engine's are: a
//! loop of independent reads overlaps ten of them and prices none.
//! `--free 1` drops the wait, for that figure. Uniform draws and the suite's
//! Zipfian, whose head is one partition. The figure is nanoseconds a read
//! at the median round.
//!
//!     cargo run --release --example keysearch -- [--keys N] [--width W]
//!         [--parts P] [--stride S] [--bucket B] [--reads N] [--rounds N] [--free 1]
use std::time::Instant;
use supdb_bench::workload::{Ids, KeyDist, KeyGen};

fn arg(name: &str, default: u64) -> u64 {
    let a: Vec<String> = std::env::args().collect();
    a.iter()
        .position(|x| x == name)
        .and_then(|i| a.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One partition's numbers: where its ranks begin, every `stride`th number
/// of it, and where each of its equal spans of numbers begins.
struct Part {
    first: usize,
    len: usize,
    samples: Vec<u64>,
    /// The most samples the straight line through the partition's first
    /// and last sample is wrong by, for any number of the partition.
    eps: usize,
    hi: u64,
    lo: u64,
    shift: u32,
    /// `table[b]` is the partition's first rank whose number is at or past
    /// `lo + (b << shift)`; one entry past the last span.
    table: Vec<u32>,
}

impl Part {
    /// Where the straight line from `(lo, 0)` to `(hi, m)` puts `x`.
    #[inline]
    fn line(lo: u64, hi: u64, m: usize, x: u64) -> usize {
        let span = (hi - lo).max(1);
        (((x - lo) as u128 * m as u128) / span as u128) as usize
    }
}

struct Store {
    values: Vec<u8>,
    width: usize,
    numbers: Vec<u64>,
    /// Rank `r`'s number and its value together, at `r * (8 + width)`.
    records: Vec<u8>,
    /// Each partition's first number, for the route a read takes to its
    /// partition in every form.
    fences: Vec<u64>,
    parts: Vec<Part>,
    stride: usize,
}

impl Store {
    fn new(keys: usize, width: usize, nparts: usize, stride: usize, bucket: usize) -> Store {
        let ids = Ids::new(keys as u64);
        let numbers: Vec<u64> = (0..keys as u64).map(|i| ids.of(i)).collect();
        assert!(numbers.windows(2).all(|w| w[0] < w[1]), "the ids ascend");
        let mut values = vec![0u8; keys * width];
        // Every page touched and every value its rank's, so a read that
        // lands on the wrong rank is caught by the sum. Bytes 8..16 stay
        // zero: the word a read's draw waits on.
        for (r, v) in values.chunks_exact_mut(width).enumerate() {
            v[..8].copy_from_slice(&(r as u64).to_le_bytes());
            v[width - 1] = r as u8;
        }
        let mut records = vec![0u8; keys * (8 + width)];
        for (r, rec) in records.chunks_exact_mut(8 + width).enumerate() {
            rec[..8].copy_from_slice(&numbers[r].to_le_bytes());
            rec[8..].copy_from_slice(&values[r * width..(r + 1) * width]);
        }
        let per = keys.div_ceil(nparts);
        let mut parts = Vec::new();
        let mut fences = Vec::new();
        let mut first = 0;
        while first < keys {
            let len = per.min(keys - first);
            let run = &numbers[first..first + len];
            let (lo, hi) = (run[0], run[len - 1]);
            // The widest spans that leave `bucket` keys a span or fewer on
            // average: a shift, so a number's span is a subtract and a
            // shift and no divide.
            let want = (len / bucket).max(1) as u64;
            let mut shift = 0u32;
            while ((hi - lo) >> shift) + 1 > want {
                shift += 1;
            }
            let spans = (((hi - lo) >> shift) + 1) as usize;
            let mut table = Vec::with_capacity(spans + 1);
            let mut r = 0usize;
            for b in 0..spans as u64 {
                let start = lo + (b << shift);
                while r < len && run[r] < start {
                    r += 1;
                }
                table.push(r as u32);
            }
            table.push(len as u32);
            fences.push(lo);
            let samples: Vec<u64> = run.iter().step_by(stride).copied().collect();
            // The line's error, measured at every sample: a number between
            // two samples is guessed between their guesses, so the samples
            // bound it to one more.
            let m = samples.len();
            let eps = samples
                .iter()
                .enumerate()
                .map(|(i, &v)| Part::line(lo, hi, m, v).abs_diff(i))
                .max()
                .unwrap_or(0)
                + 1;
            parts.push(Part {
                first,
                len,
                samples,
                eps,
                hi,
                lo,
                shift,
                table,
            });
            first += len;
        }
        Store {
            values,
            width,
            numbers,
            records,
            fences,
            parts,
            stride,
        }
    }

    #[inline]
    fn value(&self, rank: usize) -> &[u8] {
        &self.values[rank * self.width..(rank + 1) * self.width]
    }

    #[inline]
    fn part_of(&self, x: u64) -> Option<&Part> {
        let p = self.fences.partition_point(|&f| f <= x).checked_sub(1)?;
        Some(&self.parts[p])
    }

    #[inline]
    fn ask(run: &[u64]) {
        #[cfg(target_arch = "x86_64")]
        // The run's lines, asked for together: one wait for them all.
        unsafe {
            use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
            let base = run.as_ptr() as *const i8;
            let mut off = 0;
            while off < run.len() * 8 {
                _mm_prefetch(base.add(off), _MM_HINT_T0);
                off += 64;
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = run;
    }

    /// The rank of `x` by the samples of its partition and the run between
    /// two of them.
    #[inline]
    fn rank_by_samples(&self, x: u64) -> Option<usize> {
        let part = self.part_of(x)?;
        let s = part.samples.partition_point(|&v| v <= x).checked_sub(1)?;
        let lo = part.first + s * self.stride;
        let hi = (lo + self.stride).min(part.first + part.len);
        let run = &self.numbers[lo..hi];
        Store::ask(run);
        let i = run.partition_point(|&v| v < x);
        (i < run.len() && run[i] == x).then_some(lo + i)
    }

    /// The rank of `x` by the samples within the line's error of where the
    /// line puts it, and the run between two of them.
    #[inline]
    fn rank_by_guess(&self, x: u64) -> Option<usize> {
        let part = self.part_of(x)?;
        let m = part.samples.len();
        let g = Part::line(part.lo, part.hi, m, x.min(part.hi));
        let (a, z) = (g.saturating_sub(part.eps), (g + part.eps + 1).min(m));
        let s = (a + part.samples[a..z].partition_point(|&v| v <= x)).checked_sub(1)?;
        let lo = part.first + s * self.stride;
        let hi = (lo + self.stride).min(part.first + part.len);
        let run = &self.numbers[lo..hi];
        Store::ask(run);
        let i = run.partition_point(|&v| v < x);
        (i < run.len() && run[i] == x).then_some(lo + i)
    }

    /// The rank of `x` by the table of its partition's spans and the one
    /// span it is in.
    #[inline]
    fn rank_by_buckets(&self, x: u64) -> Option<usize> {
        let part = self.part_of(x)?;
        let b = ((x - part.lo) >> part.shift) as usize;
        let (lo, hi) = (
            *part.table.get(b)? as usize,
            *part.table.get(b + 1)? as usize,
        );
        let run = &self.numbers[part.first + lo..part.first + hi];
        Store::ask(run);
        let i = run.partition_point(|&v| v < x);
        (i < run.len() && run[i] == x).then_some(part.first + lo + i)
    }

    #[inline]
    fn record_number(&self, rank: usize) -> u64 {
        let at = rank * (8 + self.width);
        u64::from_le_bytes(self.records[at..at + 8].try_into().unwrap())
    }

    /// `x`'s value by the table of its partition's spans and the records
    /// of the one span it is in, each record's first line asked for
    /// together.
    #[inline]
    fn value_by_records(&self, x: u64) -> Option<&[u8]> {
        let part = self.part_of(x)?;
        let b = ((x - part.lo) >> part.shift) as usize;
        let (lo, hi) = (
            *part.table.get(b)? as usize,
            *part.table.get(b + 1)? as usize,
        );
        let (lo, hi) = (part.first + lo, part.first + hi);
        let rec = 8 + self.width;
        #[cfg(target_arch = "x86_64")]
        unsafe {
            use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
            let base = self.records.as_ptr() as *const i8;
            for r in lo..hi {
                _mm_prefetch(base.add(r * rec), _MM_HINT_T0);
            }
        }
        let (mut a, mut z) = (lo, hi);
        while a < z {
            let m = a + (z - a) / 2;
            if self.record_number(m) < x {
                a = m + 1;
            } else {
                z = m;
            }
        }
        (a < hi && self.record_number(a) == x).then(|| &self.records[a * rec + 8..(a + 1) * rec])
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Read {
    Dense,
    Samples,
    Buckets,
    Records,
    Guess,
}

/// `draws` read one after another, `xs` their numbers; with `serial` each
/// read's draw waits on a word of the value before it, which is zero.
fn pass(
    store: &Store,
    draws: &[u64],
    xs: &[u64],
    read: Read,
    serial: bool,
    sink: &mut [u8],
) -> (f64, u64) {
    let t = Instant::now();
    let mut sum = 0u64;
    let mut wait = 0u64;
    for (&d, &x) in draws.iter().zip(xs) {
        let v = match read {
            Read::Dense => store.value((d + wait) as usize),
            Read::Samples => store.value(store.rank_by_samples(x + wait).expect("a loaded key")),
            Read::Buckets => store.value(store.rank_by_buckets(x + wait).expect("a loaded key")),
            Read::Records => store.value_by_records(x + wait).expect("a loaded key"),
            Read::Guess => store.value(store.rank_by_guess(x + wait).expect("a loaded key")),
        };
        sink[..v.len()].copy_from_slice(v);
        sum = sum.wrapping_add(u64::from_le_bytes(sink[..8].try_into().unwrap()));
        if serial {
            wait = u64::from_le_bytes(v[8..16].try_into().unwrap());
        }
    }
    (t.elapsed().as_nanos() as f64 / draws.len() as f64, sum)
}

fn main() {
    let keys = arg("--keys", 10_000_000) as usize;
    let width = (arg("--width", 100) as usize).max(16);
    let nparts = arg("--parts", 29) as usize;
    let stride = arg("--stride", 64) as usize;
    let bucket = arg("--bucket", 8) as usize;
    let reads = arg("--reads", 2_000_000) as usize;
    let rounds = arg("--rounds", 7) as usize;
    let serial = arg("--free", 0) == 0;
    let store = Store::new(keys, width, nparts, stride, bucket);
    let samples: usize = store.parts.iter().map(|p| p.samples.len() * 8).sum();
    let tables: usize = store.parts.iter().map(|p| p.table.len() * 4).sum();
    let eps = store.parts.iter().map(|p| p.eps).max().unwrap_or(0);
    let widest = store
        .parts
        .iter()
        .flat_map(|p| p.table.windows(2).map(|w| w[1] - w[0]))
        .max()
        .unwrap_or(0);
    println!(
        "keysearch: {keys} keys of {width} bytes in {} partitions, {} MiB of numbers, {} MiB of \
         values; a sample every {stride} numbers ({} KiB, the line wrong by {eps} samples at most), a span of {bucket} keys ({} KiB of \
         tables, the fullest span {widest} keys); {reads} reads a pass, {}",
        store.parts.len(),
        (keys * 8) >> 20,
        (keys * width) >> 20,
        samples >> 10,
        tables >> 10,
        if serial { "in series" } else { "free to overlap" },
    );
    let mut sink = vec![0u8; width];
    for dist in [KeyDist::Uniform, KeyDist::Zipfian] {
        let mut g = KeyGen::new(dist, keys as u64, 0x5EED);
        let draws: Vec<u64> = (0..reads).map(|_| g.next()).collect();
        let xs: Vec<u64> = draws.iter().map(|&d| store.numbers[d as usize]).collect();
        let order = [
            Read::Dense,
            Read::Samples,
            Read::Buckets,
            Read::Records,
            Read::Guess,
        ];
        let mut ns = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut sums = [0u64; 5];
        for round in 0..rounds {
            for k in 0..5 {
                let i = (k + round) % 5;
                let (t, s) = pass(&store, &draws, &xs, order[i], serial, &mut sink);
                ns[i].push(t);
                sums[i] = s;
            }
        }
        assert!(
            sums.iter().all(|&s| s == sums[0]),
            "the reads agree on every value"
        );
        let med = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let (d, s, b, r, g) = (
            med(&mut ns[0]),
            med(&mut ns[1]),
            med(&mut ns[2]),
            med(&mut ns[3]),
            med(&mut ns[4]),
        );
        println!(
            "{:8} dense {d:6.1} ns   samples {s:6.1} ns (+{:.0})   buckets {b:6.1} ns (+{:.0})   \
             records {r:6.1} ns (+{:.0})   guess {g:6.1} ns (+{:.0})",
            match dist {
                KeyDist::Uniform => "uniform",
                _ => "zipfian",
            },
            s - d,
            b - d,
            r - d,
            g - d,
        );
    }
}
