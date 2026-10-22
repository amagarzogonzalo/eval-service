# eval-service: canonical developer and runner entry points.
#
# GNU make (Linux, macOS, WSL). On Windows without make, run the
# equivalent cargo/eval commands directly (see README.md and AGENTS.md).
# The runner exports EVAL_RUNNER=1, which selects /var/lib/eval-runs as
# the store; developer machines keep the repo-local eval-runs/.

SHELL := /bin/sh

CARGO       ?= cargo
RUFF        ?= ruff
PYTHON      ?= python3
EVAL        ?= ./target/debug/eval
SUITE       ?= datasets/cv_demo/suite.yaml
LIVE_SUITE  ?= integrations/digest_agent/live_bench.yaml
OFFLINE_SUITE ?= integrations/digest_agent/suite.yaml
PROJECT     ?= digest-agent
REPLAY_PORT ?= 8788
CASSETTE_DIR ?=
WATCH_CONFIG ?= eval-watch.toml
CALIBRATION ?= datasets/judge_calibration/digest_v1.jsonl
CALIBRATION_OUT ?= judge-calibration.json
CALIBRATION_CANDIDATES ?= judge-candidates.jsonl
NAME        ?=
TEMPLATE    ?= cli
DIR         ?= .
TERRAFORM   ?= terraform
EVAL_RUNNER ?= 0
STORE       ?= $(if $(filter 1,$(EVAL_RUNNER)),/var/lib/eval-runs,eval-runs)
STORE_DIR   ?= $(STORE)
STORES3     ?=
LIVE_SLEEP  ?= 90
WEB_OUT     ?= web/dist
SANDBOX_IMAGE ?= eval-sandbox:24.04

.DEFAULT_GOAL := help

.PHONY: help build release test fmt fmt-check clippy typecheck check doc quality verify python-lint python-test demo-store calibration-export schema \
	check baseline work compare gate onboard init dashboard projects web \
	watch live-digest live-research live-status calibrate phase3-verify phase3-live phase3-replay \
	sandbox-image sandbox-verify store-sync store-restore store-retention restore-drill store-bucket-config \
	tf-fmt tf-validate clean

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-18s %s\n", $$1, $$2}'

build: ## Build the debug eval CLI (target/debug/eval)
	$(CARGO) build -p eval-cli

release: ## Build the release eval CLI (target/release/eval)
	$(CARGO) build --release -p eval-cli

test: ## Run the full workspace test suite (all targets/features)
	$(CARGO) test --workspace --locked --all-targets --all-features

fmt: ## Format Rust sources
	$(CARGO) fmt --all

fmt-check: ## Check Rust formatting
	$(CARGO) fmt --all -- --check

clippy: ## Lint all targets/features (warnings are errors)
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

typecheck: ## Type-check all targets/features
	$(CARGO) check --workspace --all-targets --all-features

doc: ## Build rustdoc with warnings denied
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps --all-features

quality: fmt-check clippy typecheck test doc ## Full Rust quality gate
	@echo "quality: OK"

verify: quality python-test ## Pre-commit gate: Rust quality + Python unit/API tests
	@echo "verify: OK"

python-lint: ## Lint Python adapters/examples/integrations/web/scripts (ruff)
	$(RUFF) check adapters examples integrations web scripts --line-length 100
	$(PYTHON) -m compileall -q adapters examples integrations web scripts

python-test: ## Run Python unit/API tests (offline)
	$(PYTHON) -m unittest discover -s web -p "test_*.py" -v

demo-store: ## Seed a synthetic offline store for the private UI
	$(PYTHON) scripts/seed_demo_store.py --out $(or $(DEMO_STORE),demo-store)

schema: build ## Print the machine-readable platform manifest
	$(EVAL) schema

check: build ## Validate a suite and probe its adapter (SUITE=...)
	$(EVAL) check $(SUITE) --probe

baseline: build ## Record a baseline run (SUITE=...)
	$(EVAL) run $(SUITE) --store $(STORE) --ref baseline --set-baseline

work: build ## Record a candidate run (SUITE=...)
	$(EVAL) run $(SUITE) --store $(STORE) --ref work

compare: build ## Compare baseline vs work (SUITE=...)
	$(EVAL) compare --suite $(SUITE) --store $(STORE)

gate: build ## Compare + enforce provenance policy (SUITE=...)
	$(EVAL) gate --suite $(SUITE) --store $(STORE)

projects: build ## Show portfolio status (eval-workspace.toml)
	$(EVAL) projects --workspace eval-workspace.toml

dashboard: build ## Regenerate the static portfolio dashboard
	$(EVAL) dashboard --workspace eval-workspace.toml --out dashboard

onboard: build ## Scaffold + verify + baseline (NAME=... TEMPLATE=cli|llm|agent)
	@test -n "$(NAME)" || { echo "usage: make onboard NAME=<project> [TEMPLATE=cli|llm|agent] [DIR=.]"; exit 2; }
	$(EVAL) onboard $(NAME) --template $(TEMPLATE) --dir $(DIR) --store $(STORE)

init: build ## Scaffold only (NAME=... TEMPLATE=cli|llm|agent)
	@test -n "$(NAME)" || { echo "usage: make init NAME=<project> [TEMPLATE=cli|llm|agent] [DIR=.]"; exit 2; }
	$(EVAL) init $(NAME) --template $(TEMPLATE) --dir $(DIR)

watch: build ## Run the read-only GitHub watcher (WATCH_CONFIG=...)
	$(EVAL) watch --config $(WATCH_CONFIG)

