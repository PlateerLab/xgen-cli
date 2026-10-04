import importlib.util
from pathlib import Path
import unittest
import tempfile
import json

SPEC = importlib.util.spec_from_file_location("rrsi_experiment", Path(__file__).resolve().parents[1] / "experiment-rrsi.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def records():
    return [{"case": case, "split": split, "variant": variant,
             "cli_exit_code": 0, "initial_tests_failed": True, "tests_passed": True,
             "tests_unchanged": True, "task_completed": True, "quote_status": "match",
             "model_calls": 4, "tool_effects": 5, "elapsed_seconds": 1,
             "model": "fixed-model", "request_profile_digest": "fixed-profile"}
            for split in ("design", "held_out") for case in (split + "-a", split + "-b")
            for variant in ("baseline", "readback")]


class AdoptionGateTests(unittest.TestCase):
    def decision(self, rows):
        return MODULE.gate(rows, MODULE.summarize(rows))

    def promising_rows(self):
        rows = records()
        for row in rows:
            if row["split"] == "design" and row["variant"] == "baseline":
                row["quote_status"] = "missing_claim"
        return rows

    def test_ceiling_success_is_not_claimed_as_improvement(self):
        self.assertEqual(self.decision(records()), "do_not_adopt_failure_not_reproduced_in_two_cases")

    def test_two_failure_cases_with_transfer_are_only_promising(self):
        self.assertEqual(self.decision(self.promising_rows()), "promising_requires_larger_validation")

    def test_held_out_failure_blocks_a_design_gain(self):
        rows = self.promising_rows()
        rows[-1]["tests_passed"] = False
        self.assertEqual(self.decision(rows), "do_not_adopt_workflow_regression")

    def test_expensive_candidate_and_changed_profile_cannot_be_promoted(self):
        rows = self.promising_rows()
        for row in rows:
            if row["variant"] == "readback":
                row["model_calls"] = 6
        self.assertEqual(self.decision(rows), "do_not_adopt_call_budget")
        rows = self.promising_rows()
        rows[-1]["request_profile_digest"] = "changed"
        self.assertEqual(self.decision(rows), "inconclusive_model_profile")

    def test_before_after_quotes_require_review_before_selection(self):
        rows = self.promising_rows()
        rows[-1]["quote_status"] = "mixed_quotes"
        self.assertEqual(self.decision(rows), "requires_quote_review")

    def test_empty_results_and_failed_partial_trials_are_preserved(self):
        self.assertEqual(self.decision([]), "inconclusive_incomplete_experiment")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            MODULE.save_failure(root, {"hashes": {}}, [], {"attempt": 1}, "TimeoutExpired")
            report = json.loads((root / "report.json").read_text())
            self.assertEqual(report["decision"], "inconclusive_interrupted_experiment")
            self.assertEqual(report["error"], "TimeoutExpired")
            self.assertEqual(report["records"], [])

    def test_workflow_regression_rejects_even_with_unreviewed_quotes(self):
        for split in ("design", "held_out"):
            rows = self.promising_rows()
            rows[0]["quote_status"] = "mixed_quotes"
            row = next(r for r in rows if r["split"] == split and r["variant"] == "readback")
            row["task_completed"] = False
            self.assertEqual(self.decision(rows), "do_not_adopt_workflow_regression")


if __name__ == "__main__":
    unittest.main()
