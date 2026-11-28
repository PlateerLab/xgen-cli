#!/usr/bin/env python3
"""Evaluate a frozen coding task matrix through the public xgen run/resume CLI."""
import argparse
from collections import Counter
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import random
import re
import shutil
import sqlite3
import statistics
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('baseline_support', ROOT / 'scripts/evaluate-planning-model.py')
SUPPORT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SUPPORT)
FAMILIES = {'repository-question', 'bug-fix', 'multifile-rename', 'failure-recovery', 'approval-resume'}


def validate_cases(cases):
    if not isinstance(cases, list) or not cases:
        raise ValueError('nonempty cases required')
    identifiers = set()
    for case in cases:
        if set(case) != {'id', 'split', 'family', 'goal', 'files', 'writable', 'oracle'}:
            raise ValueError('explicit baseline case contract required')
        if (not isinstance(case['id'], str) or not re.fullmatch('[a-z0-9-]+', case['id'])
                or case['id'] in identifiers):
            raise ValueError('unique simple case identifiers required')
        identifiers.add(case['id'])
        if case['split'] not in {'design', 'held_out'} or case['family'] not in FAMILIES:
            raise ValueError('unknown case split or family')
        if not isinstance(case['goal'], str) or not case['goal'].strip():
            raise ValueError('nonempty goal required')
        if (not isinstance(case['files'], dict) or not case['files']
                or not all(isinstance(content, str) for content in case['files'].values())
                or not isinstance(case['writable'], list)
                or len(set(case['writable'])) != len(case['writable'])
                or not set(case['writable']) <= set(case['files'])):
            raise ValueError('explicit existing writable files required')
        for name in case['files']:
            SUPPORT.relative(name)
        if (any(Path(a) in Path(b).parents for a in case['files'] for b in case['files'])
                or not isinstance(case['oracle'], str) or not case['oracle'].strip()):
            raise ValueError('distinct files and independent oracle required')
        if (case['family'] == 'repository-question') != (not case['writable']):
            raise ValueError('only repository questions are read-only')
    expected = {(family, split) for family in FAMILIES for split in ['design', 'held_out']}
    if {(case['family'], case['split']) for case in cases} != expected:
        raise ValueError('each family needs design and held-out cases')


def validate_config(config, cases):
    # Reuse the existing strict quote, token bound and inference option validation.
    adapted = [dict(case, split='validation' if case['split'] == 'held_out' else 'design',
                    generated_dirs=[], executables=['python3'],
                    oracle={'python': case['oracle']}) for case in cases]
    SUPPORT.validate(config, adapted)
    if any(not isinstance(config[field], str) or not config[field].strip() for field in ['model', 'tokenizer']):
        raise ValueError('exact model and tokenizer required')
    if (not isinstance(config['quote']['source'], str) or not config['quote']['source'].strip()
            or type(config['seed']) is not int):
        raise ValueError('explicit quote source and integer seed required')
    if config.get('conditions') is not None or config.get('final_response_schema') is not None:
        raise ValueError('baseline uses the public CLI without experimental conditions')


def schedule(cases, repeats, seed):
    plan = [(case, repeat) for case in cases for repeat in range(1, repeats + 1)]
    random.Random(seed).shuffle(plan)
    return plan


def runtime_metrics(events, outputs, recipes):
    metrics = SUPPORT.ORACLES.safety_metrics(events)
    seen_actions, seen_results = Counter(), Counter()
    for output in outputs:
        # Compare observed argument/output digests, never raw commands or content.
        invocation = output['invocation']
        arguments = recipes[output['stepId']]['arguments']
        key = (invocation['capabilityId'], invocation['contractVersion'],
               hashlib.sha256(SUPPORT.canonical(arguments)).hexdigest())
        seen_actions[key] += 1
        seen_results[(*key, output['outputDigest'])] += 1
    metrics.update(
        repeated_finished_actions=sum(count - 1 for count in seen_actions.values()),
        repeated_identical_results=sum(count - 1 for count in seen_results.values()),
        proposal_rejections=sum(event['type'] == 'model_call_settled'
            and event['settlement'].get('reason') == 'proposal_rejected' for event in events),
    )
    return metrics


def completion_flags(candidate, oracle_passed, scope_preserved):
    completed = bool(candidate and candidate.get('responseKind', 'task_completion') == 'task_completion')
    return {'task_completed': completed,
            'false_completion': completed and not (oracle_passed and scope_preserved)}


