//! Percentiles of TSC samples, in milliseconds.

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// `samples` in TSC cycles, at `per_ms` cycles a millisecond.
pub fn summary(samples: &[u64], per_ms: u64) -> Summary {
    if samples.is_empty() {
        return Summary::default();
    }
    let mut sorted: Vec<u64> = samples.to_vec();
    sorted.sort_unstable();
    let ms = |c: u64| c as f64 / per_ms.max(1) as f64;
    // Nearest rank: the smallest sample at or above the fraction.
    let at = |q: f64| {
        let rank = libm_ceil(q * sorted.len() as f64) as usize;
        ms(sorted[rank.clamp(1, sorted.len()) - 1])
    };
    Summary {
        n: sorted.len(),
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
        max: ms(sorted[sorted.len() - 1]),
    }
}

fn libm_ceil(x: f64) -> f64 {
    let t = x as u64 as f64;
    if t < x { t + 1.0 } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let samples: Vec<u64> = (1..=100).collect();
        let s = summary(&samples, 1);
        assert_eq!((s.p50, s.p95, s.p99, s.max), (50.0, 95.0, 99.0, 100.0));
    }
}
