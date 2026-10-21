"""Plan or apply conservative run-store retention.

Rules:
- keep the newest ``--keep-latest`` runs in every suite/ref group;
- delete only runs older than ``--days`` days;
- dry-run by default; ``--apply`` is required to remove directories.

Use this after a restore drill, never as a substitute for S3 versioning.
"""

from __future__ import annotations

import argparse
import datetime
import pathlib
import shutil
import time


def run_groups(root: pathlib.Path) -> dict[pathlib.Path, list[pathlib.Path]]:
    groups: dict[pathlib.Path, list[pathlib.Path]] = {}
    for run_dir in root.glob("runs/*/*/run-*"):
        if run_dir.is_dir():
            groups.setdefault(run_dir.parent, []).append(run_dir)
    for runs in groups.values():
        runs.sort(key=lambda path: path.name)
    return groups


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", type=pathlib.Path, required=True)
    parser.add_argument("--keep-latest", type=int, default=20)
    parser.add_argument("--days", type=int, default=365)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    if args.keep_latest < 1 or args.days < 1:
        raise SystemExit("--keep-latest and --days must be positive")

    root = args.store.resolve()
    if not root.is_dir():
        raise SystemExit(f"store is not a directory: {root}")
    cutoff = time.time() - args.days * 86400
    groups = run_groups(root)
    planned = []
    for runs in groups.values():
        protected = set(runs[-args.keep_latest :])
        for run_dir in runs:
            if run_dir in protected:
                continue
            if run_dir.stat().st_mtime < cutoff:
                planned.append(run_dir)

    for run_dir in planned:
        size = sum(path.stat().st_size for path in run_dir.rglob("*") if path.is_file())
        stamp = datetime.datetime.fromtimestamp(run_dir.stat().st_mtime).isoformat(timespec="seconds")
        print(f"{'DELETE' if args.apply else 'PLAN'}: {run_dir} ({size} bytes, mtime {stamp})")
        if args.apply:
            shutil.rmtree(run_dir)
    print(f"{len(planned)} run(s) {'deleted' if args.apply else 'selected (dry-run)'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

