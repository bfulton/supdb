//! Key and value generation.
//!
//! The design document confesses to getting payload generation wrong three
//! times, each time in the direction that flattered the engine: a fixed RNG
//! seed that made every value byte-identical apart from eight bytes; an
//! identical 50-byte prefix on every value; and a `format!` per key that made
//! the harness 45% of the measured cost. This module exists so those are
//! structural impossibilities rather than things to remember.
//!
//! Three rules:
//!
//! 1. **Generation is not on the clock.** Keys are written into a caller-owned
//!    buffer; values are slices of a pre-built pool. The measured loop does an
//!    index, not an allocation.
//! 2. **Entropy is a stated parameter.** `Payload::compressibility` is
//!    explicit, so a disk-footprint comparison can say what it assumed instead
//!    of inheriting it from a bug.
//! 3. **Key distribution is a stated parameter.** Uniform-random is the
//!    default only because db_bench uses it; Cao et al. (FAST'20) found it
//!    unrepresentative of every production RocksDB workload they measured, so
//!    Zipfian is a first-class option rather than an afterthought.

/// xorshift64*, seeded explicitly. Fast enough not to show up in the profile.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }
    // Deliberately not `Iterator`: this is an infinite generator on the hot
    // path and the trait's Option wrapper is measurable there.
    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
    #[inline]
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// How keys are drawn from the space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDist {
    /// db_bench's default. Retained for comparability, not for realism.
    Uniform,
    /// Zipfian with theta 0.99, YCSB's default skew, over the key space
    /// itself: rank 0 is key 0, so the hot keys are the lowest keys and
    /// contiguous.
    Zipfian,
    /// YCSB's scrambled Zipfian, the shape its `requestdistribution=zipfian`
    /// runs: a Zipfian draw over YCSB's ten billion items, hashed with its
    /// FNV-1a and folded onto the key space, so the hot keys are spread
    /// over it.
    Scrambled,
    /// YCSB's `latest`: a Zipfian over the keys that exist with the newest
    /// the hottest, over a count that grows as inserts commit
    /// (`KeyGen::set_keys`). Key 0 is never drawn, as in YCSB.
    Latest,
    /// Ascending. The best case for any structure with sorted layout.
    Sequential,
}

impl KeyDist {
    pub fn parse(s: &str) -> Option<KeyDist> {
        match s {
            "uniform" => Some(KeyDist::Uniform),
            "zipfian" => Some(KeyDist::Zipfian),
            "scrambled" => Some(KeyDist::Scrambled),
            "latest" => Some(KeyDist::Latest),
            "sequential" => Some(KeyDist::Sequential),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyDist::Uniform => "uniform",
            KeyDist::Zipfian => "zipfian",
            KeyDist::Scrambled => "scrambled",
            KeyDist::Latest => "latest",
            KeyDist::Sequential => "sequential",
        }
    }
}

/// The item count YCSB's scrambled Zipfian draws over before the hash folds
/// the draw onto the key space, and the harmonic sum over it at theta 0.99,
/// both as YCSB's `ScrambledZipfianGenerator` has them. The sum would take
/// ten billion terms to recompute, so it is YCSB's constant, and a test
/// holds it to an Euler-Maclaurin estimate.
pub const SCRAMBLED_ITEMS: u64 = 10_000_000_000;
pub const SCRAMBLED_ZETAN: f64 = 26.469_028_201_783_02;

/// YCSB's `fnvhash64`: FNV-1a over the eight bytes of the value, lowest
/// first, and the magnitude of the result read as a signed word, which is
/// what the Java takes `Math.abs` of.
#[inline]
pub fn fnv64(mut v: u64) -> u64 {
    const OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
    const PRIME: u64 = 1_099_511_628_211;
    let mut h = OFFSET;
    for _ in 0..8 {
        h ^= v & 0xff;
        v >>= 8;
        h = h.wrapping_mul(PRIME);
    }
    (h as i64).unsigned_abs()
}

