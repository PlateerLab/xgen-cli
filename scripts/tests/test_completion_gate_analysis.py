import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('gate_analysis', Path(__file__).resolve().parents[1] / 'analyze-completion-gate.py')
M = importlib.util.module_from_spec(spec)
spec.loader.exec_module(M)


class MissingJournalTests(unittest.TestCase):
    def test_missing_journal_is_failed_audit(self):
        with tempfile.TemporaryDirectory() as temp:
            self.assertFalse(M.audit(Path(temp) / 'missing.sqlite3')['passed'])


@unittest.skipUnless(os.environ.get('XGEN_GATE_AUDIT_FIXTURES'), 'private failed-effect journals are optional')
class FailedEffectAuditTests(unittest.TestCase):
    def test_failure_preserves_authorization_checks_across_two_cases(self):
        root = Path(os.environ['XGEN_GATE_AUDIT_FIXTURES'])
        sources = sorted(root.glob('*/state/runs/*/run.sqlite3'))
        failures = []
        for source in sources:
            result = M.audit(source)
            self.assertTrue(result['passed'], str(result))
            if result['failed_effects']:
                failures.append(source)
        distinct = {}
        for source in failures:
            case = json.loads((source.parents[3] / 'result.json').read_bytes())['case']
            distinct.setdefault(case, source)
        self.assertGreaterEqual(len(distinct), 2)
        for source in list(distinct.values())[:2]:
            for mutation in ('authorization', 'failure_step', 'missing_failure'):
                with tempfile.TemporaryDirectory() as temp:
                    destination = Path(temp)
                    for name in ('run.sqlite3', 'materials.sqlite3'):
                        shutil.copyfile(source.parent / name, destination / name)
                    database = destination / 'run.sqlite3'
                    with sqlite3.connect(database) as connection:
                        rows = [(rowid, json.loads(value)) for rowid, value in connection.execute('SELECT rowid,event_json FROM run_events')]
                        effect = next(event['body']['effectId'] for _, event in rows if event['body']['type'] == 'effect_failed')
                        kind = 'effect_intent_committed' if mutation == 'authorization' else 'effect_failed'
                        rowid, event = next((i,e) for i,e in rows if e['body']['type'] == kind and (e['body'].get('intent') or e['body']).get('effectId') == effect)
                        if mutation == 'missing_failure':
                            connection.execute('DELETE FROM run_events WHERE rowid=?', (rowid,))
                        else:
                            if mutation == 'authorization':
                                event['body']['intent']['authorization']['binding']['actionDigest'] = 'sha256:tampered'
                            else:
                                event['body']['stepId'] = 'wrong-step'
                            connection.execute('UPDATE run_events SET event_json=? WHERE rowid=?', (json.dumps(event).encode(), rowid))
                    self.assertFalse(M.audit(database)['passed'], mutation)


if __name__ == '__main__':
    unittest.main()
