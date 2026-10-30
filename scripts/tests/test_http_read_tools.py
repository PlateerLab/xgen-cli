"""Real loopback HTTP + compiled harness tests. No production API or paid LLM calls."""
import copy
import gzip
import http.server
import importlib.util
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import tempfile
import threading
import unittest
import urllib.parse

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


class OptionalParameterContractTests(unittest.TestCase):
    @staticmethod
    def describe(source, path="/assets", method="get"):
        artifact = {"metadata": {"xgen_http_sources": {"fixture": source}}}
        tool = {"name": "fixture", "metadata": {"source_url": "fixture", "path": path, "method": method}}
        contract = WORKER.http_read_contract(artifact, tool)
        return contract, tool

    def test_optional_unsupported_parameters_preserve_independent_contracts(self):
        for source, path in [(asset_spec(), "/assets"), (calendar_spec(), "/events")]:
            with self.subTest(path=path):
                parameters = [{"name":"keep","in":"query","type":"string","schema":{"type":"string"}},
                              {"name":"filters","in":"query","type":"array","explode":False,"schema":{"type":"array","items":{"type":"string"}}},
                              {"name":"trace","in":"header","schema":{"type":"string"}}]
                source["paths"][path]["get"]["parameters"] = parameters
                result, tool = self.describe(source, path)
                self.assertTrue(result["supported"], result)
                contract = result["contract"]
                self.assertEqual([p["name"] for p in contract["parameters"]], ["keep"])
                self.assertEqual(contract["excluded_parameters"], [
                    {"location":"query","name":"filters","reason":"http_parameter_serialization_unsupported"},
                    {"location":"header","name":"trace","reason":"http_parameter_location_unsupported"}])
                self.assertEqual(result["contract_digest"], WORKER.digest(contract))
                view = WORKER.describe_view({"collection":"fixture","artifact_digest":"a"*64,"artifact":{"tools":[tool],"metadata":{"source_count":1}}}, tool, result, 0)
                self.assertEqual([p["name"]["text"] for p in view["tool"]["parameters"]], ["keep"])
                self.assertEqual([p["name"]["text"] for p in view["http_read"]["excluded_parameters"]], ["filters","trace"])
                self.assertTrue(view["http_read"]["excluded_parameters_complete"])

    def test_required_shapes_and_path_parameters_still_close_the_operation(self):
        for location, required in [("query",True),("header",True),("path",False)]:
            source = asset_spec()
            source["paths"]["/assets"]["get"]["parameters"] = [{"name":"bad","in":location,"required":required,"explode":False,"schema":{"type":"array","items":{"type":"string"}}}]
            result, _ = self.describe(source)
            self.assertFalse(result["supported"], result)

    def test_overrides_precede_support_classification(self):
        source = asset_spec()
        source["paths"]["/assets"]["parameters"] = [{"name":"filter","in":"query","required":True,"schema":{"type":"array","items":{"type":"string"}}}]
        source["paths"]["/assets"]["get"]["parameters"] = [{"name":"filter","in":"query","schema":{"type":"string"}}]
        result, _ = self.describe(source)
        self.assertTrue(result["supported"], result)
        self.assertEqual(result["contract"]["parameters"][0]["schema"], {"type":"string"})
        self.assertNotIn("excluded_parameters", result["contract"])

    def test_missing_or_malformed_schema_is_not_treated_as_optional_unsupported_shape(self):
        for fields in [{}, {"schema":None}, {"schema":"invalid"},
                       {"schema":{"type":"string"},"content":{"application/json":{}}}]:
            source = asset_spec()
            source["paths"]["/assets"]["get"]["parameters"] = [{"name":"bad","in":"query",**fields}]
            self.assertEqual(self.describe(source)[0], {"supported":False,"error":"http_original_contract_invalid"})
        source = asset_spec()
        source["paths"]["/assets"]["get"]["parameters"] = [
            {"name":"json","in":"query","content":{"application/json":{"schema":{"type":"string"}}}}]
        result, _ = self.describe(source)
        self.assertTrue(result["supported"], result)
        self.assertEqual(result["contract"]["excluded_parameters"][0]["reason"], "http_parameter_serialization_unsupported")

    def test_duplicate_or_malformed_parameters_do_not_remove_required_constraints(self):
        for level in ["path", "operation"]:
            source = asset_spec()
            item = source["paths"]["/assets"]
            owner = item if level == "path" else item["get"]
            owner["parameters"] = [
                {"name":"same","in":"query","required":True,"schema":{"type":"string"}},
                {"name":"same","in":"query","schema":{"type":"array"}}]
            self.assertEqual(self.describe(source)[0], {"supported":False,"error":"http_original_contract_invalid"})
        for bad in [[None], {"name":"not-a-list"}, [{"name":"bad","in":"query","required":0,"schema":{"type":"array"}}]]:
            source = asset_spec()
            source["paths"]["/assets"]["get"]["parameters"] = bad
            self.assertEqual(self.describe(source)[0], {"supported":False,"error":"http_original_contract_invalid"})

    def test_optional_schema_references_are_isolated_but_unknown_parameter_refs_are_not(self):
        source = asset_spec()
        operation = source["paths"]["/assets"]["get"]
        operation["parameters"] = [{"name":"remote","in":"query","schema":{"$ref":"https://example.com/schema.json"}}]
        result, _ = self.describe(source)
        self.assertTrue(result["supported"], result)
        self.assertEqual(result["contract"]["excluded_parameters"][0]["reason"], "http_schema_reference_unsupported")
        operation["parameters"][0]["required"] = True
        self.assertFalse(self.describe(source)[0]["supported"])
        operation["parameters"] = [{"$ref":"https://example.com/parameter.json"}]
        self.assertFalse(self.describe(source)[0]["supported"])
        source["components"] = {"schemas":{"Scalar":{"type":"integer","minimum":2}}}
        operation["parameters"] = [{"name":"value","in":"query","required":True,"schema":{"$ref":"#/components/schemas/Scalar"}}]
        self.assertEqual(self.describe(source)[0]["contract"]["parameters"][0]["schema"], {"type":"integer","minimum":2})

    def test_exclusion_metadata_is_bounded_and_committed_in_the_contract_digest(self):
        source = asset_spec()
        source["paths"]["/assets"]["get"]["parameters"] = [
            {"name": "filter" + str(i) + "x" * 2048, "in":"query", "schema":{"type":"array"}}
            for i in range(100)]
        result, tool = self.describe(source)
        self.assertTrue(result["supported"], result)
        view = WORKER.describe_view({"collection":"fixture","artifact_digest":"a"*64,
            "artifact":{"tools":[tool],"metadata":{"source_count":1}}}, tool, result, 0)
        self.assertLessEqual(len(WORKER.canonical(view)), 48 * 1024)
        self.assertEqual(view["http_read"]["excluded_parameter_count"], 100)
        self.assertFalse(view["http_read"]["excluded_parameters_complete"])
        old_digest = result["contract_digest"]
        source["paths"]["/assets"]["get"]["parameters"][0]["name"] += "changed"
        self.assertNotEqual(self.describe(source)[0]["contract_digest"], old_digest)

    def test_optional_serialization_and_dialect_are_reported_but_security_is_not_dropped(self):
        source = asset_spec()
        source["paths"]["/assets"]["get"]["parameters"] = [
            {"name":"shape","in":"query","style":"deepObject","schema":{"type":"string"}},
            {"name":"legacy","in":"query","schema":{"type":"string","nullable":True}}]
        result, _ = self.describe(source)
        self.assertTrue(result["supported"], result)
        self.assertEqual([p["reason"] for p in result["contract"]["excluded_parameters"]],
                         ["http_parameter_serialization_unsupported","http_schema_dialect_unsupported"])
        source["security"] = [{"unsupported":[]}]
        result, _ = self.describe(source)
        self.assertEqual(result, {"supported":False,"error":"http_security_unsupported"})


