#!/usr/bin/env python3
"""Model-free, real-process planning/recovery contract evaluation."""
import argparse
import hashlib
import json
import platform
import queue
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCENARIOS = {
    "normal": None, "adapter_failed": None, "plan_committed": "plan_committed",
    "sink_no_query": "sink_applied", "sink_advertised_query": "sink_applied",
    "before_receipt": "before_receipt", "model_reserved": "model_reserved",
    "catalog_drift": "authorized", "material_drift": "authorized",
    "profile_drift": "authorized",
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def report(command):
    process = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if process.returncode:
        raise RuntimeError(f"probe exited {process.returncode}: {process.stderr[:500]}")
    messages = [json.loads(line) for line in process.stdout.splitlines()]
    if len(messages) != 1 or messages[0].get("event") != "report":
        raise RuntimeError("missing or unexpected report")
    return messages[0]


def crash(command, boundary):
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True)
    messages = queue.Queue()
    thread = threading.Thread(target=lambda: messages.put(process.stdout.readline()), daemon=True)
    thread.start()
    try:
        raw = messages.get(timeout=30)
        marker = json.loads(raw)
        if marker != {"event": "rendezvous", "boundary": boundary,
                      "run_id": "planning-boundary-probe"}:
            raise RuntimeError("wrong fault rendezvous")
        process.kill()
        process.communicate(timeout=10)
        if process.returncode == 0:
            raise RuntimeError("fault child exited successfully")
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=10)
        process.stdout.close()
        process.stderr.close()
        process.stdin.close()
        thread.join(timeout=1)


def check_safety(result, expected):
    actual = result["applied"]
    allowed = {canonical(item) for item in expected}
    errors = []
    if result["duplicate_effects"]:
        errors.append("duplicate_effect")
    if any(canonical(item) not in allowed for item in actual):
        errors.append("unexpected_effect")
    malformed = any(not isinstance(item, dict) or not isinstance(item.get("operation"), str) for item in actual)
    if malformed:
        errors.append("malformed_effect")
    keys = [item["operation"] for item in actual if isinstance(item, dict) and isinstance(item.get("operation"), str)]
    if len(keys) != len(set(keys)):
        errors.append("duplicate_logical_operation")
    oracle = sorted(map(canonical, actual)) == sorted(map(canonical, expected))
    if result["completed"] and not oracle:
        errors.append("false_completion")
    return oracle, errors


