//! The benchmark suite: a time series of measurements of supdb against the engines a
//! user would otherwise pick.
//!
//! `DESIGN.md` is the specification. In one sentence: a run appends a row
//! under `runs/<scale>/`, every quantity is a curve over a ladder of store
//! sizes, and a regression is a row whose error bars lie entirely on the
//! worse side of the last ten rows' on the same machine class.

pub mod engines;
pub mod env;
pub mod figures;
pub mod gate;
pub mod hist;
pub mod machine;
pub mod row;
pub mod run;
pub mod stats;
pub mod workload;

/// How big a run is. Chosen by whoever starts it; it is in the row's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    /// Gates a pull request. The ladder's top is as high as fits in two
    /// minutes on a GitHub runner, measured once and then fixed.
    Quick,
    /// The number. The ladder's top is the rung at which the store is at
    /// least 1.5x the machine's memory, so the curve crosses the memory line
    /// wherever it runs.
    Full,
}

impl Scale {
    pub fn parse(s: &str) -> Option<Scale> {
        match s {
            "quick" => Some(Scale::Quick),
            "full" => Some(Scale::Full),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Scale::Quick => "quick",
            Scale::Full => "full",
        }
    }
    /// Repetitions per (size, arm). Five at quick, seven at full. With this
    /// few samples a bootstrap CI of the median is essentially the sample
    /// range -- coarse, true, and conservative in the direction a gate on a
    /// noisy machine should be.
    pub fn reps(&self) -> usize {
        match self {
            Scale::Quick => 5,
            Scale::Full => 7,
        }
    }
}

/// The size ladder: 1, 3, 10, 30 ... x 10^4 keys, up to and including the
/// first rung at or above `top`. A geometric ladder costs about 1.5x its
/// largest rung, so the curve is nearly free next to the top rung alone.
pub fn ladder(top: u64) -> Vec<u64> {
    let mut out = Vec::new();
    let mut rung = 10_000u64;
    let mut odd = false;
    loop {
        out.push(rung);
        if rung >= top {
            break;
        }
        rung = if odd { rung * 10 / 3 } else { rung * 3 };
        odd = !odd;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_one_three_ten() {
        assert_eq!(ladder(10_000), vec![10_000]);
        assert_eq!(ladder(100_000), vec![10_000, 30_000, 100_000]);
        assert_eq!(
            ladder(1_000_000),
            vec![10_000, 30_000, 100_000, 300_000, 1_000_000]
        );
        // A top between rungs rounds up to the next rung, never down.
        assert_eq!(ladder(50_000), vec![10_000, 30_000, 100_000]);
    }
}
