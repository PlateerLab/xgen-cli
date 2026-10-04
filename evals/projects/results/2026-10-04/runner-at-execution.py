#!/usr/bin/env python3
"""Exercise local-build coding workflows on frozen disposable language projects."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def quote_matches(summary, source):
    return any(block.strip() == source.strip() for block in
               re.findall(r"```[^\n]*\n(.*?)```", summary, re.S))


def run(command, cwd, environment, timeout, script=None):
    return subprocess.run(command, cwd=cwd, env=environment, input=script,
                          capture_output=True, text=True, timeout=timeout)


def inspect(database):
    with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as connection:
        state = json.loads(connection.execute("SELECT state_json FROM run_projection").fetchone()[0])
        events = [json.loads(row[0])["body"] for row in
                  connection.execute("SELECT event_json FROM run_events ORDER BY sequence")]
        summaries = [json.loads(row[0])["summary"] for row in
                     connection.execute("SELECT record_json FROM completion_outputs ORDER BY event_sequence")]
        receipts = connection.execute("SELECT COUNT(*) FROM execution_receipts").fetchone()[0]
        outputs = [json.loads(row[0]) for row in
                   connection.execute("SELECT record_json FROM tool_outputs ORDER BY event_sequence")]
    return state, events, summaries, receipts, outputs


def preserved(workspace, originals):
    return all((workspace / name).is_file() and (workspace / name).read_bytes() == content
               for name, content in originals.items())


def evaluate(binary, case, directory, timeout, base_environment):
    directory.mkdir(mode=0o700)
    workspace = directory / "workspace"
    workspace.mkdir()
    state_root = directory / "state"
    environment = dict(base_environment, XGEN_STATE_HOME=str(state_root))
    for name, content in case["files"].items():
        relative = Path(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("fixture paths must stay inside workspace")
        path = workspace / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    originals = {name: (workspace / name).read_bytes() for name in case["files"] if name != case["source"]}
    commands = [[shutil.which(command[0]), *command[1:]] for command in case["commands"]]
    record = {"case": case["id"], "split": case["split"], "language": case["language"],
              "mode": case["mode"], "outcome": "not_completed", "model_calls": 0,
              "tool_effects": 0, "automatic_retries": 0}
    started = time.monotonic()
    try:
        before = run(commands[0], workspace, environment, timeout)
        record["initial_tests_failed"] = before.returncode != 0
        if before.returncode == 0:
            record["outcome"] = "invalid_fixture"
            return record
        goal = (f"실패 원인을 찾아 {case['source']}만 수정해줘. 기존 테스트와 설정 파일은 변경하지 마. "
                f"먼저 테스트 실패를 관찰하고 수정 뒤 다음 검증 명령을 모두 실행해줘: {json.dumps(case['commands'])}. "
                "최종 응답에는 실제 검증 결과와 수정한 소스 파일 전체를 코드 블록으로 그대로 넣어줘.")
        if case["mode"] == "bare":
            script = ("/permissions model allow\n/permissions read allow\n/permissions write allow\n"
                      "/permissions execute allow\n" + goal + "\n/status\n/usage\n/exit\n")
            first = run([str(binary)], workspace, environment, timeout, script)
        else:
            executable = case["commands"][0][0]
            common = ["--workspace", str(workspace), "--allow-dir", ".", "--allow-executable",
                      f"{executable}={shutil.which(executable)}", "--allow-remote-model-egress",
                      "--allow-read", "--allow-write", "--max-ticks", "256"]
            first = run([str(binary), "run", *common, "--max-model-turns", "24", goal],
                        workspace, environment, timeout)
        databases = list((state_root / "runs").glob("*/run.sqlite3"))
        if len(databases) != 1:
            record["outcome"] = "run_not_created"
            return record
        database = databases[0]
        run_id = database.parent.name
        if case["mode"] == "run-resume":
            prior = inspect(database)
            record["execute_approval_pause"] = (first.returncode == 10 and
                "reason=execute_approval_required" in first.stderr)
            record["process_outputs_before_approval"] = sum(
                output["invocation"]["capabilityId"] == "xgeny.process/execute" for output in prior[4])
            if record["execute_approval_pause"]:
                first = run([str(binary), "resume", run_id, *common, "--allow-execute"],
                            workspace, environment, timeout)
            else:
                record["outcome"] = "expected_pause_missing"
        state, events, summaries, receipts, outputs = inspect(database)
        candidate = (state.get("agentLoop") or {}).get("completionCandidate")
        record["task_completed"] = bool(candidate and candidate.get("responseKind", "task_completion") == "task_completion")
        record["tests_preserved"] = preserved(workspace, originals)
        checks = [run(command, workspace, environment, timeout).returncode == 0 for command in commands]
        record["independent_tests_passed"], record["independent_build_passed"] = checks
        validation = directory / "validation"
        shutil.copytree(workspace, validation, ignore=shutil.ignore_patterns("target", "__pycache__"))
        (validation / case["hidden_path"]).write_text(case["hidden_tests"])
        hidden_command = commands[0]
        if case["language"] == "node":
            hidden_command = [shutil.which("node"), "--test", "visible.test.mjs", case["hidden_path"]]
        record["hidden_tests_passed"] = run(hidden_command, validation, environment, timeout).returncode == 0
        record["exact_final_source_quote"] = bool(summaries and quote_matches(summaries[-1], (workspace / case["source"]).read_text()))
        record["model_calls"] = sum(event["type"] == "model_call_reserved" for event in events)
        record["tool_effects"] = sum(event["type"] == "effect_succeeded" for event in events)
        record["unknown_events"] = sum(event["type"] in ("model_call_marked_unknown", "effect_outcome_unknown") for event in events)
        record["receipt_count"] = receipts
        process_outputs = [output for output in outputs if output["invocation"]["capabilityId"] == "xgeny.process/execute"]
        record["observed_failed_process"] = any(output["output"].get("success") is False for output in process_outputs)
        usage = run([str(binary), "usage", run_id], workspace, environment, timeout)
        if usage.returncode == 0:
            raw = json.loads(usage.stdout)
            record["usage"] = {key: raw[key] for key in ("reservedCalls", "observedCalls", "callsWithTokenUsage",
                "callsWithCacheUsage", "elapsedMillisSubtotal", "tokenSubtotal", "completeTokenUsage")}
        manifest = json.loads((database.parent / "manifest.json").read_text())["record"]
        record["model"] = manifest["model"]
        record["request_profile_digest"] = manifest["requestProfileDigest"]
        if record["task_completed"]:
            before_database, before_usage = digest(database), digest(database.parent / "usage.sqlite3")
            replay_environment = dict(environment, XGEN_OPENAI_BASE_URL="unused-invalid-endpoint")
            replay = run([str(binary), "resume", run_id], directory, replay_environment, timeout)
            record["offline_replay"] = replay.returncode == 0 and replay.stdout == summaries[-1]
            record["replay_journal_unchanged"] = before_database == digest(database)
            record["replay_usage_unchanged"] = before_usage == digest(database.parent / "usage.sqlite3")
        record["diagnostic_codes"] = sorted(set(re.findall(r"(?:code|reason)=([a-z_.]+)", first.stderr)))
        acceptance = (record["task_completed"] and record["tests_preserved"] and all(checks)
            and record["hidden_tests_passed"] and record.get("offline_replay", False)
            and record.get("replay_journal_unchanged", False) and record.get("replay_usage_unchanged", False))
        if case["mode"] == "run-resume":
            acceptance = acceptance and record["execute_approval_pause"] and record["process_outputs_before_approval"] == 0
        record["outcome"] = "pass" if acceptance else "workflow_failure"
    except subprocess.TimeoutExpired:
        record["outcome"] = "timeout_no_retry"
    except (OSError, ValueError, sqlite3.Error, KeyError):
        record["outcome"] = "observation_failure_no_retry"
    finally:
        record["elapsed_seconds"] = time.monotonic() - started
        (directory / "result.json").write_text(json.dumps(record, ensure_ascii=False, indent=2))
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--cases", type=Path, default=HERE.parent / "evals/projects/cases.json")
    parser.add_argument("--attempts", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    if args.attempts < 1 or args.timeout < 1:
        parser.error("positive attempts and timeout required")
    binary = args.binary.resolve()
    cases = json.loads(args.cases.read_text())
    if any(shutil.which(runtime) is None for runtime in ("cargo", "node", "python3")):
        parser.error("cargo, node and python3 required")
    root = Path(tempfile.mkdtemp(prefix="xgen-project-acceptance-"))
    environment = {name: value for name, value in os.environ.items()
                   if not name.startswith(("XGEN_", "XGENY_", "DEEPSEEK_"))}
    registration = {"binary_sha256": digest(binary), "cases_sha256": digest(args.cases),
                    "runner_sha256": digest(Path(__file__)), "attempts": args.attempts,
                    "timeout": args.timeout, "source_commit": subprocess.check_output(
                        ["git", "rev-parse", "origin/main"], cwd=HERE, text=True).strip()}
    (root / "registration.json").write_text(json.dumps(registration, indent=2))
    shutil.copyfile(args.cases, root / "cases.json")
    shutil.copyfile(__file__, root / "runner.py")
    records = []
    print(f"REPORT_ROOT {root}", flush=True)
    for split in ("design", "held_out"):
        for case in (case for case in cases if case["split"] == split):
            for attempt in range(1, args.attempts + 1):
                record = evaluate(binary, case, root / f"{case['id']}-{attempt}", args.timeout, environment)
                record["attempt"] = attempt
                records.append(record)
                (root / "report.json").write_text(json.dumps({"registration": registration, "records": records}, ensure_ascii=False, indent=2))
                print(json.dumps({"case": case["id"], "attempt": attempt, "outcome": record["outcome"], "model_calls": record["model_calls"]}), flush=True)
    return int(any(record["outcome"] != "pass" for record in records))


if __name__ == "__main__":
    raise SystemExit(main())
