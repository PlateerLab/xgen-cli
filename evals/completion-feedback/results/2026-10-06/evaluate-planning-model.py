#!/usr/bin/env python3
"""Serial planning-size pilot using the real driver; no live request without a frozen manifest."""
import argparse
from datetime import datetime, timezone
import hashlib
import http.server
import importlib.util
import ipaddress
import itertools
import json
import os
from pathlib import Path
import random
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('project_oracles', ROOT / 'scripts/evaluate-projects.py')
ORACLES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ORACLES)
MAX_BODY = 1024 * 1024


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()


def digest(path):
    checksum = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda: stream.read(65536), b''):
            checksum.update(chunk)
    return checksum.hexdigest()


def strict_json(raw):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError('duplicate JSON field')
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite JSON')))


def utc(value):
    result = datetime.fromisoformat(value.replace('Z', '+00:00'))
    if result.tzinfo is None:
        raise ValueError('explicit UTC quote window required')
    return result.astimezone(timezone.utc)


def relative(name):
    path = Path(name)
    if not name or path.is_absolute() or '..' in path.parts or any(ord(c) < 32 for c in name):
        raise ValueError('fixture paths must remain inside workspace')
    return path


def validate(config, cases, process_path=None):
    required = {'model', 'tokenizer', 'response_format', 'thinking', 'max_output_tokens',
                'request_timeout_seconds', 'max_model_turns', 'max_ticks', 'max_input_tokens',
                'trial_timeout_seconds', 'spend_cap_nano_usd', 'quote', 'seed', 'repeats'}
    if not required <= set(config) or set(config) - required - {'expected_response_identity', 'final_response_schema', 'conditions'}:
        raise ValueError('explicit complete non-secret pilot config required')
    conditions = config.get('conditions', ['X0','X1','XN'])
    if conditions not in (['X0','X1','XN'], ['C0','C1'], ['F0','F1']):
        raise ValueError('registered condition family required')
    response_schema = config.get('final_response_schema')
    if response_schema is not None and (not isinstance(response_schema, dict) or response_schema.get('type') != 'object'
            or len(canonical(response_schema)) > 32768):
        raise ValueError('bounded final response object schema required')
    identity = config.get('expected_response_identity')
    if identity is not None and (not isinstance(identity, list) or len(identity) != 2
            or identity[0] != config['model'] or not isinstance(identity[1], str) or not identity[1]):
        raise ValueError('frozen response model and fingerprint required')
    if config['response_format'] not in {'json_schema', 'json_object'} or config['thinking'] not in {'default', 'disabled', 'enabled', 'chat_template_disabled'}:
        raise ValueError('unsupported wire options')
    for key in ['max_output_tokens', 'request_timeout_seconds', 'max_model_turns', 'max_ticks',
                'max_input_tokens', 'trial_timeout_seconds', 'spend_cap_nano_usd', 'repeats']:
        if type(config[key]) is not int or config[key] <= 0:
            raise ValueError('positive integer limits required')
    if config['trial_timeout_seconds'] <= config['request_timeout_seconds']:
        raise ValueError('trial timeout must exceed request timeout')
    quote = config['quote']
    if set(quote) != {'kind', 'source', 'valid_from_utc', 'valid_until_utc', 'cached_micro_usd_per_million', 'input_micro_usd_per_million', 'output_micro_usd_per_million'} or quote['kind'] not in {'fixed_tier', 'upper_bound'}:
        raise ValueError('frozen quote kind and source required')
    prices = [quote[key] for key in ['cached_micro_usd_per_million', 'input_micro_usd_per_million', 'output_micro_usd_per_million']]
    if any(type(x) is not int or x < 0 for x in prices) or not 0 <= prices[0] <= prices[1] or prices[1] == 0 or prices[2] == 0:
        raise ValueError('invalid integer prices')
    if utc(quote['valid_from_utc']) >= utc(quote['valid_until_utc']):
        raise ValueError('invalid quote window')
    if not isinstance(cases, list) or not cases:
        raise ValueError('nonempty fixtures required')
    ids = set()
    for case in cases:
        if set(case) - {'completion_checker'} != {'id', 'split', 'family', 'goal', 'files', 'writable', 'generated_dirs', 'executables', 'oracle'}:
            raise ValueError('explicit fixture contract required')
        if not isinstance(case['id'], str) or not case['id'].replace('-', '').isalnum() or case['id'] in ids:
            raise ValueError('unique simple fixture IDs required')
        ids.add(case['id'])
        if case['split'] not in {'design', 'validation'} or not case['family'] or not case['goal']:
            raise ValueError('fixture split, family, and goal required')
        for name in [*case['files'], *case['writable']]:
            relative(name)
        for name in case['generated_dirs']:
            if relative(name) == Path('.'):
                raise ValueError('generated directory cannot exclude the whole workspace')
        if any(any(relative(generated) in relative(name).parents or relative(generated) == relative(name)
                   for generated in case['generated_dirs']) for name in case['files']):
            raise ValueError('visible fixture inputs cannot be excluded from measurement')
        if any(not isinstance(text, str) for text in case['files'].values()):
            raise ValueError('UTF-8 fixture content required')
        if set(case['oracle']) != {'python'} or not isinstance(case['oracle']['python'], str):
            raise ValueError('independent Python oracle required')
        if conditions in (['C0','C1'], ['F0','F1']) and (response_schema is None or not isinstance(case.get('completion_checker'),str) or not case['completion_checker']):
            raise ValueError('completion comparison requires a schema and host checker for every case')
        if conditions in (['C0','C1'], ['F0','F1']) and ('python3' not in case['executables'] or any(c.isspace() for c in (shutil.which('python3', path=process_path) or ''))):
            raise ValueError('checker interpreter must be registered and have a shebang-safe path')
        if any(not name.isidentifier() or shutil.which(name, path=process_path) is None for name in case['executables']):
            raise ValueError('declared executable must resolve')