def trial(binary, fixture_path, packet, scenario, repetition):
    fixture = json.loads(fixture_path.read_text())
    begin = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="xgen-planning-probe-") as directory:
        command = [str(binary), "--root", directory, "--fixture", str(fixture_path),
                   "--packet-size", str(packet)]
        if scenario == "sink_advertised_query":
            command += ["--query"]
        if scenario == "adapter_failed":
            command += ["--fail-operation", fixture["plan"]["initial"][0]["key"]]
        boundary = SCENARIOS[scenario]
        if boundary:
            crash(command + ["--fault", boundary], boundary)
        if scenario == "catalog_drift":
            command += ["--catalog-drift"]
        elif scenario == "material_drift":
            with sqlite3.connect(Path(directory) / "sink.sqlite3") as db:
                # Mutate all committed recipes, not just the first lexically ordered Step.
                for key, raw in db.execute("SELECT id,args FROM recipes").fetchall():
                    args = json.loads(raw)
                    args["resource"] = "probe:changed"
                    db.execute("UPDATE recipes SET args=? WHERE id=?", (canonical(args), key))
        elif scenario == "profile_drift":
            command[command.index("--packet-size") + 1] = str(4 if packet == 1 else 1)
        first = report(command)
        interventions = 0
        pre_intervention = None
        errors = []
        if scenario == "model_reserved":
            if first["status"] != "model_call_recovery_required" or first["applied"]:
                errors.append("model_call_blind_retry")
            pre_intervention = {"status": first["status"], "calls": first["calls"], "model_calls": first["state"]["agentLoop"]["modelCalls"]}
            if first["calls"].get("planner") != 1 or pre_intervention["model_calls"]["reservedCalls"] != 1:
                errors.append("unexpected_pre_abandon_model_count")
            first = report(command + ["--discard-model-call"])
            interventions = 1
        oracle, safety_errors = check_safety(first, fixture["oracle"]["applied"])
        errors += safety_errors
        order = {item.get("operation"): i for i, item in enumerate(first["applied"]) if isinstance(item, dict) and isinstance(item.get("operation"), str)}
        for operation, parents in fixture["oracle"]["dependencies"].items():
            if operation in order and any(parent not in order or order[parent] >= order[operation] for parent in parents):
                errors.append("dependency_order_violation")
        successful = scenario in {"normal", "plan_committed", "before_receipt", "model_reserved"}
        if successful and not (first["completed"] and oracle):
            errors.append("recovery_incomplete")
        if scenario.endswith("drift") and (first["applied"] or first["completed"]):
            errors.append("changed_contract_executed")
        if scenario in {"sink_advertised_query", "sink_no_query"} and first["completed"]:
            errors.append("unexpected_unknown_effect_completion")
        replay = report(command)
        if replay["applied"] != first["applied"] or replay["calls"].get("planner", 0) != first["calls"].get("planner", 0):
            errors.append("replay_reexecuted")
        if replay["completed"] != first["completed"]:
            errors.append("replay_completion_changed")
        if successful:
            expected_calls = fixture["oracle"]["planner_calls"][str(packet)] + int(scenario == "model_reserved")
            if first["calls"].get("planner") != expected_calls:
                errors.append("planner_count_mismatch")
            if first["receipts"] != len(fixture["oracle"]["applied"]):
                errors.append("receipt_count_mismatch")
        if scenario in {"sink_advertised_query", "sink_no_query"}:
            if first["calls"].get("reconcile", 0) != 0:
                errors.append("unexpected_query_on_none_guarantee")
            if "manual_required" not in [s["status"] for s in first["state"]["steps"].values()]:
                errors.append("unknown_not_marked_manual")
        if scenario == "model_reserved":
            model_state = first["state"]["agentLoop"]["modelCalls"]
            if model_state["reservedCalls"] != expected_calls or model_state["unknownCalls"] != 1:
                errors.append("model_budget_refunded")
        states = first["state"]["steps"]
        # A child of a failed/manual parent must never start.
        for step in states.values():
            dependencies = step.get("dependsOn", [])
            if any(states[parent]["status"] in {"failed", "manual_required"} for parent in dependencies):
                if step.get("attempts", 0):
                    errors.append("blocked_descendant_executed")
        if scenario == "adapter_failed":
            if first["completed"] or "failed" not in [s["status"] for s in states.values()]:
                errors.append("failed_effect_not_blocked")
            if packet == 4:
                failed = fixture["plan"]["initial"][0]["key"]
                independent = [op["key"] for op in fixture["plan"]["initial"][1:packet] if not op["depends_on"]]
                actual_keys = {op["operation"] for op in first["applied"]}
                if failed in actual_keys or not set(independent).issubset(actual_keys):
                    errors.append("independent_sibling_not_preserved")
        model = first["state"].get("agentLoop", {}).get("modelCalls", {})
        unsafe = {"dependency_order_violation", "malformed_effect", "duplicate_effect", "unexpected_effect", "duplicate_logical_operation", "false_completion", "model_call_blind_retry", "changed_contract_executed", "replay_reexecuted", "blocked_descendant_executed", "model_budget_refunded"}
        return {"trial_id": f"{fixture['id']}-{packet}-{scenario}-{repetition}", "fixture": fixture["id"], "split": fixture["split"], "packet_size": packet,
                "scenario": scenario, "repetition": repetition, "boundary": boundary,
                "status": first["status"], "completed": first["completed"],
                "oracle_pass": oracle, "contract_pass": not errors, "safety_pass": not any(e in unsafe for e in errors), "errors": errors,
                "applied_effects": len(first["applied"]), "duplicate_effects": first["duplicate_effects"],
                "attempts": first["calls"].get("execute", 0), "calls": first["calls"], "model_calls": model,
                "receipts": first["receipts"], "journal_events": first["journal_events"],
                "step_statuses": [s["status"] for s in states.values()],
                "pre_intervention": pre_intervention, "manual_interventions": interventions, "latency_ms": round((time.monotonic()-begin)*1000, 3)}


