//! `eval`: run suites, compare runs against baselines, render reports,
//! scaffold new adapters.
//!
//! Exit codes: 0 pass, 1 regression/failure, 2 infra/protocol error.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use eval_core::{
    BaselinePointer, GateDecision, Lane, ModelRef, RunManifest, RunRecord, RunStore, Suite,
    TargetSpec, TrialObservation, record::assemble_case,
};
use std::path::{Path, PathBuf};

mod calibrate;
mod gate;
mod notify;
mod templates;
mod watch;

#[derive(Debug, Parser)]
#[command(
    name = "eval",
    version = env!("EVAL_BUILD_INFO"),
    about = "Reusable Rust evaluation platform"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a suite, grade it, and store the record.
    Run {
        /// Suite YAML file.
        suite: PathBuf,
        /// Run ref (baseline pointer namespace). Default `work`.
        #[arg(long, default_value = "work")]
        r#ref: String,
        /// Store root directory. Default `eval-runs`.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Override the target argv (e.g. point at a local adapter).
        #[arg(long)]
        target: Vec<String>,
        /// Execution lane recorded in `manifest.json`.
        #[arg(long, default_value = "batch")]
        lane: String,
        /// Target version recorded in `manifest.json`.
        #[arg(long)]
        target_version: Option<String>,
        /// Model under test as `provider:model` (provider optional).
        #[arg(long)]
        model: Option<String>,
        /// Record that only free-tier/free models were eligible.
        #[arg(long, conflicts_with = "paid")]
        free_only: bool,
        /// Record that paid models were eligible (explicit, never assumed).
        #[arg(long)]
        paid: bool,
        /// Point the suite baseline at this run after storing.
        #[arg(long)]
        set_baseline: bool,
        /// Provenance for this run: live or replay. Required for gated
        /// promotion; replay can never become a live baseline.
        #[arg(long)]
        origin: Option<String>,
        /// Source ref (default: GITHUB_REF, else `local`).
        #[arg(long)]
        source_ref: Option<String>,
        /// Trigger event (default: GITHUB_EVENT_NAME, else `local`).
        #[arg(long)]
        event: Option<String>,
        /// Worker slots (default: min(4, parallelism)). Use 1 to force
        /// shared-process sequencing.
        #[arg(long)]
        workers: Option<usize>,
        /// Run the target in a Docker sandbox: `docker`. Linux runners only.
        #[arg(long)]
        sandbox: Option<String>,
        /// Sandbox image with python3 for the adapter.
        #[arg(long, default_value = "eval-sandbox:24.04")]
        sandbox_image: String,
        /// Internal Docker network shared only with the proxy sidecar.
        #[arg(long, default_value = "eval-internal")]
        sandbox_network: String,
        /// Extra host path to mount read-only at the same absolute path
        /// (repeatable; target venvs and source checkouts).
        #[arg(long)]
        sandbox_mount: Vec<PathBuf>,
        #[arg(long, default_value_t = 2048)]
        sandbox_memory_mb: u64,
        #[arg(long, default_value_t = 2.0)]
        sandbox_cpus: f64,
        #[arg(long, default_value_t = 128)]
        sandbox_pids: u64,
        /// Non-root container identity; must be able to read the mounts.
        #[arg(long, default_value = "65534:65534")]
        sandbox_user: String,
        /// Machine-readable JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Compare two runs and decide the gate.
    Compare {
        /// Suite YAML file (gate policy + case names).
        #[arg(long)]
        suite: PathBuf,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Baseline ref. Default `baseline`.
        #[arg(long, default_value = "baseline")]
        baseline_ref: String,
        /// Candidate ref. Default `work`.
        #[arg(long, default_value = "work")]
        candidate_ref: String,
        /// Explicit run ids (default: latest of each ref).
        #[arg(long)]
        baseline_id: Option<String>,
        #[arg(long)]
        candidate_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Render stored runs (one `--suite` per suite section).
    Report {
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Suite name(s) to render.
        #[arg(long, required = true)]
        suite: Vec<String>,
        /// Run ref. Default `work`.
        #[arg(long, default_value = "work")]
        r#ref: String,
        /// Explicit run id (default: latest).
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create `manifest.json` sidecars for legacy run directories.
    ManifestBackfill {
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Scaffold a runnable adapter + suite + readme in `--dir`.
    Init {
        /// Adapter name.
        name: String,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Integration pattern: `cli` (wrap a command), `llm` (wrap a
        /// model call, emit usage), `agent` (drive a multi-step
        /// workflow with events).
        #[arg(long, default_value = "cli")]
        template: String,
        /// Overwrite an existing scaffold.
        #[arg(long)]
        force: bool,
    },
    /// List registered projects and their evaluation status.
    Projects {
        /// Workspace file (default `eval-workspace.toml`).
        #[arg(long, default_value = "eval-workspace.toml")]
        workspace: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Render a read-only static dashboard from canonical run stores.
    Dashboard {
        /// Workspace file (default `eval-workspace.toml`).
        #[arg(long, default_value = "eval-workspace.toml")]
        workspace: PathBuf,
        /// Output directory for `index.html`.
        #[arg(long, default_value = "dashboard")]
        out: PathBuf,
    },
    /// Record LLM-call usage events from a JSONL file.
    UsageImport {
        /// JSONL event file (one `llm/v1` event per line).
        file: PathBuf,
        /// Project id the batch belongs to.
        #[arg(long)]
        project: String,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Tag stamped on events that lack one (benchmark/variant label).
        #[arg(long)]
        tag: Option<String>,
        /// Run id stamped on events that lack one (joins usage to grades).
        #[arg(long)]
        run_id: Option<String>,
        /// Suite file: resolves the latest run id of --ref automatically
        /// and binds --tag to that run for dashboard comparison.
        #[arg(long)]
        suite: Option<PathBuf>,
        /// Ref used with --suite (default `work`).
        #[arg(long = "ref", default_value = "work")]
        ref_name: String,
        #[arg(long)]
        json: bool,
    },
    /// Report aggregated LLM usage for a project.
    UsageReport {
        /// Project id.
        #[arg(long)]
        project: String,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Pricing table (default `<store>/pricing.toml`, else unknown costs).
        #[arg(long)]
        pricing: Option<PathBuf>,
        /// Filter to one model.
        #[arg(long)]
        model: Option<String>,
        /// Filter to one tag.
        #[arg(long)]
        tag: Option<String>,
        /// Filter to one traffic context: production, benchmark, or dev.
        /// Default aggregates all contexts (labeled as such).
        #[arg(long)]
        context: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Run a loopback-only local collector for usage events.
    Collect {
        /// Port to bind on 127.0.0.1.
        #[arg(long, default_value_t = 4317)]
        port: u16,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Optional shared-token file (Bearer auth when present).
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    /// Serve a loopback OpenAI-compatible reverse proxy that observes
    /// real application LLM calls: injects the provider key from
    /// `--key-env` (the app sees only the loopback URL), applies
    /// optional model swaps, passes non-streaming and SSE responses
    /// through, enforces a request/cost budget, and records one
    /// llm/v1 event per attempt in the usage store.
    Proxy {
        #[arg(long, default_value_t = 8787)]
        port: u16,
        /// Bind address. Default loopback; use 0.0.0.0 only inside a
        /// Docker sidecar network.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Upstream OpenAI-compatible base URL, e.g.
        /// <https://api.groq.com/openai/v1>.
        upstream: String,
        /// Env var holding the real provider key (read once, never
        /// logged or stored).
        #[arg(long)]
        key_env: String,
        /// Project id stamped on every usage event.
        #[arg(long)]
        project: String,
        /// Usage store root.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Optional tag stamped on every event.
        #[arg(long)]
        tag: Option<String>,
        /// Optional run id stamped on every event.
        #[arg(long)]
        run_id: Option<String>,
        /// Provider label for usage events.
        #[arg(long, default_value = "openai-compat")]
        provider: String,
        /// Traffic context: benchmark | production | dev.
        #[arg(long, default_value = "benchmark")]
        context: String,
        /// Model swap `requested=served`; repeatable.
        #[arg(long = "override")]
        overrides: Vec<String>,
        /// Max forwarded requests; rejects beyond it.
        #[arg(long)]
        max_requests: Option<u32>,
        /// Max lifetime cost (USD); requires both per-1M prices (fail
        /// closed otherwise).
        #[arg(long)]
        max_cost_usd: Option<f64>,
        /// Input price per 1M tokens (reservation accounting).
        #[arg(long)]
        price_in_per_1m: Option<f64>,
        /// Output price per 1M tokens (reservation accounting).
        #[arg(long)]
        price_out_per_1m: Option<f64>,
        /// Record successful exchanges as cassettes under this
        /// directory (for deterministic offline replay).
        #[arg(long)]
        cassette_dir: Option<PathBuf>,
        /// Reserved output tokens per request when the caller omits
        /// max_tokens.
        #[arg(long, default_value_t = 1024)]
        out_reserve_tokens: u64,
        /// Upstream timeout in seconds.
        #[arg(long, default_value_t = 120)]
        upstream_timeout_s: u64,
    },
    /// Serve recorded cassettes as an offline OpenAI-compatible replay
    /// endpoint. Replay is deterministic and honest: an unmatched
    /// request is a 404 `cassette_miss`, and every hit stamps
    /// `origin=replay` on its llm/v1 event — replay must never satisfy
    /// a live gate.
    Cassette {
        /// Directory holding recorded cassette JSON files.
        dir: PathBuf,
        #[arg(long, default_value_t = 8788)]
        port: u16,
        /// Project id stamped on replay usage events.
        #[arg(long)]
        project: String,
        /// Usage store root.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Optional tag stamped on every event.
        #[arg(long)]
        tag: Option<String>,
        /// Optional run id stamped on every event.
        #[arg(long)]
        run_id: Option<String>,
        /// Provider label for usage events.
        #[arg(long, default_value = "openai-compat")]
        provider: String,
        /// Traffic context: benchmark | production | dev.
        #[arg(long, default_value = "benchmark")]
        context: String,
    },
    /// Print the machine-readable platform manifest (for agents/tooling).
    Schema {
        /// Emit compact one-line JSON instead of pretty.
        #[arg(long)]
        compact: bool,
    },
    /// Validate a suite and (with --probe) conformance-test its adapter.
    /// The deterministic gate an agent runs while building an integration.
    Check {
        /// Suite YAML file.
        suite: PathBuf,
        /// Spawn the target and run one case end-to-end to verify the
        /// eval/v1 handshake and terminal message.
        #[arg(long)]
        probe: bool,
        #[arg(long)]
        json: bool,
    },
    /// Calibrate a judge against human labels and write a versioned
    /// report. Offline: it measures labels/scores the judge already
    /// produced and never calls a model.
    Calibrate {
        /// JSONL rows with human labels and judge labels/scores.
        dataset: PathBuf,
        /// JSON report output.
        #[arg(long, default_value = "judge-calibration.json")]
        out: PathBuf,
        /// Optional Markdown summary.
        #[arg(long)]
        markdown: Option<PathBuf>,
        /// Judge id pinned in the report (e.g. provider/model@revision).
        #[arg(long)]
        judge_model: Option<String>,
        /// Prompt/rubric revision string for the judge.
        #[arg(long)]
        prompt_version: Option<String>,
        /// Rubric id; defaults to the first non-empty rubric in the set.
        #[arg(long)]
        rubric: Option<String>,
        /// PASS threshold over judge scores in [0, 10].
        #[arg(long, default_value_t = 8.0)]
        threshold: f64,
        /// Minimum kappa lower CI for gating eligibility.
        #[arg(long, default_value_t = 0.6)]
        kappa_min: f64,
        /// Confidence level for the kappa bootstrap interval.
        #[arg(long, default_value_t = 0.95)]
        ci: f64,
        /// Deterministic bootstrap replicates.
        #[arg(long, default_value_t = 1000)]
        bootstrap: u32,
        /// Minimum labelled items for gating eligibility.
        #[arg(long, default_value_t = 30)]
        min_items: usize,
        /// Minimum A/B both-order pairs for position checks.
        #[arg(long, default_value_t = 10)]
        min_position_items: usize,
        /// Minimum A/B order consistency for gating eligibility.
        #[arg(long, default_value_t = 0.8)]
        min_position_consistency: f64,
        /// Maximum judge-minus-human "picks longer" gap.
        #[arg(long, default_value_t = 0.2)]
        max_verbosity_gap: f64,
        /// Exit 1 when the evidence does not clear the gating bar.
        #[arg(long)]
        require_gating: bool,
        /// Print the JSON report to stdout as well as writing it.
        #[arg(long)]
        json: bool,
    },
    /// Poll GitHub read-only for `eval-request` issues and run
    /// allowlisted suites with a restart-safe queue and heartbeat.
    Watch {
        /// Watch config file.
        #[arg(long, default_value = "eval-watch.toml")]
        config: PathBuf,
        /// Poll and process once, then exit.
        #[arg(long)]
        once: bool,
        /// Poll and print due requests without running or saving them.
        #[arg(long)]
        dry_run: bool,
        /// Override the poll interval in seconds.
        #[arg(long)]
        interval: Option<u64>,
        /// Override the run store path.
        #[arg(long)]
        store: Option<PathBuf>,
        /// Override the queue state path.
        #[arg(long)]
        state: Option<PathBuf>,
        /// Override the read-only GitHub token environment variable.
        #[arg(long)]
        token_env: Option<String>,
        /// Override the GitHub API base URL (tests/local proxy).
        #[arg(long)]
        base_url: Option<String>,
        /// Override the issue-settling debounce in seconds.
        #[arg(long)]
        debounce_seconds: Option<u64>,
    },
    /// Compare a candidate against a baseline and, optionally, promote
    /// only a policy-compliant live main-branch run.
    Gate {
        /// Suite YAML file.
        #[arg(long)]
        suite: PathBuf,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        #[arg(long, default_value = "baseline")]
        baseline_ref: String,
        #[arg(long, default_value = "work")]
        candidate_ref: String,
        #[arg(long)]
        baseline_id: Option<String>,
        #[arg(long)]
        candidate_id: Option<String>,
        /// Require the candidate provenance to be live or replay.
        #[arg(long)]
        origin: Option<String>,
        /// Promote the passing candidate to the suite baseline after the
        /// live/main/sandbox policy check.
        #[arg(long)]
        promote: bool,
        #[arg(long)]
        json: bool,
    },
    /// Post an idempotent comment/status using a narrow-write token that
    /// is separate from the read-only watch token.
    Notify {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        issue: Option<u64>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long, default_value = "eval-gate")]
        marker: String,
        #[arg(long, default_value = "EVAL_GITHUB_WRITE_TOKEN")]
        token_env: String,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        status_sha: Option<String>,
        #[arg(long)]
        status_state: Option<String>,
        #[arg(long)]
        status_context: Option<String>,
        #[arg(long, default_value = "https://api.github.com")]
        base_url: String,
    },
    /// One command from zero to a verified baseline: scaffold, check
    /// (with probe), then run and set the baseline. Stops at the first
    /// failure so an agent gets a single pass/fail.
    Onboard {
        /// Adapter name.
        name: String,
        /// Integration pattern: cli | llm | agent.
        #[arg(long, default_value = "cli")]
        template: String,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
    },
}

fn main() {
    let code = std::thread::Builder::new()
        .name("eval-main".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("building tokio runtime");
            runtime.block_on(async_main())
        })
        .expect("spawning eval main thread")
        .join()
        .unwrap_or(2);
    std::process::exit(code);
}

async fn async_main() -> i32 {
    let cli = Cli::parse();
    match Box::pin(run(cli)).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("eval: error: {e:#}");
            2
        }
    }
}

async fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Run {
            suite,
            r#ref,
            store,
            target,
            lane,
            target_version,
            model,
            free_only,
            paid,
            set_baseline,
            origin,
            source_ref,
            event,
            workers,
            sandbox,
            sandbox_image,
            sandbox_network,
            sandbox_mount,
            sandbox_memory_mb,
            sandbox_cpus,
            sandbox_pids,
            sandbox_user,
            json,
        } => {
            cmd_run(
                &suite,
                &r#ref,
                &store,
                target,
                &lane,
                target_version,
                model,
                if free_only {
                    Some(true)
                } else if paid {
                    Some(false)
                } else {
                    None
                },
                set_baseline,
                origin,
                source_ref,
                event,
                workers,
                sandbox.as_deref(),
                sandbox_image,
                sandbox_network,
                sandbox_mount,
                sandbox_memory_mb,
                sandbox_cpus,
                sandbox_pids,
                sandbox_user,
                json,
            )
            .await
        }
        Command::Compare {
            suite,
            store,
            baseline_ref,
            candidate_ref,
            baseline_id,
            candidate_id,
            json,
        } => cmd_compare(
            &suite,
            &store,
            &baseline_ref,
            &candidate_ref,
            baseline_id,
            candidate_id,
            json,
        ),
        Command::Report {
            store,
            suite,
            r#ref,
            run_id,
            json,
        } => cmd_report(&store, &suite, &r#ref, run_id, json),
        Command::ManifestBackfill { store, json } => cmd_manifest_backfill(&store, json),
        Command::Init {
            name,
            dir,
            template,
            force,
        } => cmd_init(&name, &dir, &template, force),
        Command::Projects { workspace, json } => cmd_projects(&workspace, json),
        Command::Dashboard { workspace, out } => cmd_dashboard(&workspace, &out),
        Command::UsageImport {
            file,
            project,
            store,
            tag,
            run_id,
            suite,
            ref_name,
            json,
        } => cmd_usage_import(
            &file,
            &project,
            &store,
            tag.as_deref(),
            run_id.as_deref(),
            suite.as_deref(),
            &ref_name,
            json,
        ),
        Command::UsageReport {
            project,
            store,
            pricing,
            model,
            tag,
            context,
            json,
        } => cmd_usage_report(
            &project,
            &store,
            pricing.as_deref(),
            model.as_deref(),
            tag.as_deref(),
            context.as_deref(),
            json,
        ),
        Command::Collect {
            port,
            store,
            token_file,
        } => cmd_collect(port, &store, token_file.as_deref()).await,
        Command::Proxy {
            port,
            bind,
            upstream,
            key_env,
            project,
            store,
            tag,
            run_id,
            provider,
            context,
            overrides,
            max_requests,
            max_cost_usd,
            price_in_per_1m,
            price_out_per_1m,
            cassette_dir,
            out_reserve_tokens,
            upstream_timeout_s,
        } => {
            cmd_proxy(
                port,
                &bind,
                &upstream,
                &key_env,
                &project,
                &store,
                tag.as_deref(),
                run_id.as_deref(),
                &provider,
                &context,
                &overrides,
                max_requests,
                max_cost_usd,
                price_in_per_1m,
                price_out_per_1m,
                cassette_dir.as_deref(),
                out_reserve_tokens,
                upstream_timeout_s,
            )
            .await
        }
        Command::Cassette {
            dir,
            port,
            project,
            store,
            tag,
            run_id,
            provider,
            context,
        } => {
            cmd_cassette(
                &dir,
                port,
                &project,
                &store,
                tag.as_deref(),
                run_id.as_deref(),
                &provider,
                &context,
            )
            .await
        }
        Command::Schema { compact } => cmd_schema(compact),
        Command::Check { suite, probe, json } => cmd_check(&suite, probe, json).await,
        Command::Calibrate {
            dataset,
            out,
            markdown,
            judge_model,
            prompt_version,
            rubric,
            threshold,
            kappa_min,
            ci,
            bootstrap,
            min_items,
            min_position_items,
            min_position_consistency,
            max_verbosity_gap,
            require_gating,
            json,
        } => calibrate::cmd_calibrate(calibrate::CalibrateArgs {
            dataset,
            out,
            markdown,
            judge_model,
            prompt_version,
            rubric,
            threshold,
            kappa_min,
            ci_level: ci,
            bootstrap_reps: bootstrap,
            min_items,
            min_position_items,
            min_position_consistency,
            max_verbosity_gap,
            require_gating,
            json,
        }),
        Command::Watch {
            config,
            once,
            dry_run,
            interval,
            store,
            state,
            token_env,
            base_url,
            debounce_seconds,
        } => {
            watch::cmd_watch(watch::WatchArgs {
                config,
                once,
                dry_run,
                interval,
                store,
                state,
                token_env,
                base_url,
                debounce_seconds,
            })
            .await
        }
        Command::Gate {
            suite,
            store,
            baseline_ref,
            candidate_ref,
            baseline_id,
            candidate_id,
            origin,
            promote,
            json,
        } => gate::cmd_gate(gate::GateArgs {
            suite,
            store,
            baseline_ref,
            candidate_ref,
            baseline_id,
            candidate_id,
            origin,
            promote,
            json,
        }),
        Command::Notify {
            repo,
            issue,
            body,
            marker,
            token_env,
            token_file,
            status_sha,
            status_state,
            status_context,
            base_url,
        } => {
            notify::cmd_notify(notify::NotifyArgs {
                repo,
                base_url,
                issue,
                body,
                marker,
                token_env,
                token_file,
                status_sha,
                status_state,
                status_context,
            })
            .await
        }
        Command::Onboard {
            name,
            template,
            dir,
            store,
        } => cmd_onboard(&name, &dir, &template, &store).await,
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    suite_path: &Path,
    ref_name: &str,
    store_root: &Path,
    target_override: Vec<String>,
    lane: &str,
    target_version: Option<String>,
    model: Option<String>,
    free_only: Option<bool>,
    set_baseline: bool,
    origin: Option<String>,
    source_ref: Option<String>,
    event: Option<String>,
    workers: Option<usize>,
    sandbox: Option<&str>,
    sandbox_image: String,
    sandbox_network: String,
    sandbox_mount: Vec<PathBuf>,
    sandbox_memory_mb: u64,
    sandbox_cpus: f64,
    sandbox_pids: u64,
    sandbox_user: String,
    as_json: bool,
) -> Result<i32> {
    let suite = Suite::load(suite_path).context("loading suite")?;
    let origin = origin.as_deref().map(gate::Origin::parse).transpose()?;
    if set_baseline && origin == Some(gate::Origin::Replay) {
        bail!(
            "origin=replay cannot set a live baseline; use a live main-branch run and gate promotion"
        );
    }
    let origin_label = origin.map(gate::Origin::as_str).unwrap_or("unknown");
    let mut target: TargetSpec = suite.target.clone().context("suite has no target")?;
    if !target_override.is_empty() {
        target.argv = target_override;
    }
    let target_label = target.argv.join(" ");
    let trace = eval_telemetry::TraceContext::start();
    let telemetry = eval_telemetry::Telemetry::from_env();
    let mut policy = eval_runner::RunPolicy::default_for(suite.env_allowlist.clone());
    policy.traceparent = Some(trace.traceparent.clone());
    policy.trace_id = Some(trace.trace_id.clone());
    if let Some(w) = workers {
        policy.workers = w.max(1);
    }
    if let Some(kind) = sandbox {
        if kind != "docker" {
            bail!("unknown sandbox {kind:?}; supported: docker");
        }
        if cfg!(windows) {
            bail!("docker sandbox is supported on the Linux runner (or WSL), not native Windows");
        }
        policy.sandbox = Some(eval_runner::SandboxSpec {
            image: sandbox_image,
            network: sandbox_network,
            mounts: sandbox_mount,
            memory_mb: sandbox_memory_mb,
            cpus: sandbox_cpus,
            pids: sandbox_pids,
            user: sandbox_user,
        });
    }
    let suite_dir = suite_path.parent().unwrap_or(Path::new("."));
    let files_root = std::env::current_dir().context("resolving cwd")?;
    let manifest = ManifestOptions {
        lane: parse_lane(lane)?,
        eval_version: None,
        target_version,
        model: model.as_deref().map(parse_model).transpose()?,
        free_only,
    };

    let started_ns = eval_telemetry::now_ns();
    let (record, run_id) = match execute_and_store(
        &suite,
        target,
        &policy,
        ref_name,
        store_root,
        &files_root,
        set_baseline,
        None,
        &manifest,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let span = eval_telemetry::SpanData::new(
                trace.trace_id.clone(),
                trace.span_id.clone(),
                None,
                "eval.run",
                started_ns,
                eval_telemetry::now_ns(),
            )
            .attr("eval.suite", eval_telemetry::Attr::Str(suite.suite.clone()))
            .attr("eval.target", eval_telemetry::Attr::Str(target_label))
            .attr(
                "eval.origin",
                eval_telemetry::Attr::Str(origin_label.into()),
            )
            .attr(
                "eval.sandbox",
                eval_telemetry::Attr::Bool(sandbox.is_some()),
            )
            .error(true);
            if let Err(export_error) = telemetry.export(&[span]).await {
                eprintln!("eval: telemetry export failed: {export_error}");
            }
            return Err(error);
        }
    };

    if let Some(origin) = origin {
        let source_ref = source_ref
            .or_else(|| std::env::var("GITHUB_REF").ok())
            .unwrap_or_else(|| "local".to_string());
        let event = event
            .or_else(|| std::env::var("GITHUB_EVENT_NAME").ok())
            .unwrap_or_else(|| "local".to_string());
        let provenance = gate::Provenance::new(origin, source_ref, event, sandbox.is_some());
        gate::write_provenance(store_root, &suite.suite, ref_name, &run_id, &provenance)?;
    }

    let spans = run_spans(
        &record,
        &trace,
        &run_id,
        &target_label,
        origin_label,
        sandbox.is_some(),
        started_ns,
    );
    write_trace_sidecar(store_root, &suite.suite, ref_name, &run_id, &trace);
    if let Err(export_error) = telemetry.export(&spans).await {
        eprintln!("eval: telemetry export failed: {export_error}");
    }

    if as_json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        print_human(&record, None);
        println!("run: {run_id} (ref {ref_name})");
    }
    let _ = suite_dir;
    Ok(if record.pass() { 0 } else { 1 })
}

