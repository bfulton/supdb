//! Repetition, dispersion, and the significance gate.
//!
//! The design document established that this machine's write path has a
//! standard deviation of roughly 55,000 ops/s, and stated the consequence:
//! "nothing under ~15% means anything without repetition." It then reported a
//! 13.9% difference as a win, with no error bars. That is the failure this
//! module exists to make impossible.
//!
//! The rule enforced here is deliberately conservative and has two parts, both
//! of which must pass before a difference may be called one:
//!
//!   1. A two-sided Mann-Whitney U test at p < 0.05. Non-parametric, because
//!      throughput samples are skewed by scheduler and page-cache effects and
//!      are not normally distributed -- a t-test would over-report.
//!   2. A minimum effect size on the medians. Statistical significance on a
//!      2% difference is not a result anyone should build on; with enough runs
//!      any bias becomes detectable, so the effect floor is what keeps
//!      "significant" and "meaningful" from drifting apart.
//!
//! Interleaving matters as much as the test. Running all of A then all of B
//! confounds the comparison with anything that drifts over the run -- thermal
//! state, page cache, another tenant. `Trial` therefore drives configurations
//! round-robin rather than in blocks.

use super::J;
use crate::jobj;

/// The default minimum effect size: a 5% difference in medians.
///
/// Lower than the document's 15% rule of thumb, because that figure was the
/// noise of a *single unrepeated* run. With interleaved repetition the noise
/// floor is measured per comparison rather than assumed, and the U test
/// handles the rest.
pub const MIN_EFFECT: f64 = 0.05;

/// The default number of repetitions. Seven is the smallest n at which a
/// two-sided Mann-Whitney U test can reach p < 0.05 against another sample of
/// seven at all -- with fewer, no difference is ever reportable, which would
/// make the gate vacuous rather than strict.
pub const DEFAULT_REPS: usize = 7;

/// An ordered set of measurements of one quantity.
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

    /// The `q`-quantile by linear interpolation between order statistics.
    pub fn quantile(&self, q: f64) -> f64 {
        let v = self.sorted();
        if v.is_empty() {
            return f64::NAN;
        }
        if v.len() == 1 {
            return v[0];
        }
        let pos = q.clamp(0.0, 1.0) * (v.len() - 1) as f64;
        let lo = pos.floor() as usize;
        let hi = pos.ceil() as usize;
        if lo == hi {
            v[lo]
        } else {
            v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
        }
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

    /// Interquartile range. Reported instead of standard deviation because the
    /// distributions here are skewed and a single slow run should widen the
    /// reported spread without dragging the centre with it.
    pub fn iqr(&self) -> f64 {
        self.quantile(0.75) - self.quantile(0.25)
    }

    /// Relative IQR, as a fraction of the median. This is the honest one-number
    /// answer to "how noisy was this measurement".
    pub fn rel_iqr(&self) -> f64 {
        let m = self.median();
        if m == 0.0 {
            f64::NAN
        } else {
            self.iqr() / m
        }
    }

    pub fn mean(&self) -> f64 {
        if self.values.is_empty() {
            return f64::NAN;
        }
        self.values.iter().sum::<f64>() / self.values.len() as f64
    }

    /// A deterministic bootstrap percentile CI for the median.
    ///
    /// The RNG is seeded from the sample values themselves, so re-running the
    /// analysis over committed results reproduces the same interval. A
    /// confidence interval that moves when you recompute it is not evidence.
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
        let s = Samples::new(meds);
        let tail = (1.0 - conf) / 2.0;
        (s.quantile(tail), s.quantile(1.0 - tail))
    }

    pub fn to_json(&self) -> J {
        let (lo, hi) = self.median_ci(0.95, 2000);
        jobj! {
            "n" => J::u(self.len() as u64),
            "median" => J::fp(self.median(), 2),
            "iqr" => J::fp(self.iqr(), 2),
            "rel_iqr" => J::fp(self.rel_iqr(), 4),
            "min" => J::fp(self.min(), 2),
            "max" => J::fp(self.max(), 2),
            "ci95_lo" => J::fp(lo, 2),
            "ci95_hi" => J::fp(hi, 2),
            "values" => J::arr(self.values.iter().map(|v| J::fp(*v, 2)).collect()),
        }
    }
}

