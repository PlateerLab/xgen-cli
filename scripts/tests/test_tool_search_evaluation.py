"""Checks for evaluation validity rather than an engine-specific success case."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT=Path(__file__).resolve().parents[2]
SPEC=importlib.util.spec_from_file_location('tool_search_eval',ROOT/'scripts/evaluate-tool-search.py')
EVAL=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(EVAL)


class EvaluationTests(unittest.TestCase):
    def test_rank_metrics_use_position_and_top5_cutoff(self):
        rows=[{'tool':str(i)} for i in range(8)]
        self.assertEqual(EVAL.rank_metrics(rows,'0'),{'rank':1,'top1':True,'top5':True,'rr5':1.0})
        self.assertEqual(EVAL.rank_metrics(rows,'4')['rr5'],0.2)
        self.assertFalse(EVAL.rank_metrics(rows,'5')['top5'])
        self.assertEqual(EVAL.rank_metrics(rows,'missing')['rr5'],0.0)

    def test_model_material_excludes_gold_and_extra_case_fields(self):
        case={'query':'Which tool reads the resource?','expected_tool':'SECRET_GOLD_LABEL','expected_path':'/label-only','private_extra':'must-not-transfer'}
        for operation in ['query','select-fixed','select-generated']:
            prompt,body=EVAL.model_input(case,[{'name':'actual-candidate'}],operation)
            encoded=json.dumps(body)
            self.assertNotIn('SECRET_GOLD_LABEL',encoded)
            self.assertNotIn('label-only',encoded)
            self.assertNotIn('must-not-transfer',encoded)
            self.assertIn(case['query'],encoded)
            self.assertIn('JSON',prompt)

    def test_registration_rejects_binary_or_source_change_before_network(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory);binary=path/'binary';binary.write_bytes(b'first executable')
            config=EVAL.freeze(binary,path/'result')
            self.assertEqual(EVAL.validate_registration(binary,path/'result'),config)
            binary.write_bytes(b'changed executable')
            with self.assertRaisesRegex(ValueError,'changed'):
                EVAL.validate_registration(binary,path/'result')
            binary.write_bytes(b'first executable')
            registration=json.loads((path/'result/registration.json').read_bytes())
            registration['source_files']['scripts/evaluate-tool-search.py']='0'*64
            EVAL.write(path/'result/registration.json',registration)
            with self.assertRaisesRegex(ValueError,'changed'):
                EVAL.validate_registration(binary,path/'result')

    def test_live_is_blocked_before_credential_or_model_access_on_transfer_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/'binary';binary.write_bytes(b'fixed executable')
            EVAL.freeze(binary,root/'result')
            summary={k:0 for k in ('direct_cli_mismatches','describe_mismatches','agent_search_mismatches','agent_describe_mismatches','agent_failures','unstable_cases')}
            summary['agent_describe_mismatches']=1
            EVAL.write(root/'result/summary.json',summary)
            with patch.dict('sys.modules',{'graph_tool_call':SimpleNamespace(ToolGraph=None)}):
                with self.assertRaisesRegex(ValueError,'transport evaluation failed'):
                    EVAL.live(binary,root/'result')
            self.assertFalse((root/'result/live-registration.json').exists())

    def test_cases_have_independent_intents_and_paired_languages(self):
        cases=json.loads((EVAL.FIXTURES/'cases.json').read_bytes())
        self.assertEqual(len({c['id'] for c in cases}),32)
        self.assertEqual({c['system'] for c in cases},{'gitea','wiremock'})
        for system in {'gitea','wiremock'}:
            rows=[c for c in cases if c['system']==system]
            self.assertEqual(len({c['expected_path'] for c in rows}),8)
            for path in {c['expected_path'] for c in rows}:
                self.assertEqual({c['language'] for c in rows if c['expected_path']==path},{'en','ko'})
        self.assertTrue(all(c['expected_method']=='get' and c['split']=='validation' for c in cases))


if __name__=='__main__': unittest.main()
