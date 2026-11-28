import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import textwrap
import unittest

spec = importlib.util.spec_from_file_location(
    'model_request_diff', Path(__file__).resolve().parents[1] / 'diff-model-requests.py')
M = importlib.util.module_from_spec(spec)
spec.loader.exec_module(M)

# A stand-in xgen: posts one request per scripted reply with a random run id, like the real
# host. VARIANT changes what it sends so the tests can see each kind of difference.
FAKE_XGEN = textwrap.dedent('''\
    #!{python}
    import json, os, secrets, sys, urllib.request
    variant = {variant!r}
    run_id = "run-" + secrets.token_hex(16)
    goal = "Read README.md" + ("\\n\\nExtra instruction." if variant == "goal" else "")
    for call in range(2):
        context = {{"runId": run_id, "goal": goal, "call": call}}
        if variant == "dropkey":
            context = {{"runId": run_id, "call": call}}
        if variant == "order":
            context = {{"goal": goal, "runId": run_id, "call": call}}
        content = json.dumps({{"profileVersion": "v1", "planningContext": context}})
        body = json.dumps({{"model": "m", "messages": [{{"role": "system", "content": "fixed"}},
                                                       {{"role": "user", "content": content}}]}})
        request = urllib.request.Request(os.environ["XGEN_OPENAI_BASE_URL"] + "/chat/completions",
                                         data=body.encode(), headers={{"Content-Type": "application/json"}})
        urllib.request.urlopen(request, timeout=10).read()
    ''')

CASES = {'formatVersion': 1, 'cases': [
    {'name': 'one', 'args': [], 'files': {'README.md': 'x\n'}, 'stdin': '',
     'replies': [{'completion': 'a'}, {'completion': 'b'}]},
    {'name': 'two', 'args': [], 'files': {}, 'stdin': '', 'replies': [{'response': 'c'}, {'response': 'd'}]},
]}


# A stand-in for a Run and its resume in a second process: the first session announces its
# Run on stderr, and "resume RUN" continues it.
RESUMABLE_XGEN = textwrap.dedent('''\
    #!{python}
    import json, os, secrets, sys, urllib.request
    if sys.argv[1:2] == ["resume"]:
        run_id = sys.argv[2]
    else:
        run_id = "run-" + secrets.token_hex(16)
        print("XGEN_STARTED run_id=" + run_id, file=sys.stderr)
    body = json.dumps({{"messages": [{{"role": "user", "content": json.dumps({{"runId": run_id}})}}]}})
    request = urllib.request.Request(os.environ["XGEN_OPENAI_BASE_URL"] + "/chat/completions",
                                     data=body.encode(), headers={{"Content-Type": "application/json"}})
    urllib.request.urlopen(request, timeout=10).read()
    sys.exit(10 if sys.argv[1:2] != ["resume"] else 0)
    ''')


class ModelRequestDiffTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.cases = self.root / 'cases.json'
        self.cases.write_text(json.dumps(CASES))

    def tearDown(self):
        self.temp.cleanup()

    def fake(self, variant):
        path = self.root / f'xgen-{variant}'
        path.write_text(FAKE_XGEN.format(python=sys.executable, variant=variant))
        os.chmod(path, 0o755)
        return path

    def run_main(self, base, head):
        report = self.root / 'report.json'
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = M.main(['--base', str(self.fake(base)), '--head', str(self.fake(head)),
                           '--cases', str(self.cases), '--json', str(report)])
        return code, json.loads(report.read_text()), out.getvalue()

    @unittest.skipIf(os.name == 'nt', 'the stand-in xgen is a shebang script')
    def test_random_ids_are_noise_and_the_same_build_matches(self):
        code, report, out = self.run_main('same', 'same')
        self.assertEqual(code, 0, out)
        self.assertIn('model requests match', out)
        self.assertTrue(all(case['noiseFields'] >= 1 for case in report['cases']))

    @unittest.skipIf(os.name == 'nt', 'the stand-in xgen is a shebang script')
    def test_a_changed_field_is_grouped_by_fingerprint_across_cases_and_calls(self):
        code, report, out = self.run_main('same', 'goal')
        self.assertEqual(code, 1)
        self.assertEqual(len(report['differences']), 4, 'two cases x two calls')
        [group] = report['groups']
        self.assertEqual(group['count'], 4)
        self.assertEqual(group['fingerprint'],
                         '/messages/*/content~json/planningContext/goal | changed | +20 chars')
        self.assertEqual(group['example']['offset'], len('Read README.md'))
        self.assertEqual({prefix['call'] for prefix in report['prefixes']}, {0, 1})

    @unittest.skipIf(os.name == 'nt', 'the stand-in xgen is a shebang script')
    def test_key_order_is_a_difference_even_when_values_match(self):
        code, report, _ = self.run_main('same', 'order')
        self.assertEqual(code, 1)
        [group] = report['groups']
        self.assertTrue(group['fingerprint'].endswith('planningContext#keys | key order'))
        self.assertEqual(group['example']['base'][0], 'runId')
        self.assertEqual(group['example']['head'][0], 'goal')
        # The cache prefix breaks where the reordered object starts, not at the end.
        prefix = report['prefixes'][0]
        self.assertLess(prefix['offset'], prefix['baseLength'] // 2 + 60)

    @unittest.skipIf(os.name == 'nt', 'the stand-in xgen is a shebang script')
    def test_a_removed_key_is_reported_once_not_also_as_a_key_order_change(self):
        code, report, _ = self.run_main('same', 'dropkey')
        self.assertEqual(code, 1)
        [group] = report['groups']
        self.assertEqual(group['fingerprint'], '/messages/*/content~json/planningContext/goal | removed')

    @unittest.skipIf(os.name == 'nt', 'the stand-in xgen is a shebang script')
    def test_sessions_share_state_and_later_sessions_get_the_started_run_id(self):
        binary = self.root / 'xgen-resumable'
        binary.write_text(RESUMABLE_XGEN.format(python=sys.executable))
        os.chmod(binary, 0o755)
        case = {'name': 'resume', 'files': {}, 'replies': [{'completion': 'a'}, {'completion': 'b'}],
                'sessions': [{'args': ['run']}, {'args': ['resume', '{run_id}']}]}
        result = M.run_case(binary, case, self.root / 'case')
        self.assertEqual(result['exit'], [10, 0])
        first, second = (json.loads(json.loads(r['body'])['messages'][0]['content'])['runId']
                         for r in result['requests'])
        self.assertEqual(first, second)
        self.assertEqual(result['unused'], 0)

    def test_noisy_fields_still_compare_by_shape(self):
        base = {'runId': 'run-' + 'a' * 32}
        self.assertEqual(M.shape(base['runId']), M.shape('run-' + 'b' * 32))
        self.assertNotEqual(M.shape('run-' + 'a' * 32), M.shape('job-' + 'a' * 32))

    def test_flatten_records_key_order_array_length_and_embedded_json(self):
        tree = M.parse_tree(json.loads('{"b": 1, "a": ["x", "{\\"k\\": 2}"]}',
                                       object_pairs_hook=lambda pairs: ('obj', pairs)))
        fields = M.flatten(tree)
        self.assertEqual(fields['#keys'], ['b', 'a'])
        self.assertEqual(fields['/a#len'], 2)
        self.assertEqual(fields['/a/1~json/k'], 2)

    def test_an_unrepeatable_build_is_an_error_not_a_difference(self):
        runs = [{'exit': 0, 'requests': [], 'unused': 0}, {'exit': 0, 'requests': [{}], 'unused': 0}]
        stable = [{'exit': 0, 'requests': [], 'unused': 0}] * 2
        with self.assertRaises(M.CaseError):
            M.compare_case({'name': 'flaky'}, runs, stable)


if __name__ == '__main__':
    unittest.main()
