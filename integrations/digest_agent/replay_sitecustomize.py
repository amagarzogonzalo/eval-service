"""Offline replay transport for the real digest entrypoint.

Copied to a temporary working directory as ``sitecustomize.py`` and put
on ``PYTHONPATH`` only for replay. It replaces external HTTP with
recorded fixtures; the real ``python -m digest_agent.main`` entrypoint,
orchestrator, renderer, and SMTP notifier still run unchanged.

- GitHub REST calls are served from ``github_activity.json``.
- Groq chat completions are served from ``llm_fixture.json`` and only
  permit the target's reviewed free Groq candidates.
- Any unexpected network call raises, so replay cannot silently reach
  the internet.
"""

import atexit
import json
import os
import threading
from datetime import UTC, datetime
from urllib.parse import urlsplit

import requests

_LOCK = threading.Lock()
_COUNTS = {
    "github_requests": 0,
    "model_requests": 0,
    "paid_model_attempts": 0,
    "unexpected_calls": 0,
    "models": [],
}

_GITHUB_FIXTURE = os.environ.get("EVAL_DIGEST_FIXTURE_GITHUB", "")
_LLM_FIXTURE = os.environ.get("EVAL_DIGEST_FIXTURE_LLM", "")
_COUNTS_OUT = os.environ.get("EVAL_DIGEST_FIXTURE_COUNTS", "")
_FREE_MODELS = {
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b",
    "groq/compound-mini",
}
_ACTIVITY = {}
_LLM_PAYLOAD = {}


def _load():
    global _LLM_PAYLOAD
    with open(_GITHUB_FIXTURE, encoding="utf-8") as handle:
        data = json.load(handle)
    repos = data.get("repos", data) if isinstance(data, dict) else data
    for item in repos:
        _ACTIVITY[item["repo"]] = item
    with open(_LLM_FIXTURE, encoding="utf-8") as handle:
        _LLM_PAYLOAD = json.load(handle)


class _Response:
    def __init__(self, status_code, payload, url=""):
        self.status_code = status_code
        self._payload = payload
        self.url = url
        self.headers = {"content-type": "application/json"}
        self.text = json.dumps(payload)

    def json(self):
        return self._payload

    def raise_for_status(self):
        if self.status_code >= 400:
            raise requests.HTTPError(
                f"HTTP {self.status_code} for {self.url}", response=self
            )


def _write_counts():
    if not _COUNTS_OUT:
        return
    with _LOCK:
        payload = json.dumps(_COUNTS)
    temp = _COUNTS_OUT + ".tmp"
    try:
        with open(temp, "w", encoding="utf-8") as handle:
            handle.write(payload)
        os.replace(temp, _COUNTS_OUT)
    except OSError:
        pass


def _bump(name):
    with _LOCK:
        _COUNTS[name] += 1
    _write_counts()


def _now():
    return datetime.now(UTC).isoformat()


def _github(repo):
    activity = _ACTIVITY.get(repo)
    if activity is None:
        return _Response(404, {"message": "Not Found"}, repo)
    return activity


def _get(url, *args, **kwargs):
    parsed = urlsplit(str(url))
    path = parsed.path
    if parsed.netloc != "api.github.com":
        _bump("unexpected_calls")
        raise RuntimeError(f"unexpected replay GET: {url}")
    _bump("github_requests")
    parts = [p for p in path.split("/") if p]
    # /repos/{owner}/{repo}/...
    if len(parts) < 3 or parts[0] != "repos":
        return _Response(404, {"message": "Not Found"}, str(url))
    repo = parts[1] + "/" + parts[2]
    activity = _github(repo)
    if isinstance(activity, _Response):
        return activity
    tail = parts[3:]
    if tail[:1] == ["commits"] and len(tail) == 1:
        commits = []
        for commit in activity.get("commits", []):
            commits.append({
                "sha": commit["sha"],
                "commit": {
                    "message": commit.get("message", ""),
                    "author": {"name": commit.get("author", "dev")},
                },
            })
        return _Response(200, commits, str(url))
    if tail[:1] == ["commits"] and len(tail) == 2:
        want = tail[1]
        for commit in activity.get("commits", []):
            if commit["sha"].startswith(want) or want.startswith(commit["sha"]):
                return _Response(200, {"files": commit.get("files", [])},
                                 str(url))
        return _Response(200, {"files": []}, str(url))
    if tail[:1] == ["pulls"] and len(tail) == 1:
        pull_requests = []
        for pr in activity.get("pull_requests", []):
            merged = bool(pr.get("merged"))
            pull_requests.append({
                "number": pr["number"],
                "title": pr.get("title", ""),
                "state": pr.get("state", "closed"),
                "merged_at": _now() if merged else None,
                "updated_at": _now(),
                "changed_files": pr.get("changed_files", len(pr.get("files", []))),
                "additions": pr.get("additions", 0),
                "deletions": pr.get("deletions", 0),
            })
        return _Response(200, pull_requests, str(url))
    if tail[:1] == ["pulls"] and len(tail) == 3 and tail[2] == "files":
        number = int(tail[1])
        for pr in activity.get("pull_requests", []):
            if int(pr["number"]) == number:
                return _Response(200, pr.get("files", []), str(url))
        return _Response(404, {"message": "Not Found"}, str(url))
    return _Response(404, {"message": "Not Found"}, str(url))


def _post(url, *args, **kwargs):
    parsed = urlsplit(str(url))
    if parsed.netloc != "api.groq.com":
        _bump("unexpected_calls")
        raise RuntimeError(f"unexpected replay POST: {url}")
    body = kwargs.get("json") or {}
    model = body.get("model", "")
    if model not in _FREE_MODELS:
        _bump("paid_model_attempts")
        return _Response(403, {"error": "paid model forbidden"}, str(url))
    _bump("model_requests")
    with _LOCK:
        if model not in _COUNTS["models"]:
            _COUNTS["models"].append(model)
    payload = dict(_LLM_PAYLOAD)
    payload["model"] = model
    return _Response(200, payload, str(url))


_load()
requests.get = _get
requests.post = _post
atexit.register(_write_counts)