def schedule(cases, repeats, seed, conditions=('X0','X1','XN')):
    rng = random.Random(seed)
    permutations = list(itertools.permutations(conditions))
    rng.shuffle(permutations)
    blocks = [(case, repeat) for case in cases for repeat in range(repeats)]
    rng.shuffle(blocks)
    return [(case, repeat, condition) for index, (case, repeat) in enumerate(blocks)
            for condition in permutations[index % len(permutations)]]


def usage(raw, model):
    try:
        envelope = strict_json(raw)
        if envelope.get('model') != model:
            return None
        data = envelope['usage']
        i, o, total = [data[k] for k in ['prompt_tokens', 'completion_tokens', 'total_tokens']]
        if any(type(n) is not int or n < 0 for n in (i, o, total)) or i + o != total:
            return None
        native = data.get('prompt_cache_hit_tokens')
        standard = data.get('prompt_tokens_details', {}).get('cached_tokens')
        if native is not None and standard is not None and native != standard:
            return None
        cache = native if native is not None else standard
        miss = data.get('prompt_cache_miss_tokens')
        if cache is not None and (type(cache) is not int or not 0 <= cache <= i):
            return None
        if miss is not None and (type(miss) is not int or cache is None or miss + cache != i):
            return None
        reasoning = data.get('completion_tokens_details', {}).get('reasoning_tokens')
        if reasoning is not None and (type(reasoning) is not int or not 0 <= reasoning <= o):
            return None
        return {'input': i, 'output': o, 'cache': cache, 'reasoning': reasoning}
    except (ValueError, KeyError, TypeError, AttributeError):
        return None


def price_nano(tokens, quote, all_miss=False):
    if tokens['cache'] is None and not all_miss:
        return None
    cache = 0 if all_miss else tokens['cache']
    numerator = ((tokens['input'] - cache) * quote['input_micro_usd_per_million']
                 + cache * quote['cached_micro_usd_per_million']
                 + tokens['output'] * quote['output_micro_usd_per_million'])
    return (numerator + 999) // 1000


