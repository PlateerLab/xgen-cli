import importlib.util
import json
from pathlib import Path
import sys
import unittest

spec = importlib.util.spec_from_file_location("planning_probe", Path(__file__).resolve().parents[1] / "evaluate-planning-boundary.py")
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class PlanningBoundaryEvaluatorTests(unittest.TestCase):
    def test_false_completion_is_rejected_on_distinct_output_types(self):
        for value in ({"structured": [1, False]}, "bounded text"):
            expected = [{"operation": "one", "resource": "probe:one", "value": value}]
            result = {"applied": [], "duplicate_effects": 0, "completed": True}
            oracle, errors = probe.check_safety(result, expected)
            self.assertFalse(oracle)
            self.assertIn("false_completion", errors)
            result["completed"] = False
            self.assertEqual(probe.check_safety(result, expected), (False, []))

    def test_duplicate_is_detected_even_when_sink_key_counter_lies(self):
        output = {"operation": "one", "resource": "probe:one", "value": [1, 2]}
        result = {"applied": [output, output], "duplicate_effects": 0, "completed": True}
        _, errors = probe.check_safety(result, [output])
        self.assertIn("duplicate_logical_operation", errors)
        self.assertIn("false_completion", errors)

    def test_unexpected_effect_is_rejected_without_completion(self):
        result = {"applied": [{"operation": "unexpected"}], "duplicate_effects": 0, "completed": False}
        self.assertIn("unexpected_effect", probe.check_safety(result, [])[1])

    def test_malformed_effect_is_recorded_instead_of_crashing(self):
        for actual in ({"unexpected": True}, "not an object", {"operation": [1]}):
            result = {"applied": [actual], "duplicate_effects": 0, "completed": False}
            errors = probe.check_safety(result, [])[1]
            self.assertIn("malformed_effect", errors)
            self.assertIn("unexpected_effect", errors)

    def test_wrong_boundary_is_not_accepted_as_process_crash(self):
        marker = json.dumps({"event": "rendezvous", "boundary": "wrong", "run_id": "planning-boundary-probe"})
        command = [sys.executable, "-c", f"print({marker!r}, flush=True); input()"]
        with self.assertRaisesRegex(RuntimeError, "wrong fault rendezvous"):
            probe.crash(command, "plan_committed")

    def test_extra_stdout_is_not_accepted_as_report(self):
        command = [sys.executable, "-c", "print('{}'); print('{}')"]
        with self.assertRaisesRegex(RuntimeError, "unexpected report"):
            probe.report(command)


if __name__ == "__main__":
    unittest.main()
