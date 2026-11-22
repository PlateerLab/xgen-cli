#!/usr/bin/env python3
"""Bounded agy proposal transport for the frozen public-CLI coding evaluation."""
import argparse
from collections import Counter
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('coding_baseline', ROOT / 'scripts/evaluate-coding-baseline.py')
BASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BASE)
S = BASE.SUPPORT

AGENT = '''---
name: xgen-planner-only
description: Return host proposals without executing any action.
mainAgent: true
subagent: false
model: inherit
excludeDefaultComponents: true
tools: []
inheritMcp: false
inheritCustomizations: false
preloadSkills: []
rules: []
agents: []
hooks: []
commandExecutionPolicy: "off"
---
You are a proposal-only planner. Use only the supplied host instructions, request and observations.
Return exactly one proposal JSON object, without Markdown fences or commentary.
Never call your own tools, access files, execute commands, launch agents, or perform proposed actions.
The host owns tools, permissions, memory and execution. Listed host capabilities are proposal choices,
not tools you can execute. Claim completion only when supplied observations establish the result.
'''


def validate(config):
    expected = {'model', 'repeats', 'seed', 'max_cli_calls', 'max_cli_calls_per_trial',
                'request_timeout_seconds', 'trial_timeout_seconds', 'batch_timeout_seconds',
                'max_model_turns', 'max_ticks', 'host_max_output_tokens', 'max_prompt_bytes',
                'max_stdout_bytes', 'max_stderr_bytes'}
    if not isinstance(config, dict) or set(config) != expected:
        raise ValueError('explicit agy evaluation config required')
    if not isinstance(config['model'], str) or not config['model'].strip():
        raise ValueError('exact model required')
    if any(type(config[k]) is not int or config[k] <= 0 for k in expected - {'model', 'seed'}):
        raise ValueError('positive integer bounds required')
    if type(config['seed']) is not int or not config['request_timeout_seconds'] < config['trial_timeout_seconds'] <= config['batch_timeout_seconds']:
        raise ValueError('ordered timeout bounds and integer seed required')
    if config['max_prompt_bytes'] > 100_000:
        raise ValueError('print-mode argument must fit the bounded local transport')


def usage(value):
    """CLI-reported subtotals; missing or inconsistent fields never become zero."""
    result = {'input': None, 'output': None, 'cache': None, 'reasoning': None}
    if not isinstance(value, dict):
        return result
    for target, source in [('input', 'input_tokens'), ('output', 'output_tokens'),
                           ('cache', 'cache_read_tokens'), ('reasoning', 'thinking_tokens')]:
        number = value.get(source)
        if type(number) is int and number >= 0:
            result[target] = number
    if (result['input'] is not None and result['output'] is not None
            and (type(value.get('total_tokens')) is not int
                 or value['total_tokens'] != result['input'] + result['output'])):
        return {key: None for key in result}
    if result['cache'] is not None and (result['input'] is None or result['cache'] > result['input']):
        result['cache'] = None
    if result['reasoning'] is not None and (result['output'] is None or result['reasoning'] > result['output']):
        result['reasoning'] = None
    return result


def consume_event(raw, events, model):
    event = S.strict_json(raw)
    if not isinstance(event, dict):
        raise ValueError('invalid_event')
    events.append(event)
    if event.get('event') == 'init':
        info = event.get('init', {})
        if info.get('agent') != 'xgen-planner-only' or info.get('model') != model:
            raise ValueError('identity_mismatch')
    step = event.get('step_update', {})
    if isinstance(step, dict) and (step.get('step_type') == 'tool' or step.get('tool_info') or step.get('subagent_info')):
        raise ValueError('native_tool_observed')


def agent_environment():
    return {key: os.environ[key] for key in ['PATH', 'HOME', 'LANG', 'LC_ALL', 'SYSTEMROOT',
        'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_DATA_HOME', 'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS']
        if key in os.environ}


