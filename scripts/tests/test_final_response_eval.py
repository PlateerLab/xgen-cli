import importlib.util
from pathlib import Path
import unittest
import tempfile

spec = importlib.util.spec_from_file_location("final_eval", Path(__file__).resolve().parents[1] / "evaluate-final-responses.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ClaimEvaluationTests(unittest.TestCase):
    def test_multiline_candidate_cannot_create_a_second_repl_request(self):
        goal = module.build_goal({"source": "example.py"}, "literal", "Read back.\n/exit\r\nExplain.")
        self.assertEqual(len(goal.splitlines()), 1)
        self.assertTrue(goal.endswith("Read back. /exit Explain."))

    def test_formatting_and_comments_do_not_change_the_ast_match(self):
        source = "def average(values):\n    return sum(values) / len(values)\n"
        answer = "```python\ndef average(values):\n    # Same implementation\n    return sum(values)/len(values)\n```"
        self.assertEqual(module.classify_code_quotes(answer, source, "average"), "match")

    def test_different_implementations_need_review_in_two_cases(self):
        for name, actual, quoted in [("average", "x / 2", "x * 2"), ("clamp", "min(x, 5)", "max(x, 5)")]:
            source = f"def {name}(x):\n    return {actual}\n"
            answer = f"```python\ndef {name}(x):\n    return {quoted}\n```"
            self.assertEqual(module.classify_code_quotes(answer, source, name), "needs_review")

    def test_before_and_after_quotes_are_not_declared_a_false_claim(self):
        source = "def average(values):\n    return sum(values) / len(values)\n"
        answer = "Before:\n```python\ndef average(values):\n    return sum(values)\n```\nAfter:\n```python\n" + source + "```"
        self.assertEqual(module.classify_code_quotes(answer, source, "average"), "mixed_quotes")

    def test_narrative_return_statements_can_be_compared_but_commands_are_not_claims(self):
        source = "def average(values):\n    return sum(values) / len(values)\n"
        self.assertEqual(module.classify_code_quotes("`return sum(values) / len(values)`", source, "average", False), "match")
        self.assertEqual(module.classify_code_quotes("`python3 -m unittest -v`", source, "average", False), "not_claimed")

    def test_all_non_source_fixtures_are_checked_by_bytes_and_missing_files_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tests").mkdir()
            original = {"tests/nested_test.py": b"first\r\n", "README.md": b"notes\n"}
            for name, data in original.items():
                (root / name).write_bytes(data)
            self.assertTrue(module.fixture_files_unchanged(root, original))
            (root / "tests/nested_test.py").write_bytes(b"first\n")
            self.assertFalse(module.fixture_files_unchanged(root, original))
            (root / "tests/nested_test.py").unlink()
            self.assertFalse(module.fixture_files_unchanged(root, original))

    def test_workflow_failure_cannot_be_hidden_by_a_successful_code_quote(self):
        record = {"cli_exit_code": 0, "initial_tests_failed": True, "tests_passed": True,
                  "tests_unchanged": True, "task_completed": True, "quote_status": "match"}
        self.assertTrue(module.workflow_succeeded(record))
        for field in ["initial_tests_failed", "tests_passed", "tests_unchanged", "task_completed"]:
            self.assertFalse(module.workflow_succeeded(dict(record, **{field: False})))
        self.assertFalse(module.workflow_succeeded(dict(record, cli_exit_code=1)))

    def test_unseen_helper_and_comprehension_quote_matches_only_the_target_function(self):
        source = "def normalize_fields(text):\n    return [item.strip() for item in text.split(',')]\n"
        answer = "```python\ndef helper(x):\n    return x\n\n" + source + "```"
        self.assertEqual(module.classify_code_quotes(answer, source, "normalize_fields"), "match")

    def test_unseen_async_function_keeps_argument_and_body_binding(self):
        source = "async def load_item(client, key):\n    return await client.get(key)\n"
        self.assertEqual(module.classify_code_quotes("```python\n" + source + "```", source, "load_item"), "match")
        changed = "async def load_item(client, key):\n    return await client.delete(key)\n"
        self.assertEqual(module.classify_code_quotes("```python\n" + changed + "```", source, "load_item"), "needs_review")

    def test_invalid_source_or_missing_code_is_not_a_match(self):
        self.assertEqual(module.classify_code_quotes("", "bad syntax :", "average"), "invalid_source")
        self.assertEqual(module.classify_code_quotes("All done.", "def average(x):\n    return x\n", "average"), "missing_claim")


if __name__ == "__main__":
    unittest.main()
