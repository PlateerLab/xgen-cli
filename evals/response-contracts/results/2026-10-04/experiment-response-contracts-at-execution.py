#!/usr/bin/env python3
"""Compare one frozen response instruction on explicit fresh input/output contracts."""
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


def quality(record):
    return record['outcome'] == 'pass' and record.get('exact_final_source_quote', False)


def summarize(records):
    summary = {}
    for split in ('design', 'held_out'):
        summary[split] = {}
        for variant in ('baseline', 'copy_observation'):
            rs = [r for r in records if r['split'] == split and r['variant'] == variant]
            usages = [r['usage']['tokenSubtotal'] for r in rs if r.get('usage', {}).get('completeTokenUsage')]
            summary[split][variant] = {
                'trials': len(rs), 'workflow_passes': sum(r['outcome'] == 'pass' for r in rs),
                'exact_quote_passes': sum(quality(r) for r in rs),
                'final_source_observed': sum(r.get('final_source_observed_after_execution', False) for r in rs),
                'mean_calls': statistics.mean(r['model_calls'] for r in rs) if rs and all(r['model_calls'] is not None for r in rs) else None,
                'mean_total_tokens': statistics.mean(u['totalTokens'] for u in usages) if len(usages) == len(rs) and rs else None,
                'mean_cached_input_tokens': statistics.mean(u['cachedInputTokens'] for u in usages) if usages and all(u['cachedInputTokens'] is not None for u in usages) else None,
            }
    return summary


def gate(records, expected_per_group):
    summary = summarize(records)
    if any(summary[s][v]['trials'] != expected_per_group for s in summary for v in summary[s]):
        return 'inconclusive_incomplete'
    if any(r['outcome'] in {'timeout_no_retry', 'observation_failure_no_retry', 'invalid_fixture', 'run_not_created'} for r in records):
        return 'inconclusive_observation'
    profiles = {(r.get('model'), r.get('request_profile_digest')) for r in records}
    if len(profiles) != 1 or any(None in p for p in profiles):
        return 'inconclusive_profile_change'
    if any(r.get('unknown_events') or r.get('duplicate_effect_starts') for r in records):
        return 'do_not_adopt_safety_failure'
    for split in summary:
        base, candidate = summary[split]['baseline'], summary[split]['copy_observation']
        if candidate['workflow_passes'] < base['workflow_passes']:
            return 'do_not_adopt_workflow_regression'
        if candidate['exact_quote_passes'] < base['exact_quote_passes']:
            return 'do_not_adopt_quote_regression'
        if base['mean_total_tokens'] is None or candidate['mean_total_tokens'] is None:
            return 'inconclusive_usage'
        if candidate['mean_calls'] > base['mean_calls'] * 1.25 or candidate['mean_total_tokens'] > base['mean_total_tokens'] * 1.25:
            return 'do_not_adopt_resource_budget'
    reproduced = {r['case'] for r in records if r['split'] == 'design' and r['variant'] == 'baseline'
                  and r['outcome'] == 'pass' and not r['exact_final_source_quote']}
    if len(reproduced) < 2:
        return 'do_not_adopt_failure_not_reproduced_in_two_cases'
    if summary['design']['copy_observation']['exact_quote_passes'] <= summary['design']['baseline']['exact_quote_passes']:
        return 'do_not_adopt_no_design_gain'
    if summary['held_out']['copy_observation']['exact_quote_passes'] <= summary['held_out']['baseline']['exact_quote_passes']:
        return 'do_not_adopt_no_held_out_gain'
    return 'promising_requires_larger_validation'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--attempts', type=int, default=3)
    parser.add_argument('--timeout', type=int, default=240)
    args = parser.parse_args()
    if args.attempts < 2 or args.timeout < 1:
        parser.error('at least two repetitions and a positive timeout required')
    corpus = HERE.parent / 'evals/response-contracts/cases.json'
    candidate = HERE.parent / 'evals/response-contracts/copy-observation.txt'
    cases = json.loads(corpus.read_text())
    EVAL.validate_cases(cases)
    if any(len([c for c in cases if c['split'] == split]) != 2 for split in ('design', 'held_out')):
        parser.error('two fresh cases per split required')
    root = Path(tempfile.mkdtemp(prefix='xgen-response-contracts-'))
    binary = args.binary.resolve()
    inputs = {'binary': binary, 'cases': corpus, 'candidate': candidate, 'runner': Path(__file__), 'evaluator': HERE / 'evaluate-projects.py'}
    hashes = {name: EVAL.digest(path) for name, path in inputs.items()}
    registration = {'hashes': hashes, 'attempts': args.attempts, 'timeout': args.timeout,
        'gate': 'two distinct completed baseline design failures; design and held-out quote gains; no workflow/quote regression; <=25% calls and total tokens per split',
        'order': 'design then held_out; alternating variant order by case and attempt',
        'limits': 'one task-prompt candidate; no production engine/system prompt changes; no statistical significance or monetary saving claim'}
    (root / 'registration.json').write_text(json.dumps(registration, indent=2))
    snapshot = root / 'source-snapshot'; snapshot.mkdir()
    for name, path in inputs.items():
        if name != 'binary': shutil.copyfile(path, snapshot / path.name)
    environment = {k: v for k, v in os.environ.items() if not k.startswith(('XGEN_', 'XGENY_', 'DEEPSEEK_'))}
    records = []
    print('EXPERIMENT ' + str(root), flush=True)
    for split in ('design', 'held_out'):
        for index, case in enumerate(c for c in cases if c['split'] == split):
            for attempt in range(1, args.attempts + 1):
                variants = ['baseline', 'copy_observation']
                if (index + attempt) % 2 == 0: variants.reverse()
                for variant in variants:
                    trial = root / f"{case['id']}-{attempt}-{variant}"
                    record = EVAL.evaluate(binary, case, trial, args.timeout, environment,
                        candidate.read_text().strip() if variant == 'copy_observation' else '')
                    record.update(variant=variant, attempt=attempt)
                    records.append(record)
                    report = {'registration': registration, 'records': records, 'summary': summarize(records),
                              'decision': gate(records, args.attempts * 2)}
                    (root / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
                    print(json.dumps({k: record.get(k) for k in ('case','attempt','variant','outcome','exact_final_source_quote','model_calls')}), flush=True)
    if any(EVAL.digest(path) != hashes[name] for name, path in inputs.items()):
        raise RuntimeError('registered input changed during experiment')
    print('DECISION ' + report['decision'], flush=True)
    print('REPORT ' + str(root / 'report.json'), flush=True)


if __name__ == '__main__':
    main()
