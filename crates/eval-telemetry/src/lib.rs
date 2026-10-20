//! Minimal OTLP/HTTP JSON trace exporter.
//!
//! eval-service keeps its own canonical run store; this crate is only the
//! observability seam. Export failures are reported to the caller and never
//! mutate a run or gate decision.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static ID_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq)]
pub enum Attr {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpanData {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_ns: u128,
    pub end_ns: u128,
    pub attrs: BTreeMap<String, Attr>,
    pub error: bool,
}

impl SpanData {
    pub fn new(
        trace_id: impl Into<String>,
        span_id: impl Into<String>,
        parent_span_id: Option<String>,
        name: impl Into<String>,
        start_ns: u128,
        end_ns: u128,
    ) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
            parent_span_id,
            name: name.into(),
            start_ns,
            end_ns,
            attrs: BTreeMap::new(),
            error: false,
        }
    }

    pub fn attr(mut self, key: impl Into<String>, value: Attr) -> Self {
        self.attrs.insert(key.into(), value);
        self
    }

    pub fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }
}

#[derive(Debug, Clone)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub traceparent: String,
}

impl TraceContext {
    pub fn start() -> Self {
        let trace_id = new_trace_id();
        let span_id = new_span_id();
        let traceparent = format!("00-{trace_id}-{span_id}-01");
        Self {
            trace_id,
            span_id,
            traceparent,
        }
    }

    pub fn child(&self) -> Self {
        let span_id = new_span_id();
        Self {
            trace_id: self.trace_id.clone(),
            span_id: span_id.clone(),
            traceparent: format!("00-{}-{span_id}-01", self.trace_id),
        }
    }

    pub fn from_traceparent(value: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = value.trim().split('-').collect();
        if parts.len() != 4 || parts[0] != "00" {
            return None;
        }
        if parts[1].len() != 32
            || parts[2].len() != 16
            || !parts[1].chars().all(|c| c.is_ascii_hexdigit())
            || !parts[2].chars().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        Some((parts[1].to_string(), parts[2].to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct Telemetry {
    endpoint: Option<String>,
    service_name: String,
    client: reqwest::Client,
}

impl Telemetry {
    pub fn from_env() -> Self {
        let endpoint = std::env::var("EVAL_OTEL_ENDPOINT")
            .ok()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").ok())
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
            .filter(|value| !value.trim().is_empty());
        let service_name = std::env::var("OTEL_SERVICE_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "eval-service".to_string());
        Self {
            endpoint,
            service_name,
            client: reqwest::Client::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.endpoint.is_some()
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub async fn export(&self, spans: &[SpanData]) -> Result<(), String> {
        let Some(endpoint) = &self.endpoint else {
            return Ok(());
        };
        if spans.is_empty() {
            return Ok(());
        }
        let url = traces_url(endpoint);
        let payload = otlp_payload(&self.service_name, spans);
        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|error| format!("OTLP export to {url}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("OTLP export to {url}: HTTP {}", response.status()));
        }
        Ok(())
    }
}

pub fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

pub fn new_trace_id() -> String {
    let nanos = now_ns();
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed) as u128;
    format!(
        "{:032x}",
        nanos.rotate_left(17) ^ (seq << 64) ^ u128::from(std::process::id())
    )
}

pub fn new_span_id() -> String {
    let nanos = now_ns();
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:016x}",
        (nanos as u64).rotate_left(13) ^ seq ^ u64::from(std::process::id())
    )
}

fn traces_url(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/v1/traces") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/v1/traces")
    }
}

fn otlp_payload(service_name: &str, spans: &[SpanData]) -> Value {
    let spans: Vec<Value> = spans.iter().map(span_json).collect();
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    attr_json("service.name", &Attr::Str(service_name.to_string())),
                    attr_json("service.version", &Attr::Str(env!("CARGO_PKG_VERSION").to_string())),
                ]
            },
            "scopeSpans": [{
                "scope": {"name": "eval-service", "version": env!("CARGO_PKG_VERSION")},
                "spans": spans
            }]
        }]
    })
}

fn span_json(span: &SpanData) -> Value {
    let mut value = json!({
        "traceId": span.trace_id,
        "spanId": span.span_id,
        "name": span.name,
        "kind": 1,
        "startTimeUnixNano": span.start_ns.to_string(),
        "endTimeUnixNano": span.end_ns.to_string(),
        "attributes": span.attrs.iter().map(|(key, value)| attr_json(key, value)).collect::<Vec<_>>(),
        "status": {"code": if span.error { 2 } else { 1 }}
    });
    if let Some(parent) = &span.parent_span_id {
        value["parentSpanId"] = json!(parent);
    }
    value
}

fn attr_json(key: &str, value: &Attr) -> Value {
    let value = match value {
        Attr::Str(value) => json!({"stringValue": value}),
        Attr::Int(value) => json!({"intValue": value.to_string()}),
        Attr::Float(value) => json!({"doubleValue": value}),
        Attr::Bool(value) => json!({"boolValue": value}),
    };
    json!({"key": key, "value": value})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_is_w3c_shaped_and_round_trips() {
        let context = TraceContext::start();
        assert_eq!(context.trace_id.len(), 32);
        assert_eq!(context.span_id.len(), 16);
        assert!(context.traceparent.starts_with("00-"));
        assert_eq!(
            TraceContext::from_traceparent(&context.traceparent),
            Some((context.trace_id.clone(), context.span_id.clone()))
        );
    }

    #[test]
    fn child_keeps_trace_and_changes_span() {
        let parent = TraceContext::start();
        let child = parent.child();
        assert_eq!(parent.trace_id, child.trace_id);
        assert_ne!(parent.span_id, child.span_id);
        assert!(child.traceparent.contains(&parent.trace_id));
    }

    #[test]
    fn invalid_traceparent_is_rejected() {
        assert_eq!(TraceContext::from_traceparent("bad"), None);
        assert_eq!(
            TraceContext::from_traceparent("00-nothex-0000000000000000-01"),
            None
        );
    }

    #[test]
    fn payload_contains_trace_and_attributes() {
        let span = SpanData::new("a".repeat(32), "b".repeat(16), None, "eval.run", 1, 2)
            .attr("eval.run_id", Attr::Str("run-1".into()))
            .attr("eval.cases", Attr::Int(3));
        let payload = otlp_payload("eval-service", &[span]);
        let item = &payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(item["name"], "eval.run");
        assert_eq!(item["traceId"], "a".repeat(32));
        assert_eq!(item["attributes"][0]["key"], "eval.cases");
    }
}