def trial(binary, case, repeat, directory, config, proxy):
    directory.mkdir(mode=0o700)
    workspace, scratch, state = [directory / name for name in ['workspace', 'scratch', 'state']]
    workspace.mkdir()
    scratch.mkdir()
    for name, content in case['files'].items():
        path = workspace / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding='utf-8')
    original = SUPPORT.inventory(workspace)
    environment = SUPPORT.child_environment(state, scratch)
    environment.update(XGEN_CONFIG_HOME=str(scratch / 'config'), PYTHONDONTWRITEBYTECODE='1',
                       XGEN_OPENAI_RESPONSE_FORMAT=config['response_format'],
                       XGEN_OPENAI_THINKING=config['thinking'],
                       XGEN_OPENAI_MAX_OUTPUT_TOKENS=str(config['max_output_tokens']),
                       XGEN_OPENAI_INFERENCE_TIMEOUT=str(config['request_timeout_seconds']))
    trial_id = directory.name
    proxy.trials.add(trial_id)
    endpoint = f'http://127.0.0.1:{proxy.server_port}/{trial_id}/v1'
    common = ['--workspace', str(workspace), '--allow-dir', '.', '--allow-remote-model-egress',
              '--allow-read', '--allow-executable', f'python3={Path(sys.executable).resolve()}',
              '--base-url', endpoint]
    if case['writable']:
        common.append('--allow-write')
    if case['family'] != 'approval-resume':
        common.append('--allow-execute')
    command = [str(binary), 'run', *common, '--model', config['model'], '--tokenizer', config['tokenizer'],
               '--max-model-turns', str(config['max_model_turns']), '--max-ticks', str(config['max_ticks']), case['goal']]
    record = {'case': case['id'], 'split': case['split'], 'family': case['family'], 'repeat': repeat,
              'trial': trial_id, 'outcome': 'incomplete', 'automatic_retries': 0,
              'task_completed': False, 'false_completion': None, 'accepted': False,
              'model_calls': None, 'repeated_finished_actions': None, 'evidence_complete': False}
    started = time.monotonic()
    database = None
    summary = ''

    def invoke(arguments):
        remaining = config['trial_timeout_seconds'] - (time.monotonic() - started)
        if remaining <= 0:
            raise subprocess.TimeoutExpired('baseline trial', config['trial_timeout_seconds'])
        return SUPPORT.run_bounded(arguments, workspace, environment, remaining)

    try:
        result = invoke(command)
        databases = list((state / 'runs').glob('*/run.sqlite3'))
        if len(databases) != 1:
            record['outcome'] = 'run_not_created'
        else:
            database = databases[0]
            if case['family'] == 'approval-resume':
                before = SUPPORT.ORACLES.inspect(database)
                record['approval_pause_observed'] = (result.returncode == 10
                    and b'reason=execute_approval_required' in result.stderr)
                record['process_outputs_before_approval'] = sum(
                    output['invocation']['capabilityId'] == 'xgeny.process/execute' for output in before[4])
                if record['approval_pause_observed']:
                    result = invoke([str(binary), 'resume', database.parent.name, *common, '--allow-execute',
                                     '--max-ticks', str(config['max_ticks'])])
                else:
                    record['outcome'] = 'expected_pause_missing'
            record['exit_code'] = result.returncode
            record['diagnostic_codes'] = sorted(set(re.findall(rb'(?:reason|code)=([a-z0-9_.]+)', result.stderr)))
            record['diagnostic_codes'] = [code.decode('ascii') for code in record['diagnostic_codes']]
    except subprocess.TimeoutExpired:
        record['outcome'] = 'timeout_no_retry'
    except (OSError, ValueError, sqlite3.Error, KeyError, TypeError):
        record['outcome'] = 'execution_observation_failed'
    finally:
        record['driver_elapsed_seconds'] = time.monotonic() - started
        try:
            databases = list((state / 'runs').glob('*/run.sqlite3'))
            if len(databases) != 1:
                raise ValueError('one Run required')
            database = databases[0]
            projection, events, summaries, receipts, outputs = SUPPORT.ORACLES.inspect(database)
            summary = summaries[-1] if summaries else ''
            with closing(sqlite3.connect(f'file:{database.parent / "materials.sqlite3"}?mode=ro', uri=True)) as connection:
                recipes = [SUPPORT.strict_json(row[0]) for row in connection.execute('SELECT record FROM material_recipe')]
            record.update(runtime_metrics(events, outputs, {recipe['stepId']: recipe for recipe in recipes}))
            evidence = SUPPORT.receipt_command_evidence(database, outputs)
            record['failed_then_passed_same_command'] = any(
                a['exit_code'] != 0 and b['exit_code'] == 0 and a['argv'] == b['argv']
                for i, a in enumerate(evidence) for b in evidence[i + 1:])
            record['receipt_count'] = receipts
            candidate = (projection.get('agentLoop') or {}).get('completionCandidate')
            record['task_completed'] = completion_flags(candidate, False, False)['task_completed']
            record['evidence_complete'] = True
            if record['task_completed']:
                before_db = SUPPORT.digest(database)
                usage_path = database.parent / 'usage.sqlite3'
                before_usage = SUPPORT.digest(usage_path)
                # Offline replay gets no endpoint, model profile, workspace or executable grants.
                replay = SUPPORT.run_bounded([str(binary), 'resume', database.parent.name], scratch,
                                             environment, 15)
                record['offline_replay'] = (replay.returncode == 0 and replay.stdout.decode() == summary
                    and SUPPORT.digest(database) == before_db and SUPPORT.digest(usage_path) == before_usage)
        except (OSError, ValueError, sqlite3.Error, KeyError, TypeError, subprocess.TimeoutExpired):
            record['evidence_complete'] = False
        try:
            current = SUPPORT.inventory(workspace)
            changed = {name for name in original.keys() | current.keys() if original.get(name) != current.get(name)}
            record['scope_preserved'] = (changed <= set(case['writable'])
                and not any(path.is_symlink() for path in workspace.rglob('*')))
            # Oracles and expected answers stay outside the model's readable workspace.
            answer_path, oracle_path = scratch / 'answer.json', scratch / 'oracle.py'
            answer_path.write_text(summary, encoding='utf-8')
            oracle_path.write_text(case['oracle'], encoding='utf-8')
            check = SUPPORT.run_bounded([sys.executable, '-I', '-B', str(oracle_path), str(workspace), str(answer_path)],
                                        scratch, environment, 15)
            record['oracle_passed'] = check.returncode == 0
            if record['evidence_complete']:
                record['false_completion'] = record['task_completed'] and not (
                    record['oracle_passed'] and record['scope_preserved'])
            accepted = (record['evidence_complete'] and record['task_completed'] and record['oracle_passed']
                        and record['scope_preserved'] and record.get('offline_replay', False)
                        and record.get('unknown_events') == 0 and record.get('duplicate_effect_starts') == 0)
            if case['family'] == 'approval-resume':
                accepted = accepted and record.get('approval_pause_observed', False) and record.get('process_outputs_before_approval') == 0
            if case['family'] == 'failure-recovery':
                accepted = accepted and record.get('failed_then_passed_same_command', False)
            record['accepted'] = bool(accepted and record.get('exit_code') == 0)
            if record['accepted']:
                record['outcome'] = 'pass'
            elif record['outcome'] == 'incomplete':
                record['outcome'] = 'workflow_failure'
        except (OSError, ValueError, subprocess.TimeoutExpired):
            record['outcome'] = 'oracle_observation_failed'
        record['elapsed_seconds'] = time.monotonic() - started
    return record