fn run_spans(
    record: &RunRecord,
    trace: &eval_telemetry::TraceContext,
    run_id: &str,
    target: &str,
    origin: &str,
    sandboxed: bool,
    started_ns: u128,
) -> Vec<eval_telemetry::SpanData> {
    let end_ns = eval_telemetry::now_ns();
    let mut spans = vec![
        eval_telemetry::SpanData::new(
            trace.trace_id.clone(),
            trace.span_id.clone(),
            None,
            "eval.run",
            started_ns,
            end_ns,
        )
        .attr("eval.run_id", eval_telemetry::Attr::Str(run_id.to_string()))
        .attr(
            "eval.suite",
            eval_telemetry::Attr::Str(record.suite.clone()),
        )
        .attr("eval.target", eval_telemetry::Attr::Str(target.to_string()))
        .attr("eval.origin", eval_telemetry::Attr::Str(origin.to_string()))
        .attr("eval.sandbox", eval_telemetry::Attr::Bool(sandboxed))
        .attr(
            "eval.cases",
            eval_telemetry::Attr::Int(record.cases.len() as i64),
        )
        .attr(
            "eval.trials",
            eval_telemetry::Attr::Int(record.trials_total as i64),
        )
        .attr("eval.pass", eval_telemetry::Attr::Bool(record.pass()))
        .error(!record.pass()),
    ];
    for case in &record.cases {
        let case_span_id = eval_telemetry::new_span_id();
        spans.push(
            eval_telemetry::SpanData::new(
                trace.trace_id.clone(),
                case_span_id.clone(),
                Some(trace.span_id.clone()),
                "eval.case",
                started_ns,
                end_ns,
            )
            .attr(
                "eval.case_id",
                eval_telemetry::Attr::Str(case.case_id.clone()),
            )
            .attr("eval.pass", eval_telemetry::Attr::Bool(case.pass))
            .attr(
                "eval.deterministic",
                eval_telemetry::Attr::Bool(case.deterministic),
            )
            .error(!case.pass),
        );
        for trial in &case.trials {
            let trial_end = started_ns + u128::from(trial.wall_ms) * 1_000_000;
            spans.push(
                eval_telemetry::SpanData::new(
                    trace.trace_id.clone(),
                    eval_telemetry::new_span_id(),
                    Some(case_span_id.clone()),
                    "eval.trial",
                    started_ns,
                    trial_end,
                )
                .attr(
                    "eval.case_id",
                    eval_telemetry::Attr::Str(case.case_id.clone()),
                )
                .attr(
                    "eval.trial",
                    eval_telemetry::Attr::Int(i64::from(trial.trial)),
                )
                .attr(
                    "eval.status",
                    eval_telemetry::Attr::Str(trial.status.as_str().to_string()),
                )
                .attr(
                    "eval.wall_ms",
                    eval_telemetry::Attr::Int(trial.wall_ms as i64),
                )
                .error(trial.status.is_infra()),
            );
        }
    }
    spans
}