class ArrayQueryContractTests(unittest.TestCase):
    describe = staticmethod(OptionalParameterContractTests.describe)

    def test_form_explode_and_swagger_multi_keep_complete_item_and_array_constraints(self):
        for source, path in [(asset_spec(), "/assets"), (calendar_spec(), "/events")]:
            for items in [{"type":"string","enum":["open","closed"],"pattern":"^[a-z]+$"},
                          {"type":"integer","minimum":1,"maximum":5},
                          {"type":"number","minimum":0.1}, {"type":"boolean"}]:
                with self.subTest(path=path, items=items):
                    schema = {"type":"array","items":items,"minItems":1,"maxItems":3,"uniqueItems":True}
                    parameter = {"name":"values","in":"query","required":True,"schema":schema}
                    if source.get("swagger"):
                        parameter = {"name":"values","in":"query","required":True,
                                     "collectionFormat":"multi",**schema}
                    source["paths"][path]["get"]["parameters"] = [parameter]
                    result, _ = self.describe(source, path)
                    self.assertTrue(result["supported"], result)
                    self.assertEqual(result["contract"]["parameters"][0]["schema"],schema)
                    self.assertNotIn("excluded_parameters", result["contract"])

    def test_other_array_serializations_and_object_items_follow_optional_required_policy(self):
        for fields, reason in [({"explode":False}, "http_parameter_serialization_unsupported"),
                               *[({"style":style}, "http_parameter_serialization_unsupported")
                                 for style in ["spaceDelimited","pipeDelimited","deepObject"]],
                               ({"schema":{"type":"array","items":{"type":"object"}}}, "http_parameter_type_unsupported")]:
            for required in [False, True]:
                source = asset_spec()
                parameter = {"name":"values","in":"query","required":required,
                             "schema":{"type":"array","items":{"type":"string"}},**fields}
                source["paths"]["/assets"]["get"]["parameters"] = [parameter]
                result, _ = self.describe(source)
                if required:
                    self.assertEqual(result,{"supported":False,"error":reason})
                else:
                    self.assertTrue(result["supported"],result)
                    self.assertEqual(result["contract"]["excluded_parameters"][0]["reason"],reason)
        for encoding in [None,"csv","ssv","tsv","pipes"]:
            source = calendar_spec()
            parameter = {"name":"values","in":"query","type":"array","items":{"type":"integer"}}
            if encoding is not None: parameter["collectionFormat"] = encoding
            source["paths"]["/events"]["get"]["parameters"] = [parameter]
            result, _ = self.describe(source,"/events")
            self.assertTrue(result["supported"],result)
            self.assertEqual(result["contract"]["excluded_parameters"][0]["reason"],"http_parameter_serialization_unsupported")


