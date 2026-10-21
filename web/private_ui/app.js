const state = { runs: [], view: "overview", selected: null, files: {}, file: "", tab: "overview" };
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
  const model = run.manifest?.model;
  if (!model) return "n/a";
  return model.provider ? `${model.provider}:${model.model}` : model.model;
}

function laneOf(run) {
  return run.manifest?.lane || "n/a";
}

function freeOf(run) {
  const value = run.manifest?.free_only;
  return value === true ? "free-only" : value === false ? "paid" : "n/a";
}

function passOf(run) {
  return run.pass === true;
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

function latestRuns() {
  const map = new Map();
  for (const run of state.runs) {
    if (!map.has(run.suite)) map.set(run.suite, run);
  }
  return Array.from(map.values());
}

function allSuites() {
  return Array.from(new Set(state.runs.map((run) => run.suite))).sort();
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
  const totalCases = state.runs.reduce((sum, run) => sum + run.cases_total, 0);
  const passed = state.runs.reduce((sum, run) => sum + run.cases_passed, 0);
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
      </div>
      <div class="metricline">
        <span><b>${run.cases_passed}/${run.cases_total}</b> cases</span>
        <span><b>${pct(rateOf(run))}</b> pass rate</span>
        <span><b>${esc(run.trace?.trace_id || "n/a")}</b> trace</span>
      </div>
      <button data-open-run="${esc([run.suite, run.ref, run.run_id].join("|"))}">Open run</button>
    </article>`).join("");
  $("#view-overview").innerHTML = `
    <div class="hero">
      <p class="eyebrow">private evaluation evidence</p>
      <h1>Benchmarks, metrics and full evidence</h1>
      <p class="muted">Every visible number is traceable to a run, a manifest, a trace or an explicit n/a.</p>
      <div class="kpis">
        <div class="kpi"><b>${state.runs.length}</b><span>runs</span></div>
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
        ${spark(state.runs.slice(0, 20).reverse().map(rateOf))}
        <h3>Coverage</h3>
        <p class="muted small">${suites.map(esc).join(" · ") || "n/a"}</p>
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
      <thead><tr><th>Status</th><th>Suite</th><th>Ref</th><th>Lane</th><th>Model</th><th>Free</th><th>Cases</th><th>Started</th><th>Run</th></tr></thead>
      <tbody id="run-rows"></tbody>
    </table>`;
  const update = () => {
    const rows = filteredRuns();
    $("#run-count").textContent = `${rows.length} of ${state.runs.length} runs`;
    $("#run-rows").innerHTML = rows.map((run) => `
      <tr class="click" data-open-run="${esc([run.suite, run.ref, run.run_id].join("|"))}">
        <td>${statusBadge(run)}</td>
        <td>${esc(run.suite)}</td>
        <td>${esc(run.ref)}</td>
        <td>${esc(laneOf(run))}</td>
        <td>${esc(modelOf(run))}</td>
        <td>${esc(freeOf(run))}</td>
        <td>${run.cases_passed}/${run.cases_total}</td>
        <td>${when(run.started_at_ms)}</td>
        <td><code>${esc(run.run_id)}</code></td>
      </tr>`).join("") || '<tr><td colspan="9" class="empty">No matching runs.</td></tr>';
    $$("[data-open-run]").forEach((row) => {
      row.addEventListener("click", () => openRun(row.dataset.openRun.split("|")));
    });
  };
  $("#run-search").addEventListener("input", update);
  $("#run-suite").addEventListener("change", update);
  $("#run-status").addEventListener("change", update);
  update();
}

function renderBench() {
  const latest = new Map();
  for (const run of state.runs) {
    const key = [run.suite, run.ref, laneOf(run), modelOf(run), freeOf(run)].join("|");
    if (!latest.has(key)) latest.set(key, run);
  }
  const rows = Array.from(latest.values()).map((run) => `
    <tr>
      <td>${esc(run.suite)}</td>
      <td>${esc(laneOf(run))}</td>
      <td>${esc(modelOf(run))}</td>
      <td>${esc(freeOf(run))}</td>
      <td>${statusBadge(run)}</td>
      <td>${pct(rateOf(run))}</td>
      <td>${run.cases_passed}/${run.cases_total}</td>
      <td>${esc(run.manifest?.metrics?.latency?.p50_ms ?? "n/a")} / ${esc(run.manifest?.metrics?.latency?.p95_ms ?? "n/a")} ms</td>
      <td>${esc(run.manifest?.metrics?.usage?.tokens_in ?? "n/a")} / ${esc(run.manifest?.metrics?.usage?.tokens_out ?? "n/a")}</td>
    </tr>`).join("");
  $("#view-bench").innerHTML = `
    <div class="hero">
      <p class="eyebrow">like-for-like</p>
      <h1>Benchmark matrix</h1>
      <p class="muted">Model, lane and free-only are provenance, not decoration. Missing measurements stay n/a.</p>
    </div>
    <div class="card">
      <table>
        <thead><tr><th>Suite</th><th>Lane</th><th>Model</th><th>Policy</th><th>Status</th><th>Rate</th><th>Cases</th><th>p50/p95</th><th>Tokens in/out</th></tr></thead>
        <tbody>${rows || '<tr><td colspan="9" class="empty">No benchmark runs yet.</td></tr>'}</tbody>
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
        <p>The UI is private. API keys and tokens are never exported. Full prompts and logs are visible only after Basic Auth and API session authentication.</p>
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
          <p class="muted"><code>${esc(run.run_id)}</code> · ${when(run.started_at_ms)} · ${esc(modelOf(run))} · ${esc(freeOf(run))}</p>
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
        <span><b>${esc(manifest.metrics?.latency?.p50_ms ?? "n/a")}</b> ms p50</span>
        <span><b>${esc(manifest.metrics?.latency?.p95_ms ?? "n/a")}</b> ms p95</span>
        <span><b>${esc(manifest.metrics?.quality?.mean_score ?? "n/a")}</b> quality</span>
        <span><b>${esc(manifest.metrics?.stability?.passed ?? "n/a")}/${esc(manifest.metrics?.stability?.total ?? "n/a")}</b> stable</span>
      </div>
    </div>
    <div class="tabs">
      <button data-tab="overview" class="active">Cases</button>
      <button data-tab="transcript">Transcripts</button>
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
  if (state.tab === "transcript") {
    const transcript = state.files["transcript.jsonl"] || state.files["observations.jsonl"] || "";
    panel.innerHTML = renderTranscript(transcript);
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
        <thead><tr><th>Case</th><th>Status</th><th>Trials</th><th>Grades</th><th>Evidence</th></tr></thead>
        <tbody>${cases.map((item) => {
          const trials = item.trials || [];
          const grades = item.grades || [];
          return `<tr>
            <td><code>${esc(item.case_id)}</code></td>
            <td>${item.pass ? '<span class="badge pass">pass</span>' : '<span class="badge fail">fail</span>'}</td>
            <td>${trials.length}</td>
            <td>${grades.filter((grade) => grade.pass).length}/${grades.length}</td>
            <td>${grades.map((grade) => `<div class="small ${grade.pass ? "muted" : ""}">${esc(grade.grader)}: ${esc(grade.evidence || "")}</div>`).join("")}</td>
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
