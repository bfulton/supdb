//! Which form a workload fits: each form's faults an operation under a page
//! cache of a given size, by simulation, for every pass of a row.
//!
//! A form is a map from a key's rank to the pages a read of it touches: the
//! stride form's value at `rank * W`, the block form's page at the rank's
//! share of its pages, the raw form's hash slot and its record. A pass of the
//! suite is a sequence of ranks. So what a form costs a workload in faults
//! under a cache of `C` bytes is a property of the two, computable with no
//! store: this probe drives the row's own passes (`run::replay_row`, the
//! suite's generators and `ycsb` itself) through an LRU over each form's
//! pages, at each capacity asked for, and prints the misses an operation.
//!
//! Beside each exact cache it prints two estimates a store could afford:
//!
//! - `minip`: a miniature of the same cache over the pages a hash of the
//!   page's number picks, one in sixty-four (`--rate`), scaled back. It
//!   needs every read's page under every form, so it is a hook on the read
//!   path.
//! - `che`: no cache at all. Each partition's part of a form is a
//!   population of pages touched at the rate the pass touched it, as if
//!   uniformly, and the engine's model gives the misses
//!   (`zselect::model_populations`, Che's approximation to LRU). Its inputs
//!   are a count of reads a partition and the partition's bytes under the
//!   form, which a store has without a hook; what it cannot see is skew
//!   inside a partition, or what the pass before left in the cache.
//!
//! What it does not model, each a reason a measured figure can differ: the
//! kernel's two lists (a page read once is not a page read twice there), the
//! readahead of a mapping not advised random, the memo of decoded groups and
//! the block cache's copies (a hit on either touches no page), the level-0
//! pieces a write mix leaves, and the capacity itself, which is the bound
//! less whatever else the process and the page cache hold at that moment and
//! moves through a pass. Two parts of that are a form's own and are not in
//! the model: the pages its reads keep beside its file (the block form's
//! scans seek the ordered index's heads, eight bytes a key) and the process
//! memory its read path grows. So a capacity solved from one store's
//! faults is not another store's: the two-form stores it priced at the
//! stride arm's capacity faulted where it gave none (`docs/engine.md`). A key a mix has written is read from the memtable
//! and touches no page of a form (`--no-overlay` reads it through the form);
//! a scan touches its pages whatever was written under it.
//!
//! ```text
//! cargo run --release --example formfit -- --size 10000000 \
//!     --cache-mb 850,900,950,975,1000,1100,1200
//! ```
//!
//! `--stride-pct` adds stores in two forms, the first share of the keys in
//! the stride form and the rest in the block form, which is what a store
//! that chose a form a partition would hold; `head_ops` is the operations
//! that began in the first share, for pricing the two forms' own costs.
//!
//! One line a form, a capacity and a pass: the operations, the exact cache's
//! page misses an operation and the share of operations that missed at all,
//! and the estimates of the first.

use supdb::db::zselect::{model_populations, Population};
use supdb_bench::engines::{Engine, Features, ReaderOpener, Res};
use supdb_bench::run::{replay_row, Plan};
use supdb_bench::Scale;

const PAGE: u64 = 4096;
const NIL: u32 = u32::MAX;

