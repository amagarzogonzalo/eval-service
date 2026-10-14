//! Usage contract tests: validation, idempotent import, pricing
//! provenance, aggregation honesty (§11 coverage).

use eval_usage::*;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("eval-usage-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn event(id: &str) -> LlmEvent {
    LlmEvent {
        event: "llm.call".to_string(),
        event_version: EVENT_VERSION,
        event_id: id.to_string(),
        project: "p".to_string(),
        trace_id: None,
        task_id: Some("task-1".to_string()),
        request_id: "req-1".to_string(),
        attempt: 0,
        provider: "groq".to_string(),
        model: "openai/gpt-oss-120b".to_string(),
        started_at_ms: 1_700_000_000_000,
        duration_ms: Some(120),
        streaming: false,
        ttft_ms: None,
        input_tokens: Some(1000),
        output_tokens: Some(200),
        cached_tokens: None,
        reasoning_tokens: None,
        outcome: Outcome::Ok,
        error_code: None,
        retriable: false,
        cost_usd: None,
        cost_provenance: None,
        prompt_id: Some("review/v3".to_string()),
        prompt_hash: None,
        run_id: None,
        case_id: None,
        tag: None,
        origin: EventOrigin::Live,
        context: UsageContext::Benchmark,
        extensions: None,
    }
}

fn prices() -> PriceTable {
    PriceTable {
        pricing_version: 7,
        currency: "USD".to_string(),
        rates: vec![PriceRate {
            provider: "groq".to_string(),
            model: "openai/gpt-oss-120b".to_string(),
            input_per_1m: 1.0,
            output_per_1m: 2.0,
        }],
    }
}

#[test]
fn valid_event_passes() {
    assert!(validate_event(&event("e1")).is_ok());
}

#[test]
fn unknown_version_rejected() {
    let mut bad = event("e1");
    bad.event_version = 99;
    assert!(matches!(
        validate_event(&bad),
        Err(UsageError::UnsupportedVersion(99))
    ));
}

#[test]
fn empty_ids_rejected() {
    for field in ["event_id", "project", "request_id", "provider", "model"] {
        let mut bad = event("e1");
        match field {
            "event_id" => bad.event_id = "  ".to_string(),
            "project" => bad.project = String::new(),
            "request_id" => bad.request_id = String::new(),
            "provider" => bad.provider = String::new(),
            _ => bad.model = String::new(),
        }
        assert!(validate_event(&bad).is_err(), "{field}");
    }
}

#[test]
fn ok_with_error_code_rejected() {
    let mut bad = event("e1");
    bad.error_code = Some("boom".to_string());
    assert!(validate_event(&bad).is_err());
}

#[test]
fn import_accepts_duplicates_rejects() {
    let dir = tmpdir("import");
    let store = UsageStore::new(&dir);
    let good = serde_json::to_string(&event("e1")).unwrap();
    let body = format!("{good}\n{good}\nnot json\n{{\"event_version\": 99}}\n");
    let summary = store.import_str("p", &body, None, None).unwrap();
    assert_eq!(summary.accepted, 1);
    assert_eq!(summary.duplicates, 1);
    assert_eq!(summary.rejected.len(), 2);
    // Reprocessing is all-duplicates; history unchanged.
    let again = store.import_str("p", &body, None, None).unwrap();
    assert_eq!(again.accepted, 0);
    assert_eq!(again.duplicates, 2);
    assert_eq!(again.rejected.len(), 2);
    let (events, skipped) = store.read_events("p").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(skipped, 0);
}

#[test]
fn project_mismatch_rejected() {
    let dir = tmpdir("mismatch");
    let store = UsageStore::new(&dir);
    let mut foreign = event("e1");
    foreign.project = "other".to_string();
    let summary = store
        .import_str("p", &serde_json::to_string(&foreign).unwrap(), None, None)
        .unwrap();
    assert_eq!(summary.accepted, 0);
    assert_eq!(summary.rejected.len(), 1);
    assert!(summary.rejected[0].reason.contains("mismatch"));
}

#[test]
fn tag_stamping_prefers_event_tag() {
    let dir = tmpdir("tag");
    let store = UsageStore::new(&dir);
    let mut tagged = event("e1");
    tagged.tag = Some("bench-a".to_string());
    let plain = serde_json::to_string(&event("e2")).unwrap();
    let body = format!("{}\n{plain}\n", serde_json::to_string(&tagged).unwrap());
    store.import_str("p", &body, Some("bench-b"), None).unwrap();
    let (events, _) = store.read_events("p").unwrap();
    assert_eq!(events[0].tag.as_deref(), Some("bench-a"));
    assert_eq!(events[1].tag.as_deref(), Some("bench-b"));
}