/// Draws key indices according to a distribution.
///
/// The Zipfian generator is the standard YCSB one (Gray et al.'s bulk-loading
/// method): the expensive harmonic sum is computed once at construction, so
/// drawing a key is a log and a couple of multiplies rather than a scan.
pub struct KeyGen {
    dist: KeyDist,
    /// The key space: every draw is below it.
    n: u64,
    rng: Rng,
    seq: u64,
    /// The Zipfian's own domain: the key space for `Zipfian`, YCSB's ten
    /// billion for `Scrambled`, the newest key's index for `Latest`.
    items: u64,
    zeta_n: f64,
    alpha: f64,
    eta: f64,
    theta: f64,
}

impl KeyGen {
    pub fn new(dist: KeyDist, n: u64, seed: u64) -> KeyGen {
        let theta = 0.99f64;
        let n = n.max(1);
        let items = KeyGen::items_of(dist, n);
        let zeta_n = match dist {
            KeyDist::Zipfian | KeyDist::Latest => zeta(items, theta),
            KeyDist::Scrambled => SCRAMBLED_ZETAN,
            KeyDist::Uniform | KeyDist::Sequential => 0.0,
        };
        let mut g = KeyGen {
            dist,
            n,
            rng: Rng::new(seed),
            seq: 0,
            items,
            zeta_n,
            alpha: 1.0 / (1.0 - theta),
            eta: 0.0,
            theta,
        };
        g.eta = g.eta_of();
        g
    }

    fn items_of(dist: KeyDist, n: u64) -> u64 {
        match dist {
            KeyDist::Scrambled => SCRAMBLED_ITEMS,
            KeyDist::Latest => (n - 1).max(1),
            _ => n,
        }
    }

    /// YCSB's `eta` for the Zipfian's domain and sum as they stand.
    fn eta_of(&self) -> f64 {
        if self.zeta_n == 0.0 {
            return 0.0;
        }
        (1.0 - (2.0 / self.items as f64).powf(1.0 - self.theta))
            / (1.0 - zeta(2, self.theta) / self.zeta_n)
    }

    /// The key space grown to `n` keys. The latest distribution keeps the
    /// newest key the hottest by extending its harmonic sum one term a key
    /// -- one `powf` each -- and re-deriving `eta`, as YCSB's generator does
    /// when its item count moves; the plain Zipfian grows the same way, and
    /// the others fold onto the new space. Never shrinks.
    pub fn set_keys(&mut self, n: u64) {
        let n = n.max(1);
        if n <= self.n {
            return;
        }
        let items = KeyGen::items_of(self.dist, n);
        if matches!(self.dist, KeyDist::Zipfian | KeyDist::Latest) && items > self.items {
            for i in self.items + 1..=items {
                self.zeta_n += 1.0 / (i as f64).powf(self.theta);
            }
            self.items = items;
            self.eta = self.eta_of();
        }
        self.n = n;
    }

    /// YCSB's Zipfian draw over `items`: rank 0 the most likely.
    #[inline]
    fn zipf(&mut self) -> u64 {
        let u = self.rng.unit();
        let uz = u * self.zeta_n;
        if uz < 1.0 {
            return 0;
        }
        if uz < 1.0 + 0.5f64.powf(self.theta) {
            return 1;
        }
        let v = (self.items as f64 * (self.eta * u - self.eta + 1.0).powf(self.alpha)) as u64;
        v.min(self.items - 1)
    }

    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next(&mut self) -> u64 {
        match self.dist {
            KeyDist::Uniform => self.rng.below(self.n),
            KeyDist::Sequential => {
                let v = self.seq % self.n;
                self.seq += 1;
                v
            }
            KeyDist::Zipfian => self.zipf(),
            KeyDist::Scrambled => fnv64(self.zipf()) % self.n,
            // YCSB's `max - zipfian(max)`: the newest key for rank 0, and
            // never key 0.
            KeyDist::Latest => (self.items - self.zipf()).min(self.n - 1),
        }
    }
}

fn zeta(n: u64, theta: f64) -> f64 {
    let mut s = 0.0;
    for i in 1..=n {
        s += 1.0 / (i as f64).powf(theta);
    }
    s
}

