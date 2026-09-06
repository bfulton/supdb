//! The statistics a row is read with. Rows store raw samples; everything
//! here is computed from them at read time.

/// An ordered set of measurements of one quantity, one per repetition.
#[derive(Clone, Debug, Default)]
pub struct Samples {
    pub values: Vec<f64>,
}

impl Samples {
    pub fn new(values: Vec<f64>) -> Samples {
        Samples { values }
    }

    pub fn push(&mut self, v: f64) {
        self.values.push(v);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn sorted(&self) -> Vec<f64> {
        let mut v = self.values.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// Linear interpolation between order statistics.
    pub fn quantile(&self, q: f64) -> f64 {
        let v = self.sorted();
        if v.is_empty() {
            return f64::NAN;
        }
        let pos = q.clamp(0.0, 1.0) * (v.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
    }

    pub fn median(&self) -> f64 {
        self.quantile(0.5)
    }

    pub fn min(&self) -> f64 {
        self.sorted().first().copied().unwrap_or(f64::NAN)
    }

    pub fn max(&self) -> f64 {
        self.sorted().last().copied().unwrap_or(f64::NAN)
    }

    /// Interquartile range, as a fraction of the median: the one-number
    /// answer to "how noisy was this".
    pub fn rel_iqr(&self) -> f64 {
        let m = self.median();
        if m == 0.0 {
            f64::NAN
        } else {
            (self.quantile(0.75) - self.quantile(0.25)) / m
        }
    }

    /// A percentile bootstrap CI for the median, deterministic.
    ///
    /// The RNG is seeded from the sample values, so reading the same row
    /// twice gives the same interval -- a confidence interval that moves when
    /// you recompute it is not evidence. With five to seven samples the
    /// interval is essentially the sample range: coarse, true, and
    /// conservative in the direction a gate on a noisy machine should be.
    pub fn median_ci(&self, conf: f64, resamples: usize) -> (f64, f64) {
        let n = self.values.len();
        if n < 2 {
            let m = self.median();
            return (m, m);
        }
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for v in &self.values {
            seed ^= v.to_bits();
            seed = seed.wrapping_mul(0x1000_0000_01b3).rotate_left(27);
        }
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut meds = Vec::with_capacity(resamples);
        let mut buf = vec![0.0f64; n];
        for _ in 0..resamples {
            for slot in buf.iter_mut() {
                *slot = self.values[(next() % n as u64) as usize];
            }
            buf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            meds.push(if n % 2 == 1 {
                buf[n / 2]
            } else {
                (buf[n / 2 - 1] + buf[n / 2]) / 2.0
            });
        }
        let meds = Samples::new(meds);
        let tail = (1.0 - conf) / 2.0;
        (meds.quantile(tail), meds.quantile(1.0 - tail))
    }
}

/// The confidence and resample count every reader uses. One place.
pub const CI_CONF: f64 = 0.95;
pub const CI_RESAMPLES: usize = 2000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even() {
        assert_eq!(Samples::new(vec![3.0, 1.0, 2.0]).median(), 2.0);
        assert_eq!(Samples::new(vec![4.0, 1.0, 3.0, 2.0]).median(), 2.5);
    }

    #[test]
    fn ci_is_deterministic_and_inside_the_range() {
        let s = Samples::new(vec![10.0, 12.0, 11.0, 13.0, 9.0]);
        let a = s.median_ci(CI_CONF, CI_RESAMPLES);
        let b = s.median_ci(CI_CONF, CI_RESAMPLES);
        assert_eq!(a, b);
        assert!(a.0 >= 9.0 && a.1 <= 13.0 && a.0 <= a.1);
    }
}
