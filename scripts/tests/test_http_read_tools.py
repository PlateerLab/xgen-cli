"""Real loopback HTTP + compiled harness tests. No production API or paid LLM calls."""
import copy
import http.server
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import tempfile
import threading
import unittest

import test_tool_graph_worker as base
from test_tool_graph_worker import WORKER, asset_spec, calendar_spec


def held_out_spec(name):
    if name == "shipments":
        return {"openapi":"3.1.0","info":{"title":"Shipment tracking","version":"1"},
            "components":{"securitySchemes":{"access":{"type":"http","scheme":"bearer"}},
                "schemas":{"Result":{"type":"object","required":["tracking","delivered"],"properties":{"tracking":{"type":"string"},"delivered":{"type":"boolean"}},"additionalProperties":False}}},
            "security":[{"access":[]}],"paths":{"/shipments/{tracking}":{"get":{
                "operationId":"readShipment","summary":"Read shipment tracking status",
                "parameters":[{"name":"tracking","in":"path","required":True,"schema":{"type":"string","pattern":"^[A-Z][0-9]+$"}},
                    {"name":"history","in":"query","schema":{"type":"boolean"}}],
                "responses":{"200":{"description":"Status","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Result"}}}}}}}}}
    return {"swagger":"2.0","info":{"title":"Sensor measurements","version":"1"},"basePath":"/v2","produces":["application/json"],
        "paths":{"/sensors/{sensorId}":{"get":{"operationId":"readSensor","summary":"Read sensor measurement",
            "parameters":[{"name":"sensorId","in":"path","required":True,"type":"integer","minimum":1},
                {"name":"limit","in":"query","required":True,"type":"integer","minimum":1,"maximum":5}],
            "responses":{"200":{"description":"Measurement","schema":{"type":"object","required":["values"],"properties":{"values":{"type":"array","items":{"type":"number"}}},"additionalProperties":False}}}}}}}


class ContractTests(unittest.TestCase):
    setUp = base.WorkerTests.setUp
    call = base.WorkerTests.call
    install = base.WorkerTests.install
    def test_original_constraints_refs_and_legacy_base_path(self):
        for name, source, tool in [("shipments",held_out_spec("shipments"),"readShipment"),("sensors",held_out_spec("sensors"),"readSensor")]:
            self.install(name,source)
            descriptor=self.call("describe",name=name,tool=tool)["http_read"]
            self.assertTrue(descriptor["supported"],descriptor)
            contract=descriptor["contract"]
            self.assertEqual(descriptor["contract_digest"],WORKER.digest(contract))
            self.assertEqual(contract["source_digest"],WORKER.digest(source))
            if name=="shipments":
                self.assertTrue(contract["bearer_required"])
                self.assertEqual(contract["parameters"][0]["schema"]["pattern"],"^[A-Z][0-9]+$")
                self.assertEqual(contract["responses"]["200"]["required"],["tracking","delivered"])
            else:
                self.assertEqual(contract["base_path"],"/v2")
                self.assertEqual(contract["parameters"][1]["schema"]["maximum"],5)

    def test_unsupported_contracts_are_not_invented(self):
        self.assertEqual(WORKER.http_read_contract({}, {"metadata":{}}),{"supported":False,"error":"http_original_source_unavailable"})
        for kind in ["post","header","array","xml","recursive"]:
            source=asset_spec()
            op=source["paths"]["/assets"]["get"]
            if kind=="post": source["paths"]["/assets"]={"post":op}
            elif kind=="header": op["parameters"]=[{"name":"x","in":"header","schema":{"type":"string"}}]
            elif kind=="array": op["parameters"]=[{"name":"x","in":"query","schema":{"type":"array","items":{"type":"string"}}}]
            elif kind=="xml": op["responses"]["200"]["content"]={"application/xml":{"schema":{"type":"object"}}}
            else:
                source["components"]={"schemas":{"Recursive":{"$ref":"#/components/schemas/Recursive"}}}
                op["responses"]["200"]["content"]["application/json"]["schema"]={"$ref":"#/components/schemas/Recursive"}
            self.install(kind,source)
            descriptor=self.call("describe",name=kind,tool="listAssets")["http_read"]
            self.assertFalse(descriptor["supported"],descriptor)
        legacy=calendar_spec()
        legacy["paths"]["/events"]["get"]["produces"]=["application/xml"]
        self.install("legacyxml",legacy)
        self.assertFalse(self.call("describe",name="legacyxml",tool="listCalendarEvents")["http_read"]["supported"])