/// Sixteen decimal digits, db_bench's key shape, into a caller-owned buffer.
///
/// Fixed-width decimal is also the shape that broke FxHash in the design
/// document, so it is the right default for anything touching the key table.
#[inline]
pub fn db_key_into(n: u64, buf: &mut [u8; 16]) {
    let mut v = n;
    for i in (0..16).rev() {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
}

/// What a store's keys are, which is a stated parameter as their
/// distribution is: the same workloads run over either, and what an engine
/// makes of a key's bytes and of the gaps between keys is part of what is
/// measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum KeyShape {
    /// Sixteen decimal digits of the key's index (`db_key_into`): db_bench's
    /// keys, a dense run of numbers in which the next key is the next number.
    /// Every row before the shape was a parameter is of these.
    #[default]
    Numbered,
    /// Sixteen hex digits of a random 64-bit id (`id_of`): what an
    /// application's own identifiers look like to a store -- no key the
    /// successor of another, the gaps between neighbours random, a key's
    /// bytes of no use to a compressor, and a key inserted later falling
    /// anywhere among the ones there.
    Ids,
}

impl KeyShape {
    pub fn parse(s: &str) -> Option<KeyShape> {
        match s {
            "numbered" => Some(KeyShape::Numbered),
            "ids" => Some(KeyShape::Ids),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            KeyShape::Numbered => "numbered",
            KeyShape::Ids => "ids",
        }
    }
}

/// SplitMix64's finalizer: a bijection of the 64-bit words whose output
/// bits each depend on every input bit.
#[inline]
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The ids of a store's keys (`KeyShape::Ids`): which 64-bit id each key
/// index has, in a store loaded with `loaded` keys.
///
/// The loaded keys' ids are `loaded` independent uniform draws, sorted, as
/// far as a function of the index can give them with nothing sorted and
/// little stored. The space is cut into cells of one width, each loaded
/// key is dealt to a cell by a hash of its number, and the cells' running
/// counts are kept: so how many ids fall in any stretch of the space
/// varies as independent draws' counts do, at every scale from a cell up --
/// some cells hold half again their share and some hold none -- which is
/// what a structure that guesses a key's place from its value is priced
/// by. One id a stratum of the whole space, the first version of this,
/// puts key `n` within one stride of `n` strides in; a guess was never
/// wrong there by more than a key, and a search built on guessing read
/// better than random ids would let it. Inside a cell the keys it was
/// dealt are one to a stratum of the cell, a random point of each, so the
/// ids ascend with the index and no two are one id: a load in key order, a
/// scan from a drawn key and a read of a drawn key are each a function of
/// an index. A cell holds sixteen keys or so, fewer in a small store and
/// more past a million keys, where the cells stop at 65,536 and the two
/// tables at half a megabyte.
///
/// An index at or above `loaded` is a key inserted after the load: a
/// uniform draw of the whole space, so it falls among the loaded keys and
/// not above them. It is one of the loaded ids with a chance in 2^64 a
/// loaded key, and an insert that was would be an update.
#[derive(Debug)]
pub struct Ids {
    loaded: u64,
    /// The id's bits below its cell's: a cell is `1 << shift` ids wide.
    shift: u32,
    /// `cum[c]` is the loaded keys in the cells below `c`; one past the
    /// last cell is `loaded`.
    cum: Vec<u32>,
    /// `first[t]` is the cell key index `t << step` is in: where the walk
    /// to an index's cell starts, a cell or two short of it.
    first: Vec<u32>,
    step: u32,
}

/// The most cells the id space is cut into: the two tables are a word a
/// cell each, and stay in the core's cache.
const ID_CELLS_MAX: u64 = 1 << 16;
/// The loaded keys a cell holds on average, while the cells are under
/// their most.
const ID_CELL_KEYS: u64 = 16;

impl Ids {
    pub fn new(loaded: u64) -> Ids {
        assert!(
            loaded < u64::from(u32::MAX),
            "the cells' counts are 32-bit: {loaded} keys"
        );
        // A power of two of cells, two at least, so a cell is a shift.
        let cells = (loaded / ID_CELL_KEYS)
            .next_power_of_two()
            .clamp(2, ID_CELLS_MAX);
        let bits = cells.trailing_zeros();
        let shift = 64 - bits;
        let mut cum = vec![0u32; cells as usize + 1];
        for i in 0..loaded {
            cum[(mix64(i ^ 0x9E37_79B9_7F4A_7C15) >> shift) as usize + 1] += 1;
        }
        for c in 0..cells as usize {
            cum[c + 1] += cum[c];
        }
        // One entry a cell's worth of indexes: the walk from an entry to
        // an index's cell crosses the cells those few indexes span.
        let step = (loaded / cells).max(1).ilog2();
        let mut first = Vec::with_capacity((loaded >> step) as usize + 1);
        let mut c = 0usize;
        for t in 0..=(loaded >> step) {
            let n = (t << step).min(loaded.saturating_sub(1));
            while u64::from(cum[c + 1]) <= n && c + 1 < cells as usize {
                c += 1;
            }
            first.push(c as u32);
        }
        Ids {
            loaded,
            shift,
            cum,
            first,
            step,
        }
    }

