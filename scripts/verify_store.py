"""Validate an eval run store without network access.

The check is intentionally conservative: it proves that the store is readable
and that each run has parseable metadata, and that current-looking manifests
are valid JSON objects. It does not re-run graders or trust the results.
"""

from __future__ import annotations

import argparse
import json
import pathlib


def load_json(path: pathlib.Path) -> object:
    return json.loads(path.read_text(encoding="utf-8"))


def validate_store(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    run_dirs = sorted(root.glob("runs/*/*/run-*"))
    if not run_dirs:
        errors.append(f"no run directories found under {root}")
        return errors
    for run_dir in run_dirs:
        metadata = run_dir / "metadata.json"
        if not metadata.is_file():
            errors.append(f"missing metadata: {run_dir}")
            continue
        try:
            value = load_json(metadata)
        except (OSError, ValueError) as exc:
            errors.append(f"invalid metadata {metadata}: {exc}")
            continue
        if not isinstance(value, dict):
            errors.append(f"metadata is not an object: {metadata}")
            continue
        if not value.get("cases"):
            errors.append(f"metadata has no cases: {metadata}")
        manifest = run_dir / "manifest.json"
        if manifest.is_file():
            try:
                manifest_value = load_json(manifest)
            except (OSError, ValueError) as exc:
                errors.append(f"invalid manifest {manifest}: {exc}")
                continue
            if not isinstance(manifest_value, dict):
                errors.append(f"manifest is not an object: {manifest}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("store", type=pathlib.Path)
    args = parser.parse_args()
    root = args.store.resolve()
    if not root.is_dir():
        print(f"store is not a directory: {root}")
        return 2
    errors = validate_store(root)
    if errors:
        for error in errors:
            print(f"ERROR: {error}")
        return 1
    count = len(list(root.glob("runs/*/*/run-*")))
    print(f"store OK: {count} run(s) under {root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