class PostReadContractTests(unittest.TestCase):
    def test_only_selected_post_has_original_json_body_contract(self):
        for source,path in ((asset_spec(),"/assets"),(held_out_spec("shipments"),"/shipments/{tracking}")):
            operation=source["paths"][path].pop("get")
            source["paths"][path]["post"]=operation
            operation["requestBody"]={"required":True,"content":{"application/json":{"schema":{"type":"object","required":["limit"],"properties":{"limit":{"type":"integer","minimum":1}},"additionalProperties":False}}}}
            artifact={"metadata":{"xgen_http_sources":{"fixture":source}}}
            tool={"metadata":{"source_url":"fixture","path":path,"method":"post"}}
            self.assertFalse(WORKER.http_read_contract(artifact,tool)["supported"])
            policy={"allow_get":False,"read_operations":[operation["operationId"]]}
            result=WORKER.http_read_contract(artifact,tool,policy)
            self.assertTrue(result["supported"],result)
            self.assertEqual(result["contract"]["request_body"]["schema"],operation["requestBody"]["content"]["application/json"]["schema"])
            operation["requestBody"]["content"]={"application/xml":{"schema":{}}}
            self.assertFalse(WORKER.http_read_contract(artifact,tool,policy)["supported"])


@unittest.skipUnless(importlib.util.find_spec("graph_tool_call"),"run with pinned graph-tool-call environment")
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
            elif kind=="header": op["parameters"]=[{"name":"x","in":"header","required":True,"schema":{"type":"string"}}]
            elif kind=="array": op["parameters"]=[{"name":"x","in":"query","required":True,"explode":False,"schema":{"type":"array","items":{"type":"string"}}}]
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
        self.bodies=[]
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
            def do_POST(self):
                test.bodies.append(json.loads(self.rfile.read(int(self.headers.get("Content-Length",0))) or b"null"))
                self.do_GET()
            def do_GET(self):
                test.calls.append((self.path,self.headers.get("Authorization")))
                test.accepted.set()
                if test.release: test.release.wait(timeout=30)
                value=test.response(self.path) if callable(test.response) else test.response
                body=value if isinstance(value,bytes) else json.dumps(value).encode()
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

    def cli(self,*args,check=True,binary=None):
        result=subprocess.run([binary or os.environ["XGEN_TOOL_TEST_BINARY"],*args],env=self.env,capture_output=True,timeout=150)
        if check: self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertNotIn(self.secret.encode(),result.stdout+result.stderr)
        return result

    def install(self,name="shipments",spec=None):
        source=self.directory/(name+".json");source.write_text(json.dumps(held_out_spec(name) if spec is None else spec))
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
        return {"formatVersion":1,"kind":"plan","summary":"","steps":[{"key":key,"objective":"Read connected API","dependsOn":[],"capability":{"capabilityId":capability,"contractVersion":"2.0.0" if capability in ("xgen.tools/describe", "xgen.http/read") else "1.0.0"},"arguments":args}]}

    @staticmethod
    def context(payload):
        return json.loads(next(m["content"] for m in payload["messages"] if m["role"]=="user"))["planningContext"]

    def run_cli(self,allow=True,binary=None,extra=()):
        args=["run","--workspace",str(self.workspace),"--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--model","fixture-model","--tokenizer","fixture-tokenizer","--allow-dir",".","--allow-remote-model-egress"]
        if allow: args += ["--allow-read"]
        return self.cli(*args,*extra,"Read connected fixture API.",check=False,binary=binary)

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

    def test_unsupported_optional_array_is_hidden_and_injected_input_is_rejected_before_http(self):
        for name, path, tool in [("shipments","/shipments/{tracking}","readShipment"),
                                 ("sensors","/sensors/{sensorId}","readSensor")]:
            with self.subTest(name=name):
                source = held_out_spec(name)
                parameter = {"name":"states","in":"query","required":False,"explode":False,
                             "schema":{"type":"array","items":{"type":"string"}}}
                if name == "sensors":
                    parameter = {"name":"states","in":"query","required":False,"collectionFormat":"csv",
                                 "type":"array","items":{"type":"integer"}}
                source["paths"][path]["get"]["parameters"].append(parameter)
                arguments = self.install(name, source)
                view = json.loads(self.cli("tools","describe","--name",name,tool).stdout)
                self.assertNotIn("states", [row["name"] for row in view["http_read"]["contract"]["parameters"]])
                self.assertEqual(view["http_read"]["contract"]["excluded_parameters"][0]["name"], "states")
                self.assertEqual(view["http_read"]["contract"]["excluded_parameters"][0]["reason"], "http_parameter_serialization_unsupported")
                self.response = {"tracking":"A42","delivered":True} if name == "shipments" else {"values":[2.5]}
                self.calls.clear(); self.requests.clear()
                self.proposals = [self.plan("describe", "xgen.tools/describe", {"collection":name,"tool":tool}),
                                  self.plan("read", "xgen.http/read", arguments)]
                result = self.run_cli()
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertEqual(len(self.calls), 1)
                self.assertNotIn("states", self.calls[0][0])
                model_view = self.context(self.requests[1])["toolOutputs"][0]["output"]
                self.assertNotIn("states", [row["name"]["text"] for row in model_view["tool"]["parameters"]])
                self.assertEqual(model_view["http_read"]["excluded_parameters"][0]["name"]["text"], "states")
                self.calls.clear(); self.requests.clear()
                bad = copy.deepcopy(arguments)
                bad["parameters"]["query"]["states"] = ["open"]
                self.proposals = [self.plan("bad", "xgen.http/read", bad)]
                result = self.run_cli()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.calls, [])

    def test_repeated_query_pairs_and_invalid_array_inputs_on_independent_sources(self):
        for name, path in [("shipments","/shipments/{tracking}"),("sensors","/sensors/{sensorId}")]:
            with self.subTest(name=name):
                self.state=self.directory/(name+"-arrays-state");self.env["XGEN_STATE_HOME"]=str(self.state)
                source=held_out_spec(name)
                schema={"type":"array","items":{"type":"string","enum":["open","closed"]},
                        "minItems":1,"maxItems":2,"uniqueItems":True}
                parameter={"name":"states","in":"query","schema":schema}
                values=["open","closed"]
                invalid=[[],["open","open"],["unknown"],["open","closed","open"]]
                if name=="sensors":
                    schema={"type":"array","items":{"type":"integer","minimum":1,"maximum":5},
                            "minItems":1,"maxItems":2,"uniqueItems":True}
                    parameter={"name":"states","in":"query","collectionFormat":"multi",**schema}
                    values=[1,5];invalid=[[],[1,1],[0],[6],[1,2,3]]
                source["paths"][path]["get"]["parameters"].append(parameter)
                arguments=self.install(name,source)
                arguments["parameters"]["query"]["states"]=values
                self.response={"tracking":"A42","delivered":True} if name=="shipments" else {"values":[2.5]}
                self.calls.clear();self.requests.clear()
                self.proposals=[self.plan("read","xgen.http/read",arguments)]
                result=self.run_cli();self.assertEqual(result.returncode,0,result.stderr.decode())
                self.assertEqual(len(self.calls),1)
                pairs=urllib.parse.parse_qsl(urllib.parse.urlsplit(self.calls[0][0]).query)
                self.assertEqual([value for key,value in pairs if key=="states"],[str(value) for value in values])
                for bad_value in invalid:
                    self.calls.clear();self.requests.clear()
                    bad=copy.deepcopy(arguments);bad["parameters"]["query"]["states"]=bad_value
                    self.proposals=[self.plan("bad","xgen.http/read",bad)]
                    result=self.run_cli();self.assertNotEqual(result.returncode,0)
                    self.assertEqual(self.calls,[])

    @unittest.skipUnless(os.environ.get("XGEN_HTTP_V1_TEST_BINARY"),"previous HTTP profile binary opt-in")
    def test_previous_http_profile_does_not_resume_under_new_wire_rules(self):
        arguments=self.install();self.proposals=[self.plan("read","xgen.http/read",arguments)]
        self.proposals[0]["steps"][0]["capability"]["contractVersion"]="1.0.0"
        paused=self.run_cli(allow=False,binary=os.environ["XGEN_HTTP_V1_TEST_BINARY"])
        self.assertEqual(paused.returncode,10,paused.stderr.decode())
        run_id=self.manifest()["record"]["runId"]
        result=self.cli("resume",run_id,"--workspace",str(self.workspace),"--allow-dir",".",
                        "--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--allow-read",check=False)
        self.assertEqual(result.returncode,64,result.stderr.decode())
        self.assertIn(b"configuration=execution_profile",result.stderr)
        self.assertEqual(self.calls,[])
        self.assertEqual(len(self.requests),1)

    @unittest.skipUnless(os.environ.get("XGEN_HTTP_V1_TEST_BINARY"),"previous HTTP profile binary opt-in")
    def test_completed_previous_http_profile_replays_without_network(self):
        arguments=self.install();self.proposals=[self.plan("read","xgen.http/read",arguments)]
        self.proposals[0]["steps"][0]["capability"]["contractVersion"]="1.0.0"
        completed=self.run_cli(binary=os.environ["XGEN_HTTP_V1_TEST_BINARY"])
        self.assertEqual(completed.returncode,0,completed.stderr.decode())
        calls=len(self.calls);requests=len(self.requests)
        result=self.cli("resume",self.manifest()["record"]["runId"],check=False)
        self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertEqual(len(self.calls),calls)
        self.assertEqual(len(self.requests),requests)

    def install_post(self, name="shipments", approve=True):
        source=held_out_spec(name)
        if name=="shipments":
            item=source["paths"]["/shipments/{tracking}"]
        else:
            source={"openapi":"3.1.0","info":{"title":"Measurements","version":"1"},"paths":{
                "/sensors/{sensorId}":{"get":{"operationId":"readSensor","parameters":[{"name":"sensorId","in":"path","required":True,"schema":{"type":"integer","minimum":1}}],
                "responses":{"200":{"description":"Values","content":{"application/json":{"schema":{"type":"object","required":["values"],"properties":{"values":{"type":"array","items":{"type":"number"}}},"additionalProperties":False}}}}}}}}}
            item=source["paths"]["/sensors/{sensorId}"]
        operation=item.pop("get")
        operation["requestBody"]={"required":True,"content":{"application/json":{"schema":{
            "type":"object","required":["limit"],"properties":{"limit":{"type":"integer","minimum":1,"maximum":5}},"additionalProperties":False}}}}
        item["post"]=operation
        target=operation["operationId"]
        path=self.directory/(name+".json");path.write_text(json.dumps(source))
        self.cli("tools","import","--name",name,"--source",str(path))
        args=["tools","connect","--name",name,"--base-url",f"http://127.0.0.1:{self.api.server_port}","--allow-get"]
        if name=="shipments": args += ["--bearer-env","XGEN_FIXTURE_BEARER"]
        if approve: args += ["--read-operation",target]
        self.cli(*args)
        result=json.loads(self.cli("tools","describe","--name",name,target).stdout)
        return {"collection":name,"tool":target,"contractDigest":result["http_read"].get("contract_digest","0"*64),
            "parameters":{"path":{"tracking":"A42"} if name=="shipments" else {"sensorId":7},"query":{}},"body":{"limit":2}},result

    def test_host_selected_post_body_and_unapproved_operation(self):
        for name in ("shipments","sensors"):
            with self.subTest(name=name):
                args,result=self.install_post(name)
                self.assertTrue(result["http_read"]["supported"],result)
                self.assertEqual(result["http_read"]["contract"]["method"],"POST")
                view=json.loads(self.cli("tools","describe","--name",name,args["tool"],"--model-view").stdout)
                self.assertTrue(view["http_read"]["request_body"]["required"])
                self.response={"tracking":"A42","delivered":True} if name=="shipments" else {"values":[1.5]}
                self.requests.clear();self.calls.clear();self.bodies.clear()
                self.proposals=[self.plan("post","xgen.http/read",args)]
                completed=self.run_cli()
                self.assertEqual(completed.returncode,0,completed.stderr.decode())
                self.assertEqual(self.bodies,[{"limit":2}])
                for body in ({"limit":0},{"limit":2,"extra":True},None):
                    bad=copy.deepcopy(args);bad["body"]=body
                    self.requests.clear();self.calls.clear();self.bodies.clear()
                    self.proposals=[self.plan("bad","xgen.http/read",bad)]
                    self.run_cli()
                    self.assertEqual(self.calls,[])
        args,result=self.install_post("unapproved",approve=False)
        self.assertFalse(result["http_read"]["supported"])
        self.requests.clear();self.calls.clear();self.proposals=[self.plan("bad","xgen.http/read",args)]
        self.run_cli();self.assertEqual(self.calls,[])

    def test_connection_rejects_unknown_and_mutating_operation_selections(self):
        source=asset_spec()
        source["paths"]["/assets"]["delete"]={"operationId":"removeAssets","responses":{"204":{"description":"Deleted"}}}
        path=self.directory/"selection.json";path.write_text(json.dumps(source))
        self.cli("tools","import","--name","selection","--source",str(path))
        for selected in ("missingOperation","removeAssets"):
            result=self.cli("tools","connect","--name","selection","--base-url",f"http://127.0.0.1:{self.api.server_port}","--read-operation",selected,check=False)
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((self.state/"http-connections"/"selection.json").exists())
        self.cli("tools","connect","--name","selection","--base-url",f"http://127.0.0.1:{self.api.server_port}","--read-operation","listAssets")
        saved=json.loads((self.state/"http-connections"/"selection.json").read_text())
        self.assertFalse(saved["allow_get"])
        self.assertEqual(saved["read_operations"],["listAssets"])

    def test_selected_post_operation_drift_rejects_resume(self):
        args,_=self.install_post()
        self.proposals=[self.plan("post","xgen.http/read",args)]
        paused=self.run_cli(allow=False)
        self.assertEqual(paused.returncode,10,paused.stderr.decode())
        path=self.state/"http-connections"/"shipments.json"
        connection=json.loads(path.read_text());connection["read_operations"]=[]
        path.write_text(json.dumps(connection));path.chmod(0o600)
        result=self.cli("resume",self.manifest()["record"]["runId"],"--workspace",str(self.workspace),"--allow-dir",".",
            "--base-url",f"http://127.0.0.1:{self.model.server_port}/v1","--allow-read",check=False)
        self.assertEqual(result.returncode,64,result.stderr.decode())
        self.assertIn(b"configuration=http_connections",result.stderr)
        self.assertEqual(self.calls,[]);self.assertEqual(len(self.requests),1)

    def assert_tokens_absent(self, tokens):
        for path in self.state.rglob("*"):
            if path.is_file():
                data=path.read_bytes()
                for token in tokens:
                    self.assertFalse(token.encode() in data,"credential persisted in state")
        serialized=json.dumps(self.requests).encode()
        for token in tokens:
            self.assertFalse(token.encode() in serialized,"credential reached model")

    def test_run_bearer_override_changes_neither_connection_nor_persisted_material(self):
        tokens=[]
        for name in ("shipments","sensors"):
            args=self.install(name)
            path=self.state/"http-connections"/(name+".json");original=path.read_bytes()
            self.response={"tracking":"A42","delivered":True} if name=="shipments" else {"values":[2.5]}
            for variable in ("FIXTURE_REQUEST_A","FIXTURE_REQUEST_B"):
                token=secrets.token_hex(24);tokens.append(token);self.env[variable]=token
                self.requests.clear();self.calls.clear();self.proposals=[self.plan("read","xgen.http/read",args)]
                result=self.run_cli(extra=("--bearer-env-for",name+"="+variable))
                self.assertEqual(result.returncode,0,result.stderr.decode())
                self.assertEqual(len(self.calls),1)
                self.assertTrue(self.calls[0][1]=="Bearer "+token,"override authorization mismatch")
                self.assertEqual(path.read_bytes(),original)
                self.assert_tokens_absent(tokens)
        self.requests.clear();self.calls.clear();self.proposals=[self.plan("read","xgen.http/read",args)]
        result=self.run_cli()
        self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertIsNone(self.calls[0][1])

    def test_multiple_connections_use_separate_run_bearers(self):
        shipment=self.install();sensor=self.install("sensors")
        tokens=[secrets.token_hex(24),secrets.token_hex(24)]
        self.env["FIXTURE_SHIPMENT_TOKEN"],self.env["FIXTURE_SENSOR_TOKEN"]=tokens
        self.response=lambda path: {"tracking":"A42","delivered":True} if path.startswith("/shipments") else {"values":[3]}
        self.proposals=[self.plan("shipment","xgen.http/read",shipment),self.plan("sensor","xgen.http/read",sensor)]
        result=self.run_cli(extra=("--bearer-env-for","shipments=FIXTURE_SHIPMENT_TOKEN","--bearer-env-for","sensors=FIXTURE_SENSOR_TOKEN"))
        self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertEqual(len(self.calls),2)
        for call,token in zip(self.calls,tokens):
            self.assertTrue(call[1]=="Bearer "+token,"connection token routing mismatch")
        self.assert_tokens_absent(tokens)

    def test_bearer_resume_pins_reference_but_allows_token_rotation_and_offline_replay(self):
        args=self.install();variable="FIXTURE_REQUEST_TOKEN"
        first=secrets.token_hex(24);second=secrets.token_hex(24)
        self.env[variable]=first;self.proposals=[self.plan("read","xgen.http/read",args)]
        paused=self.run_cli(allow=False,extra=("--bearer-env-for","shipments="+variable))
        self.assertEqual(paused.returncode,10,paused.stderr.decode())
        manifest=self.manifest();run_id=manifest["record"]["runId"]
        self.assertEqual(manifest["record"]["httpBearerOverrides"],{"shipments":variable})
        resume=["resume",run_id,"--workspace",str(self.workspace),"--allow-dir",".","--allow-read"]
        for extra in ([],["--bearer-env-for","shipments=FIXTURE_OTHER_TOKEN"]):
            result=self.cli(*resume,*extra,check=False)
            self.assertEqual(result.returncode,64,result.stderr.decode())
            self.assertIn(b"configuration=http_bearer_overrides",result.stderr)
        self.assertEqual(self.calls,[]);self.assertEqual(len(self.requests),1)
        self.env.pop(variable)
        result=self.cli(*resume,"--bearer-env-for","shipments="+variable,check=False)
        self.assertEqual(result.returncode,64,result.stderr.decode());self.assertEqual(self.calls,[])
        self.env[variable]=second
        result=self.cli(*resume,"--bearer-env-for","shipments="+variable,check=False)
        self.assertEqual(result.returncode,10,result.stderr.decode())
        self.assertEqual(len(self.calls),1)
        self.assertTrue(self.calls[0][1]=="Bearer "+second,"rotated authorization mismatch")
        result=self.cli(*resume,"--bearer-env-for","shipments="+variable,"--allow-remote-model-egress", "--base-url",f"http://127.0.0.1:{self.model.server_port}/v1",check=False)
        self.assertEqual(result.returncode,0,result.stderr.decode())
        self.env.pop(variable);calls=len(self.calls);requests=len(self.requests)
        replay=self.cli("resume",run_id,check=False)
        self.assertEqual(replay.returncode,0,replay.stderr.decode())
        self.assertEqual((len(self.calls),len(self.requests)),(calls,requests))
        self.assert_tokens_absent([first,second])

    def test_invalid_bearer_override_never_falls_back_to_stored_token(self):
        self.install()
        for reference,value in (("shipments=FIXTURE_MISSING",None),("shipments=FIXTURE_EMPTY",""),
                                ("shipments=FIXTURE_INVALID","bad token"),("unknown=FIXTURE_UNKNOWN","synthetic"),
                                ("shipments=LANG","synthetic")):
            variable=reference.split("=",1)[1]
            if value is not None: self.env[variable]=value
            result=self.run_cli(extra=("--bearer-env-for",reference))
            self.assertEqual(result.returncode,64,result.stderr.decode())
            self.assertEqual(self.calls,[]);self.assertEqual(self.requests,[])
        result=self.run_cli(extra=("--bearer-env-for","shipments=FIXTURE_EMPTY","--bearer-env-for","shipments=FIXTURE_INVALID"))
        self.assertEqual(result.returncode,64,result.stderr.decode())
        self.assertFalse((self.state/"runs").exists())

    def test_override_credential_echo_is_not_persisted(self):
        args=self.install();token=secrets.token_hex(24);self.env["FIXTURE_REQUEST_TOKEN"]=token
        self.response={"tracking":token,"delivered":True}
        def report(payload):
            self.assertEqual(self.context(payload)["toolOutputs"],[])
            return {"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":"No verified API result."}
        self.proposals=[self.plan("read","xgen.http/read",args),report]
        result=self.run_cli(extra=("--bearer-env-for","shipments=FIXTURE_REQUEST_TOKEN"))
        self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertEqual(len(self.calls),1)
        self.assert_tokens_absent([token])

    def test_http_failures_do_not_publish_unverified_body_or_follow_redirect(self):
        arguments=self.install()
        cases=[(302,b"redirect"),(500,b"internal"),(200,b"not-json"),(200,{"tracking":42,"delivered":True}),(200,b"x"*32769),(200,{"tracking":self.secret,"delivered":True}), (200,json.dumps({"tracking":self.secret,"delivered":True}).replace(self.secret, "".join("\\u%04x" % ord(c) for c in self.secret)).encode())]
        for case_index, (status,body) in enumerate(cases):
            self.calls.clear();self.requests.clear();self.status=status;self.response=body
            self.redirect=f"http://127.0.0.1:{self.api.server_port}/must-not-follow"
            def report_failure(payload):
                context = self.context(payload)
                self.assertEqual(context["toolOutputs"], [])
                self.assertEqual(context["steps"][0]["status"], "failed", f"HTTP failure case {case_index}")
                return {"formatVersion":1,"kind":"completion_candidate","steps":[],
                        "summary":"Fixture API failed; no verified data available."}
            self.proposals=[self.plan("read","xgen.http/read",arguments), report_failure]
            result=self.run_cli()
            # ADR-0052: failed read Receipts are observations; the model may report failure.
            self.assertEqual(result.returncode,0,result.stderr.decode())
            self.assertIn(b"no verified data available",result.stdout)
            self.assertEqual(len(self.calls),1)
            self.assertEqual(len(self.requests),2)
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

    def test_rebuild_disconnect_and_remove_keep_connections_tied_to_their_collection(self):
        self.install()
        collection=self.state/"tool-collections/shipments.json.gz"
        connection=self.state/"http-connections/shipments.json"
        saved=json.loads(gzip.decompress(collection.read_bytes()));saved["backend_version"]="0.0.1"
        collection.write_bytes(gzip.compress(WORKER.canonical(saved)))
        listed=json.loads(self.cli("tools","list").stdout)["collections"]
        self.assertEqual(listed,[{"collection":"shipments","status":"rebuild_required","backend_version":"0.0.1"}])
        blocked=self.run_cli()
        self.assertNotEqual(blocked.returncode,0)
        self.assertIn(b"XGEN_TOOLS error=collection_rebuild_required collection=shipments",blocked.stderr)
        self.assertEqual(self.calls,[])
        rebuilt=json.loads(self.cli("tools","rebuild","--all").stdout)
        self.assertTrue(rebuilt["ok"]);self.assertTrue(rebuilt["collections"][0]["result"]["rebuilt"])
        self.assertEqual(json.loads(self.cli("tools","rebuild","--name","shipments").stdout)["rebuilt"],False)
        # The rebuilt collection keeps its connection and executable contract.
        self.assertTrue(connection.exists())
        self.assertTrue(json.loads(self.cli("tools","describe","--name","shipments","readShipment").stdout)["http_read"]["supported"])
        again=self.cli("tools","connect","--name","shipments","--base-url",f"http://127.0.0.1:{self.api.server_port}/v2","--allow-get",check=False)
        self.assertIn(b"http_connection_already_exists",again.stderr)
        removed=json.loads(self.cli("tools","disconnect","--name","shipments").stdout)
        self.assertEqual(removed,{"ok":True,"collection":"shipments","connection_removed":True,"stored_credential":"none"})
        self.assertTrue(collection.exists())
        self.assertIn(b"http_connection_not_found",self.cli("tools","disconnect","--name","shipments",check=False).stderr)
        self.cli("tools","connect","--name","shipments","--base-url",f"http://127.0.0.1:{self.api.server_port}","--allow-get","--bearer-env","XGEN_FIXTURE_BEARER")
        removed=json.loads(self.cli("tools","remove","--name","shipments").stdout)
        self.assertEqual(removed,{"ok":True,"collection":"shipments","collection_removed":True,"connection_removed":True,"stored_credential":"none"})
        self.assertFalse(collection.exists());self.assertFalse(connection.exists())
        missing=self.cli("tools","remove","--name","shipments",check=False)
        self.assertNotEqual(missing.returncode,0)
        self.assertEqual(json.loads(missing.stdout),{"ok":False,"error":"collection_not_found","connection_removed":False})
        # A different API imported under the freed name does not inherit the old connection.
        source=self.directory/"other.json";source.write_text(json.dumps(held_out_spec("sensors")))
        self.cli("tools","import","--name","shipments","--source",str(source))
        self.assertFalse(connection.exists())
        # Deleting only the collection file by hand leaves a connection that import refuses to reuse.
        self.cli("tools","connect","--name","shipments","--base-url",f"http://127.0.0.1:{self.api.server_port}","--allow-get")
        collection.unlink()
        refused=self.cli("tools","import","--name","shipments","--source",str(source),check=False)
        self.assertNotEqual(refused.returncode,0)
        self.assertIn(b"http_connection_without_collection",refused.stderr)
        self.assertFalse(collection.exists())
        self.cli("tools","disconnect","--name","shipments")
        self.cli("tools","import","--name","shipments","--source",str(source))
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