    /// The id of key index `n`.
    #[inline]
    pub fn of(&self, n: u64) -> u64 {
        if n >= self.loaded {
            return mix64(n ^ 0xD6E8_FEB8_6659_FD93);
        }
        let mut c = self.first[(n >> self.step) as usize] as usize;
        while u64::from(self.cum[c + 1]) <= n {
            c += 1;
        }
        let (below, here) = (
            u64::from(self.cum[c]),
            u64::from(self.cum[c + 1] - self.cum[c]),
        );
        // The cell cut into a stratum for each key dealt to it, and a
        // random point of this key's: a multiply and a shift for the
        // point, since a remainder is a second divide.
        let stratum = (1u64 << self.shift) / here;
        let point = ((u128::from(mix64(n.wrapping_add(0xA076_1D64_78BD_642F)))
            * u128::from(stratum))
            >> 64) as u64;
        ((c as u64) << self.shift) + (n - below) * stratum + point
    }
}

/// `id` as sixteen lowercase hex digits, which order as the ids do.
#[inline]
pub fn id_key_into(id: u64, buf: &mut [u8; 16]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut v = id;
    for i in (0..16).rev() {
        buf[i] = HEX[(v & 15) as usize];
        v >>= 4;
    }
}

/// The keys of one store: their shape and, for ids, which id each index
/// has (`Ids`). Every key a pass writes or asks for comes through here
/// from its index, so a pass is over one shape whichever phase or thread
/// makes the key.
#[derive(Clone, Debug)]
pub struct KeyMap {
    shape: KeyShape,
    ids: Option<std::sync::Arc<Ids>>,
}

impl KeyMap {
    /// The keys of a store loaded with `loaded` of them. For ids this
    /// deals every loaded key to its cell, a few nanoseconds a key: made
    /// once a pass, outside whatever the pass times.
    pub fn new(shape: KeyShape, loaded: u64) -> KeyMap {
        KeyMap {
            shape,
            ids: (shape == KeyShape::Ids).then(|| std::sync::Arc::new(Ids::new(loaded))),
        }
    }

    /// db_bench's keys, which need no count.
    pub fn numbered() -> KeyMap {
        KeyMap::new(KeyShape::Numbered, 0)
    }

    pub fn shape(&self) -> KeyShape {
        self.shape
    }

    /// The key of index `n`, into a caller-owned buffer.
    #[inline]
    pub fn key_into(&self, n: u64, buf: &mut [u8; 16]) {
        match &self.ids {
            None => db_key_into(n, buf),
            Some(ids) => id_key_into(ids.of(n), buf),
        }
    }
}

/// A pool of pre-generated value bytes.
///
/// Values are handed out as slices at varying offsets, so the measured loop
/// never allocates and never runs a generator. The pool is large enough that
/// LZ4's 64 KiB window cannot see the repetition, which is what keeps a
/// footprint comparison honest.
pub struct Payload {
    pool: Vec<u8>,
    value_size: usize,
    /// Fraction of each value that is drawn from a repeating pattern rather
    /// than from entropy. db_bench's default is about 0.5.
    pub compressibility: f64,
}

impl Payload {
    pub fn new(value_size: usize, compressibility: f64, seed: u64) -> Payload {
        let target = (8 << 20).max(value_size * 16);
        let mut rng = Rng::new(seed);
        let mut pool = Vec::with_capacity(target + value_size);
        // A repeating motif for the compressible part, then real entropy for
        // the rest. Crucially the motif's phase varies with position, so no
        // two values share a byte-identical prefix -- the exact defect
        // confessed to twice in the design document.
        let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
        while pool.len() < target + value_size {
            let start = pool.len();
            let comp = ((value_size as f64) * compressibility.clamp(0.0, 1.0)) as usize;
            for i in 0..comp {
                pool.push(motif[(start + i) % motif.len()]);
            }
            for _ in comp..value_size {
                pool.push((rng.next() & 0xff) as u8);
            }
        }
        Payload {
            pool,
            value_size,
            compressibility,
        }
    }