/// The outcome of comparing two sets of measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// `a` is meaningfully greater than `b`.
    Greater,
    /// `a` is meaningfully less than `b`.
    Less,
    /// The data does not support calling these different. This is the honest
    /// answer far more often than benchmark write-ups admit.
    NoDifference,
    /// Not enough samples to decide either way.
    Underpowered,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Greater => "greater",
            Verdict::Less => "less",
            Verdict::NoDifference => "no_difference",
            Verdict::Underpowered => "underpowered",
        }
    }
}

/// A full comparison: the verdict, the effect, and the evidence behind both.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub verdict: Verdict,
    /// Median of `a` divided by median of `b`.
    pub ratio: f64,
    pub p_value: f64,
    pub min_effect: f64,
    pub a: Samples,
    pub b: Samples,
}

impl Comparison {
    pub fn to_json(&self) -> J {
        jobj! {
            "verdict" => J::s(self.verdict.as_str()),
            "ratio" => J::fp(self.ratio, 4),
            "p_value" => J::fp(self.p_value, 5),
            "min_effect" => J::fp(self.min_effect, 3),
            "a" => self.a.to_json(),
            "b" => self.b.to_json(),
        }
    }

    /// A one-line human summary that refuses to overstate.
    pub fn summary(&self, a_name: &str, b_name: &str) -> String {
        match self.verdict {
            Verdict::NoDifference => format!(
                "{a_name} vs {b_name}: NO DIFFERENCE (ratio {:.3}, p={:.4}) \
                 -- within noise, not a result",
                self.ratio, self.p_value
            ),
            Verdict::Underpowered => format!(
                "{a_name} vs {b_name}: UNDERPOWERED (n={}, {}) -- cannot decide",
                self.a.len(),
                self.b.len()
            ),
            _ => format!(
                "{a_name} vs {b_name}: {} {:.3}x (p={:.4}, rel_iqr {:.1}%/{:.1}%)",
                self.verdict.as_str(),
                self.ratio,
                self.p_value,
                self.a.rel_iqr() * 100.0,
                self.b.rel_iqr() * 100.0
            ),
        }
    }
}

/// Compare two samples through the gate.
pub fn compare(a: &Samples, b: &Samples, min_effect: f64) -> Comparison {
    let ratio = a.median() / b.median();
    let p = mann_whitney_p(&a.values, &b.values);
    // A sample of fewer than five cannot reach p < 0.05 two-sided against
    // another of five, so report that rather than a verdict the data cannot
    // carry.
    let verdict = if a.len() < 5 || b.len() < 5 {
        Verdict::Underpowered
    } else if p >= 0.05 || !(ratio - 1.0).abs().ge(&min_effect) {
        Verdict::NoDifference
    } else if ratio > 1.0 {
        Verdict::Greater
    } else {
        Verdict::Less
    };
    Comparison {
        verdict,
        ratio,
        p_value: p,
        min_effect,
        a: a.clone(),
        b: b.clone(),
    }
}

/// Two-sided Mann-Whitney U, normal approximation with tie correction.
///
/// Chosen over Welch's t because throughput samples are not normal: a
/// scheduler hiccup or a page-cache miss produces a long left tail, and a
/// parametric test reads that tail as a shifted mean.
pub fn mann_whitney_p(a: &[f64], b: &[f64]) -> f64 {
    let (n1, n2) = (a.len(), b.len());
    if n1 == 0 || n2 == 0 {
        return 1.0;
    }
    // Rank the pooled sample, averaging ranks across ties.
    let mut pooled: Vec<(f64, bool)> = a
        .iter()
        .map(|v| (*v, true))
        .chain(b.iter().map(|v| (*v, false)))
        .collect();
    pooled.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    let n = pooled.len();
    let mut ranks = vec![0.0f64; n];
    let mut tie_term = 0.0f64;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && pooled[j + 1].0 == pooled[i].0 {
            j += 1;
        }
        let count = (j - i + 1) as f64;
        let avg = ((i + 1 + j + 1) as f64) / 2.0;
        for r in ranks.iter_mut().take(j + 1).skip(i) {
            *r = avg;
        }
        if count > 1.0 {
            tie_term += count * count * count - count;
        }
        i = j + 1;
    }

    let r1: f64 = ranks
        .iter()
        .zip(&pooled)
        .filter(|(_, p)| p.1)
        .map(|(r, _)| *r)
        .sum();
    let n1f = n1 as f64;
    let n2f = n2 as f64;
    let u1 = r1 - n1f * (n1f + 1.0) / 2.0;
    let u = u1.min(n1f * n2f - u1);

    let mu = n1f * n2f / 2.0;
    let nf = n as f64;
    let var = (n1f * n2f / 12.0) * ((nf + 1.0) - tie_term / (nf * (nf - 1.0)));
    if var <= 0.0 {
        return 1.0;
    }
    // Continuity correction, then two-sided.
    let z = ((u - mu).abs() - 0.5) / var.sqrt();
    let p = 2.0 * (1.0 - normal_cdf(z.max(0.0)));
    p.clamp(0.0, 1.0)
}