def bounded_agent(command, workspace, config, deadline, cancelled, request_cancelled=None):
    """Drain bounded pipes and stop the owned process tree on timeout or native tool use."""
    started = time.monotonic()
    process = subprocess.Popen(command, cwd=workspace, env=agent_environment(), stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    buffers = {'stdout': bytearray(), 'stderr': bytearray()}
    pending, events, reason = bytearray(), [], None
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ, 'stdout')
        selector.register(process.stderr, selectors.EVENT_READ, 'stderr')
        try:
            while selector.get_map():
                remaining = min(deadline, started + config['request_timeout_seconds']) - time.monotonic()
                if cancelled.is_set() or (request_cancelled is not None and request_cancelled.is_set()):
                    raise ValueError('cancelled')
                if remaining <= 0:
                    raise ValueError('request_timeout')
                for key, _ in selector.select(min(remaining, .1)):
                    chunk = os.read(key.fd, 16_384)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    name = key.data
                    bound = config[f'max_{name}_bytes']
                    available = max(0, bound - len(buffers[name]))
                    buffers[name].extend(chunk[:available])
                    if len(chunk) > available:
                        raise ValueError('output_limit')
                    if name == 'stdout':
                        pending.extend(chunk)
                        while b'\n' in pending:
                            line, _, tail = pending.partition(b'\n')
                            pending = bytearray(tail)
                            if line.strip():
                                consume_event(line, events, config['model'])
            if pending.strip():
                consume_event(pending, events, config['model'])
            process.wait(timeout=max(.1, min(deadline, started + config['request_timeout_seconds']) - time.monotonic()))
        except (ValueError, OSError, subprocess.TimeoutExpired) as error:
            known = {'identity_mismatch', 'native_tool_observed', 'cancelled', 'request_timeout', 'output_limit'}
            reason = str(error) if isinstance(error, ValueError) and str(error) in known else 'invalid_agent_stream'
        finally:
            if reason or process.poll() is None:
                if process.poll() is None:
                    S.stop_process_tree(process)
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
            process.stdout.close()
            process.stderr.close()
    return {'events': events, 'reason': reason, 'exit_code': process.returncode,
            'elapsed_seconds': time.monotonic() - started,
            'stdout': bytes(buffers['stdout']), 'stderr': bytes(buffers['stderr'])}


