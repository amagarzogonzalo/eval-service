//! Build stamp: lets operators tell a stale binary from a fresh one
//! with `eval --version` (see the stale-count investigation: the only
//! unanswered question was *which* binary had run).

fn main() {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=EVAL_BUILD_INFO=0.1.0 (build {secs})");
    println!("cargo:rerun-if-changed=build.rs");
}