/// Standard normal CDF via the Abramowitz-Stegun 7.1.26 erf approximation.
/// Absolute error below 1.5e-7, which is far finer than any decision made on it.
fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Drives repetitions of several configurations, interleaved.
///
/// Blocked execution (all of A, then all of B) confounds a comparison with
/// anything that drifts across the run. Round-robin spreads that drift evenly
/// over every configuration, so it inflates the noise rather than the effect.
pub struct Trial {
    pub reps: usize,
    pub warmup: usize,
}

impl Trial {
    pub fn new(reps: usize) -> Trial {
        Trial { reps, warmup: 1 }
    }

    /// Run `f(config_index, rep)` for every configuration, round-robin, and
    /// collect one sample per configuration per repetition.
    ///
    /// Warmup repetitions are executed and discarded: the first touch of a
    /// fresh file pays for allocation and first-fault costs that no steady
    /// state repeats.
    pub fn run<F>(&self, configs: usize, mut f: F) -> Vec<Samples>
    where
        F: FnMut(usize, usize) -> f64,
    {
        let mut out = vec![Samples::default(); configs];
        for w in 0..self.warmup {
            for c in 0..configs {
                let _ = f(c, w);
            }
        }
        for rep in 0..self.reps {
            for (c, samples) in out.iter_mut().enumerate() {
                samples.push(f(c, self.warmup + rep));
            }
        }
        out
    }
}

