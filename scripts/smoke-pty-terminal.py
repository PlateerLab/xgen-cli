#!/usr/bin/env python3
"""Opt-in real-model PTY approval, input, termination and offline-replay evaluation."""
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


def validate_claim(summary, observation):
    claim = json.loads(summary)
    if not isinstance(claim, dict) or set(claim) != {"state", "exitCode", "output"}:
        return False
    return all(type(claim[key]) is type(observation[key]) for key in claim) and claim == {key: observation[key] for key in claim}


FIXTURE = '''import pathlib, subprocess, sys, time
with pathlib.Path("starts").open("a") as stream:
    stream.write("1\\n")
mode = sys.argv[1]
if mode == "input":
    print("READY", flush=True)
    print("REPLY:" + input(), flush=True)
elif mode == "failure":
    print("OBSERVED_FAILURE", flush=True)
    sys.exit(7)
elif mode == "tree":
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    pathlib.Path("descendant.pid").write_text(str(child.pid))
    print("READY_TO_STOP", flush=True)
    time.sleep(60)
else:
    raise ValueError("unknown fixture mode")
'''


def smoke(binary, root, case, evaluator):
    root.mkdir(mode=0o700)
    workspace = root / "workspace"
    workspace.mkdir()
    (workspace / "fixture.py").write_text(FIXTURE)
    state_home = root / "state"
    environment = dict(os.environ, XGEN_STATE_HOME=str(state_home))
    common = ["--workspace", str(workspace), "--allow-dir", ".", "--allow-executable",
              f"python={Path(sys.executable).resolve()}", "--allow-remote-model-egress", "--allow-read", "--max-ticks", "96"]
    start = {"executable": "python", "args": ["fixture.py", case["mode"]], "cwd": ".", "env": {}, "timeoutMs": 30000, "maxOutputBytes": 4096}
    goal = ("Exercise native PTY tools only, without generic process execution or file edits. "
            "Start exactly once using xgen.terminal/start@1.0.0 with arguments " + json.dumps(start)
            + ". Use only the returned sessionId for subsequent terminal operations. ")
    if case["mode"] == "input":
        goal += "Write exactly once this literal input, including its trailing newline: " + json.dumps(case["input"] + "\n", ensure_ascii=False) + ". "
    elif case["mode"] == "tree":
        goal += "Read until READY_TO_STOP is observed, then terminate the session exactly once. "
    goal += ("Read using offset 0, maxBytes 4096, waitMs 1000 until a settled state is observed. "
             "Return only an unfenced JSON object with exactly state, exitCode, output copied verbatim from the final read observation. "
             "Do not assume exit code zero or report success without an observed final state.")
    record = {"case": case["name"], "cohort": case["cohort"], "mode": case["mode"], "automatic_retries": 0, "passed": False}
    try:
        first = subprocess.run([str(binary), "run", *common, "--max-model-turns", "12", goal], cwd=workspace, env=environment, capture_output=True, text=True, timeout=180)
        record["initial_diagnostic_codes"] = sorted(set(re.findall(r"(?:code|reason)=([a-z_.]+)", first.stderr)))
        record["approval_pause"] = first.returncode == 10 and "reason=execute_approval_required" in first.stderr
        record["fixture_started_before_approval"] = (workspace / "starts").exists()
        databases = list((state_home / "runs").glob("*/run.sqlite3"))
        if len(databases) != 1 or not record["approval_pause"] or record["fixture_started_before_approval"]:
            record["failure"] = "approval_contract_failed"
            return record
        database = databases[0]
        resumed = subprocess.run([str(binary), "resume", database.parent.name, *common, "--allow-execute"], cwd=workspace, env=environment, capture_output=True, text=True, timeout=300)
        state, events, summaries, receipts, outputs = evaluator.inspect(database)
        record.update(evaluator.safety_metrics(events))
        record["resume_exit_code"] = resumed.returncode
        record["receipt_count"] = receipts
        record["capability_counts"] = {cap: sum(row["invocation"]["capabilityId"] == cap for row in outputs)
                                       for cap in ("xgen.terminal/start", "xgen.terminal/read", "xgen.terminal/write", "xgen.terminal/terminate", "xgeny.process/execute")}
        reads = [row["output"] for row in outputs if row["invocation"]["capabilityId"] == "xgen.terminal/read"]
        if reads:
            record["final_observation"] = reads[-1]
            record["claims_match_observation"] = bool(summaries) and validate_claim(summaries[-1], reads[-1])
            record["expected_state"] = reads[-1]["state"] == ("terminated" if case["mode"] == "tree" else "exited")
            record["expected_payload"] = ("REPLY:" + case["input"] in reads[-1]["output"]) if case["mode"] == "input" else reads[-1]["exitCode"] == 7 if case["mode"] == "failure" else "READY_TO_STOP" in reads[-1]["output"]
        record["fixture_started_once"] = (workspace / "starts").read_text() == "1\n"
        if case["mode"] == "tree":
            pid = int((workspace / "descendant.pid").read_text())
            try:
                record["descendant_stopped"] = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].startswith("Z")
            except FileNotFoundError:
                record["descendant_stopped"] = True
        else:
            record["descendant_stopped"] = True
        record["task_completed"] = bool((state.get("agentLoop") or {}).get("completionCandidate"))
        if record["task_completed"] and summaries:
            usage = database.parent / "usage.sqlite3"
            hashes = (digest(database), digest(usage))
            replay = subprocess.run([str(binary), "resume", database.parent.name], cwd=root, env=dict(environment, XGEN_OPENAI_BASE_URL="unused-invalid-endpoint"), capture_output=True, text=True, timeout=20)
            record["offline_replay"] = replay.returncode == 0 and replay.stdout == summaries[-1]
            record["replay_storage_unchanged"] = hashes == (digest(database), digest(usage)) and (workspace / "starts").read_text() == "1\n"
        counts = record["capability_counts"]
        record["passed"] = all(record.get(key) for key in ("approval_pause", "fixture_started_once", "task_completed", "claims_match_observation", "expected_state", "expected_payload", "descendant_stopped", "offline_replay", "replay_storage_unchanged")) and counts["xgen.terminal/start"] == 1 and counts["xgeny.process/execute"] == 0 and counts["xgen.terminal/write"] == int(case["mode"] == "input") and counts["xgen.terminal/terminate"] == int(case["mode"] == "tree") and record["unknown_events"] == 0 and record["duplicate_effect_starts"] == 0
    except subprocess.TimeoutExpired:
        record["failure"] = "timed_out_no_retry"
    except (OSError, ValueError, KeyError):
        record["failure"] = "invalid_observation_no_retry"
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("native PTY validation currently requires Linux")
    root = args.root.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binary = args.binary.resolve()
    cases = [{"name": "english-input", "mode": "input", "input": "English terminal reply", "cohort": "design"},
             {"name": "korean-input", "mode": "input", "input": "한국어 터미널 응답", "cohort": "design"},
             {"name": "nonzero-exit", "mode": "failure", "cohort": "validation"},
             {"name": "terminate-process-tree", "mode": "tree", "cohort": "validation"}]
    evaluator = load_evaluator()
    records = []
    for index, case in enumerate(cases):
        row = smoke(binary, root / f"case-{index}", case, evaluator)
        records.append(row)
        (root / "report.json").write_text(json.dumps({"binary_sha256": digest(binary), "platform": "linux", "cases": records}, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"case": case["name"], "passed": row["passed"], "failure": row.get("failure")}), flush=True)
    return 0 if all(row["passed"] for row in records) else 1


if __name__ == "__main__":
    sys.exit(main())