fn write_trace_sidecar(
    store_root: &Path,
    suite: &str,
    ref_name: &str,
    run_id: &str,
    trace: &eval_telemetry::TraceContext,
) {
    let path = store_root
        .join("runs")
        .join(suite)
        .join(ref_name)
        .join(format!("run-{run_id}"))
        .join("trace.json");
    let body = serde_json::json!({
        "schema_version": 1,
        "trace_id": trace.trace_id,
        "traceparent": trace.traceparent,
    });
    let _ = std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap_or_default());
}

struct ManifestOptions {
    lane: Lane,
    eval_version: Option<String>,
    target_version: Option<String>,
    model: Option<ModelRef>,
    free_only: Option<bool>,
}

impl ManifestOptions {
    pub(crate) fn batch() -> Self {
        Self {
            lane: Lane::Batch,
            eval_version: None,
            target_version: None,
            model: None,
            free_only: None,
        }
    }
}

fn parse_lane(value: &str) -> Result<Lane> {
    Lane::ALL
        .iter()
        .copied()
        .find(|lane| lane.as_str() == value)
        .with_context(|| {
            format!(
                "unknown lane {value:?}; choose one of {:?}",
                Lane::ALL.map(|lane| lane.as_str())
            )
        })
}

fn parse_model(value: &str) -> Result<ModelRef> {
    if value.trim().is_empty() {
        bail!("--model must be non-empty");
    }
    let (provider, model) = match value.split_once(':') {
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
            (Some(provider.to_string()), model.to_string())
        }
        _ => (None, value.to_string()),
    };
    Ok(ModelRef { provider, model })
}

