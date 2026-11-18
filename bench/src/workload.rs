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
