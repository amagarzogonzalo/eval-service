use crate::event::{LlmEvent, Outcome};
use crate::pricing::PriceTable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Optional sum: value plus how many records contributed. `None`
/// value with `reporting == 0` renders as `n/a`, never zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptSum {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub reporting: usize,
    pub total: usize,
}

impl OptSum {
    fn add(&mut self, value: Option<f64>) {
        self.total += 1;
        if let Some(value) = value {
            self.value = Some(self.value.unwrap_or(0.0) + value);
            self.reporting += 1;
        }
    }
}

/// Latency distribution with its sample size attached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyDist {
    pub n: usize,
    pub min_ms: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

fn latency_of(durations: &[u64]) -> Option<LatencyDist> {
    if durations.is_empty() {
        return None;
    }
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let rank =
        |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
    Some(LatencyDist {
        n: sorted.len(),
        min_ms: sorted[0],
        p50_ms: rank(0.5),
        p95_ms: rank(0.95),
        max_ms: sorted[sorted.len() - 1],
    })
}

/// One logical request (latest attempt decides the outcome; every
/// attempt's consumption still counts toward usage).
#[derive(Debug, Clone)]
struct Request {
    retries: usize,
    outcome: Outcome,
    duration_ms: Option<u64>,
    input_tokens: OptSum,
    output_tokens: OptSum,
    reported_usd: OptSum,
}

fn fold_requests(events: &[&LlmEvent]) -> BTreeMap<String, Request> {
    let mut by_request: BTreeMap<String, Vec<&LlmEvent>> = BTreeMap::new();
    for event in events {
        by_request
            .entry(event.request_id.clone())
            .or_default()
            .push(event);
    }
    let mut out = BTreeMap::new();
    for (request_id, mut attempts) in by_request {
        attempts.sort_by_key(|e| e.attempt);
        let latest = attempts[attempts.len() - 1];
        let mut duration_ms: Option<u64> = None;
        let mut duration_sum: u64 = 0;
        let mut duration_any = false;
        let mut input = OptSum::default();
        let mut output = OptSum::default();
        let mut reported = OptSum::default();
        for attempt in &attempts {
            if let Some(d) = attempt.duration_ms {
                duration_sum += d;
                duration_any = true;
            }
            input.add(attempt.input_tokens.map(|v| v as f64));
            output.add(attempt.output_tokens.map(|v| v as f64));
            if attempt.cost_provenance == Some(crate::event::CostProvenance::Reported) {
                reported.add(attempt.cost_usd);
            }
        }
        if duration_any {
            duration_ms = Some(duration_sum);
        }
        out.insert(
            request_id,
            Request {
                retries: attempts.len().saturating_sub(1),
                outcome: latest.outcome,
                duration_ms,
                input_tokens: input,
                output_tokens: output,
                reported_usd: reported,
            },
        );
    }
    out
}

/// Per-model aggregate row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRow {
    pub provider: String,
    pub model: String,
    pub origin: String,
    pub calls: usize,
    pub requests: usize,
    pub success_rate: Option<f64>,
    pub input_tokens: OptSum,
    pub output_tokens: OptSum,
    pub reported_usd: OptSum,
    pub estimated_usd: OptSum,
    pub pricing_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyDist>,
    pub errors: usize,
    pub timeouts: usize,
    pub retries: usize,
}

/// Whole-project usage aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectUsage {
    pub project: String,
    pub events: usize,
    pub calls: usize,
    pub requests: usize,
    pub requests_ok: usize,
    pub requests_error: usize,
    pub requests_timeout: usize,
    pub retries: usize,
    pub input_tokens: OptSum,
    pub output_tokens: OptSum,
    pub reported_usd: OptSum,
    pub estimated_usd: OptSum,
    pub pricing_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyDist>,
    pub by_model: Vec<ModelRow>,
}