/// Execute a suite and persist it (used by `run` and `onboard`).
#[allow(clippy::too_many_arguments)]
async fn execute_and_store(
    suite: &Suite,
    target: TargetSpec,
    policy: &eval_runner::RunPolicy,
    ref_name: &str,
    store_root: &Path,
    files_root: &Path,
    set_baseline: bool,
    request_id: Option<&str>,
    manifest_options: &ManifestOptions,
) -> Result<(RunRecord, String)> {
    let raw = eval_runner::run_suite(suite, &target, policy, files_root)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let (mut record, observations) = assemble(suite, target, files_root, ref_name, raw, request_id);
    let store = RunStore::new(store_root);
    let run_id = store
        .save(&mut record, &observations)
        .context("storing run")?;
    let mut manifest = RunManifest::from_record(
        &record,
        Some(manifest_options.lane),
        manifest_options
            .eval_version
            .clone()
            .or_else(|| Some(record.code_version.clone())),
        manifest_options.target_version.clone(),
        manifest_options.model.clone(),
        manifest_options.free_only,
    );
    manifest.budgets.max_case_timeout_ms =
        suite.cases.iter().map(|case| case.limits.timeout_ms).max();
    manifest.budgets.max_output_bytes = suite
        .cases
        .iter()
        .map(|case| case.limits.max_output_bytes)
        .max();
    store
        .save_manifest(&suite.suite, ref_name, &run_id, &manifest)
        .context("storing run manifest")?;
    if set_baseline {
        store
            .set_baseline(&BaselinePointer {
                suite: suite.suite.clone(),
                ref_name: ref_name.to_string(),
                run_id: run_id.clone(),
            })
            .context("setting baseline")?;
    }
    Ok((record, run_id))
}