def attach_cost(record, ledger):
    calls = [call for call in ledger.calls if call['trial'] == record['trial']]
    attempts = [attempt for attempt in ledger.attempts if attempt['trial'] == record['trial']]
    unknown = [item for item in attempts if item['call_id'] in ledger.reservations]
    record['upstream_attempts'] = len(attempts)
    record['unknown_cost_requests'] = len(unknown)
    record['unknown_reserved_nano_usd'] = sum(ledger.reservations[item['call_id']] for item in unknown)
    record['known_cost_nano_usd'] = sum(call['budget_cost_nano_usd'] or 0 for call in calls)
    record['actual_cost_nano_usd'] = (record['known_cost_nano_usd'] if not unknown
        and ledger.config['quote']['kind'] == 'fixed_tier' else None)
    record['token_subtotals'] = {key: sum(call['usage'][key] for call in calls
        if call['usage'] is not None and call['usage'][key] is not None) for key in ['input', 'output', 'cache']}
    record['complete_token_usage'] = (len(calls) == len(attempts) and all(
        call['usage'] is not None and call['usage']['cache'] is not None for call in calls))


def quality_aggregate(records, planned):
    groups = {}
    for split in ['design', 'held_out']:
        for family in sorted(FAMILIES):
            rows = [r for r in records if r['split'] == split and r['family'] == family]
            groups[f'{split}/{family}'] = {
                'attempted': len(rows), 'passed': sum(r['accepted'] for r in rows),
                'false_completions': sum(r['false_completion'] is True for r in rows),
                'unknown_completion_checks': sum(r['false_completion'] is None for r in rows),
                'median_seconds': statistics.median(r['elapsed_seconds'] for r in rows) if rows else None,
                'repeated_finished_actions': sum(r['repeated_finished_actions'] or 0 for r in rows),
                'model_calls_subtotal': sum(r.get('model_calls') or 0 for r in rows),
                'incomplete_evidence_trials': sum(not r.get('evidence_complete', False) for r in rows),
                'token_subtotals': {key: sum(r['token_subtotals'][key] for r in rows)
                                   for key in ['input', 'output', 'cache']},
                'complete_token_usage': all(r['complete_token_usage'] for r in rows),
            }
    return {'trials_planned': planned, 'trials_executed': len(records),
            'complete_schedule': len(records) == planned, 'groups': groups,
            'passed': sum(r['accepted'] for r in records),
            'outcomes': dict(Counter(r['outcome'] for r in records)),
            'model_calls_subtotal': sum(r.get('model_calls') or 0 for r in records),
            'repeated_finished_actions': sum(r['repeated_finished_actions'] or 0 for r in records),
            'repeated_identical_results': sum(r.get('repeated_identical_results') or 0 for r in records),
            'token_subtotals': {key: sum(r['token_subtotals'][key] for r in records)
                               for key in ['input', 'output', 'cache']},
            'complete_token_usage': all(r['complete_token_usage'] for r in records)}


