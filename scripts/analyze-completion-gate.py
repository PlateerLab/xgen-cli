#!/usr/bin/env python3
"""Read-only C0/C1 pilot analysis; relational bindings, not cryptographic proof."""
import argparse
import collections
import hashlib
import importlib.util
import json
from pathlib import Path
import sqlite3
import statistics

def audit(database):
    """Check prior authorization and retained invocation/output/receipt relationships."""
    errors = []
    starts = 0
    failed = {}
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
        intents, plans, started, succeeded, verified, failed = {}, {}, set(), {}, set(), {}
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
            elif kind == 'effect_failed':
                effect = body['effectId']
                check(effect in started and effect not in failed and effect not in succeeded, 'invalid_failure_transition', effect)
                check(body['stepId'] == intents.get(effect, (None,None,{}))[1], 'failure_step_binding', effect)
                check(isinstance(body.get('receiptDigest'), str) and body['receiptDigest'].startswith('sha256:') and len(body['receiptDigest']) == 71, 'failure_evidence_reference', effect)
                failed[effect] = body
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
            check((effect in succeeded and effect in verified and effect not in failed) or (effect in failed and effect not in succeeded and effect not in verified), 'unfinished_or_conflicting_effect', effect)
        check(set(outputs) == set(receipts) == set(succeeded) == verified and set(materials) == set(intents) == started and set(failed).isdisjoint(outputs),
              'sidecar_coverage', None)
    except (sqlite3.Error, KeyError, TypeError, ValueError, OSError) as error:
        errors.append({'effect': None, 'code': 'audit_unavailable', 'error_class': type(error).__name__})
    return {'passed': not errors, 'effect_starts': starts, 'failed_effects': len(failed), 'errors': errors,
            'scope': 'relational journal and sidecar bindings; no independent digest/signature recomputation or OS sandbox proof'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--preregistration', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root=args.results
    manifest=json.loads((root/'manifest.json').read_bytes())
    summary=json.loads((root/'summary.json').read_bytes())
    registration=json.loads(args.preregistration.read_bytes())
    records=[json.loads(line) for line in (root/'trials.jsonl').read_text().splitlines()]
    helper_path=Path(__file__).with_name('analyze-planning-pilot.py')
    spec=importlib.util.spec_from_file_location('cost_helper',helper_path)
    helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
    audits=[]
    for row in records:
        databases=list((root/row['trial']).rglob('run.sqlite3'))
        result=audit(databases[0]) if len(databases)==1 else {'passed':False,'errors':['missing_or_multiple_journals']}
        audits.append(dict(trial=row['trial'],**result))
    groups={}
    for condition in ('C0','C1'):
        for split in ('all','design','validation'):
            selected=[r for r in records if r['condition']==condition and (split=='all' or r['split']==split)]
            groups[split+'/'+condition]={
                'trials':len(selected),
                'task_success':sum(r['oracle_passed'] and r['protected_preserved'] and r['writes_in_scope'] for r in selected),
                'accepted':sum(r['accepted'] for r in selected),
                'reported_completion':sum(r['task_completed'] for r in selected),
                'false_completion':sum(r['false_completion'] for r in selected),
                'verified_recovery_and_task_success':sum(r.get('failed_check_then_passed',False) and r['oracle_passed'] and r['protected_preserved'] and r['writes_in_scope'] for r in selected),
                'gate_rejections':dict(collections.Counter(reason for r in selected for reason in r.get('gate_rejections',[]))),
                'model_calls':sum(r['model_calls'] for r in selected),
                'cost_nano_usd':sum(r['actual_cost_nano_usd'] for r in selected),
                'median_elapsed_seconds':statistics.median(r['elapsed_seconds'] for r in selected) if selected else None}
    cost=helper.cost_audit(root,manifest,summary,records)
    repository=Path(__file__).resolve().parents[1]
    checks={
        'registered_sources_match':all(hashlib.sha256((repository/name).read_bytes()).hexdigest()==digest for name,digest in registration['source_sha256'].items()),
        'registered_inputs_match':hashlib.sha256((repository/registration['config_file']).read_bytes()).hexdigest()==registration['config_sha256'] and all(hashlib.sha256((repository/item['path']).read_bytes()).hexdigest()==item['sha256'] for item in registration['fixture_files']),
        'schedule_matches_registration':manifest['schedule']==registration['schedule'] and summary['complete_schedule'] and len(records)==registration['trials'] and [(r['case'],r['repeat'],r['condition']) for r in records]==[(r['case'],r['repeat'],r['condition']) for r in registration['schedule']],
        'source_unchanged':summary['source_unchanged'],
        'binary_matches_registration':manifest['binary_sha256']==registration['binary_sha256'] and hashlib.sha256((root/'planning_model_pilot').read_bytes()).hexdigest()==registration['binary_sha256'],
        'independent_cost_audit':cost['passed'],
        'no_duplicate_effect_starts':all(r['duplicate_effect_starts']==0 for r in records),
        'relational_authorization_and_receipts':len(audits)==len(records) and all(a['passed'] for a in audits),
        'no_evidence_errors':not any(r.get('evidence_error') for r in records)}
    blocks=collections.defaultdict(dict)
    for row in records:blocks[(row['case'],row['repeat'])][row['condition']]=row
    paired=[{'case':case,'repeat':repeat,'C0_accepted':rows['C0']['accepted'],'C1_accepted':rows['C1']['accepted'],'calls_C1_minus_C0':rows['C1']['model_calls']-rows['C0']['model_calls'],'cost_C1_minus_C0_nano_usd':rows['C1']['actual_cost_nano_usd']-rows['C0']['actual_cost_nano_usd']} for (case,repeat),rows in sorted(blocks.items()) if set(rows)=={'C0','C1'}]
    result={'paired_blocks':paired,'checks':checks,'audit_passed':all(checks.values()),'groups':groups,'cost_audit':cost,'journal_audits':audits,'interpretation':'Descriptive paired prompt+gate bundle pilot. Failed effects require prior authorization and retained material, durable failure with matching step, and absence of successful output/Receipt. Failure evidence digest contents and journal hashes are not independently recomputed. No default-adoption or statistical-significance claim.'}
    args.output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps({'checks':checks,'groups':groups},ensure_ascii=False))

if __name__=='__main__':main()