fn assemble(
    suite: &Suite,
    target: TargetSpec,
    files_root: &Path,
    ref_name: &str,
    raw: eval_runner::SuiteRun,
    request_id: Option<&str>,
) -> (RunRecord, Vec<TrialObservation>) {
    let mut cases = Vec::new();
    let mut observations = Vec::new();
    for spec in &suite.cases {
        let trials = raw
            .cases
            .iter()
            .find(|c| c.case_id == spec.id)
            .map(|c| c.trials.clone())
            .unwrap_or_default();
        let grades = eval_graders::grade_case(spec, &trials, files_root);
        observations.extend(trials.clone());
        cases.push(assemble_case(spec, trials, grades));
    }
    let trials_total = cases.iter().map(|c| c.trials.len()).sum();
    (
        RunRecord {
            schema_version: eval_core::record::SCHEMA_VERSION,
            run_id: String::new(),
            suite: suite.suite.clone(),
            suite_version: suite.version,
            code_version: RunRecord::code_version(),
            ref_name: ref_name.to_string(),
            started_at_ms: raw.started_ms,
            request_id: request_id.map(str::to_string),
            target: Some(target),
            files_root: files_root.display().to_string(),
            trials_total,
            cases,
            gate: None,
        },
        observations,
    )
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

fn cmd_compare(
    suite_path: &Path,
    store_root: &Path,
    baseline_ref: &str,
    candidate_ref: &str,
    baseline_id: Option<String>,
    candidate_id: Option<String>,
    as_json: bool,
) -> Result<i32> {
    let suite = Suite::load(suite_path).context("loading suite")?;
    let store = RunStore::new(store_root);
    let resolve = |ref_name: &str, id: Option<String>| -> Result<(String, RunRecord)> {
        let id = match id {
            Some(id) => id,
            None => store
                .latest(&suite.suite, ref_name)?
                .context(format!("no runs for {}/{ref_name}", suite.suite))?,
        };
        Ok((id.clone(), store.load(&suite.suite, ref_name, &id)?))
    };
    let (base_id, baseline) = resolve(baseline_ref, baseline_id)?;
    let (cand_id, mut candidate) = resolve(candidate_ref, candidate_id)?;
    let baseline_manifest = store.load_manifest(&suite.suite, baseline_ref, &base_id)?;
    let candidate_manifest = store.load_manifest(&suite.suite, candidate_ref, &cand_id)?;
    if let Some(reason) = eval_graders::comparison_blocker(
        &baseline,
        &candidate,
        baseline_manifest.as_ref(),
        candidate_manifest.as_ref(),
    ) {
        anyhow::bail!("runs are not like-for-like: {reason}; no regression claim");
    }

    let decision: GateDecision =
        eval_graders::decide_gate(Some(&baseline), &candidate, &suite.gate);
    let pass = decision.pass;
    candidate.gate = Some(decision.clone());
    // Persist the decision alongside the candidate run.
    let decision_path = store_root
        .join("runs")
        .join(&suite.suite)
        .join(candidate_ref)
        .join(format!("run-{cand_id}"))
        .join("decision.json");
    std::fs::write(&decision_path, serde_json::to_string_pretty(&decision)?)?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&decision)?);
    } else {
        print_decision(&suite.suite, &base_id, &cand_id, &decision);
    }
    Ok(if pass { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// report
// ---------------------------------------------------------------------------

fn cmd_report(
    store_root: &Path,
    suites: &[String],
    ref_name: &str,
    run_id: Option<String>,
    as_json: bool,
) -> Result<i32> {
    let store = RunStore::new(store_root);
    let mut records = Vec::new();
    for suite in suites {
        let id = match &run_id {
            Some(id) => id.clone(),
            None => store
                .latest(suite, ref_name)?
                .context(format!("no runs for {suite}/{ref_name}"))?,
        };
        records.push(store.load(suite, ref_name, &id)?);
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        for record in &records {
            print_human(record, None);
            println!();
        }
    }
    Ok(if records.iter().all(|r| r.pass()) {
        0
    } else {
        1
    })
}

// ---------------------------------------------------------------------------
// manifest backfill
// ---------------------------------------------------------------------------

fn cmd_manifest_backfill(store_root: &Path, as_json: bool) -> Result<i32> {
    let store = RunStore::new(store_root);
    let report = store
        .backfill_manifests()
        .context("backfilling manifests")?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "manifest backfill: {} created, {} existing, {} failed",
            report.created,
            report.existing,
            report.failed.len()
        );
        for failed in &report.failed {
            println!("  failed: {failed}");
        }
    }
    Ok(if report.failed.is_empty() { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn cmd_init(name: &str, dir: &Path, template: &str, force: bool) -> Result<i32> {
    let Some(scaffold) = templates::scaffold(template, name) else {
        anyhow::bail!(
            "unknown template {template:?}; choose one of {:?}",
            templates::TEMPLATE_NAMES
        );
    };
    let root = dir.join(name);
    if root.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            root.display()
        );
    }
    std::fs::create_dir_all(root.join("adapter"))?;
    std::fs::write(root.join("adapter").join("target.py"), scaffold.adapter)?;
    std::fs::write(root.join("suite.yaml"), scaffold.suite)?;
    std::fs::write(root.join("README.md"), scaffold.readme)?;
    println!("scaffolded {} ({template}) in {}", name, root.display());
    println!("next:");
    println!(
        "  cd {}   # target/file paths resolve from the cwd you run eval in",
        root.display()
    );
    println!("  eval check suite.yaml --probe   # verify suite + adapter handshake");
    println!("  eval run suite.yaml --ref baseline --set-baseline");
    Ok(0)
}

// ---------------------------------------------------------------------------
// projects + dashboard
// ---------------------------------------------------------------------------

fn load_portfolio(workspace: &Path) -> Result<eval_graders::portfolio::PortfolioReport> {
    let file = eval_core::WorkspaceFile::load(workspace).context("loading workspace")?;
    let mut report = eval_graders::portfolio::build_portfolio(workspace, &file);
    // Usage views aggregate llm/v1 events only (never legacy ai sums).
    eval_graders::portfolio::attach_usage(workspace, &file, &mut report);
    Ok(report)
}

fn cmd_projects(workspace: &Path, as_json: bool) -> Result<i32> {
    let report = load_portfolio(workspace)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(0);
    }
    println!(
        "workspace {} ({} projects)",
        report.workspace_name,
        report.projects.len()
    );
    for project in &report.projects {
        if !project.available {
            println!(
                "  [unavailable] {} ({}) — {}",
                project.name,
                project.id,
                project
                    .unavailability
                    .as_deref()
                    .unwrap_or("unknown reason")
            );
            continue;
        }
        let regressed = project
            .suites
            .iter()
            .filter(|s| s.regression == Some(true))
            .count();
        let mark = if project.cases_infra > 0 {
            "error"
        } else if project.cases_failed > 0 || regressed > 0 {
            "FAIL"
        } else if project.cases_total == 0 {
            "no runs"
        } else {
            "ok"
        };
        println!(
            "  [{mark}] {} ({}) — {} suites, {} cases ({} pass / {} fail / {} infra), regressed suites: {regressed}",
            project.name,
            project.id,
            project.suites.len(),
            project.cases_total,
            project.cases_passed,
            project.cases_failed,
            project.cases_infra,
        );
        for suite in &project.suites {
            let latest = suite
                .history
                .first()
                .map(|h| {
                    format!(
                        "{}:{} {}",
                        h.suite,
                        h.ref_name,
                        &h.run_id[..h.run_id.len().min(13)]
                    )
                })
                .unwrap_or_else(|| "no runs".to_string());
            println!("      suite {} — latest {latest}", suite.suite_id);
        }
    }
    let t = &report.totals;
    println!(
        "totals: {}/{} projects available, {} cases pass / {} fail / {} infra",
        t.projects_available, t.projects_total, t.cases_passed, t.cases_failed, t.cases_infra
    );
    Ok(0)
}

