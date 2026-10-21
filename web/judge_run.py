"""Free-LLM content judge for a stored eval run.

Reads a run's query and captured result, asks a free OpenRouter model for a
rubric score, and writes judge.json into the run directory. It fails closed
when the model is not free, the response is malformed, or cost is non-zero.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import time
import urllib.request

FREE_MODEL = "nvidia/nemotron-3-ultra-550b-a55b:free"
RUBRIC = """Score each dimension 0-10 using only the supplied query and result.
Return strict JSON with:
{
  "grounding": number,
  "relevance": number,
  "completeness": number,
  "clarity": number,
  "no_padding": number,
  "overall": number,
  "rationale": string,
  "issues": [string]
}
Judge the content, not the formatting. Do not invent facts. If the result is
empty or unusable, set overall to 0 and explain why."""


def load_json(path: pathlib.Path) -> object | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def is_free_model(model: str) -> bool:
    return model.endswith(":free") or model in {"openrouter/free"}


def latest_run(store: pathlib.Path, suite: str, ref: str) -> tuple[pathlib.Path, dict]:
    root = store / "runs" / suite / ref
    candidates = sorted(
        (path for path in root.glob("run-*") if path.is_dir()),
        key=lambda path: path.name,
    )
    for run_dir in reversed(candidates):
        meta = load_json(run_dir / "metadata.json")
        if isinstance(meta, dict):
            return run_dir, meta
    raise SystemExit(f"no runs for {suite}/{ref}")


def extract_result(run_dir: pathlib.Path, meta: dict) -> str:
    artifact = run_dir / "artifact.md"
    if artifact.is_file():
        return artifact.read_text(encoding="utf-8", errors="replace")
    for case in meta.get("cases") or []:
        for trial in case.get("trials") or []:
            output = (trial.get("output") or {}).get("output") or {}
            data = output.get("data") or {}
            path = data.get("artifact_path")
            if isinstance(path, str) and path and pathlib.Path(path).is_file():
                text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
                artifact.write_text(text, encoding="utf-8")
                return text
            text = output.get("text")
            if isinstance(text, str) and text.strip():
                return text
    return ""


def extract_query(meta: dict) -> str:
    for case in meta.get("cases") or []:
        for trial in case.get("trials") or []:
            output = (trial.get("output") or {}).get("output") or {}
            data = output.get("data") or {}
            for key in ("query", "topic"):
                value = data.get(key)
                if isinstance(value, str) and value.strip():
                    return value
    return ""


def parse_json(text: str) -> dict:
    cleaned = text.strip()
    if cleaned.startswith("```"):
        cleaned = re.sub(r"^```(?:json)?\s*", "", cleaned)
        cleaned = re.sub(r"\s*```$", "", cleaned)
    value = json.loads(cleaned)
    if not isinstance(value, dict):
        raise TypeError("judge response is not an object")
    for key in ("grounding", "relevance", "completeness", "clarity", "no_padding", "overall"):
        number = value.get(key)
        if not isinstance(number, (int, float)) or not 0 <= number <= 10:
            raise ValueError(f"invalid judge score: {key}")
    return value


def call_model(base_url: str, model: str, key: str, query: str, result: str) -> tuple[dict, dict]:
    body = {
        "model": model,
        "temperature": 0,
        "messages": [
            {"role": "system", "content": RUBRIC},
            {
                "role": "user",
                "content": f"QUERY:\n{query}\n\nRESULT:\n{result[:30000]}",
            },
        ],
    }
    request = urllib.request.Request(
        base_url.rstrip("/") + "/chat/completions",
        data=json.dumps(body).encode("utf-8"),
        headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        payload = json.loads(response.read().decode("utf-8"))
    content = ((payload.get("choices") or [{}])[0].get("message") or {}).get("content", "")
    return parse_json(content), payload


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", type=pathlib.Path, default=pathlib.Path("/var/lib/eval-runs"))
    parser.add_argument("--suite", required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--run-id", default="")
    parser.add_argument("--model", default=os.environ.get("EVAL_JUDGE_MODEL", FREE_MODEL))
    parser.add_argument("--base-url", default="https://openrouter.ai/api/v1")
    parser.add_argument("--key-env", default="OPENROUTER_API_KEY")
    args = parser.parse_args()
    if not is_free_model(args.model):
        raise SystemExit(f"refusing non-free judge model: {args.model}")
    key = os.environ.get(args.key_env, "")
    if not key:
        raise SystemExit(f"{args.key_env} is required")
    if args.run_id:
        run_dir = args.store / "runs" / args.suite / args.ref / ("run-" + args.run_id)
        meta = load_json(run_dir / "metadata.json")
        if not isinstance(meta, dict):
            raise SystemExit(f"run not found: {run_dir}")
    else:
        run_dir, meta = latest_run(args.store, args.suite, args.ref)
    query = extract_query(meta)
    result = extract_result(run_dir, meta)
    if not result.strip():
        raise SystemExit("run has no captured result to judge")
    scores, payload = call_model(args.base_url, args.model, key, query, result)
    usage = payload.get("usage") or {}
    cost = usage.get("cost")
    if isinstance(cost, (int, float)) and cost > 0:
        raise SystemExit(f"judge reported non-zero cost: {cost}")
    output = {
        "schema": "eval.judge/v1",
        "run_id": run_dir.name.removeprefix("run-"),
        "suite": args.suite,
        "ref": args.ref,
        "model": args.model,
        "rubric": "content-quality/v1",
        "query": query,
        "scores": scores,
        "overall": scores["overall"],
        "usage": {
            "input_tokens": usage.get("prompt_tokens"),
            "output_tokens": usage.get("completion_tokens"),
            "cost_usd": cost if cost is not None else 0,
        },
        "generated_at_ms": int(time.time() * 1000),
    }
    path = run_dir / "judge.json"
    path.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())



