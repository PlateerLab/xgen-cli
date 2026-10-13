#!/usr/bin/env python3
"""Read-only pilot aggregation and relational journal audit (not cryptographic verification)."""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import sqlite3
import statistics


def read_json(path):
    return json.loads(Path(path).read_bytes())


def checksum(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def claim_failure(database, record):
    """Categorize existing strict-contract failures without changing their score."""
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as connection:
        summaries = [json.loads(row[0])['summary'] for row in
                     connection.execute('SELECT record_json FROM completion_outputs ORDER BY event_sequence')]
    reason = 'contract_format'
    claim = None
    if len(summaries) == 1:
        try:
            claim = json.loads(summaries[0])
            if isinstance(claim, dict) and set(claim) == {'commands', 'changed_files'}:
                expected = record['observed_commands']
                commands = claim['commands']
                changed = sorted(claim['changed_files']) == sorted(record['changed_files'])
                if commands != expected:
                    canonical = lambda items: sorted(json.dumps(x, sort_keys=True) for x in items)
                    reason = 'command_order' if canonical(commands) == canonical(expected) else 'command_content'
                    if not changed:
                        reason += '_and_changed_files'
                elif not changed:
                    reason = 'changed_files'
                else:
                    reason = 'strict_contract_other'
        except (ValueError, TypeError):
            pass
    return {'trial': record['trial'], 'case': record['case'], 'condition': record['condition'],
            'split': record['split'], 'reason': reason, 'observed_commands': record['observed_commands'],
            'observed_changed_files': record['changed_files'], 'claimed_summary': summaries,
            'note': 'false_claim includes response-format violations; it is not synonymous with fabricated facts'}


def audit(database):
    """Check prior authorization and retained invocation/output/receipt relationships."""
    errors = []
    starts = 0
    try:
        with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as connection:
            connection.row_factory = sqlite3.Row
            rows = list(connection.execute('SELECT * FROM run_events ORDER BY sequence'))
            heads = {row['sequence']: row['digest'] for row in rows}
            materials = {row['effect_id']: json.loads(row['record_json']) for row in
                         connection.execute('SELECT * FROM invocation_materials')}
            outputs = {row['effect_id']: json.loads(row['record_json']) for row in
                       connection.execute('SELECT * FROM tool_outputs')}
            receipts = {row['effect_id']: json.loads(row['receipt_json']) for row in
                        connection.execute('SELECT * FROM execution_receipts')}
            consumptions = {row['effect_id']: dict(row) for row in
                            connection.execute('SELECT * FROM authorization_consumption')}
        with sqlite3.connect(f'file:{database.parent / "materials.sqlite3"}?mode=ro', uri=True) as connection:
            recipes = {(row[0], row[1]): json.loads(row[2]) for row in
                       connection.execute('SELECT reference_id, revision, record FROM material_recipe')}
        intents, plans, started, succeeded, verified = {}, {}, set(), {}, set()
        grant_uses = collections.Counter()

        def check(value, code, effect):
            if not value:
                errors.append({'effect': effect, 'code': code})

        for row in rows:
            event = json.loads(row['event_json'])
            body, sequence = event['body'], row['sequence']
            kind = body['type']
            if kind == 'plan_accepted':
                for step in body['steps']:
                    plans[step['stepId']] = (sequence, step.get('invocation'))
            elif kind == 'effect_intent_committed':
                effect = body['intent']['effectId']
                check(effect not in intents, 'duplicate_intent', effect)
                intents[effect] = (sequence, body['stepId'], body['intent'])
            elif kind == 'effect_execution_started':
                effect, step = body['effectId'], body['stepId']
                starts += 1
                check(effect not in started, 'duplicate_start', effect)
                started.add(effect)
                if effect not in intents:
                    check(False, 'missing_prior_intent', effect)
                    continue
                prior, intent_step, intent = intents[effect]
                check(prior < sequence and intent_step == step, 'intent_order_or_step', effect)
                grant = intent['authorization']
                binding = grant['binding']
                grant_uses[grant['grantId']] += 1
                check(grant_uses[grant['grantId']] <= grant['maxUses'], 'grant_overuse', effect)
                expected = dict(intent['invocation'], actionDigest=intent['actionDigest'],
                                runId=event['runId'], stepId=step, authority=event['authority'],
                                authorityEpoch=event['authorityEpoch'])
                check(all(binding.get(k) == v for k, v in expected.items()), 'authorization_binding', effect)
                issued = binding['issuedAtSequence']
                check(issued < prior and heads.get(issued) == binding['issuedAtHeadDigest'],
                      'authorization_head', effect)
                consumed = consumptions.get(effect, {})
                expected_consumed = {'grant_id': grant['grantId'], 'grant_digest': grant['grantDigest'],
                                     'action_digest': intent['actionDigest'], 'max_uses': grant['maxUses']}
                check(all(consumed.get(k) == v for k, v in expected_consumed.items()),
                      'authorization_consumption', effect)
                material = materials.get(effect, {})
                expected_material = dict(runId=event['runId'], stepId=step, effectId=effect,
                                         actionDigest=intent['actionDigest'], invocation=intent['invocation'],
                                         materialDigest=binding['materialDigest'])
                check(all(material.get(k) == v for k, v in expected_material.items()), 'material_binding', effect)
                plan_sequence, plan = plans.get(step, (sequence, None))
                check(plan_sequence < prior and plan is not None, 'prior_plan', effect)
                if plan is not None:
                    check(all(plan.get(k) == intent['invocation'][k] for k in
                              ('capabilityId', 'contractVersion', 'definitionDigest')) and
                          plan['actionDigest'] == intent['actionDigest'] and
                          plan['planInputDigest'] == binding['materialDigest'], 'plan_binding', effect)
                reference = material.get('retention', {}).get('reference', {})
                recipe = recipes.get((reference.get('referenceId'), reference.get('revision')), {})
                expected_recipe = dict(runId=event['runId'], stepId=step,
                                       materialDigest=binding['materialDigest'],
                                       capability={k: intent['invocation'][k] for k in ('capabilityId', 'contractVersion')})
                check(all(recipe.get(k) == v for k, v in expected_recipe.items()), 'retained_recipe', effect)
                if plan is not None:
                    check(recipe.get('proposalDigest') == plan['proposalDigest'], 'recipe_proposal', effect)
            elif kind == 'effect_succeeded':
                effect = body['effectId']
                check(effect in started, 'success_without_start', effect)
                succeeded[effect] = body
                output = outputs.get(effect, {})
                intent = intents.get(effect, (None, None, {}))[2]
                expected_output = dict(effectId=effect, stepId=body['stepId'], runId=event['runId'],
                                       evidenceDigest=body['receiptDigest'], recordDigest=body['outputRecordDigest'],
                                       invocation=intent.get('invocation'), actionDigest=intent.get('actionDigest'))
                expected_output['planId'] = intent.get('receiptProvenance', {}).get('planId')
                check(all(output.get(k) == v for k, v in expected_output.items()), 'output_binding', effect)
            elif kind == 'verification_recorded':
                effect = body['effectId']
                verified.add(effect)
                receipt = receipts.get(effect, {})
                output, material = outputs.get(effect, {}), materials.get(effect, {})
                intent = intents.get(effect, (None, None, {}))[2]
                provenance = intent.get('receiptProvenance', {})
                expected_receipt = dict(runId=event['runId'], stepId=body['stepId'],
                                        receiptId=body['receiptId'], receiptDigest=body['receiptDigest'],
                                        inputDigest=material.get('materialDigest'), outputDigest=output.get('outputDigest'),
                                        invocationId=provenance.get('invocationId'), planId=provenance.get('planId'))
                invocation = intent.get('invocation', {})
                expected_receipt.update(capability={k: invocation.get(k) for k in ('capabilityId', 'contractVersion')},
                                        instanceId=invocation.get('instanceId'),
                                        policy={'decisionId': provenance.get('policyDecisionId'),
                                                'decisionDigest': provenance.get('policyDecisionDigest')},
                                        executor={'id': provenance.get('executorId'), 'placement': provenance.get('executorPlacement'),
                                                  'platform': provenance.get('executorPlatform')})
                check(effect in succeeded, 'verification_without_success', effect)
                check(all(receipt.get(k) == v for k, v in expected_receipt.items()), 'receipt_binding', effect)
                check(receipt.get('effect', {}).get('class') == intent.get('effectClass') and
                      receipt.get('effect', {}).get('started') is True, 'receipt_effect', effect)
                check(body['disposition'] == 'passed' and receipt.get('status') == 'succeeded' and
                      all(v['result'] == 'passed' for v in receipt.get('verification', []) if v['required']),
                      'verification_not_passed', effect)
        for effect in started:
            check(effect in succeeded and effect in verified, 'unfinished_effect', effect)
        check(set(outputs) == set(receipts) == set(materials) == set(intents) == started,
              'sidecar_coverage', None)
    except (sqlite3.Error, KeyError, TypeError, ValueError, OSError) as error:
        errors.append({'effect': None, 'code': 'audit_unavailable', 'error_class': type(error).__name__})
    return {'passed': not errors, 'effect_starts': starts, 'errors': errors,
            'scope': 'relational journal and sidecar bindings; no independent digest/signature recomputation or OS sandbox proof'}


def aggregate(records):
    groups = {}
    for split, family in sorted({(r['split'], r['family']) for r in records} | {(r['split'], '*') for r in records}):
        for condition in ('X0', 'X1', 'XN'):
            rows = [r for r in records if r['split'] == split and
                    (family == '*' or r['family'] == family) and r['condition'] == condition]
            if not rows:
                continue
            def median(field):
                values = [r.get(field) for r in rows]
                return statistics.median(values) if all(v is not None for v in values) else None
            groups[f'{split}/{family}/{condition}'] = {
                'trials': len(rows), 'distinct_cases': len({r['case'] for r in rows}),
                **{key: sum(bool(r.get(key)) for r in rows) for key in
                   ('accepted', 'task_completed', 'oracle_passed', 'false_claim', 'false_completion')},
                'model_calls': sum(r['model_calls'] for r in rows),
                'median_model_calls': median('model_calls'),
                'median_elapsed_seconds': median('elapsed_seconds'),
                'median_actual_cost_nano_usd': median('actual_cost_nano_usd'),
                'median_all_miss_cost_nano_usd': median('all_miss_nano_usd'),
                'known_cost_nano_usd': sum(r['known_budget_cost_nano_usd'] for r in rows),
                'all_trials_included': True}
    return groups


def cost_audit(root, manifest, summary, records):
    entries = [json.loads(line) for line in (root / 'cost-ledger.jsonl').read_text().splitlines()]
    reservations = [e for e in entries if e['event'] == 'reserved']
    settlements = [e for e in entries if e['event'] == 'settled']
    reserved = {e['call_id']: e for e in reservations}
    settled = {e['call_id']: e for e in settlements}
    errors = []
    if len(reserved) != len(reservations) or len(settled) != len(settlements) or set(reserved) != set(settled):
        errors.append('missing_or_duplicate_settlement')
    quote = manifest['config']['quote']
    identity = manifest['config'].get('expected_response_identity')
    for entry in settlements:
        if identity is not None:
            metadata = entry.get('response_metadata') or {}
            if [metadata.get('model'), metadata.get('system_fingerprint')] != identity:
                errors.append('response_identity_changed_or_missing')
        tokens = entry['usage']
        if tokens is None or tokens['cache'] is None:
            errors.append('unknown_usage_or_cache')
            continue
        numerator = (tokens['cache'] * quote['cached_micro_usd_per_million'] +
                     (tokens['input'] - tokens['cache']) * quote['input_micro_usd_per_million'] +
                     tokens['output'] * quote['output_micro_usd_per_million'])
        all_miss = tokens['input'] * quote['input_micro_usd_per_million'] + tokens['output'] * quote['output_micro_usd_per_million']
        if entry['budget_cost_nano_usd'] != (numerator + 999) // 1000 or entry['all_miss_nano_usd'] != (all_miss + 999) // 1000:
            errors.append('quoted_cost_mismatch')
        if entry['bound_exceeded']:
            errors.append('provider_bound_exceeded')
    known = sum(e['budget_cost_nano_usd'] or 0 for e in settlements)
    if known != summary['budget_accounted_nano_usd'] or known != sum(r['known_budget_cost_nano_usd'] for r in records):
        errors.append('total_cost_mismatch')
    if summary['unknown_requests'] or summary['unknown_reserved_nano_usd']:
        errors.append('unresolved_reservation')
    return {'passed': not errors, 'errors': sorted(set(errors)), 'upstream_attempts': len(reservations),
            'settlements': len(settlements), 'known_cost_nano_usd': known,
            'all_miss_nano_usd': sum(e['all_miss_nano_usd'] or 0 for e in settlements),
            'http_statuses': dict(collections.Counter(str(e['http_status']) for e in settlements)),
            'scope': 'recomputed registered quote from retained usage; no provider invoice reconciliation'}


def paired(records):
    blocks = collections.defaultdict(dict)
    for row in records:
        blocks[(row['split'], row['case'], row['repeat'])][row['condition']] = row
    result = []
    for (split, case, repeat), rows in sorted(blocks.items()):
        if 'X1' not in rows or 'XN' not in rows:
            continue
        one, many = rows['X1'], rows['XN']
        baseline, comparison = one.get('actual_cost_nano_usd'), many.get('actual_cost_nano_usd')
        reduction = None if baseline is None or comparison is None or baseline <= 0 else 1 - comparison / baseline
        result.append({'split': split, 'case': case, 'repeat': repeat,
                       'X1_accepted': one['accepted'], 'XN_accepted': many['accepted'],
                       'model_call_difference_XN_minus_X1': many['model_calls'] - one['model_calls'],
                       'actual_cost_reduction': reduction,
                       'note': 'same-case repeated block; cache and response variation remain'})
    return result


def gates(summary, records, audits, groups):
    validation = [r for r in records if r['split'] == 'validation']
    families = sorted({r['family'] for r in records if r['split'] == 'validation'})
    completion = bool(families) and all(
        groups.get(f'validation/{f}/XN', {}).get('accepted', -1) >=
        groups.get(f'validation/{f}/X1', {}).get('accepted', float('inf')) for f in families)
    one = groups.get('validation/*/X1', {}).get('median_actual_cost_nano_usd')
    many = groups.get('validation/*/XN', {}).get('median_actual_cost_nano_usd')
    reduction = None if one is None or many is None or one <= 0 else 1 - many / one
    checks = {
        'complete_and_sources_unchanged': bool(summary.get('complete_schedule') and summary.get('source_unchanged')),
        'cost_known': summary.get('actual_cost_nano_usd') is not None and
                      all(r.get('actual_cost_nano_usd') is not None for r in records) and
                      sum(r.get('actual_cost_nano_usd') or 0 for r in records) == summary.get('actual_cost_nano_usd'),
        'validation_completion_per_family_noninferior': completion,
        'no_duplicate_effect_starts': all(r['duplicate_effect_starts'] == 0 for r in records),
        'no_validation_false_claims': bool(validation) and all(not r['false_claim'] for r in validation),
        'no_validation_false_completions': bool(validation) and all(not r['false_completion'] for r in validation),
        'validation_median_cost_reduction_at_least_15_percent': reduction is not None and reduction >= .15,
        'journal_binding_audit_passed': len(audits) == len(records) and all(a['passed'] for a in audits),
        'no_all_incomplete_cost_advantage': bool(families) and all(
            any(r['accepted'] for r in validation if r['family'] == family and r['condition'] == condition)
            for family in families for condition in ('X1', 'XN'))}
    return {'checks': checks, 'passed': all(checks.values()), 'validation_median_cost_reduction': reduction,
            'interpretation': 'descriptive pilot only; passing permits a larger study, not default adoption'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--preregistration', type=Path, required=True)
    args = parser.parse_args()
    root = args.results.resolve()
    summary = read_json(root / 'summary.json')
    records = [json.loads(line) for line in (root / 'trials.jsonl').read_text().splitlines()]
    audits = []
    failures = []
    journal_hashes = {}
    for record in records:
        databases = list((root / record['trial'] / 'state/runs').glob('*/run.sqlite3'))
        result = audit(databases[0]) if len(databases) == 1 else {'passed': False, 'errors': [{'code': 'database_count'}]}
        audits.append(dict(trial=record['trial'], **result))
        if len(databases) == 1:
            if record['false_claim']:
                failures.append(claim_failure(databases[0], record))
            for path in databases[0].parent.glob('*.sqlite3'):
                journal_hashes[str(path.relative_to(root))] = checksum(path)
    groups = aggregate(records)
    verdict = gates(summary, records, audits, groups)
    manifest = read_json(root / 'manifest.json')
    expected_schedule = manifest['schedule']
    actual_schedule = [{k: row[k] for k in ('case', 'repeat', 'condition')} for row in records]
    verdict['checks']['registered_schedule_exact'] = actual_schedule == expected_schedule
    expected_identity = manifest['config'].get('expected_response_identity')
    verdict['checks']['response_identity_unchanged'] = (
        not summary['batch_aborted'] and not summary['provider_bound_violation'] and
        (expected_identity is None or list(summary['response_identity']) == expected_identity))
    registration = read_json(args.preregistration)
    repository = args.preregistration.resolve().parents[2]
    registered_config = read_json(repository / registration['config_file'])
    verdict['checks']['preregistration_matches_manifest'] = (
        manifest['schedule'] == registration['schedule'] and
        manifest['fixture_sha256'] == registration['cases_sha256'] and
        manifest['config'] == registered_config and
        manifest['binary_sha256'] == registration['binary_sha256'] and
        all(manifest['source_hashes'].get(str(repository / path)) == digest
            for path, digest in registration['source_sha256'].items()))
    costs = cost_audit(root, manifest, summary, records)
    verdict['checks']['cost_ledger_reconciled'] = costs['passed']
    verdict['passed'] = all(verdict['checks'].values())
    case_groups = {key: value for key, value in aggregate([dict(r, family=r['case']) for r in records]).items() if '/*/' not in key}
    report = {'format_version': 1, 'groups': groups, 'case_groups': case_groups, 'gates': verdict,
              'claim_failures': failures, 'cost_audit': costs, 'paired_blocks': paired(records),
              'audits': audits, 'private_journal_sha256': journal_hashes,
              'input_sha256': {name: checksum(root / name) for name in ('manifest.json', 'summary.json', 'trials.jsonl', 'cost-ledger.jsonl')},
              'analysis_sha256': checksum(__file__), 'experimental_unit': '8 distinct cases; 5 repeated runs per condition; 4 held-out cases',
              'preregistration_sha256': checksum(args.preregistration),
              'cost_note': 'Provider usage times registered fixed-tier quote; not independently reconciled billing. Failures included.'}
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(report['gates']))


if __name__ == '__main__':
    main()
