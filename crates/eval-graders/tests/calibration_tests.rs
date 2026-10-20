//! Judge-calibration reference tests: kappa math, deterministic CI,
//! position swap, thresholds, verbosity, and the advisory bar.

use eval_graders::calibration::*;

fn labels(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn item(id: &str, human: &str, judge: &str, score: f64) -> CalibrationItem {
    CalibrationItem {
        id: id.to_string(),
        rubric: "digest-mr-review/v1".to_string(),
        judge_model: Some("judge-test/v1".to_string()),
        human_label: Some(human.to_string()),
        judge_label: Some(judge.to_string()),
        human_score: Some(score),
        judge_score: Some(score),
        pair: None,
        notes: None,
    }
}

fn pair(id: &str, human_winner: &str, ab: &str, ba: &str) -> CalibrationItem {
    let mut row = item(id, "pass", "pass", 8.0);
    row.pair = Some(PairItem {
        a: "A".to_string(),
        b: "B".to_string(),
        human_winner: human_winner.to_string(),
        judge_ab: Some(PairVerdict {
            winner: ab.to_string(),
        }),
        judge_ba: Some(PairVerdict {
            winner: ba.to_string(),
        }),
    });
    row
}

#[test]
fn kappa_reference_value() {
    let mut rows = Vec::new();
    for _ in 0..10 {
        rows.push(("pass".to_string(), "pass".to_string()));
    }
    for _ in 0..5 {
        rows.push(("revise".to_string(), "revise".to_string()));
    }
    for _ in 0..2 {
        rows.push(("pass".to_string(), "revise".to_string()));
    }
    for _ in 0..3 {
        rows.push(("revise".to_string(), "pass".to_string()));
    }
    let kappa = cohen_kappa(&rows).unwrap();
    assert!((kappa - 0.468_085_106).abs() < 1e-6, "kappa={kappa}");
}

#[test]
fn kappa_perfect_and_undefined() {
    assert_eq!(
        cohen_kappa(&labels(&[("pass", "pass"), ("revise", "revise")])),
        Some(1.0)
    );
    assert_eq!(
        cohen_kappa(&labels(&[("pass", "pass"), ("pass", "pass")])),
        None
    );
    assert_eq!(cohen_kappa(&[]), None);
}

#[test]
fn kappa_ci_is_deterministic_and_brackets_point() {
    let rows = labels(&[
        ("pass", "pass"),
        ("pass", "pass"),
        ("revise", "revise"),
        ("revise", "revise"),
        ("pass", "revise"),
        ("revise", "pass"),
        ("pass", "pass"),
        ("revise", "revise"),
    ]);
    let a = bootstrap_kappa_ci(&rows, 500, 0.95).unwrap();
    let b = bootstrap_kappa_ci(&rows, 500, 0.95).unwrap();
    assert_eq!(a.low, b.low);
    assert_eq!(a.high, b.high);
    assert!(a.low <= a.high);
    let point = cohen_kappa(&rows).unwrap();
    assert!(point + 1e-9 >= a.low && point <= a.high + 1e-9);
}

#[test]
fn position_swap_and_verbosity() {
    let mut items = vec![
        pair("p1", "a", "a", "a"),
        pair("p2", "b", "b", "b"),
        pair("p3", "a", "a", "b"),
    ];
    items[0].pair.as_mut().unwrap().a = "short".to_string();
    items[0].pair.as_mut().unwrap().b = "a much longer candidate".to_string();
    let pairwise = pairwise_analysis(&items).unwrap();
    assert_eq!(pairwise.n_pairs, 3);
    assert_eq!(pairwise.n_both_orders, 3);
    assert_eq!(pairwise.consistent, 2);
    assert!((pairwise.consistency_rate.unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(pairwise.first_position_rate, Some(0.5));
    let verbosity = verbosity_analysis(&items).unwrap();
    assert_eq!(verbosity.n, 1);
    assert_eq!(verbosity.judge_picks_longer, 0);
    assert_eq!(verbosity.human_picks_longer, 0);
}

#[test]
fn threshold_sweep_finds_perfect_split() {
    let items = vec![
        item("a", "pass", "pass", 9.0),
        item("b", "pass", "pass", 8.5),
        item("c", "revise", "revise", 5.0),
        item("d", "revise", "revise", 6.0),
    ];
    let params = CalibrationParams::default();
    let thresholds = threshold_analysis(&items, &params).unwrap();
    let fixed = thresholds.fixed.unwrap();
    assert!((fixed.accuracy - 1.0).abs() < 1e-9);
    assert_eq!(fixed.kappa, Some(1.0));
    assert!((thresholds.best_by_kappa.unwrap().kappa.unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn default_sample_is_advisory_below_minimum() {
    let items = vec![
        item("a", "pass", "pass", 9.0),
        item("b", "revise", "revise", 5.0),
    ];
    let report = calibrate(
        &items,
        CalibrationParams::default(),
        DatasetInfo {
            path: "tiny.jsonl".to_string(),
            blake3: "hash".to_string(),
            n_items: items.len(),
        },
        JudgeInfo {
            model: "judge-test/v1".to_string(),
            prompt_version: None,
            rubric: "digest-mr-review/v1".to_string(),
        },
        1,
    );
    assert!(report.decision.advisory);
    assert!(!report.decision.gating_eligible);
    assert!(
        report
            .decision
            .reasons
            .iter()
            .any(|r| r.contains("minimum"))
    );
    assert!(render_markdown(&report).contains("ADVISORY"));
}

#[test]
fn clean_study_clears_the_bar() {
    let mut items = Vec::new();
    for i in 0..20 {
        items.push(item(&format!("pass-{i}"), "pass", "pass", 9.0));
    }
    for i in 0..20 {
        items.push(item(&format!("revise-{i}"), "revise", "revise", 4.0));
    }
    for i in 0..10 {
        items.push(pair(&format!("pair-{i}"), "a", "a", "a"));
    }
    let params = CalibrationParams::default();
    let report = calibrate(
        &items,
        params,
        DatasetInfo {
            path: "clean.jsonl".to_string(),
            blake3: "hash".to_string(),
            n_items: items.len(),
        },
        JudgeInfo {
            model: "judge-test/v1".to_string(),
            prompt_version: Some("prompt-v1".to_string()),
            rubric: "digest-mr-review/v1".to_string(),
        },
        1,
    );
    assert!(
        report.decision.gating_eligible,
        "{:?}",
        report.decision.reasons
    );
    assert!(!report.decision.advisory);
    assert_eq!(report.absolute.unwrap().kappa, Some(1.0));
}

#[test]
fn report_id_ignores_generation_time() {
    let items = vec![item("a", "pass", "pass", 9.0)];
    let dataset = DatasetInfo {
        path: "x.jsonl".to_string(),
        blake3: "hash".to_string(),
        n_items: 1,
    };
    let judge = JudgeInfo {
        model: "judge-test/v1".to_string(),
        prompt_version: None,
        rubric: "r".to_string(),
    };
    let one = calibrate(
        &items,
        CalibrationParams::default(),
        dataset.clone(),
        judge.clone(),
        1,
    );
    let two = calibrate(&items, CalibrationParams::default(), dataset, judge, 2);
    assert_eq!(one.report_id, two.report_id);
    assert_ne!(one.generated_at_ms, two.generated_at_ms);
}

#[test]
fn threshold_and_pair_metrics_are_optional() {
    let items = vec![item("a", "pass", "pass", 9.0)];
    let report = calibrate(
        &items,
        CalibrationParams::default(),
        DatasetInfo {
            path: "x".to_string(),
            blake3: "hash".to_string(),
            n_items: 1,
        },
        JudgeInfo {
            model: "j".to_string(),
            prompt_version: None,
            rubric: "r".to_string(),
        },
        0,
    );
    assert!(report.thresholds.is_some());
    assert!(report.pairwise.is_none());
    assert!(report.verbosity.is_none());
    assert!(report.decision.reasons.iter().any(|r| r.contains("A/B")));
}
