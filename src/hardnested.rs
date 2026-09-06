// SPDX-License-Identifier: GPL-2.0-or-later
//! Hardnested attack building blocks — Crypto1 sum-property foundations.
//!
//! Pure-Rust port of the sum-property mathematics from
//! nfc-tools/mfoc-hardnested `src/cmdhfmfhard.c` (commit a6007437405a),
//! GPL-2.0-or-later. No C code is linked, called, or bundled; only the
//! algorithm is reimplemented in safe Rust. See THIRD_PARTY.md.
//!
//! Background: C. Meijer and R. Verdult, "Ciphertext-only Cryptanalysis on
//! Hardened Mifare Classic Cards" (ACM CCS 2015). The full hardnested attack
//! (nonce collection over hardened PRNG cards, bit-flip state tables, and
//! Bayesian state-space reduction) is NOT implemented here yet — this module
//! provides only the sum-property primitives every later stage builds on.

use crate::crypto1::{Crypto1, filter};

/// The 19 possible values of the Crypto1 sum property `a` (used for `a0`/`a8`).
///
/// Mirrors `sums[NUM_SUMS]` in the reference implementation.
pub const SUMS: [u16; 19] = [
    0, 32, 56, 64, 80, 96, 104, 112, 120, 128, 136, 144, 152, 160, 176, 192, 200, 224, 256,
];

/// Number of distinct partial-sum-property values.
///
/// `partial_sum_property` returns an even number in `0..=16`; dividing by two
/// yields an index in `0..=8`, i.e. `NUM_PART_SUMS` distinct values.
pub const NUM_PART_SUMS: usize = 9;

/// Which half of the split Crypto1 state a partial sum is computed over.
///
/// The odd half is filtered one extra time (and its parity flips), matching the
/// alternating odd/even feed of the Crypto1 filter function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    Even,
    Odd,
}

/// Partial sum property of one split half-state.
///
/// Returns an even value in `0..=16`. Port of `PartialSumProperty`.
pub fn partial_sum_property(state: u32, half: Half) -> u16 {
    let mut sum = 0u16;
    for j in 0..16u32 {
        let mut st = state;
        let mut part = 0u32;
        match half {
            Half::Odd => {
                part ^= filter(st);
                for i in 0..4u32 {
                    st = (st << 1) | ((j >> (3 - i)) & 1);
                    part ^= filter(st);
                }
                // The remaining 8 filtered bits cancel to a constant 1.
                part ^= 1;
            }
            Half::Even => {
                for i in 0..4u32 {
                    st = (st << 1) | ((j >> (3 - i)) & 1);
                    part ^= filter(st);
                }
            }
        }
        sum += part as u16;
    }
    sum
}

/// Combine the odd and even partial sums into the full sum property.
///
/// Both inputs are the even values in `0..=16` returned by
/// [`partial_sum_property`]. Port of the `2*p*(16-2*q) + (16-2*p)*2*q`
/// combination (here `p`/`q` are already doubled).
pub fn combine_sum(part_odd: u16, part_even: u16) -> u16 {
    part_odd * (16 - part_even) + (16 - part_odd) * part_even
}

/// Index of a full sum value within [`SUMS`], or `None` if it is not a legal
/// sum property.
pub fn sum_index(sum: u16) -> Option<usize> {
    SUMS.iter().position(|&s| s == sum)
}

/// Full sum property `a0` of a complete Crypto1 state.
pub fn sum_property(state: &Crypto1) -> u16 {
    combine_sum(
        partial_sum_property(state.odd, Half::Odd),
        partial_sum_property(state.even, Half::Even),
    )
}

/// Total number of distinct nonce first-byte values (`N` in the hypergeometric
/// model): each Crypto1 nonce first byte is one of 256 values.
pub const NUM_FIRST_BYTES: u16 = 256;

