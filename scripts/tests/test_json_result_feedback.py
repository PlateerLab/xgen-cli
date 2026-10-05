import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('json_feedback', Path(__file__).resolve().parents[1] / 'check-json-result.py')
M = importlib.util.module_from_spec(spec)
spec.loader.exec_module(M)


class JsonFeedbackTests(unittest.TestCase):
    def run_check(self, expected, content=None, feedback=True):
        with tempfile.TemporaryDirectory() as temp:
            actual = Path(temp) / 'result.json'
            if content is not None:
                actual.write_bytes(content.encode())
            stream = io.StringIO()
            with contextlib.redirect_stderr(stream):
                result = M.verify(expected, actual, feedback=feedback)
            return result, stream.getvalue()

    def test_success_and_type_preservation(self):
        self.assertEqual(self.run_check({'b': [2], 'a': '한'}, '{"a":"한","b":[2]}'), (0, ''))
        self.assertEqual(self.run_check({'n': 1}, '{"n":true}')[0], 1)
        self.assertEqual(self.run_check({'n': 1}, '{"n":1.0}')[0], 1)

    def test_mismatch_diagnostics_and_silent_control_across_two_families(self):
        for expected, actual in [({'sum': 5}, '{"sum":4}'), ({'text': 'é'}, '{"text":"e"}')]:
            code, output = self.run_check(expected, actual)
            diagnostic = json.loads(output)
            self.assertEqual(code, 1)
            self.assertEqual(diagnostic['expected'], expected)
            self.assertEqual(diagnostic['actual'], json.loads(actual))
            self.assertEqual(diagnostic['reason'], 'value_mismatch')
            self.assertEqual(self.run_check(expected, actual, False), (1, ''))

    def test_missing_invalid_and_oversized_result(self):
        for content, reason in [(None, 'missing_result'), ('{', 'invalid_result_json'),
                                ('{"x":1,"x":2}', 'invalid_result_json'),
                                ('{"x":NaN}', 'invalid_result_json'),
                                ('"' + 'a' * M.MAX_INPUT_BYTES + '"', 'invalid_result_json')]:
            code, output = self.run_check({}, content)
            self.assertEqual(code, 1)
            self.assertEqual(json.loads(output)['reason'], reason)

    def test_output_limit_and_escape_sequences(self):
        code, output = self.run_check({'text': '한' * 1000}, '{"text":"x"}')
        self.assertEqual(code, 1)
        self.assertLessEqual(len(output.encode()), M.MAX_DIAGNOSTIC_BYTES)
        self.assertTrue(json.loads(output)['values_omitted'])
        _, output = self.run_check({'text': '\x1b[2J'}, '{"text":"x"}')
        self.assertNotIn('\x1b', output)
        self.assertEqual(json.loads(output)['expected']['text'], '\x1b[2J')


if __name__ == '__main__':
    unittest.main()
