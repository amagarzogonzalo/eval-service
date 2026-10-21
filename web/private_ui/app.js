const state = { runs: [], view: "overview", selected: null, files: {}, checks: [], file: "", tab: "overview" };
const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

function esc(value) {
  return String(value ?? "").replace(/[&<>"']/g, (ch) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"
  }[ch]));
}

function parse(text, fallback = null) {
  if (!text) return fallback;
  try { return JSON.parse(text); } catch { return fallback; }
}

function num(value, digits = 0) {
  return typeof value === "number" && Number.isFinite(value) ? value.toFixed(digits) : "n/a";
}

function pct(value) {
  return typeof value === "number" && Number.isFinite(value) ? `${(value * 100).toFixed(0)}%` : "n/a";
}

function when(ms) {
  if (!ms) return "n/a";
  return new Date(ms).toISOString().replace("T", " ").slice(0, 16) + " UTC";
}

function modelOf(run) {
  const model = run.manifest?.model || run.model;
  if (!model) return "n/a";
  const name = model.model || (run.models || [])[0];
  if (!name) return "n/a";
  return model.provider ? `${model.provider}:${name}` : name;
}

function laneOf(run) {
  return run.manifest?.lane || "n/a";
}

function legacyOf(run) {
  return run.legacy === true || !run.manifest;
}

function contentOf(run) {
  return run.content || {};
}

function judgeScoreOf(run) {
  const judge = run.judge || {};
  const value = judge.overall ?? judge.scores?.overall;
  return typeof value === "number" ? value : null;
}

function judgeCell(run) {
  const score = judgeScoreOf(run);
  if (score === null) return '<span class="muted">n/a</span>';
  const cls = score >= 7 ? "good" : score >= 4 ? "warn" : "bad";
  return `<span class="judge-pill ${cls}">${score}/10</span>`;
}

function shortHash(value) {
  const text = String(value || "n/a");
  return text.length > 12 ? "…" + text.slice(-8) : text;
}

function tick(value) {
  const absolute = Math.abs(value);
  if (absolute >= 1e6) return `${(value / 1e6).toFixed(1)}M`;
  if (absolute >= 1e3) return `${(value / 1e3).toFixed(1)}k`;
  if (absolute >= 10) return value.toFixed(0);
  return value.toFixed(absolute < 1 ? 2 : 1);
}

function freeOf(run) {
  const value = run.manifest?.free_only;
  return value === true ? "free-only" : value === false ? "paid" : "n/a";
}

function passOf(run) {
  return run.pass === true;
}


function latencyOf(run) {
  const m = run.metrics || {};
  const legacy = run.manifest?.metrics?.latency || {};
  return {
    p50: m.p50_ms ?? legacy.p50_ms ?? null,
    p95: m.p95_ms ?? legacy.p95_ms ?? null,
    min: m.min_ms ?? legacy.min_ms ?? null,
    max: m.max_ms ?? legacy.max_ms ?? null,
    n: m.trials ?? legacy.n ?? null,
  };
}

function throughputOf(run) {
  return run.metrics?.trials_per_min ?? null;
}

function graderExplain(name) {
  const text = String(name || "");
  if (text.startsWith("exit_code")) return "The target process must exit successfully.";
  if (text.startsWith("exact[output.data.status]")) return "The pipeline must report the expected status.";
  if (text.startsWith("exact[output.data.outcome]")) return "The target must finish with the expected outcome.";
  if (text.startsWith("exact[output.data.pdf_ok]")) return "The generated artifact must be a valid PDF.";
  if (text.startsWith("exact[output.data.email_sent]")) return "The notifier must deliver the PDF.";
  if (text.startsWith("json_schema")) return "The output must match the required contract shape.";
  if (text.startsWith("not_regex")) return "Forbidden placeholder or error text must not appear.";
  if (text.startsWith("range[output.data.citations]")) return "The artifact must cite enough sources.";
  if (text.startsWith("range[output.data.nsources]")) return "The research run must retrieve enough sources.";
  if (text.startsWith("range[output.data.artifact_chars]")) return "The artifact must contain enough content.";
  if (text.startsWith("range[output.data.pdf_bytes]")) return "The PDF must have a plausible size.";
  if (text.startsWith("range[output.data.service.poll_count]")) return "The job must be polled to completion.";
  if (text.startsWith("range[output.data.service.e2e_ms]")) return "End-to-end service latency must be observed.";
  if (text.startsWith("range")) return "A measured value must fall inside the expected bounds.";
  if (text.startsWith("stability")) return "Repeated trials must produce consistent results.";
  if (text.startsWith("judge")) return "A rubric-based judge evaluates content quality.";
  return "Deterministic check on the run output.";
}

