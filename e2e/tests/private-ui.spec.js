import { expect, test as base } from "@playwright/test";

const RAG_QUERY = "How does retrieval-augmented generation reduce hallucinations?";

const test = base.extend({
  page: async ({ page }, use) => {
    const errors = [];
    page.on("console", (message) => {
      if (message.type() !== "error") return;
      const location = message.location();
      if (location.url?.endsWith("/favicon.ico")) return;
      errors.push(`console: ${message.text()} (${location.url}:${location.lineNumber})`);
    });
    page.on("pageerror", (error) => {
      errors.push(`pageerror: ${error.message}\n${error.stack || ""}`);
    });
    await use(page);
    expect(errors, "browser console/page errors").toEqual([]);
  },
});

async function login(page) {
  await page.goto("/login");
  await page.locator("#token").fill("demo");
  await page.getByRole("button", { name: "Unlock" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(
    page.getByRole("heading", { name: "Benchmarks, metrics and full evidence" }),
  ).toBeVisible();
}

async function openDemoRun(page) {
  await page.getByRole("button", { name: "Runs", exact: true }).click();
  const row = page.locator("#run-rows tr", { hasText: "demo-rag-001" });
  await expect(row).toHaveCount(1);
  await row.click();
  await expect(page.locator("#view-run")).toBeVisible();
  await expect(page.locator("#view-run")).toContainText("research-agents-service");
  await expect(page.locator("#view-run")).not.toContainText("legacy run");
}

function benchRows(page) {
  return page.locator("#view-bench > .card table tbody tr:visible");
}

function chart(page, title) {
  return page.locator("#bench-charts .chart", {
    has: page.getByRole("heading", { name: title, exact: true }),
  });
}

test("unauthenticated access redirects pages and rejects API calls", async ({ page, request }) => {
  const pageResponse = await request.get("/", { maxRedirects: 0 });
  expect(pageResponse.status()).toBe(303);
  expect(pageResponse.headers().location).toBe("/login");

  const apiResponse = await request.get("/api/runs");
  expect(apiResponse.status()).toBe(401);
  expect(await apiResponse.json()).toEqual({ error: "unauthorized" });

  await page.goto("/");
  expect(new URL(page.url()).pathname).toBe("/login");
  await expect(page.locator("#token")).toBeVisible();
});

test("login with the demo token loads the overview", async ({ page }) => {
  await login(page);
  await expect(page.getByRole("button", { name: "Overview" })).toHaveClass(/active/);
  await expect(page.locator("#view-overview")).toContainText("research-agents-service");
  await expect(page.locator("#view-overview")).toContainText("demo:synthetic");
  await expect(page.locator("#view-overview")).toContainText("replay");
  await expect(page.locator("#generated")).toContainText("3 runs loaded.");
  await expect(page.locator("#view-overview")).toContainText("1 legacy runs excluded from charts");
});

test("runs table opens the demo run", async ({ page }) => {
  await login(page);
  await openDemoRun(page);
  await expect(page.locator("#view-run")).toContainText("demo-rag-001");
  await expect(page.locator("#view-run")).toContainText(RAG_QUERY);
  await expect(page.locator("#view-run")).toContainText("demo:synthetic");
  await expect(page.locator("#view-run")).toContainText("offline-fixture");
  await expect(page.locator("#view-run")).not.toContainText("legacy run");
});

test("query, sources, and answer show the synthetic RAG evidence", async ({ page }) => {
  await login(page);
  await openDemoRun(page);
  await page.getByRole("tab", { name: "Query / Sources / Answer" }).click();

  const panel = page.locator("#run-panel");
  await expect(panel).toContainText(RAG_QUERY);
  await expect(panel.locator(".source-card")).toHaveCount(3);
  await expect(panel).toContainText("Retrieval-Augmented Generation for Knowledge-Intensive NLP");
  await expect(panel).toContainText("Lost in the Middle");
  await expect(panel).toContainText("RAGAS");
  await expect(panel.locator(".markdown")).toContainText(
    "Retrieval-augmented generation (RAG) separates knowledge from model weights.",
  );
  await expect(panel.locator(".markdown")).toContainText(
    "This is synthetic demo content for the offline UI",
  );
});

test("judge tab shows score, five dimensions, rationale, and issues", async ({ page }) => {
  await login(page);
  await openDemoRun(page);
  await page.getByRole("tab", { name: "Judge", exact: true }).click();

  const panel = page.locator("#run-panel");
  await expect(panel.locator(".judge-score")).toContainText("8/10 overall");
  await expect(panel.locator(".score-row")).toHaveCount(5);
  await expect(panel.locator(".score-row .bar i")).toHaveCount(5);
  for (const dimension of ["grounding", "relevance", "completeness", "clarity", "no padding"]) {
    await expect(panel.locator(".score-row", { hasText: dimension })).toHaveCount(1);
  }
  await expect(panel).toContainText("Synthetic demo content; not production evidence.");
  await expect(panel).toContainText("Synthetic fixture, not a real model run.");
});

test("benchmarks render charts, filters, model cards, and disaggregated table", async ({ page }) => {
  await login(page);
  await page.getByRole("button", { name: "Benchmarks" }).click();

  await expect(page.locator("#view-bench")).toBeVisible();
  await expect(page.locator("#bench-charts .chart")).toHaveCount(4);
  for (const title of [
    "Pass rate vs p50",
    "Throughput vs p50",
    "Sources vs citations",
    "Judge vs pass rate",
  ]) {
    await expect(chart(page, title)).toHaveCount(1);
  }
  await expect(page.locator("#bench-charts svg")).toHaveCount(4);
  await expect(page.locator("#bench-query")).toBeVisible();
  await expect(page.locator("#bench-model")).toBeVisible();
  await expect(page.locator("#bench-model option")).toHaveCount(4);
  await expect(page.locator("#bench-model-cards .model-card")).toHaveCount(2);
  await expect(page.locator("#bench-model-summary tbody tr")).toHaveCount(2);
  await expect(benchRows(page)).toHaveCount(2);
  await expect(page.locator("#bench-legacy")).not.toBeChecked();
});

test("selecting the RAG query filters the table and charts", async ({ page }) => {
  await login(page);
  await page.getByRole("button", { name: "Benchmarks" }).click();
  await expect(benchRows(page)).toHaveCount(2);

  const sources = chart(page, "Sources vs citations");
  const judge = chart(page, "Judge vs pass rate");
  const passRate = chart(page, "Pass rate vs p50");
  await expect(sources.locator("circle.dot")).toHaveCount(2);
  await expect(judge.locator("circle.dot")).toHaveCount(2);
  await expect(passRate.locator("circle.dot")).toHaveCount(1);

  await page.selectOption("#bench-query", { value: RAG_QUERY });
  await expect(page.locator("#bench-query")).toHaveValue(RAG_QUERY);
  await expect(benchRows(page)).toHaveCount(1);
  await expect(benchRows(page)).toContainText(RAG_QUERY);

  await expect(sources.locator("circle.dot")).toHaveCount(1);
  await expect(judge.locator("circle.dot")).toHaveCount(1);
  await expect(passRate.locator("circle.dot")).toHaveCount(0);
  await expect(passRate.locator(".empty")).toContainText("No numeric runs for this view.");
  await expect(page.locator("#bench-model-cards .model-card")).toHaveCount(1);
  await expect(page.locator("#bench-model-summary tbody tr")).toHaveCount(1);
});

test("legacy toggle hides the legacy fixture and the demo run stays current", async ({ page }) => {
  await login(page);
  await page.getByRole("button", { name: "Runs", exact: true }).click();

  const demoRow = page.locator("#run-rows tr", { hasText: "demo-rag-001" });
  const legacyRow = page.locator("#run-rows tr", { hasText: "e2e-legacy-001" });
  await expect(demoRow).toHaveCount(1);
  await expect(demoRow.locator(".badge.legacy")).toHaveCount(0);
  await expect(legacyRow).toHaveCount(1);
  await expect(legacyRow.locator(".badge.legacy")).toHaveCount(1);

  await page.getByRole("button", { name: "Benchmarks" }).click();
  await expect(page.locator("#bench-legacy")).not.toBeChecked();
  await expect(benchRows(page)).toHaveCount(2);
  await expect(benchRows(page).filter({ hasText: "Legacy synthetic fixture" })).toHaveCount(0);

  await page.locator("#bench-legacy").check();
  await expect(benchRows(page)).toHaveCount(3);
  const visibleLegacy = benchRows(page).filter({ hasText: "Legacy synthetic fixture" });
  await expect(visibleLegacy).toHaveCount(1);
  await expect(visibleLegacy.locator(".badge.legacy")).toHaveCount(1);

  await page.locator("#bench-legacy").uncheck();
  await expect(benchRows(page)).toHaveCount(2);
});
