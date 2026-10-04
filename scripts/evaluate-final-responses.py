#!/usr/bin/env python3
"""Evaluate literal final-code claims against files, independently of model claims."""
import argparse
import ast
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


def classify_code_quotes(answer, source, function, literal=True):
    """Classify quote evidence, not semantic truth; mixed historical quotes need review."""
    try:
        parsed_source = ast.parse(source)
    except SyntaxError:
        return "invalid_source"
    actual = next((node for node in parsed_source.body
                   if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
                   and node.name == function), None)
    claims = []
    malformed = False
    blocks = re.findall(r"```(?:python|py)\s*\n(.*?)```", answer, re.S)
    if not literal:
        blocks += re.findall(r"(?<!`)`([^`\n]+)`(?!`)", answer)
    for block in blocks:
        try:
            nodes = ast.parse(block).body
        except SyntaxError:
            malformed = malformed or literal
            continue
        if not literal:
            claims.extend(node for node in nodes if isinstance(node, ast.Return))
        claims.extend(node for node in nodes
                      if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
                      and node.name == function)
    if not claims:
        return "missing_claim" if literal else "not_claimed"
    actual_nodes = [actual] if literal or actual is None else list(ast.walk(actual))
    if malformed or actual is None:
        return "needs_review"
    matches = [any(ast.dump(claim, include_attributes=False) == ast.dump(node, include_attributes=False)
                   for node in actual_nodes) for claim in claims]
    if all(matches):
        return "match"
    if any(matches):
        return "mixed_quotes"
    return "needs_review"


def fixture_files_unchanged(workspace, original_bytes):
    """Check exact bytes for every fixture outside the allowed source file."""
    try:
        return all((workspace / name).read_bytes() == contents for name, contents in original_bytes.items())
    except OSError:
        return False


def build_goal(case, claim_style, goal_suffix):
    """Keep experimental instructions in one line of the line-oriented REPL."""
    goal = (f"이 프로젝트의 테스트를 실행해 실패 원인을 찾고 {case['source']}만 수정해줘. "
            "테스트 파일은 변경하지 마. 수정 뒤 같은 테스트를 다시 실행하고 실제 변경 내용과 테스트 결과를 설명해줘.")
    if claim_style == "literal":
        goal += " 최종 응답에는 실제 파일에 저장된 수정 함수 전체를 python 코드 블록으로 그대로 보여줘."
    if goal_suffix:
        goal += " " + " ".join(goal_suffix.splitlines())
    return goal


