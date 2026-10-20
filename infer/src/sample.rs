//! Choosing each token from the logits: greedily, or at a temperature from a seeded
//! generator. Only `libm` and fixed-order sums are used, so the same logits, seed
//! and temperature pick the same tokens on every tier and every machine; the
//! tiers give the same logits bit for bit, so they give the same answers too.

use crate::argmax;

/// Picks tokens, greedily or by drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct Sampler {
    /// Positive and finite to draw; anything else is greedy.
    temperature: f32,
    /// splitmix64's state.
    state: u64,
}

impl Sampler {
    /// Always the most likely token, as [`argmax`] picks it.
    pub const fn greedy() -> Self {
        Self {
            temperature: 0.0,
            state: 0,
        }
    }

    /// Draws each token from the softmax of the logits divided by `temperature`,
    /// with a generator started from `seed`; the same seed draws the same tokens.
    /// A temperature that is not a positive, finite number is [`Sampler::greedy`].
    pub const fn seeded(temperature: f32, seed: u64) -> Self {
        Self {
            temperature,
            state: seed,
        }
    }

    pub fn is_greedy(&self) -> bool {
        !(self.temperature > 0.0 && self.temperature.is_finite())
    }

    /// A token from the whole vocabulary.
    pub fn pick(&mut self, logits: &[f32]) -> u32 {
        if self.is_greedy() {
            return argmax(logits);
        }
        let all = 0..u32::try_from(logits.len()).unwrap_or(u32::MAX);
        self.draw(logits, all, argmax(logits))
    }

    /// One of `candidates`, ids of `logits` in rising order, or none when there
    /// are none. Greedily it is picked as [`argmax`] picks: from the first, moving
    /// only to a strictly larger logit, so a NaN after a number never wins and a
    /// NaN first stays first.
    pub fn pick_from(&mut self, logits: &[f32], candidates: &[u32]) -> Option<u32> {
        let mut best = *candidates.first()?;
        for &id in &candidates[1..] {
            if logits[id as usize] > logits[best as usize] {
                best = id;
            }
        }
        if self.is_greedy() {
            return Some(best);
        }
        Some(self.draw(logits, candidates.iter().copied(), best))
    }

    /// Draws from `ids` with weights `exp((logit - best) / temperature)`, summed in
    /// f64 in id order. A NaN logit weighs nothing, and `best` (weight 1 when its
    /// logit is a number) is the answer when nothing else can be.
    fn draw(&mut self, logits: &[f32], ids: impl Iterator<Item = u32> + Clone, best: u32) -> u32 {
        let (top, temperature) = (logits[best as usize], self.temperature);
        if !top.is_finite() {
            return best;
        }
        let weight = |id: u32| {
            let logit = logits[id as usize];
            match logit.is_nan() {
                true => 0.0,
                false => f64::from(libm::expf((logit - top) / temperature)),
            }
        };
        let total: f64 = ids.clone().map(weight).sum();
        let target = self.unit() * total;
        let mut sum = 0.0f64;
        for id in ids {
            sum += weight(id);
            if target < sum {
                return id;
            }
        }
        best
    }

    /// splitmix64.
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1), from 53 bits.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn greedy_is_argmax_and_ignores_the_seed() {
        let logits = [0.5, 2.0, -1.0, 2.0, f32::NAN];
        for sampler in [Sampler::greedy(), Sampler::seeded(0.0, 7)] {
            let mut sampler = sampler;
            assert!(sampler.is_greedy());
            assert_eq!(sampler.pick(&logits), 1);
            assert_eq!(sampler.pick_from(&logits, &[0, 2, 3, 4]), Some(3));
            assert_eq!(sampler.pick_from(&logits, &[4, 0]), Some(4));
            assert_eq!(sampler.pick_from(&logits, &[]), None);
        }
        for odd in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(Sampler::seeded(odd, 1).is_greedy());
        }
    }

    #[test]
    fn the_same_seed_draws_the_same_tokens() {
        let logits: Vec<f32> = (0..50).map(|i| (i % 7) as f32 * 0.3).collect();
        let draws = |seed| {
            let mut sampler = Sampler::seeded(0.8, seed);
            (0..200).map(|_| sampler.pick(&logits)).collect::<Vec<_>>()
        };
        assert_eq!(draws(42), draws(42));
        assert_ne!(draws(42), draws(43));
        // Every draw is a real token, and more than one token comes up.
        let tokens = draws(42);
        assert!(tokens.iter().all(|&t| (t as usize) < logits.len()));
        assert!(tokens.iter().any(|&t| t != tokens[0]));
    }

    #[test]
    fn draws_follow_the_softmax() {
        // Weights 1, e^-1 and e^-2 at temperature 1.
        let logits = [3.0f32, 2.0, 1.0];
        let mut sampler = Sampler::seeded(1.0, 9);
        let mut counts = [0usize; 3];
        let n = 30_000;
        for _ in 0..n {
            counts[sampler.pick(&logits) as usize] += 1;
        }
        let total = 1.0 + (-1.0f64).exp() + (-2.0f64).exp();
        for (i, &count) in counts.iter().enumerate() {
            let expected = (-(i as f64)).exp() / total;
            let seen = count as f64 / n as f64;
            assert!((seen - expected).abs() < 0.01, "{i}: {seen} vs {expected}");
        }
        // A low temperature all but always takes the best.
        let mut cold = Sampler::seeded(0.05, 9);
        let best = (0..1000).filter(|_| cold.pick(&logits) == 0).count();
        assert!(best > 990, "{best}");
    }

    #[test]
    fn draws_stay_among_the_candidates() {
        let logits = vec![9.0, 1.0, f32::NEG_INFINITY, 1.0, f32::NAN, 0.5];
        let mut sampler = Sampler::seeded(2.0, 3);
        let mut seen = [0; 6];
        for _ in 0..500 {
            let picked = sampler.pick_from(&logits, &[1, 2, 3, 4, 5]).unwrap();
            assert!([1, 3, 5].contains(&picked), "{picked}");
            seen[picked as usize] += 1;
        }
        // A NaN weighs nothing, so the tokens after it still come up.
        assert!(seen[1] > 50 && seen[3] > 50 && seen[5] > 50, "{seen:?}");
        // Nothing but masked or broken logits leaves the first of the highest.
        let broken = [f32::NEG_INFINITY, f32::NAN, f32::NEG_INFINITY];
        assert_eq!(sampler.pick_from(&broken, &[0, 1, 2]), Some(0));
        assert_eq!(sampler.pick(&[f32::NAN, f32::NAN]), 0);
    }
}