class Ledger:
    """Durable reservation before forwarding; unresolved attempts retain their whole bound."""
    def __init__(self, config, path):
        self.config, self.path = config, path
        self.committed = 0
        self.reservations = {}
        self.calls = []
        self.attempts = []
        self.aborted = False
        self.abort_reason = None
        identity = config.get('expected_response_identity')
        self.response_identity = tuple(identity) if identity is not None else None
        self.lock = threading.Lock()

    def append(self, event):
        with self.path.open('ab') as stream:
            stream.write(canonical(event) + b'\n')
            stream.flush()
            os.fsync(stream.fileno())

    def reserve(self, call_id, trial):
        with self.lock:
            now = datetime.now(timezone.utc)
            quote = self.config['quote']
            bound = price_nano({'input':self.config['max_input_tokens'], 'output':self.config['max_output_tokens'], 'cache':0}, quote)
            if (self.aborted or call_id in self.reservations or any(c['call_id'] == call_id for c in self.calls)
                    or not utc(quote['valid_from_utc']) <= now < utc(quote['valid_until_utc'])
                    or self.committed + sum(self.reservations.values()) + bound > self.config['spend_cap_nano_usd']):
                self.append({'event':'blocked', 'call_id':call_id, 'trial':trial})
                return False
            event = {'event':'reserved', 'call_id':call_id, 'trial':trial, 'bound_nano_usd':bound, 'utc':now.isoformat()}
            self.append(event)
            self.attempts.append(event)
            self.reservations[call_id] = bound
            return True

    def settle(self, call_id, trial, raw, status):
        with self.lock:
            try:
                envelope = strict_json(raw)
                metadata = {key:envelope.get(key) for key in ['model', 'system_fingerprint']}
                if any(value is not None and (not isinstance(value, str) or len(value) > 512) for value in metadata.values()):
                    metadata = {'model':None, 'system_fingerprint':None}
            except (ValueError, AttributeError):
                metadata = {'model':None, 'system_fingerprint':None}
            if metadata['model'] is not None:
                identity = (metadata['model'], metadata['system_fingerprint'])
                if metadata['model'] != self.config['model'] or (self.response_identity is not None and identity != self.response_identity):
                    self.aborted, self.abort_reason = True, 'response_identity_changed'
                elif self.response_identity is None:
                    self.response_identity = identity
            elif self.response_identity is not None:
                self.aborted, self.abort_reason = True, 'response_identity_unavailable'
            tokens = usage(raw, self.config['model'])
            cost = price_nano(tokens, self.config['quote']) if tokens else None
            bound = self.reservations[call_id]
            exceeded = bool(tokens and (tokens['input'] > self.config['max_input_tokens'] or tokens['output'] > self.config['max_output_tokens']))
            if exceeded or (cost is not None and cost > bound):
                self.aborted = True
                self.abort_reason = 'provider_token_bound_exceeded'
                cost = None
            record = {'event':'settled', 'trial':trial, 'call_id':call_id, 'http_status':status,
                      'usage':tokens, 'budget_cost_nano_usd':cost, 'bound_exceeded':exceeded,
                      'response_metadata':metadata,
                      'all_miss_nano_usd':price_nano(tokens, self.config['quote'], True) if tokens else None}
            self.append(record)
            if cost is not None:
                self.committed += cost
                del self.reservations[call_id]
            self.calls.append(record)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Proxy(http.server.HTTPServer):
    def __init__(self, config, ledger, upstream, key=None):
        if key is not None and (not key or any(ord(character) <= 32 or ord(character) > 126 for character in key)):
            raise ValueError('invalid pilot credential')
        if any(ord(character) <= 32 for character in upstream):
            raise ValueError('invalid upstream URL')
        parsed = urllib.parse.urlsplit(upstream)
        loopback = False
        try:
            loopback = ipaddress.ip_address(parsed.hostname).is_loopback
        except (ValueError, TypeError):
            pass
        if (parsed.username or parsed.password or parsed.query or parsed.fragment or not parsed.path.endswith('/v1')
                or (parsed.scheme != 'https' and not (parsed.scheme == 'http' and loopback and key is None))):
            raise ValueError('HTTPS upstream required; credential-free literal loopback allowed for tests')
        self.config, self.ledger, self.upstream, self.key = config, ledger, upstream, key
        self.trials = set()
        super().__init__(('127.0.0.1', 0), ProxyHandler)


class ProxyHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body):
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        self.connection.settimeout(5)
        try:
            size = int(self.headers.get('Content-Length', '0'))
            route = self.path.split('/')
            if len(route) != 5 or route[2:] != ['v1', 'chat', 'completions'] or route[1] not in self.server.trials or not 0 < size <= MAX_BODY:
                raise ValueError('bounded chat request required')
            raw = self.rfile.read(size)
            body = strict_json(raw)
            context = strict_json(body['messages'][1]['content'])
            call_id = context['callId']
            if (not isinstance(call_id, str) or not call_id or len(call_id) > 256
                    or body['model'] != self.server.config['model'] or body.get('stream') is not False
                    or body['max_tokens'] != self.server.config['max_output_tokens']):
                raise ValueError('request differs from frozen model limits')
            trial = route[1]
            if not self.server.ledger.reserve(call_id, trial):
                self.reply(429, b'{"error":"pilot_budget_or_quote_blocked"}')
                return
        except (ValueError, KeyError, IndexError, TypeError, AttributeError, TimeoutError):
            self.reply(400, b'{"error":"invalid_pilot_request"}')
            return
        headers = {'Content-Type':'application/json'}
        if self.server.key:
            headers['Authorization'] = 'Bearer ' + self.server.key
        request = urllib.request.Request(self.server.upstream + '/chat/completions', data=raw, headers=headers)
        status, response = 502, b'{"error":"upstream_unknown"}'
        try:
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
            try:
                handle = opener.open(request, timeout=self.server.config['request_timeout_seconds'])
            except urllib.error.HTTPError as error:
                handle = error
            with handle:
                status = handle.code
                response = handle.read(MAX_BODY + 1)
                if len(response) > MAX_BODY:
                    status, response = 502, b'{"error":"upstream_oversized"}'
        except (OSError, ValueError, urllib.error.URLError, TimeoutError):
            pass
        # Persist settlement before returning bytes, including HTTP errors and malformed proposals.
        self.server.ledger.settle(call_id, trial, response, status)
        self.reply(status, response)


