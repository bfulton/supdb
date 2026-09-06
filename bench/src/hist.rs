//! A latency histogram.
//!
//! The design document reports throughput means and nothing else. For an
//! ingest engine that is the wrong summary: `merge_key` decompresses,
//! concatenates, recompresses and writes a key's whole run synchronously while
//! holding both the shard lock and the appender lock, so the cost of the
//! design lands on an occasional unlucky `append` rather than on the average
//! one. A mean cannot show that. A p99.9 can.
//!
//! HdrHistogram's scheme, implemented directly: values are bucketed by
//! magnitude with a fixed number of linear sub-buckets inside each, giving
//! constant relative precision across the whole range at bounded memory.

/// Sub-buckets per power of two. 128 gives worst-case ~0.8% relative error,
/// which is finer than any conclusion drawn from these numbers.
const SUB_BITS: u32 = 7;
const SUB: usize = 1 << SUB_BITS;

/// Percentiles reported by default. p99.9 and p99.99 are the point of the
/// exercise, so they are not optional.
pub const REPORTED: &[f64] = &[50.0, 90.0, 99.0, 99.9, 99.99];

#[derive(Clone)]
pub struct Hist {
    counts: Vec<u64>,
    total: u64,
    max: u64,
    min: u64,
    sum: u128,
}

impl Default for Hist {
    fn default() -> Self {
        Self::new()
    }
}

impl Hist {
    pub fn new() -> Hist {
        Hist {
            counts: vec![0; 64 * SUB],
            total: 0,
            max: 0,
            min: u64::MAX,
            sum: 0,
        }
    }

    #[inline]
    fn index(v: u64) -> usize {
        if v < SUB as u64 {
            return v as usize;
        }
        // Bucket by magnitude, then linearly within it.
        let mag = 63 - v.leading_zeros();
        let shift = mag - SUB_BITS;
        let sub = (v >> shift) as usize & (SUB - 1);
        ((shift as usize) + 1) * SUB + sub
    }

    /// Smallest value that lands in `i`; used to report a bucket as a number.
    fn value_at(i: usize) -> u64 {
        if i < SUB {
            return i as u64;
        }
        let shift = (i / SUB - 1) as u32;
        let sub = (i % SUB) as u64;
        (SUB as u64 | sub) << shift
    }

    /// Record one observation, in nanoseconds.
    #[inline]
    pub fn record(&mut self, nanos: u64) {
        let i = Self::index(nanos);
        if i < self.counts.len() {
            self.counts[i] += 1;
            self.total += 1;
            self.sum += nanos as u128;
            if nanos > self.max {
                self.max = nanos;
            }
            if nanos < self.min {
                self.min = nanos;
            }
        }
    }

    pub fn len(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    pub fn max(&self) -> u64 {
        self.max
    }

    pub fn min(&self) -> u64 {
        if self.total == 0 {
            0
        } else {
            self.min
        }
    }

    pub fn mean(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.sum as f64 / self.total as f64
        }
    }

    /// The value at the given percentile, in nanoseconds.
    pub fn percentile(&self, p: f64) -> u64 {
        if self.total == 0 {
            return 0;
        }
        // Integer arithmetic on a parts-per-million percentile.
        //
        // The obvious `((p / 100.0) * total).ceil()` is wrong at exact
        // boundaries: 99.9/100.0 is 0.9990000000000001 in binary floating
        // point, so at a total of 100,000 it ceilings to 99,901 instead of
        // 99,900 and the reported percentile skips into the next populated
        // bucket. For a tail-latency histogram that silently *overstates* the
        // tail -- the one direction a measurement tool must never err in.
        let ppm = (p.clamp(0.0, 100.0) * 10_000.0).round() as u128;
        let want = ((ppm * self.total as u128).div_ceil(1_000_000) as u64).max(1);
        let mut seen = 0u64;
        for (i, c) in self.counts.iter().enumerate() {
            if *c == 0 {
                continue;
            }
            seen += c;
            if seen >= want {
                // The bucket's upper edge: reporting the lower edge would
                // understate every percentile by up to the bucket width.
                let next = Self::value_at(i + 1);
                return next.saturating_sub(1).max(Self::value_at(i)).min(self.max);
            }
        }
        self.max
    }

