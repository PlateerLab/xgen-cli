#!/usr/bin/env python3
"""Compare unconditional and quote-only readback across matched synthetic source sizes."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import statistics
import tempfile

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('projects', HERE / 'evaluate-projects.py')
EVAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVAL)
VARIANTS = ('baseline', 'always', 'quote_only')


def response_pass(record):
    presentation = record.get('exact_final_source_quote') if record['response_mode'] == 'quote' else record.get('source_blocks_omitted')
    return bool(record['outcome'] == 'pass' and presentation
                and record.get('command_claims_match') and record.get('changed_file_claims_match'))


def summarize(records):
    groups = {}
    for record in records:
        key = f"{record['family']}/{record['size']}/{record['response_mode']}"
        groups.setdefault(key, {v: [] for v in VARIANTS})[record['variant']].append(record)
    def mean(rs, get):
        values = [get(r) for r in rs]
        return statistics.mean(values) if values and all(v is not None for v in values) else None
    return {key: {variant: {
        'trials':len(rs), 'workflow_passes':sum(r['outcome']=='pass' for r in rs),
        'response_passes':sum(response_pass(r) for r in rs),
        'all_sources_read_back':sum(r.get('final_source_observed_after_execution',False) for r in rs),
        'mean_calls':mean(rs,lambda r:r.get('model_calls')),
        'mean_total_tokens':mean(rs,lambda r:r['usage']['tokenSubtotal']['totalTokens'] if r.get('usage',{}).get('completeTokenUsage') else None),
        'mean_driver_seconds':mean(rs,lambda r:r.get('driver_elapsed_seconds')),
        'mean_provider_millis':mean(rs,lambda r:r.get('usage',{}).get('elapsedMillisSubtotal')),
    } for variant, rs in variants.items()} for key, variants in groups.items()}


def gate(records, attempts):
    groups = summarize(records)
    if len(groups)!=8 or any(g[v]['trials']!=attempts for g in groups.values() for v in VARIANTS):
        return 'inconclusive_incomplete'
    if any(r['outcome'] in {'timeout_no_retry','observation_failure_no_retry','invalid_fixture','run_not_created'} for r in records):
        return 'inconclusive_observation'
    profiles={(r.get('model'),r.get('request_profile_digest')) for r in records}
    if len(profiles)!=1 or any(None in p for p in profiles):return 'inconclusive_profile_change'
    if any(r.get('unknown_events')!=0 or r.get('duplicate_effect_starts')!=0 for r in records):
        return 'do_not_adopt_safety_failure'
    for key,g in groups.items():
        base,candidate=g['baseline'],g['quote_only']
        if candidate['workflow_passes']<base['workflow_passes'] or candidate['response_passes']<base['response_passes']:
            return 'do_not_adopt_regression'
        if candidate['workflow_passes']!=attempts or candidate['response_passes']!=attempts:
            return 'do_not_adopt_response_boundary'
        cap=1.25 if key.endswith('/quote') else 1.05
        for metric in ('mean_calls','mean_total_tokens','mean_driver_seconds'):
            if base[metric] is None or candidate[metric] is None:return 'inconclusive_usage_or_latency'
            if candidate[metric]>base[metric]*(1.30 if metric=='mean_driver_seconds' else cap):
                return 'do_not_adopt_resource_budget'
    gained={r['family'] for r in records if r['variant']=='baseline' and r['response_mode']=='quote'
            and r['outcome']=='pass' and not response_pass(r)}
    if len(gained)<2:return 'do_not_adopt_gain_not_reproduced_in_two_families'
    for family in gained:
        related=[g for k,g in groups.items() if k.startswith(family+'/') and k.endswith('/quote')]
        if sum(g['quote_only']['response_passes'] for g in related)<=sum(g['baseline']['response_passes'] for g in related):
            return 'do_not_adopt_no_quote_gain'
    summaries=[g for k,g in groups.items() if k.endswith('/summary')]
    if any(g[v]['mean_total_tokens'] is None for g in summaries for v in ('always','quote_only')):
        return 'inconclusive_usage_or_latency'
    if sum(g['quote_only']['mean_total_tokens'] for g in summaries)>=sum(g['always']['mean_total_tokens'] for g in summaries):
        return 'do_not_adopt_no_scope_saving'
    return 'promising_quote_only_requires_broader_validation'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--attempts',type=int,default=2)
    parser.add_argument('--timeout',type=int,default=240)
    args=parser.parse_args()
    if args.attempts<2 or args.timeout<1:parser.error('two repetitions and positive timeout required')
    corpus=HERE.parent/'evals/response-scaling'
    cases=json.loads((corpus/'cases.json').read_text());EVAL.validate_cases(cases)
    keys={(c['family'],c['size'],c['response_mode']) for c in cases}
    if len(cases)!=8 or len(keys)!=8:parser.error('eight unique matched cells required')
    root=Path(tempfile.mkdtemp(prefix='xgen-response-scaling-'))
    inputs={'binary':args.binary.resolve(),'cases':corpus/'cases.json','builder':corpus/'build-cases.py',
            'always':corpus/'always.txt','quote_only':corpus/'quote-only.txt','runner':Path(__file__),
            'evaluator':HERE/'evaluate-projects.py'}
    hashes={k:EVAL.digest(p) for k,p in inputs.items()}
    registration={'hashes':hashes,'attempts':args.attempts,'timeout':args.timeout,
        'gate':'all eight cells: no regression and every quote-only response passes; two family quote gains; <=25% quote calls/tokens, <=5% summary calls/tokens, <=30% driver latency; quote-only summary token saving vs always',
        'limits':'synthetic preserved metadata for matched source sizes, not a real large-repository benchmark; active inference limits unchanged; no default prompt/profile change; no retries or monetary claim',
        'order':'rotate variants by case and repetition; reverse on even repetitions',
        'source_sizes':{c['id']:sum(len(c['files'][s].encode()) for s in c['sources']) for c in cases}}
    (root/'registration.json').write_text(json.dumps(registration,indent=2))
    snap=root/'source-snapshot';snap.mkdir()
    for k,p in inputs.items():
        if k!='binary':shutil.copyfile(p,snap/p.name)
    env={k:v for k,v in os.environ.items() if not k.startswith(('XGEN_','XGENY_','DEEPSEEK_'))}
    records=[];print('EXPERIMENT '+str(root),flush=True)
    for i,case in enumerate(cases):
        for attempt in range(1,args.attempts+1):
            offset=(i+attempt)%3;order=list(VARIANTS[offset:]+VARIANTS[:offset])
            if attempt%2==0:order.reverse()
            for variant in order:
                instruction='' if variant=='baseline' else inputs[variant].read_text().strip()
                trial=root/f"{case['id']}-{attempt}-{variant}"
                r=EVAL.evaluate(inputs['binary'],case,trial,args.timeout,env,instruction)
                r.update(family=case['family'],size=case['size'],response_mode=case['response_mode'],variant=variant,attempt=attempt)
                records.append(r)
                report={'registration':registration,'records':records,'summary':summarize(records),'decision':gate(records,args.attempts)}
                (root/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
                print(json.dumps({k:r.get(k) for k in ('case','variant','attempt','outcome','exact_final_source_quote','source_blocks_omitted','model_calls','diagnostic_codes')}),flush=True)
    if any(EVAL.digest(p)!=hashes[k] for k,p in inputs.items()):raise RuntimeError('registered input changed')
    print('DECISION '+report['decision'],flush=True);print('REPORT '+str(root/'report.json'),flush=True)


if __name__=='__main__':main()
