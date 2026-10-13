import http.server
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

M = load('feedback_runner', SCRIPTS / 'evaluate-planning-model.py')
T = load('runner_fixtures', SCRIPTS / 'tests/test_planning_model_eval.py')


@unittest.skipUnless(os.environ.get('XGEN_PILOT_TEST_BINARY'), 'requires explicit built example binary')
class FeedbackRepairTests(unittest.TestCase):
    def test_diagnostics_reach_next_turn_and_allow_repair_before_completion(self):
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                context = json.loads(body['messages'][1]['content'])['planningContext']
                outputs = [item['output'] for item in context['toolOutputs']]
                count = len(outputs)
                check = ('check', 'xgeny.process/execute', {'executable': 'verify', 'args': [], 'cwd': '.', 'env': {}, 'timeoutMs': 1000, 'maxOutputBytes': 4096})
                if count == 0:
                    action = ('wrong', 'xgeny.fs/write-atomic', {'path': 'result.json', 'content': '{"result":false}\n', 'expectedDigest': None})
                elif count in (1, 4):
                    action = check
                elif count == 2 and outputs[-1]['stderr']:
                    diagnostic = json.loads(outputs[-1]['stderr'])
                    self.server.diagnostics.append(diagnostic)
                    self.server.assertions.append('untrusted data' in body['messages'][0]['content'])
                    action = ('read', 'xgeny.fs/read-text', {'path': 'result.json'})
                elif count == 3:
                    diagnostic = json.loads(outputs[1]['stderr'])
                    action = ('repair', 'xgeny.fs/write-atomic', {'path': 'result.json', 'content': json.dumps(diagnostic['expected']), 'expectedDigest': outputs[-1]['digest']})
                else:
                    action = None
                proposal = {'formatVersion': 1, 'kind': 'completion_candidate', 'steps': [], 'summary': '{"outcome":"completed"}'}
                if action:
                    key, cap, arguments = action
                    proposal.update(kind='plan', summary='', steps=[{'key': key, 'objective': 'Perform operation', 'dependsOn': [], 'capability': {'capabilityId': cap, 'contractVersion': '1.0.0'}, 'arguments': arguments}])
                encoded = json.dumps({'model': 'fixture-model', 'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': json.dumps(proposal)}, 'finish_reason': 'stop'}], 'usage': {'prompt_tokens': 100, 'completion_tokens': 20, 'total_tokens': 120, 'prompt_cache_hit_tokens': 30, 'prompt_cache_miss_tokens': 70}}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)
        server = http.server.HTTPServer(('127.0.0.1', 0), Upstream)
        server.diagnostics, server.assertions = [], []
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            fixtures = []
            for name, expected in [('numeric', {'result': 6}), ('unicode', {'result': 'é한'})]:
                fixtures.append({'id': name, 'split': 'design', 'family': name, 'goal': 'Create the JSON result and verify it.', 'files': {'input.json': '{}\n'}, 'writable': ['result.json'], 'generated_dirs': [], 'executables': ['python3'], 'completion_checker': f'raise SystemExit(verify({expected!r}, "result.json", feedback=FEEDBACK))\n', 'oracle': {'python': f'import json,sys\nfrom pathlib import Path\nassert json.loads((Path(sys.argv[1])/"result.json").read_text()) == {expected!r}\n'}})
            config = dict(T.config(), conditions=['F0', 'F1'], final_response_schema={'type': 'object', 'properties': {'outcome': {'type': 'string', 'enum': ['completed']}}, 'required': ['outcome'], 'additionalProperties': False})
            with tempfile.TemporaryDirectory() as temp:
                output = Path(temp) / 'results'
                summary = M.execute(Path(os.environ['XGEN_PILOT_TEST_BINARY']).resolve(), fixtures, config, output, f'http://127.0.0.1:{server.server_port}/v1', input_paths=[Path(__file__)])
                rows = [json.loads(line) for line in (output/'trials.jsonl').read_text().splitlines()]
                self.assertTrue(summary['source_unchanged'])
                self.assertTrue(all(row['accepted'] and row['failed_check_then_passed'] and row['check_executions'] == 2 for row in rows if row['condition'] == 'F1'), str(rows))
                self.assertTrue(all(not row['task_completed'] and row['gate_rejections'] == ['failed'] for row in rows if row['condition'] == 'F0'))
                self.assertEqual(len(server.diagnostics), 2)
                self.assertTrue(all(server.assertions))
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == '__main__':
    unittest.main()
