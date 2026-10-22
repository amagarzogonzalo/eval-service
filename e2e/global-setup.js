import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import { writeSyntheticFixtures } from "./fixtures/synthetic-runs.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const port = process.env.EVAL_E2E_PORT || "8091";
const baseURL = `http://127.0.0.1:${port}`;

function python() {
  const explicit = process.env.EVAL_E2E_PYTHON;
  if (explicit) return { command: explicit, prefix: [] };
  for (const command of ["python3", "python"]) {
    try {
      execFileSync(command, ["--version"], { stdio: "ignore" });
      return { command, prefix: [] };
    } catch {
      continue;
    }
  }
  try {
    execFileSync("py", ["-3", "--version"], { stdio: "ignore" });
    return { command: "py", prefix: ["-3"] };
  } catch {
    throw new Error(
      "Python 3 was not found. Set EVAL_E2E_PYTHON to the interpreter used by this checkout.",
    );
  }
}

async function ready() {
  try {
    const health = await fetch(`${baseURL}/healthz`, {
      signal: AbortSignal.timeout(1_000),
    });
    if (!health.ok) return false;
    const body = await health.json();
    if (body?.ok !== true) return false;
    const login = await fetch(`${baseURL}/login`, {
      signal: AbortSignal.timeout(1_000),
    });
    if (!login.ok) return false;
    return (await login.text()).includes("eval-service");
  } catch {
    return false;
  }
}

async function waitForServer(child, output, getSpawnError) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const spawnError = getSpawnError();
    if (spawnError) {
      throw new Error(`could not start the private UI server: ${spawnError.message}`);
    }
    if (child.exitCode !== null) {
      throw new Error(`private UI server exited early.\n${output()}`);
    }
    if (await ready()) return;
    await delay(250);
  }
  throw new Error(`private UI server did not become healthy at ${baseURL}.\n${output()}`);
}

function stopServer(child) {
  return new Promise((resolve) => {
    if (child.exitCode !== null) {
      resolve();
      return;
    }
    const kill = (signal) => {
      try {
        child.kill(signal);
      } catch {
        resolve();
      }
    };
    const timer = setTimeout(() => {
      kill("SIGKILL");
      resolve();
    }, 5_000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
    kill();
  });
}

export default async function globalSetup() {
  const interpreter = python();
  const store = fs.mkdtempSync(path.join(os.tmpdir(), "eval-private-e2e-"));
  const seed = path.join(repoRoot, "scripts", "seed_demo_store.py");
  const service = path.join(repoRoot, "web", "private_service.py");
  const uiRoot = path.join(repoRoot, "web", "private_ui");
  let child = null;

  try {
    execFileSync(interpreter.command, [...interpreter.prefix, seed, "--out", store], {
      cwd: repoRoot,
      stdio: "inherit",
    });
    const demo = JSON.parse(
      fs.readFileSync(path.join(store, "runs", "research-agents-service", "demo",
        "run-demo-rag-001", "metadata.json"), "utf8"),
    );
    writeSyntheticFixtures(store, demo.started_at_ms);

    let stdout = "";
    let stderr = "";
    let spawnError = null;
    child = spawn(
      interpreter.command,
      [
        ...interpreter.prefix,
        service,
        "--root",
        uiRoot,
        "--store",
        store,
        "--host",
        "127.0.0.1",
        "--port",
        port,
        "--allow-http-cookie",
      ],
      {
        cwd: repoRoot,
        env: {
          ...process.env,
          EVAL_SITE_API_TOKEN: "demo",
          EVAL_SITE_SIGNING_KEY: "demo-only",
        },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    child.on("error", (error) => {
      spawnError = error;
    });
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });

    await waitForServer(child, () => `${stdout}${stderr}`, () => spawnError);
    console.log(`E2E private UI ready at ${baseURL} (store ${store})`);
  } catch (error) {
    if (child) await stopServer(child);
    fs.rmSync(store, { recursive: true, force: true });
    throw error;
  }

  return async () => {
    await stopServer(child);
    if (process.env.EVAL_E2E_KEEP !== "1") {
      fs.rmSync(store, { recursive: true, force: true });
    } else {
      console.log(`Kept E2E store at ${store}`);
    }
  };
}
