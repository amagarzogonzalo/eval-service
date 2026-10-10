use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch (runner-measured wall clocks use this).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// First 8 hex chars of the blake3 hash of `bytes`.
pub fn hash8(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().as_str()[..8].to_string()
}

/// `"<started_ms>-<hash8>"`, stable for identical content.
pub fn new_run_id(started_ms: u64, canonical_bytes: &[u8]) -> String {
    format!("{started_ms}-{}", hash8(canonical_bytes))
}