fn cmd_dashboard(workspace: &Path, out: &Path) -> Result<i32> {
    let report = load_portfolio(workspace)?;
    let html = eval_graders::dashboard::render_portfolio(&report);
    std::fs::create_dir_all(out).context("creating output dir")?;
    let page = out.join("index.html");
    std::fs::write(&page, html).context("writing dashboard")?;
    println!(
        "dashboard: {} projects, {} suites",
        report.projects.len(),
        report.totals.suites_total
    );
    println!("wrote {}", page.display());
    println!(
        "open it with: start {}  (Windows) / open {} (macOS) / xdg-open {} (Linux)",
        page.display(),
        page.display(),
        page.display()
    );
    Ok(0)
}

// ---------------------------------------------------------------------------
// usage import / report / collect
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn cmd_usage_import(
    file: &Path,
    project: &str,
    store_root: &Path,
    tag: Option<&str>,
    run_id: Option<&str>,
    suite_path: Option<&Path>,
    ref_name: &str,
    as_json: bool,
) -> Result<i32> {
    let resolved_run_id: Option<String> = match (run_id, suite_path) {
        (Some(id), _) => Some(id.to_string()),
        (None, Some(suite_path)) => {
            let suite = Suite::load(suite_path).context("loading suite")?;
            let runs = RunStore::new(store_root);
            runs.latest(&suite.suite, ref_name)
                .context("resolving latest run")?
                .context(format!("no runs for {}/{}", suite.suite, ref_name))?
                .into()
        }
        (None, None) => None,
    };
    let store = eval_usage::UsageStore::new(store_root);
    let summary = store
        .import_file(project, file, tag, resolved_run_id.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let (Some(tag), Some(suite_path)) = (tag, suite_path) {
        let suite = Suite::load(suite_path).context("loading suite")?;
        if let Some(run_id) = &resolved_run_id {
            store
                .bind_tag(
                    project,
                    &eval_usage::TagBinding {
                        tag: tag.to_string(),
                        suite: suite.suite.clone(),
                        ref_name: ref_name.to_string(),
                        run_id: run_id.clone(),
                        imported_at_ms: eval_core::now_ms(),
                    },
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "usage import: {} accepted, {} duplicates, {} rejected (project {project})",
            summary.accepted,
            summary.duplicates,
            summary.rejected.len()
        );
        for rejection in &summary.rejected {
            println!("  rejected line {}: {}", rejection.line, rejection.reason);
        }
    }
    Ok(0)
}

fn resolve_pricing(store_root: &Path, explicit: Option<&Path>) -> eval_usage::PriceTable {
    if let Some(path) = explicit {
        return eval_usage::PriceTable::load(path).unwrap_or_else(|e| {
            eprintln!("eval: warning: pricing file unreadable ({e}); costs stay unknown");
            eval_usage::PriceTable::empty()
        });
    }
    let candidate = store_root.join("pricing.toml");
    if candidate.exists() {
        return eval_usage::PriceTable::load(&candidate).unwrap_or_else(|e| {
            eprintln!("eval: warning: pricing file unreadable ({e}); costs stay unknown");
            eval_usage::PriceTable::empty()
        });
    }
    eval_usage::PriceTable::empty()
}

#[allow(clippy::too_many_arguments)]
fn cmd_usage_report(
    project: &str,
    store_root: &Path,
    pricing_path: Option<&Path>,
    model_filter: Option<&str>,
    tag_filter: Option<&str>,
    context_filter: Option<&str>,
    as_json: bool,
) -> Result<i32> {
    let context = match context_filter {
        None => None,
        Some("production") => Some(eval_usage::UsageContext::Production),
        Some("benchmark") => Some(eval_usage::UsageContext::Benchmark),
        Some("dev") => Some(eval_usage::UsageContext::Dev),
        Some(other) => anyhow::bail!("unknown context {other:?} (production|benchmark|dev)"),
    };
    let store = eval_usage::UsageStore::new(store_root);
    let (mut events, skipped) = store
        .read_events(project)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(model) = model_filter {
        events.retain(|e| e.model == model);
    }
    if let Some(tag) = tag_filter {
        events.retain(|e| e.tag.as_deref() == Some(tag));
    }
    let pricing = resolve_pricing(store_root, pricing_path);
    let usage = eval_usage::aggregate_project(project, &events, &pricing, context);
    let (tasks, tasks_skipped) = eval_usage::aggregate_tasks(&events, &pricing, context);
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "usage": usage,
                "tasks": tasks,
                "tasks_without_id_skipped": tasks_skipped,
                "unreadable_stored_lines_skipped": skipped,
            }))?
        );
    } else {
        println!(
            "context filter: {}",
            context_filter.unwrap_or("all (production + benchmark + dev)")
        );
        print_usage_human(&usage, &tasks, skipped);
    }
    Ok(0)
}

