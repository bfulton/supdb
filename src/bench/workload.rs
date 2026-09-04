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

use super::J;
use crate::jobj;

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
    /// Zipfian with theta 0.99, YCSB's default skew.
    Zipfian,
    /// Ascending. The best case for any structure with sorted layout.
    Sequential,
}

impl KeyDist {
    pub fn parse(s: &str) -> Option<KeyDist> {
        match s {
            "uniform" => Some(KeyDist::Uniform),
            "zipfian" => Some(KeyDist::Zipfian),
            "sequential" => Some(KeyDist::Sequential),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyDist::Uniform => "uniform",
            KeyDist::Zipfian => "zipfian",
            KeyDist::Sequential => "sequential",
        }
    }
}

/// Draws key indices according to a distribution.
///
/// The Zipfian generator is the standard YCSB one (Gray et al.'s bulk-loading
/// method): the expensive harmonic sum is computed once at construction, so
/// drawing a key is a log and a couple of multiplies rather than a scan.
pub struct KeyGen {
    dist: KeyDist,
    n: u64,
    rng: Rng,
    seq: u64,
    zeta_n: f64,
    alpha: f64,
    eta: f64,
    theta: f64,
}

impl KeyGen {
    pub fn new(dist: KeyDist, n: u64, seed: u64) -> KeyGen {
        let theta = 0.99f64;
        let n = n.max(1);
        let zeta_n = if dist == KeyDist::Zipfian {
            zeta(n, theta)
        } else {
            0.0
        };
        let zeta_theta = if dist == KeyDist::Zipfian {
            zeta(2, theta)
        } else {
            0.0
        };
        let alpha = 1.0 / (1.0 - theta);
        let eta = (1.0 - (2.0 / n as f64).powf(1.0 - theta)) / (1.0 - zeta_theta / zeta_n);
        KeyGen {
            dist,
            n,
            rng: Rng::new(seed),
            seq: 0,
            zeta_n,
            alpha,
            eta,
            theta,
        }
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
            KeyDist::Zipfian => {
                let u = self.rng.unit();
                let uz = u * self.zeta_n;
                if uz < 1.0 {
                    return 0;
                }
                if uz < 1.0 + 0.5f64.powf(self.theta) {
                    return 1;
                }
                let v = (self.n as f64 * (self.eta * u - self.eta + 1.0).powf(self.alpha)) as u64;
                v.min(self.n - 1)
            }
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

    pub fn to_json(&self) -> J {
        jobj! {
            "value_size" => J::u(self.value_size as u64),
            "compressibility" => J::fp(self.compressibility, 2),
            "pool_mb" => J::fp(self.pool.len() as f64 / 1048576.0, 2),
        }
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
        for d in [KeyDist::Uniform, KeyDist::Zipfian, KeyDist::Sequential] {
            let mut g = KeyGen::new(d, 1000, 5);
            for _ in 0..5000 {
                assert!(g.next() < 1000, "{d:?} out of range");
            }
        }
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
