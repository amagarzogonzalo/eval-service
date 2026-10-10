use crate::stats::{bootstrap_paired_delta, wilson_interval};
use eval_core::{CaseRecord, GateDecision, GatePolicy, RunRecord};
use std::collections::{BTreeMap, BTreeSet};

/// Decide the suite gate: compare `candidate` against `baseline`.
///
/// Deterministic first: status flips, grade flips, and (deterministic
/// cases only) digest changes fail the gate with evidence lines.
/// Statistics second: the candidate suite pass rate must clear
/// `min_pass_rate`, and stay within `delta_floor` of the baseline
/// (paired bootstrap over per-case scores; diagnostic output is never
/// presented as significance beyond the written CI).
pub fn decide_gate(
    baseline: Option<&RunRecord>,
    candidate: &RunRecord,
    policy: &GatePolicy,
) -> GateDecision {
    let Some(baseline) = baseline else {
        return GateDecision {
            pass: true,
            what_changed: format!(
                "no baseline set; candidate recorded ({} cases, pass rate {:.2})",
                candidate.cases.len(),
                candidate.pass_rate()
            ),
            affected_cases: Vec::new(),
            evidence: Vec::new(),
            uncertainty: None,
        };
    };

    let base_cases: BTreeMap<&str, &CaseRecord> = baseline
        .cases
        .iter()
        .map(|c| (c.case_id.as_str(), c))
        .collect();
    let mut affected: BTreeSet<String> = BTreeSet::new();
    let mut evidence: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    for cand in &candidate.cases {
        let Some(base) = base_cases.get(cand.case_id.as_str()) else {
            notes.push(format!("new case `{}` (no baseline)", cand.case_id));
            continue;
        };
        // Status flips per trial position (up to the shorter run).
        for (i, (b, c)) in base.trials.iter().zip(cand.trials.iter()).enumerate() {
            if b.status != c.status {
                if c.status == eval_core::TrialStatus::Ok {
                    notes.push(format!(
                        "`{}` trial {i}: {} -> ok (improvement)",
                        cand.case_id,
                        b.status.as_str()
                    ));
                } else {
                    affected.insert(cand.case_id.clone());
                    evidence.push(format!(
                        "`{}` trial {i}: status {} -> {}",
                        cand.case_id,
                        b.status.as_str(),
                        c.status.as_str()
                    ));
                }
            }
        }
        // Grade flips.
        let base_grades: BTreeMap<&str, bool> = base
            .grades
            .iter()
            .map(|g| (g.grader.as_str(), g.pass))
            .collect();
        for grade in &cand.grades {
            match base_grades.get(grade.grader.as_str()) {
                Some(true) if !grade.pass => {
                    affected.insert(cand.case_id.clone());
                    evidence.push(format!(
                        "`{}` grader {}: pass -> fail ({})",
                        cand.case_id, grade.grader, grade.evidence
                    ));
                }
                Some(false) if grade.pass => {
                    notes.push(format!(
                        "`{}` grader {}: fail -> pass (improvement)",
                        cand.case_id, grade.grader
                    ));
                }
                None => {
                    notes.push(format!(
                        "`{}` new grader {} ({})",
                        cand.case_id, grade.grader, grade.evidence
                    ));
                }
                _ => {}
            }
        }
        // Deterministic digest changes (sampling noise stays quiet by design).
        if cand.deterministic && base.pass && cand.pass && base.digest != cand.digest {
            affected.insert(cand.case_id.clone());
            evidence.push(format!(
                "`{}` deterministic output changed (digest {} -> {})",
                cand.case_id, base.digest, cand.digest
            ));
        }
    }
    for base in &baseline.cases {
        if !candidate.cases.iter().any(|c| c.case_id == base.case_id) {
            affected.insert(base.case_id.clone());
            evidence.push(format!("`{}` removed from candidate", base.case_id));
        }
    }

    // Suite-level statistics (diagnostic, with written uncertainty).
    let mut uncertainty: Option<String> = None;
    let cand_rate = candidate.pass_rate();
    if cand_rate < policy.min_pass_rate {
        affected.insert("*suite*".to_string());
        evidence.push(format!(
            "suite pass rate {cand_rate:.3} below minimum {:.3}",
            policy.min_pass_rate
        ));
    }
    let (base_scores, cand_scores) = aligned_scores(baseline, candidate);
    if !base_scores.is_empty() {
        let (lo, hi) =
            wilson_interval(cand_rate, candidate.cases.len() as u64, policy.confidence_z);
        if let Some((delta, b_lo, b_hi)) =
            bootstrap_paired_delta(&base_scores, &cand_scores, policy.bootstrap_reps)
        {
            uncertainty = Some(format!(
                "candidate Wilson 95% CI [{lo:.3}, {hi:.3}] over {} cases; paired bootstrap delta {delta:+.3} 95% CI [{b_lo:+.3}, {b_hi:+.3}] (seeded, {} reps); floor {:.2}",
                candidate.cases.len(),
                policy.bootstrap_reps,
                policy.delta_floor
            ));
            // Fire only on a clearly negative paired delta beyond the floor.
            if delta < -policy.delta_floor && b_hi < 0.0 {
                affected.insert("*suite*".to_string());
                evidence.push(format!(
                    "suite-level degradation: paired delta {delta:+.3} (95% CI [{b_lo:+.3}, {b_hi:+.3}]) beyond floor {:.2}",
                    policy.delta_floor
                ));
            }
        }
    }

    let pass = affected.is_empty();
    let mut changes: Vec<String> = Vec::new();
    if affected.is_empty() && notes.is_empty() {
        changes.push("no changes".to_string());
    } else {
        if !affected.is_empty() {
            changes.push(format!("{} regressed case(s)", affected.len()));
        }
        changes.extend(notes);
    }
    GateDecision {
        pass,
        what_changed: changes.join("; "),
        affected_cases: affected.into_iter().collect(),
        evidence,
        uncertainty,
    }
}

/// Per-case 0/1 scores aligned on the intersection of case ids.
fn aligned_scores(baseline: &RunRecord, candidate: &RunRecord) -> (Vec<f64>, Vec<f64>) {
    let base: BTreeMap<&str, f64> = baseline
        .cases
        .iter()
        .map(|c| (c.case_id.as_str(), f64::from(c.pass)))
        .collect();
    let mut b = Vec::new();
    let mut c = Vec::new();
    for case in &candidate.cases {
        if let Some(score) = base.get(case.case_id.as_str()) {
            b.push(*score);
            c.push(f64::from(case.pass));
        }
    }
    (b, c)
}
