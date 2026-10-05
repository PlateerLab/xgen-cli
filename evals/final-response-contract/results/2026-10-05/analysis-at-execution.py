#!/usr/bin/env python3
"""Audit a registered response-contract batch using retained private SQLite journals."""
import argparse
import importlib.util
import json
from pathlib import Path


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', type=Path, required=True)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--registration', type=Path, required=True)
    args = parser.parse_args()
    pilot = module('pilot', args.repository / 'scripts/evaluate-planning-model.py')
    helper_path = args.repository / 'scripts/analyze-planning-pilot.py'
    helper = module('helper', helper_path)
    registration = pilot.strict_json(args.registration.read_bytes())
    manifest = pilot.strict_json((args.results / 'manifest.json').read_bytes())
    summary = pilot.strict_json((args.results / 'summary.json').read_bytes())
    rows = [pilot.strict_json(line) for line in (args.results / 'trials.jsonl').read_bytes().splitlines()]
    audits = []
    for row in rows:
        database, = (args.results / row['trial'] / 'state/runs').glob('*/run.sqlite3')
        audit = helper.audit(database)
        _, _, summaries, _, outputs = pilot.ORACLES.inspect(database)
        report = pilot.strict_json(summaries[-1]) if len(summaries) == 1 else None
        evidence = pilot.receipt_command_evidence(database, outputs)
        audit.update(trial=row['trial'], exact_host_report=bool(
            isinstance(report, dict) and set(report) == {'format_version', 'response', 'commands'}
            and type(report['format_version']) is int and report['format_version'] == 1
            and report['response'] == row['validated_response']
            and report['commands'] == evidence == row['observed_command_evidence']))
        audits.append(audit)
    schedule = [{key:row[key] for key in ('case', 'repeat', 'condition')} for row in rows]
    registered_sources = all(pilot.digest(args.repository / name) == checksum
                             for name, checksum in registration['source_sha256'].items())
    # The original temporary executable can be gone after the batch; use the
    # retained identical binary rather than requiring its old filesystem path.
    frozen_sources = all(pilot.digest(
        args.results / 'planning_model_pilot'
        if Path(name).name == 'planning_model_pilot' and checksum == manifest['binary_sha256']
        else Path(name)) == checksum for name, checksum in manifest['source_hashes'].items())
    cost = helper.cost_audit(args.results, manifest, summary, rows)
    checks = {
        'complete_registered_schedule': summary['complete_schedule'] and schedule == registration['schedule'] == manifest['schedule'],
        'registered_sources_unchanged': registered_sources and frozen_sources and summary['source_unchanged'],
        'registered_config_fixtures_binary': (
            manifest['config'] == pilot.strict_json((args.repository / registration['config_file']).read_bytes())
            and manifest['fixture_sha256'] == registration['cases_sha256']
            and manifest['binary_sha256'] == registration['binary_sha256'] == pilot.digest(args.results / 'planning_model_pilot')),
        'all_response_contracts_match': all(row['claims_match'] and a['exact_host_report'] for row,a in zip(rows,audits,strict=True)),
        'journal_bindings_pass': all(a['passed'] for a in audits),
        'no_duplicate_starts': all(row['duplicate_effect_starts'] == 0 for row in rows),
        'known_cost_and_stable_response_identity': cost['passed'] and not summary['batch_aborted'],
        'all_tasks_accepted': all(row['accepted'] for row in rows),
        'no_false_completion': not any(row['false_completion'] for row in rows),
    }
    result = dict(format_version=1, checks=checks, verdict='PASS' if all(checks.values()) else 'FAIL',
                  groups=helper.aggregate(rows), audits=audits, cost_audit=cost,
                  contract_matches=sum(row['claims_match'] for row in rows),
                  oracle_passed=sum(row['oracle_passed'] for row in rows),
                  accepted=sum(row['accepted'] for row in rows),
                  effect_starts=sum(a['effect_starts'] for a in audits),
                  task_failures=[{key:row[key] for key in ('trial','case','split','condition','oracle_passed','false_completion')} for row in rows if not row['accepted']],
                  registration_sha256=pilot.digest(args.registration),
                  helper_sha256=pilot.digest(helper_path), analysis_sha256=pilot.digest(Path(__file__)),
                  interpretation='Structural contract checks and independent task oracle are separate; no uncontracted control or comparative accuracy claim. Audit does not independently recompute signatures/digests or prove an OS sandbox.')
    (args.results / 'analysis.json').write_bytes(pilot.canonical(result))
    print(json.dumps({key:result[key] for key in ('verdict','checks','contract_matches','oracle_passed','accepted','effect_starts')},ensure_ascii=False))


if __name__ == '__main__':
    main()