def evaluate(binary, case, root, attempt, timeout, claim_style, goal_suffix=""):
    directory = root / f"{case['id']}-{attempt}"
    directory.mkdir(mode=0o700)
    workspace = directory / "workspace"
    workspace.mkdir()
    for name, contents in case["files"].items():
        relative = Path(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("fixture paths must stay within the workspace")
        (workspace / relative).parent.mkdir(parents=True, exist_ok=True)
        (workspace / relative).write_text(contents)
    original_bytes = {name: (workspace / name).read_bytes() for name in case["files"] if name != case["source"]}
    tool_environment = {name: os.environ[name] for name in
        ("PATH", "LANG", "LC_ALL", "TZ", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR")
        if name in os.environ}
    before = subprocess.run([shutil.which("python3"), "-m", "unittest", "-v"],
        cwd=workspace, env=tool_environment, capture_output=True, text=True, timeout=timeout)
    goal = build_goal(case, claim_style, goal_suffix)
    script = ("/permissions model allow\n/permissions read allow\n"
              "/permissions write allow\n/permissions execute allow\n" + goal + "\n/exit\n")
    environment = dict(os.environ)
    # Credentials stay in the existing OS store; tools never inherit ambient API keys.
    for name in list(environment):
        if name.startswith(("DEEPSEEK_", "XGEN_", "XGENY_")):
            del environment[name]
    environment["XGEN_STATE_HOME"] = str(directory / "state")
    environment["XGENY_STATE_HOME"] = str(directory / "state")
    started = time.monotonic()
    result = subprocess.run([str(binary)], input=script, cwd=workspace,
        env=environment, capture_output=True, text=True, timeout=timeout)
    elapsed = time.monotonic() - started
    after = subprocess.run([shutil.which("python3"), "-m", "unittest", "-v"],
        cwd=workspace, env=tool_environment, capture_output=True, text=True, timeout=timeout)
    transcript = result.stdout
    # This evaluator is restricted to synthetic fixtures, never customer workspaces.
    (directory / "transcript.txt").write_text(transcript)
    (directory / "tests.txt").write_text(after.stdout + after.stderr)
    unchanged_tests = fixture_files_unchanged(workspace, original_bytes)
    record = {"case": case["id"], "split": case["split"], "attempt": attempt,
        "claim_style": claim_style, "cli_exit_code": result.returncode,
        "elapsed_seconds": elapsed,
        "goal_suffix_sha256": hashlib.sha256(goal_suffix.encode()).hexdigest(),
        "initial_tests_failed": before.returncode != 0,
        "tests_passed": after.returncode == 0, "tests_unchanged": unchanged_tests,
        "task_completed": False, "quote_status": "missing_claim" if claim_style == "literal" else "not_claimed",
        "model_calls": 0, "tool_effects": 0, "final_content_observed_after_write": None}
    source = (workspace / case["source"]).read_bytes().decode("utf-8")
    databases = list((directory / "state" / "runs").glob("*/run.sqlite3"))
    if len(databases) > 1:
        raise RuntimeError("expected exactly one Run per fixture")
    for database in databases:
        manifest = json.loads((database.parent / "manifest.json").read_text())["record"]
        record["model"] = manifest["model"]
        record["request_profile_digest"] = manifest["requestProfileDigest"]
        with sqlite3.connect(database) as connection:
            state = json.loads(connection.execute("SELECT state_json FROM run_projection").fetchone()[0])
            candidate = (state.get("agentLoop") or {}).get("completionCandidate")
            record["task_completed"] = bool(candidate and candidate.get("responseKind", "task_completion") == "task_completion")
            events = [json.loads(row[0])["body"]["type"] for row in
                connection.execute("SELECT event_json FROM run_events ORDER BY sequence")]
            record["model_calls"] += events.count("model_call_reserved")
            record["tool_effects"] += events.count("effect_succeeded")
            outputs = [json.loads(row[0]) for row in connection.execute(
                "SELECT record_json FROM tool_outputs ORDER BY event_sequence")]
            (directory / "outputs.json").write_text(json.dumps(outputs, ensure_ascii=False, indent=2))
            summaries = [json.loads(row[0])["summary"] for row in connection.execute(
                "SELECT record_json FROM completion_outputs ORDER BY event_sequence")]
            if summaries:
                (directory / "answer.txt").write_text(summaries[-1])
                record["quote_status"] = classify_code_quotes(summaries[-1], source, case["function"], claim_style == "literal")
            last_write = max((index for index, output in enumerate(outputs)
                if output["invocation"]["capabilityId"] in {"xgeny.fs/apply-patch", "xgeny.fs/write-atomic"}
                and output["output"].get("path") == "workspace:primary/" + case["source"]), default=-1)
            record["final_content_observed_after_write"] = None if last_write < 0 else any(
                output["invocation"]["capabilityId"] == "xgeny.fs/read-text"
                and output["output"].get("content") == source for output in outputs[last_write + 1:])
    (directory / "result.json").write_text(json.dumps(record, ensure_ascii=False, indent=2))
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(shutil.which("xgen") or "xgen"))
    parser.add_argument("--cases", type=Path, default=Path(__file__).resolve().parents[1] / "evals/final-responses/cases.json")
    parser.add_argument("--attempts", type=int, default=3)
    parser.add_argument("--split", choices=["design", "held_out", "all"], default="all")
    parser.add_argument("--claim-style", choices=["literal", "narrative"], default="literal")
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    if args.attempts < 1 or args.timeout < 1:
        parser.error("attempts and timeout must be positive")
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("binary must be an executable xgen build")
    binary_digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    root = Path(tempfile.mkdtemp(prefix="xgen-final-response-eval-"))
    records = []
    catalog = args.cases.read_bytes()
    (root / "cases.json").write_bytes(catalog)
    for case in json.loads(catalog):
        if case["source"] not in case["files"] or not case["function"].isidentifier():
            parser.error("source and function must identify a configured fixture")
        if not re.fullmatch(r"[a-zA-Z0-9_-]+", case["id"]):
            parser.error("case IDs must be simple identifiers")
        if args.split != "all" and case["split"] != args.split:
            continue
        for attempt in range(1, args.attempts + 1):
            record = evaluate(binary, case, root, attempt, args.timeout, args.claim_style)
            records.append(record)
            print(json.dumps(record, ensure_ascii=False), flush=True)
    if not records:
        parser.error("no cases selected")
    if hashlib.sha256(binary.read_bytes()).hexdigest() != binary_digest:
        raise RuntimeError("binary changed during evaluation")
    report = {"binary_sha256": binary_digest,
        "catalog_sha256": hashlib.sha256(catalog).hexdigest(), "records": records}
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print("REPORT " + str(root / "report.json"), flush=True)
    return 0 if all(workflow_succeeded(record) for record in records) else 1


def workflow_succeeded(record):
    """Workflow success does not assert semantic truth of the answer."""
    return (record["cli_exit_code"] == 0 and record["initial_tests_failed"]
            and record["tests_passed"] and record["tests_unchanged"] and record["task_completed"])


if __name__ == "__main__":
    raise SystemExit(main())
