"""Generate the hosted benchmark page from the canonical run store."""

import argparse
import html
import json
import pathlib
import statistics
from datetime import datetime, timezone

S3_BUCKET = "eval-store-342940030437-eu-north-1"
S3_PREFIX = "eval-runs"
S3_URL = (
    "https://s3.console.aws.amazon.com/s3/buckets/"
    f"{S3_BUCKET}?region=eu-north-1&prefix={S3_PREFIX}/"
)

SUITE_INFO = {
    "digest-agent-e2e": {
        "title": "digest_agent — real E2E",
        "kind": "Real GitHub + real model + real PDF + SMTP test inbox",
        "description": "The real digest pipeline against a real GitHub activity source, the real ReportLab renderer, and a real SMTP protocol exchange.",
        "use_cases": [
            "A real merged PR in the test repo should be summarized by the real model.",
            "A bad repository should fail closed without spending a model call.",
            "The rendered PDF and SMTP message should be inspectable as evidence.",
        ],
    },
    "research-agents-e2e": {
        "title": "research-agents — real E2E",
        "kind": "Real search + real scrape + real model",
        "description": "The real research workflow with live scholarly search, live page fetches, live model calls through the proxy, and the target's renderer.",
        "use_cases": [
            "A real research topic should produce an artifact with citations and sources.",
            "Disallowed hosts and over-budget retrieval should fail closed.",
            "The source count, artifact size and model usage are recorded as evidence.",
        ],
    },
    "digest-agent": {
        "title": "digest_agent",
        "kind": "Real orchestrator / offline fakes",
        "description": (
            "LangGraph digest workflow: empty window, bounded single-PR review, "
            "weak-item critique, and fetch-failure handling."
        ),
        "use_cases": [
            "No activity should produce an empty digest.",
            "A PR should be summarized with changed files and a bounded review loop.",
            "A weak completion should be surfaced without an unbounded retry loop.",
            "A fetch outage should be reported as a handled error.",
        ],
    },
    "digest-live-bench": {
        "title": "digest_agent — live model",
        "kind": "Real model / live proxy",
        "description": "Live digest summarization through the loopback eval proxy.",
        "use_cases": ["Real provider output quality and usage cost for digest summaries."],
    },
    "research-agents-replay": {
        "title": "research-agents — replay",
        "kind": "Real workflow / recorded retrieval",
        "description": (
            "The target's own research workflow with recorded LLM/search/scrape "
            "boundaries; deterministic, no provider keys."
        ),
        "use_cases": [
            "A research question should produce an artifact with sources.",
            "Token and call budgets should stay bounded.",
            "A missing source should fail visibly, not be invented.",
        ],
    },
    "research-agents-live": {
        "title": "research-agents — live",
        "kind": "Real model / recorded retrieval",
        "description": "Live model calls through the proxy with recorded retrieval evidence.",
        "use_cases": ["Real model quality, cost and latency for the research workflow."],
    },
}


