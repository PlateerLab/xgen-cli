#!/usr/bin/env python3
"""Recompute diagnostic digests from fixture bytes and receipt-verified file writes."""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3


def digest(raw):
    return 'sha256:' + hashlib.sha256(raw).hexdigest()


def audit(database, initial_bytes, condition):
    errors, observations = [], []
    current = initial_bytes
    connection = sqlite3.connect(f'file:{database}?mode=ro', uri=True)
    recipes = sqlite3.connect(f'file:{database.parent / "materials.sqlite3"}?mode=ro', uri=True)
    try:
        outputs = {effect: json.loads(raw) for effect, raw in connection.execute('SELECT effect_id,record_json FROM tool_outputs')}
        receipts = {effect: json.loads(raw) for effect, raw in connection.execute('SELECT effect_id,receipt_json FROM execution_receipts')}
        materials = {effect: json.loads(raw) for effect, raw in connection.execute('SELECT effect_id,record_json FROM invocation_materials')}
        retained = {}
        for effect, material in materials.items():
            reference = material['retention']['reference']
            raw = recipes.execute('SELECT record FROM material_recipe WHERE reference_id=? AND revision=?', (reference['referenceId'], reference['revision'])).fetchone()
            retained[effect] = json.loads(raw[0])
        for (raw,) in connection.execute('SELECT event_json FROM run_events ORDER BY sequence'):
            body = json.loads(raw)['body']
            if body['type'] not in {'verification_recorded', 'effect_failed'}:
                continue
            effect = body['effectId']
            recipe = retained[effect]
            capability, arguments = recipe['capability']['capabilityId'], recipe['arguments']
            if capability == 'xgeny.fs/write-atomic' and arguments['path'] == 'workspace:primary/result.json':
                desired = arguments['content'].encode()
                fresh = current is not None and arguments['expectedDigest'] == digest(current)
                same_content = current == desired
                if body['type'] == 'effect_failed':
                    conflict_digest = digest(b'xgeny.fs/write-atomic/failure/v1/precondition-conflict')
                    if fresh or same_content or body['receiptDigest'] != conflict_digest or effect in outputs or effect in receipts:
                        errors.append('invalid_conflict_failure')
                else:
                    output = outputs[effect]['output']
                    if not (fresh or same_content) or output['digest'] != digest(desired) or body['disposition'] != 'passed':
                        errors.append('write_precondition_or_output')
                    current = desired
            elif capability == 'xgeny.process/execute':
                if not (arguments['executable'].endswith('/executables/verify') and arguments['args'] == [] and arguments['cwd'] == '.' and arguments['env'] == {}):
                    # Arbitrary process writes cannot be inferred from declared intent.
                    current = None
                    errors.append('untracked_process_effect')
                    continue
                output = outputs[effect]['output']
                if output.get('outcome') != 'exited':
                    errors.append('check_not_exited')
                if type(output.get('exitCode')) is not int or output['exitCode'] not in (0, 1):
                    errors.append('unexpected_check_exit_code')
                if output['exitCode'] != 1:
                    continue
                record = json.loads(output['stderr'])
                if current is None or record.get('reason') != 'value_mismatch' or json.dumps(record['actual'], sort_keys=True, ensure_ascii=True) != json.dumps(json.loads(current), sort_keys=True, ensure_ascii=True):
                    errors.append('diagnostic_actual_snapshot')
                expected = digest(current) if current is not None else None
                if condition == 'D1' and record.get('actual_digest') != expected:
                    errors.append('diagnostic_digest_snapshot')
                if condition == 'D0' and 'actual_digest' in record:
                    errors.append('control_exposes_digest')
                observations.append({'effect_id':effect, 'raw_digest':expected, 'digest_exposed':condition=='D1'})
            elif capability not in {'xgeny.fs/read-text', 'xgeny.fs/list-directory', 'xgeny.fs/search-text', 'xgeny.fs/stat'}:
                current = None
                errors.append('untracked_mutating_effect')
        return {'passed':not errors, 'errors':errors, 'observations':observations,
                'scope':'Raw SHA-256 recomputation using registered initial bytes and verified write recipes. Assumes only modeled writes; rejects other process effects. Separate relational journal audit is required; not full journal cryptographic verification.'}
    finally:
        connection.close()
        recipes.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--fixtures', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    cases = {case['id']:case for case in json.loads(args.fixtures.read_bytes())}
    rows = [json.loads(line) for line in (args.results/'trials.jsonl').read_text().splitlines()]
    results = []
    for row in rows:
        databases = list((args.results/row['trial']).rglob('run.sqlite3'))
        result = audit(databases[0], cases[row['case']]['files']['result.json'].encode(), row['condition'])
        results.append(dict(trial=row['trial'], **result))
    args.output.write_text(json.dumps({'passed':all(r['passed'] for r in results), 'trials':results}, indent=2)+'\n')
    print(json.dumps({'passed':all(r['passed'] for r in results), 'trials':len(results)}))


if __name__ == '__main__':
    main()