    /// A value-sized slice at a pseudo-random offset. No allocation, no copy.
    #[inline]
    pub fn get(&self, rng: &mut Rng) -> &[u8] {
        let span = self.pool.len() - self.value_size;
        let off = (rng.next() as usize) % span;
        &self.pool[off..off + self.value_size]
    }

    pub fn value_size(&self) -> usize {
        self.value_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The ids of the loaded keys ascend with their index and so do their
    /// keys' bytes, across the whole 64-bit space; no two of a sample are
    /// one id; their low digits are no run of numbers; and an index past the
    /// load is a key among the loaded ones, not above them, the same key
    /// every time it is asked for.
    #[test]
    fn the_ids_ascend_with_the_index_and_an_insert_falls_among_them() {
        for loaded in [1u64, 2, 17, 1_000, 300_000, 10_000_000] {
            let km = KeyMap::new(KeyShape::Ids, loaded);
            let ids = Ids::new(loaded);
            // Every index of a small store; of a large one its first
            // stretch, its last, and one across the middle.
            let spans: Vec<(u64, u64)> = if loaded <= 300_000 {
                vec![(0, loaded)]
            } else {
                vec![
                    (0, 20_000),
                    (loaded / 2 - 10_000, loaded / 2 + 10_000),
                    (loaded - 20_000, loaded),
                ]
            };
            let mut seen = std::collections::HashSet::new();
            let mut steps = std::collections::HashSet::new();
            let mut n = 0u64;
            for (from, to) in spans {
                let (mut prev, mut pk) = (None, [0u8; 16]);
                for i in from..to {
                    let id = ids.of(i);
                    let mut k = [0u8; 16];
                    km.key_into(i, &mut k);
                    assert!(k.iter().all(|b| b.is_ascii_hexdigit()), "{k:?}");
                    assert_eq!(
                        u64::from_str_radix(std::str::from_utf8(&k).unwrap(), 16),
                        Ok(id)
                    );
                    if let Some(p) = prev {
                        assert!(id > p, "{loaded}: id {i} above the one before");
                        assert!(k > pk, "{loaded}: key {i} above the one before");
                        steps.insert(id - p);
                    }
                    assert!(seen.insert(id));
                    (prev, pk) = (Some(id), k);
                    n += 1;
                }
            }
            if loaded >= 1_000 {
                assert!(
                    steps.len() as u64 > n / 2,
                    "{loaded}: the gaps are not one stride: {} of {n}",
                    steps.len()
                );
                // The top of the space is reached, not a corner of it.
                assert!(ids.of(loaded - 1) > u64::MAX / 2);
                // Inserts: among the loaded keys, spread, and stable.
                let top = ids.of(loaded - 1);
                let (mut below_mid, mut below_top) = (0, 0);
                for j in 0..1_000u64 {
                    let id = ids.of(loaded + j);
                    assert_eq!(id, ids.of(loaded + j));
                    below_mid += u64::from(id < u64::MAX / 2);
                    below_top += u64::from(id < top);
                }
                assert!(
                    (350..=650).contains(&below_mid),
                    "{loaded}: inserts on both sides of the middle: {below_mid}"
                );
                assert!(
                    below_top >= 985,
                    "{loaded}: inserts under the greatest loaded key: {below_top}"
                );
            }
        }
        // The numbered shape is db_bench's, whatever the count.
        let (mut a, mut b) = ([0u8; 16], [0u8; 16]);
        KeyMap::numbered().key_into(42, &mut a);
        db_key_into(42, &mut b);
        assert_eq!(a, b);
        assert_eq!(KeyShape::parse("ids"), Some(KeyShape::Ids));
        assert_eq!(
            KeyShape::parse(KeyShape::Numbered.name()),
            Some(KeyShape::Numbered)
        );
        assert_eq!(KeyShape::parse("uuid"), None);
    }

    /// The ids are spread as independent draws are and no more evenly: how
    /// far key `n`'s id lies from where an even spread would put it is,
    /// over a store, as large as a random walk's -- some hundreds of keys'
    /// worth at a million keys, where one id a stratum would be within
    /// one -- and the counts of ids in equal stretches of the space vary as
    /// a Poisson count's do, their variance near their mean.
    #[test]
    fn the_ids_are_spread_as_independent_draws_are() {
        let loaded = 1_000_000u64;
        let ids = Ids::new(loaded);
        let stride = u64::MAX / loaded;
        let worst = (0..loaded)
            .step_by(97)
            .map(|n| (ids.of(n) / stride).abs_diff(n))
            .max()
            .unwrap();
        // The greatest deviation of a million sorted uniform draws from
        // their even places is about 0.87 of a thousand keys at the
        // median, and under 300 or over 2,500 one time in thousands.
        assert!(
            (300..2_500).contains(&worst),
            "the farthest an id lies from its even place, in keys: {worst}"
        );
        // Counts in stretches of sixty-four keys' worth: a mean of 64 and
        // a variance near it, where one id a stratum has a variance under
        // one.
        let cells = (loaded / 64) as usize;
        let width = u64::MAX / cells as u64;
        let mut counts = vec![0u32; cells + 1];
        for n in 0..loaded {
            counts[(ids.of(n) / width) as usize] += 1;
        }
        let mean = loaded as f64 / cells as f64;
        let var = counts[..cells]
            .iter()
            .map(|&c| (f64::from(c) - mean).powi(2))
            .sum::<f64>()
            / cells as f64;
        assert!(
            (0.7 * mean..1.3 * mean).contains(&var),
            "the counts' variance {var:.1} beside their mean {mean:.1}"
        );
    }

    #[test]
    fn keys_are_sixteen_digits_and_distinct() {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        db_key_into(1, &mut a);
        db_key_into(2, &mut b);
        assert_eq!(&a, b"0000000000000001");
        assert_ne!(a, b);
    }

    /// The confessed defect, as a test: no two values may share a long prefix.
    #[test]
    fn values_do_not_share_an_identical_prefix() {
        let p = Payload::new(100, 0.5, 7);
        let mut rng = Rng::new(11);
        let mut prefixes = HashSet::new();
        for _ in 0..500 {
            prefixes.insert(p.get(&mut rng)[..50].to_vec());
        }
        assert!(
            prefixes.len() > 400,
            "only {} distinct 50-byte prefixes",
            prefixes.len()
        );
    }

    #[test]
    fn compressibility_is_actually_a_dial() {
        let low = Payload::new(200, 0.0, 3);
        let high = Payload::new(200, 1.0, 3);
        let (mut rl, mut rh) = (Rng::new(1), Rng::new(1));
        let cl = lz4_flex::compress(low.get(&mut rl)).len();
        let ch = lz4_flex::compress(high.get(&mut rh)).len();
        assert!(ch < cl, "compressible={ch} incompressible={cl}");
    }

    #[test]
    fn zipfian_is_skewed_and_uniform_is_not() {
        let n = 10_000u64;
        let mut z = KeyGen::new(KeyDist::Zipfian, n, 1);
        let mut u = KeyGen::new(KeyDist::Uniform, n, 1);
        let (mut zhot, mut uhot) = (0, 0);
        for _ in 0..20_000 {
            if z.next() < n / 100 {
                zhot += 1;
            }
            if u.next() < n / 100 {
                uhot += 1;
            }
        }
        // The hottest 1% of keys should take a large share under Zipfian and
        // about 1% under uniform.
        assert!(zhot > uhot * 5, "zipf hot={zhot} uniform hot={uhot}");
    }

    #[test]
    fn every_generator_stays_in_range() {
        for d in [
            KeyDist::Uniform,
            KeyDist::Zipfian,
            KeyDist::Scrambled,
            KeyDist::Latest,
            KeyDist::Sequential,
        ] {
            for n in [1u64, 2, 1000] {
                let mut g = KeyGen::new(d, n, 5);
                for _ in 0..5000 {
                    assert!(g.next() < n, "{d:?} out of range over {n}");
                }
            }
        }
    }

    /// YCSB's `fnvhash64`, pinned on values computed from its Java: the
    /// zero word, small words, and a word whose hash the Java's `Math.abs`
    /// reads as a negative long before it takes the magnitude.
    #[test]
    fn the_hash_is_ycsbs_fnv() {
        assert_eq!(fnv64(0), 6_284_781_860_667_377_211);
        assert_eq!(fnv64(1), 8_517_097_267_634_966_620);
        assert_eq!(fnv64(2), 1_820_151_046_732_198_393);
        assert_eq!(fnv64(12_345), 1_792_800_413_050_876_852);
        assert_eq!(fnv64(1 << 63), 6_284_922_598_155_788_219);
        assert_eq!(fnv64(u64::MAX), 8_289_690_350_564_177_859);
    }

    /// YCSB's harmonic sum over its ten billion items is the constant it
    /// ships: the first two million terms summed and the rest by the
    /// Euler-Maclaurin tail agree with it to a part in a billion.
    #[test]
    fn the_scrambled_zipfians_constant_is_ycsbs_sum() {
        let th = 0.99f64;
        let m = 2_000_000u64;
        let mut s = 0.0;
        for i in 1..=m {
            s += 1.0 / (i as f64).powf(th);
        }
        let (mf, nf) = (m as f64, SCRAMBLED_ITEMS as f64);
        let tail = (nf.powf(1.0 - th) - mf.powf(1.0 - th)) / (1.0 - th) - 0.5 * mf.powf(-th)
            + 0.5 * nf.powf(-th)
            + th / 12.0 * (mf.powf(-th - 1.0) - nf.powf(-th - 1.0));
        let est = s + tail;
        assert!(
            (est / SCRAMBLED_ZETAN - 1.0).abs() < 1e-9,
            "estimate {est} against YCSB's {SCRAMBLED_ZETAN}"
        );
    }

    /// The scrambled Zipfian keeps the skew and spreads the hot keys: its
    /// hottest key is where YCSB's hash puts rank 0, its twenty hottest
    /// take more than a tenth of the draws, and the lowest hundred keys of
    /// ten thousand take about their uniform share where the plain
    /// Zipfian puts half of everything there.
    #[test]
    fn the_scrambled_zipfian_spreads_the_hot_keys_it_keeps() {
        let n = 10_000u64;
        let draws = 200_000u32;
        let mut s = KeyGen::new(KeyDist::Scrambled, n, 1);
        let mut z = KeyGen::new(KeyDist::Zipfian, n, 1);
        let mut counts = vec![0u32; n as usize];
        let (mut low_s, mut low_z) = (0u32, 0u32);
        for _ in 0..draws {
            let k = s.next();
            counts[k as usize] += 1;
            if k < n / 100 {
                low_s += 1;
            }
            if z.next() < n / 100 {
                low_z += 1;
            }
        }
        assert!(low_s < draws / 100 * 3, "lowest 1% took {low_s} of {draws}");
        assert!(
            low_z > draws / 3,
            "the plain Zipfian's lowest 1% took {low_z}"
        );
        let hottest = (0..n).max_by_key(|&k| counts[k as usize]).unwrap();
        assert_eq!(hottest, fnv64(0) % n);
        let mut sorted = counts.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        let top20: u32 = sorted[..20].iter().sum();
        assert!(
            top20 > draws / 10,
            "the twenty hottest took {top20} of {draws}"
        );
    }

    /// The latest distribution reads the newest keys: every draw is a key
    /// that exists and above zero, the newest is the hottest and the newest
    /// hundred take a quarter of the draws, and once the keys grow the
    /// hottest key moves with them.
    #[test]
    fn the_latest_distribution_follows_the_newest_key() {
        let n = 10_000u64;
        let draws = 50_000u32;
        let mut g = KeyGen::new(KeyDist::Latest, n, 3);
        let hottest_of = |g: &mut KeyGen, keys: u64| {
            let mut counts = vec![0u32; keys as usize];
            for _ in 0..draws {
                let k = g.next();
                assert!(k >= 1 && k < keys, "{k} over {keys}");
                counts[k as usize] += 1;
            }
            let newest: u32 = counts[keys as usize - 100..].iter().sum();
            assert!(newest > draws / 4, "the newest hundred took {newest}");
            (0..keys).max_by_key(|&k| counts[k as usize]).unwrap()
        };
        assert_eq!(hottest_of(&mut g, n), n - 1);
        g.set_keys(n + 1_000);
        assert_eq!(hottest_of(&mut g, n + 1_000), n + 999);
    }

    /// A sum grown a term at a time is the sum: a Zipfian grown to twenty
    /// thousand keys draws as one made over twenty thousand.
    #[test]
    fn a_grown_zipfian_is_the_zipfian_over_the_grown_space() {
        let mut grown = KeyGen::new(KeyDist::Zipfian, 10_000, 9);
        grown.set_keys(20_000);
        let fresh = KeyGen::new(KeyDist::Zipfian, 20_000, 9);
        assert!((grown.zeta_n / fresh.zeta_n - 1.0).abs() < 1e-12);
        assert!((grown.eta / fresh.eta - 1.0).abs() < 1e-12);
        assert_eq!(grown.items, fresh.items);
        let mut grown_latest = KeyGen::new(KeyDist::Latest, 10_000, 9);
        grown_latest.set_keys(20_000);
        let fresh_latest = KeyGen::new(KeyDist::Latest, 20_000, 9);
        assert!((grown_latest.zeta_n / fresh_latest.zeta_n - 1.0).abs() < 1e-12);
        // Shrinking is refused: the keys a store has do not go away.
        grown.set_keys(5);
        assert_eq!(grown.n, 20_000);
    }

    #[test]
    fn sequential_covers_the_space_in_order() {
        let mut g = KeyGen::new(KeyDist::Sequential, 5, 0);
        assert_eq!(
            (0..7).map(|_| g.next()).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 0, 1]
        );
    }
}