fn print_usage_human(
    usage: &eval_usage::ProjectUsage,
    tasks: &[eval_usage::TaskRollup],
    skipped: usize,
) {
    println!(
        "usage {} — {} events, {} calls, {} requests",
        usage.project, usage.events, usage.calls, usage.requests
    );
    println!(
        "  requests: {} ok / {} error / {} timeout, {} retries",
        usage.requests_ok, usage.requests_error, usage.requests_timeout, usage.retries
    );
    println!(
        "  tokens: in {} / out {}",
        opt_sum(&usage.input_tokens),
        opt_sum(&usage.output_tokens)
    );
    println!(
        "  cost: reported {} / estimated {} (pricing v{})",
        opt_usd(&usage.reported_usd),
        opt_usd(&usage.estimated_usd),
        usage.pricing_version.unwrap_or(0)
    );
    println!("  latency: {}", opt_latency(usage.latency.as_ref()));
    for row in &usage.by_model {
        println!(
            "  model {}:{} [{}] — {} calls, {} requests, success {}",
            row.provider,
            row.model,
            row.origin,
            row.calls,
            row.requests,
            row.success_rate
                .map_or("n/a".to_string(), |r| format!("{:.0}%", r * 100.0))
        );
        println!(
            "    tokens in {} / out {}; cost reported {} / estimated {}; latency {}; errors {} timeouts {} retries {}",
            opt_sum(&row.input_tokens),
            opt_sum(&row.output_tokens),
            opt_usd(&row.reported_usd),
            opt_usd(&row.estimated_usd),
            opt_latency(row.latency.as_ref()),
            row.errors,
            row.timeouts,
            row.retries
        );
    }
    if !tasks.is_empty() {
        println!("  tasks:");
        for task in tasks {
            println!(
                "    {} — {} calls, {} requests, {} (models: {}), tokens in {} / out {}, cost est {}, duration {}",
                task.task_id,
                task.calls,
                task.requests,
                if task.success { "success" } else { "FAILED" },
                task.models.join(", "),
                opt_sum(&task.input_tokens),
                opt_sum(&task.output_tokens),
                opt_usd(&task.estimated_usd),
                task.duration_ms
                    .map_or("n/a".to_string(), |d| format!("{d}ms"))
            );
        }
    }
    if skipped > 0 {
        println!("  note: {skipped} unreadable stored lines skipped");
    }
}

fn opt_sum(sum: &eval_usage::aggregate::OptSum) -> String {
    match sum.value {
        Some(v) => format!("{v:.0} ({} reporting / {} total)", sum.reporting, sum.total),
        None => format!("n/a (0/{} reporting)", sum.total),
    }
}

fn opt_usd(sum: &eval_usage::aggregate::OptSum) -> String {
    match sum.value {
        Some(v) => format!("${v:.4} ({} reporting)", sum.reporting),
        None => "n/a".to_string(),
    }
}

fn opt_latency(latency: Option<&eval_usage::LatencyDist>) -> String {
    match latency {
        Some(l) => format!("p50 {}ms / p95 {}ms (n={})", l.p50_ms, l.p95_ms, l.n),
        None => "n/a".to_string(),
    }
}

async fn cmd_collect(port: u16, store_root: &Path, token_file: Option<&Path>) -> Result<i32> {
    let config = eval_usage::collector::CollectorConfig {
        port,
        store_root: store_root.to_path_buf(),
        token_file: token_file.map(Path::to_path_buf),
    };
    println!("collecting usage events on 127.0.0.1:{port} (POST /v1/events; Ctrl-C to stop)");
    eval_usage::collector::serve(config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_pass_by_value)]