class Bridge(http.server.HTTPServer):
    def __init__(self, config, output, workspace, agent_command, settings, pins):
        self.config, self.output, self.workspace = config, output, workspace
        self.agent_command, self.settings, self.pins = agent_command, settings, pins
        self.trials, self.calls, self.seen = set(), [], set()
        self.cancelled = threading.Event()
        self.idle = threading.Event()
        self.idle.set()
        self.active_trial, self.active_cancel = None, None
        self.deadline = time.monotonic() + config['batch_timeout_seconds']
        self.abort_reason = None
        super().__init__(('127.0.0.1', 0), Handler)

    def append(self, value):
        with (self.output / 'agy-ledger.jsonl').open('ab') as stream:
            stream.write(S.canonical(value) + b'\n')
            stream.flush()
            os.fsync(stream.fileno())

    def unchanged(self):
        return (not (Path.home() / '.gemini/config/hooks.json').exists()
                and all(path.is_file() and S.digest(path) == checksum for path, checksum in self.pins.items()))

    def reserve(self, trial, call_id):
        if not self.unchanged():
            self.abort_reason = 'pinned_inputs_changed'
        if self.abort_reason or self.cancelled.is_set() or time.monotonic() >= self.deadline:
            raise ValueError('batch_stopped')
        if (call_id in self.seen or len(self.calls) >= self.config['max_cli_calls']
                or sum(call['trial'] == trial for call in self.calls) >= self.config['max_cli_calls_per_trial']):
            raise ValueError('call_limit_or_duplicate')
        self.seen.add(call_id)
        record = {'index': len(self.calls) + 1, 'trial': trial, 'status': 'reserved',
                  'accepted': False, 'reason': None, 'native_tool_events': 0,
                  'usage': usage(None), 'usd_cost': None}
        self.append(record)
        self.calls.append(record)
        return record


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, value):
        raw = S.canonical(value)
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        try:
            self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        self.connection.settimeout(5)
        record = None
        try:
            route = self.path.split('/')
            size = int(self.headers.get('Content-Length', '0'))
            if (len(route) != 5 or route[2:] != ['v1', 'chat', 'completions']
                    or route[1] not in self.server.trials or not 0 < size <= self.server.config['max_prompt_bytes']):
                raise ValueError('invalid_request')
            body = S.strict_json(self.rfile.read(size))
            request = S.strict_json(body['messages'][1]['content'])
            if (body['model'] != self.server.config['model'] or body.get('stream') is not False
                    or body['max_tokens'] != self.server.config['host_max_output_tokens']
                    or not isinstance(request['callId'], str) or not request['callId']):
                raise ValueError('invalid_request')
            prompt = ('HOST SYSTEM INSTRUCTIONS:\n' + body['messages'][0]['content']
                      + '\nHOST REQUEST AND OBSERVATIONS:\n' + body['messages'][1]['content']
                      + '\nReturn exactly one proposal JSON object. No Markdown fences or commentary. '
                      'All actions remain proposals for the host. Use only supplied observations; '
                      'never call your own tools or access the environment.')
            if len(prompt.encode()) > self.server.config['max_prompt_bytes']:
                raise ValueError('prompt_limit')
            record = self.server.reserve(route[1], request['callId'])
            self.server.active_trial, self.server.active_cancel = route[1], threading.Event()
            self.server.idle.clear()
            before = S.inventory(self.server.workspace)
            command = [*self.server.agent_command, '--agent', 'xgen-planner-only', '--model', self.server.config['model'],
                       '--settings', str(self.server.settings), '--disable-slash-commands', '--output-format', 'stream-json',
                       '--print-timeout', f'{self.server.config["request_timeout_seconds"]}s', '-p', prompt]
            result = bounded_agent(command, self.server.workspace, self.server.config, self.server.deadline,
                                   self.server.cancelled, self.server.active_cancel)
            prefix = self.server.output / f'agy-call-{record["index"]:04d}'
            prefix.with_suffix('.ndjson').write_bytes(result['stdout'])
            prefix.with_suffix('.stderr').write_bytes(result['stderr'])
            record.update(elapsed_seconds=result['elapsed_seconds'], exit_code=result['exit_code'])
            finals = [event.get('result', event) for event in result['events'] if event.get('event') == 'result']
            if finals:
                record['usage'] = usage(finals[-1].get('usage'))
            record['native_tool_events'] = int(result['reason'] == 'native_tool_observed')
            if before != S.inventory(self.server.workspace) or not self.server.unchanged():
                self.server.abort_reason = 'agent_workspace_or_pins_changed'
                raise ValueError('agent_workspace_or_pins_changed')
            if result['reason']:
                raise ValueError(result['reason'])
            if (result['exit_code'] != 0 or len(finals) != 1 or finals[0].get('status') != 'SUCCESS'
                    or sum(event.get('event') == 'init' for event in result['events']) != 1):
                raise ValueError('agent_failed')
            response = finals[0]['response']
            proposal = S.strict_json(response)
            if not isinstance(proposal, dict) or set(proposal) != {'formatVersion', 'kind', 'steps', 'summary'}:
                raise ValueError('invalid_proposal_json')
            record.update(accepted=True, status='settled', proposal_kind=proposal['kind'])
            envelope = {'model': body['model'], 'choices': [{'index': 0, 'message': {
                'role': 'assistant', 'content': response}, 'finish_reason': 'stop'}]}
            observed = record['usage']
            if all(observed[key] is not None for key in ['input', 'output', 'cache']):
                envelope['usage'] = {'prompt_tokens': observed['input'], 'completion_tokens': observed['output'],
                    'total_tokens': observed['input'] + observed['output'],
                    'prompt_tokens_details': {'cached_tokens': observed['cache']}}
            self.reply(200, envelope)
        except (ValueError, KeyError, TypeError, AttributeError, OSError) as error:
            known = {'invalid_request', 'prompt_limit', 'call_limit_or_duplicate', 'batch_stopped', 'identity_mismatch',
                     'native_tool_observed', 'cancelled', 'request_timeout', 'output_limit', 'agent_failed',
                     'agent_workspace_or_pins_changed', 'invalid_agent_stream', 'invalid_proposal_json'}
            reason = str(error) if isinstance(error, ValueError) and str(error) in known else 'invalid_proposal_json'
            if record is not None:
                record.update(status='settled', reason=reason)
            else:
                self.server.append({'event': 'blocked', 'reason': reason})
            self.reply(502 if record is not None else 429, {'error': reason})
        finally:
            if record is not None:
                self.server.append(record)
                self.server.active_trial, self.server.active_cancel = None, None
                self.server.idle.set()