def load_json(path: pathlib.Path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def latest_runs(store: pathlib.Path):
    found = []
    runs = store / "runs"
    if not runs.is_dir():
        return found
    for suite_dir in sorted(runs.iterdir()):
        if not suite_dir.is_dir():
            continue
        for ref_dir in sorted(suite_dir.iterdir()):
            if not ref_dir.is_dir():
                continue
            candidates = sorted(
                (path for path in ref_dir.iterdir() if path.is_dir()),
                key=lambda path: path.name,
            )
            if not candidates:
                continue
            loaded = []
            for run_dir in candidates:
                meta = load_json(run_dir / "metadata.json")
                if not meta:
                    continue
                loaded.append((run_dir, meta))
            if not loaded:
                continue
            latest_dir, latest_meta = loaded[-1]
            passing = [
                (run_dir, meta)
                for run_dir, meta in loaded
                if meta.get("cases") and all(case.get("pass") for case in meta["cases"])
            ]
            run_dir, meta = passing[-1] if passing else (latest_dir, latest_meta)
            found.append(
                {
                    "suite": meta.get("suite", suite_dir.name),
                    "ref": meta.get("ref_name", ref_dir.name),
                    "run_id": meta.get("run_id", run_dir.name.removeprefix("run-")),
                    "run_dir": run_dir,
                    "meta": meta,
                    "trace": load_json(run_dir / "trace.json"),
                    "decision": load_json(run_dir / "decision.json"),
                    "latest_attempt_failed": not all(
                        case.get("pass") for case in latest_meta.get("cases", [])
                    ),
                    "latest_attempt_id": latest_meta.get(
                        "run_id", latest_dir.name.removeprefix("run-")
                    ),
                }
            )
    return found


def case_summary(case):
    trials = case.get("trials", [])
    wall = [trial.get("wall_ms", 0) for trial in trials if trial.get("wall_ms")]
    p50 = statistics.median(wall) if wall else 0
    evidence = []
    for grade in case.get("grades", []):
        if grade.get("evidence"):
            evidence.append(grade["evidence"])
    return {
        "id": case.get("case_id", "?"),
        "pass": bool(case.get("pass")),
        "trials": len(trials),
        "p50_ms": round(p50),
        "evidence": evidence[:3],
    }


def render_case(case):
    label = "PASS" if case["pass"] else "FAIL"
    cls = "pass" if case["pass"] else "fail"
    evidence = "".join(f"<li>{html.escape(item)}</li>" for item in case["evidence"])
    return f"""
    <details class="case {cls}">
      <summary><span class="badge {cls}">{label}</span> {html.escape(case['id'])}
        <span class="muted">{case['trials']} trial(s) · p50 {case['p50_ms']} ms</span>
      </summary>
      <ul>{evidence or '<li>No grade evidence</li>'}</ul>
    </details>"""


def render_run(run):
    meta = run["meta"]
    cases = [case_summary(case) for case in meta.get("cases", [])]
    passed = sum(1 for case in cases if case["pass"])
    total = len(cases)
    p50_values = [case["p50_ms"] for case in cases if case["p50_ms"]]
    p50 = round(statistics.median(p50_values)) if p50_values else 0
    trace = run["trace"] or {}
    trace_id = trace.get("trace_id", "")
    s3_path = f"s3://{S3_BUCKET}/{S3_PREFIX}/runs/{meta.get('suite')}/{run['ref']}/{run['run_id']}/"
    good = next((case for case in cases if case["pass"]), None)
    bad = next((case for case in cases if not case["pass"]), None)
    examples = ""
    if good:
        examples += f"<p class=\"good\"><b>Good example:</b> {html.escape(good['id'])}</p>"
    if bad:
        examples += f"<p class=\"bad\"><b>Bad example:</b> {html.escape(bad['id'])}</p>"
    if not bad:
        examples += "<p class=\"muted\">No failing case in this run.</p>"
    latest_note = ""
    if run.get("latest_attempt_failed"):
        attempt = html.escape(str(run.get("latest_attempt_id", "?")))
        latest_note = (
            f"<p class=\"bad\"><b>Latest attempt failed:</b> {attempt}; "
            "showing the last passing run.</p>"
        )
    return f"""
    <article class="run">
      <header>
        <div>
          <h3>{html.escape(meta.get('suite', '?'))} <span class="ref">{html.escape(run['ref'])}</span></h3>
          <p class="muted">run {html.escape(run['run_id'])} · code {html.escape(str(meta.get('code_version', '?')))}</p>
        </div>
        <div class="score">{passed}/{total}<span>cases</span></div>
      </header>
      <div class="metrics">
        <span><b>{p50}</b> ms p50</span>
        <span><b>{meta.get('trials_total', 0)}</b> trials</span>
        <span><b>{'pass' if passed == total else 'fail'}</b> gate</span>
      </div>
      {examples}
      {latest_note}
      <div class="cases">{''.join(render_case(case) for case in cases)}</div>
      <p class="links">
        <a href="{html.escape(S3_URL)}">S3 artifacts</a>
        <span>trace <code>{html.escape(trace_id or 'not exported')}</code></span>
        <span>artifact <code>{html.escape(s3_path)}</code></span>
      </p>
    </article>"""


def render_suite(suite, runs):
    info = SUITE_INFO.get(
        suite,
        {
            "title": suite,
            "kind": "Evaluation suite",
            "description": "Stored evaluation runs.",
            "use_cases": [],
        },
    )
    use_cases = "".join(f"<li>{html.escape(item)}</li>" for item in info["use_cases"])
    return f"""
    <section class="suite">
      <div class="suite-head">
        <div>
          <p class="eyebrow">{html.escape(info['kind'])}</p>
          <h2>{html.escape(info['title'])}</h2>
          <p>{html.escape(info['description'])}</p>
        </div>
      </div>
      <ul class="use-cases">{use_cases}</ul>
      {''.join(render_run(run) for run in runs)}
    </section>"""


def build(store: pathlib.Path, out: pathlib.Path):
    runs = latest_runs(store)
    by_suite = {}
    for run in runs:
        by_suite.setdefault(run["suite"], []).append(run)
    sections = "".join(render_suite(suite, items) for suite, items in sorted(by_suite.items()))
    if not sections:
        sections = '<p class="empty">No stored runs yet.</p>'
    generated = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    page = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>eval-service benchmarks</title>
<style>
:root {{ color-scheme: dark; --bg:#0b1020; --panel:#141b30; --line:#26314f;
  --text:#e8edff; --muted:#93a0c7; --green:#45d483; --red:#ff6b7a; --blue:#7aa2ff; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:15px/1.55 ui-sans-serif,system-ui,-apple-system,Segoe UI,sans-serif;
  background:radial-gradient(circle at top left,#1a2544,var(--bg) 45%); color:var(--text); }}
main {{ max-width:1120px; margin:0 auto; padding:48px 20px 80px; }}
.hero {{ border:1px solid var(--line); background:linear-gradient(135deg,#182344,#10172a);
  border-radius:20px; padding:28px; margin-bottom:28px; }}
.eyebrow {{ text-transform:uppercase; letter-spacing:.14em; font-size:11px; color:var(--blue);
  margin:0 0 8px; font-weight:700; }}
h1 {{ margin:0 0 8px; font-size:34px; letter-spacing:-.02em; }}
h2 {{ margin:0 0 8px; font-size:25px; }}
h3 {{ margin:0; font-size:17px; }}
p {{ margin:0 0 10px; }}
.muted {{ color:var(--muted); }}
.hero p {{ color:var(--muted); max-width:760px; }}
.suite {{ margin:34px 0; }}
.suite-head {{ border-bottom:1px solid var(--line); padding-bottom:12px; margin-bottom:16px; }}
.use-cases {{ color:var(--muted); margin:0 0 18px 20px; }}
.run {{ background:var(--panel); border:1px solid var(--line); border-radius:16px;
  padding:20px; margin:14px 0; box-shadow:0 12px 30px #0003; }}
.run header {{ display:flex; justify-content:space-between; gap:16px; align-items:flex-start; }}
.ref {{ font-size:12px; color:var(--blue); border:1px solid var(--line); border-radius:999px;
  padding:2px 8px; margin-left:6px; vertical-align:middle; }}
.score {{ font-size:30px; font-weight:800; text-align:right; line-height:1; }}
.score span {{ display:block; font-size:11px; color:var(--muted); font-weight:500; margin-top:5px; }}
.metrics {{ display:flex; gap:22px; flex-wrap:wrap; margin:16px 0; color:var(--muted); }}
.metrics b {{ color:var(--text); }}
.good {{ color:var(--green); }} .bad {{ color:var(--red); }}
.case {{ border:1px solid var(--line); border-radius:12px; padding:10px 12px; margin:8px 0; }}
.case summary {{ cursor:pointer; display:flex; gap:10px; align-items:center; flex-wrap:wrap; }}
.badge {{ font-size:10px; font-weight:800; letter-spacing:.08em; border-radius:999px; padding:2px 8px; }}
.badge.pass {{ background:#123d2b; color:var(--green); }}
.badge.fail {{ background:#471a22; color:var(--red); }}
.case ul {{ margin:10px 0 0 12px; color:var(--muted); }}
.links {{ display:flex; gap:16px; flex-wrap:wrap; color:var(--muted); font-size:12px;
  border-top:1px solid var(--line); padding-top:12px; margin-top:14px; }}
a {{ color:var(--blue); text-decoration:none; }} a:hover {{ text-decoration:underline; }}
code {{ font-size:11px; color:#c8d3ff; }}
.empty {{ color:var(--muted); padding:30px; }}
footer {{ color:var(--muted); font-size:12px; margin-top:40px; }}
</style>
</head>
<body>
<main>
  <div class="hero">
    <p class="eyebrow">eval-service · real benchmark evidence</p>
    <h1>Digest &amp; research benchmarks</h1>
    <p>Latest passing run per suite, with real case evidence, pass/fail examples, cost/latency context and links to canonical S3 artifacts. Generated from the run store; missing data is shown as missing.</p>
    <p class="muted">Generated {generated}</p>
  </div>
  {sections}
  <footer>Canonical records live in <code>{html.escape(str(store))}</code> and S3. Traces are linked by ID in the local Phoenix UI.</footer>
</main>
</body>
</html>
"""
    out.mkdir(parents=True, exist_ok=True)
    (out / "index.html").write_text(page, encoding="utf-8")
    print(f"wrote {out / 'index.html'} ({len(runs)} runs)")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", default="/var/lib/eval-runs")
    parser.add_argument("--out", default="/srv/bench-web")
    args = parser.parse_args()
    build(pathlib.Path(args.store), pathlib.Path(args.out))


if __name__ == "__main__":
    main()
