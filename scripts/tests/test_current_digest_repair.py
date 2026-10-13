import http.server
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest

from test_feedback_repair_loop import M, T


@unittest.skipUnless(os.environ.get('XGEN_PILOT_TEST_BINARY'), 'requires explicit built example binary')
class CurrentDigestRepairTests(unittest.TestCase):
    def test_two_families_repair_from_snapshot_and_preserve_later_edits(self):
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                context = json.loads(body['messages'][1]['content'])['planningContext']
                if context['runId'] not in self.server.run_ids:
                    self.server.run_ids.append(context['runId'])
                outputs = [item['output'] for item in context['toolOutputs']]
                check = ('check', 'xgeny.process/execute', {'executable': 'verify', 'args': [], 'cwd': '.', 'env': {}, 'timeoutMs': 1000, 'maxOutputBytes': 4096})
                action = None
                if not outputs:
                    action = ('wrong', 'xgeny.fs/write-atomic', {'path': 'result.json', 'content': '{"result":false}\n', 'expectedDigest': None})
                elif 'changed' in outputs[-1]:
                    action = check
                elif outputs[-1].get('exitCode') == 1:
                    diagnostic = json.loads(outputs[-1]['stderr'])
                    if 'actual_digest' not in diagnostic:
                        action = ('refresh', 'xgeny.fs/read-text', {'path': 'result.json'})
                    else:
                        if self.server.external_edit:
                            index = self.server.run_ids.index(context['runId'])
                            self.server.paths[index].write_text('{"external":true}\n')
                        action = ('repair', 'xgeny.fs/write-atomic', {'path': 'result.json', 'content': json.dumps(diagnostic['expected']), 'expectedDigest': diagnostic['actual_digest']})
                elif 'content' in outputs[-1]:
                    diagnostic = json.loads(outputs[-2]['stderr'])
                    action = ('repair', 'xgeny.fs/write-atomic', {'path': 'result.json', 'content': json.dumps(diagnostic['expected']), 'expectedDigest': outputs[-1]['digest']})
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
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        fixtures = []
        for name, expected in [('numeric', {'result': 6}), ('unicode', {'result': 'é한'})]:
            fixtures.append({'id': name, 'split': 'design', 'family': name, 'goal': 'Repair and verify.', 'files': {'input.json': '{}\n'}, 'writable': ['result.json'], 'generated_dirs': [], 'executables': ['python3'], 'completion_checker': f'raise SystemExit(verify({expected!r}, "result.json", include_digest=INCLUDE_DIGEST))\n', 'oracle': {'python': f'import json,sys\nfrom pathlib import Path\nassert json.loads((Path(sys.argv[1])/"result.json").read_text()) == {expected!r}\n'}})
        config = dict(T.config(), conditions=['D0', 'D1'], final_response_schema={'type': 'object', 'properties': {'outcome': {'type': 'string', 'enum': ['completed']}}, 'required': ['outcome'], 'additionalProperties': False})
        try:
            with tempfile.TemporaryDirectory() as temp:
                for changed in (False, True):
                    output = Path(temp) / str(changed)
                    schedule = M.schedule(fixtures, config['repeats'], config['seed'], config['conditions'])
                    server.paths = [output / f'{i:04d}-{case["id"]}-{repeat}-{condition}' / 'workspace/result.json' for i, (case, repeat, condition) in enumerate(schedule)]
                    server.run_ids, server.external_edit = [], changed
                    M.execute(Path(os.environ['XGEN_PILOT_TEST_BINARY']).resolve(), fixtures, config, output, f'http://127.0.0.1:{server.server_port}/v1', input_paths=[Path(__file__)])
                    rows = [json.loads(line) for line in (output/'trials.jsonl').read_text().splitlines()]
                    for row in rows:
                        if changed and row['condition'] == 'D1':
                            self.assertFalse(row['task_completed'])
                            self.assertEqual((output/row['trial']/'workspace/result.json').read_text(), '{"external":true}\n')
                        else:
                            self.assertTrue(row['accepted'] and row['failed_check_then_passed'], str(row))
                            self.assertEqual(row['model_calls'], 5 if row['condition'] == 'D1' else 6)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == '__main__':
    unittest.main()
