import copy
import http.server
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock
import urllib.error
import urllib.request

SPEC = importlib.util.spec_from_file_location('agy_baseline', Path(__file__).parents[1] / 'evaluate-agy-coding-baseline.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
CONFIG = json.loads((M.ROOT / 'evals/coding-baseline/agy-config.json').read_text())


def script(root, body):
    path = root / 'fake.py'
    path.write_text('import json,sys,time,subprocess\n' + body)
    return [sys.executable, str(path)]


def init():
    return {'event': 'init', 'init': {'agent': 'xgen-planner-only', 'model': CONFIG['model'], 'tools': []}}


class AgentTransportTests(unittest.TestCase):
    def test_missing_or_inconsistent_usage_stays_unknown(self):
        self.assertEqual(M.usage(None), {'input': None, 'uncached_input': None, 'output': None, 'cache': None, 'reasoning': None})
        partial = M.usage({'input_tokens': 30, 'output_tokens': 4, 'total_tokens': 34})
        self.assertEqual(partial['uncached_input'], 30)
        self.assertIsNone(partial['input'])
        self.assertIsNone(partial['cache'])
        self.assertIsNone(M.usage({'input_tokens': 30, 'output_tokens': 4, 'total_tokens': 33})['input'])
        for uncached, cached, output in [(278, 30214, 4), (2087, 7003, 109), (2789, 5234, 107)]:
            result = M.usage({'input_tokens': uncached, 'output_tokens': output,
                              'total_tokens': uncached + output, 'cache_read_tokens': cached})
            self.assertEqual(result['input'], uncached + cached)
            self.assertEqual(result['cache'], cached)
            self.assertEqual(result['uncached_input'], uncached)

    def test_invalid_counters_are_not_fabricated(self):
        for counter in [True, -1, 1.5, '30', None]:
            result = M.usage({'input_tokens': counter, 'output_tokens': 4,
                              'total_tokens': 34, 'cache_read_tokens': 100})
            self.assertIsNone(result['input'])
            self.assertIsNone(result['uncached_input'])
        result = M.usage({'input_tokens': 30, 'output_tokens': 4, 'total_tokens': 34,
                          'cache_read_tokens': 100, 'thinking_tokens': 3})
        self.assertEqual(result['input'], 130)
        self.assertEqual(result['output'], 4)  # Thinking must not be added twice.
        self.assertEqual(result['reasoning'], 3)
        for cached in [None, True, -1, '100']:
            invalid = M.usage({'input_tokens': 30, 'output_tokens': 4, 'total_tokens': 34,
                               'cache_read_tokens': cached, 'thinking_tokens': 5})
            self.assertIsNone(invalid['input'])
            self.assertEqual(invalid['uncached_input'], 30)
            self.assertIsNone(invalid['cache'])
            self.assertIsNone(invalid['reasoning'])

    def test_offline_usage_reanalysis_preserves_evidence_and_missing_attempts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'manifest.json').write_text('{}')
            ledger = root / 'agy-ledger.jsonl'
            ledger.write_text(''.join(json.dumps({'status': 'reserved', 'index': index, 'trial': 'fixture'})
                                      + '\n' for index in [1, 2, 3]))
            (root / 'agy-call-0001.ndjson').write_text(json.dumps({'event': 'result', 'result': {
                'usage': {'input_tokens': 30, 'output_tokens': 4, 'total_tokens': 34,
                          'cache_read_tokens': 100, 'thinking_tokens': 3}}}) + '\n')
            (root / 'agy-call-0002.ndjson').write_text('{"event":')
            before = M.S.inventory(root)
            result = M.reanalyse_usage(root)
            self.assertEqual(M.S.inventory(root), before)
            self.assertEqual(result['cli_attempts'], 3)
            self.assertEqual(result['unknown_usage_calls'], 2)
            self.assertEqual(result['coverage_calls']['input'], 1)
            self.assertEqual(result['token_subtotals']['input'], 130)
            self.assertEqual(result['token_subtotals']['uncached_input'], 30)
            self.assertIsNone(result['usd_cost'])
            self.assertFalse(result['model_io_performed'])
            self.assertFalse(result['quality_remeasured'])
            ledger.write_text(ledger.read_text() + json.dumps({'status': 'reserved', 'index': 3, 'trial': 'fixture'}))
            with self.assertRaises(ValueError):
                M.reanalyse_usage(root)

    def test_offline_usage_rejects_orphan_settlement_and_ambiguous_final(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'manifest.json').write_text('{}')
            ledger = root / 'agy-ledger.jsonl'
            reserved = {'status': 'reserved', 'index': 1, 'trial': 'fixture'}
            ledger.write_text(json.dumps(reserved) + '\n' + json.dumps({'status': 'settled', 'index': 2}))
            with self.assertRaises(ValueError):
                M.reanalyse_usage(root)
            ledger.write_text(json.dumps(reserved))
            final = {'event': 'result', 'result': {'usage': {
                'input_tokens': 30, 'output_tokens': 4, 'total_tokens': 34, 'cache_read_tokens': 100}}}
            for duplicate in [{'event': 'result', 'result': None}, final]:
                (root / 'agy-call-0001.ndjson').write_text(json.dumps(final) + '\n' + json.dumps(duplicate))
                result = M.reanalyse_usage(root)
                self.assertEqual(result['unknown_usage_calls'], 1)
                self.assertEqual(result['coverage_calls']['input'], 0)
                self.assertEqual(result['calls'][0]['final_count'], 2)

    def test_agent_environment_excludes_unrelated_parent_variables(self):
        with tempfile.TemporaryDirectory() as temporary, mock.patch.dict(os.environ, {'XGEN_EVAL_ENV_SENTINEL': 'private-parent-only'}):
            root = Path(temporary)
            command = script(root, 'import os\nassert "XGEN_EVAL_ENV_SENTINEL" not in os.environ\n'
                f'print({json.dumps(json.dumps(init()))},flush=True)\n')
            result = M.bounded_agent(command, root, CONFIG, time.monotonic() + 5, threading.Event())
            self.assertEqual(result['exit_code'], 0)
            self.assertIsNone(result['reason'])

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'pidfd_open'), 'Linux cleanup required')
    def test_native_tool_event_stops_before_following_side_effect(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            event = {'event': 'step_update', 'step_update': {'step_type': 'tool', 'tool_name': 'manage_task'}}
            command = script(root, f'print({json.dumps(json.dumps(init()))},flush=True)\n'
                f'print({json.dumps(json.dumps(event))},flush=True)\ntime.sleep(2)\n'
                'from pathlib import Path\nPath("must-not-exist").write_text("executed")\n')
            result = M.bounded_agent(command, root, CONFIG, time.monotonic() + 5, threading.Event())
            self.assertEqual(result['reason'], 'native_tool_observed')
            self.assertFalse((root / 'must-not-exist').exists())

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'pidfd_open'), 'Linux cleanup required')
    def test_timeout_kills_descendant_and_stdout_flood_is_bounded(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            command = script(root, 'subprocess.Popen([sys.executable,"-c",'
                '"import time;from pathlib import Path;time.sleep(1);Path(\'leak\').write_text(\'bad\')"])\n'
                'time.sleep(10)\n')
            config = dict(CONFIG, request_timeout_seconds=.15)
            result = M.bounded_agent(command, root, config, time.monotonic() + 5, threading.Event())
            self.assertEqual(result['reason'], 'request_timeout')
            time.sleep(1.1)
            self.assertFalse((root / 'leak').exists())
            command = script(root, 'print("x"*300000,flush=True)\ntime.sleep(5)\n')
            config = dict(CONFIG, max_stdout_bytes=1024)
            result = M.bounded_agent(command, root, config, time.monotonic() + 5, threading.Event())
            self.assertEqual(result['reason'], 'output_limit')
            self.assertEqual(len(result['stdout']), 1024)

    def test_durable_call_caps_and_pin_drift_block_before_launch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            settings = root / 'settings.json'
            settings.write_text('{}')
            config = dict(CONFIG, max_cli_calls=2, max_cli_calls_per_trial=1)
            bridge = M.Bridge(config, root, root, ['never-executed'], settings, {settings: M.S.digest(settings)})
            try:
                bridge.reserve('one', 'a')
                with self.assertRaises(ValueError):
                    bridge.reserve('one', 'b')
                bridge.reserve('two', 'b')
                with self.assertRaises(ValueError):
                    bridge.reserve('three', 'c')
                rows = [json.loads(line) for line in (root / 'agy-ledger.jsonl').read_text().splitlines()]
                self.assertEqual(len(rows), 2)
                settings.write_text('{"changed":true}')
                with self.assertRaises(ValueError):
                    bridge.reserve('three', 'd')
                self.assertEqual(bridge.abort_reason, 'pinned_inputs_changed')
            finally:
                bridge.server_close()

    def test_native_subagent_and_duplicate_event_keys_rejected(self):
        events = []
        for step in [{'subagent_info': {'subagents': ['unexpected']}}, {'tool_info': {'name': 'read_file'}}]:
            with self.assertRaises(ValueError):
                M.consume_event(json.dumps({'event': 'step_update', 'step_update': step}), events, CONFIG['model'])
        with self.assertRaises(ValueError):
            M.consume_event('{"event":"init","event":"result"}', events, CONFIG['model'])

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'pidfd_open')
                         and (M.ROOT / 'target/debug/xgen').is_file(), 'requires Linux and built xgen')
    def test_public_cli_and_approval_use_fake_proposals_without_native_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            agent_workspace = root / 'agent'
            agent_workspace.mkdir()
            settings = root / 'settings.json'
            settings.write_text('{}')
            # Choose proposals only from context supplied by the host, not local files.
            body = '''prompt=sys.argv[-1]
context=json.loads(prompt.split("HOST REQUEST AND OBSERVATIONS:\\n",1)[1].split("\\nReturn exactly",1)[0])["planningContext"]
if not context["toolOutputs"]:
 proposal={"formatVersion":1,"kind":"plan","summary":"","steps":[{"key":"write","objective":"Write requested text","dependsOn":[],"capability":{"capabilityId":"xgeny.fs/write-atomic","contractVersion":"1.0.0"},"arguments":{"path":"result.txt","content":"ready\\n","expectedDigest":None}}]}
elif len(context["toolOutputs"])==1:
 proposal={"formatVersion":1,"kind":"plan","summary":"","steps":[{"key":"verify","objective":"Verify requested text","dependsOn":[],"capability":{"capabilityId":"xgeny.process/execute","contractVersion":"1.0.0"},"arguments":{"executable":"python3","args":["-B","tests.py"],"cwd":".","env":{},"timeoutMs":5000,"maxOutputBytes":4096}}]}
else:
 proposal={"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":"Verified artifact."}
print(json.dumps({"event":"init","init":{"agent":"xgen-planner-only","model":"gemini-3.8-flash-low","tools":[]}}),flush=True)
print(json.dumps({"event":"result","result":{"status":"SUCCESS","response":json.dumps(proposal),"usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120,"cache_read_tokens":150}}}),flush=True)
'''
            command = script(root, body)
            bridge = M.Bridge(CONFIG, root, agent_workspace, command, settings, {settings: M.S.digest(settings)})
            worker = threading.Thread(target=bridge.serve_forever, daemon=True)
            worker.start()
            try:
                case = {'id': 'fixture', 'split': 'design', 'family': 'approval-resume',
                        'goal': 'Write ready and newline to result.txt and run tests.py.',
                        'files': {'note.txt': 'input\n', 'tests.py': 'from pathlib import Path\nassert Path("result.txt").read_text()=="ready\\n"\n'},
                        'writable': ['result.txt'],
                        'oracle': 'from pathlib import Path\nimport sys\nassert (Path(sys.argv[1])/"result.txt").read_text()=="ready\\n"\n'}
                config = {'model': CONFIG['model'], 'tokenizer': CONFIG['model'], 'response_format': 'json_object',
                          'thinking': 'default', 'max_output_tokens': 4096, 'request_timeout_seconds': 10,
                          'max_model_turns': 8, 'max_ticks': 128, 'trial_timeout_seconds': 20}
                row = M.BASE.trial(M.ROOT / 'target/debug/xgen', case, 1, root / 'trial-001', config, bridge)
                M.attach_usage(row, bridge)
                self.assertTrue(row['accepted'], row)
                self.assertTrue(row['approval_pause_observed'])
                self.assertEqual(row['agy_cli_calls'], 3)
                self.assertEqual(row['native_tool_events'], 0)
                self.assertIsNone(row['usd_cost'])
                report = M.aggregate([row], 20, bridge, None)
                self.assertFalse(report['complete_schedule'])
                self.assertIsNone(report['usd_cost'])
                self.assertEqual(report['token_subtotals']['cache'], 450)
                self.assertEqual(report['token_subtotals']['input'], 750)
            finally:
                bridge.shutdown()
                bridge.server_close()
                worker.join()


if __name__ == '__main__':
    unittest.main()