#[test]
fn pricing_resolution_order() {
    let table = PriceTable {
        pricing_version: 1,
        currency: "USD".to_string(),
        rates: vec![
            PriceRate {
                provider: "*".to_string(),
                model: "*".to_string(),
                input_per_1m: 1.0,
                output_per_1m: 1.0,
            },
            PriceRate {
                provider: "groq".to_string(),
                model: "*".to_string(),
                input_per_1m: 2.0,
                output_per_1m: 2.0,
            },
            PriceRate {
                provider: "groq".to_string(),
                model: "m".to_string(),
                input_per_1m: 3.0,
                output_per_1m: 3.0,
            },
        ],
    };
    assert_eq!(table.resolve("groq", "m"), Some((3.0, 3.0)));
    assert_eq!(table.resolve("groq", "other"), Some((2.0, 2.0)));
    assert_eq!(table.resolve("unknown", "unknown"), Some((1.0, 1.0)));
    assert_eq!(PriceTable::empty().resolve("groq", "m"), None);
}

#[test]
fn pricing_change_does_not_rewrite_history() {
    // Same events under two table versions: different estimates,
    // identical raw events.
    let events = vec![event("e1")];
    let v1 = prices();
    let mut v2 = prices();
    v2.pricing_version = 8;
    v2.rates[0].input_per_1m = 10.0;
    let a = aggregate_project("p", &events, &v1, None);
    let b = aggregate_project("p", &events, &v2, None);
    let ea = a.estimated_usd.value.unwrap();
    let eb = b.estimated_usd.value.unwrap();
    assert!((ea - 0.0014).abs() < 1e-9, "{ea}");
    assert!((eb - 0.0104).abs() < 1e-9, "{eb}");
    assert_eq!(a.pricing_version, Some(7));
    assert_eq!(b.pricing_version, Some(8));
}

#[test]
fn partial_tokens_never_estimated() {
    let mut half = event("e1");
    half.output_tokens = None;
    let usage = aggregate_project("p", &[half], &prices(), None);
    assert_eq!(usage.estimated_usd.value, None);
    assert_eq!(usage.estimated_usd.reporting, 0);
    // ...but observed input tokens still aggregate.
    assert_eq!(usage.input_tokens.value, Some(1000.0));
    assert_eq!(usage.output_tokens.value, None);
}

#[test]
fn retry_counts_once_as_request() {
    // One logical request, three physical attempts, final success.
    // Consumption counts every attempt; success counts the request once.
    let mut attempts = Vec::new();
    for (i, outcome) in [Outcome::Error, Outcome::Timeout, Outcome::Ok]
        .into_iter()
        .enumerate()
    {
        let mut e = event(&format!("e{i}"));
        e.request_id = "req-r".to_string();
        e.attempt = i as u32;
        e.outcome = outcome;
        e.duration_ms = Some(100);
        if outcome != Outcome::Ok {
            e.error_code = Some("boom".to_string());
        }
        attempts.push(e);
    }
    let usage = aggregate_project("p", &attempts, &prices(), None);
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.requests_ok, 1);
    assert_eq!(usage.retries, 2);
    assert_eq!(usage.requests_error, 0);
    // 3 attempts x (1000 in + 200 out).
    assert_eq!(usage.input_tokens.value, Some(3000.0));
    assert_eq!(usage.output_tokens.value, Some(600.0));
    let row = &usage.by_model[0];
    assert_eq!(row.retries, 2);
    assert_eq!(row.success_rate, Some(1.0));
}

#[test]
fn reported_cost_kept_distinct_from_estimate() {
    let mut e = event("e1");
    e.cost_usd = Some(0.5);
    e.cost_provenance = Some(CostProvenance::Reported);
    let usage = aggregate_project("p", &[e], &prices(), None);
    assert_eq!(usage.reported_usd.value, Some(0.5));
    assert_eq!(usage.reported_usd.reporting, 1);
    // Estimate computed independently alongside, never merged.
    assert!(usage.estimated_usd.value.unwrap() < 0.5);
}