async fn cmd_proxy(
    port: u16,
    bind: &str,
    upstream: &str,
    key_env: &str,
    project: &str,
    store_root: &Path,
    tag: Option<&str>,
    run_id: Option<&str>,
    provider: &str,
    context: &str,
    overrides: &[String],
    max_requests: Option<u32>,
    max_cost_usd: Option<f64>,
    price_in_per_1m: Option<f64>,
    price_out_per_1m: Option<f64>,
    cassette_dir: Option<&Path>,
    out_reserve_tokens: u64,
    upstream_timeout_s: u64,
) -> Result<i32> {
    let mut model_overrides = std::collections::BTreeMap::new();
    for entry in overrides {
        let Some((requested, served)) = entry.split_once('=') else {
            anyhow::bail!("--override must be requested=served, got {entry:?}");
        };
        let requested = requested.trim();
        let served = served.trim();
        if requested.is_empty() || served.is_empty() {
            anyhow::bail!("--override sides must be non-empty, got {entry:?}");
        }
        model_overrides.insert(requested.to_string(), served.to_string());
    }
    let context = parse_context(context)?;
    let config = eval_proxy::ProxyConfig {
        bind_addr: bind.to_string(),
        port,
        upstream: upstream.to_string(),
        key_env: key_env.to_string(),
        project: project.to_string(),
        provider_label: provider.to_string(),
        tag: tag.map(str::to_string),
        run_id: run_id.map(str::to_string),
        context,
        origin: eval_usage::event::EventOrigin::Live,
        model_overrides,
        max_requests,
        max_cost_usd,
        price_in_per_1m,
        price_out_per_1m,
        out_reserve_tokens,
        store_root: store_root.to_path_buf(),
        upstream_timeout: std::time::Duration::from_secs(upstream_timeout_s),
        cassette_dir: cassette_dir.map(Path::to_path_buf),
    };
    if let Err(e) = config.validate() {
        anyhow::bail!("{e}");
    }
    println!(
        "proxy serving {bind}:{port} -> {upstream} (project {project}; events under {})",
        store_root.display()
    );
    eval_proxy::serve(config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(0)
}

fn parse_context(context: &str) -> Result<eval_usage::event::UsageContext> {
    match context.trim() {
        "benchmark" => Ok(eval_usage::event::UsageContext::Benchmark),
        "production" => Ok(eval_usage::event::UsageContext::Production),
        "dev" => Ok(eval_usage::event::UsageContext::Dev),
        other => anyhow::bail!("unknown context {other:?}: benchmark | production | dev"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_cassette(
    dir: &Path,
    port: u16,
    project: &str,
    store_root: &Path,
    tag: Option<&str>,
    run_id: Option<&str>,
    provider: &str,
    context: &str,
) -> Result<i32> {
    let config = eval_proxy::cassette::CassetteServeConfig {
        port,
        dir: dir.to_path_buf(),
        project: project.to_string(),
        provider_label: provider.to_string(),
        tag: tag.map(str::to_string),
        run_id: run_id.map(str::to_string),
        context: parse_context(context)?,
        store_root: store_root.to_path_buf(),
    };
    println!(
        "cassette replay 127.0.0.1:{port} <- {} (project {project}; hits stamp origin=replay)",
        dir.display()
    );
    eval_proxy::cassette::serve(config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(0)
}

// ---------------------------------------------------------------------------
// schema / check
// ---------------------------------------------------------------------------

fn cmd_schema(compact: bool) -> Result<i32> {
    let schema = eval_graders::manifest::platform_schema();
    if compact {
        println!("{}", serde_json::to_string(&schema)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&schema)?);
    }
    Ok(0)
}

/// `eval check`: validate a suite deterministically, optionally probing
/// the target so an agent gets a single pass/fail gate instead of prose.
async fn cmd_check(suite_path: &Path, probe: bool, as_json: bool) -> Result<i32> {
    let mut report = match Suite::load(suite_path) {
        Ok(suite) => eval_graders::validate::check_suite(&suite),
        Err(e) => {
            // Structural load failure: surface it as a single error issue
            // so the output shape is identical for agents.
            let issue = eval_graders::validate::Issue {
                severity: eval_graders::validate::Severity::Error,
                code: "suite.load_failed".to_string(),
                location: suite_path.display().to_string(),
                message: e.to_string(),
                hint: "fix the YAML so the suite parses; run `eval schema` for the accepted keys"
                    .to_string(),
            };
            eval_graders::validate::CheckReport {
                suite: suite_path.display().to_string(),
                cases: 0,
                issues: vec![issue],
            }
        }
    };

    if probe && report.ok() {
        let suite = Suite::load(suite_path).context("loading suite")?;
        match probe_target(&suite, suite_path).await {
            Ok(probe_issues) => report.issues.extend(probe_issues),
            Err(e) => report.issues.push(eval_graders::validate::Issue {
                severity: eval_graders::validate::Severity::Error,
                code: "probe.spawn_failed".to_string(),
                location: "target".to_string(),
                message: e.to_string(),
                hint: "check target.argv; use $PYTHON or an absolute path to the interpreter"
                    .to_string(),
            }),
        }
    }

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_check(&report, probe);
    }
    Ok(if report.ok() { 0 } else { 1 })
}

/// Human rendering of a check report (shared by `check` and `onboard`).
fn print_check(report: &eval_graders::validate::CheckReport, probed: bool) {
    println!(
        "check {} — {} cases, {} error(s), {} warning(s)",
        report.suite,
        report.cases,
        report.errors(),
        report.issues.len() - report.errors()
    );
    for issue in &report.issues {
        let tag = match issue.severity {
            eval_graders::validate::Severity::Error => "ERROR",
            eval_graders::validate::Severity::Warning => "warn ",
        };
        println!(
            "  [{tag}] {} {}: {}",
            issue.code, issue.location, issue.message
        );
        println!("          hint: {}", issue.hint);
    }
    if report.ok() {
        println!(
            "  OK — suite is valid{}",
            if probed {
                " and the target answered eval/v1"
            } else {
                ""
            }
        );
    }
}

/// `eval onboard`: scaffold, verify (check + probe), and set a baseline in
/// one command, stopping at the first failure.
async fn cmd_onboard(name: &str, dir: &Path, template: &str, store_root: &Path) -> Result<i32> {
    cmd_init(name, dir, template, false)?;
    let project = dir.join(name);
    let project_abs = std::fs::canonicalize(&project).unwrap_or_else(|_| project.clone());
    let suite_path = project.join("suite.yaml");
    let mut suite = Suite::load(&suite_path).context("loading scaffolded suite")?;
    // Resolve the target's relative adapter path against the project dir,
    // not the caller's cwd, so onboard works from anywhere.
    if let Some(target) = suite.target.as_mut() {
        if target.cwd.is_none() {
            target.cwd = Some(project_abs.display().to_string());
        }
    }

    let mut report = eval_graders::validate::check_suite(&suite);
    if report.ok() {
        match probe_target(&suite, &suite_path).await {
            Ok(probe_issues) => report.issues.extend(probe_issues),
            Err(e) => report.issues.push(eval_graders::validate::Issue {
                severity: eval_graders::validate::Severity::Error,
                code: "probe.spawn_failed".to_string(),
                location: "target".to_string(),
                message: e.to_string(),
                hint: "check target.argv; use $PYTHON or an absolute path to the interpreter"
                    .to_string(),
            }),
        }
    }
    print_check(&report, true);
    if !report.ok() {
        println!("onboard: stopping at the failed check; fix the issue(s) above and rerun.");
        return Ok(1);
    }

    let target = suite.target.clone().context("suite has no target")?;
    let policy = eval_runner::RunPolicy::default_for(suite.env_allowlist.clone());
    let (record, run_id) = execute_and_store(
        &suite,
        target,
        &policy,
        "baseline",
        store_root,
        &project_abs,
        true,
        None,
        &ManifestOptions {
            lane: Lane::Batch,
            eval_version: None,
            target_version: None,
            model: None,
            free_only: None,
        },
    )
    .await?;
    println!(
        "baseline: {run_id} — {} cases, pass rate {:.2}",
        record.cases.len(),
        record.pass_rate()
    );
    println!(
        "next: edit adapter/target.py, then `eval run suite.yaml --ref work` and `eval compare --suite suite.yaml`"
    );
    Ok(if record.pass() { 0 } else { 1 })
}

/// Spawn the target, run the eval/v1 handshake and one case, and report
/// conformance problems as issues (never graded output).
async fn probe_target(
    suite: &Suite,
    suite_path: &Path,
) -> Result<Vec<eval_graders::validate::Issue>, anyhow::Error> {
    use eval_graders::validate::{Issue, Severity};

    let target = suite.target.clone().context("suite has no target")?;
    let policy = eval_runner::RunPolicy::default_for(suite.env_allowlist.clone());
    let some_case = suite.cases.first().context("suite has no cases")?;
    let probe_suite = Suite {
        suite: format!("{}-probe", suite.suite),
        version: suite.version,
        target: Some(target.clone()),
        env_allowlist: suite.env_allowlist.clone(),
        gate: suite.gate.clone(),
        cases: vec![some_case.clone()],
    };
    let files_root = suite_path.parent().unwrap_or(Path::new("."));
    let run = eval_runner::run_suite(&probe_suite, &target, &policy, files_root).await?;

    let mut issues = Vec::new();
    let trial = run
        .cases
        .first()
        .and_then(|c| c.trials.first())
        .context("probe produced no trial")?;
    match trial.status {
        eval_core::TrialStatus::Ok | eval_core::TrialStatus::CaseError => {}
        eval_core::TrialStatus::Timeout => issues.push(Issue {
            severity: Severity::Error,
            code: "probe.timeout".to_string(),
            location: "target".to_string(),
            message: format!("target did not answer within {}ms", some_case.limits.timeout_ms),
            hint: "make the adapter reply to the handshake immediately and finish each case within limits.timeout_ms".to_string(),
        }),
        eval_core::TrialStatus::Crash => issues.push(Issue {
            severity: Severity::Error,
            code: "probe.crash".to_string(),
            location: "target".to_string(),
            message: format!(
                "target exited {} before a terminal message; stderr: {}",
                trial.exit_code.map_or("?".to_string(), |c| c.to_string()),
                trial.stderr_tail.as_deref().unwrap_or("").chars().take(300).collect::<String>()
            ),
            hint: "the adapter must print {\"protocol\":\"eval/v1\",\"ready\":true} then one result/case_error per case, and keep logs on stderr".to_string(),
        }),
        eval_core::TrialStatus::ProtocolError => issues.push(Issue {
            severity: Severity::Error,
            code: "probe.protocol_error".to_string(),
            location: "target".to_string(),
            message: trial.error_message.clone().unwrap_or_default(),
            hint: "stdout must be eval/v1 JSONL only; see `eval schema` protocol_messages".to_string(),
        }),
    }
    Ok(issues)
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

fn print_human(record: &RunRecord, decision: Option<&GateDecision>) {
    println!(
        "suite {} v{}  ref {}",
        record.suite, record.suite_version, record.ref_name
    );
    println!(
        "run {}  {} cases / {} trials  pass rate {:.2}",
        if record.run_id.is_empty() {
            "(unstored)"
        } else {
            &record.run_id
        },
        record.cases.len(),
        record.trials_total,
        record.pass_rate()
    );
    for case in &record.cases {
        let mark = if case.pass { "ok  " } else { "FAIL" };
        let statuses: Vec<String> = case
            .trials
            .iter()
            .map(|t| t.status.as_str().to_string())
            .collect();
        let walls: Vec<String> = case
            .trials
            .iter()
            .map(|t| format!("{}ms", t.wall_ms))
            .collect();
        println!(
            "  [{mark}] {}  [{}]  wall [{}]",
            case.case_id,
            statuses.join(","),
            walls.join(",")
        );
        for grade in &case.grades {
            let gmark = if grade.pass { "pass" } else { "FAIL" };
            println!("         {gmark:4} {} — {}", grade.grader, grade.evidence);
        }
    }
    if let Some(gate) = decision.or(record.gate.as_ref()) {
        print_decision(&record.suite, "baseline", &record.run_id, gate);
    }
}

fn print_decision(suite: &str, base_id: &str, cand_id: &str, decision: &GateDecision) {
    let verdict = if decision.pass { "PASS" } else { "FAIL" };
    println!("gate [{verdict}] {suite}: baseline {base_id} vs candidate {cand_id}");
    println!("  what changed: {}", decision.what_changed);
    for case in &decision.affected_cases {
        println!("  affected: {case}");
    }
    for line in &decision.evidence {
        println!("  evidence: {line}");
    }
    if let Some(uncertainty) = &decision.uncertainty {
        println!("  uncertainty: {uncertainty}");
    }
}