def inventory(root, generated_dirs=()):
    files = {}
    for path in root.rglob('*'):
        relative_path = path.relative_to(root)
        if any(Path(name) == relative_path or Path(name) in relative_path.parents for name in generated_dirs):
            continue
        if path.is_symlink():
            files[str(path.relative_to(root))] = 'SYMLINK'
        elif path.is_file():
            files[str(path.relative_to(root))] = digest(path)
    return files


def child_environment(state, scratch):
    # Never pass the upstream credential or the parent's provider configuration to the bridge/tools.
    result = {key:os.environ[key] for key in ['PATH', 'LANG', 'SYSTEMROOT'] if key in os.environ}
    result.update(HOME=str(scratch), TMPDIR=str(scratch), XGEN_STATE_HOME=str(state))
    return result


def stop_process_tree(process):
    """Freeze the owned Linux process tree before killing it; pidfds prevent PID reuse."""
    handles = {}
    def freeze(pid, parent_pid=None):
        fd = None
        try:
            fd = os.pidfd_open(pid)
            if parent_pid is not None:
                fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
                if int(fields[1]) != parent_pid:
                    os.close(fd)
                    return
            signal.pidfd_send_signal(fd, signal.SIGSTOP)
            handles[pid] = fd
        except (ProcessLookupError, FileNotFoundError):
            if fd is not None:
                os.close(fd)
    freeze(process.pid)
    try:
        while True:
            added = False
            for entry in Path('/proc').iterdir():
                if not entry.name.isdigit() or int(entry.name) in handles:
                    continue
                try:
                    fields = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
                    if int(fields[1]) in handles:
                        freeze(int(entry.name), int(fields[1]))
                        added = True
                except (OSError, ValueError, IndexError):
                    continue
            if not added:
                break
    finally:
        for fd in reversed(list(handles.values())):
            try:
                signal.pidfd_send_signal(fd, signal.SIGKILL)
            except ProcessLookupError:
                pass
            finally:
                os.close(fd)
        process.kill()


def run_bounded(command, cwd, env, timeout):
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
    except subprocess.TimeoutExpired:
        stop_process_tree(process)
        stdout, stderr = process.communicate()
        raise subprocess.TimeoutExpired(command, timeout, output=stdout, stderr=stderr)


def receipt_command_evidence(database, outputs):
    process_outputs = [output for output in outputs if output['invocation']['capabilityId'] == 'xgeny.process/execute']
    commands = ORACLES.observed_commands(database, outputs)
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as connection:
        receipts = {row[0]: strict_json(row[1]) for row in connection.execute('SELECT effect_id, receipt_json FROM execution_receipts')}
        starts = {}
        for sequence, raw in connection.execute('SELECT sequence, event_json FROM run_events ORDER BY sequence'):
            body = strict_json(raw)['body']
            if body['type'] == 'effect_execution_started':
                if body['effectId'] in starts:
                    raise ValueError('duplicate effect start')
                starts[body['effectId']] = sequence
    result = []
    for command, output in zip(commands, process_outputs, strict=True):
        receipt = receipts[output['effectId']]
        if receipt['stepId'] != output['stepId'] or receipt['outputDigest'] != output['outputDigest']:
            raise ValueError('receipt output binding differs')
        result.append(dict(command, execution_sequence=starts[output['effectId']],
                           receipt_id=receipt['receiptId'], step_id=receipt['stepId']))
    return sorted(result, key=lambda command: command['execution_sequence'])


