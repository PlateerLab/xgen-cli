import copy
from datetime import datetime, timedelta, timezone
import http.server
import importlib.util
import json
import os
import sqlite3
from pathlib import Path
import tempfile
import subprocess
import sys
import types
import threading
import unittest

SPEC = importlib.util.spec_from_file_location('planning_model', Path(__file__).parents[1] / 'evaluate-planning-model.py')
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


def config():
    now = datetime.now(timezone.utc)
    return {'model':'fixture-model', 'tokenizer':'fixture-tokenizer', 'response_format':'json_object',
            'thinking':'disabled', 'max_output_tokens':1024, 'request_timeout_seconds':5,
            'max_model_turns':8, 'max_ticks':256, 'max_input_tokens':100000,
            'trial_timeout_seconds':20, 'spend_cap_nano_usd':1000000000, 'seed':17, 'repeats':1,
            'quote':{'kind':'fixed_tier', 'source':'synthetic test quote; not live pricing',
                     'valid_from_utc':(now-timedelta(hours=1)).isoformat(),
                     'valid_until_utc':(now+timedelta(hours=1)).isoformat(),
                     'cached_micro_usd_per_million':10000, 'input_micro_usd_per_million':1000000,
                     'output_micro_usd_per_million':2000000}}


def cases():
    return [{'id':name, 'split':'design', 'family':family,
             'goal':'input.txt를 읽고 ready와 줄바꿈을 담은 result.txt를 만들어줘.' +
                    (' python3 -c "print(\'ready\')"도 실행해줘.' if family=='process' else ''),
             'files':{'input.txt':'ready\n'}, 'writable':['result.txt'], 'generated_dirs':[],
             'executables':['python3'] if family=='process' else [],
             'oracle':{'python':"from pathlib import Path\nimport sys\np=Path(sys.argv[1])\nassert (p/'result.txt').read_text() == 'ready\\n'\nassert (p/'input.txt').read_text() == 'ready\\n'\n"}}
            for name,family in [('literal-artifact','artifact'),('process-artifact','process')]]


