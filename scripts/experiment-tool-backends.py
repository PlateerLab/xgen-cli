#!/usr/bin/env python3
"""Compare pinned external search backends without model calls or credentials."""

import argparse
import concurrent.futures
import datetime
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
from urllib.parse import urlsplit


CASES = [
    {"id": "rust-command", "split": "design", "language": "en", "query": "Rust std process Command documentation", "domains": ["doc.rust-lang.org"]},
    {"id": "python-subprocess-ko", "split": "design", "language": "ko", "query": "파이썬 subprocess 공식 문서", "domains": ["docs.python.org"]},
    {"id": "sqlite-wal", "split": "held_out", "language": "en", "query": "SQLite write ahead logging official documentation", "domains": ["sqlite.org"]},
    {"id": "node-stream", "split": "held_out", "language": "en", "query": "Node.js stream backpressure official documentation", "domains": ["nodejs.org"]},
    {"id": "git-worktree-ko", "split": "held_out", "language": "ko", "query": "git worktree 공식 문서", "domains": ["git-scm.com"]},
    {"id": "rust-cargo-ko", "split": "held_out", "language": "ko", "query": "Rust Cargo 의존성 지정 공식 문서", "domains": ["doc.rust-lang.org"]},
    {"id": "recent-python", "split": "held_out", "language": "en", "query": "Python latest release October 2026", "domains": ["python.org"], "freshness_scored": False},
    {"id": "recent-korean", "split": "held_out", "language": "ko", "query": "한국은행 2026년 9월 통화정책", "domains": ["bok.or.kr"], "freshness_scored": False},
]

DDGS_CALL = """import json, sys
from ddgs import DDGS
print(json.dumps(DDGS(timeout=8).text(sys.argv[1], max_results=5, backend='auto'), ensure_ascii=False))
"""


def normalize_results(backend, payload):
    """Reject malformed schemas; preserve original rank and unmodified text."""
    if backend == "ddgs":
        rows = payload
        url_key, snippet_key = "href", "body"
    elif backend == "openserp":
        rows = payload.get("results") if isinstance(payload, dict) else None
        url_key, snippet_key = "url", "snippet"
    else:
        raise ValueError("unknown backend")
    if not isinstance(rows, list):
        raise ValueError("results must be an array")
    results = []
    for row in rows[:5]:
        if not isinstance(row, dict):
            raise ValueError("result must be an object")
        url, title, snippet = row.get(url_key), row.get("title"), row.get(snippet_key, "")
        if not all(isinstance(value, str) for value in (url, title, snippet)):
            raise ValueError("result fields must be strings")
        parsed = urlsplit(url)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username or parsed.password:
            raise ValueError("result URL must use HTTP(S) syntax without credentials")
        results.append({"title": title, "url": url, "snippet": snippet})
    return results


def official_rank(results, domains):
    for rank, row in enumerate(results, start=1):
        host = (urlsplit(row["url"]).hostname or "").lower()
        if any(host == domain or host.endswith("." + domain) for domain in domains):
            return rank
    return None


def stop_group(process):
    """Signal the owned group before reaping its leader, preventing PID reuse."""
    if os.name == "posix":
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    elif process.poll() is None:
        process.kill()


def wait_unreaped(process, timeout):
    """Observe exit without freeing the group leader's PID before cleanup."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if status is not None and status.si_pid == process.pid:
            return
        time.sleep(0.01)
    raise subprocess.TimeoutExpired(process.args, timeout)


def trial(args, backend, case, repetition):
    if backend == "ddgs":
        command = [str(args.ddgs_python), "-c", DDGS_CALL, case["query"]]
    else:
        command = [str(args.openserp), "search", "duckduckgo", case["query"], "--limit", "5", "--timeout", "8", "--search-timeout", "12", "--max_retries", "0", "--format", "json", "--leakless", "--browser-path", str(args.browser)]
    # Never forward API keys, tokens, proxies with credentials, or the caller's full environment.
    environment = {key: os.environ[key] for key in ("PATH", "HOME", "LANG", "SYSTEMROOT", "WINDIR", "TMP", "TEMP", "TMPDIR") if key in os.environ}
    record = {"backend": backend, "case": case["id"], "split": case["split"], "language": case["language"], "repetition": repetition, "status": "unknown", "results": []}
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="xgen-search-trial-") as directory:
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            process = subprocess.Popen(command, cwd=directory, env=environment, stdout=stdout, stderr=stderr, start_new_session=os.name == "posix")
            try:
                wait_unreaped(process, timeout=20)
            except subprocess.TimeoutExpired:
                record["status"] = "timed_out"
            finally:
                stop_group(process)
                record["exit_code"] = process.wait(timeout=5)
            stdout.seek(0)
            raw = stdout.read(512 * 1024 + 1)
            stderr.seek(0)
            record["stderr"] = stderr.read(4096).decode("utf-8", errors="replace")
    record["elapsed_ms"] = round((time.monotonic() - started) * 1000)
    record["stdout_sha256"] = hashlib.sha256(raw).hexdigest()
    if record["status"] != "timed_out":
        if len(raw) > 512 * 1024:
            record["status"] = "output_too_large"
        elif record["exit_code"] != 0:
            record["status"] = "backend_failed"
        else:
            try:
                record["results"] = normalize_results(backend, json.loads(raw))
                record["status"] = "ok" if record["results"] else "empty"
            except (ValueError, UnicodeError):
                record["status"] = "invalid_response"
    record["official_rank"] = official_rank(record["results"], case["domains"])
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ddgs-python", type=Path, required=True)
    parser.add_argument("--openserp", type=Path, required=True)
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=2)
    args = parser.parse_args()
    if args.repetitions < 1 or os.name != "posix" or not hasattr(os, "waitid") or not hasattr(os, "WNOWAIT"):
        parser.error("positive repetitions and POSIX waitid/WNOWAIT process-group cleanup required")
    for key in ("ddgs_python", "openserp", "browser"):
        value = getattr(args, key).absolute()
        if not value.is_file():
            parser.error(f"{key} must identify an installed executable")
        setattr(args, key, value)
    version = subprocess.check_output([str(args.ddgs_python), "-c", "import importlib.metadata; print(importlib.metadata.version('ddgs'))"], text=True).strip()
    jobs = [(backend, case, repetition) for repetition in range(1, args.repetitions + 1) for case in CASES for backend in ("ddgs", "openserp")]
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        trials = list(pool.map(lambda job: trial(args, *job), jobs))
    summary = {}
    for backend in ("ddgs", "openserp"):
        rows = [row for row in trials if row["backend"] == backend]
        summary[backend] = {"trials": len(rows), "nonempty": sum(row["status"] == "ok" for row in rows), "official_top5": sum(row["official_rank"] is not None for row in rows), "mean_elapsed_ms": round(sum(row["elapsed_ms"] for row in rows) / len(rows)), "held_out_official_top5": sum(row["official_rank"] is not None for row in rows if row["split"] == "held_out"), "held_out_trials": sum(row["split"] == "held_out" for row in rows)}
    report = {"created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "versions": {"ddgs": version, "openserp_binary_sha256": hashlib.sha256(args.openserp.read_bytes()).hexdigest()}, "protocol": {"concurrency": 2, "deadline_seconds": 20, "result_limit": 5, "ddgs_backend": "auto", "openserp_backend": "duckduckgo", "openserp_mode": "browser", "same_engine_comparison": False, "freshness_scored": False, "adoption_gate": "No adoption on availability alone; review relevance and failure handling separately."}, "cases": CASES, "summary": summary, "trials": trials}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
