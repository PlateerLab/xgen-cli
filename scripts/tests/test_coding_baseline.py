import copy
from datetime import datetime, timedelta, timezone
import http.server
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest

SPEC = importlib.util.spec_from_file_location('coding_baseline', Path(__file__).parents[1] / 'evaluate-coding-baseline.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
CASES = json.loads((M.ROOT / 'evals/coding-baseline/cases.json').read_text())

# Known solutions audit the fixtures and runner; they are never passed to a live model.
SOLUTIONS = {
    'bug-counts': {'source.py': 'def ordered_counts(items):\n    counts={}\n    for item in items:\n        counts[item]=counts.get(item,0)+1\n    return sorted(counts.items(),key=lambda pair:(-pair[1],pair[0]))\n'},
    'bug-window': {'source.py': 'def window(items,start,stop):\n    return items[max(0,start):min(len(items),stop)]\n'},
    'rename-field': {
        'model.py': 'from dataclasses import dataclass\n@dataclass\nclass Item:\n    rank:int\n',
        'service.py': 'from model import Item\ndef make(value):\n    return Item(rank=value)\n',
        'report.py': 'def serialize(item):\n    return {"rank":item.rank}\n'},
    'rename-function': {
        'engine.py': 'def decode_line(line):\n    return line.strip().split(":",1)\n',
        'api.py': 'from engine import decode_line\ndef read(line):\n    return decode_line(line)\n',
        'app.py': 'import engine\ndef render(line):\n    return "=".join(engine.decode_line(line))\n'},
    'recover-flag': {'source.py': 'def parse_flag(value):\n    value=value.strip().lower()\n    if value not in {"true","false"}:\n        raise ValueError("invalid flag")\n    return value=="true"\n'},
    'recover-transpose': {'source.py': 'def transpose(rows):\n    return [list(column) for column in zip(*rows)]\n'},
    'resume-rounding': {'source.py': 'def ceil_div(total,size):\n    return (total+size-1)//size\n'},
    'resume-lines': {'source.py': 'def nonempty_lines(text):\n    return [line.strip() for line in text.splitlines() if line.strip()]\n'},
}
ANSWERS = {'question-routing': {'handler': 'serial', 'batch_size': 12},
           'question-fallback': {'timeout': 45, 'retries': 2}}


def config():
    now = datetime.now(timezone.utc)
    return {'model': 'baseline-fixture', 'tokenizer': 'baseline-fixture',
            'response_format': 'json_object', 'thinking': 'disabled',
            'max_output_tokens': 4096, 'request_timeout_seconds': 5,
            'max_model_turns': 24, 'max_ticks': 256, 'max_input_tokens': 131072,
            'trial_timeout_seconds': 30, 'spend_cap_nano_usd': 1000000000,
            'seed': 20261120, 'repeats': 1,
            'quote': {'kind': 'fixed_tier', 'source': 'synthetic offline test quote',
                      'valid_from_utc': (now - timedelta(hours=1)).isoformat(),
                      'valid_until_utc': (now + timedelta(hours=1)).isoformat(),
                      'cached_micro_usd_per_million': 1, 'input_micro_usd_per_million': 2,
                      'output_micro_usd_per_million': 3}}


def plan(index, capability, arguments, version='1.0.0'):
    return {'formatVersion': 1, 'kind': 'plan', 'summary': '', 'steps': [{
        'key': f'step-{index}', 'objective': 'contract fixture action', 'dependsOn': [],
        'capability': {'capabilityId': capability, 'contractVersion': version}, 'arguments': arguments}]}


def completion(summary):
    return {'formatVersion': 1, 'kind': 'completion_candidate', 'steps': [], 'summary': summary}


def responses(case, solve=True):
    result = []
    if solve and case['family'] == 'failure-recovery':
        result.append(plan(0, 'xgeny.process/execute', {'executable': 'python3', 'args': ['-B', 'tests.py'],
            'cwd': '.', 'env': {}, 'timeoutMs': 5000, 'maxOutputBytes': 4096}))
    for name in case['writable'] or [next(iter(case['files']))]:
        result.append(plan(len(result), 'xgeny.fs/read-text', {'path': name}))
        if solve and name in case['writable']:
            result.append(plan(len(result), 'xgeny.fs/apply-patch', {'path': name,
                'expectedDigest': 'sha256:' + M.hashlib.sha256(case['files'][name].encode()).hexdigest(),
                'edits': [{'oldText': case['files'][name], 'newText': SOLUTIONS[case['id']][name]}]}, '2.0.0'))
    if solve and case['writable']:
        result.append(plan(len(result), 'xgeny.process/execute', {'executable': 'python3', 'args': ['-B', 'tests.py'],
            'cwd': '.', 'env': {}, 'timeoutMs': 5000, 'maxOutputBytes': 4096}))
    result.append(completion(json.dumps(ANSWERS.get(case['id'], {})) if solve else '{}'))
    return result


class BaselineTests(unittest.TestCase):
    def test_fixture_oracles_reject_originals_and_accept_known_solutions(self):
        M.validate_cases(CASES)
        M.validate_config(config(), CASES)
        for case in CASES:
            with self.subTest(case=case['id']), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                workspace = root / 'workspace'
                workspace.mkdir()
                for name, content in case['files'].items():
                    path = workspace / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                oracle, answer = root / 'oracle.py', root / 'answer.json'
                oracle.write_text(case['oracle'])
                answer.write_text('{}')
                command = [sys.executable, '-I', '-B', str(oracle), str(workspace), str(answer)]
                self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
                for name, content in SOLUTIONS.get(case['id'], {}).items():
                    (workspace / name).write_text(content)
                answer.write_text(json.dumps(ANSWERS.get(case['id'], {})))
                good = subprocess.run(command, capture_output=True)
                self.assertEqual(good.returncode, 0, good.stderr.decode())
                # A fixture cannot pass by deleting its visible tests.
                if case['writable']:
                    (workspace / 'tests.py').unlink()
                    self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)

    def test_repetition_uses_same_capability_version_arguments_and_result(self):
        recipes = {'a': {'arguments': {'path': 'one'}}, 'b': {'arguments': {'path': 'one'}},
                   'c': {'arguments': {'path': 'two'}}, 'd': {'arguments': {'path': 'one'}}}
        outputs = [{'stepId': step, 'invocation': {'capabilityId': 'read', 'contractVersion': '1'},
                    'outputDigest': value} for step, value in [('a', 'old'), ('b', 'old'), ('c', 'old'), ('d', 'new')]]
        events = [{'type': 'model_call_settled', 'settlement': {'disposition': 'rejected', 'reason': 'proposal_rejected'}},
                  {'type': 'model_call_settled', 'settlement': {'disposition': 'rejected', 'reason': 'planner_invalid_response'}}]
        result = M.runtime_metrics(events, outputs, recipes)
        self.assertEqual(result['repeated_finished_actions'], 2)
        self.assertEqual(result['repeated_identical_results'], 1)
        self.assertEqual(result['proposal_rejections'], 1)

    def test_completion_claim_requires_both_oracle_and_file_scope(self):
        candidate = {'responseKind': 'task_completion'}
        self.assertTrue(M.completion_flags(candidate, True, False)['false_completion'])
        self.assertTrue(M.completion_flags(candidate, False, True)['false_completion'])
        self.assertFalse(M.completion_flags(candidate, True, True)['false_completion'])
        self.assertFalse(M.completion_flags(None, False, False)['task_completed'])

    def test_frozen_matrix_and_paths_reject_missing_holdout_and_escape(self):
        with self.assertRaises(ValueError):
            M.validate_cases([case for case in CASES if case['split'] == 'design'])
        cases = copy.deepcopy(CASES)
        cases[0]['files']['../oracle.py'] = 'unreadable expected answer'
        with self.assertRaises(ValueError):
            M.validate_cases(cases)
        self.assertEqual(M.schedule(CASES, 2, 17), M.schedule(CASES, 2, 17))
        self.assertEqual(len(M.schedule(CASES, 2, 17)), 20)

    def test_unknown_usage_and_failed_trials_remain_in_aggregates(self):
        with tempfile.TemporaryDirectory() as temporary:
            ledger = M.SUPPORT.Ledger(config(), Path(temporary) / 'cost.jsonl')
            self.assertTrue(ledger.reserve('call', 'trial'))
            ledger.settle('call', 'trial', b'{"error":"failed"}', 502)
            record = {'trial': 'trial', 'split': 'held_out', 'family': 'bug-fix', 'accepted': False,
                      'false_completion': None, 'elapsed_seconds': 1, 'outcome': 'workflow_failure',
                      'repeated_finished_actions': None}
            M.attach_cost(record, ledger)
            report = M.aggregate([record], 20, ledger)
            self.assertIsNone(report['actual_cost_nano_usd'])
            self.assertGreater(report['unknown_reserved_nano_usd'], 0)
            self.assertFalse(report['complete_token_usage'])
            self.assertFalse(report['complete_schedule'])
            self.assertEqual(report['groups']['held_out/bug-fix']['attempted'], 1)
            self.assertEqual(report['groups']['held_out/bug-fix']['unknown_completion_checks'], 1)

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'pidfd_open')
                         and (M.ROOT / 'target/debug/xgen').is_file(), 'requires Linux and a built public xgen')
    def test_public_cli_success_and_false_completion_with_isolated_mock_model(self):
        binary = M.ROOT / 'target/debug/xgen'
        for solve in [True, False]:
            sequences = {case['goal']: responses(case, solve) for case in CASES}
            failures = []

            class Model(http.server.BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def do_POST(self):
                    try:
                        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                        context = json.loads(body['messages'][1]['content'])['planningContext']
                        if not sequences[context['goal']]:
                            raise ValueError(str(context.get('previousResponseRejection')))
                        proposal = sequences[context['goal']].pop(0)
                        result = {'model': body['model'], 'system_fingerprint': 'offline-fixture',
                            'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': json.dumps(proposal)},
                                         'finish_reason': 'stop'}],
                            'usage': {'prompt_tokens': 100, 'completion_tokens': 10, 'total_tokens': 110,
                                      'prompt_tokens_details': {'cached_tokens': 25}}}
                        raw = json.dumps(result).encode()
                        self.send_response(200)
                    except Exception as error:
                        failures.append(f'{type(error).__name__}: {error}')
                        raw = b'{}'
                        self.send_response(500)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)

            upstream = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
            worker = threading.Thread(target=upstream.serve_forever, daemon=True)
            worker.start()
            try:
                with self.subTest(solve=solve), tempfile.TemporaryDirectory() as temporary:
                    output = Path(temporary) / 'result'
                    report = M.execute(binary, CASES, config(), output,
                                       f'http://127.0.0.1:{upstream.server_port}/v1')
                    self.assertFalse(failures, failures)
                    self.assertTrue(report['complete_schedule'], report)
                    self.assertEqual(report['passed'], 10 if solve else 0, (output / 'trials.jsonl').read_text())
                    records = [json.loads(line) for line in (output / 'trials.jsonl').read_text().splitlines()]
                    self.assertEqual(sum(row['false_completion'] is True for row in records), 0 if solve else 10)
                    self.assertTrue(report['complete_token_usage'])
                    self.assertGreater(report['known_cost_nano_usd'], 0)
                    self.assertEqual(json.loads((output / 'manifest.json').read_text())['result_kind'], 'loopback-contract-test')
                    self.assertTrue(all(row['evidence_complete'] for row in records))
            finally:
                upstream.shutdown()
                upstream.server_close()
                worker.join()


if __name__ == '__main__':
    unittest.main()
