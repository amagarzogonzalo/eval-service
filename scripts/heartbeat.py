"""Send a dead-man heartbeat without printing the endpoint URL."""

from __future__ import annotations

import argparse
import os
import urllib.request


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url-env", default="HEARTBEAT_URL")
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--required", action="store_true")
    args = parser.parse_args()
    url = os.environ.get(args.url_env, "").strip()
    if not url:
        print(f"{args.url_env} is not set; heartbeat skipped")
        return 2 if args.required else 0
    try:
        with urllib.request.urlopen(url, timeout=args.timeout) as response:
            if response.status >= 300:
                print(f"heartbeat returned HTTP {response.status}")
                return 1
    except OSError as exc:
        print(f"heartbeat failed: {type(exc).__name__}")
        return 1
    print("heartbeat sent")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