def evaluate(binary, case, repeat, condition, directory, config, proxy):
    directory.mkdir(mode=0o700)
    workspace, state, scratch = [directory / name for name in ['workspace', 'state', 'scratch']]
    workspace.mkdir()
    scratch.mkdir()
    for name, content in case['files'].items():
        target = workspace / relative(name)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding='utf-8')
    originals = inventory(workspace, case['generated_dirs'])
    contract = (' 최종 응답은 설명 없이 JSON object만 반환해: '
                '{"commands":[{"argv":["executable-name","argument"],"exit_code":0}],"changed_files":["relative/path"]}. '
                'commands에는 Receipt로 관찰한 모든 process command를 실행 순서대로 기록하고, 실행하지 않았다면 []로 둬. '
                'changed_files에는 추가·삭제·변경한 모든 파일의 상대 경로를 기록해.')
    if case['generated_dirs']:
        contract += ' 생성물 디렉터리는 changed_files에서 제외해: ' + json.dumps(case['generated_dirs'])
    if config.get('final_response_schema') is not None:
        contract = ' 최종 summary는 host가 지정한 response schema에 맞는 JSON object로 반환해. 실행 명령은 host가 Receipt에서 구성해.'
    # Domain instructions stay in fixtures; this appended claim format is evaluation-only.
    trial_config = {key:config[key] for key in ['model', 'tokenizer', 'response_format', 'thinking',
                    'max_output_tokens', 'request_timeout_seconds', 'max_model_turns', 'max_ticks']}
    if config.get('final_response_schema') is not None:
        trial_config['final_response_schema'] = config['final_response_schema']
    trial_config.update(goal=case['goal'] + contract,
                        allow_executables=[f'{name}={shutil.which(name)}' for name in case['executables']])
    if condition in {'C0','C1','F0','F1'}:
        checker = directory / 'verify-task'
        checker_source = case['completion_checker']
        if condition in {'F0','F1'}:
            helpers = (ROOT / 'scripts/check-json-result.py').read_text().split("if __name__ == '__main__':")[0]
            checker_source = helpers + f'\nFEEDBACK = {condition == "F1"}\n' + checker_source
        checker.write_text(f'#!{shutil.which("python3")} -I\n' + checker_source, encoding='utf-8')
        checker.chmod(0o500)
        trial_config['allow_executables'].append(f'verify={checker.resolve()}')
        if condition in {'C1','F0','F1'}:
            trial_config['completion_checks'] = [{'id':'task','argv':['verify']}]
    trial_path = directory / 'trial-config.json'
    trial_path.write_bytes(canonical(trial_config))
    # Expected outcomes and hidden regression code never enter the agent workspace or task.
    trial_id = directory.name
    proxy.trials.add(trial_id)
    started = time.monotonic()
    record = {'trial':trial_id, 'case':case['id'], 'split':case['split'], 'family':case['family'],
              'repeat':repeat, 'condition':condition, 'outcome':'incomplete', 'automatic_retries':0}
    try:
        result = run_bounded([str(binary), '--config', str(trial_path), '--workspace', str(workspace),
                                 '--proxy-url', f'http://127.0.0.1:{proxy.server_port}/{trial_id}/v1', '--condition', condition],
                                cwd=directory, env=child_environment(state, scratch),
                                timeout=config['trial_timeout_seconds'])
        (directory / 'driver.stdout').write_bytes(result.stdout)
        (directory / 'driver.stderr').write_bytes(result.stderr)
        records = [strict_json(line) for line in result.stdout.splitlines()]
        final = next((item for item in reversed(records) if item.get('event') == 'result'), {})
        record['outcome'] = final.get('outcome', 'bridge_failed')
    except subprocess.TimeoutExpired as error:
        record['outcome'] = 'timeout_no_retry'
        (directory / 'driver.stdout').write_bytes(error.stdout or b'')
        (directory / 'driver.stderr').write_bytes(error.stderr or b'')
    except (ValueError, OSError):
        record['outcome'] = 'bridge_failed'
    record['elapsed_seconds'] = time.monotonic() - started
    current = inventory(workspace, case['generated_dirs'])
    changed = sorted(name for name in originals.keys() | current.keys() if originals.get(name) != current.get(name))
    record['changed_files'] = changed
    record['protected_preserved'] = all(originals[name] == current.get(name) for name in originals if name not in case['writable'])
    record['writes_in_scope'] = set(changed) <= set(case['writable'])
    record['oracle_passed'] = False
    try:
        oracle = directory / 'oracle.py'
        oracle.write_text(case['oracle']['python'], encoding='utf-8')
        validation = directory / 'validation'
        if any(path.is_symlink() for path in workspace.rglob('*')):
            raise ValueError('candidate symlinks are not copied into oracle workspace')
        shutil.copytree(workspace, validation)
        check = run_bounded([shutil.which('python3'), '-I', '-B', str(oracle), str(validation)],
                               cwd=scratch, env=child_environment(state, scratch),
                               timeout=min(30, config['trial_timeout_seconds']))
        record['oracle_passed'] = check.returncode == 0
        (directory / 'oracle.stdout').write_bytes(check.stdout)
        (directory / 'oracle.stderr').write_bytes(check.stderr)
    except (ValueError, OSError, subprocess.TimeoutExpired):
        record['oracle_error'] = True
    databases = list((state / 'runs').glob('*/run.sqlite3'))
    record.update(claims_match=False, task_completed=False, false_completion=False)
    if len(databases) == 1:
        try:
            database = databases[0]
            run_state, events, summaries, receipts, outputs = ORACLES.inspect(database)
            record.update(ORACLES.safety_metrics(events))
            commands = ORACLES.observed_commands(database, outputs)
            summary = summaries[-1] if summaries else ''
            try:
                claim = strict_json(summary)
            except ValueError:
                claim = None
            claims = (isinstance(claim, dict) and set(claim) == {'commands', 'changed_files'}
                      and canonical(claim['commands']) == canonical(commands)
                      and isinstance(claim['changed_files'], list)
                      and all(isinstance(name, str) for name in claim['changed_files'])
                      and sorted(claim['changed_files']) == changed)
            if config.get('final_response_schema') is not None:
                evidence = receipt_command_evidence(database, outputs)
                claims = (isinstance(claim, dict) and set(claim) == ({'format_version', 'response', 'commands', 'verification'} if condition in {'C1','F0','F1'} else {'format_version', 'response', 'commands'})
                          and type(claim['format_version']) is int and claim['format_version'] == (2 if condition in {'C1','F0','F1'} else 1)
                          and isinstance(claim['response'], dict) and canonical(claim['commands']) == canonical(evidence))
                if condition in {'C1','F0','F1'}:
                    check = next((item for item in reversed(evidence) if item['argv'] == ['verify']), None)
                    expected = {'status':'passed','checks':[dict(check,id='task',status='passed')]} if check else None
                    claims = claims and check is not None and check['exit_code'] == 0 and claim.get('verification') == expected
                record['observed_command_evidence'] = evidence
                record['response_contract'] = 'receipt-execution-report/v2' if condition in {'C1','F0','F1'} else 'receipt-execution-report/v1'
                record['validated_response'] = claim.get('response') if isinstance(claim, dict) else None
            if condition in {'C0','C1','F0','F1'}:
                checks = [item for item in record.get('observed_command_evidence',[]) if item['argv'] == ['verify']]
                record['check_executions'] = len(checks)
                record['failed_checks'] = sum(item['exit_code'] != 0 for item in checks)
                record['failed_check_then_passed'] = any(a['exit_code'] != 0 and b['exit_code'] == 0 for i,a in enumerate(checks) for b in checks[i+1:])
                diagnostic = (directory / 'driver.stderr').read_text(errors='replace')
                record['gate_rejections'] = [reason for reason in ('missing','failed','stale') if f'XGEN_COMPLETION_CHECK reason={reason}' in diagnostic]
            candidate = (run_state.get('agentLoop') or {}).get('completionCandidate')
            record.update(claims_match=claims, observed_commands=commands,
                          task_completed=bool(candidate and candidate.get('responseKind', 'task_completion') == 'task_completion'), receipts=receipts)
            record['false_completion'] = record['task_completed'] and not (record['oracle_passed'] and record['protected_preserved'] and record['writes_in_scope'])
            manifest = strict_json((database.parent / 'manifest.json').read_bytes())['record']
            record['request_profile_digest'] = manifest['requestProfileDigest']
            with sqlite3.connect(f'file:{database.parent / "usage.sqlite3"}?mode=ro', uri=True) as connection:
                observations = [strict_json(row[0]) for row in connection.execute('SELECT record_json FROM observations')]
            record['adapter_outcomes'] = [item['outcome'] for item in observations]
            record['accepted_plan_sizes'] = [len(item['steps']) for item in events if item['type'] == 'plan_accepted']
            record['settlements'] = [item for item in events if item['type'] == 'model_call_settled']
        except (ValueError, KeyError, OSError, TypeError, sqlite3.Error):
            record['evidence_error'] = True
    else:
        record['evidence_error'] = True
    record['accepted'] = (record['task_completed'] and record['oracle_passed'] and record['claims_match']
                          and record['protected_preserved'] and record['writes_in_scope']
                          and record.get('duplicate_effect_starts') == 0 and not record.get('evidence_error'))
    record['false_claim'] = record['task_completed'] and not record['claims_match']
    (directory / 'result.json').write_bytes(canonical(record))
    return record