web: ## Regenerate the hosted benchmark page (WEB_OUT=...)
	$(PYTHON) web/build_benchmarks.py --store $(STORE) --out $(WEB_OUT)

live-digest: ## Trigger the repeatable digest live E2E systemd job (VM)
	sudo systemctl start eval-live-digest.service

live-research: ## Trigger the repeatable research live E2E systemd job (VM)
	sudo systemctl start eval-live-research.service

live-status: ## Show live E2E timers and service state (VM)
	systemctl list-timers 'eval-live-*' --no-pager
	@echo "--- digest ---"
	systemctl status eval-live-digest.service --no-pager -n 3 || true
	@echo "--- research ---"
	systemctl status eval-live-research.service --no-pager -n 3 || true

calibration-export: ## Export current runs as a human-label worksheet
	$(PYTHON) scripts/export_calibration.py --store $(STORE) --out $(CALIBRATION_CANDIDATES) --limit $(or $(CALIBRATION_LIMIT),40)

calibrate: build ## Judge calibration report (CALIBRATION=... CALIBRATION_OUT=...)
	$(EVAL) calibrate $(CALIBRATION) --out $(CALIBRATION_OUT) --markdown $(CALIBRATION_OUT:.json=.md)

phase3-verify: ## Phase 3 offline gate: ARM64 + fmt/clippy/tests + real probe
	@test "$$(uname -m)" = "aarch64" || { echo "phase3-verify requires the aarch64 runner"; exit 1; }
	$(MAKE) verify
	$(MAKE) check SUITE=$(OFFLINE_SUITE)
	@echo "phase3-verify: offline checks OK; live cycle and cassette replay remain manual"

phase3-live: build ## Live proxy cycle: probe -> baseline -> work -> compare (opt-in)
	@test "$$EVAL_DIGEST_LIVE" = "1" || { echo "refusing: set EVAL_DIGEST_LIVE=1 and point EVAL_DIGEST_ENDPOINT at eval proxy"; exit 2; }
	$(MAKE) check SUITE=$(LIVE_SUITE)
	$(EVAL) run $(LIVE_SUITE) --store $(STORE) --ref baseline --set-baseline --workers 1
	sleep $(LIVE_SLEEP)
	$(EVAL) run $(LIVE_SUITE) --store $(STORE) --ref work --workers 1
	$(EVAL) compare --suite $(LIVE_SUITE) --store $(STORE)

phase3-replay: build ## Replay cassettes offline (CASSETTE_DIR=..., live env set)
	@test -n "$(CASSETTE_DIR)" || { echo "usage: make phase3-replay CASSETTE_DIR=<dir> [REPLAY_PORT=8788]"; exit 2; }
	@test "$$EVAL_DIGEST_LIVE" = "1" || { echo "refusing: set the recording's live env first (dummy key is fine)"; exit 2; }
	@set -e; \
		$(EVAL) cassette $(CASSETTE_DIR) --project $(PROJECT) --port $(REPLAY_PORT) --store $(STORE) & \
		pid=$$!; \
		trap 'kill $$pid 2>/dev/null || true' EXIT; \
		sleep 1; \
		EVAL_DIGEST_ENDPOINT=http://127.0.0.1:$(REPLAY_PORT)/v1 \
			$(EVAL) run $(LIVE_SUITE) --store $(STORE) --ref replay --workers 1

sandbox-image: ## Build the Docker sandbox target image
	docker build -f infra/docker/sandbox.Dockerfile -t $(SANDBOX_IMAGE) .

sandbox-verify: sandbox-image ## Unit + adversarial Docker isolation proof
	$(CARGO) test -p eval-runner --test sandbox_tests
	EVAL_DOCKER=1 $(CARGO) test -p eval-runner --test sandbox_tests -- --nocapture

store-sync: ## Sync the runner store to S3 (STORES3=s3://bucket/prefix)
	@test -n "$(STORES3)" || { echo "usage: make store-sync STORES3=s3://bucket/eval-runs"; exit 2; }
	aws s3 sync $(STORE_DIR) $(STORES3) --only-show-errors

store-restore: ## Restore the runner store from S3 (STORES3=... [STORE_DIR=...])
	@test -n "$(STORES3)" || { echo "usage: make store-restore STORES3=s3://bucket/eval-runs"; exit 2; }
	mkdir -p $(STORE_DIR)
	aws s3 sync $(STORES3) $(STORE_DIR) --only-show-errors

store-retention: ## Dry-run local retention (KEEP_LATEST=20 DAYS=365)
	$(PYTHON) scripts/retention.py --store $(STORE) --keep-latest $(or $(KEEP_LATEST),20) --days $(or $(DAYS),365)

restore-drill: ## Restore S3 into a temporary directory and validate it
	STORES3="$(STORES3)" bash scripts/restore_drill.sh

store-bucket-config: ## Enable S3 versioning + lifecycle (EVAL_STORE_BUCKET=...)
	bash scripts/configure_store_bucket.sh "$(EVAL_STORE_BUCKET)"

tf-fmt: ## Check Terraform formatting (infra/aws)
	$(TERRAFORM) fmt -check -recursive infra/aws

tf-validate: ## Validate both Terraform stacks (no credentials)
	$(TERRAFORM) fmt -check -recursive infra/aws
	$(TERRAFORM) -chdir=infra/aws init -backend=false -input=false -lockfile=readonly
	$(TERRAFORM) -chdir=infra/aws validate
	$(TERRAFORM) -chdir=infra/aws/bootstrap init -backend=false -input=false -lockfile=readonly
	$(TERRAFORM) -chdir=infra/aws/bootstrap validate

clean: ## Remove build artifacts
	$(CARGO) clean