class RunnerContractTests(unittest.TestCase):
    def test_receipt_evidence_uses_start_order_and_checks_output_binding(self):
        with tempfile.TemporaryDirectory() as temp:
            database=Path(temp)/'run.sqlite3'
            with sqlite3.connect(database) as connection:
                connection.executescript('CREATE TABLE run_events(sequence INTEGER,event_json BLOB); CREATE TABLE execution_receipts(effect_id TEXT,receipt_json BLOB);')
                outputs=[]
                for effect,step,sequence,code in [('effect-b','step-b',10,7),('effect-a','step-a',20,0)]:
                    connection.execute('INSERT INTO run_events VALUES (?,?)',(sequence,json.dumps({'body':{'type':'effect_execution_started','effectId':effect}}).encode()))
                    connection.execute('INSERT INTO execution_receipts VALUES (?,?)',(effect,json.dumps({'stepId':step,'receiptId':'receipt-'+step,'outputDigest':'digest-'+step}).encode()))
                    outputs.append({'effectId':effect,'stepId':step,'outputDigest':'digest-'+step,'invocation':{'capabilityId':'xgeny.process/execute'},'output':{'exitCode':code}})
            with sqlite3.connect(database.parent/'materials.sqlite3') as connection:
                connection.execute('CREATE TABLE material_recipe(record BLOB)')
                for step in ['step-a','step-b']:
                    connection.execute('INSERT INTO material_recipe VALUES (?)',(json.dumps({'stepId':step,'capability':{'capabilityId':'xgeny.process/execute'},'arguments':{'executable':'process:primary/executables/helper','args':[step]}}).encode(),))
            evidence=M.receipt_command_evidence(database,list(reversed(outputs)))
            self.assertEqual([x['step_id'] for x in evidence],['step-b','step-a'])
            self.assertEqual([x['exit_code'] for x in evidence],[7,0])
            self.assertEqual([x['execution_sequence'] for x in evidence],[10,20])
            outputs[0]['outputDigest']='unbound'
            with self.assertRaises(ValueError):M.receipt_command_evidence(database,outputs)

    def test_preregistration_checks_binary_config_fixtures_sources_and_schedule(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            cfg=config()
            inputs={'config.json':M.canonical(cfg),'fixtures.json':M.canonical(cases()),
                    'source.rs':b'fixture source','binary':b'fixture executable'}
            for name,raw in inputs.items():(root/name).write_bytes(raw)
            executable=Path(M.shutil.which('python3')).resolve()
            doc={'format_version':1,'config_file':'config.json','config_sha256':M.digest(root/'config.json'),
                 'fixture_files':[{'path':'fixtures.json','sha256':M.digest(root/'fixtures.json')}],
                 'cases_sha256':M.hashlib.sha256(M.canonical(cases())).hexdigest(),
                 'source_sha256':{'source.rs':M.digest(root/'source.rs')},'binary_sha256':M.digest(root/'binary'),
                 'executables':{'python3':{'path':str(executable),'sha256':M.digest(executable)}},
                 'process_path':os.environ['PATH'],'upstream':'https://example.invalid/v1',
                 'schedule':[{'case':c['id'],'repeat':r,'condition':x} for c,r,x in M.schedule(cases(),cfg['repeats'],cfg['seed'])]}
            registration=root/'prereg.json';registration.write_bytes(M.canonical(doc))
            loaded=M.load_preregistration(registration,root/'binary',root)
            self.assertEqual(loaded[1],cases())
            ambient_path=os.environ['PATH']
            try:
                os.environ['PATH']=''
                self.assertEqual(M.load_preregistration(registration,root/'binary',root)[1],cases())
            finally:
                os.environ['PATH']=ambient_path
            for name in inputs:
                (root/name).write_bytes(b'changed')
                with self.assertRaises(ValueError):M.load_preregistration(registration,root/'binary',root)
                (root/name).write_bytes(inputs[name])
            doc['schedule'].reverse();registration.write_bytes(M.canonical(doc))
            with self.assertRaises(ValueError):M.load_preregistration(registration,root/'binary',root)

    def test_response_model_and_fingerprint_drift_stop_batch(self):
        for drift in [{'model':'different-model'}, {'system_fingerprint':'fp_changed'}]:
            with tempfile.TemporaryDirectory() as temp:
                cfg=config()
                ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
                first={'model':cfg['model'],'system_fingerprint':'fp_initial',
                       'usage':{'prompt_tokens':10,'completion_tokens':2,'total_tokens':12,'prompt_cache_hit_tokens':0,'prompt_cache_miss_tokens':10}}
                ledger.reserve('first','t')
                ledger.settle('first','t',json.dumps(first).encode(),200)
                second=dict(first,**drift)
                ledger.reserve('second','t')
                ledger.settle('second','t',json.dumps(second).encode(),200)
                self.assertTrue(ledger.aborted)
                self.assertEqual(ledger.abort_reason,'response_identity_changed')
                self.assertFalse(ledger.reserve('third','t'))
        with tempfile.TemporaryDirectory() as temp:
            cfg=config()
            cfg['expected_response_identity']=[cfg['model'],'fp_frozen']
            ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
            ledger.reserve('first','t')
            ledger.settle('first','t',json.dumps({'model':cfg['model'],'system_fingerprint':'fp_other'}).encode(),200)
            self.assertTrue(ledger.aborted)
        with tempfile.TemporaryDirectory() as temp:
            cfg=config()
            cfg['expected_response_identity']=[cfg['model'],'fp_frozen']
            ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
            ledger.reserve('missing-identity','t')
            ledger.settle('missing-identity','t',b'{"error":"unavailable"}',502)
            self.assertTrue(ledger.aborted)
            self.assertEqual(ledger.abort_reason,'response_identity_unavailable')
            self.assertIn('missing-identity',ledger.reservations)

    def test_proxy_blocks_upstream_after_unknown_reserved_cost(self):
        class Unknown(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_):
                pass
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                self.server.calls+=1
                self.send_response(502)
                self.send_header('Content-Length','2')
                self.end_headers()
                self.wfile.write(b'{}')
        upstream=http.server.HTTPServer(('127.0.0.1',0),Unknown)
        upstream.calls=0
        upstream_thread=threading.Thread(target=upstream.serve_forever,daemon=True)
        upstream_thread.start()
        with tempfile.TemporaryDirectory() as temp:
            cfg=config()
            cfg['spend_cap_nano_usd']=M.price_nano({'input':cfg['max_input_tokens'],'output':cfg['max_output_tokens'],'cache':0},cfg['quote'])
            ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
            proxy=M.Proxy(cfg,ledger,f'http://127.0.0.1:{upstream.server_port}/v1')
            proxy.trials.update(['original','later'])
            worker=threading.Thread(target=proxy.serve_forever,daemon=True)
            worker.start()
            try:
                for call_id,expected in [('a',502),('b',429)]:
                    body={'model':cfg['model'],'stream':False,'max_tokens':cfg['max_output_tokens'],
                          'messages':[{'content':'fixture system'},{'content':json.dumps({'callId':call_id})}]}
                    request=M.urllib.request.Request(f'http://127.0.0.1:{proxy.server_port}/original/v1/chat/completions',data=json.dumps(body).encode())
                    with self.assertRaises(M.urllib.error.HTTPError) as error:
                        M.urllib.request.urlopen(request,timeout=5)
                    self.assertEqual(error.exception.code,expected)
                    error.exception.close()
                self.assertEqual(upstream.calls,1)
                self.assertEqual(ledger.attempts[0]['trial'],'original')
                self.assertIsNone(ledger.calls[0]['budget_cost_nano_usd'])
            finally:
                proxy.shutdown();proxy.server_close();worker.join()
                upstream.shutdown();upstream.server_close();upstream_thread.join()

    def test_missing_journal_is_retained_as_failed_trial(self):
        with tempfile.TemporaryDirectory() as temp:
            proxy=types.SimpleNamespace(trials=set(),server_port=9)
            result=M.evaluate(Path('/bin/false'),cases()[0],0,'X1',Path(temp)/'trial',config(),proxy)
            self.assertTrue(result['evidence_error'])
            self.assertFalse(result['accepted'])
            self.assertEqual(result['outcome'],'bridge_failed')
            self.assertTrue((Path(temp)/'trial/result.json').exists())

    def test_timeout_stops_owned_process_descendants(self):
        with tempfile.TemporaryDirectory() as temp:
            pid_path=Path(temp)/'child.pid'
            code="import subprocess,sys,time; from pathlib import Path; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); Path(sys.argv[1]).write_text(str(p.pid)); time.sleep(60)"
            with self.assertRaises(subprocess.TimeoutExpired):
                M.run_bounded([sys.executable,'-c',code,str(pid_path)],Path(temp),dict(os.environ),1)
            pid=int(pid_path.read_text())
            status=Path(f'/proc/{pid}/stat')
            try:
                # Linux may briefly expose X (dead) before the process disappears.
                self.assertIn(status.read_text().rsplit(')',1)[1].split()[0],('Z','X','x'))
            except FileNotFoundError:
                pass

    def test_budget_reservation_retains_unknown_and_blocks_next_call(self):
        with tempfile.TemporaryDirectory() as temp:
            cfg=config()
            bound=M.price_nano({'input':cfg['max_input_tokens'],'output':cfg['max_output_tokens'],'cache':0},cfg['quote'])
            cfg['spend_cap_nano_usd']=bound
            ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
            self.assertTrue(ledger.reserve('a','t'))
            ledger.settle('a','t',b'{"error":"timeout"}',502)
            self.assertFalse(ledger.reserve('b','t'))
            self.assertEqual(sum(ledger.reservations.values()),bound)
            self.assertEqual(ledger.committed,0)
            self.assertIsNone(ledger.calls[0]['budget_cost_nano_usd'])
            events=[json.loads(line) for line in ledger.path.read_text().splitlines()]
            self.assertEqual([event['event'] for event in events],['reserved','settled','blocked'])

    def test_usage_rejection_costs_and_bound_violation(self):
        cfg=config()
        good={'model':cfg['model'],'usage':{'prompt_tokens':100,'completion_tokens':20,'total_tokens':120,
              'prompt_cache_hit_tokens':30,'prompt_cache_miss_tokens':70,
              'completion_tokens_details':{'reasoning_tokens':10}}}
        tokens=M.usage(json.dumps(good).encode(),cfg['model'])
        self.assertEqual(M.price_nano(tokens,cfg['quote']),110300)
        # A malformed proposal still carries chargeable usage; reasoning is a subset.
        with tempfile.TemporaryDirectory() as temp:
            ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
            ledger.reserve('bad-proposal','t')
            ledger.settle('bad-proposal','t',json.dumps(good).encode(),200)
            self.assertEqual(ledger.committed,110300)
            self.assertFalse(ledger.reservations)
            oversized=copy.deepcopy(good)
            oversized['usage']['completion_tokens']=cfg['max_output_tokens']+1
            oversized['usage']['total_tokens']=100+cfg['max_output_tokens']+1
            ledger.reserve('over-bound','t')
            ledger.settle('over-bound','t',json.dumps(oversized).encode(),200)
            self.assertTrue(ledger.aborted)
            self.assertIn('over-bound',ledger.reservations)
        conflicting=copy.deepcopy(good)
        conflicting['usage']['prompt_tokens_details']={'cached_tokens':31}
        self.assertIsNone(M.usage(json.dumps(conflicting).encode(),cfg['model']))
        self.assertIsNone(M.usage(b'{"model":"x","model":"fixture-model"}',cfg['model']))

    def test_quote_expiry_and_duplicate_call_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            cfg=config()
            ledger=M.Ledger(cfg,Path(temp)/'ledger.jsonl')
            self.assertTrue(ledger.reserve('once','t'))
            self.assertFalse(ledger.reserve('once','t'))
            cfg['quote']['valid_until_utc']=(datetime.now(timezone.utc)-timedelta(seconds=1)).isoformat()
            self.assertFalse(ledger.reserve('expired','t'))

    def test_schedule_is_paired_reproducible_and_balanced(self):
        plan=M.schedule(cases(),3,17)
        self.assertEqual(plan,M.schedule(cases(),3,17))
        self.assertEqual(len(plan),18)
        orders=[]
        for start in range(0,len(plan),3):
            block=plan[start:start+3]
            self.assertEqual(len({(c['id'],r) for c,r,x in block}),1)
            self.assertEqual({x for c,r,x in block},{'X0','X1','XN'})
            orders.append(tuple(x for c,r,x in block))
        self.assertEqual(len(set(orders)),6)
        cfg=config()
        M.validate(cfg,cases())
        broken=cases()
        broken[0]['files']['../oracle.py']='leak'
        with self.assertRaises(ValueError):
            M.validate(cfg,broken)

    def test_secret_environment_does_not_reach_bridge(self):
        prior=os.environ.get('XGEN_PILOT_API_KEY')
        os.environ['XGEN_PILOT_API_KEY']='synthetic-never-forward-to-tools'
        try:
            self.assertNotIn('XGEN_PILOT_API_KEY',M.child_environment(Path('/state'),Path('/scratch')))
        finally:
            if prior is None:
                del os.environ['XGEN_PILOT_API_KEY']
            else:
                os.environ['XGEN_PILOT_API_KEY']=prior

    @unittest.skipUnless(os.environ.get('XGEN_PILOT_TEST_BINARY'), 'requires explicit built example binary')
    def test_real_driver_filesystem_process_claims_and_oracles(self):
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_):
                pass

            def do_POST(self):
                body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                system=body['messages'][0]['content']
                context=json.loads(body['messages'][1]['content'])['planningContext']
                self.server.assertions.append('oracle' not in json.dumps(context))
                process='xgeny.process/execute' in json.dumps(context['capabilities'])
                steps=[('read','xgeny.fs/read-text',{'path':'input.txt'}),
                       ('write','xgeny.fs/write-atomic',{'path':'result.txt','content':'ready\n','expectedDigest':None})]
                if process:
                    steps.append(('execute','xgeny.process/execute',{'executable':'python3','args':['-c',"print('ready')"],
                                  'cwd':'.','env':{},'timeoutMs':1000,'maxOutputBytes':1024}))
                completed=len(context['toolOutputs'])
                if completed < len(steps):
                    limit=4 if 'maxProposalSteps=4' in system else 1
                    proposed=[]
                    for key,cap,args in steps[completed:completed+limit]:
                        proposed.append({'key':key,'objective':'Perform the specified operation','dependsOn':[],
                                         'capability':{'capabilityId':cap,'contractVersion':'1.0.0'},'arguments':args})
                    proposal={'formatVersion':1,'kind':'plan','steps':proposed,'summary':''}
                else:
                    commands=[{'argv':['python3','-c',"print('ready')"],'exit_code':0}] if process else []
                    proposal={'formatVersion':1,'kind':'completion_candidate','steps':[],
                              'summary':json.dumps({'commands':commands,'changed_files':['result.txt']})}
                    if 'RECEIPT_EXECUTION_REPORT_V1' in system:
                        proposal['summary']=json.dumps({'outcome':'completed'})
                    if getattr(self.server,'wrong_claim',False):
                        proposal['summary']=json.dumps({'commands':[],'changed_files':[]})
                if getattr(self.server,'invalid_proposal',False):
                    proposal['unknown_field']=True
                response=json.dumps({'model':'fixture-model','choices':[{'index':0,'message':{'role':'assistant','content':json.dumps(proposal)},'finish_reason':'stop'}],
                    'usage':{'prompt_tokens':100,'completion_tokens':20,'total_tokens':120,'prompt_cache_hit_tokens':30,'prompt_cache_miss_tokens':70}}).encode()
                self.send_response(200)
                self.send_header('Content-Type','application/json')
                self.send_header('Content-Length',str(len(response)))
                self.end_headers()
                self.wfile.write(response)
        server=http.server.HTTPServer(('127.0.0.1',0),Upstream)
        server.assertions=[]
        thread=threading.Thread(target=server.serve_forever,daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as temp:
                output=Path(temp)/'results'
                summary=M.execute(Path(os.environ['XGEN_PILOT_TEST_BINARY']).resolve(),cases(),config(),output,f'http://127.0.0.1:{server.server_port}/v1',input_paths=[Path(__file__)])
                self.assertTrue(summary['complete_schedule'])
                self.assertTrue(summary['source_unchanged'])
                results=[json.loads(line) for line in (output/'trials.jsonl').read_text().splitlines()]
                self.assertTrue(all(r['accepted'] for r in results),json.dumps(results,indent=2))
                self.assertTrue(all(server.assertions))
                self.assertEqual(summary['unknown_requests'],0)
                self.assertTrue(all(r['actual_cost_nano_usd'] is not None for r in results))
                self.assertTrue(any(r['observed_commands'] for r in results))
                self.assertTrue(any(max(r['accepted_plan_sizes'])>1 for r in results if r['condition']=='XN'))
                contracted_config=config()
                contracted_config['final_response_schema']={'type':'object','properties':{'outcome':{'type':'string','enum':['completed']}},'required':['outcome'],'additionalProperties':False}
                contracted=Path(temp)/'contracted'
                M.execute(Path(os.environ['XGEN_PILOT_TEST_BINARY']).resolve(),cases(),contracted_config,contracted,f'http://127.0.0.1:{server.server_port}/v1',input_paths=[Path(__file__)])
                reports=[json.loads(line) for line in (contracted/'trials.jsonl').read_text().splitlines()]
                self.assertTrue(all(r['accepted'] and r['validated_response']=={'outcome':'completed'} for r in reports))
                self.assertTrue(any(r['observed_command_evidence'] for r in reports))
                server.invalid_proposal=True
                rejected=Path(temp)/'rejected'
                M.execute(Path(os.environ['XGEN_PILOT_TEST_BINARY']).resolve(),cases(),config(),rejected,f'http://127.0.0.1:{server.server_port}/v1',input_paths=[Path(__file__)])
                failures=[json.loads(line) for line in (rejected/'trials.jsonl').read_text().splitlines()]
                self.assertTrue(all(not r['accepted'] and r['actual_cost_nano_usd']>0 for r in failures))
                self.assertTrue(all(r['adapter_outcomes']==['response_rejected'] for r in failures))
                server.invalid_proposal=False
                server.wrong_claim=True
                wrong=Path(temp)/'wrong-claims'
                M.execute(Path(os.environ['XGEN_PILOT_TEST_BINARY']).resolve(),cases(),config(),wrong,f'http://127.0.0.1:{server.server_port}/v1',input_paths=[Path(__file__)])
                failures=[json.loads(line) for line in (wrong/'trials.jsonl').read_text().splitlines()]
                self.assertTrue(all(r['oracle_passed'] and r['false_claim'] and not r['accepted'] for r in failures))
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == '__main__':
    unittest.main()
