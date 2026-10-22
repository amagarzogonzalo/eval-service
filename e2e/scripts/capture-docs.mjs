import fs from "node:fs";
import path from "node:path";

import { chromium } from "@playwright/test";

import globalSetup from "../global-setup.js";

const port = process.env.EVAL_E2E_PORT || "8091";
const baseURL = `http://127.0.0.1:${port}`;
const outDir = path.resolve("..", "docs", "assets");
fs.mkdirSync(outDir, { recursive: true });

const teardown = await globalSetup();
let browser = null;

try {
  browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1440, height: 820 } });
  await page.goto(`${baseURL}/login`);
  await page.locator("#token").fill("demo");
  await page.getByRole("button", { name: "Unlock" }).click();
  await page.getByRole("heading", { name: "Benchmarks, metrics and full evidence" }).waitFor();
  await page.addStyleTag({
    content: "* { animation: none !important; transition: none !important; }",
  });
  await page.waitForTimeout(300);

  await page.screenshot({ path: path.join(outDir, "private-ui-overview.png") });

  await page.getByRole("button", { name: "Runs", exact: true }).click();
  await page.locator("#run-rows tr", { hasText: "demo-rag-001" }).click();
  await page.locator("#view-run").waitFor();
  await page.getByRole("button", { name: "Query / Sources / Answer" }).click();
  await page.locator("#run-panel .source-card").first().waitFor();
  await page.waitForTimeout(200);
  await page.locator("#run-panel").screenshot({
    path: path.join(outDir, "private-ui-run.png"),
  });

  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole("button", { name: "Benchmarks" }).click();
  await page.locator("#bench-charts .chart").first().waitFor();
  await page.addStyleTag({ content: ".topbar { display: none !important; }" });
  await page.waitForTimeout(200);
  await page.locator("#bench-charts").screenshot({
    path: path.join(outDir, "private-ui-benchmarks.png"),
  });

  console.log(`wrote documentation screenshots to ${outDir}`);
} finally {
  if (browser) await browser.close();
  await teardown();
}