/// Hypergeometric probability `P(T = k | S = sums[i_k])`.
///
/// Models drawing `n` nonce first bytes without replacement from a population
/// of [`NUM_FIRST_BYTES`] in which `K = SUMS[i_k]` have the odd-parity bit set,
/// observing `k` of them. Port of `p_hypergeometric`, using logarithms in the
/// boundary cases to avoid factorial overflow and the published recursion
/// elsewhere. All arithmetic is done in `i64` so the `n - k` / `N - K - n + k`
/// terms never underflow.
pub fn p_hypergeometric(i_k: usize, n: u16, k: u16) -> f64 {
    let n_total: i64 = NUM_FIRST_BYTES as i64;
    let big_k: i64 = SUMS[i_k] as i64;
    let n = n as i64;
    let k = k as i64;
    if n - k > n_total - big_k || k > big_k {
        return 0.0;
    }
    if k == 0 {
        let mut log_result = 0.0;
        for i in (n_total - big_k - n + 1)..=(n_total - big_k) {
            log_result += (i as f64).ln();
        }
        for i in (n_total - n + 1)..=n_total {
            log_result -= (i as f64).ln();
        }
        log_result.exp()
    } else if n - k == n_total - big_k {
        // Special case: the recursion below would divide by zero here.
        let mut log_result = 0.0;
        for i in (k + 1)..=n {
            log_result += (i as f64).ln();
        }
        for i in (big_k + 1)..=n_total {
            log_result -= (i as f64).ln();
        }
        log_result.exp()
    } else {
        p_hypergeometric(i_k, n as u16, (k - 1) as u16)
            * (big_k - k + 1) as f64
            * (n - k + 1) as f64
            / (k as f64 * (n_total - big_k - n + k) as f64)
    }
}