function caseLabel(caseId) {
  return String(caseId || "case").replace(/[-_]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

function queryOf(run) {
  if (run.query) return run.query;
  return groupOf(run) === "digest" ? "GitHub activity digest" : "";
}

function runQuery(run) {
  return queryOf(run) || "n/a";
}
function rateOf(run) {
  if (!run.cases_total) return null;
  return run.cases_passed / run.cases_total;
}

function statusBadge(run) {
  return passOf(run)
    ? '<span class="badge pass">pass</span>'
    : '<span class="badge fail">fail</span>';
}

function currentRuns() {
  return state.runs.filter((run) => !legacyOf(run));
}

function latestRuns() {
  const map = new Map();
  for (const run of currentRuns()) {
    if (!map.has(run.suite)) map.set(run.suite, run);
  }
  return Array.from(map.values());
}

function allSuites() {
  return Array.from(new Set(currentRuns().map((run) => run.suite))).sort();
}

async function api(path) {
  const response = await fetch(path, { credentials: "same-origin" });
  if (response.status === 401) {
    location.href = "/login";
    throw new Error("unauthorized");
  }
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

function notice(message) {
  const node = $("#notice");
  node.textContent = message;
  node.classList.toggle("hidden", !message);
}

function spark(values) {
  const data = values.filter((value) => typeof value === "number");
  if (data.length < 2) return '<p class="muted small">Not enough history for a trend.</p>';
  const width = 320, height = 80, pad = 4;
  const min = Math.min(...data), max = Math.max(...data);
  const span = max - min || 1;
  const points = data.map((value, index) => {
    const x = pad + (index * (width - pad * 2)) / (data.length - 1);
    const y = height - pad - ((value - min) / span) * (height - pad * 2);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
  return `<svg class="spark" viewBox="0 0 ${width} ${height}" role="img" aria-label="Trend">
    <polyline fill="none" stroke="var(--blue)" stroke-width="2" points="${points}"/>
  </svg>`;
}

function renderOverview() {
  const latest = latestRuns();
  const current = currentRuns();
  const totalCases = current.reduce((sum, run) => sum + run.cases_total, 0);
  const passed = current.reduce((sum, run) => sum + run.cases_passed, 0);
  const rate = totalCases ? passed / totalCases : null;
  const suites = allSuites();
  const cards = latest.map((run) => `
    <article class="card">
      <header>
        <div>
          <p class="eyebrow">${esc(laneOf(run))}</p>
          <h3>${esc(run.suite)}</h3>
          <p class="muted small">${esc(run.ref)} · ${when(run.started_at_ms)}</p>
        </div>
        ${statusBadge(run)}
      </header>
      <div class="chips">
        <span class="chip">${esc(modelOf(run))}</span>
        <span class="chip">${esc(freeOf(run))}</span>
        <span class="chip">${run.trials_total} trials</span>
        <span class="chip">judge ${judgeScoreOf(run) ?? "n/a"}</span>
        <span class="chip">${contentOf(run).nsources ?? "n/a"} sources</span>
      </div>
      <div class="metricline">
        <span><b>${run.cases_passed}/${run.cases_total}</b> cases</span>
        <span><b>${pct(rateOf(run))}</b> pass rate</span>
        <span><b>${num(latencyOf(run).p50, 0)}</b> ms p50</span>
        <span><b>${num(throughputOf(run), 2)}</b> trials/min</span>
        <span title="${esc(run.trace?.trace_id || "n/a")}"><b>${esc(shortHash(run.trace?.trace_id))}</b> trace</span>
      </div>
      <button data-open-run="${esc([run.suite, run.ref, run.run_id].join("|"))}">Open run</button>
    </article>`).join("");
  $("#view-overview").innerHTML = `
    <div class="hero">
      <p class="eyebrow">private evaluation evidence</p>
      <h1>Benchmarks, metrics and full evidence</h1>
      <p class="muted">Every visible number is traceable to a run, a manifest, a trace or an explicit n/a.</p>
      <div class="kpis">
        <div class="kpi"><b>${current.length}</b><span>current runs</span></div>
        <div class="kpi"><b>${state.runs.length}</b><span>stored runs</span></div>
        <div class="kpi"><b>${suites.length}</b><span>suites</span></div>
        <div class="kpi"><b>${pct(rate)}</b><span>case pass rate</span></div>
        <div class="kpi"><b>${totalCases}</b><span>cases observed</span></div>
      </div>
    </div>
    <div class="grid">
      <div>
        <h2>Latest per suite</h2>
        <div class="cards">${cards || '<div class="empty">No runs yet.</div>'}</div>
      </div>
      <aside class="card">
        <h2>Health</h2>
        <p class="muted">Private service, read-only API, session cookie auth.</p>
        <div class="metricline"><span><b>${latest.filter(passOf).length}/${latest.length}</b> suites green</span></div>
        <h3>Recent rate</h3>
        ${spark(current.slice(0, 20).reverse().map(rateOf))}
        <h3>Coverage</h3>
        <p class="muted small">${suites.map(esc).join(" · ") || "n/a"}</p>
        <p class="muted small">${state.runs.length - current.length} legacy runs excluded from charts</p>
      </aside>
    </div>`;
  $$("[data-open-run]").forEach((button) => {
    button.addEventListener("click", () => openRun(button.dataset.openRun.split("|")));
  });
}

function filteredRuns() {
  const query = ($("#run-search")?.value || "").toLowerCase();
  const suite = $("#run-suite")?.value || "";
  const status = $("#run-status")?.value || "";
  return state.runs.filter((run) => {
    const haystack = [run.suite, run.ref, run.run_id, modelOf(run), laneOf(run)].join(" ").toLowerCase();
    if (query && !haystack.includes(query)) return false;
    if (suite && run.suite !== suite) return false;
    if (status === "pass" && !passOf(run)) return false;
    if (status === "fail" && passOf(run)) return false;
    return true;
  });
}

function renderRuns() {
  const suiteOptions = allSuites().map((suite) => `<option value="${esc(suite)}">${esc(suite)}</option>`).join("");
  $("#view-runs").innerHTML = `
    <div class="hero">
      <p class="eyebrow">run explorer</p>
      <h1>Every run</h1>
      <p class="muted">Newest first. Select a run to inspect cases, grades, logs and transcripts.</p>
    </div>
    <div class="toolbar">
      <input id="run-search" placeholder="Search suite, ref, model, run id">
      <select id="run-suite"><option value="">All suites</option>${suiteOptions}</select>
      <select id="run-status"><option value="">All statuses</option><option value="pass">Pass</option><option value="fail">Fail</option></select>
      <span id="run-count" class="muted small"></span>
    </div>
    <table>
      <thead><tr><th>Status</th><th>Suite</th><th>Ref</th><th>Lane</th><th>Model</th><th>Free</th><th>Cases</th><th>p50 / p95</th><th>Trials/min</th><th>Judge</th><th>Sources</th><th>Started</th><th>Run</th></tr></thead>
      <tbody id="run-rows"></tbody>
    </table>`;
  const update = () => {
    const rows = filteredRuns();
    $("#run-count").textContent = `${rows.length} of ${state.runs.length} runs`;
    $("#run-rows").innerHTML = rows.map((run) => `
      <tr class="click" data-open-run="${esc([run.suite, run.ref, run.run_id].join("|"))}">
        <td>${statusBadge(run)}</td>
        <td>${esc(run.suite)} ${legacyOf(run) ? '<span class="badge legacy">legacy</span>' : ""}</td>
        <td>${esc(run.ref)}</td>
        <td>${esc(laneOf(run))}</td>
        <td>${esc(modelOf(run))}</td>
        <td>${esc(freeOf(run))}</td>
        <td>${run.cases_passed}/${run.cases_total}</td>
        <td>${num(latencyOf(run).p50, 0)} / ${num(latencyOf(run).p95, 0)} ms</td>
        <td>${num(throughputOf(run), 2)}</td>
        <td>${judgeCell(run)}</td>
        <td>${esc(contentOf(run).nsources ?? "n/a")}</td>
        <td>${when(run.started_at_ms)}</td>
        <td><code>${esc(run.run_id)}</code></td>
      </tr>`).join("") || '<tr><td colspan="13" class="empty">No matching runs.</td></tr>';
    $$("[data-open-run]").forEach((row) => {
      row.addEventListener("click", () => openRun(row.dataset.openRun.split("|")));
    });
  };
  $("#run-search").addEventListener("input", update);
  $("#run-suite").addEventListener("change", update);
  $("#run-status").addEventListener("change", update);
  update();
}

function groupOf(run) {
  const suite = String(run.suite || "");
  if (suite.startsWith("digest")) return "digest";
  if (suite.startsWith("research")) return "research";
  return "other";
}


function visibleBenchRuns() {
  const group = $("#bench-group")?.value || "";
  const suite = $("#bench-suite")?.value || "";
  const model = $("#bench-model")?.value || "";
  const query = $("#bench-query")?.value || "";
  const includeLegacy = $("#bench-legacy")?.checked === true;
  return state.runs.filter((run) => (!group || groupOf(run) === group)
    && (!suite || run.suite === suite)
    && (!model || modelOf(run) === model)
    && (!query || queryOf(run) === query)
    && (includeLegacy || !legacyOf(run)));
}

function scatterSvg(title, runs, xValue, yValue, xLabel, yLabel, xFmt = tick, yFmt = tick, yFloor = null) {
  const points = runs.map((run) => ({ run, x: xValue(run), y: yValue(run) }))
    .filter((point) => Number.isFinite(point.x) && Number.isFinite(point.y));
  if (!points.length) return `<div class="chart"><h3>${esc(title)}</h3><div class="empty">No numeric runs for this view.</div></div>`;
  const width = 640, height = 250, pad = 48;
  const xMin = Math.min(...points.map((point) => point.x));
  const xMax = Math.max(...points.map((point) => point.x));
  const yMin = Math.min(...points.map((point) => point.y));
  const yMax = Math.max(...points.map((point) => point.y));
  const xSpan = xMax - xMin || 1;
  const ySpan = yMax - yMin || 1;
  const color = (run) => groupOf(run) === "digest" ? "#45d483" : "#7aa2ff";
  const dots = points.map((point) => {
    const cx = pad + ((point.x - xMin) / xSpan) * (width - pad * 2);
    const cy = height - pad - ((point.y - yMin) / ySpan) * (height - pad * 2);
    return `<circle class="dot" cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="4" fill="${color(point.run)}" opacity=".85"><title>${esc(point.run.run_id)} · ${esc(modelOf(point.run))} · ${esc(xFmt(point.x))} ${xLabel} · ${esc(yFmt(point.y))} ${yLabel}</title></circle>`;
  }).join("");
  const xTicks = [0, 1, 2, 3].map((i) => {
    const ratio = i / 3;
    const x = pad + ratio * (width - pad * 2);
    return `<line class="grid" x1="${x.toFixed(1)}" y1="${pad}" x2="${x.toFixed(1)}" y2="${height - pad}"/><text class="tick" x="${x.toFixed(1)}" y="${height - pad + 15}" text-anchor="middle">${esc(xFmt(xMin + ratio * xSpan))}</text>`;
  }).join("");
  const yTicks = [0, 1, 2, 3].map((i) => {
    const ratio = i / 3;
    const y = height - pad - ratio * (height - pad * 2);
    return `<line class="grid" x1="${pad}" y1="${y.toFixed(1)}" x2="${width - pad}" y2="${y.toFixed(1)}"/><text class="tick" x="${pad - 8}" y="${(y + 3).toFixed(1)}" text-anchor="end">${esc(yFmt(yMin + ratio * ySpan))}</text>`;
  }).join("");
  return `<div class="chart"><div class="chart-head"><h3>${esc(title)}</h3><span class="muted small">${points.length} runs</span></div><div class="legend"><span><i class="dot digest"></i>digest</span><span><i class="dot research"></i>research</span></div><svg viewBox="0 0 ${width} ${height}" role="img" aria-label="${esc(title)}">
    ${xTicks}${yTicks}
    <line class="axis" x1="${pad}" y1="${height - pad}" x2="${width - pad}" y2="${height - pad}"/>
    <line class="axis" x1="${pad}" y1="${pad}" x2="${pad}" y2="${height - pad}"/>
    <text class="tick" x="${width / 2}" y="${height - 4}" text-anchor="middle">${esc(xLabel)}</text>
    <text class="tick" x="4" y="${pad - 12}">${esc(yLabel)}</text>
    ${dots}
  </svg></div>`;
}

function renderBenchCharts() {
  const runs = visibleBenchRuns();
  $("#bench-charts").innerHTML = [
    scatterSvg("Pass rate vs p50", runs, (run) => latencyOf(run).p50, (run) => rateOf(run), "p50 ms", "pass rate", tick, pct, 0),
    scatterSvg("Throughput vs p50", runs, (run) => latencyOf(run).p50, (run) => throughputOf(run), "p50 ms", "trials/min"),
    scatterSvg("Sources vs citations", runs, (run) => contentOf(run).nsources, (run) => contentOf(run).citations, "sources", "citations", tick, tick, 0),
    scatterSvg("Judge vs pass rate", runs, (run) => judgeScoreOf(run), (run) => rateOf(run), "judge", "pass rate", tick, pct, 0),
  ].join("");
  const groups = new Map();
  for (const run of runs) {
    const key = [groupOf(run), modelOf(run)].join("|");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(run);
  }
  const rows = Array.from(groups.entries()).map(([key, items]) => {
    const [group, model] = key.split("|");
    const passed = items.reduce((sum, run) => sum + run.cases_passed, 0);
    const total = items.reduce((sum, run) => sum + run.cases_total, 0);
    const p50 = items.map((run) => latencyOf(run).p50).filter(Number.isFinite);
    const throughput = items.map(throughputOf).filter(Number.isFinite);
    const latest = [...items].sort((a, b) => (a.started_at_ms || 0) - (b.started_at_ms || 0)).pop();
    const sources = items.map((run) => contentOf(run).nsources).filter(Number.isFinite);
    return `<tr><td>${esc(group)}</td><td>${esc(model)}</td><td>${items.length}</td><td>${total ? pct(passed / total) : "n/a"}</td><td>${p50.length ? num(statMedian(p50), 0) : "n/a"} ms</td><td>${throughput.length ? num(statMedian(throughput), 2) : "n/a"}</td><td>${judgeCell(latest)}</td><td>${sources.length ? num(statMedian(sources), 0) : "n/a"}</td></tr>`;
  }).join("");
  $("#bench-model-summary").innerHTML = `<div class="card model-table"><h2>Disaggregated by target + LLM</h2><table><thead><tr><th>Target</th><th>LLM / target model</th><th>Runs</th><th>Pass</th><th>p50</th><th>Trials/min</th><th>Judge (latest)</th><th>Sources</th></tr></thead><tbody>${rows || '<tr><td colspan="8" class="empty">No runs for these filters.</td></tr>'}</tbody></table></div>`;
}

function statMedian(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}
function renderBench() {
  const suites = allSuites().map((suite) => `<option value="${esc(suite)}">${esc(suite)}</option>`).join("");
  const queries = [...new Set(currentRuns().map(queryOf).filter(Boolean))].sort();
  const rows = state.runs.map((run) => `
    <tr data-suite="${esc(run.suite)}" data-group="${groupOf(run)}" data-model="${esc(modelOf(run))}" data-query="${esc(queryOf(run))}" data-legacy="${legacyOf(run)}">
      <td><input type="checkbox" data-compare-id="${esc(run.run_id)}" aria-label="select ${esc(run.run_id)}"></td>
      <td>${statusBadge(run)}</td>
      <td>${esc(run.suite)}<br><span class="muted small">${esc(laneOf(run))}</span>${legacyOf(run) ? ' <span class="badge legacy">legacy</span>' : ""}<br><span class="query-label">${esc(queryOf(run) || "no query")}</span></td>
      <td>${esc(modelOf(run))}</td>
      <td>${pct(rateOf(run))}</td>
      <td>${num(latencyOf(run).p50, 0)} / ${num(latencyOf(run).p95, 0)} ms</td>
      <td>${num(throughputOf(run), 2)}</td>
      <td>${judgeCell(run)}</td>
      <td>${esc(contentOf(run).nsources ?? "n/a")}</td>
      <td><button data-open-run="${esc([run.suite, run.ref, run.run_id].join("|"))}">Open</button></td>
    </tr>`).join("");
  $("#view-bench").innerHTML = `
    <div class="hero">
      <p class="eyebrow">cross-run comparison</p>
      <h1>Benchmark matrix</h1>
      <p class="muted">Every run, grouped by target repository. Select two rows to compare quality, latency and throughput.</p>
      <div class="metricline">
        <span><b>${state.runs.filter((run) => groupOf(run) === "digest").length}</b> digest runs</span>
        <span><b>${state.runs.filter((run) => groupOf(run) === "research").length}</b> research runs</span>
        <span><b>${state.runs.filter((run) => run.judge).length}</b> judged runs</span>
        <span><b>${state.runs.filter((run) => !legacyOf(run)).length}</b> current manifested runs</span>
      </div>
    </div>
    <div class="toolbar">
      <select id="bench-group"><option value="">All repos</option><option value="digest">digest_agent</option><option value="research">research_agents</option></select>
      <select id="bench-suite"><option value="">All suites</option>${suites}</select>
      <select id="bench-query"><option value="">All queries</option>${queries.map((query) => `<option value="${esc(query)}">${esc(query.length > 72 ? query.slice(0, 72) + "…" : query)}</option>`).join("")}</select>
      <select id="bench-model"><option value="">All LLMs / targets</option>${[...new Set(state.runs.map(modelOf))].sort().map((model) => `<option value="${esc(model)}">${esc(model)}</option>`).join("")}</select>
      <label class="toggle"><input type="checkbox" id="bench-legacy"> Include legacy runs</label>
      <button id="bench-compare" disabled>Compare selected</button>
      <span class="muted small">Select exactly two runs</span>
    </div>
    <div id="bench-compare-panel"></div>
    <div id="bench-charts" class="charts"></div>
    <div id="bench-model-summary"></div>
    <div class="card">
      <table>
<thead><tr><th></th><th>Status</th><th>Suite / lane / query</th><th>LLM / target</th><th>Rate</th><th>p50 / p95</th><th>Trials/min</th><th>Judge</th><th>Sources</th><th></th></tr></thead>
        <tbody>${rows || '<tr><td colspan="10" class="empty">No benchmark runs yet.</td></tr>'}</tbody>
      </table>
    </div>`;
  const apply = () => {
    const group = $("#bench-group").value;
    const suite = $("#bench-suite").value;
    const model = $("#bench-model").value;
    const query = $("#bench-query").value;
    const includeLegacy = $("#bench-legacy").checked;
    $$("#view-bench tbody tr").forEach((row) => {
      const ok = (!group || row.dataset.group === group)
        && (!suite || row.dataset.suite === suite)
        && (!model || row.dataset.model === model)
        && (!query || row.dataset.query === query)
        && (includeLegacy || row.dataset.legacy !== "true");
      row.style.display = ok ? "" : "none";
    });
  };
  $("#bench-group").addEventListener("change", () => { apply(); renderBenchCharts(); });
  $("#bench-suite").addEventListener("change", () => { apply(); renderBenchCharts(); });
  $("#bench-model").addEventListener("change", () => { apply(); renderBenchCharts(); });
  $("#bench-query").addEventListener("change", () => { apply(); renderBenchCharts(); });
  $("#bench-legacy").addEventListener("change", () => { apply(); renderBenchCharts(); });
  apply();
  renderBenchCharts();
  $$("[data-compare-id]").forEach((box) => box.addEventListener("change", () => {
    const selected = $$("[data-compare-id]:checked");
    if (selected.length > 2) box.checked = false;
    $("#bench-compare").disabled = $$("[data-compare-id]:checked").length !== 2;
  }));
  $("#bench-compare").addEventListener("click", () => {
    const ids = $$("[data-compare-id]:checked").map((box) => box.dataset.compareId);
    renderCompare(ids);
  });
  $$("[data-open-run]").forEach((button) => button.addEventListener("click", () => openRun(button.dataset.openRun.split("|"))));
}

function cleanSnippet(value) {
  const raw = String(value || "").replace(/<[^>]*>/g, " ");
  const node = document.createElement("textarea");
  node.innerHTML = raw;
  return node.value.replace(/\s+/g, " ").trim();
}

function renderMarkdown(text) {
  const body = String(text || "")
    .replace(/^---\s*\n[\s\S]*?\n---\s*\n/, "")
    .replace(/\\u200[bBcC]/g, "")
    .replace(/[\u200b\u200c]/g, "")
    .replace(/\s*\[eva[^\n\]]*\]?/g, "");
  const inline = (line) => esc(line)
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  const html = [];
  let inList = false;
  const closeList = () => { if (inList) { html.push("</ul>"); inList = false; } };
  for (const line of body.split("\n")) {
    const heading = line.match(/^(#{1,4})\s+(.+)$/);
    const bullet = line.match(/^\s*[-*]\s+(.+)$/);
    if (bullet) {
      if (!inList) { html.push("<ul>"); inList = true; }
      html.push(`<li>${inline(bullet[1])}</li>`);
    } else {
      closeList();
      if (heading) html.push(`<h${Math.min(4, heading[1].length + 1)}>${inline(heading[2])}</h${Math.min(4, heading[1].length + 1)}>`);
      else if (line.trim()) html.push(`<p>${inline(line)}</p>`);
    }
  }
  closeList();
  return html.join("") || '<p class="muted">No rendered answer stored.</p>';
}

function renderJudge(judge) {
  const scores = judge.scores || judge;
  const dims = ["grounding", "relevance", "completeness", "clarity", "no_padding"];
  const bars = dims.map((key) => {
    const value = Number(scores[key] ?? 0);
    return `<div class="score-row"><span>${esc(key.replace("_", " "))}</span><div class="bar"><i style="width:${Math.max(0, Math.min(100, value * 10))}%"></i></div><b>${value}/10</b></div>`;
  }).join("");
  const issues = (scores.issues || []).map((item) => `<li>${esc(item)}</li>`).join("");
  return `<div class="card"><h2>Content judge</h2><p class="muted">Free model: ${esc(judge.model || "n/a")} · rubric ${esc(judge.rubric || "n/a")}</p>
    <div class="judge-grid"><div class="judge-score">${esc(judge.overall ?? scores.overall ?? "n/a")}<span>/10 overall</span></div><div class="judge-bars">${bars}</div></div>
    <h3>Rationale</h3><p>${esc(scores.rationale || "No rationale recorded.")}</p>
    ${issues ? `<h3>Issues</h3><ul>${issues}</ul>` : ""}
    <details><summary>Raw judge JSON</summary><pre>${esc(JSON.stringify(judge, null, 2))}</pre></details></div>`;
}

function renderCompare(ids) {
  const runs = ids.map((id) => state.runs.find((run) => run.run_id === id)).filter(Boolean);
  if (runs.length !== 2) return;
  const [a, b] = runs;
  const metric = (label, value) => `<tr><th>${esc(label)}</th><td>${value(a)}</td><td>${value(b)}</td></tr>`;
  $("#bench-compare-panel").innerHTML = `
    <div class="card">
      <h2>Selected comparison</h2>
      <table>
        <thead><tr><th>Metric</th><th><code>${esc(a.run_id)}</code></th><th><code>${esc(b.run_id)}</code></th></tr></thead>
        <tbody>
          ${metric("Suite", (run) => esc(run.suite))}
          ${metric("Lane", (run) => esc(laneOf(run)))}
          ${metric("Model", (run) => esc(modelOf(run)))}
          ${metric("Pass", (run) => statusBadge(run))}
          ${metric("Pass rate", (run) => pct(rateOf(run)))}
          ${metric("Cases", (run) => `${run.cases_passed}/${run.cases_total}`)}
          ${metric("p50", (run) => `${num(latencyOf(run).p50, 0)} ms`)}
          ${metric("p95", (run) => `${num(latencyOf(run).p95, 0)} ms`)}
          ${metric("Trials/min", (run) => num(throughputOf(run), 2))}
          ${metric("Judge", (run) => run.judge ? esc(run.judge.overall ?? run.judge.score ?? "recorded") : "n/a")}
          ${metric("Query", (run) => esc(runQuery(run)))}
        </tbody>
      </table>
    </div>`;
}

function renderMethod() {
  $("#view-method").innerHTML = `
    <div class="hero">
      <p class="eyebrow">methodology</p>
      <h1>What the numbers mean</h1>
      <p class="muted">Deterministic checks gate. LLM judges are advisory. Missing values are n/a, never zero.</p>
    </div>
    <div class="grid">
      <div class="card">
        <h2>Metrics</h2>
        <p>Pass rate is passed cases / total cases. Latency is wall time from the run store. Token and cost fields come only from provider usage events; if the target bypasses the proxy they are n/a.</p>
        <h2>Lanes</h2>
        <p><b>batch</b> is a real entrypoint run. <b>local-service</b> is the target HTTP service on this VM. <b>remote-service</b> is the deployed service. <b>replay</b> uses recorded boundaries and is never live evidence.</p>
        <h2>Privacy</h2>
        <p>The UI is private. API keys and tokens are never exported. Full prompts and logs are visible only after the signed API session and private tunnel authentication.</p>
      </div>
      <aside class="card">
        <h2>Evidence chain</h2>
        <p class="muted">run → manifest → grades → trace → S3</p>
        <p>Each run has a stable ID, a manifest with lane/model/free-only, grades, trace IDs and artifacts. Use the run detail view to inspect every file we have.</p>
      </aside>
    </div>`;
}

async function openRun(parts) {
  const [suite, ref, runId] = parts;
  notice("");
  try {
    const data = await api(`/api/run?suite=${encodeURIComponent(suite)}&ref=${encodeURIComponent(ref)}&run_id=${encodeURIComponent(runId)}`);
    state.selected = data.run;
    state.files = data.files || {};
    state.checks = data.checks || [];
    state.file = Object.keys(state.files)[0] || "";
    state.tab = "overview";
    showView("run");
    renderRun();
  } catch (error) {
    notice(`Could not load run: ${error.message}`);
  }
}

function fileJson(name) {
  return parse(state.files[name], null);
}

function renderRun() {
  const run = state.selected;
  if (!run) return;
  const meta = fileJson("metadata.json") || {};
  const manifest = fileJson("manifest.json") || run.manifest || {};
  const cases = meta.cases || [];
  const traceId = run.trace?.trace_id || manifest.evidence?.trace_id || "n/a";
  const scores = cases.map((item) => item.pass ? "1" : "0").join("");
  $("#view-run").innerHTML = `
    <button id="back-runs">Back to runs</button>
    <div class="hero" style="margin-top:14px">
      <div class="run-head">
        <div>
          <p class="eyebrow">${esc(manifest.lane || "run")}</p>
          <h1>${esc(run.suite)} <span class="muted">${esc(run.ref)}</span></h1>
          <p class="muted"><code>${esc(run.run_id)}</code> · ${when(run.started_at_ms)} · ${esc(modelOf(run))} · ${esc(freeOf(run))} ${legacyOf(run) ? '· legacy run' : ""}</p>
          <p><b>Query/input:</b> ${esc(runQuery(run))}</p>
          <div class="chips">
            <span class="chip">code ${esc(manifest.eval_version || run.code_version || "n/a")}</span>
            <span class="chip">target ${esc(manifest.target?.version || "n/a")}</span>
            <span class="chip">trace ${esc(traceId)}</span>
          </div>
        </div>
        <div class="score">${run.cases_passed}/${run.cases_total}<span>cases ${passOf(run) ? "pass" : "fail"}</span></div>
      </div>
      <div class="metricline">
        <span><b>${run.trials_total}</b> trials</span>
        <span><b>${num(latencyOf(run).p50, 0)}</b> ms p50</span>
        <span><b>${num(latencyOf(run).p95, 0)}</b> ms p95</span>
        <span><b>${num(throughputOf(run), 2)}</b> trials/min</span>
        <span><b>${esc(contentOf(run).nsources ?? "n/a")}</b> sources</span>
        <span><b>${esc(contentOf(run).citations ?? "n/a")}</b> citations</span>
        <span><b>${judgeScoreOf(run) ?? "n/a"}</b> judge</span>
        ${manifest.metrics?.quality?.mean_score != null ? `<span><b>${esc(manifest.metrics.quality.mean_score)}</b> quality</span>` : ""}
        ${manifest.metrics?.stability?.total != null ? `<span><b>${esc(manifest.metrics.stability.passed)}/${esc(manifest.metrics.stability.total)}</b> stable</span>` : ""}
      </div>
    </div>
    <div class="tabs">
      <button data-tab="overview" class="active">Cases</button>
      <button data-tab="content">Query / Sources / Answer</button>
      <button data-tab="checks">Checks</button>
      <button data-tab="transcript">Transcripts</button>
      <button data-tab="result">Result</button>
      <button data-tab="judge">Judge</button>
      <button data-tab="logs">Logs</button>
      <button data-tab="files">Files</button>
    </div>
    <div id="run-panel"></div>`;
  $("#back-runs").addEventListener("click", () => showView("runs"));
  $$("[data-tab]").forEach((button) => button.addEventListener("click", () => {
    state.tab = button.dataset.tab;
    $$("[data-tab]").forEach((node) => node.classList.toggle("active", node === button));
    renderRunPanel();
  }));
  renderRunPanel();
}

function renderRunPanel() {
  const run = state.selected;
  const meta = fileJson("metadata.json") || {};
  const cases = meta.cases || [];
  const panel = $("#run-panel");
  if (!panel) return;
  if (state.tab === "content") {
    const contentCase = cases.find((item) => !String(item.case_id || "").toLowerCase().includes("health"));
    const data = ((contentCase?.trials?.[0]?.output || {}).output || {}).data || {};
    const sources = Array.isArray(data.sources) ? data.sources : [];
    const answer = state.files["artifact.md"] || ((contentCase?.trials?.[0]?.output || {}).output || {}).text || "";
    const sourceCards = sources.map((source, index) => {
      const title = source.title || source.id || `Source ${index + 1}`;
      const doi = String(source.doi || "").replace(/^https?:\/\/doi\.org\//, "");
      const href = source.url || (doi ? `https://doi.org/${doi}` : "");
      const meta = [source.provider, source.year, doi].filter(Boolean).join(" · ");
      const excerpt = cleanSnippet(source.abstract).slice(0, 180);
      return `<article class="source-card" data-source="${esc(`${title} ${meta}`.toLowerCase())}">
        <div class="source-num">${index + 1}</div>
        <div><h4>${href ? `<a href="${esc(href)}" target="_blank" rel="noopener">${esc(title)}</a>` : esc(title)}</h4>
        <p class="muted small">${esc(meta || "No provider metadata")}</p>
        ${excerpt ? `<p class="source-excerpt">${esc(excerpt)}</p>` : ""}</div>
      </article>`;
    }).join("");
    panel.innerHTML = `
      <div class="card"><p class="eyebrow">query</p><h2>${esc(runQuery(run) || "No query recorded")}</h2>
        <p class="muted">${sources.length || data.nsources || 0} sources returned · ${esc(modelOf(run))} · ${esc(laneOf(run))}</p></div>
      <div class="card" style="margin-top:14px"><div class="card-head"><h2>Sources</h2><input id="source-filter" placeholder="Filter sources" aria-label="Filter sources"></div>
        ${sourceCards ? `<div class="source-grid">${sourceCards}</div>` : '<p class="empty">No source records stored for this run.</p>'}
      </div>
      <div class="card" style="margin-top:14px"><h2>Answer</h2><div class="markdown">${renderMarkdown(answer || "No rendered answer stored.")}</div></div>`;
    const filter = $("#source-filter");
    if (filter) filter.addEventListener("input", () => {
      const needle = filter.value.toLowerCase();
      $$(".source-card").forEach((node) => node.classList.toggle("hidden", !node.dataset.source.includes(needle)));
    });
    return;
  }
  if (state.tab === "checks") {
    panel.innerHTML = state.checks.length
      ? `<div class="card"><h2>Saved checks</h2><p class="muted">Human-readable names for every persisted grader result.</p><table><thead><tr><th>Case</th><th>Check</th><th>Result</th><th>Evidence</th></tr></thead><tbody>${state.checks.map((check) => `<tr><td>${esc(check.case_label)}</td><td><b>${esc(check.label)}</b><br><span class="muted small">${esc(check.description)}</span></td><td>${check.pass ? "PASS" : "FAIL"}</td><td class="small">${esc(check.evidence)}</td></tr>`).join("")}</tbody></table></div>`
      : '<div class="empty">No saved checks for this run.</div>';
    return;
  }
  if (state.tab === "transcript") {
    const transcript = state.files["transcript.jsonl"] || state.files["observations.jsonl"] || "";
    panel.innerHTML = renderTranscript(transcript);
    return;
  }
  if (state.tab === "result") {
    const artifact = state.files["artifact.md"] || "";
    const texts = cases.flatMap((item) => (item.trials || []).map((trial) => {
      const output = (trial.output || {}).output || {};
      return output.text || "";
    })).filter(Boolean);
    const body = artifact || texts.join("\n\n") || "No rendered result was stored for this run.";
    panel.innerHTML = `<div class="card"><h2>Result</h2><p class="muted">Artifact or target output captured for this run.</p><pre>${esc(body)}</pre></div>`;
    return;
  }
  if (state.tab === "judge") {
    const judge = fileJson("judge.json") || run.judge;
    panel.innerHTML = judge ? renderJudge(judge) : '<div class="empty">No judge result recorded. Run the free judge task for this run.</div>';
    return;
  }
  if (state.tab === "logs") {
    const logs = ["stdout.log", "stderr.log"].filter((name) => state.files[name]);
    panel.innerHTML = logs.length
      ? `<div class="split">${logs.map((name) => `<div class="pane"><h4>${esc(name)}</h4><pre>${esc(state.files[name])}</pre></div>`).join("")}</div>`
      : '<div class="empty">No stdout/stderr logs recorded for this run.</div>';
    return;
  }
  if (state.tab === "files") {
    const names = Object.keys(state.files);
    panel.innerHTML = names.length
      ? `<div class="toolbar"><select id="file-select">${names.map((name) => `<option ${name === state.file ? "selected" : ""}>${esc(name)}</option>`).join("")}</select></div><div class="pane"><h4>${esc(state.file)}</h4><pre>${esc(state.files[state.file] || "")}</pre></div>`
      : '<div class="empty">No raw files available.</div>';
    const select = $("#file-select");
    if (select) select.addEventListener("change", () => {
      state.file = select.value;
      renderRunPanel();
    });
    return;
  }
  panel.innerHTML = `
    <div class="card">
      <table>
        <thead><tr><th>Case</th><th>Status</th><th>Trials</th><th>Checks</th><th>What the checks mean</th></tr></thead>
        <tbody>${cases.map((item) => {
          const trials = item.trials || [];
          const grades = item.grades || [];
          return `<tr>
            <td><b>${esc(caseLabel(item.case_id))}</b><br><code>${esc(item.case_id)}</code></td>
            <td>${item.pass ? '<span class="badge pass">pass</span>' : '<span class="badge fail">fail</span>'}</td>
            <td>${trials.length}</td>
            <td>${grades.filter((grade) => grade.pass).length}/${grades.length}</td>
            <td>${grades.map((grade) => `<details class="small"><summary>${grade.pass ? "PASS" : "FAIL"} · ${esc(grade.grader)}</summary><p>${esc(graderExplain(grade.grader))}</p><p class="muted">${esc(grade.evidence || "")}</p></details>`).join("")}</td>
          </tr>`;
        }).join("") || '<tr><td colspan="5" class="empty">No cases in metadata.</td></tr>'}</tbody>
      </table>
    </div>`;
}

function renderTranscript(text) {
  const events = String(text || "").split("\n").map((line) => parse(line)).filter(Boolean);
  if (!events.length) return '<div class="empty">No transcript or observations recorded for this run.</div>';
  return `<div class="split">${events.slice(0, 40).map((event, index) => {
    const prompt = event.prompt ?? event.request ?? event.input ?? event.messages ?? event;
    const response = event.response ?? event.completion ?? event.output ?? event.text ?? "";
    return `<div class="pane"><h4>Event ${index + 1}</h4><div class="split">
      <div><h4>Prompt / request</h4><pre>${esc(typeof prompt === "string" ? prompt : JSON.stringify(prompt, null, 2))}</pre></div>
      <div><h4>Response / output</h4><pre>${esc(typeof response === "string" ? response : JSON.stringify(response, null, 2))}</pre></div>
    </div></div>`;
  }).join("")}</div>`;
}

function showView(view) {
  state.view = view;
  $$(".view").forEach((node) => node.classList.add("hidden"));
  $(`#view-${view}`).classList.remove("hidden");
  $$("nav button").forEach((button) => button.classList.toggle("active", button.dataset.view === view));
  if (view === "overview") renderOverview();
  if (view === "runs") renderRuns();
  if (view === "bench") renderBench();
  if (view === "method") renderMethod();
}

function theme() {
  const current = localStorage.getItem("eval-theme") || "dark";
  const next = current === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  localStorage.setItem("eval-theme", next);
}

async function init() {
  document.documentElement.dataset.theme = localStorage.getItem("eval-theme") || "dark";
  $("#theme").addEventListener("click", theme);
  $$("nav button").forEach((button) => button.addEventListener("click", () => showView(button.dataset.view)));
  try {
    const data = await api("/api/runs");
    state.runs = (data.runs || []).sort((a, b) => (b.started_at_ms || 0) - (a.started_at_ms || 0));
    $("#generated").textContent = ` ${state.runs.length} runs loaded.`;
    showView("overview");
  } catch (error) {
    notice(`Could not load runs: ${error.message}`);
  }
}

init();



