/// Aggregate one project's events. `context` selects a single
/// traffic class (`Some`) or all traffic (`None`, labeled as such by
/// callers). Production and benchmark traffic must never be summed
/// together without saying so.
pub fn aggregate_project(
    project: &str,
    events: &[LlmEvent],
    pricing: &PriceTable,
    context: Option<crate::event::UsageContext>,
) -> ProjectUsage {
    let selected: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| context.is_none_or(|c| e.context == c))
        .collect();
    let mut by_model: BTreeMap<(String, String, String), Vec<&LlmEvent>> = BTreeMap::new();
    for event in &selected {
        by_model
            .entry((
                event.provider.clone(),
                event.model.clone(),
                format!("{:?}", event.origin).to_lowercase(),
            ))
            .or_default()
            .push(event);
    }
    let mut rows = Vec::new();
    for ((provider, model, origin), group) in &by_model {
        rows.push(aggregate_model(provider, model, origin, group, pricing));
    }
    // Project totals fold request-level data across the selected
    // events so a retried request counts once for success rates but
    // every attempt counts toward consumption.
    let requests = fold_requests(&selected);
    let mut durations = Vec::new();
    let mut input = OptSum::default();
    let mut output = OptSum::default();
    let mut reported = OptSum::default();
    let mut estimated = OptSum::default();
    let mut retries = 0;
    let (mut ok, mut err, mut timeout) = (0, 0, 0);
    for request in requests.values() {
        match request.outcome {
            Outcome::Ok => ok += 1,
            Outcome::Error => err += 1,
            Outcome::Timeout => timeout += 1,
        }
        retries += request.retries;
        if let Some(d) = request.duration_ms {
            durations.push(d);
        }
        merge_opt(&mut input, &request.input_tokens);
        merge_opt(&mut output, &request.output_tokens);
        merge_opt(&mut reported, &request.reported_usd);
    }
    // Estimated cost folds per-attempt token pairs (both present only).
    let mut est_total = 0.0;
    let mut est_basis = 0;
    for event in selected.iter() {
        if let Some(usd) = estimate_event(pricing, event) {
            est_total += usd;
            est_basis += 1;
        }
    }
    if est_basis > 0 {
        estimated.value = Some(est_total);
        estimated.reporting = est_basis;
    }
    estimated.total = selected.len();
    ProjectUsage {
        project: project.to_string(),
        events: selected.len(),
        calls: selected.len(),
        requests: requests.len(),
        requests_ok: ok,
        requests_error: err,
        requests_timeout: timeout,
        retries,
        input_tokens: input,
        output_tokens: output,
        reported_usd: reported,
        estimated_usd: estimated,
        pricing_version: Some(pricing.pricing_version),
        latency: latency_of(&durations),
        by_model: rows,
    }
}

/// Estimated cost for one event under `pricing`, or `None` when
/// either token count is absent (never zero-fill partial data).
/// Nested matches (not let-chains): MSRV 1.85.
fn estimate_event(pricing: &PriceTable, event: &LlmEvent) -> Option<f64> {
    match (event.input_tokens, event.output_tokens) {
        (Some(input_tokens), Some(output_tokens)) => pricing
            .estimate(&event.provider, &event.model, input_tokens, output_tokens)
            .map(|est| est.usd),
        _ => None,
    }
}

fn merge_opt(into: &mut OptSum, from: &OptSum) {
    into.total += from.total;
    into.reporting += from.reporting;
    if let Some(value) = from.value {
        into.value = Some(into.value.unwrap_or(0.0) + value);
    }
}

fn aggregate_model(
    provider: &str,
    model: &str,
    origin: &str,
    group: &[&LlmEvent],
    pricing: &PriceTable,
) -> ModelRow {
    let requests = fold_requests(group);
    let mut durations = Vec::new();
    let mut input = OptSum::default();
    let mut output = OptSum::default();
    let mut reported = OptSum::default();
    let mut estimated = OptSum::default();
    let (mut ok, mut errors, mut timeouts, mut retries) = (0, 0, 0, 0);
    for request in requests.values() {
        match request.outcome {
            Outcome::Ok => ok += 1,
            Outcome::Error => errors += 1,
            Outcome::Timeout => timeouts += 1,
        }
        retries += request.retries;
        if let Some(d) = request.duration_ms {
            durations.push(d);
        }
        merge_opt(&mut input, &request.input_tokens);
        merge_opt(&mut output, &request.output_tokens);
        merge_opt(&mut reported, &request.reported_usd);
    }
    let mut est_total = 0.0;
    let mut est_basis = 0;
    for event in group.iter() {
        if let Some(usd) = estimate_event(pricing, event) {
            est_total += usd;
            est_basis += 1;
        }
    }
    if est_basis > 0 {
        estimated.value = Some(est_total);
        estimated.reporting = est_basis;
    }
    estimated.total = group.len();
    ModelRow {
        provider: provider.to_string(),
        model: model.to_string(),
        origin: origin.to_string(),
        calls: group.len(),
        requests: requests.len(),
        success_rate: if requests.is_empty() {
            None
        } else {
            Some(ok as f64 / requests.len() as f64)
        },
        input_tokens: input,
        output_tokens: output,
        reported_usd: reported,
        estimated_usd: estimated,
        pricing_version: Some(pricing.pricing_version),
        latency: latency_of(&durations),
        errors,
        timeouts,
        retries,
    }
}

