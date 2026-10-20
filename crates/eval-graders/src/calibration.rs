//! Judge calibration: human labels against judge labels, Cohen's kappa
//! with a deterministic bootstrap interval, A/B position-swap checks,
//! rubric-threshold diagnostics, and an advisory/gating decision.
//!
//! Calibration never turns a judge into a gate on its own. The report
//! says whether the evidence clears the configured bar; callers decide
//! whether to use it. The default bar is the repository decision H4:
//! kappa lower confidence bound >= 0.6, plus position and verbosity
//! diagnostics.

use crate::stats::SplitMix64;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CALIBRATION_SCHEMA_VERSION: u32 = 1;

/// One human-labelled calibration row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationItem {
    pub id: String,
    #[serde(default)]
    pub rubric: String,
    #[serde(default)]
    pub judge_model: Option<String>,
    #[serde(default)]
    pub human_label: Option<String>,
    #[serde(default)]
    pub judge_label: Option<String>,
    #[serde(default)]
    pub human_score: Option<f64>,
    #[serde(default)]
    pub judge_score: Option<f64>,
    #[serde(default)]
    pub pair: Option<PairItem>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// A/B pair with the human preference and the judge's verdict in both
/// presentation orders. Winners are always in the original `a`/`b`
/// orientation, so a position swap cannot silently relabel the winner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairItem {
    pub a: String,
    pub b: String,
    /// `a`, `b`, or `tie`.
    pub human_winner: String,
    #[serde(default)]
    pub judge_ab: Option<PairVerdict>,
    #[serde(default)]
    pub judge_ba: Option<PairVerdict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairVerdict {
    /// `a`, `b`, or `tie`, in the original orientation.
    pub winner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationParams {
    pub threshold: f64,
    pub kappa_min: f64,
    pub ci_level: f64,
    pub bootstrap_reps: u32,
    pub min_items: usize,
    pub min_position_items: usize,
    pub min_position_consistency: f64,
    pub max_verbosity_gap: f64,
}

impl Default for CalibrationParams {
    fn default() -> Self {
        Self {
            threshold: 8.0,
            kappa_min: 0.6,
            ci_level: 0.95,
            bootstrap_reps: 1000,
            min_items: 30,
            min_position_items: 10,
            min_position_consistency: 0.8,
            max_verbosity_gap: 0.2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetInfo {
    pub path: String,
    pub blake3: String,
    pub n_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeInfo {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    pub rubric: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceInterval {
    pub level: f64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agreement {
    pub n: usize,
    pub observed: f64,
    pub expected: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kappa: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<ConfidenceInterval>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelPairCount {
    pub human: String,
    pub judge: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairwiseAnalysis {
    pub n_pairs: usize,
    pub n_both_orders: usize,
    pub n_decisions: usize,
    pub consistent: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_position_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_agreement_ab: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_agreement_ba: Option<f64>,
    pub judge_winner_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdPoint {
    pub threshold: f64,
    pub accuracy: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kappa: Option<f64>,
    pub true_pass: usize,
    pub false_pass: usize,
    pub true_revise: usize,
    pub false_revise: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdAnalysis {
    pub threshold: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<ThresholdPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_by_kappa: Option<ThresholdPoint>,
    pub sweep: Vec<ThresholdPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerbosityAnalysis {
    pub n: usize,
    pub judge_picks_longer: usize,
    pub human_picks_longer: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_picks_longer_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_picks_longer_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationDecision {
    pub advisory: bool,
    pub gating_eligible: bool,
    pub kappa_min: f64,
    pub ci_level: f64,
    pub min_items: usize,
    pub min_position_items: usize,
    pub min_position_consistency: f64,
    pub max_verbosity_gap: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub schema_version: u32,
    pub report_id: String,
    pub generated_at_ms: u64,
    pub dataset: DatasetInfo,
    pub judge: JudgeInfo,
    pub params: CalibrationParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute: Option<Agreement>,
    #[serde(default)]
    pub confusion: Vec<LabelPairCount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairwise: Option<PairwiseAnalysis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<ThresholdAnalysis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<VerbosityAnalysis>,
    pub decision: CalibrationDecision,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Content hash for the dataset reference in a report.
pub fn dataset_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Normalize the common PASS/REVISE spellings; other labels are
/// lower-cased and kept so a study may use more than two categories.
pub fn normalize_label(label: &str) -> String {
    match label.trim().to_ascii_lowercase().as_str() {
        "pass" | "passed" | "ok" | "accept" | "accepted" => "pass".to_string(),
        "revise" | "reject" | "rejected" | "fail" | "failed" => "revise".to_string(),
        "tie" => "tie".to_string(),
        other => other.to_string(),
    }
}

/// Cohen's kappa for two raters over any number of categories.
/// `None` when the sample is empty or chance agreement is total.
pub fn cohen_kappa(pairs: &[(String, String)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let mut categories = BTreeSet::new();
    for (a, b) in pairs {
        categories.insert(a.as_str());
        categories.insert(b.as_str());
    }
    let categories: Vec<&str> = categories.into_iter().collect();
    let index: BTreeMap<&str, usize> = categories
        .iter()
        .enumerate()
        .map(|(i, name)| (*name, i))
        .collect();
    let k = categories.len();
    let mut matrix = vec![vec![0usize; k]; k];
    for (a, b) in pairs {
        let row = index[a.as_str()];
        let col = index[b.as_str()];
        matrix[row][col] += 1;
    }
    let n = pairs.len() as f64;
    let observed = (0..k).map(|i| matrix[i][i]).sum::<usize>() as f64 / n;
    let row_sums: Vec<usize> = matrix.iter().map(|row| row.iter().sum()).collect();
    let col_sums: Vec<usize> = (0..k)
        .map(|j| matrix.iter().map(|row| row[j]).sum())
        .collect();
    let mut expected = 0.0;
    for (row, col) in row_sums.iter().zip(col_sums.iter()) {
        expected += (*row as f64 / n) * (*col as f64 / n);
    }
    if (1.0 - expected).abs() < 1e-12 {
        return None;
    }
    Some((observed - expected) / (1.0 - expected))
}

/// Deterministic bootstrap percentile interval for Cohen's kappa.
/// Resamples items with replacement; undefined resamples are skipped.
pub fn bootstrap_kappa_ci(
    pairs: &[(String, String)],
    reps: u32,
    level: f64,
) -> Option<ConfidenceInterval> {
    if pairs.is_empty() || reps == 0 || !(0.0..1.0).contains(&level) {
        return None;
    }
    let mut rng = SplitMix64(0x9A11_B8A7 ^ 0xCA1B_2A7E);
    let mut values = Vec::with_capacity(reps as usize);
    for _ in 0..reps {
        let mut sample = Vec::with_capacity(pairs.len());
        for _ in 0..pairs.len() {
            sample.push(pairs[rng.below(pairs.len())].clone());
        }
        if let Some(kappa) = cohen_kappa(&sample) {
            if kappa.is_finite() {
                values.push(kappa);
            }
        }
    }
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let alpha = (1.0 - level) / 2.0;
    let low_idx = ((values.len() as f64 - 1.0) * alpha).round() as usize;
    let high_idx = ((values.len() as f64 - 1.0) * (1.0 - alpha)).round() as usize;
    Some(ConfidenceInterval {
        level,
        low: values[low_idx.min(values.len() - 1)],
        high: values[high_idx.min(values.len() - 1)],
    })
}

fn absolute_agreement(
    items: &[CalibrationItem],
    params: &CalibrationParams,
) -> (Option<Agreement>, Vec<LabelPairCount>) {
    let mut pairs = Vec::new();
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for item in items {
        let human = item.human_label.as_deref().map(normalize_label);
        let judge = item.judge_label.as_deref().map(normalize_label);
        if let (Some(h), Some(j)) = (human, judge) {
            *counts.entry((h.clone(), j.clone())).or_default() += 1;
            pairs.push((h, j));
        }
    }
    if pairs.is_empty() {
        return (None, Vec::new());
    }
    let mut categories = BTreeSet::new();
    for (h, j) in &pairs {
        categories.insert(h.as_str());
        categories.insert(j.as_str());
    }
    let k = categories.len();
    let mut matrix = vec![vec![0usize; k]; k];
    let index: BTreeMap<&str, usize> = categories
        .iter()
        .enumerate()
        .map(|(i, name)| (*name, i))
        .collect();
    for (h, j) in &pairs {
        matrix[index[h.as_str()]][index[j.as_str()]] += 1;
    }
    let n = pairs.len() as f64;
    let observed = (0..k).map(|i| matrix[i][i]).sum::<usize>() as f64 / n;
    let row_sums: Vec<usize> = matrix.iter().map(|row| row.iter().sum()).collect();
    let col_sums: Vec<usize> = (0..k)
        .map(|j| matrix.iter().map(|row| row[j]).sum())
        .collect();
    let mut expected = 0.0;
    for (row, col) in row_sums.iter().zip(col_sums.iter()) {
        expected += (*row as f64 / n) * (*col as f64 / n);
    }
    let agreement = Agreement {
        n: pairs.len(),
        observed,
        expected,
        kappa: cohen_kappa(&pairs),
        ci: bootstrap_kappa_ci(&pairs, params.bootstrap_reps, params.ci_level),
    };
    let confusion = counts
        .into_iter()
        .map(|((human, judge), count)| LabelPairCount {
            human,
            judge,
            count,
        })
        .collect();
    (Some(agreement), confusion)
}

fn threshold_point(items: &[CalibrationItem], threshold: f64) -> Option<ThresholdPoint> {
    let mut pairs = Vec::new();
    let mut true_pass = 0;
    let mut false_pass = 0;
    let mut true_revise = 0;
    let mut false_revise = 0;
    for item in items {
        let human = item.human_label.as_deref().map(normalize_label);
        let Some(score) = item.judge_score else {
            continue;
        };
        let Some(human) = human else {
            continue;
        };
        if human != "pass" && human != "revise" {
            continue;
        }
        let judge = if score >= threshold {
            "pass".to_string()
        } else {
            "revise".to_string()
        };
        match (human.as_str(), judge.as_str()) {
            ("pass", "pass") => true_pass += 1,
            ("revise", "pass") => false_pass += 1,
            ("revise", "revise") => true_revise += 1,
            ("pass", "revise") => false_revise += 1,
            _ => {}
        }
        pairs.push((human, judge));
    }
    if pairs.is_empty() {
        return None;
    }
    let total = pairs.len() as f64;
    Some(ThresholdPoint {
        threshold,
        accuracy: (true_pass + true_revise) as f64 / total,
        kappa: cohen_kappa(&pairs),
        true_pass,
        false_pass,
        true_revise,
        false_revise,
    })
}

pub fn threshold_analysis(
    items: &[CalibrationItem],
    params: &CalibrationParams,
) -> Option<ThresholdAnalysis> {
    let fixed = threshold_point(items, params.threshold);
    let mut sweep = Vec::new();
    for step in 0..=20 {
        let threshold = step as f64 * 0.5;
        if let Some(point) = threshold_point(items, threshold) {
            sweep.push(point);
        }
    }
    if fixed.is_none() && sweep.is_empty() {
        return None;
    }
    let best_by_kappa = sweep
        .iter()
        .max_by(|a, b| {
            a.kappa
                .unwrap_or(f64::NEG_INFINITY)
                .partial_cmp(&b.kappa.unwrap_or(f64::NEG_INFINITY))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.accuracy
                        .partial_cmp(&b.accuracy)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .cloned();
    Some(ThresholdAnalysis {
        threshold: params.threshold,
        fixed,
        best_by_kappa,
        sweep,
    })
}

pub fn pairwise_analysis(items: &[CalibrationItem]) -> Option<PairwiseAnalysis> {
    let mut n_pairs = 0;
    let mut n_both_orders = 0;
    let mut n_decisions = 0;
    let mut consistent = 0;
    let mut first = 0;
    let mut ab_agree = 0;
    let mut ab_n = 0;
    let mut ba_agree = 0;
    let mut ba_n = 0;
    let mut judge_winner_counts: BTreeMap<String, usize> = BTreeMap::new();

    for item in items {
        let Some(pair) = &item.pair else {
            continue;
        };
        n_pairs += 1;
        let human = normalize_label(&pair.human_winner);
        for verdict in [&pair.judge_ab, &pair.judge_ba].into_iter().flatten() {
            let winner = normalize_label(&verdict.winner);
            *judge_winner_counts.entry(winner.clone()).or_default() += 1;
            if winner == "a" || winner == "b" {
                n_decisions += 1;
            }
        }
        if let (Some(ab), Some(ba)) = (&pair.judge_ab, &pair.judge_ba) {
            n_both_orders += 1;
            let ab_win = normalize_label(&ab.winner);
            let ba_win = normalize_label(&ba.winner);
            if ab_win == ba_win {
                consistent += 1;
            }
            if ab_win == "a" || ba_win == "b" {
                first += 1;
            }
            if ab_win == "a" || ab_win == "b" {
                ab_n += 1;
                if ab_win == human {
                    ab_agree += 1;
                }
            }
            if ba_win == "a" || ba_win == "b" {
                ba_n += 1;
                if ba_win == human {
                    ba_agree += 1;
                }
            }
        }
    }
    if n_pairs == 0 {
        return None;
    }
    Some(PairwiseAnalysis {
        n_pairs,
        n_both_orders,
        n_decisions,
        consistent,
        consistency_rate: if n_both_orders == 0 {
            None
        } else {
            Some(consistent as f64 / n_both_orders as f64)
        },
        first_position_rate: if n_decisions == 0 {
            None
        } else {
            Some(first as f64 / n_decisions as f64)
        },
        human_agreement_ab: if ab_n == 0 {
            None
        } else {
            Some(ab_agree as f64 / ab_n as f64)
        },
        human_agreement_ba: if ba_n == 0 {
            None
        } else {
            Some(ba_agree as f64 / ba_n as f64)
        },
        judge_winner_counts,
    })
}

pub fn verbosity_analysis(items: &[CalibrationItem]) -> Option<VerbosityAnalysis> {
    let mut n = 0;
    let mut judge_longer = 0;
    let mut human_longer = 0;
    for item in items {
        let Some(pair) = &item.pair else {
            continue;
        };
        let Some(verdict) = pair.judge_ab.as_ref().or(pair.judge_ba.as_ref()) else {
            continue;
        };
        let winner = normalize_label(&verdict.winner);
        let human = normalize_label(&pair.human_winner);
        if winner == "tie" || human == "tie" {
            continue;
        }
        let len_a = pair.a.chars().count();
        let len_b = pair.b.chars().count();
        if len_a == len_b {
            continue;
        }
        let longer = if len_a > len_b { "a" } else { "b" };
        n += 1;
        if winner == longer {
            judge_longer += 1;
        }
        if human == longer {
            human_longer += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let judge_rate = judge_longer as f64 / n as f64;
    let human_rate = human_longer as f64 / n as f64;
    Some(VerbosityAnalysis {
        n,
        judge_picks_longer: judge_longer,
        human_picks_longer: human_longer,
        judge_picks_longer_rate: Some(judge_rate),
        human_picks_longer_rate: Some(human_rate),
        gap: Some(judge_rate - human_rate),
    })
}

/// Build a calibration report from parsed rows.
pub fn calibrate(
    items: &[CalibrationItem],
    params: CalibrationParams,
    dataset: DatasetInfo,
    judge: JudgeInfo,
    generated_at_ms: u64,
) -> CalibrationReport {
    let (absolute, confusion) = absolute_agreement(items, &params);
    let pairwise = pairwise_analysis(items);
    let thresholds = threshold_analysis(items, &params);
    let verbosity = verbosity_analysis(items);
    let mut warnings = Vec::new();

    let rubrics: BTreeSet<&str> = items
        .iter()
        .map(|item| item.rubric.as_str())
        .filter(|r| !r.is_empty())
        .collect();
    if rubrics.len() > 1 {
        warnings.push(format!(
            "dataset mixes rubric ids: {}",
            rubrics.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let models: BTreeSet<&str> = items
        .iter()
        .filter_map(|item| item.judge_model.as_deref())
        .filter(|m| !m.is_empty())
        .collect();
    if models.len() > 1 {
        warnings.push(format!(
            "dataset mixes judge models: {}",
            models.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if items.len() < params.min_items {
        warnings.push(format!(
            "only {} labelled items; the bar requires at least {}",
            items.len(),
            params.min_items
        ));
    }

    let mut reasons = Vec::new();
    let mut kappa_low = None;
    if let Some(agreement) = &absolute {
        if let Some(ci) = &agreement.ci {
            kappa_low = Some(ci.low);
        }
    }
    match kappa_low {
        Some(low) if low >= params.kappa_min => {}
        Some(low) => reasons.push(format!(
            "kappa lower CI {low:.3} is below the {:.3} bar",
            params.kappa_min
        )),
        None => reasons.push("no absolute human/judge labels with a defined kappa".to_string()),
    }
    if items.len() < params.min_items {
        reasons.push(format!(
            "{} items is below the {} item minimum",
            items.len(),
            params.min_items
        ));
    }
    match &pairwise {
        None => reasons.push("no A/B swap pairs".to_string()),
        Some(pairwise) => {
            if pairwise.n_both_orders < params.min_position_items {
                reasons.push(format!(
                    "{} both-order A/B pairs is below the {} pair minimum",
                    pairwise.n_both_orders, params.min_position_items
                ));
            }
            if let Some(rate) = pairwise.consistency_rate {
                if rate < params.min_position_consistency {
                    reasons.push(format!(
                        "position consistency {rate:.3} is below {:.3}",
                        params.min_position_consistency
                    ));
                }
            } else {
                reasons.push("position consistency is undefined".to_string());
            }
        }
    }
    if let Some(verbosity) = &verbosity {
        if let Some(gap) = verbosity.gap {
            if gap > params.max_verbosity_gap {
                reasons.push(format!(
                    "judge favors longer outputs by {gap:.3} more than humans"
                ));
            }
        }
    }
    let gating_eligible = reasons.is_empty();
    let decision = CalibrationDecision {
        advisory: !gating_eligible,
        gating_eligible,
        kappa_min: params.kappa_min,
        ci_level: params.ci_level,
        min_items: params.min_items,
        min_position_items: params.min_position_items,
        min_position_consistency: params.min_position_consistency,
        max_verbosity_gap: params.max_verbosity_gap,
        reasons,
    };

    let mut report = CalibrationReport {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        report_id: String::new(),
        generated_at_ms,
        dataset,
        judge,
        params,
        absolute,
        confusion,
        pairwise,
        thresholds,
        verbosity,
        decision,
        warnings,
    };
    report.report_id = report_id(&report);
    report
}

fn report_id(report: &CalibrationReport) -> String {
    let mut canonical = report.clone();
    canonical.report_id.clear();
    canonical.generated_at_ms = 0;
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    blake3::hash(&bytes).to_hex()[..16].to_string()
}

/// Human-readable summary for CLI and CI logs.
pub fn render_markdown(report: &CalibrationReport) -> String {
    let mut out = String::new();
    out.push_str("# Judge calibration report\n\n");
    out.push_str(&format!(
        "- Report: `{}` (schema {})\n",
        report.report_id, report.schema_version
    ));
    out.push_str(&format!("- Dataset: `{}`\n", report.dataset.path));
    out.push_str(&format!(
        "- Dataset hash: `{}` ({} items)\n",
        report.dataset.blake3, report.dataset.n_items
    ));
    out.push_str(&format!(
        "- Judge: `{}` · rubric `{}`\n",
        report.judge.model, report.judge.rubric
    ));
    out.push_str(&format!(
        "- Decision: **{}**\n\n",
        if report.decision.gating_eligible {
            "GATING ELIGIBLE"
        } else {
            "ADVISORY"
        }
    ));
    if !report.decision.reasons.is_empty() {
        out.push_str("## Why advisory\n\n");
        for reason in &report.decision.reasons {
            out.push_str(&format!("- {reason}\n"));
        }
        out.push('\n');
    }
    out.push_str("## Metrics\n\n");
    out.push_str("| Metric | Value |\n|---|---|\n");
    if let Some(absolute) = &report.absolute {
        out.push_str(&format!(
            "| Absolute agreement | n={} observed={:.3} expected={:.3} |\n",
            absolute.n, absolute.observed, absolute.expected
        ));
        if let Some(kappa) = absolute.kappa {
            out.push_str(&format!("| Cohen's kappa | {kappa:.3} |\n"));
        }
        if let Some(ci) = &absolute.ci {
            out.push_str(&format!(
                "| Kappa {:.0}% CI | [{:.3}, {:.3}] |\n",
                ci.level * 100.0,
                ci.low,
                ci.high
            ));
        }
    }
    if let Some(pairwise) = &report.pairwise {
        out.push_str(&format!(
            "| A/B both orders | {} |\n| Position consistency | {} |\n| First-position rate | {} |\n",
            pairwise.n_both_orders,
            pairwise
                .consistency_rate
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "n/a".to_string()),
            pairwise
                .first_position_rate
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "n/a".to_string())
        ));
    }
    if let Some(thresholds) = &report.thresholds {
        if let Some(fixed) = &thresholds.fixed {
            out.push_str(&format!(
                "| Threshold {:.1} accuracy | {:.3} |\n",
                fixed.threshold, fixed.accuracy
            ));
        }
    }
    if let Some(verbosity) = &report.verbosity {
        if let Some(gap) = verbosity.gap {
            out.push_str(&format!("| Verbosity gap | {gap:+.3} |\n"));
        }
    }
    if !report.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for warning in &report.warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out.push_str("\nA judge is advisory until its kappa lower confidence bound clears the configured bar; calibration does not promote a judge automatically.\n");
    out
}