/// A multiplicative hash of a small number, the high bits taken.
fn mix(x: u64) -> u64 {
    let x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// An LRU over page numbers below a fixed count.
struct Lru {
    cap: u32,
    len: u32,
    head: u32,
    tail: u32,
    prev: Vec<u32>,
    next: Vec<u32>,
    held: Vec<bool>,
}

impl Lru {
    fn new(pages: usize, cap: u64) -> Lru {
        Lru {
            cap: cap.min(pages as u64).max(1) as u32,
            len: 0,
            head: NIL,
            tail: NIL,
            prev: vec![NIL; pages],
            next: vec![NIL; pages],
            held: vec![false; pages],
        }
    }

    fn unlink(&mut self, p: u32) {
        let (a, b) = (self.prev[p as usize], self.next[p as usize]);
        if a == NIL {
            self.head = b;
        } else {
            self.next[a as usize] = b;
        }
        if b == NIL {
            self.tail = a;
        } else {
            self.prev[b as usize] = a;
        }
    }

    fn push_front(&mut self, p: u32) {
        self.prev[p as usize] = NIL;
        self.next[p as usize] = self.head;
        if self.head != NIL {
            self.prev[self.head as usize] = p;
        }
        self.head = p;
        if self.tail == NIL {
            self.tail = p;
        }
    }

    /// The page touched; whether the cache held it.
    fn touch(&mut self, p: u32) -> bool {
        if self.held[p as usize] {
            if self.head != p {
                self.unlink(p);
                self.push_front(p);
            }
            return true;
        }
        if self.len == self.cap {
            let t = self.tail;
            self.unlink(t);
            self.held[t as usize] = false;
            self.len -= 1;
        }
        self.held[p as usize] = true;
        self.push_front(p);
        self.len += 1;
        false
    }
}

/// How a form lays a store's keys out in pages.
#[derive(Clone, Copy)]
enum Kind {
    /// The keys in key order, the ranks below `cut` at `head` bytes a key
    /// and the rest at `tail` after them: one form where `cut` is every key
    /// or none, and otherwise a store whose first partitions are in one
    /// form and whose rest are in another. A point read below the cut
    /// touches the pages its bytes span (the stride form's value), and
    /// above it the one page its first byte is on (the block form's group
    /// lies within a page).
    Ordered { cut: u64, head: f64, tail: f64 },
    /// The raw form, a partition at a time: its hash slots, its records at
    /// `rec` bytes each in key order, and its ordered index's heads at eight
    /// bytes a key, which a scan's seek reads and a point read does not.
    Raw { rec: u64, part_keys: u64 },
}

struct Geometry {
    name: String,
    kind: Kind,
    size: u64,
    pages: u64,
    /// Raw only: a partition's first page, its hash pages and its record
    /// pages, the heads after them.
    parts: Vec<(u64, u64, u64)>,
}

impl Geometry {
    fn ordered(name: String, size: u64, cut: u64, head: f64, tail: f64) -> Geometry {
        let mut g = Geometry {
            name,
            kind: Kind::Ordered { cut, head, tail },
            size,
            pages: 0,
            parts: Vec::new(),
        };
        g.pages = g.end_of(size).div_ceil(PAGE);
        g
    }

    /// The ranks below which an operation is the head form's.
    fn cut(&self) -> u64 {
        match self.kind {
            Kind::Ordered { cut, .. } => cut,
            Kind::Raw { .. } => 0,
        }
    }

    /// An ordered form's bytes before `rank`'s, read as a range's end: the
    /// head's bytes up to the cut, and past it the head's to a page's edge,
    /// since the tail is another file, and the tail's after.
    fn end_of(&self, rank: u64) -> u64 {
        let Kind::Ordered { cut, head, tail } = self.kind else {
            unreachable!("the raw form's pages are by partition")
        };
        if rank <= cut {
            (rank as f64 * head) as u64
        } else {
            ((cut as f64 * head) as u64).div_ceil(PAGE) * PAGE + ((rank - cut) as f64 * tail) as u64
        }
    }

    /// An ordered form's byte offset of `rank`'s first byte: `end_of`, but
    /// the cut's own rank is the tail's first.
    fn start_of(&self, rank: u64) -> u64 {
        let Kind::Ordered { cut, head, .. } = self.kind else {
            unreachable!("the raw form's pages are by partition")
        };
        if rank == cut {
            ((cut as f64 * head) as u64).div_ceil(PAGE) * PAGE
        } else {
            self.end_of(rank)
        }
    }

    fn raw(size: u64, rec: u64, part_keys: u64) -> Geometry {
        let mut parts = Vec::new();
        let mut at = 0u64;
        let mut left = size;
        while left > 0 {
            let n = left.min(part_keys);
            // `flatindex`: slots of eight bytes, a power of two of them, at
            // most three quarters full.
            let slots = (n * 4).div_ceil(3).next_power_of_two();
            let hash = (slots * 8).div_ceil(PAGE);
            let recs = (n * rec).div_ceil(PAGE);
            let heads = (n * 8).div_ceil(PAGE);
            parts.push((at, hash, recs));
            at += hash + recs + heads;
            left -= n;
        }
        Geometry {
            name: String::from("raw"),
            kind: Kind::Raw { rec, part_keys },
            size,
            pages: at,
            parts,
        }
    }

    /// Each page's population and each population's pages: a partition's
    /// pages of an ordered form, by the first rank a page holds; a
    /// partition's hash pages, its record pages and its heads' of the raw
    /// form.
    fn populations(&self, part_keys: u64) -> (Vec<u32>, Vec<(u64, u64)>) {
        let mut pop_of = Vec::with_capacity(self.pages as usize);
        match self.kind {
            Kind::Ordered { .. } => {
                let mut rank = 0u64;
                for page in 0..self.pages {
                    // The first rank whose bytes end past this page's start.
                    while rank + 1 < self.size && self.end_of(rank + 1) <= page * PAGE {
                        rank += 1;
                    }
                    pop_of.push((rank / part_keys) as u32);
                }
            }
            Kind::Raw { .. } => {
                for (i, &(base, hash, recs)) in self.parts.iter().enumerate() {
                    let end = self.parts.get(i + 1).map_or(self.pages, |p| p.0);
                    for page in base..end {
                        let kind =
                            u32::from(page >= base + hash) + u32::from(page >= base + hash + recs);
                        pop_of.push(i as u32 * 3 + kind);
                    }
                }
            }
        }
        let mut pops = vec![(0u64, 0u64); pop_of.iter().max().map_or(0, |m| *m as usize + 1)];
        for &p in &pop_of {
            pops[p as usize].0 += 1;
        }
        (pop_of, pops)
    }

    /// The pages a point read of `rank` touches.
    fn point(&self, rank: u64, out: &mut Vec<u32>) {
        match self.kind {
            Kind::Ordered { cut, .. } => {
                let a = self.start_of(rank) / PAGE;
                out.push(a as u32);
                if rank < cut {
                    let b = (self.end_of(rank + 1) - 1) / PAGE;
                    if b != a {
                        out.push(b as u32);
                    }
                }
            }
            Kind::Raw { rec, part_keys } => {
                let (base, hash, _) = self.parts[(rank / part_keys) as usize];
                let local = rank % part_keys;
                out.push((base + mix(rank) % hash) as u32);
                let (a, b) = (local * rec / PAGE, (local * rec + rec - 1) / PAGE);
                out.push((base + hash + a) as u32);
                if b != a {
                    out.push((base + hash + b) as u32);
                }
            }
        }
    }

    /// The pages a scan of `n` keys from `rank` touches, in the order it
    /// touches them.
    fn scan(&self, rank: u64, n: u64, out: &mut Vec<u32>) {
        let end = (rank + n).min(self.size);
        if end <= rank {
            return;
        }
        match self.kind {
            Kind::Ordered { cut, .. } => {
                // A scan across the cut reads the head's last pages and the
                // tail's first, two ranges.
                let mut r = rank;
                while r < end {
                    let stop = if r < cut { end.min(cut) } else { end };
                    let a = self.start_of(r) / PAGE;
                    let b = (self.end_of(stop) - 1) / PAGE;
                    out.extend((a..=b).map(|p| p as u32));
                    r = stop;
                }
            }
            Kind::Raw { rec, part_keys } => {
                let mut r = rank;
                let mut seek = true;
                while r < end {
                    let part = r / part_keys;
                    let (base, hash, recs) = self.parts[part as usize];
                    let local = r % part_keys;
                    let stop = end.min((part + 1) * part_keys);
                    if seek {
                        out.push((base + hash + recs + local * 8 / PAGE) as u32);
                        seek = false;
                    }
                    let a = local * rec / PAGE;
                    let b = ((local + (stop - r)) * rec - 1) / PAGE;
                    out.extend((a..=b).map(|p| (base + hash + p) as u32));
                    r = stop;
                }
            }
        }
    }
}

struct Cache {
    lru: Lru,
    /// Whether a page is this cache's: every page of the exact one, and of
    /// a miniature the pages a hash of the page's number picks.
    member: Vec<bool>,
    /// Pages over member pages: what a miss in a miniature stands for.
    scale: f64,
    page_misses: u64,
    op_misses: u64,
}

impl Cache {
    fn new(g: &Geometry, cap_pages: u64, rate: u64) -> Cache {
        let pages = g.pages as usize;
        let member: Vec<bool> = (0..g.pages).map(|p| mix(p).is_multiple_of(rate)).collect();
        let members = member.iter().filter(|m| **m).count().max(1);
        let scale = pages as f64 / members as f64;
        let mut c = Cache {
            lru: Lru::new(pages, ((cap_pages as f64) / scale).round() as u64),
            member,
            scale,
            page_misses: 0,
            op_misses: 0,
        };
        // The store as a load leaves it: the pages in the order they were
        // written, so the cache holds the file's tail.
        for p in 0..g.pages {
            if c.member[p as usize] {
                c.lru.touch(p as u32);
            }
        }
        c
    }

    /// One operation's pages.
    fn op(&mut self, pages: &[u32]) {
        let mut missed = false;
        for &p in pages {
            if self.member[p as usize] && !self.lru.touch(p) {
                self.page_misses += 1;
                missed = true;
            }
        }
        self.op_misses += u64::from(missed);
    }
}

/// The engine's own model over the populations' pages and touches
/// (`zselect::model_populations`), so the estimate the probe holds to the
/// exact cache is the one a store would compute.
fn che(pops: &[(u64, u64)], cap_bytes: u64) -> f64 {
    let pops: Vec<Population> = pops
        .iter()
        .map(|&(pages, touches)| Population {
            bytes: pages * PAGE,
            touches: touches as f64,
        })
        .collect();
    model_populations(cap_bytes, &pops)
}

struct Model {
    g: Geometry,
    /// A capacity's megabytes, its exact cache and its miniature.
    caps: Vec<(u64, Cache, Cache)>,
    /// The population each page is of -- a partition's part of an ordered
    /// form; a partition's hash pages, its records and its heads for the
    /// raw form -- and each population's pages and its touches since the
    /// last phase line.
    pop_of: Vec<u32>,
    pops: Vec<(u64, u64)>,
    /// Operations that began below the cut, since the last phase line.
    head_ops: u64,
}

/// The store the suite's passes are replayed over: the keys the mixes have
/// written and every form's caches.
struct Sim {
    size: u64,
    overlay: bool,
    written: Vec<u64>,
    models: Vec<Model>,
    pages: Vec<u32>,
    /// Point reads the overlay answered, since the last phase line.
    overlaid: u64,
}

impl Model {
    /// One operation's pages, to every cache and to the populations' counts.
    fn touch(&mut self, pages: &[u32]) {
        for (_, exact, mini) in &mut self.caps {
            exact.op(pages);
            mini.op(pages);
        }
        for &p in pages {
            self.pops[self.pop_of[p as usize] as usize].1 += 1;
        }
    }
}

fn key_number(k: &[u8]) -> u64 {
    k.iter().fold(0u64, |n, b| n * 10 + u64::from(b - b'0'))
}

impl Sim {
    fn is_written(&self, k: u64) -> bool {
        k >= self.size || self.written[(k / 64) as usize] >> (k % 64) & 1 == 1
    }
}

impl Engine for Sim {
    fn name(&self) -> &'static str {
        "formfit"
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: true,
            transactions: false,
            checksums: false,
            reopen_for_write: false,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        for (k, _) in items {
            let k = key_number(k);
            if k < self.size {
                self.written[(k / 64) as usize] |= 1 << (k % 64);
            }
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        let k = key_number(key);
        if k >= self.size || self.overlay && self.is_written(k) {
            self.overlaid += 1;
            return Ok(0);
        }
        for m in &mut self.models {
            self.pages.clear();
            m.g.point(k, &mut self.pages);
            m.head_ops += u64::from(k < m.g.cut());
            m.touch(&self.pages);
        }
        Ok(0)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        let k = key_number(from).min(self.size - 1);
        for m in &mut self.models {
            self.pages.clear();
            m.g.scan(k, n as u64, &mut self.pages);
            m.head_ops += u64::from(k < m.g.cut());
            m.touch(&self.pages);
        }
        Ok(0)
    }
    fn sync(&mut self) -> Res<()> {
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        0
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        Err("a model has no reader".into())
    }
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let size: u64 = arg(&args, "--size", 10_000_000);
    let ops_div: u64 = arg(&args, "--ops-div", 10);
    let rate: u64 = arg(&args, "--rate", 64);
    // The block form's bytes a key in its pages and the raw form's record,
    // as the suite's ten-million-key store measures them; the stride is the
    // value's width.
    let block_bpk: f64 = arg(&args, "--block-bpk", 74.4);
    let raw_rec: u64 = arg(&args, "--raw-rec", 124);
    let part_keys: u64 = arg(&args, "--part-keys", 345_000);
    let caps_mb: Vec<u64> = arg(
        &args,
        "--cache-mb",
        String::from("850,900,950,1000,1100,1200"),
    )
    .split(',')
    .filter_map(|v| v.parse().ok())
    .collect();
    let overlay = !args.iter().any(|a| a == "--no-overlay");

    let mut plan = Plan::new(Scale::Ooc, Vec::new(), size, 4);
    plan.ops_div = ops_div;
    let stride = plan.value_size as f64;
    // The store's first share in the stride form and its rest in the block
    // form, for each share asked: what a store that chooses a form a
    // partition would hold, the hot partitions first where the hot keys are
    // the lowest.
    let shares: Vec<u64> = arg(&args, "--stride-pct", String::from("25,50,75"))
        .split(',')
        .filter_map(|v| v.parse().ok())
        .collect();
    let mut geometries = vec![
        Geometry::ordered(String::from("stride"), size, size, stride, block_bpk),
        Geometry::ordered(String::from("block"), size, 0, stride, block_bpk),
    ];
    for pct in shares {
        geometries.push(Geometry::ordered(
            format!("stride{pct}"),
            size,
            size / 100 * pct,
            stride,
            block_bpk,
        ));
    }
    geometries.push(Geometry::raw(size, raw_rec, part_keys));
    let models: Vec<Model> = geometries
        .into_iter()
        .map(|g| {
            eprintln!(
                "form {} pages {} mb {:.0}",
                g.name,
                g.pages,
                (g.pages * PAGE) as f64 / 1e6
            );
            let caps = caps_mb
                .iter()
                .map(|&mb| {
                    let cap = mb * 1_000_000 / PAGE;
                    (mb, Cache::new(&g, cap, 1), Cache::new(&g, cap, rate))
                })
                .collect();
            let (pop_of, pops) = g.populations(part_keys);
            Model {
                g,
                caps,
                pop_of,
                pops,
                head_ops: 0,
            }
        })
        .collect();
    let mut sim = Sim {
        size,
        overlay,
        written: vec![0; size.div_ceil(64) as usize],
        models,
        pages: Vec::new(),
        overlaid: 0,
    };
    println!("form cache_mb pass ops overlaid head_ops exact_pm exact_om minip_pm che_pm");
    replay_row(&mut sim, size, &plan, |sim, pass, ops| {
        for m in &mut sim.models {
            for (mb, exact, mini) in &mut m.caps {
                let per = |c: &Cache| c.page_misses as f64 * c.scale / ops as f64;
                println!(
                    "{} {} {} {} {} {} {:.5} {:.5} {:.5} {:.5}",
                    m.g.name,
                    mb,
                    pass,
                    ops,
                    sim.overlaid,
                    m.head_ops,
                    per(exact),
                    exact.op_misses as f64 / ops as f64,
                    per(mini),
                    che(&m.pops, *mb * 1_000_000) / ops as f64,
                );
                for c in [exact, mini] {
                    c.page_misses = 0;
                    c.op_misses = 0;
                }
            }
            m.head_ops = 0;
            for p in &mut m.pops {
                p.1 = 0;
            }
        }
        sim.overlaid = 0;
    })
}