/// Least squares fit of `y = a + b*x`.
///
/// Neither coefficient is a quantity unless the data is actually affine, and
/// that is a claim about the data like any other. `ext-sweep` fitted this over
/// scan lengths 1..400 and reported `a` as an engine's fixed cost per scan,
/// across a range where the marginal cost of one more entry falls from about
/// 89ns to 15 before settling near 20. It read 952ns for a scan that had been
/// observed to finish in 692. Check the intercept against the smallest point
/// you measured before calling it a floor -- see the test below, which carries
/// the curve that refuted it.
pub fn affine_fit(xs: &[f64], ys: &[f64]) -> (f64, f64) {
    let k = xs.len() as f64;
    let sx: f64 = xs.iter().sum();
    let sy: f64 = ys.iter().sum();
    let sxx: f64 = xs.iter().map(|x| x * x).sum();
    let sxy: f64 = xs.iter().zip(ys).map(|(x, y)| x * y).sum();
    let d = k * sxx - sx * sx;
    if d.abs() < 1e-12 {
        return (sy / k, 0.0);
    }
    let b = (k * sxy - sx * sy) / d;
    ((sy - b * sx) / k, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_interpolate() {
        let s = Samples::new(vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(s.median(), 2.5);
        assert_eq!(s.min(), 1.0);
        assert_eq!(s.max(), 4.0);
    }

    #[test]
    fn identical_samples_are_not_a_difference() {
        let a = Samples::new(vec![100.0; 8]);
        let b = Samples::new(vec![100.0; 8]);
        assert_eq!(compare(&a, &b, MIN_EFFECT).verdict, Verdict::NoDifference);
    }

    /// The regression this whole module exists to prevent: a 13.9% median
    /// difference buried in noise of the same magnitude must not be reported
    /// as a win.
    #[test]
    fn noisy_fourteen_percent_is_not_a_win() {
        let a = Samples::new(vec![
            485_479.0, 402_000.0, 530_000.0, 441_000.0, 505_000.0, 398_000.0, 512_000.0,
        ]);
        let b = Samples::new(vec![
            426_374.0, 470_000.0, 389_000.0, 501_000.0, 412_000.0, 455_000.0, 433_000.0,
        ]);
        let c = compare(&a, &b, MIN_EFFECT);
        assert_eq!(
            c.verdict,
            Verdict::NoDifference,
            "p={} ratio={}",
            c.p_value,
            c.ratio
        );
    }

    /// A real, clean separation must still be reported, or the gate is useless.
    #[test]
    fn clean_separation_is_reported() {
        let a = Samples::new(vec![1000.0, 1010.0, 990.0, 1005.0, 995.0, 1002.0, 998.0]);
        let b = Samples::new(vec![500.0, 505.0, 495.0, 502.0, 498.0, 501.0, 499.0]);
        let c = compare(&a, &b, MIN_EFFECT);
        assert_eq!(c.verdict, Verdict::Greater);
        assert!(c.p_value < 0.05, "p={}", c.p_value);
        assert!((c.ratio - 2.0).abs() < 0.02);
    }

    /// Statistically detectable but too small to build on.
    #[test]
    fn tiny_but_consistent_effect_is_gated_by_effect_size() {
        let a = Samples::new((0..12).map(|i| 1000.0 + i as f64 * 0.1).collect());
        let b = Samples::new((0..12).map(|i| 990.0 + i as f64 * 0.1).collect());
        let c = compare(&a, &b, MIN_EFFECT);
        assert_eq!(c.verdict, Verdict::NoDifference, "1% effect must not pass");
    }

    #[test]
    fn small_samples_are_underpowered_not_confident() {
        let a = Samples::new(vec![100.0, 101.0, 99.0]);
        let b = Samples::new(vec![50.0, 51.0, 49.0]);
        assert_eq!(compare(&a, &b, MIN_EFFECT).verdict, Verdict::Underpowered);
    }

    #[test]
    fn mann_whitney_matches_known_case() {
        // Fully separated samples of 7 give the smallest attainable U.
        let p = mann_whitney_p(
            &[1., 2., 3., 4., 5., 6., 7.],
            &[8., 9., 10., 11., 12., 13., 14.],
        );
        assert!(p < 0.01, "p={p}");
    }

    #[test]
    fn bootstrap_ci_is_deterministic() {
        let s = Samples::new(vec![10.0, 12.0, 11.0, 13.0, 9.0, 11.5, 10.5]);
        assert_eq!(s.median_ci(0.95, 500), s.median_ci(0.95, 500));
    }

    #[test]
    fn trial_interleaves_configurations() {
        let order = std::cell::RefCell::new(Vec::new());
        let t = Trial { reps: 3, warmup: 0 };
        t.run(2, |c, _| {
            order.borrow_mut().push(c);
            1.0
        });
        assert_eq!(
            *order.borrow(),
            vec![0, 1, 0, 1, 0, 1],
            "must be round-robin, not blocked"
        );
    }
    /// A fitted intercept is not a floor. These are Supdb's measured scan
    /// costs at lengths 1, 2, 5, 10, 25, 50, 100, 200 and 400 from
    /// `results/ext-sweep.full.json`. A least-squares line through them puts
    /// the per-scan constant 261ns ABOVE the one-entry scan that was actually
    /// observed -- a constant larger than the cheapest thing it is supposed to
    /// bound. `ext-sweep` reported that number as Supdb's fixed cost, and
    /// compared it against LMDB's version of the same artifact, for as long as
    /// it fitted rather than measured. Both quantities are read off the curve
    /// directly now; this keeps the case that made the difference visible.
    #[test]
    fn a_fitted_intercept_is_not_a_floor() {
        let xs = [1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 400.0];
        let ys = [
            691.6, 781.0, 969.6, 1218.1, 1678.3, 2243.7, 2995.4, 5009.1, 8894.7,
        ];
        let (a, b) = affine_fit(&xs, &ys);
        assert!(
            b > 19.0 && b < 21.0,
            "the slope was always about right; it is the intercept that breaks: b={b}"
        );
        assert!(
            a > ys[0],
            "if this ever stops holding the curve changed, not the argument: a={a} floor={}",
            ys[0]
        );
        // The size of the lie, not merely its direction.
        assert!(
            (a - ys[0]) > 250.0,
            "intercept exceeds the floor by only {}",
            a - ys[0]
        );
    }
}
