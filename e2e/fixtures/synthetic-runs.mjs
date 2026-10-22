import fs from "node:fs";
import path from "node:path";

const SUITE = "research-agents-service";
const ALT_QUERY = "How should retrieval quality be evaluated in a synthetic RAG demo?";

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function currentRun(store, startedAtMs) {
  const runId = "e2e-query-002";
  const run = path.join(store, "runs", SUITE, "demo", `run-${runId}`);
  const sources = [
    {
      title: "Synthetic E2E fixture source A",
      doi: "10.0000/synthetic.e2e.a",
    },
    {
      title: "Synthetic E2E fixture source B",
      doi: "10.0000/synthetic.e2e.b",
    },
  ];
  const metadata = {
    run_id: runId,
    suite: SUITE,
    ref_name: "demo",
    started_at_ms: startedAtMs,
    trials_total: 1,
    synthetic: true,
    cases: [
      {
        case_id: "e2e-query-fixture",
        pass: true,
        trials: [
          {
            wall_ms: 900,
            output: {
              output: {
                text: "Synthetic E2E fixture answer. Not production evidence.",
                data: {
                  query: ALT_QUERY,
                  status: "pass",
                  nsources: 5,
                  citations: 4,
                  artifact_chars: 420,
                  sources,
                },
              },
            },
          },
        ],
      },
    ],
  };
  const manifest = {
    manifest_version: 1,
    lane: "replay",
    model: { provider: "demo", model: "synthetic-b" },
    target: { version: "synthetic-e2e-fixture" },
    free_only: true,
    synthetic: true,
    metrics: { quality: {}, stability: {} },
  };
  const judge = {
    schema: "eval.judge/v1",
    model: "demo",
    overall: 6,
    scores: {
      grounding: 6,
      relevance: 7,
      completeness: 5,
      clarity: 8,
      no_padding: 6,
      overall: 6,
      rationale: "Synthetic E2E fixture; not production evidence.",
      issues: ["Synthetic fixture, not a real model run."],
    },
  };
  writeJson(path.join(run, "metadata.json"), metadata);
  writeJson(path.join(run, "manifest.json"), manifest);
  writeJson(path.join(run, "judge.json"), judge);
  writeJson(path.join(run, "grades.json"), []);
}

function legacyRun(store, startedAtMs) {
  const runId = "e2e-legacy-001";
  const run = path.join(store, "runs", SUITE, "demo-legacy", `run-${runId}`);
  const metadata = {
    run_id: runId,
    suite: SUITE,
    ref_name: "demo-legacy",
    started_at_ms: startedAtMs,
    trials_total: 1,
    synthetic: true,
    cases: [
      {
        case_id: "e2e-legacy-fixture",
        pass: true,
        trials: [
          {
            output: {
              output: {
                text: "Legacy synthetic E2E fixture answer. Not production evidence.",
                data: {
                  query: "Legacy synthetic fixture for the UI toggle.",
                  status: "pass",
                },
              },
            },
          },
        ],
      },
    ],
  };
  writeJson(path.join(run, "metadata.json"), metadata);
}

export function writeSyntheticFixtures(store, demoStartedAtMs) {
  const base = Number.isFinite(demoStartedAtMs) ? demoStartedAtMs : Date.now();
  currentRun(store, base - 60_000);
  legacyRun(store, base - 120_000);
}