def attach_usage(record, bridge):
    calls = [call for call in bridge.calls if call['trial'] == record['trial']]
    record['agy_cli_calls'] = len(calls)
    record['agy_rejections'] = dict(Counter(call['reason'] for call in calls if call['reason']))
    record['native_tool_events'] = sum(call['native_tool_events'] for call in calls)
    record['unknown_usage_calls'] = sum(any(call['usage'][key] is None for key in ['input', 'output', 'cache']) for call in calls)
    record['token_subtotals'] = {key: sum(call['usage'][key] for call in calls if call['usage'][key] is not None)
                               for key in ['input', 'output', 'cache']}
    record['complete_token_usage'] = record['unknown_usage_calls'] == 0
    record['usd_cost'] = None
    if any(not call['accepted'] for call in calls):
        record['accepted'] = False
        record['outcome'] = 'agy_transport_failure'


def aggregate(records, planned, bridge, stop_reason):
    result = BASE.quality_aggregate(records, planned)
    result.update(agy_cli_calls=len(bridge.calls), usd_cost=None, cost_status='UNKNOWN',
                  provider_internal_request_count=None, stop_reason=stop_reason,
                  native_tool_events=sum(call['native_tool_events'] for call in bridge.calls),
                  agy_rejections=dict(Counter(call['reason'] for call in bridge.calls if call['reason'])),
                  unknown_usage_calls=sum(row['unknown_usage_calls'] for row in records),
                  pinned_inputs_unchanged=bridge.unchanged())
    return result


