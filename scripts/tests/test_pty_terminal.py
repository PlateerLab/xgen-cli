import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("pty_smoke", Path(__file__).resolve().parents[1] / "smoke-pty-terminal.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class PtyEvaluationTests(unittest.TestCase):
    def test_claim_matches_observation_without_boolean_numeric_coercion(self):
        for output in ("English\r\n", "한국어\r\n"):
            observation = {"state": "exited", "exitCode": 0, "output": output}
            self.assertTrue(smoke.validate_claim(json.dumps(observation), observation))
            for wrong in (dict(observation, exitCode=False), dict(observation, exitCode="0"), dict(observation, output="invented"), dict(observation, state="running"), dict(observation, extra=True)):
                self.assertFalse(smoke.validate_claim(json.dumps(wrong), observation))
        with self.assertRaises(ValueError):
            smoke.validate_claim('```json\n{}\n```', observation)

    def test_fixture_uses_exact_input_and_retains_nonzero_exit(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            program = root / "fixture.py"
            program.write_text(smoke.FIXTURE)
            for text in ("English reply", "한국어 응답"):
                result = subprocess.run([sys.executable, str(program), "input"], input=text + "\n", cwd=root, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0)
                self.assertIn("REPLY:" + text, result.stdout)
            result = subprocess.run([sys.executable, str(program), "failure"], cwd=root, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 7)
            self.assertIn("OBSERVED_FAILURE", result.stdout)
            self.assertEqual((root / "starts").read_text(), "1\n1\n1\n")


if __name__ == "__main__":
    unittest.main()