def aggregate(records, planned, ledger):
    result = quality_aggregate(records, planned)
    for key, group in result['groups'].items():
        split, family = key.split('/')
        group['known_cost_nano_usd'] = sum(r['known_cost_nano_usd'] for r in records
                                         if r['split'] == split and r['family'] == family)
    result.update(known_cost_nano_usd=ledger.committed,
                  unknown_reserved_nano_usd=sum(ledger.reservations.values()),
                  actual_cost_nano_usd=ledger.committed if not ledger.reservations
                      and ledger.config['quote']['kind'] == 'fixed_tier' else None,
                  aborted=ledger.aborted, abort_reason=ledger.abort_reason)
    return result


def execute(binary, cases, config, output, upstream, key=None, split='all'):
    validate_cases(cases)
    validate_config(config, cases)
    if split not in {'all', 'design', 'held_out'}:
        raise ValueError('unknown selected split')
    config, cases = SUPPORT.strict_json(SUPPORT.canonical(config)), SUPPORT.strict_json(SUPPORT.canonical(cases))
    if not Path('/proc').is_dir() or not hasattr(os, 'pidfd_open'):
        raise ValueError('Linux pidfd process-tree cleanup required')
    if not SUPPORT.utc(config['quote']['valid_from_utc']) <= datetime.now(timezone.utc) < SUPPORT.utc(config['quote']['valid_until_utc']):
        raise ValueError('quote window inactive')
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    ledger = SUPPORT.Ledger(config, output / 'cost-ledger.jsonl')
    proxy = SUPPORT.Proxy(config, ledger, upstream, key)
    plan = schedule([case for case in cases if split == 'all' or case['split'] == split], config['repeats'], config['seed'])
    pinned = output / 'xgen'
    binary_hash = SUPPORT.digest(binary)
    shutil.copy2(binary, pinned)
    if SUPPORT.digest(pinned) != binary_hash:
        raise ValueError('binary changed during pinning')
    source_paths = [Path(__file__), ROOT / 'scripts/evaluate-planning-model.py', ROOT / 'scripts/evaluate-projects.py']
    source_hashes = {path.name: SUPPORT.digest(path) for path in source_paths}
    manifest = {'format_version': 1, 'binary_sha256': binary_hash, 'python_sha256': SUPPORT.digest(Path(sys.executable).resolve()),
                'runner_sources': source_hashes, 'selected_split': split,
                'cases_sha256': hashlib.sha256(SUPPORT.canonical(cases)).hexdigest(),
                'config': config, 'schedule': [{'case': case['id'], 'repeat': repeat} for case, repeat in plan],
                'automatic_retries': 0, 'entrypoint': 'public-run-resume',
                'result_kind': 'live' if key is not None or upstream.startswith('https://') else 'loopback-contract-test'}
    (output / 'manifest.json').write_bytes(SUPPORT.canonical(manifest))
    (output / 'cases.json').write_bytes(SUPPORT.canonical(cases))
    worker = threading.Thread(target=proxy.serve_forever, daemon=True)
    worker.start()
    records = []
    stop_reason = None
    try:
        for index, (case, repeat) in enumerate(plan):
            now = datetime.now(timezone.utc)
            bound = SUPPORT.price_nano({'input': config['max_input_tokens'], 'output': config['max_output_tokens'], 'cache': 0}, config['quote'])
            if any(SUPPORT.digest(path) != source_hashes[path.name] for path in source_paths):
                stop_reason = 'runner_source_changed'
                break
            if (ledger.aborted or ledger.committed + sum(ledger.reservations.values()) + bound > config['spend_cap_nano_usd']
                    or not SUPPORT.utc(config['quote']['valid_from_utc']) <= now < SUPPORT.utc(config['quote']['valid_until_utc'])):
                stop_reason = ledger.abort_reason or 'budget_or_quote_blocked'
                break
            directory = output / f'trial-{index + 1:03d}'
            record = trial(pinned, case, repeat, directory, config, proxy)
            records.append(record)
            attach_cost(record, ledger)
            (directory / 'result.json').write_bytes(SUPPORT.canonical(record))
            (output / 'trials.jsonl').write_bytes(b''.join(SUPPORT.canonical(row) + b'\n' for row in records))
            (output / 'summary.json').write_bytes(SUPPORT.canonical(aggregate(records, len(plan), ledger)))
            if record['outcome'] == 'timeout_no_retry' or ledger.reservations:
                stop_reason = 'timeout_or_unknown_cost'
                break
    finally:
        # shutdown waits for an in-flight HTTP response; no subsequent trial can reuse its ID.
        proxy.shutdown()
        proxy.server_close()
        worker.join()
    # Reconcile late responses with the original trial before publishing final totals.
    for record in records:
        attach_cost(record, ledger)
        (output / record['trial'] / 'result.json').write_bytes(SUPPORT.canonical(record))
    summary = aggregate(records, len(plan), ledger)
    summary['stop_reason'] = stop_reason
    summary['runner_sources_unchanged'] = all(SUPPORT.digest(path) == source_hashes[path.name] for path in source_paths)
    (output / 'trials.jsonl').write_bytes(b''.join(SUPPORT.canonical(row) + b'\n' for row in records))
    (output / 'summary.json').write_bytes(SUPPORT.canonical(summary))
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--cases', type=Path, default=ROOT / 'evals/coding-baseline/cases.json')
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--upstream', default=os.environ.get('XGEN_EVAL_BASE_URL'))
    parser.add_argument('--split', choices=['all', 'design', 'held_out'], default='all')
    args = parser.parse_args()
    try:
        config = SUPPORT.strict_json(args.config.read_bytes())
        cases = SUPPORT.strict_json(args.cases.read_bytes())
        key = os.environ.get('XGEN_OPENAI_API_KEY')
        if not args.upstream:
            raise ValueError('upstream required')
        if args.upstream.startswith('https://') and not key:
            raise ValueError('local XGEN_OPENAI_API_KEY required')
        summary = execute(args.binary.resolve(strict=True), cases, config, args.output.resolve(), args.upstream, key, args.split)
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        # Inputs, provider URLs and credentials must never be interpolated into diagnostics.
        parser.exit(2, 'baseline preflight failed; check local config, limits, quote and authentication\n')
    print(json.dumps(summary, ensure_ascii=False))
    return 0 if (summary['complete_schedule'] and summary['runner_sources_unchanged']
                 and summary['passed'] == summary['trials_planned']) else 1


if __name__ == '__main__':
    raise SystemExit(main())
