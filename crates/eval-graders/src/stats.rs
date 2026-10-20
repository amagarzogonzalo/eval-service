//! Hand-rolled statistics: unbiased pass@k, Wilson score intervals,
//! and a paired bootstrap for suite-level gate decisions.
//!
//! Each function is small and covered by unit tests against reference
//! values. Revisit only if richer methods prove better on measured
//! gate performance.

/// Unbiased pass@k estimator: with `n` trials of which `c` pass, the
/// probability that at least one of k draws passes.
/// Returns `None` when undefined (`n == 0` or `k > n`).
pub fn pass_at_k(n: u64, c: u64, k: u64) -> Option<f64> {
    if n == 0 || k == 0 || k > n {
        return None;
    }
    let c = c.min(n);
    if c == 0 {
        return Some(0.0);
    }
    if n - c < k {
        return Some(1.0);
    }
    let mut prod = 1.0f64;
    for i in 0..k {
        prod *= (n - c - i) as f64 / (n - i) as f64;
    }
    Some(1.0 - prod)
}

/// Wilson score interval for a binomial proportion.
/// Returns `(lower, upper)` clamped to [0, 1]. `n == 0` yields `(0, 1)`
/// (total uncertainty, never a false claim).
pub fn wilson_interval(p_hat: f64, n: u64, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let n = n as f64;
    let denom = 1.0 + z * z / n;
    let center = p_hat + z * z / (2.0 * n);
    let spread = z * (p_hat * (1.0 - p_hat) / n + z * z / (4.0 * n * n)).sqrt();
    (
        ((center - spread) / denom).clamp(0.0, 1.0),
        ((center + spread) / denom).clamp(0.0, 1.0),
    )
}

/// Deterministic PRNG (SplitMix64) so bootstrap results are reproducible.
pub(crate) struct SplitMix64(pub(crate) u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64)) as usize
    }
}

/// Paired bootstrap over per-case scores: resample case pairs with
/// replacement, compute mean(candidate − baseline) per replicate, and
/// return `(point_delta, ci_lo, ci_hi)` at the 95% empirical interval.
///
/// Fixed seed ⇒ deterministic. Returns `None` on empty or mismatched input.
pub fn bootstrap_paired_delta(
    baseline: &[f64],
    candidate: &[f64],
    reps: u32,
) -> Option<(f64, f64, f64)> {
    if baseline.is_empty() || baseline.len() != candidate.len() || reps == 0 {
        return None;
    }
    let n = baseline.len();
    let point: f64 = candidate
        .iter()
        .zip(baseline.iter())
        .map(|(c, b)| c - b)
        .sum::<f64>()
        / n as f64;
    let mut rng = SplitMix64(0xE1A1_8EE0 ^ 0xC0FFEE);
    let mut deltas = Vec::with_capacity(reps as usize);
    for _ in 0..reps {
        let mut sum = 0.0;
        for _ in 0..n {
            let i = rng.below(n);
            sum += candidate[i] - baseline[i];
        }
        deltas.push(sum / n as f64);
    }
    deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = deltas[(reps as f64 * 0.025).floor() as usize % deltas.len()];
    let hi = deltas[(reps as f64 * 0.975).floor() as usize % deltas.len()];
    Some((point, lo, hi))
}
