#!/usr/bin/env python3
"""Opt-in live search through the existing approval/receipt/replay process route."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def load_evaluator():
    path = Path(__file__).with_name("evaluate-projects.py")
    spec = importlib.util.spec_from_file_location("project_evaluator", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def claimed_sources(summary):
    claim = json.loads(summary)
    if not isinstance(claim, dict) or set(claim) != {"sources"}:
        raise ValueError("one exact JSON object required")
    rows = claim["sources"]
    if not isinstance(rows, list) or len(rows) != 2:
        raise ValueError("two source records required")
    if not all(isinstance(row, dict) and set(row) == {"title", "url"}
               and all(isinstance(value, str) for value in row.values()) for row in rows):
        raise ValueError("source fields must be strings")
    return rows


def execute_approval_pause(exit_code, stderr):
    return exit_code == 10 and "reason=execute_approval_required" in stderr


def smoke(binary, backend, root, query, evaluator):
    root.mkdir(mode=0o700)
    workspace = root / "workspace"
    workspace.mkdir()
    state_home = root / "state"
    environment = dict(os.environ, XGEN_STATE_HOME=str(state_home))
    common = ["--workspace", str(workspace), "--allow-dir", ".", "--allow-executable",
              f"web-search={backend}", "--allow-remote-model-egress", "--allow-read", "--max-ticks", "64"]
    argv = ["search", "duckduckgo", query, "--limit", "3", "--timeout", "8", "--search-timeout", "12",
            "--max_retries", "0", "--format", "json", "--leakless"]
    goal = ("Run the catalogued web-search executable exactly once with this literal argv: "
            + json.dumps(argv, ensure_ascii=False)
            + ". Use cwd '.', env {}, timeoutMs 20000, maxOutputBytes 32768. "
            "Do not edit files. After successful execution, return only a JSON object "
            "with one key sources containing the first two search results in their original order. "
            "Each item must contain exactly title and url copied verbatim from the tool output. "
            "If execution fails, report the failure without inventing sources.")
    record = {"query": query, "automatic_retries": 0, "passed": False}
    try:
        first = subprocess.run([str(binary), "run", *common, "--max-model-turns", "8", goal],
                               cwd=workspace, env=environment, capture_output=True, text=True, timeout=180)
        record["initial_exit_code"] = first.returncode
        record["initial_diagnostic_codes"] = sorted(set(re.findall(r"(?:code|reason)=([a-z_.]+)", first.stderr)))
        databases = list((state_home / "runs").glob("*/run.sqlite3"))
        if len(databases) != 1:
            record["failure"] = "run_not_created"
            record["initial_exit_code"] = first.returncode
            return record
        database = databases[0]
        before = evaluator.inspect(database)
        record.update(evaluator.safety_metrics(before[1]))
        record["approval_pause"] = execute_approval_pause(first.returncode, first.stderr)
        record["process_outputs_before_approval"] = sum(row["invocation"]["capabilityId"] == "xgeny.process/execute" for row in before[4])
        if not record["approval_pause"] or record["process_outputs_before_approval"]:
            record["failure"] = "execution_before_approval" if record["process_outputs_before_approval"] else "expected_execute_approval_pause_missing"
            return record
        resumed = subprocess.run([str(binary), "resume", database.parent.name, *common, "--allow-execute"],
                                 cwd=workspace, env=environment, capture_output=True, text=True, timeout=180)
        state, events, summaries, receipts, outputs = evaluator.inspect(database)
        record.update(evaluator.safety_metrics(events))
        process_outputs = [row for row in outputs if row["invocation"]["capabilityId"] == "xgeny.process/execute"]
        record["process_outputs_after_approval"] = len(process_outputs)
        record["receipt_count"] = receipts
        record["resume_exit_code"] = resumed.returncode
        record["task_completed"] = bool((state.get("agentLoop") or {}).get("completionCandidate"))
        record["observed_commands"] = evaluator.observed_commands(database, outputs)
        if len(process_outputs) == 1:
            output = process_outputs[0]["output"]
            record["process_success"] = output["success"]
            record["output_truncated"] = output["stdoutTruncated"] or output["stderrTruncated"]
            if output["success"] and not record["output_truncated"]:
                payload = json.loads(output["stdout"])
                expected = [{"title": row["title"], "url": row["url"]} for row in payload["results"][:2]]
                record["observed_sources"] = expected
                if summaries:
                    try:
                        record["claimed_sources"] = claimed_sources(summaries[-1])
                        record["claims_match_observation"] = record["claimed_sources"] == expected
                    except ValueError:
                        record["claims_match_observation"] = False
        if record["task_completed"] and summaries:
            usage_database = database.parent / "usage.sqlite3"
            hashes = (digest(database), digest(usage_database))
            replay_env = dict(environment, XGEN_OPENAI_BASE_URL="unused-invalid-endpoint")
            replay = subprocess.run([str(binary), "resume", database.parent.name], cwd=root,
                                    env=replay_env, capture_output=True, text=True, timeout=20)
            record["offline_replay"] = replay.returncode == 0 and replay.stdout == summaries[-1]
            record["replay_journal_unchanged"] = digest(database) == hashes[0]
            record["replay_usage_unchanged"] = digest(usage_database) == hashes[1]
        record["passed"] = all(record.get(key) for key in ("approval_pause", "task_completed", "process_success", "claims_match_observation", "offline_replay", "replay_journal_unchanged", "replay_usage_unchanged")) and record["process_outputs_after_approval"] == 1 and record["unknown_events"] == 0 and record["duplicate_effect_starts"] == 0
    except subprocess.TimeoutExpired:
        record["failure"] = "timed_out_no_retry"
    except (OSError, ValueError, KeyError):
        record["failure"] = "invalid_observation_no_retry"
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--openserp", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    binary, backend, root = args.binary.resolve(), args.openserp.resolve(), args.root.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    evaluator = load_evaluator()
    queries = ["curl CURLOPT_SSL_VERIFYPEER official documentation", "파이썬 ast 공식 문서"]
    records = [smoke(binary, backend, root / f"case-{index}", query, evaluator) for index, query in enumerate(queries)]
    report = {"binary_sha256": digest(binary), "openserp_binary_sha256": digest(backend), "production_search_capability_registered": False, "route": "existing xgeny.process/execute catalog and approval", "cases": records}
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"cases": len(records), "passed": sum(row["passed"] for row in records)}))
    return 0 if all(row["passed"] for row in records) else 1


if __name__ == "__main__":
    sys.exit(main())
