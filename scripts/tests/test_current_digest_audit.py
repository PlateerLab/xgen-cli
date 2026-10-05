import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('digest_audit', ROOT/'scripts/audit-current-digest.py')
M = importlib.util.module_from_spec(spec)
spec.loader.exec_module(M)


@unittest.skipUnless(os.environ.get('XGEN_CURRENT_DIGEST_AUDIT_FIXTURES'), 'private registered journals are optional')
class CurrentDigestAuditTests(unittest.TestCase):
    def test_all_journals_pass_and_tampered_digests_fail_across_two_families(self):
        root = Path(os.environ['XGEN_CURRENT_DIGEST_AUDIT_FIXTURES'])
        cases = {case['id']:case for case in json.loads((ROOT/'evals/current-digest/cases.json').read_bytes())}
        rows = [json.loads(line) for line in (root/'trials.jsonl').read_text().splitlines()]
        sources = {}
        for row in rows:
            database = next((root/row['trial']).rglob('run.sqlite3'))
            initial = cases[row['case']]['files']['result.json'].encode()
            self.assertTrue(M.audit(database, initial, row['condition'])['passed'])
            if row['condition'] == 'D1':
                sources.setdefault(row['family'], (database, initial))
        self.assertEqual(len(sources), 2)
        for database, initial in sources.values():
            for mutation, code in [('digest', 'diagnostic_digest_snapshot'), ('json_type', 'diagnostic_actual_snapshot'), ('exit_code', 'unexpected_check_exit_code')]:
                with tempfile.TemporaryDirectory() as temp:
                    copy = Path(temp)/'run.sqlite3'
                    shutil.copyfile(database, copy)
                    shutil.copyfile(database.parent/'materials.sqlite3', Path(temp)/'materials.sqlite3')
                    with sqlite3.connect(copy) as connection:
                        rowid, output = next((rowid,json.loads(raw)) for rowid,raw in connection.execute('SELECT rowid,record_json FROM tool_outputs') if json.loads(raw)['output'].get('exitCode') == 1)
                        diagnostic = json.loads(output['output']['stderr'])
                        if mutation == 'digest':
                            diagnostic['actual_digest'] = 'sha256:' + '0' * 64
                        elif mutation == 'json_type':
                            # bool and int compare equal in Python, but are distinct JSON types.
                            self.assertEqual(diagnostic['actual'], {'incorrect': True})
                            diagnostic['actual'] = {'incorrect': 1}
                        else:
                            output['output']['exitCode'] = 2
                        output['output']['stderr'] = json.dumps(diagnostic)
                        connection.execute('UPDATE tool_outputs SET record_json=? WHERE rowid=?', (json.dumps(output).encode(), rowid))
                    result = M.audit(copy, initial, 'D1')
                    self.assertFalse(result['passed'])
                    self.assertIn(code, result['errors'])


if __name__ == '__main__':
    unittest.main()
