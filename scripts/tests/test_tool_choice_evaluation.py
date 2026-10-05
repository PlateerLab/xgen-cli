"""Validity tests for the standalone evaluation; no model or business API calls."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[2]
SPEC=importlib.util.spec_from_file_location('choice_eval',ROOT/'scripts/evaluate-tool-choice.py')
EVAL=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(EVAL)


class ToolChoiceTests(unittest.TestCase):
    def test_model_material_excludes_all_oracle_fields(self):
        case={'query':'Read the resource','system':'public-test','primary_tool':'SECRET_GOLD','accepted_tools':['LABEL_ALTERNATIVE'],'acceptance_reason':'DO_NOT_TRANSFER','private_note':'PRIVATE'}
        for operation in ['query','select-fixed','select-generated']:
            prompt,body=EVAL.model_input(case,[{'tool':{'name':'available'}}],operation)
            self.assertEqual(set(body),{'request','collection'} if operation=='query' else {'request','collection','candidates'})
            encoded=json.dumps(body)
            for forbidden in ['SECRET_GOLD','LABEL_ALTERNATIVE','DO_NOT_TRANSFER','PRIVATE']:
                self.assertNotIn(forbidden,encoded)
            self.assertIn('JSON',prompt)

    def test_primary_alternative_null_and_outside_candidates_are_separate(self):
        candidates=[{'tool':{'name':'first'}},{'tool':{'name':'alternative'}}]
        result=EVAL.selection({'tool':'alternative'},candidates,'first',['first','alternative'])
        self.assertTrue(result['accepted_correct']);self.assertFalse(result['primary_correct'])
        for value in [{'tool':'invented'},{'tool':False},{'tool':'first','extra':1},None]:
            self.assertFalse(EVAL.selection(value,candidates,'first',['first'])['valid'])
        self.assertTrue(EVAL.selection({'tool':None},[],None,[])['accepted_correct'])
        self.assertFalse(EVAL.selection({'tool':None},[], 'missing',['missing'])['accepted_correct'])

    def test_conditional_denominator_excludes_missing_candidates_and_negatives(self):
        class Ledger:
            calls=[];response_identity=None;committed=0;reservations={}
        records=[]
        for present,correct in [(True,True),(True,False),(False,False)]:
            record={'id':str(len(records)),'system':'gitea','language':'en','query_valid':True,'primary_tool':'target'}
            for condition in ['fixed','generated']:
                record[condition]={'candidate_present':present,'primary_correct':correct,'accepted_correct':correct,'valid':True,'tool':'target' if correct else None}
                record[condition+'_primary_present']=present
            records.append(record)
        result=EVAL.summarize(records,Ledger(),'frozen')
        self.assertEqual(result['groups']['gitea-en']['fixed']['conditional_n'],2)
        self.assertEqual(result['groups']['gitea-en']['fixed']['conditional_correct'],1)

    def test_freeze_blocks_changes_before_network(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/'binary';binary.write_bytes(b'first')
            config=EVAL.freeze(binary,root/'output')
            self.assertEqual(EVAL.registered(binary,root/'output'),config)
            binary.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'changed'):EVAL.registered(binary,root/'output')

    def test_registered_graph_mutation_blocks_before_live(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/'binary';binary.write_bytes(b'first')
            output=root/'output';EVAL.freeze(binary,output)
            graph=output/'gitea-graph.json';graph.write_bytes(b'original')
            EVAL.write(output/'transport.json',{'graph_sha256':{'gitea':EVAL.SEARCH.sha(graph)}})
            EVAL.registered(binary,output)
            graph.write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError,'graph changed'):EVAL.registered(binary,output)

    def test_case_intents_are_new_and_negative_labels_are_explicit(self):
        cases=json.loads((EVAL.FIXTURES/'cases.json').read_bytes());EVAL.validate_cases(cases)
        old=json.loads((ROOT/'scripts/fixtures/tool-search-evaluation/cases.json').read_bytes())
        old_paths={(c['system'],c['expected_path']) for c in old}
        self.assertEqual(len(cases),20)
        self.assertEqual(sum(c['primary'] is None for c in cases),4)
        for case in cases:
            if case['primary']:self.assertNotIn((case['system'],case['primary']['path']),old_paths)
            paired=[c for c in cases if c['id'].rsplit('-',1)[0]==case['id'].rsplit('-',1)[0]]
            self.assertEqual({c['language'] for c in paired},{'en','ko'})
        invalid=[dict(cases[0],extra=True)]
        with self.assertRaises(ValueError):EVAL.validate_cases(invalid)


if __name__=='__main__':unittest.main()
