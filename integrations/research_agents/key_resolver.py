"""Resolve a runtime credential without printing or committing it.

Order: environment, explicit env files, then (optionally) the local OpenCode
auth store. The value is written to stdout only so a caller can capture it;
diagnostics go to stderr and never include the value.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

OPENCODE_AUTH = Path.home() / ".local" / "share" / "opencode" / "auth.json"


def parse_env_file(path: Path, name: str) -> str:
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""
    for line in lines:
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        if key.strip() != name:
            continue
        return value.strip().strip("'\"")
    return ""


def parse_opencode(provider: str) -> str:
    try:
        data = json.loads(OPENCODE_AUTH.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return ""
    item = data.get(provider)
    if isinstance(item, dict):
        value = item.get("key") or item.get("api_key") or item.get("token")
        if isinstance(value, str):
            return value.strip()
    if isinstance(item, str):
        return item.strip()
    return ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--name", required=True)
    parser.add_argument("--file", action="append", default=[])
    parser.add_argument("--opencode-provider", default="")
    parser.add_argument("--allow-env", action="store_true", default=True)
    args = parser.parse_args(argv)

    value = os.environ.get(args.name, "").strip() if args.allow_env else ""
    if not value:
        for path in args.file:
            value = parse_env_file(Path(path), args.name)
            if value:
                break
    if not value and args.opencode_provider:
        value = parse_opencode(args.opencode_provider)
    if not value:
        sys.stderr.write(f"credential not found: {args.name}\n")
        return 1
    sys.stdout.write(value)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
