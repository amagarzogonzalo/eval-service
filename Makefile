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
NAME        ?=
TEMPLATE    ?= cli
DIR         ?= .
TERRAFORM   ?= terraform
EVAL_RUNNER ?= 0
STORE       ?= $(if $(filter 1,$(EVAL_RUNNER)),/var/lib/eval-runs,eval-runs)
STORE_DIR   ?= $(STORE)
STORES3     ?=
LIVE_SLEEP  ?= 90

.DEFAULT_GOAL := help

.PHONY: help build release test fmt fmt-check clippy verify python-lint schema \
	check baseline work compare onboard init dashboard projects \
	phase3-verify phase3-live phase3-replay store-sync store-restore \
	tf-fmt tf-validate clean

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-18s %s\n", $$1, $$2}'

build: ## Build the debug eval CLI (target/debug/eval)
	$(CARGO) build -p eval-cli

release: ## Build the release eval CLI (target/release/eval)
	$(CARGO) build --release -p eval-cli

test: ## Run the full workspace test suite
	$(CARGO) test --workspace --locked

fmt: ## Format Rust sources
	$(CARGO) fmt --all

fmt-check: ## Check Rust formatting
	$(CARGO) fmt --all -- --check

clippy: ## Lint all targets (warnings are errors)
	$(CARGO) clippy --workspace --all-targets -- -D warnings

verify: fmt-check clippy test ## Pre-commit gate: fmt, clippy, tests
	@echo "verify: OK"

python-lint: ## Lint Python adapters/examples/integrations (ruff)
	$(RUFF) check adapters examples integrations --line-length 100
	$(PYTHON) -m py_compile adapters/python_shim/main.py

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

store-sync: ## Sync the runner store to S3 (STORES3=s3://bucket/prefix)
	@test -n "$(STORES3)" || { echo "usage: make store-sync STORES3=s3://bucket/eval-runs"; exit 2; }
	aws s3 sync $(STORE_DIR) $(STORES3) --only-show-errors

store-restore: ## Restore the runner store from S3 (STORES3=... [STORE_DIR=...])
	@test -n "$(STORES3)" || { echo "usage: make store-restore STORES3=s3://bucket/eval-runs"; exit 2; }
	mkdir -p $(STORE_DIR)
	aws s3 sync $(STORES3) $(STORE_DIR) --only-show-errors

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
