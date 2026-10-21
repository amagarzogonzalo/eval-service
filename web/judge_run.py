"""Free-LLM content judge for a stored eval run.

Reads a run's query and captured result, asks a free OpenRouter model for a
rubric score, and writes judge.json into the run directory. It fails closed
when the model is not free, the response is malformed, or cost is non-zero.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import re
import time
import urllib.request
import zlib

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


def clean_text(text: str) -> str:
    text = text.replace("\u007f", " ").replace("\u00af", "-").replace("\u0097", "-")
    text = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]", " ", text)
    return re.sub(r"[ \t]+", " ", text)


def pdf_string(value: bytes) -> str:
    value = re.sub(rb"\\([()\\])", rb"\1", value)
    value = re.sub(rb"\\[0-7]{1,3}", lambda m: bytes([int(m.group()[1:], 8)]), value)
    return value.decode("latin-1", "replace")


def pdf_text(data: bytes) -> str:
    pages = []
    for stream in re.finditer(rb"stream\r?\n(.*?)endstream", data, re.DOTALL):
        raw = stream.group(1).strip()
        try:
            if raw.endswith(b"~>"):
                raw = base64.a85decode(b"<~" + raw, adobe=True)
            raw = zlib.decompress(raw)
        except (ValueError, zlib.error):
            continue
        chunks = []
        for match in re.finditer(rb"\((?:\\.|[^\\()])*\)\s*(?:Tj|TJ|')", raw, re.DOTALL):
            chunks.append(pdf_string(match.group(0).split(b")")[0][1:]))
        for match in re.finditer(rb"\[(.*?)\]\s*TJ", raw, re.DOTALL):
            for item in re.findall(rb"\((?:\\.|[^\\()])*\)", match.group(1)):
                chunks.append(pdf_string(item[1:-1]))
        text = " ".join(chunks).replace("\\n", "\n")
        if text.strip():
            pages.append(text)
    return "\n".join(pages)


def extract_pdf(path: pathlib.Path) -> str:
    try:
        return pdf_text(path.read_bytes())
    except OSError:
        return ""


def latest_run(store: pathlib.Path, suite: str, ref: str) -> tuple[pathlib.Path, dict]:
    root = store / "runs" / suite / ref
    candidates = sorted(
        (path for path in root.glob("run-*") if path.is_dir()),
        key=lambda path: path.name,
    )
    for run_dir in reversed(candidates):
        meta = load_json(run_dir / "metadata.json")
        if not isinstance(meta, dict):
            continue
        content = [
            case for case in meta.get("cases") or []
            if "health" not in str(case.get("case_id") or "").lower()
            and case.get("trials")
        ]
        if content:
            return run_dir, meta
    raise SystemExit(f"no content runs for {suite}/{ref}")


def content_trials(meta: dict):
    for case in meta.get("cases") or []:
        case_id = str(case.get("case_id") or "").lower()
        if any(word in case_id for word in ("health", "preflight", "fail-closed", "bad-repo")):
            continue
        for trial in case.get("trials") or []:
            output = (trial.get("output") or {}).get("output") or {}
            yield output, (output.get("data") or {})


def extract_result(run_dir: pathlib.Path, meta: dict) -> str:
    artifact = run_dir / "artifact.md"
    if artifact.is_file():
        return clean_text(artifact.read_text(encoding="utf-8", errors="replace"))
    for output, data in content_trials(meta):
        path = data.get("artifact_path")
        if isinstance(path, str) and path and pathlib.Path(path).is_file():
            text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
            artifact.write_text(text, encoding="utf-8")
            return clean_text(text)
        pdf_path = data.get("pdf_path")
        if isinstance(pdf_path, str) and pdf_path and pathlib.Path(pdf_path).is_file():
            text = extract_pdf(pathlib.Path(pdf_path))
            if len(text.strip()) >= 200:
                artifact.write_text(text, encoding="utf-8")
                return clean_text(text)
        text = output.get("text")
        if isinstance(text, str) and text.strip():
            return clean_text(text)
    return ""


def extract_query(meta: dict) -> str:
    for _output, data in content_trials(meta):
        for key in ("query", "topic"):
            value = data.get(key)
            if isinstance(value, str) and value.strip():
                return value
    return "Digest of recent GitHub activity"


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






