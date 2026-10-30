#!/usr/bin/env python3
"""Validate the frozen synthetic task catalog; optionally run the existing live evaluator."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[1]
RUNNER = REPO / 'scripts/evaluate-projects.py'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def execute(command, directory, environment=None):
    return subprocess.run(command, cwd=directory, env=environment, capture_output=True,
                          text=True, timeout=60)


def preflight(cases, fixes, report_root):
    spec = importlib.util.spec_from_file_location('project_eval', RUNNER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.validate_cases(cases)
    rows = []
    for case in cases:
        folder = report_root / case['id']
        folder.mkdir()
        for name, content in case['files'].items():
            path = folder / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        before = execute(case['commands'][0], folder)
        allowed = set(case.get('sources', [case['source']]))
        if set(fixes[case['id']]) != allowed:
            raise ValueError('oracle must replace exactly the allowed files')
        for name, content in fixes[case['id']].items():
            (folder / name).write_text(content)
        visible = execute(case['commands'][0], folder)
        build = execute(case['commands'][1], folder)
        (folder / case['hidden_path']).write_text(case['hidden_tests'])
        hidden_command = (['node', '--test', 'visible.test.mjs', case['hidden_path']]
                          if case['language'] == 'node' else case['commands'][0])
        hidden = execute(hidden_command, folder)
        preserved = all((folder / name).read_text() == content
                        for name, content in case['files'].items() if name not in allowed)
        row = {'case': case['id'], 'initial_tests_failed': before.returncode != 0,
               'reference_visible_passed': visible.returncode == 0,
               'reference_build_passed': build.returncode == 0,
               'reference_hidden_passed': hidden.returncode == 0,
               'protected_files_preserved': preserved}
        row['pass'] = all(value for key, value in row.items() if key != 'case')
        rows.append(row)
        # Local diagnostic files are not model observations or public evaluation results.
        (folder / 'preflight.log').write_text('\n'.join(
            f'{label}\n{result.stdout}\n{result.stderr}'
            for label, result in [('before', before), ('visible', visible), ('build', build), ('hidden', hidden)]))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--execute', action='store_true', help='Use the actual configured model after preflight')
    parser.add_argument('--binary', type=Path, default=REPO / 'target/debug/xgen')
    args = parser.parse_args()
    cases = json.loads((ROOT / 'cases.json').read_text())
    fixes = json.loads((ROOT / 'oracle-fixes.json').read_text())
    report_root = Path(tempfile.mkdtemp(prefix='xgen-created-tasks-'))
    registration = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
                    'cases_sha256': sha(ROOT / 'cases.json'), 'oracle_sha256': sha(ROOT / 'oracle-fixes.json'),
                    'runner_sha256': sha(RUNNER), 'preflight_sha256': sha(Path(__file__)),
                    'binary_sha256': sha(args.binary), 'attempts': 2, 'timeout_seconds': 240,
                    'task_count': len(cases), 'status': 'preflight_only', 'live_model_calls': 0}
    rows = preflight(cases, fixes, report_root)
    report = {'registration': registration, 'preflight': rows}
    (report_root / 'preflight.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'report_root': str(report_root), 'preflight_passed': sum(row['pass'] for row in rows),
                      'tasks': len(rows)}, ensure_ascii=False), flush=True)
    if not all(row['pass'] for row in rows):
        return 1
    if args.execute:
        # Match the existing evaluator's environment rules: auth must be in the active profile.
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(('XGEN_', 'XGENY_', 'DEEPSEEK_'))}
        try:
            checked = execute([str(args.binary.resolve()), 'model', 'check'], REPO, environment)
            codes = re.findall(r'reason=([a-z_]+)', checked.stdout + checked.stderr)
        except subprocess.TimeoutExpired:
            checked = None
            codes = ['model_check_timeout']
        if checked is None or checked.returncode:
            registration.update(status='blocked_model_configuration', check_codes=codes)
            (report_root / 'preflight.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
            print(json.dumps({'status': registration['status'], 'reason_codes': codes,
                              'live_model_calls': 0}), flush=True)
            return 2
        # No retries of failed cases: each attempt gets its own fresh project/state.
        registration.update(status='live_evaluator_started', live_model_calls=None)
        (report_root / 'preflight.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        return subprocess.call(['python3', str(RUNNER), '--binary', str(args.binary.resolve()),
                                '--cases', str(ROOT / 'cases.json'), '--attempts', '2', '--timeout', '240'], cwd=REPO)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
