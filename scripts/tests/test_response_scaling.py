import importlib.util
from pathlib import Path
import unittest

SPEC=importlib.util.spec_from_file_location('scaling',Path(__file__).parents[1]/'experiment-response-scaling.py')
M=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(M)


def records():
    result=[]
    for family in ('python','node'):
        for size in ('small','large'):
            for mode in ('quote','summary'):
                for variant in M.VARIANTS:
                    for attempt in (1,2):
                        tokens=1000 if variant=='baseline' or mode=='summary' and variant=='quote_only' else 1150 if variant=='quote_only' else 1300
                        result.append({'case':f'{family}-{size}-{mode}','family':family,'size':size,'response_mode':mode,
                            'variant':variant,'attempt':attempt,'outcome':'pass','exact_final_source_quote':not(variant=='baseline' and attempt==1),
                            'source_blocks_omitted':True,'command_claims_match':True,'changed_file_claims_match':True,
                            'model_calls':6 if variant=='baseline' or mode=='summary' and variant=='quote_only' else 7,
                            'driver_elapsed_seconds':10,'unknown_events':0,'duplicate_effect_starts':0,
                            'model':'same-model','request_profile_digest':'same-profile',
                            'usage':{'completeTokenUsage':True,'tokenSubtotal':{'totalTokens':tokens},'elapsedMillisSubtotal':8000}})
    return result


class ResponseScalingTests(unittest.TestCase):
    def test_corpus_is_derived_from_the_builder_without_manual_fixture_sync(self):
        import json
        root=Path(__file__).parents[2]/'evals/response-scaling'
        spec=importlib.util.spec_from_file_location('builder',root/'build-cases.py')
        builder=importlib.util.module_from_spec(spec);spec.loader.exec_module(builder)
        self.assertEqual(builder.build(),json.loads((root/'cases.json').read_text()))

    def test_post_execution_raw_json_audit_does_not_relax_original_presentation_check(self):
        import json
        spec=importlib.util.spec_from_file_location('audit',Path(__file__).parents[1]/'audit-response-scaling.py')
        audit=importlib.util.module_from_spec(spec);spec.loader.exec_module(audit)
        commands=[{'argv':['python3','-m','unittest'],'exit_code':1}]
        claim=json.dumps({'commands':commands,'changed_files':['source.py']})
        self.assertFalse(M.EVAL.evidence_claims(claim,commands,['source.py'])['evidence_contract_valid'])
        encoding,result=audit.content_claims(claim,commands,['source.py'])
        self.assertEqual(encoding,'raw_json');self.assertTrue(result['command_claims_match'])
        self.assertTrue(result['changed_file_claims_match'])
        self.assertFalse(audit.content_claims(claim,commands,[]) [1]['changed_file_claims_match'])
        self.assertFalse(audit.content_claims('prefix '+claim,commands,['source.py'])[1]['evidence_contract_valid'])

    def test_joint_quality_and_scope_saving_with_bounded_resources(self):
        self.assertEqual(M.gate(records(),2),'promising_quote_only_requires_broader_validation')

    def test_one_large_response_boundary_cannot_be_hidden_by_aggregate_success(self):
        rs=records();r=next(r for r in rs if r['variant']=='quote_only' and r['size']=='large' and r['response_mode']=='quote')
        r['exact_final_source_quote']=False
        self.assertEqual(M.gate(rs,2),'do_not_adopt_response_boundary')

    def test_extra_summary_tokens_and_latency_are_rejected(self):
        for field in ('tokens','latency'):
            rs=records()
            for r in rs:
                if r['variant']=='quote_only' and r['response_mode']=='summary':
                    if field=='tokens':r['usage']['tokenSubtotal']['totalTokens']=1060
                    else:r['driver_elapsed_seconds']=14
            self.assertEqual(M.gate(rs,2),'do_not_adopt_resource_budget')

    def test_summary_does_not_require_a_source_quote_but_must_omit_code(self):
        r=records()[0];r.update(response_mode='summary',exact_final_source_quote=None)
        self.assertTrue(M.response_pass(r));r['source_blocks_omitted']=False
        self.assertFalse(M.response_pass(r))

    def test_multifile_goal_and_fixture_paths_are_validated(self):
        import json
        cases=json.loads((Path(__file__).parents[2]/'evals/response-scaling/cases.json').read_text());M.EVAL.validate_cases(cases)
        for mode in ('quote','summary'):
            case=next(c for c in cases if c['response_mode']==mode)
            goal=M.EVAL.build_goal(case)
            self.assertTrue(all(source in goal for source in case['sources']))
            self.assertEqual('소스 코드 블록과 파일 전체 내용은 넣지 말고' in goal,mode=='summary')
        case=dict(cases[0],sources=['../outside.py'])
        with self.assertRaises(ValueError):M.EVAL.validate_cases([case])