/// One application task rollup (many calls, one job).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRollup {
    pub task_id: String,
    pub calls: usize,
    pub requests: usize,
    pub success: bool,
    pub input_tokens: OptSum,
    pub output_tokens: OptSum,
    pub reported_usd: OptSum,
    pub estimated_usd: OptSum,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub models: Vec<String>,
    pub errors: usize,
}

/// Roll up tasks (events without task_id are skipped, counted in the
/// returned skip count — never silently absorbed elsewhere).
/// `context` filters like [`aggregate_project`].
pub fn aggregate_tasks(
    events: &[LlmEvent],
    pricing: &PriceTable,
    context: Option<crate::event::UsageContext>,
) -> (Vec<TaskRollup>, usize) {
    let mut by_task: BTreeMap<String, Vec<&LlmEvent>> = BTreeMap::new();
    let mut skipped = 0;
    for event in events {
        if context.is_some_and(|c| event.context != c) {
            continue;
        }
        match &event.task_id {
            Some(task_id) => by_task.entry(task_id.clone()).or_default().push(event),
            None => skipped += 1,
        }
    }
    let mut rollups = Vec::new();
    for (task_id, group) in &by_task {
        let requests = fold_requests(group);
        let mut input = OptSum::default();
        let mut output = OptSum::default();
        let mut reported = OptSum::default();
        let mut errors = 0;
        let mut models: Vec<String> = Vec::new();
        let mut start: Option<u64> = None;
        let mut end: Option<u64> = None;
        for event in group.iter() {
            if !models.contains(&event.model) {
                models.push(event.model.clone());
            }
            if let Some(d) = event.duration_ms {
                start =
                    Some(start.map_or(event.started_at_ms, |s: u64| s.min(event.started_at_ms)));
                end = Some(end.map_or(event.started_at_ms + d, |e: u64| {
                    e.max(event.started_at_ms + d)
                }));
            }
        }
        for request in requests.values() {
            if !matches!(request.outcome, Outcome::Ok) {
                errors += 1;
            }
            merge_opt(&mut input, &request.input_tokens);
            merge_opt(&mut output, &request.output_tokens);
            merge_opt(&mut reported, &request.reported_usd);
        }
        let mut estimated = OptSum::default();
        let mut est_total = 0.0;
        let mut est_basis = 0;
        for event in group.iter() {
            if let Some(usd) = estimate_event(pricing, event) {
                est_total += usd;
                est_basis += 1;
            }
        }
        if est_basis > 0 {
            estimated.value = Some(est_total);
            estimated.reporting = est_basis;
        }
        estimated.total = group.len();
        models.sort();
        rollups.push(TaskRollup {
            task_id: task_id.clone(),
            calls: group.len(),
            requests: requests.len(),
            success: errors == 0 && !requests.is_empty(),
            input_tokens: input,
            output_tokens: output,
            reported_usd: reported,
            estimated_usd: estimated,
            duration_ms: match (start, end) {
                (Some(s), Some(e)) => Some(e.saturating_sub(s)),
                _ => None,
            },
            models,
            errors,
        });
    }
    rollups.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    (rollups, skipped)
}

/// Daily bucket (by started_at_ms, UTC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBucket {
    pub day: String,
    pub calls: usize,
    pub input_tokens: OptSum,
    pub output_tokens: OptSum,
    pub errors: usize,
}

pub fn bucketize_daily(events: &[LlmEvent]) -> Vec<TimeBucket> {
    let mut buckets: BTreeMap<String, Vec<&LlmEvent>> = BTreeMap::new();
    for event in events {
        buckets
            .entry(day_of(event.started_at_ms))
            .or_default()
            .push(event);
    }
    buckets
        .into_iter()
        .map(|(day, group)| {
            let mut input = OptSum::default();
            let mut output = OptSum::default();
            let mut errors = 0;
            for event in &group {
                input.add(event.input_tokens.map(|v| v as f64));
                output.add(event.output_tokens.map(|v| v as f64));
                if !matches!(event.outcome, Outcome::Ok) {
                    errors += 1;
                }
            }
            TimeBucket {
                day,
                calls: group.len(),
                input_tokens: input,
                output_tokens: output,
                errors,
            }
        })
        .collect()
}

fn day_of(ms: u64) -> String {
    let (y, m, d) = ymd(ms / 86_400_000);
    format!("{y:04}-{m:02}-{d:02}")
}

fn ymd(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
