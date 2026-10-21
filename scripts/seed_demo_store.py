"""Seed a tiny offline store for the private UI.

This is intentionally synthetic demo data, not eval evidence. The generated
run is labelled `demo` so it is not confused with production results.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import time

RUN_ID = "demo-rag-001"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=pathlib.Path, default=pathlib.Path("eval-runs"))
    args = parser.parse_args()
    root = args.out.resolve()
    run = root / "runs" / "research-agents-service" / "demo" / ("run-" + RUN_ID)
    run.mkdir(parents=True, exist_ok=True)
    sources = [
        {"title": "Retrieval-Augmented Generation for Knowledge-Intensive NLP", "doi": "10.48550/arXiv.2005.11401"},
        {"title": "Lost in the Middle: How Language Models Use Long Contexts", "doi": "10.48550/arXiv.2307.03172"},
        {"title": "RAGAS: Automated Evaluation of Retrieval Augmented Generation", "doi": "10.48550/arXiv.2309.15217"},
    ]
    answer = """# How RAG reduces hallucinations

Retrieval-augmented generation (RAG) separates knowledge from model weights.
A retriever selects passages, and the generator is conditioned on those
passages. This makes answers more grounded and easier to inspect.

## Mechanisms

- Retrieval supplies fresh, attributable context.
- Grounding instructions require the model to answer from the supplied context.
- Citation and abstention checks reduce unsupported claims.
- Evaluation should include both answer quality and source quality.

## Limitations

RAG does not eliminate hallucinations. Bad retrieval, conflicting sources, and
weak grounding instructions can still produce incorrect answers.

This is synthetic demo content for the offline UI; it is not a production run.
"""
    (run / "artifact.md").write_text(answer, encoding="utf-8")
    metadata = {
        "run_id": RUN_ID,
        "suite": "research-agents-service",
        "ref_name": "demo",
        "started_at_ms": int(time.time() * 1000),
        "trials_total": 1,
        "cases": [
            {
                "case_id": "demo-rag",
                "pass": True,
                "trials": [
                    {
                        "output": {
                            "output": {
                                "text": answer,
                                "data": {
                                    "query": "How does retrieval-augmented generation reduce hallucinations?",
                                    "status": "pass",
                                    "nsources": len(sources),
                                    "citations": 3,
                                    "artifact_chars": len(answer),
                                    "sources": sources,
                                },
                            }
                        }
                    }
                ],
            }
        ],
    }
    manifest = {
        "manifest_version": 1,
        "lane": "replay",
        "model": {"provider": "demo", "model": "synthetic"},
        "target": {"version": "offline-fixture"},
        "free_only": True,
        "metrics": {"quality": {}, "stability": {}},
    }
    for name, value in (
        ("metadata.json", metadata),
        ("manifest.json", manifest),
        ("judge.json", {
            "schema": "eval.judge/v1",
            "model": "demo",
            "overall": 8,
            "scores": {
                "grounding": 8,
                "relevance": 9,
                "completeness": 7,
                "clarity": 9,
                "no_padding": 8,
                "overall": 8,
                "rationale": "Synthetic demo content; not production evidence.",
                "issues": ["Synthetic fixture, not a real model run."],
            },
        }),
        ("grades.json", []),
    ):
        (run / name).write_text(json.dumps(value, indent=2), encoding="utf-8")
    print(f"seeded demo run at {run}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
