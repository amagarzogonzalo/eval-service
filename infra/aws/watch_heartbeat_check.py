"""External stale-heartbeat alarm for `eval watch`.

Run from a systemd timer or cron. Exits 0 when the heartbeat is fresh,
1 when it is stale (or a run is stale), and 2 when it cannot be read.
The flag file is written atomically for an external monitor or the S3
sync; the script itself never contacts GitHub or AWS.
"""

import argparse
import json
import pathlib
import sys
import time


def write_flag(path, payload):
    if not path:
        return
    target = pathlib.Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_suffix(target.suffix + ".tmp")
    temporary.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(target)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--heartbeat", required=True)
    parser.add_argument("--max-age-seconds", type=int, default=900)
    parser.add_argument("--flag", default="")
    args = parser.parse_args()

    now_ms = int(time.time() * 1000)
    try:
        data = json.loads(pathlib.Path(args.heartbeat).read_text(encoding="utf-8"))
        last_beat = int(data.get("last_beat_ms", 0))
        stale_running = int(data.get("stale_running", 0))
    except (OSError, ValueError, TypeError) as error:
        payload = {
            "alarm": "heartbeat_unreadable",
            "detail": str(error),
            "checked_at_ms": now_ms,
        }
        write_flag(args.flag, payload)
        print(json.dumps(payload))
        return 2

    age_ms = max(0, now_ms - last_beat)
    reasons = []
    if last_beat <= 0:
        reasons.append("heartbeat has no last_beat_ms")
    elif age_ms > args.max_age_seconds * 1000:
        reasons.append(
            f"heartbeat is {age_ms // 1000}s old (limit {args.max_age_seconds}s)"
        )
    if stale_running > 0:
        reasons.append(f"{stale_running} running request(s) exceeded the stale threshold")

    payload = {
        "alarm": "stale" if reasons else "ok",
        "age_seconds": age_ms // 1000,
        "max_age_seconds": args.max_age_seconds,
        "stale_running": stale_running,
        "last_beat_ms": last_beat,
        "last_run_id": data.get("last_run_id"),
        "reasons": reasons,
        "checked_at_ms": now_ms,
    }
    if reasons:
        write_flag(args.flag, payload)
        print(json.dumps(payload))
        return 1
    if args.flag:
        try:
            pathlib.Path(args.flag).unlink()
        except FileNotFoundError:
            pass
    print(json.dumps(payload))
    return 0


if __name__ == "__main__":
    sys.exit(main())
