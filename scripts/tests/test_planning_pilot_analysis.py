import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('analysis', Path(__file__).parents[1] / 'analyze-planning-pilot.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


def records():
    return [dict(case=family, family=family, split='validation', condition=condition,
                 accepted=True, task_completed=True, oracle_passed=True, false_claim=False,
                 false_completion=False, duplicate_effect_starts=0, model_calls=3,
                 actual_cost_nano_usd=cost, all_miss_nano_usd=cost * 2,
                 known_budget_cost_nano_usd=cost)
            for family in ('artifact', 'repair') for condition, cost in (('X0', 100), ('X1', 100), ('XN', 80))]


class AnalysisTests(unittest.TestCase):
    def test_failure_cost_is_included_and_blocks_adoption(self):
        rows = records()
        rows[-1].update(accepted=False, task_completed=False, oracle_passed=False, actual_cost_nano_usd=0)
        groups = M.aggregate(rows)
        self.assertEqual(groups['validation/*/XN']['median_actual_cost_nano_usd'], 40)
        gates = M.gates({'complete_schedule': True, 'source_unchanged': True, 'actual_cost_nano_usd': 400},
                        rows, [{'passed': True}] * len(rows), groups)
        self.assertTrue(gates['checks']['validation_median_cost_reduction_at_least_15_percent'])
        self.assertFalse(gates['checks']['validation_completion_per_family_noninferior'])
        self.assertFalse(gates['passed'])

    def test_unknown_cost_does_not_become_zero(self):
        rows = records()
        rows[-1]['actual_cost_nano_usd'] = None
        self.assertIsNone(M.aggregate(rows)['validation/*/XN']['median_actual_cost_nano_usd'])

    def test_design_failure_does_not_replace_validation_gate(self):
        rows = records()
        design = dict(rows[0], split='design', false_claim=True, accepted=False)
        rows.append(design)
        gates = M.gates({'complete_schedule': True, 'source_unchanged': True, 'actual_cost_nano_usd': 660},
                        rows, [{'passed': True}] * len(rows), M.aggregate(rows))
        self.assertTrue(gates['passed'])
        rows[0]['false_claim'] = True
        self.assertFalse(M.gates({'complete_schedule': True, 'source_unchanged': True, 'actual_cost_nano_usd': 660},
                                rows, [{'passed': True}] * len(rows), M.aggregate(rows))['passed'])

    def test_missing_journal_fails_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            result = M.audit(Path(temp) / 'missing.sqlite3')
            self.assertFalse(result['passed'])
            self.assertEqual(result['errors'][0]['code'], 'audit_unavailable')

    def test_cost_recomputation_and_missing_settlement(self):
        quote = {'cached_micro_usd_per_million': 3000, 'input_micro_usd_per_million': 150000,
                 'output_micro_usd_per_million': 600000}
        reservation = dict(event='reserved', call_id='call')
        settlement = dict(event='settled', call_id='call', http_status=200, bound_exceeded=False,
                          usage=dict(cache=100, input=200, output=10),
                          budget_cost_nano_usd=21300, all_miss_nano_usd=36000)
        summary = dict(budget_accounted_nano_usd=21300, unknown_requests=0, unknown_reserved_nano_usd=0)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            ledger = root / 'cost-ledger.jsonl'
            ledger.write_text('\n'.join(json.dumps(x) for x in (reservation, settlement)))
            self.assertTrue(M.cost_audit(root, {'config': {'quote': quote}}, summary,
                                        [{'known_budget_cost_nano_usd': 21300}])['passed'])
            identity_config = {'quote': quote, 'expected_response_identity': ['model', 'fingerprint']}
            settlement['response_metadata'] = {'model': 'model', 'system_fingerprint': 'changed'}
            ledger.write_text('\n'.join(json.dumps(x) for x in (reservation, settlement)))
            self.assertIn('response_identity_changed_or_missing', M.cost_audit(root, {'config': identity_config}, summary,
                                                                              [{'known_budget_cost_nano_usd': 21300}])['errors'])
            settlement['budget_cost_nano_usd'] = 1
            ledger.write_text('\n'.join(json.dumps(x) for x in (reservation, settlement)))
            self.assertIn('quoted_cost_mismatch', M.cost_audit(root, {'config': {'quote': quote}}, summary,
                                                             [{'known_budget_cost_nano_usd': 21300}])['errors'])
            ledger.write_text(json.dumps(reservation))
            self.assertIn('missing_or_duplicate_settlement', M.cost_audit(root, {'config': {'quote': quote}}, summary, [])['errors'])

    def test_paired_block_keeps_failed_comparison(self):
        rows = [dict(r, repeat=2) for r in records()]
        rows[-1].update(accepted=False, actual_cost_nano_usd=0)
        comparisons = M.paired(rows)
        self.assertEqual(len(comparisons), 2)
        self.assertFalse(comparisons[-1]['XN_accepted'])
        self.assertEqual(comparisons[-1]['actual_cost_reduction'], 1)

    def test_response_format_and_command_order_are_separate(self):
        record = dict(trial='trial', case='case', condition='XN', split='validation', changed_files=['source.py'],
                      observed_commands=[{'argv': ['tool', 'check'], 'exit_code': 1},
                                         {'argv': ['tool', 'check'], 'exit_code': 0}])
        with tempfile.TemporaryDirectory() as temp:
            database = Path(temp) / 'run.sqlite3'
            with sqlite3.connect(database) as connection:
                connection.execute('CREATE TABLE completion_outputs(event_sequence INTEGER, record_json BLOB)')
                claim = dict(commands=list(reversed(record['observed_commands'])), changed_files=['source.py'])
                connection.execute('INSERT INTO completion_outputs VALUES (1, ?)',
                                   (json.dumps({'summary': json.dumps(claim)}).encode(),))
            self.assertEqual(M.claim_failure(database, record)['reason'], 'command_order')
            with sqlite3.connect(database) as connection:
                connection.execute('UPDATE completion_outputs SET record_json=?',
                                   (json.dumps({'summary': 'The check passed after the repair.'}).encode(),))
            self.assertEqual(M.claim_failure(database, record)['reason'], 'contract_format')


@unittest.skipUnless(os.environ.get('XGEN_PILOT_AUDIT_FIXTURES'), 'private development journals are optional')
class JournalMutationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(os.environ['XGEN_PILOT_AUDIT_FIXTURES'])
        self.sources = sorted(root.glob('*/state/runs/*/run.sqlite3'))
        self.assertGreaterEqual(len(self.sources), 2)
        distinct = {}
        for source in self.sources:
            case = json.loads((source.parents[3] / 'result.json').read_bytes())['case']
            distinct.setdefault(case, source)
        self.mutation_sources = list(distinct.values())[:2]
        self.assertEqual(len(self.mutation_sources), 2)

    def copied(self, source):
        directory = Path(self.temp.name) / source.parent.name
        directory.mkdir(exist_ok=True)
        for name in ('run.sqlite3', 'materials.sqlite3'):
            shutil.copyfile(source.parent / name, directory / name)
        return directory / 'run.sqlite3'

    def test_development_journals_pass_for_multiple_cases(self):
        for source in self.sources:
            self.assertTrue(M.audit(self.copied(source))['passed'], str(source))

    def test_tampered_grant_material_receipt_and_missing_intent_fail(self):
        mutations = (
            ('run_events', 'event_json', 'effect_intent_committed',
             lambda event: event['body']['intent']['authorization']['binding'].update(actionDigest='sha256:wrong')),
            ('invocation_materials', 'record_json', None,
             lambda material: material.update(materialDigest='sha256:wrong')),
            ('execution_receipts', 'receipt_json', None,
             lambda receipt: receipt.update(invocationId='wrong-invocation')),
        )
        for source in self.mutation_sources:
            for table, column, kind, mutate in mutations:
                database = self.copied(source)
                with sqlite3.connect(database) as connection:
                    candidates = connection.execute(f'SELECT rowid, {column} FROM {table}').fetchall()
                    rowid, value = next((r, json.loads(v)) for r, v in candidates
                                        if kind is None or json.loads(v)['body']['type'] == kind)
                    mutate(value)
                    connection.execute(f'UPDATE {table} SET {column}=? WHERE rowid=?', (json.dumps(value).encode(), rowid))
                self.assertFalse(M.audit(database)['passed'], table)
            database = self.copied(source)
            with sqlite3.connect(database) as connection:
                rows = connection.execute('SELECT sequence, event_json FROM run_events').fetchall()
                sequence = next(s for s, v in rows if json.loads(v)['body']['type'] == 'effect_intent_committed')
                connection.execute('DELETE FROM run_events WHERE sequence=?', (sequence,))
            self.assertFalse(M.audit(database)['passed'])


if __name__ == '__main__':
    unittest.main()