/// Bayesian posterior `P(S = SUMS[i_k] | observed k of n)`.
///
/// `prior[i]` is `P(S = SUMS[i])` — the fraction of Crypto1 states whose sum
/// property is `SUMS[i]`. Port of `sum_probability`. Returns `0.0` when the
/// observation is impossible for this hypothesis, and `0.0` if the evidence has
/// zero total probability under the prior.
pub fn sum_probability(i_k: usize, n: u16, k: u16, prior: &[f64; SUMS.len()]) -> f64 {
    if k > SUMS[i_k] {
        return 0.0;
    }
    let likelihood = p_hypergeometric(i_k, n, k);
    let evidence: f64 = (0..SUMS.len())
        .map(|i| prior[i] * p_hypergeometric(i, n, k))
        .sum();
    if evidence == 0.0 {
        return 0.0;
    }
    likelihood * prior[i_k] / evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Partial sums are always even and within `0..=16` (so `/2` is a valid
    /// `0..NUM_PART_SUMS` index). Sampled across the 20-bit half-state space.
    #[test]
    fn partial_sums_are_even_and_bounded() {
        for state in (0..1u32 << 20).step_by(101) {
            for half in [Half::Even, Half::Odd] {
                let p = partial_sum_property(state, half);
                assert!(p <= 16, "partial sum {p} out of range for {state:#x}");
                assert_eq!(p % 2, 0, "partial sum {p} not even for {state:#x}");
                assert!((p / 2) < NUM_PART_SUMS as u16);
            }
        }
    }

    /// Every combination of legal partial sums yields a legal full sum, and the
    /// combinations cover exactly the 19 documented `SUMS` values.
    #[test]
    fn combined_sums_cover_exactly_the_sum_set() {
        let mut seen = std::collections::BTreeSet::new();
        for p in (0..=16u16).step_by(2) {
            for q in (0..=16u16).step_by(2) {
                let sum = combine_sum(p, q);
                assert!(
                    sum_index(sum).is_some(),
                    "combine({p},{q}) = {sum} not in SUMS"
                );
                seen.insert(sum);
            }
        }
        let expected: std::collections::BTreeSet<u16> = SUMS.iter().copied().collect();
        assert_eq!(
            seen, expected,
            "partial-sum combinations must cover SUMS exactly"
        );
    }

    /// `sum_index` round-trips the sum table and rejects non-members.
    #[test]
    fn sum_index_round_trips() {
        for (i, &s) in SUMS.iter().enumerate() {
            assert_eq!(sum_index(s), Some(i));
        }
        assert_eq!(sum_index(1), None);
        assert_eq!(sum_index(255), None);
        assert_eq!(sum_index(300), None);
    }

    /// The sum property of a concrete loaded key is a legal sum value, and the
    /// convenience wrapper agrees with the explicit partial/combine composition.
    /// For a fixed hypothesis and sample size, the hypergeometric distribution
    /// over all achievable `k` sums to 1.
    #[test]
    fn hypergeometric_is_a_distribution() {
        for &n in &[1u16, 5, 16, 32] {
            for (i_k, &sum) in SUMS.iter().enumerate() {
                let total: f64 = (0..=n).map(|k| p_hypergeometric(i_k, n, k)).sum();
                // k above SUMS[i_k] or beyond the population contributes 0.
                assert!((total - 1.0).abs() < 1e-6, "sum={sum} n={n} total={total}");
            }
        }
    }

    /// Probabilities are always in [0, 1]; impossible observations give exactly 0.
    #[test]
    fn hypergeometric_is_bounded() {
        for &n in &[1u16, 8, 32] {
            for (i_k, &sum) in SUMS.iter().enumerate() {
                for k in 0..=n {
                    let p = p_hypergeometric(i_k, n, k);
                    // Allow tiny floating-point drift from the recursion above 1.0.
                    assert!((-1e-9..=1.0 + 1e-9).contains(&p), "p={p} out of range");
                }
                // Observing more successes than the hypothesis allows is impossible.
                if sum < n {
                    assert_eq!(p_hypergeometric(i_k, n, n), 0.0);
                }
            }
        }
    }

    /// With a uniform prior the posterior over hypotheses is a valid
    /// distribution, and evidence favouring one sum raises its posterior above
    /// the uniform baseline.
    #[test]
    fn bayesian_posterior_normalises_and_updates() {
        let uniform = [1.0 / SUMS.len() as f64; SUMS.len()];
        let (n, k) = (16u16, 8u16);
        let posterior: Vec<f64> = (0..SUMS.len())
            .map(|i| sum_probability(i, n, k, &uniform))
            .collect();
        let total: f64 = posterior.iter().sum();
        assert!((total - 1.0).abs() < 1e-6, "posterior total={total}");
        assert!(posterior.iter().all(|&p| (0.0..=1.0).contains(&p)));
        // The middle sum value 128 explains "half the bytes set" best.
        let idx_128 = sum_index(128).unwrap();
        assert!(
            posterior[idx_128] > uniform[idx_128],
            "evidence should raise the most consistent hypothesis"
        );
    }

    /// A prior that is certain of one hypothesis keeps the posterior certain
    /// (the observation cannot contradict a degenerate prior here).
    #[test]
    fn degenerate_prior_is_preserved() {
        let idx = sum_index(128).unwrap();
        let mut prior = [0.0; SUMS.len()];
        prior[idx] = 1.0;
        assert!((sum_probability(idx, 16, 8, &prior) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn full_sum_property_of_states_is_legal() {
        for key in [0u64, 0xffffffffffff, 0xa0a1a2a3a4a5, 0x123456789abc] {
            let s = Crypto1::new(key);
            let via_wrapper = sum_property(&s);
            let via_parts = combine_sum(
                partial_sum_property(s.odd, Half::Odd),
                partial_sum_property(s.even, Half::Even),
            );
            assert_eq!(via_wrapper, via_parts);
            assert!(
                sum_index(via_wrapper).is_some(),
                "sum {via_wrapper} not in SUMS for key {key:#x}"
            );
        }
    }
}
