"""Export current runs as a human-label calibration worksheet.

The output is a JSONL candidate set. It deliberately leaves human fields
empty: labels must be supplied by a person, never inferred from the judge.
"""

from __future__ import annotations

import argparse
import json
import pathlib
from typing import Any


def load_json(path: pathlib.Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def content_trial(meta: dict) -> dict:
    for case in meta.get("cases") or []:
        case_id = str(case.get("case_id") or "").lower()
        if any(word in case_id for word in ("health", "preflight", "fail-closed", "bad-repo")):
            continue
        for trial in case.get("trials") or []:
            output = (trial.get("output") or {}).get("output") or {}
            data = output.get("data") or {}
            if data.get("artifact_path") or data.get("sources") or output.get("text"):
                return {"output": output, "data": data}
    return {}


def read_result(run_dir: pathlib.Path, data: dict, output: dict) -> str:
    artifact = run_dir / "artifact.md"
    if artifact.is_file():
        return artifact.read_text(encoding="utf-8", errors="replace")
    path = data.get("artifact_path")
    if isinstance(path, str) and path:
        candidate = pathlib.Path(path)
        if candidate.is_file():
            return candidate.read_text(encoding="utf-8", errors="replace")
    return str(output.get("text") or "")


def query_of(meta: dict, data: dict) -> str:
    for value in (data.get("query"), data.get("topic"), meta.get("query")):
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


def candidate(run_dir: pathlib.Path, max_chars: int) -> dict | None:
    meta = load_json(run_dir / "metadata.json")
    if not isinstance(meta, dict):
        return None
    manifest = load_json(run_dir / "manifest.json")
    if not isinstance(manifest, dict) or not manifest.get("model"):
        return None
    selected = content_trial(meta)
    if not selected:
        return None
    data = selected["data"]
    output = selected["output"]
    query = query_of(meta, data)
    result = read_result(run_dir, data, output)
    if not query or not result.strip():
        return None
    judge = load_json(run_dir / "judge.json") or {}
    score = judge.get("overall") if isinstance(judge, dict) else None
    label = None
    if isinstance(score, (int, float)):
        label = "pass" if score >= 8 else "revise"
    run_id = str(meta.get("run_id") or run_dir.name.removeprefix("run-"))
    suite = str(meta.get("suite") or run_dir.parents[1].name)
    ref = str(meta.get("ref_name") or run_dir.parent.name)
    return {
        "id": f"{suite}/{ref}/{run_id}",
        "suite": suite,
        "ref": ref,
        "run_id": run_id,
        "query": query,
        "rubric": "content-quality/v1",
        "judge_model": judge.get("model") if isinstance(judge, dict) else None,
        "judge_label": label,
        "judge_score": score,
        "human_label": None,
        "human_score": None,
        "notes": "Fill human_label (pass/revise) and human_score (0-10).",
        "result": result[:max_chars],
        "result_truncated": len(result) > max_chars,
    }


def export_rows(root: pathlib.Path, limit: int, max_chars: int) -> list[dict]:
    rows = []
    for run_dir in root.glob("runs/*/*/run-*"):
        item = candidate(run_dir, max_chars)
        if item:
            rows.append(item)
    rows.sort(key=lambda row: row["id"], reverse=True)
    return rows[:limit]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", type=pathlib.Path, required=True)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--limit", type=int, default=40)
    parser.add_argument("--max-chars", type=int, default=12000)
    args = parser.parse_args()
    if args.limit < 1 or args.max_chars < 1:
        raise SystemExit("--limit and --max-chars must be positive")
    rows = export_rows(args.store.resolve(), args.limit, args.max_chars)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(f"wrote {len(rows)} candidate rows to {args.out}")
    print("Human labels are intentionally empty; do not auto-fill them.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