    /// Merge another histogram in, for combining per-thread recordings.
    pub fn merge(&mut self, other: &Hist) {
        for (i, c) in other.counts.iter().enumerate() {
            self.counts[i] += c;
        }
        self.total += other.total;
        self.sum += other.sum;
        self.max = self.max.max(other.max);
        if other.total > 0 {
            self.min = self.min.min(other.min);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_are_within_precision() {
        let mut h = Hist::new();
        for v in 1..=100_000u64 {
            h.record(v);
        }
        let p50 = h.percentile(50.0) as f64;
        assert!((p50 - 50_000.0).abs() / 50_000.0 < 0.02, "p50={p50}");
        let p99 = h.percentile(99.0) as f64;
        assert!((p99 - 99_000.0).abs() / 99_000.0 < 0.02, "p99={p99}");
    }

    /// The case the whole module exists for: a rare enormous stall must be
    /// visible in the tail even though it barely moves the mean.
    #[test]
    fn a_rare_stall_shows_in_the_tail_and_not_the_mean() {
        let mut h = Hist::new();
        for _ in 0..99_900 {
            h.record(1_000); // 1us
        }
        for _ in 0..100 {
            h.record(500_000_000); // 500ms, one in a thousand
        }
        // The mean is dragged to ~0.5ms and reads as "half a millisecond,
        // fine" -- it never reveals that one call in a thousand took half a
        // second.
        assert!(h.mean() < 1_000_000.0, "mean hides it: {}", h.mean());
        // p99.9 sits exactly on the boundary: the 99,900th value is the last
        // fast one, so the tail only opens past it. That is arithmetic, not a
        // bug, and it is why p99.99 is reported and not just p99.9.
        // Reported as the bucket's upper edge, so it is >= the recorded
        // value and within one bucket width of it -- never understated.
        assert!(
            (1_000..1_100).contains(&h.percentile(99.9)),
            "p99.9 should stay on the fast bucket, got {}",
            h.percentile(99.9)
        );
        assert!(
            h.percentile(99.99) > 100_000_000,
            "p99.99 must expose the stall"
        );
        assert!(h.max() >= 500_000_000);
    }

    #[test]
    fn merge_preserves_total_and_extremes() {
        let (mut a, mut b) = (Hist::new(), Hist::new());
        a.record(10);
        a.record(20);
        b.record(5);
        b.record(9_999_999);
        a.merge(&b);
        assert_eq!(a.len(), 4);
        assert_eq!(a.min(), 5);
        assert!(a.max() >= 9_999_999);
    }

    #[test]
    fn empty_histogram_is_not_a_panic() {
        let h = Hist::new();
        assert_eq!(h.percentile(99.9), 0);
        assert_eq!(h.len(), 0);
        assert_eq!(h.min(), 0);
    }

    #[test]
    fn small_values_are_exact() {
        let mut h = Hist::new();
        for v in 0..SUB as u64 {
            h.record(v);
        }
        assert_eq!(h.percentile(100.0), SUB as u64 - 1);
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    /// Regression: `(99.9 / 100.0) * 100_000.0` ceilings to 99,901 rather than
    /// 99,900 in binary floating point, which pushed the reported percentile
    /// into the next populated bucket and overstated the tail.
    #[test]
    fn exact_percentile_boundaries_do_not_skip_a_bucket() {
        let mut h = Hist::new();
        for _ in 0..99_900 {
            h.record(1_000);
        }
        for _ in 0..100 {
            h.record(500_000_000);
        }
        assert!(
            h.percentile(99.9) < 2_000,
            "p99.9 must stay on the fast bucket"
        );
        assert!(
            h.percentile(99.99) > 100_000_000,
            "p99.99 must reach the stalls"
        );
        assert_eq!(h.percentile(100.0), h.max());
    }

    #[test]
    fn every_reported_percentile_is_monotonic() {
        let mut h = Hist::new();
        for v in 1..=50_000u64 {
            h.record(v * 3);
        }
        let mut last = 0;
        for p in REPORTED.iter().chain(std::iter::once(&100.0)) {
            let v = h.percentile(*p);
            assert!(v >= last, "p{p} = {v} went backwards from {last}");
            last = v;
        }
    }
}