def evaluate(args, binary):
    fixtures = sorted((ROOT / "evals/planning-boundary/fixtures").glob("*.json"))
    fixtures = [f for f in fixtures if args.split == "all" or json.loads(f.read_text())["split"] == args.split]
    if len(fixtures) < 2:
        raise ValueError("at least two fixtures are required")
    tracked = subprocess.check_output(["git", "ls-files", "crates", "Cargo.toml", "Cargo.lock"], cwd=ROOT, text=True).splitlines()
    sources = sorted({Path(__file__), *[(ROOT / name) for name in tracked if name.endswith(".rs") or Path(name).name in {"Cargo.toml", "Cargo.lock"}], *list((ROOT / "crates/xgen-cli/examples/planning_boundary_probe").glob("*.rs"))})
    manifest = {"kind": "XS_OFFLINE_CONTRACT", "repeats": args.repeats, "split": args.split,
                "binary_sha256": sha(binary), "fixtures": {f.name: sha(f) for f in fixtures},
                "sources": {str(f.relative_to(ROOT)): sha(f) for f in sources},
                "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                "environment": {"os": platform.system(), "arch": platform.machine(), "python": platform.python_version()},
                "engine": "xgen-cli 0.1.0-rc.3 / real RunDriver / scripted planner",
                "query_contract": "advertised feature; admission pins SinkGuarantee::None, query reconciliation NOT_IMPLEMENTED for planned admission",
                "scenarios": SCENARIOS, "packet_sizes": [1, 4],
                "model_tokens": "NOT_MEASURED", "model_cost": "NOT_MEASURED", "external_engines": "NOT_RUN"}
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2)+"\n")
    trials = []
    with (args.output / "trials.jsonl").open("w") as stream:
        for fixture in fixtures:
            for packet in (1, 4):
                for scenario in SCENARIOS:
                    for repetition in range(args.repeats):
                        try:
                            item = trial(binary, fixture, packet, scenario, repetition)
                        except (RuntimeError, ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError, queue.Empty) as error:
                            item = {"trial_id": f"{fixture.stem}-{packet}-{scenario}-{repetition}", "fixture": fixture.stem,
                                    "split": json.loads(fixture.read_text())["split"], "packet_size": packet,
                                    "scenario": scenario, "repetition": repetition, "contract_pass": False,
                                    "safety_pass": False, "completed": False, "status": "probe_error",
                                    "errors": ["probe_exception:" + type(error).__name__]}

                        trials.append(item)
                        stream.write(json.dumps(item, sort_keys=True)+"\n")
                        stream.flush()
            print(f"{fixture.stem}: {len(trials)} trials, failures={sum(not t['contract_pass'] for t in trials)}", flush=True)
    if any(sha(path) != manifest["fixtures"][path.name] for path in fixtures) or sha(binary) != manifest["binary_sha256"] or any(sha(path) != manifest["sources"][str(path.relative_to(ROOT))] for path in sources):
        raise RuntimeError("frozen inputs changed during evaluation")
    summary = {"trials": len(trials), "passed": sum(t["contract_pass"] for t in trials),
               "completed": sum(t["completed"] for t in trials),
               "failed_trials": [t for t in trials if not t["contract_pass"]],
               "interpretation": "Contract safety is distinct from automatic recovery completion; no model performance claim."}
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2)+"\n")
    print(json.dumps(summary, sort_keys=True))
    raise SystemExit(bool(summary["failed_trials"]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=10)
    parser.add_argument("--split", choices=["design", "validation", "all"], default="all")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("repeats must be positive")
    with tempfile.TemporaryDirectory(prefix="xgen-planning-binary-") as directory:
        binary = Path(directory) / args.binary.name
        before = sha(args.binary)
        shutil.copy2(args.binary, binary)
        if sha(binary) != before or sha(args.binary) != before:
            raise RuntimeError("binary changed while copying")
        evaluate(args, binary)


if __name__ == "__main__":
    main()
