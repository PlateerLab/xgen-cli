import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('response_contracts', Path(__file__).parents[1] / 'experiment-response-contracts.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def records():
    result = []
    for split in ('design', 'held_out'):
        for case in ('a', 'b'):
            for variant in ('baseline', 'copy_observation'):
                exact = variant == 'copy_observation' or (split == 'held_out' and case == 'b')
                result.append({'case': f'{split}-{case}', 'split': split, 'variant': variant,
                    'outcome': 'pass', 'exact_final_source_quote': exact,
                    'model': 'same-model', 'request_profile_digest': 'same-profile',
                    'model_calls': 8 if variant == 'copy_observation' else 7,
                    'unknown_events': 0, 'duplicate_effect_starts': 0,
                    'usage': {'completeTokenUsage': True, 'tokenSubtotal': {
                        'totalTokens': 1200 if variant == 'copy_observation' else 1000,
                        'cachedInputTokens': 400}}})
    return result


class ResponseContractTests(unittest.TestCase):
    def test_observed_commands_join_outputs_in_execution_order_not_recipe_order(self):
        import json
        import sqlite3
        import tempfile
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            with sqlite3.connect(root / 'materials.sqlite3') as connection:
                connection.execute('CREATE TABLE material_recipe (record BLOB)')
                for step, executable, args in [('build', 'node', ['--check', 'source.mjs']),
                                               ('test', 'node', ['--test', 'test.mjs']),
                                               ('unexecuted', 'python3', ['--version'])]:
                    connection.execute('INSERT INTO material_recipe VALUES (?)', (json.dumps({
                        'stepId': step, 'capability': {'capabilityId': 'xgeny.process/execute'},
                        'arguments': {'executable': f'process:primary/executables/{executable}', 'args': args}}),))
            outputs = [{'stepId': step, 'invocation': {'capabilityId': 'xgeny.process/execute'},
                        'output': {'exitCode': code}} for step, code in [('test', 1), ('test', 0), ('build', 0)]]
            self.assertEqual(MODULE.EVAL.observed_commands(root / 'run.sqlite3', outputs), [
                {'argv': ['node', '--test', 'test.mjs'], 'exit_code': 1},
                {'argv': ['node', '--test', 'test.mjs'], 'exit_code': 0},
                {'argv': ['node', '--check', 'source.mjs'], 'exit_code': 0}])

    def test_evidence_is_not_accepted_when_failed_commands_or_changed_files_are_omitted(self):
        import json
        commands = [{'argv': ['node', '--test', 'test.mjs'], 'exit_code': 1},
                    {'argv': ['node', '--test', 'test.mjs'], 'exit_code': 0}]
        for claim, expected in [
            ({'commands': commands, 'changed_files': ['source.mjs']}, (True, True)),
            ({'commands': commands[1:], 'changed_files': ['source.mjs']}, (False, True)),
            ({'commands': commands, 'changed_files': []}, (True, False)),
        ]:
            actual = MODULE.EVAL.evidence_claims('```json\n' + json.dumps(claim) + '\n```', commands, ['source.mjs'])
            self.assertEqual((actual['command_claims_match'], actual['changed_file_claims_match']), expected)

    def test_invalid_evidence_cannot_pass_as_missing_or_duplicate_json(self):
        for summary in ('no evidence', '```json\n{\n```', '```json\n{}\n```\n```json\n{}\n```'):
            self.assertFalse(MODULE.EVAL.evidence_claims(summary, [], [])['evidence_contract_valid'])

    def test_evidence_regression_is_rejected_separately_from_literal_quotes(self):
        rs = records()
        for r in rs:
            r.update(command_claims_match=True, changed_file_claims_match=True)
        for r in rs:
            if r['split'] == 'held_out' and r['variant'] == 'copy_observation':
                r['command_claims_match'] = False
        self.assertEqual(MODULE.gate(rs, 2, True), 'do_not_adopt_evidence_regression')

    def test_partial_usage_does_not_report_a_complete_cached_mean(self):
        rs = records()
        rs[0]['usage']['completeTokenUsage'] = False
        group = MODULE.summarize(rs)['design']['baseline']
        self.assertIsNone(group['mean_total_tokens'])
        self.assertIsNone(group['mean_cached_input_tokens'])

    def test_joint_design_and_held_out_gain_with_bounded_usage_is_promising(self):
        self.assertEqual(MODULE.gate(records(), 2), 'promising_requires_larger_validation')

    def test_completion_regression_is_rejected_despite_perfect_surviving_quotes(self):
        rs = records()
        rs[-1]['outcome'] = 'workflow_failure'
        self.assertEqual(MODULE.gate(rs, 2), 'do_not_adopt_workflow_regression')

    def test_no_claim_of_improvement_without_two_distinct_design_failures(self):
        rs = records()
        rs[0]['exact_final_source_quote'] = True
        self.assertEqual(MODULE.gate(rs, 2), 'do_not_adopt_failure_not_reproduced_in_two_cases')

    def test_extra_tokens_and_missing_usage_cannot_be_hidden_by_call_counts(self):
        for missing in (False, True):
            rs = records()
            for r in rs:
                if r['variant'] == 'copy_observation':
                    r['usage']['tokenSubtotal']['totalTokens'] = 1500
                    if missing:
                        r['usage']['completeTokenUsage'] = False
            expected = 'inconclusive_usage' if missing else 'do_not_adopt_resource_budget'
            self.assertEqual(MODULE.gate(rs, 2), expected)

    def test_goal_contract_and_multiline_instruction_remain_one_repl_request(self):
        case = {'source':'source.py', 'contract':'空入力は空出力。\nUnicodeを保つ。', 'commands':[['python3','-m','unittest']]}
        goal = MODULE.EVAL.build_goal(case, 'Copy observed content.\n/exit')
        self.assertEqual(len(goal.splitlines()), 1)
        self.assertIn('Unicode', goal)
        self.assertIn('입력·출력 계약', goal)
        self.assertTrue(goal.endswith('Copy observed content. /exit'))