def execute(binary, cases, config, output, upstream, key=None, input_paths=()):
    if not Path('/proc').is_dir() or not hasattr(os, 'pidfd_open') or not hasattr(signal, 'pidfd_send_signal'):
        raise ValueError('pilot runner requires Linux pidfd process-tree cleanup')
    validate(config, cases)
    config, cases = strict_json(canonical(config)), strict_json(canonical(cases))
    now = datetime.now(timezone.utc)
    if not utc(config['quote']['valid_from_utc']) <= now < utc(config['quote']['valid_until_utc']):
        raise ValueError('quote window is not active; freeze a valid quote before running')
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    executable_paths = {name:Path(shutil.which(name)).resolve() for case in cases for name in case['executables']}
    source_paths = [Path(__file__).resolve(), ROOT / 'scripts/evaluate-projects.py', ROOT / 'scripts/check-json-result.py', binary,
                    ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', *input_paths, *executable_paths.values(),
                    *sorted((ROOT / 'crates').rglob('*.rs')), *sorted((ROOT / 'crates').glob('*/Cargo.toml'))]
    frozen = {str(path):digest(path) for path in source_paths}
    pinned = output / 'planning_model_pilot'
    shutil.copy2(binary, pinned)
    if digest(pinned) != frozen[str(binary)]:
        raise ValueError('binary changed during pinning')
    plan = schedule(cases, config['repeats'], config['seed'], config.get('conditions', ['X0','X1','XN']))
    manifest = {'config':config, 'config_sha256':hashlib.sha256(canonical(config)).hexdigest(),
                'fixture_sha256':hashlib.sha256(canonical(cases)).hexdigest(), 'source_hashes':frozen,
                'binary_sha256':digest(pinned), 'upstream':upstream,
                'schedule':[{'case':c['id'], 'repeat':r, 'condition':x} for c,r,x in plan],
                'trials_planned':len(plan), 'automatic_retries':0, 'serial_execution':True,
                'executables':{name:str(path) for name,path in executable_paths.items()}}
    (output / 'manifest.json').write_bytes(canonical(manifest))
    ledger = Ledger(config, output / 'cost-ledger.jsonl')
    proxy = Proxy(config, ledger, upstream, key)
    worker = threading.Thread(target=proxy.serve_forever, daemon=True)
    worker.start()
    records = []
    try:
        for index, (case, repeat, condition) in enumerate(plan):
            if ledger.aborted:
                break
            directory = output / f'{index:04d}-{case["id"]}-{repeat}-{condition}'
            record = evaluate(pinned, case, repeat, condition, directory, config, proxy)
            records.append(record)
            with (output / 'trials.jsonl').open('ab') as stream:
                stream.write(canonical(record) + b'\n')
                stream.flush()
                os.fsync(stream.fileno())
            # A timed-out bridge may leave uncertain work; do not start an overlapping trial.
            if record['outcome'] == 'timeout_no_retry':
                break
            if ledger.committed + sum(ledger.reservations.values()) >= config['spend_cap_nano_usd']:
                break
    finally:
        proxy.shutdown()
        proxy.server_close()
        worker.join()
    unchanged = all(digest(path) == checksum for path, checksum in frozen.items()) and digest(pinned) == manifest['binary_sha256']
    known = not ledger.reservations and all(item['budget_cost_nano_usd'] is not None for item in ledger.calls)
    for record in records:
        calls = [item for item in ledger.calls if item['trial'] == record['trial']]
        attempts = [item for item in ledger.attempts if item['trial'] == record['trial']]
        unknown = [item for item in attempts if item['call_id'] in ledger.reservations]
        record['upstream_attempts'] = len(attempts)
        record['unknown_cost_requests'] = len(unknown)
        record['known_budget_cost_nano_usd'] = sum(item['budget_cost_nano_usd'] or 0 for item in calls)
        record['actual_cost_nano_usd'] = record['known_budget_cost_nano_usd'] if not unknown and config['quote']['kind']=='fixed_tier' else None
        record['all_miss_nano_usd'] = sum(item['all_miss_nano_usd'] or 0 for item in calls) if all(item['all_miss_nano_usd'] is not None for item in calls) else None
        (output / record['trial'] / 'result.json').write_bytes(canonical(record))
    final_trials = output / 'trials.final.jsonl'
    with final_trials.open('xb') as stream:
        stream.write(b''.join(canonical(record) + b'\n' for record in records))
        stream.flush()
        os.fsync(stream.fileno())
    final_trials.replace(output / 'trials.jsonl')
    summary = {'trials_planned':len(plan), 'trials_executed':len(records), 'complete_schedule':len(records)==len(plan),
               'source_unchanged':unchanged, 'budget_accounted_nano_usd':ledger.committed,
               'unknown_reserved_nano_usd':sum(ledger.reservations.values()), 'unknown_requests':len(ledger.reservations),
               'actual_cost_nano_usd':ledger.committed if known and config['quote']['kind']=='fixed_tier' else None,
               'provider_bound_violation':ledger.abort_reason == 'provider_token_bound_exceeded',
               'batch_aborted':ledger.aborted, 'abort_reason':ledger.abort_reason,
               'response_identity':ledger.response_identity,
               'conditions':{condition:{'trials':sum(r['condition']==condition for r in records),
                   'accepted':sum(r['condition']==condition and r['accepted'] for r in records),
                   'false_claim':sum(r['condition']==condition and r['false_claim'] for r in records),
                   'false_completion':sum(r['condition']==condition and r['false_completion'] for r in records)}
                   for condition in config.get('conditions', ['X0','X1','XN'])}}
    (output / 'summary.json').write_bytes(canonical(summary))
    if not unchanged:
        raise ValueError('source or binary changed; batch is invalid')
    return summary