def execute(binary, agy, config, cases, output):
    validate(config)
    BASE.validate_cases(cases)
    if not Path('/proc').is_dir() or not hasattr(os, 'pidfd_open'):
        raise ValueError('Linux pidfd support required')
    if (Path.home() / '.gemini/config/hooks.json').exists():
        raise ValueError('global hooks require a separately isolated evaluation')
    catalog = S.run_bounded([str(agy), 'models'], ROOT, agent_environment(), 30)
    if catalog.returncode or config['model'] not in [line.split()[0] for line in catalog.stdout.decode().splitlines() if line.split()]:
        raise ValueError('model catalog authentication or exact model check failed')
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    workspace = output / 'agent-workspace'
    agent = workspace / '.agents/agents/xgen-planner-only.md'
    agent.parent.mkdir(parents=True)
    agent.write_text(AGENT)
    settings = output / 'agent-settings.json'
    settings.write_bytes(S.canonical({'trustedWorkspaces': [str(workspace)], 'agentMode': '',
        'toolPermission': 'strict', 'allowNonWorkspaceAccess': False, 'disableSlashCommands': True,
        'useG1Credits': False, 'notifications': False, 'permissions': {'deny': [f'{action}(*)'
            for action in ['command', 'read_file', 'write_file', 'read_url', 'execute_url', 'mcp']]}}))
    pinned, pinned_agy = output / 'xgen', output / 'agy'
    originals = {binary: S.digest(binary), agy: S.digest(agy)}
    shutil.copy2(binary, pinned)
    shutil.copy2(agy, pinned_agy)
    if S.digest(pinned) != originals[binary] or S.digest(pinned_agy) != originals[agy]:
        raise ValueError('executable changed during pinning')
    discovery = S.run_bounded([str(pinned_agy), '--settings', str(settings), 'agent'], workspace, agent_environment(), 30)
    if discovery.returncode or 'xgen-planner-only' not in discovery.stdout.decode().splitlines():
        raise ValueError('custom agent not discovered')
    config = S.strict_json(S.canonical(config))
    cases = S.strict_json(S.canonical(cases))
    plan = BASE.schedule(cases, config['repeats'], config['seed'])
    sources = [Path(__file__).resolve(), ROOT / 'scripts/evaluate-coding-baseline.py',
               ROOT / 'scripts/evaluate-projects.py', ROOT / 'scripts/evaluate-planning-model.py']
    pins = {path: S.digest(path) for path in [*sources, pinned, pinned_agy, agent, settings]}
    bridge = Bridge(config, output, workspace, [str(pinned_agy)], settings, pins)
    manifest = {'format_version': 1, 'config': config, 'entrypoint': 'public-run-resume',
                'cases_sha256': hashlib.sha256(S.canonical(cases)).hexdigest(),
                'pins': {str(path.relative_to(output)) if path.is_relative_to(output) else path.name: checksum
                         for path, checksum in pins.items()},
                'schedule': [{'case': case['id'], 'repeat': repeat} for case, repeat in plan],
                'automatic_trial_retries': 0, 'usd_cost': None,
                'limitations': ['official CLI agent runtime remains', 'native calls rejected after observation',
                    'output bytes bounded, native output token ceiling not enforceable',
                    'provider internal retries/request count and billing unknown']}
    (output / 'manifest.json').write_bytes(S.canonical(manifest))
    (output / 'cases.json').write_bytes(S.canonical(cases))
    worker = threading.Thread(target=bridge.serve_forever, daemon=True)
    worker.start()
    records, stop_reason = [], None
    try:
        for index, (case, repeat) in enumerate(plan):
            if (bridge.abort_reason or not bridge.unchanged() or time.monotonic() >= bridge.deadline
                    or len(bridge.calls) >= config['max_cli_calls']):
                stop_reason = bridge.abort_reason or 'batch_bound_or_pin_change'
                break
            trial_config = {'model': config['model'], 'tokenizer': config['model'], 'response_format': 'json_object',
                'thinking': 'default', 'max_output_tokens': config['host_max_output_tokens'],
                'request_timeout_seconds': config['request_timeout_seconds'] + 5,
                'max_model_turns': config['max_model_turns'], 'max_ticks': config['max_ticks'],
                'trial_timeout_seconds': min(config['trial_timeout_seconds'], bridge.deadline - time.monotonic())}
            row = BASE.trial(pinned, case, repeat, output / f'trial-{index + 1:03d}', trial_config, bridge)
            # Do not let a late response from a timed-out trial starve or contaminate the next one.
            if bridge.active_trial == row['trial'] and bridge.active_cancel is not None:
                bridge.active_cancel.set()
            if not bridge.idle.wait(5):
                bridge.abort_reason = 'agent_cleanup_incomplete'
            attach_usage(row, bridge)
            records.append(row)
            (output / row['trial'] / 'result.json').write_bytes(S.canonical(row))
            (output / 'trials.jsonl').write_bytes(b''.join(S.canonical(record) + b'\n' for record in records))
            (output / 'summary.json').write_bytes(S.canonical(aggregate(records, len(plan), bridge, stop_reason)))
            print(json.dumps({'completed': len(records), 'planned': len(plan), 'case': row['case'],
                              'repeat': repeat, 'outcome': row['outcome'], 'passed': sum(r['accepted'] for r in records),
                              'agy_cli_calls': len(bridge.calls)}), flush=True)
    finally:
        bridge.cancelled.set()
        bridge.shutdown()
        bridge.server_close()
        worker.join()
    for row in records:
        attach_usage(row, bridge)
        (output / row['trial'] / 'result.json').write_bytes(S.canonical(row))
    result = aggregate(records, len(plan), bridge, stop_reason)
    result['call_count_semantics'] = 'reserved CLI attempts; provider internal requests unknown'
    (output / 'trials.jsonl').write_bytes(b''.join(S.canonical(row) + b'\n' for row in records))
    (output / 'summary.json').write_bytes(S.canonical(result))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--agy', type=Path, default=shutil.which('agy'))
    parser.add_argument('--config', type=Path, default=ROOT / 'evals/coding-baseline/agy-config.json')
    parser.add_argument('--cases', type=Path, default=ROOT / 'evals/coding-baseline/cases.json')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = execute(args.binary.resolve(strict=True), Path(args.agy).resolve(strict=True),
                         S.strict_json(args.config.read_bytes()), S.strict_json(args.cases.read_bytes()),
                         args.output.resolve())
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        parser.exit(2, 'agy baseline preflight failed; check private local configuration and pinned inputs\n')
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result['complete_schedule'] and result['pinned_inputs_unchanged'] and result['passed'] == result['trials_planned'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
