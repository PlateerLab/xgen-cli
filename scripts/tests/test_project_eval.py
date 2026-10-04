import importlib.util
from pathlib import Path
import tempfile
import json
import unittest

SPEC = importlib.util.spec_from_file_location('project_eval', Path(__file__).parents[1] / 'evaluate-projects.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProjectEvaluationTests(unittest.TestCase):
    def test_hidden_validation_paths_cannot_escape_or_overwrite_visible_tests(self):
        cases = json.loads((Path(__file__).parents[2] / 'evals/projects/cases.json').read_text())
        MODULE.validate_cases(cases)
        for hidden in ['../outside', cases[0]['source']]:
            candidate = dict(cases[0], hidden_path=hidden)
            with self.assertRaises(ValueError):
                MODULE.validate_cases([candidate])

    def test_literal_quotes_do_not_treat_equivalent_rewrites_as_exact_files(self):
        for actual, rewritten, language in [
            ('if remainder != 0 {}', 'if remainder > 0 {}', 'rust'),
            ('return list(zip(*rows))', 'return [list(c) for c in zip(*rows)]', 'python'),
        ]:
            self.assertTrue(MODULE.quote_matches(f'```{language}\n{actual}\n```', actual))
            self.assertFalse(MODULE.quote_matches(f'```{language}\n{rewritten}\n```', actual))

    def test_missing_or_changed_original_tests_fail_preservation(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original = {'tests.txt': b'original'}
            self.assertFalse(MODULE.preserved(root, original))
            (root / 'tests.txt').write_bytes(b'original')
            self.assertTrue(MODULE.preserved(root, original))
            (root / 'tests.txt').write_bytes(b'weakened')
            self.assertFalse(MODULE.preserved(root, original))

    def test_real_unknown_events_and_repeated_effect_starts_are_counted(self):
        events = [
            {'type': 'model_call_reserved'},
            {'type': 'model_call_became_unknown'},
            {'type': 'effect_execution_started', 'effectId': 'a'},
            {'type': 'effect_execution_started', 'effectId': 'a'},
            {'type': 'effect_became_unknown', 'effectId': 'a'},
            {'type': 'effect_succeeded'},
        ]
        self.assertEqual(MODULE.safety_metrics(events), {
            'model_calls': 1, 'tool_effects': 1, 'unknown_events': 2, 'duplicate_effect_starts': 1,
        })
