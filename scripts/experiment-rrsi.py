#!/usr/bin/env python3
"""Run one frozen task-prompt candidate against a counterbalanced baseline."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import statistics
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("final_eval", HERE / "evaluate-final-responses.py")
EVAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVAL)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def quality(record):
    # Mixed historical quotes remain review candidates, not proven false claims.
    return EVAL.workflow_succeeded(record) and record["quote_status"] == "match"


def summarize(records):
    result = {}
    for split in ("design", "held_out"):
        result[split] = {}
        for variant in ("baseline", "readback"):
            group = [r for r in records if r["split"] == split and r["variant"] == variant]
            result[split][variant] = {
                "trials": len(group),
                "workflow_successes": sum(EVAL.workflow_succeeded(r) for r in group),
                "exact_quote_successes": sum(quality(r) for r in group),
                "mean_model_calls": statistics.mean(r["model_calls"] for r in group) if group else None,
                "mean_tool_effects": statistics.mean(r["tool_effects"] for r in group) if group else None,
                "mean_elapsed_seconds": statistics.mean(r["elapsed_seconds"] for r in group) if group else None,
                "quote_statuses": {status: sum(r["quote_status"] == status for r in group)
                                   for status in sorted({r["quote_status"] for r in group})},
            }
    return result


def gate(records, summary):
    if not records or any(summary[s][v]["trials"] == 0
                          for s in ("design", "held_out") for v in ("baseline", "readback")):
        return "inconclusive_incomplete_experiment"
    profiles = {(r.get("model"), r.get("request_profile_digest")) for r in records}
    if len(profiles) != 1 or None in next(iter(profiles)):
        return "inconclusive_model_profile"
    # A workflow regression suffices to reject even if some quotes need review.
    if any(summary[s]["readback"]["workflow_successes"] < summary[s]["baseline"]["workflow_successes"]
           for s in ("design", "held_out")):
        return "do_not_adopt_workflow_regression"
    if any(r["quote_status"] in ("mixed_quotes", "needs_review") for r in records):
        return "requires_quote_review"
    failures = {r["case"] for r in records if r["split"] == "design"
                and r["variant"] == "baseline" and not quality(r)}
    if len(failures) < 2:
        return "do_not_adopt_failure_not_reproduced_in_two_cases"
    for split in ("design", "held_out"):
        base, candidate = (summary[split][v] for v in ("baseline", "readback"))
        if candidate["workflow_successes"] < base["workflow_successes"]:
            return "do_not_adopt_workflow_regression"
        if candidate["exact_quote_successes"] < base["exact_quote_successes"]:
            return "do_not_adopt_quote_regression"
        if candidate["mean_model_calls"] > base["mean_model_calls"] * 1.25:
            return "do_not_adopt_call_budget"
    if summary["design"]["readback"]["exact_quote_successes"] <= summary["design"]["baseline"]["exact_quote_successes"]:
        return "do_not_adopt_no_design_gain"
    return "promising_requires_larger_validation"


def save_failure(root, registration, records, trial, error):
    """Preserve partial evidence without retrying a possibly sent request."""
    report = {"registration": registration, "decision": "inconclusive_interrupted_experiment",
              "records": records, "failed_trial": trial, "error": error}
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    with (root / "ledger.jsonl").open("a") as ledger:
        ledger.write(json.dumps({"failed_trial": trial, "error": error}, ensure_ascii=False) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--cases", type=Path, default=HERE.parent / "evals/rrsi/cases.json")
    parser.add_argument("--candidate", type=Path, default=HERE.parent / "evals/rrsi/readback.txt")
    parser.add_argument("--attempts", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    if args.attempts < 2 or args.timeout < 1:
        parser.error("at least two attempts and a positive timeout are required")
    binary = args.binary.resolve()
    cases = json.loads(args.cases.read_text())
    suffix = args.candidate.read_text().strip()
    if not suffix:
        parser.error("candidate must be nonempty")
    for split in ("design", "held_out"):
        if len([c for c in cases if c["split"] == split]) < 2:
            parser.error("each split needs at least two cases")
    root = Path(tempfile.mkdtemp(prefix="xgen-rrsi-pilot-"))
    sources = {"binary": binary, "cases": args.cases, "candidate": args.candidate,
               "evaluator": HERE / "evaluate-final-responses.py", "runner": Path(__file__)}
    hashes = {name: digest(path) for name, path in sources.items()}
    registration = {
        "method": "RRSI-inspired one-candidate task-prompt pilot; not paper replication",
        "hypothesis": "Reading final files improves literal final-code evidence at bounded call cost",
        "hashes": hashes, "attempts": args.attempts,
        "candidate": suffix, "claim_style": "literal",
        "order": "design then held_out; variant order alternates by case and attempt",
        "gate": "two distinct design failures; quote gain; no workflow/held-out regression; <=25% extra calls",
        "limits": ["small sample, no statistical significance claim", "tokens and monetary cost unavailable",
                   "no semantic truth certification", "no production harness edits", "provider randomness uncontrolled"],
    }
    snapshot = root / "source-snapshot"
    snapshot.mkdir()
    for name in ("evaluator", "runner"):
        (snapshot / sources[name].name).write_bytes(sources[name].read_bytes())
    (root / "registration.json").write_text(json.dumps(registration, ensure_ascii=False, indent=2))
    (root / "cases.json").write_bytes(args.cases.read_bytes())
    print("EXPERIMENT " + str(root), flush=True)
    records = []
    for split in ("design", "held_out"):
        for index, case in enumerate(c for c in cases if c["split"] == split):
            for attempt in range(1, args.attempts + 1):
                variants = ["baseline", "readback"]
                if (index + attempt) % 2 == 0:
                    variants.reverse()
                for variant in variants:
                    parent = root / split / variant
                    parent.mkdir(parents=True, exist_ok=True)
                    try:
                        record = EVAL.evaluate(binary, case, parent, attempt, args.timeout, "literal",
                                               suffix if variant == "readback" else "")
                    except (subprocess.TimeoutExpired, RuntimeError, OSError) as error:
                        # A possibly sent request is never blindly retried.
                        trial = {"split": split, "case": case["id"], "variant": variant, "attempt": attempt}
                        save_failure(root, registration, records, trial, type(error).__name__)
                        print("INTERRUPTED " + str(root / "report.json"), flush=True)
                        return 1
                    record["variant"] = variant
                    records.append(record)
                    with (root / "ledger.jsonl").open("a") as ledger:
                        ledger.write(json.dumps(record, ensure_ascii=False) + "\n")
                    print(json.dumps(record, ensure_ascii=False), flush=True)
        (root / f"{split}-records.json").write_text(json.dumps(
            [r for r in records if r["split"] == split], ensure_ascii=False, indent=2))
    if any(digest(path) != hashes[name] for name, path in sources.items()):
        raise RuntimeError("an experiment input changed during evaluation")
    summary = summarize(records)
    report = {"registration": registration, "summary": summary, "decision": gate(records, summary),
              "records": records}
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(summary, ensure_ascii=False), flush=True)
    print("DECISION " + report["decision"], flush=True)
    print("REPORT " + str(root / "report.json"), flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