#[test]
fn task_rollup_associates_calls() {
    let mut first = event("e1");
    first.task_id = Some("t".to_string());
    first.duration_ms = Some(100);
    first.started_at_ms = 1000;
    let mut second = event("e2");
    second.request_id = "req-2".to_string();
    second.task_id = Some("t".to_string());
    second.provider = "mistral".to_string();
    second.model = "m2".to_string();
    second.duration_ms = Some(200);
    second.started_at_ms = 1100;
    second.input_tokens = None; // unobserved here: stays absent
    let (rollups, skipped) = aggregate_tasks(&[first, second], &prices(), None);
    assert_eq!(skipped, 0);
    assert_eq!(rollups.len(), 1);
    let task = &rollups[0];
    assert_eq!(task.calls, 2);
    assert_eq!(task.requests, 2);
    assert!(task.success);
    assert_eq!(task.duration_ms, Some(300));
    assert_eq!(
        task.models,
        vec!["m2".to_string(), "openai/gpt-oss-120b".to_string()]
    );
    assert_eq!(task.input_tokens.reporting, 1);
    assert_eq!(task.input_tokens.total, 2);
}

#[test]
fn streaming_and_token_categories_preserved() {
    let mut e = event("e1");
    e.streaming = true;
    e.ttft_ms = Some(45);
    e.cached_tokens = Some(500);
    e.reasoning_tokens = Some(60);
    let dir = tmpdir("stream");
    let store = UsageStore::new(&dir);
    store
        .import_str("p", &serde_json::to_string(&e).unwrap(), None, None)
        .unwrap();
    let (events, _) = store.read_events("p").unwrap();
    assert!(events[0].streaming);
    assert_eq!(events[0].ttft_ms, Some(45));
    assert_eq!(events[0].cached_tokens, Some(500));
    assert_eq!(events[0].reasoning_tokens, Some(60));
    assert_eq!(events[0].prompt_id.as_deref(), Some("review/v3"));
}

#[test]
fn contexts_aggregate_separately() {
    let mut prod = event("e1");
    prod.context = UsageContext::Production;
    let mut bench = event("e2");
    bench.context = UsageContext::Benchmark;
    bench.request_id = "req-2".to_string();
    let events = vec![prod, bench];
    let all = aggregate_project("p", &events, &prices(), None);
    assert_eq!(all.events, 2);
    let only_prod = aggregate_project("p", &events, &prices(), Some(UsageContext::Production));
    assert_eq!(only_prod.events, 1);
    assert_eq!(only_prod.requests, 1);
    let only_bench = aggregate_project("p", &events, &prices(), Some(UsageContext::Benchmark));
    assert_eq!(only_bench.events, 1);
    // Production + benchmark are never summed without saying so:
    // filtered views partition the unfiltered view.
    assert_eq!(only_prod.events + only_bench.events, all.events);
}

#[test]
fn report_on_unknown_project_is_empty_not_an_error() {
    let dir = tmpdir("unknown-project");
    let store = UsageStore::new(&dir);
    let (events, skipped) = store.read_events("ghost").unwrap();
    assert!(events.is_empty());
    assert_eq!(skipped, 0);
    let usage = aggregate_project("ghost", &events, &PriceTable::empty(), None);
    assert_eq!(usage.events, 0);
    assert_eq!(usage.requests, 0);
    assert_eq!(usage.input_tokens.value, None);
    assert_eq!(usage.by_model.len(), 0);
}

#[test]
fn crash_between_event_and_index_stays_idempotent() {
    // Simulate a crash after the event append but before the index
    // append: hand-write the event line, then import the same event.
    // The union seen-set must recognize it as a duplicate.
    let dir = tmpdir("crash");
    let store = UsageStore::new(&dir);
    let line = serde_json::to_string(&event("e1")).unwrap();
    let log = dir.join("usage").join("p").join("events-d9.jsonl");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, format!("{line}\n")).unwrap();
    let summary = store.import_str("p", &line, None, None).unwrap();
    assert_eq!(summary.accepted, 0);
    assert_eq!(summary.duplicates, 1);
    let (events, _) = store.read_events("p").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn daily_buckets_group_days() {
    let mut a = event("e1");
    a.started_at_ms = 1_700_000_000_000;
    let mut b = event("e2");
    b.started_at_ms = 1_700_000_000_000 + 86_400_000;
    let buckets = bucketize_daily(&[a, b]);
    assert_eq!(buckets.len(), 2);
    assert!(buckets[0].day < buckets[1].day);
    assert_eq!(buckets[0].calls, 1);
}