@unittest.skipUnless(os.environ.get("XGEN_TOOL_TEST_BINARY"),"compiled CLI opt-in")
class HttpCliTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory=Path(self.temp.name)
        self.workspace=self.directory/"workspace"
        self.workspace.mkdir()
        self.state=self.directory/"state"
        self.secret=secrets.token_hex(24)
        self.env=dict(os.environ,XGEN_STATE_HOME=str(self.state),XGEN_FIXTURE_BEARER=self.secret)
        self.env.pop("XGEN_OPENAI_API_KEY",None)
        self.calls=[]
        self.requests=[]
        self.response={"tracking":"A42","delivered":True}
        self.status=200
        self.redirect=None
        self.accepted=threading.Event()
        self.release=None
        self.media="application/json"
        test=self
        class ApiHandler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_GET(self):
                test.calls.append((self.path,self.headers.get("Authorization")))
                test.accepted.set()
                if test.release: test.release.wait(timeout=30)
                body=test.response if isinstance(test.response,bytes) else json.dumps(test.response).encode()
                self.send_response(test.status)
                self.send_header("Content-Type",test.media)
                if test.redirect: self.send_header("Location",test.redirect)
                self.send_header("Content-Length",str(len(body)))
                self.end_headers()
                try: self.wfile.write(body)
                except (BrokenPipeError,ConnectionResetError): pass
        self.api=self.server(ApiHandler)
        self.proposals=[]
        class ModelHandler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_POST(self):
                payload=json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                test.requests.append(payload)
                index=len(test.requests)-1
                proposal=test.proposals[index] if index<len(test.proposals) else {"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":"Completed fixture read."}
                if callable(proposal): proposal=proposal(payload)
                body=json.dumps({"id":"fixture","model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":json.dumps(proposal)},"finish_reason":"stop"}]}).encode()
                self.send_response(200);self.send_header("Content-Type","application/json");self.send_header("Content-Length",str(len(body)));self.end_headers();self.wfile.write(body)
        self.model=self.server(ModelHandler)

    def server(self,handler):
        server=http.server.ThreadingHTTPServer(("127.0.0.1",0),handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        def cleanup(): server.shutdown();server.server_close();thread.join(timeout=5)
        self.addCleanup(cleanup)
        return server

    def cli(self,*args,check=True):
        result=subprocess.run([os.environ["XGEN_TOOL_TEST_BINARY"],*args],env=self.env,capture_output=True,timeout=150)
        if check: self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertNotIn(self.secret.encode(),result.stdout+result.stderr)
        return result

    def install(self,name="shipments"):
        source=self.directory/(name+".json");source.write_text(json.dumps(held_out_spec(name)))
        self.cli("tools","import","--name",name,"--source",str(source))
        args=["tools","connect","--name",name,"--base-url",f"http://127.0.0.1:{self.api.server_port}","--allow-get"]
        if name=="shipments": args += ["--bearer-env","XGEN_FIXTURE_BEARER"]
        self.cli(*args)
        target="readShipment" if name=="shipments" else "readSensor"
        result=json.loads(self.cli("tools","describe","--name",name,target).stdout)
        self.assertTrue(result["http_read"]["supported"],result)
        return {"collection":name,"tool":target,"contractDigest":result["http_read"]["contract_digest"],"parameters":{
            "path":{"tracking":"A42"} if name=="shipments" else {"sensorId":7},
            "query":{"history":True} if name=="shipments" else {"limit":2}}}

    @staticmethod
    def plan(key,capability,args):
        return {"formatVersion":1,"kind":"plan","summary":"","steps":[{"key":key,"objective":"Read connected API","dependsOn":[],"capability":{"capabilityId":capability,"contractVersion":"2.0.0" if capability=="xgen.tools/describe" else "1.0.0"},"arguments":args}]}

    @staticmethod
    def context(payload):
        return json.loads(next(m["content"] for m in payload["messages"] if m["role"]=="user"))["planningContext"]

    def run_cli(self,allow=True):
        args=["run","--workspace",str(self.workspace),"--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--model","fixture-model","--tokenizer","fixture-tokenizer","--allow-dir",".","--allow-remote-model-egress"]
        if allow: args += ["--allow-read"]
        return self.cli(*args,"Read connected fixture API.",check=False)

    def manifest(self):
        return json.loads(next((self.state/"runs").glob("*/manifest.json")).read_text())

    def receipts(self):
        db=next((self.state/"runs").glob("*/run.sqlite3"))
        with sqlite3.connect(db) as store:
            return [json.loads(row[0]) for row in store.execute("SELECT receipt_json FROM execution_receipts ORDER BY event_sequence")]

    def test_search_describe_read_verified_response_on_two_held_out_systems(self):
        # These domains were not used to design the adapter; same engine, different source formats.
        for name in ["shipments","sensors"]:
            with self.subTest(system=name):
                self.state=self.directory/(name+"-state");self.env["XGEN_STATE_HOME"]=str(self.state)
                self.calls.clear();self.requests.clear()
                arguments=self.install(name)
                expected={"tracking":"A42","delivered":True} if name=="shipments" else {"values":[18.5,19.25]}
                self.response=expected
                def complete(payload):
                    output=next(row["output"] for row in self.context(payload)["toolOutputs"] if row["capability"]["capabilityId"]=="xgen.http/read")
                    self.assertTrue(output["ok"],output)
                    self.assertEqual(output["body"],expected)
                    self.assertNotIn(self.secret,json.dumps(payload))
                    return {"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":json.dumps(output["body"],sort_keys=True)}
                self.proposals=[self.plan("find","xgen.tools/search",{"collection":name,"query":"Read shipment tracking status" if name=="shipments" else "Read sensor measurement","topK":1}),self.plan("describe","xgen.tools/describe",{"collection":name,"tool":arguments["tool"]}),self.plan("read","xgen.http/read",arguments),complete]
                result=self.run_cli()
                self.assertEqual(result.returncode,0,result.stderr.decode())
                self.assertEqual(len(self.requests),4)
                self.assertEqual(len(self.calls),1)
                self.assertEqual(self.calls[0][0],"/shipments/A42?history=true" if name=="shipments" else "/v2/sensors/7?limit=2")
                self.assertEqual(self.calls[0][1],"Bearer "+self.secret if name=="shipments" else None)
                self.assertIn(json.dumps(expected,sort_keys=True),result.stdout.decode())
                rows=self.receipts();self.assertEqual(len(rows),3)
                self.assertTrue(all(row["status"]=="succeeded" and all(rule["result"]=="passed" for rule in row["verification"]) for row in rows),rows)
                self.assertIn(name,self.manifest()["record"]["httpReadConnections"])
                replay=self.cli("resume",self.manifest()["record"]["runId"])
                self.assertIn(json.dumps(expected,sort_keys=True),replay.stdout.decode())
                self.assertEqual(len(self.calls),1)
                self.assertEqual(len(self.requests),4)
                for path in self.state.rglob("*"):
                    if path.is_file(): self.assertNotIn(self.secret.encode(),path.read_bytes(),str(path))

    def test_http_failures_do_not_publish_unverified_body_or_follow_redirect(self):
        arguments=self.install()
        cases=[(302,b"redirect"),(500,b"internal"),(200,b"not-json"),(200,{"tracking":42,"delivered":True}),(200,b"x"*32769),(200,{"tracking":self.secret,"delivered":True}), (200,json.dumps({"tracking":self.secret,"delivered":True}).replace(self.secret, "".join("\\u%04x" % ord(c) for c in self.secret)).encode())]
        for status,body in cases:
            self.calls.clear();self.requests.clear();self.status=status;self.response=body
            self.redirect=f"http://127.0.0.1:{self.api.server_port}/must-not-follow"
            self.proposals=[self.plan("read","xgen.http/read",arguments)]
            result=self.run_cli()
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(len(self.calls),1)
            self.assertEqual(len(self.requests),1)
        # Each run stores a failed receipt, never the rejected response body.
        for db in (self.state/"runs").glob("*/run.sqlite3"):
            with sqlite3.connect(db) as store:
                rows=[json.loads(r[0]) for r in store.execute("SELECT receipt_json FROM execution_receipts")]
                self.assertEqual(len(rows),1)
                self.assertEqual(rows[0]["status"],"failed",rows)
        for path in self.state.rglob("*"):
            if path.is_file(): self.assertNotIn(self.secret.encode(),path.read_bytes(),str(path))

    def test_missing_or_invalid_runtime_credential_never_calls_api(self):
        args=self.install()
        for credential in [None,"", "line\nbreak"]:
            if credential is None: self.env.pop("XGEN_FIXTURE_BEARER",None)
            else: self.env["XGEN_FIXTURE_BEARER"]=credential
            self.requests.clear();self.proposals=[self.plan("read","xgen.http/read",args)]
            result=self.run_cli()
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(self.calls,[])
        self.env["XGEN_FIXTURE_BEARER"]=self.secret

    def test_killed_http_execution_is_not_automatically_reissued(self):
        args=self.install();self.proposals=[self.plan("read","xgen.http/read",args)]
        self.release=threading.Event()
        command=[os.environ["XGEN_TOOL_TEST_BINARY"],"run","--workspace",str(self.workspace),"--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--model","fixture-model","--tokenizer","fixture-tokenizer","--allow-dir",".","--allow-read","--allow-remote-model-egress","Read connected fixture API."]
        process=subprocess.Popen(command,env=self.env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        try:
            self.assertTrue(self.accepted.wait(timeout=30),"HTTP effect did not start")
        finally:
            process.kill();process.communicate(timeout=10);self.release.set()
        run_id=self.manifest()["record"]["runId"]
        resumed=self.cli("resume",run_id,"--workspace",str(self.workspace),"--allow-dir",".","--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--allow-read",check=False)
        self.assertNotEqual(resumed.returncode,0)
        self.assertEqual(len(self.calls),1)
        self.assertEqual(len(self.requests),1)
        self.assertEqual(self.receipts(),[])

    def test_invalid_parameters_or_digest_never_call_api(self):
        original=self.install()
        for alteration in ["missing","unknown","type","digest","escape"]:
            args=copy.deepcopy(original)
            if alteration=="missing": args["parameters"]["path"]={}
            elif alteration=="unknown": args["parameters"]["query"]["arbitrary"]="x"
            elif alteration=="type": args["parameters"]["query"]["history"]="true"
            elif alteration=="digest": args["contractDigest"]="a"*64
            else: args["parameters"]["path"]["tracking"]="../escape"
            self.requests.clear();self.calls.clear();self.proposals=[self.plan("read","xgen.http/read",args)]
            result=self.run_cli()
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(self.calls,[])

    def test_approval_resume_connection_drift_and_no_duplicate_get(self):
        args=self.install();self.proposals=[self.plan("read","xgen.http/read",args)]
        result=self.run_cli(allow=False)
        self.assertEqual(self.calls,[])
        record=self.manifest()["record"];run_id=record["runId"]
        connection=self.state/"http-connections/shipments.json"
        original=connection.read_bytes();changed=json.loads(original);changed["base_url"]=f"http://127.0.0.1:{self.api.server_port}/changed";connection.write_text(json.dumps(changed))
        resumed=self.cli("resume",run_id,"--workspace",str(self.workspace),"--allow-dir",".","--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--allow-read",check=False)
        self.assertNotEqual(resumed.returncode,0)
        self.assertEqual(self.calls,[])
        connection.write_bytes(original)
        resumed=self.cli("resume",run_id,"--workspace",str(self.workspace),"--allow-dir",".","--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--allow-read",check=False)
        self.assertEqual(resumed.returncode,10,resumed.stderr.decode())
        self.assertEqual(len(self.calls),1)
        self.cli("resume",run_id,"--workspace",str(self.workspace),"--allow-dir",".","--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--allow-read",check=False)
        self.assertEqual(len(self.calls),1)


if __name__=="__main__": unittest.main()