/// A pseudorandom permutation of `0..n` in O(1) memory.
///
/// The shuffled load walks the same keys as the ordered one in a random
/// order. A Fisher-Yates vector is 8 bytes a key, which at a full-scale run
/// sized past a machine's memory is itself a memory problem. This is a
/// four-round Feistel network over the smallest power-of-two domain that
/// covers `n`, cycle-walking past outputs at or beyond `n`, so it is a true
/// permutation and costs nothing to hold.
pub struct Permutation {
    n: u64,
    half: u32,
    mask: u64,
    keys: [u64; 4],
}

impl Permutation {
    pub fn new(n: u64, seed: u64) -> Permutation {
        let bits = (64 - n.max(2).saturating_sub(1).leading_zeros()).max(2);
        let half = bits.div_ceil(2);
        let mut r = Rng::new(seed ^ 0xA5A5_5A5A_1234_5678);
        Permutation {
            n,
            half,
            mask: (1u64 << half) - 1,
            keys: [r.next(), r.next(), r.next(), r.next()],
        }
    }

    fn round(&self, x: u64, k: u64) -> u64 {
        let mut h = (x ^ k).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        h & self.mask
    }

    fn encrypt(&self, x: u64) -> u64 {
        let (mut l, mut r) = (x >> self.half, x & self.mask);
        for k in self.keys {
            let nl = r;
            r = l ^ self.round(r, k);
            l = nl;
        }
        (l << self.half) | r
    }

    /// The `i`th key of the shuffled order, for `i < n`.
    pub fn at(&self, i: u64) -> u64 {
        debug_assert!(i < self.n, "Permutation::at({i}) with n = {}", self.n);
        let mut x = self.encrypt(i);
        while x >= self.n {
            x = self.encrypt(x);
        }
        x
    }
}

#[cfg(test)]
mod perm_tests {
    use super::*;

    #[test]
    fn is_a_permutation() {
        for n in [1u64, 2, 3, 7, 100, 1000, 12345] {
            let p = Permutation::new(n, 42);
            let mut seen = vec![false; n as usize];
            for i in 0..n {
                let k = p.at(i);
                assert!(k < n, "n={n} i={i} k={k}");
                assert!(!seen[k as usize], "n={n} repeated {k}");
                seen[k as usize] = true;
            }
        }
    }

    #[test]
    fn is_not_the_identity() {
        let p = Permutation::new(10_000, 1);
        let same = (0..10_000u64).filter(|&i| p.at(i) == i).count();
        assert!(same < 50, "{same} fixed points of 10000");
    }
}