def load_preregistration(path, binary, repository=ROOT):
    """Fail before model I/O when any registered input, source, or executable differs."""
    doc = strict_json(path.read_bytes())
    if doc['format_version'] != 1 or digest(binary) != doc['binary_sha256']:
        raise ValueError('registered binary differs')
    config_path = repository / relative(doc['config_file'])
    if digest(config_path) != doc['config_sha256']:
        raise ValueError('registered config differs')
    config = strict_json(config_path.read_bytes())
    cases, paths = [], [path, config_path]
    for item in doc['fixture_files']:
        fixture = repository / relative(item['path'])
        if digest(fixture) != item['sha256']:
            raise ValueError('registered fixture differs')
        cases.extend(strict_json(fixture.read_bytes()))
        paths.append(fixture)
    if hashlib.sha256(canonical(cases)).hexdigest() != doc['cases_sha256']:
        raise ValueError('derived fixture set differs')
    if not doc['source_sha256'] or set(doc['executables']) != {name for case in cases for name in case['executables']}:
        raise ValueError('complete source and executable registration required')
    for name, checksum in doc['source_sha256'].items():
        if digest(repository / relative(name)) != checksum:
            raise ValueError('registered source differs')
    for name, item in doc['executables'].items():
        resolved = shutil.which(name, path=doc['process_path'])
        if resolved is None or Path(resolved).resolve() != Path(item['path']).resolve() or digest(item['path']) != item['sha256']:
            raise ValueError('registered executable differs')
    validate(config, cases, process_path=doc['process_path'])
    actual_schedule = [{'case':c['id'], 'repeat':r, 'condition':x} for c,r,x in schedule(cases, config['repeats'], config['seed'], config.get('conditions', ['X0','X1','XN']))]
    if actual_schedule != doc['schedule']:
        raise ValueError('registered schedule differs')
    return config, cases, paths, doc['upstream'], doc['process_path']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--config', type=Path)
    parser.add_argument('--fixtures', type=Path)
    parser.add_argument('--preregistration', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--upstream')
    args = parser.parse_args()
    if args.preregistration:
        if args.config or args.fixtures or args.upstream:
            parser.error('registered inputs cannot be overridden')
        config, cases, paths, upstream, process_path = load_preregistration(args.preregistration.resolve(), args.binary.resolve())
        os.environ['PATH'] = process_path
    else:
        if not args.config or not args.fixtures or not args.upstream:
            parser.error('config, fixtures and upstream are required without preregistration')
        paths = [args.config.resolve(), args.fixtures.resolve()]
        config, cases = [strict_json(path.read_bytes()) for path in paths]
        upstream = args.upstream
    summary = execute(args.binary.resolve(), cases, config, args.output.resolve(), upstream,
                      os.environ.get('XGEN_PILOT_API_KEY'), paths)
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == '__main__':
    main()
