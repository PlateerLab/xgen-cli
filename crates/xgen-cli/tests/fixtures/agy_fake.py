import json
import os
import subprocess
import sys
import time
from pathlib import Path

assert 'SSH_CONNECTION' not in os.environ
assert 'SSH_CLIENT' not in os.environ
assert 'XGEN_OPENAI_API_KEY' not in os.environ
assert 'XGEN_AGY_TEST_SECRET' not in os.environ
if sys.argv[-1] == 'models':
    print('gemini-3.8-flash-low\tFixture Flash')
    print('fixture-other\tOther fixture')
    raise SystemExit(0)
model = sys.argv[sys.argv.index('--model') + 1]
print(json.dumps({'event': 'init', 'init': {'agent': 'xgen-planner-only', 'model': model}}), flush=True)
if MODE in ['native', 'timeout']:
    subprocess.Popen([sys.executable, '-c', 'import time;from pathlib import Path;time.sleep(2);Path(' + repr(MARKER) + ').write_text("leaked")'])
    Path(MARKER + '.cwd').write_text(str(Path.cwd()))
    Path(MARKER + '.pid').write_text(str(os.getpid()))
    if MODE == 'native':
        print(json.dumps({'event': 'step_update', 'step_update': {'step_type': 'tool', 'tool_info': {'name': 'manage_task'}}}), flush=True)
    time.sleep(20)
if MODE == 'flood':
    print('x' * 200000, flush=True)
    time.sleep(20)
prompt = sys.argv[-1]
user = prompt.split('HOST REQUEST AND OBSERVATIONS:\n', 1)[1].rsplit('\nReturn exactly one proposal', 1)[0]
try:
    context = json.loads(user)['planningContext']
except (ValueError, KeyError):
    context = None
proposal = {'formatVersion': 1, 'kind': 'completion_candidate', 'steps': [], 'summary': 'done'}
if context is not None:
    outputs = context['toolOutputs']
    if not outputs:
        if 'result.txt' in context['goal']:
            capability = 'xgeny.fs/write-atomic'
            arguments = {'path': 'result.txt', 'content': 'ready\n', 'expectedDigest': None}
        else:
            capability = 'xgeny.fs/read-text'
            arguments = {'path': 'note.txt'}
        proposal = {'formatVersion': 1, 'kind': 'plan', 'summary': '', 'steps': [
            {'key': 'first', 'objective': 'Perform requested action', 'dependsOn': [],
             'capability': {'capabilityId': capability, 'contractVersion': '1.0.0'}, 'arguments': arguments}]}
    elif len(outputs) == 1 and 'result.txt' in context['goal']:
        proposal = {'formatVersion': 1, 'kind': 'plan', 'summary': '', 'steps': [
            {'key': 'verify', 'objective': 'Verify result', 'dependsOn': [],
             'capability': {'capabilityId': 'xgeny.process/execute', 'contractVersion': '1.0.0'},
             'arguments': {'executable': 'python3', 'args': ['-B', 'tests.py'], 'cwd': '.', 'env': {}, 'timeoutMs': 5000, 'maxOutputBytes': 4096}}]}
if MODE == 'settings':
    Path(sys.argv[sys.argv.index('--settings') + 1]).write_text('{}')
response = json.dumps(proposal)
if MODE == 'duplicate':
    response = '{"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":"done","summary":"other"}'
print(json.dumps({'event': 'result', 'result': {'status': 'SUCCESS', 'response': response,
    'usage': {'input_tokens': 100, 'output_tokens': 20, 'thinking_tokens': 5, 'cache_read_tokens': 150, 'total_tokens': 120}}}), flush=True)
