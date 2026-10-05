"""Predeclared, independent large-contract cases; no production calls."""
import json
import os
from pathlib import Path
import unittest

import test_tool_graph_worker as base
import test_http_read_tools as http


def validation_spec(name):
    # These values are frozen before running the view/host contract checks.
    schema = {"type":"object", "required":["value"], "properties":{
        "value":{"type":"integer","minimum":1},
        **{f"field{i}":{"type":"string","description":"response detail " * 50} for i in range(180)}}}
    enum = [f"mode{i}" for i in range(500)]
    if name == "books":
        return {"openapi":"3.1.0","info":{"title":"Library catalog","version":"1"},
            "paths":{"/books/{id}":{"get":{"operationId":"readBook","summary":"Read library book " + "한글 설명 " * 12000,"description":"한글 설명 " * 12000,
                "parameters":[{"name":"id","in":"path","required":True,"schema":{"type":"integer","minimum":1}},
                    {"name":"mode","in":"query","required":True,"schema":{"type":"string","enum":enum}}],
                "responses":{"200":{"description":"Result","content":{"application/json":{"schema":schema}}}}}}}}
    return {"swagger":"2.0","info":{"title":"Device metrics","version":"1"},"basePath":"/v3","produces":["application/json"],
        "paths":{"/devices/{id}/metrics":{"get":{"operationId":"readDeviceMetrics","summary":"Read device metrics " + "Device measurement " * 12000,"description":"Device measurement " * 12000,
            "parameters":[{"name":"id","in":"path","required":True,"type":"integer","minimum":1},
                {"name":"mode","in":"query","required":True,"type":"string","enum":enum}],
            "responses":{"200":{"description":"Result","schema":schema}}}}}}


def fresh_validation_spec(name):
    # Frozen after the v2 approval correction, before evaluating these cases.
    # Different response structure and reference placement from design fixtures.
    if name == "batches":
        source=validation_spec("books")
        operation=source["paths"].pop("/books/{id}")["get"]
        operation["operationId"]="readBatch"
        source["info"]["title"]="Laboratory batches"
        operation["summary"]="Read laboratory batch " * 9000
        source["paths"]={"/batches/{id}":{"get":operation}}
        schema=operation["responses"]["200"]["content"]["application/json"]["schema"]
        schema["properties"]["value"]={"type":"array","items":{"type":"number","minimum":0},"minItems":1}
        source["components"]={"schemas":{"BatchResult":schema}}
        operation["responses"]["200"]["content"]["application/json"]["schema"]={"$ref":"#/components/schemas/BatchResult"}
        return source
    source=validation_spec("metrics")
    operation=source["paths"].pop("/devices/{id}/metrics")["get"]
    operation["operationId"]="readRecording"
    source["info"]["title"]="Recording archive"
    operation["summary"]="Read recording archive " * 9000
    source["paths"]={"/recordings/{id}":{"get":operation}}
    schema=operation["responses"]["200"]["schema"]
    schema["properties"]["value"]={"type":"string","enum":["ready","pending"]}
    source["definitions"]={"RecordingResult":schema}
    operation["responses"]["200"]["schema"]={"$ref":"#/definitions/RecordingResult"}
    return source


class ViewTests(unittest.TestCase):
    setUp = base.WorkerTests.setUp
    install = base.WorkerTests.install
    call = base.WorkerTests.call

    def test_two_fresh_large_sources_preserve_full_contract_binding(self):
        for name, tool in [("books","readBook"),("metrics","readDeviceMetrics")]:
            self.install(name,validation_spec(name))
            full=self.call("describe",name=name,tool=tool)
            view=self.call("describe_view",name=name,tool=tool)
            self.assertGreater(len(base.WORKER.canonical(full)),65536)
            self.assertLess(len(base.WORKER.canonical(view)),65536)
            self.assertEqual(view["full_tool_digest"],base.WORKER.digest(full["tool"]))
            self.assertTrue(full["http_read"]["supported"],full["http_read"])
            self.assertEqual(view["http_read"]["contract_digest"],full["http_read"]["contract_digest"])
            self.assertNotIn("contract",view["http_read"])
            self.assertFalse(view["tool"]["parameters"][1]["complete"])
            self.assertTrue(view["tool"]["description"]["omitted"])

    def test_unsupported_http_keeps_normalized_parameter_types_and_enums(self):
        for name, tool in [("books","readBook"),("metrics","readDeviceMetrics")]:
            source=validation_spec(name)
            path=next(iter(source["paths"]))
            source["paths"][path]["post"]=source["paths"][path].pop("get")
            self.install(name,source)
            view=self.call("describe_view",name=name,tool=tool)
            full=self.call("describe",name=name,tool=tool)
            self.assertFalse(view["http_read"]["supported"])
            self.assertEqual(view["tool"]["parameters"][0]["schema"]["type"],"integer")
            self.assertEqual(view["tool"]["parameters"][1]["enum_count"],500)
            self.assertTrue(all(p["schema_origin"]=="discovery_normalized" for p in view["tool"]["parameters"]))
            self.assertEqual(view["full_tool_digest"],base.WORKER.digest(full["tool"]))

    def test_control_and_unicode_text_have_a_byte_bound(self):
        for text in ["\x1b" * 10000, "한글" * 10000]:
            tool={"name":"read","description":text,"metadata":{"path":text,"method":"get"},
                  "parameters":[{"name":text,"type":"string","description":text,"required":True} for _ in range(70)]}
            envelope={"collection":"bounded","artifact_digest":"a"*64,"artifact":{"tools":[tool],"metadata":{"source_count":1}}}
            offset=0;count=0
            while True:
                view=base.WORKER.describe_view(envelope,tool,{"supported":False,"error":"http_original_source_unavailable"},offset)
                self.assertLessEqual(len(base.WORKER.canonical(view)),48*1024)
                count += len(view["tool"]["parameters"])
                offset=view["parameter_page"]["next_offset"]
                if offset is None: break
            self.assertEqual(count,70)

    def test_pages_cover_all_inputs_and_reject_invalid_offsets(self):
        source=validation_spec("books")
        operation=source["paths"]["/books/{id}"]["get"]
        operation["parameters"] += [{"name":f"p{i}","in":"query","schema":{"type":"integer"}} for i in range(68)]
        self.install("pages",source)
        rows=[];offset=0
        while True:
            view=self.call("describe_view",name="pages",tool="readBook",parameter_offset=offset)
            self.assertLess(len(base.WORKER.canonical(view)),65536)
            rows.extend(view["tool"]["parameters"])
            offset=view["parameter_page"]["next_offset"]
            if offset is None: break
        self.assertEqual([row["index"] for row in rows],list(range(70)))
        self.assertEqual(len({row["name"]["text"] for row in rows}),70)
        for offset in [-1,True,71,"0"]:
            with self.assertRaisesRegex(base.WORKER.WorkerError,"invalid_parameter_offset"):
                self.call("describe_view",name="pages",tool="readBook",parameter_offset=offset)


@unittest.skipUnless(os.environ.get("XGEN_TOOL_TEST_BINARY"),"compiled CLI opt-in")
class FullHostTests(unittest.TestCase):
    setUp = http.HttpCliTests.setUp
    server = http.HttpCliTests.server
    cli = http.HttpCliTests.cli
    plan = staticmethod(http.HttpCliTests.plan)
    context = staticmethod(http.HttpCliTests.context)
    run_cli = http.HttpCliTests.run_cli
    manifest = http.HttpCliTests.manifest
    receipts = http.HttpCliTests.receipts

    def test_large_original_schemas_stay_enforced_on_two_fresh_cases(self):
        for name,tool,source_spec,valid_body,invalid_body in [
            ("books","readBook",validation_spec("books"),{"value":3},{"value":0}),
            ("metrics","readDeviceMetrics",validation_spec("metrics"),{"value":3},{"value":0}),
            ("batches","readBatch",fresh_validation_spec("batches"),{"value":[3.5]},{"value":[]}),
            ("recordings","readRecording",fresh_validation_spec("recordings"),{"value":"ready"},{"value":"invalid"}),
        ]:
            self.state=self.directory/(name+"-state");self.env["XGEN_STATE_HOME"]=str(self.state)
            source=self.directory/(name+".json");source.write_text(json.dumps(source_spec))
            self.cli("tools","import","--name",name,"--source",str(source))
            self.cli("tools","connect","--name",name,"--base-url",f"http://127.0.0.1:{self.api.server_port}","--allow-get")
            view=json.loads(self.cli("tools","describe","--name",name,tool,"--model-view").stdout)
            arguments={"collection":name,"tool":tool,"contractDigest":view["http_read"]["contract_digest"],"parameters":{"path":{"id":7},"query":{"mode":"mode42"}}}
            self.response=valid_body;self.calls.clear();self.requests.clear()
            def read(payload):
                observation=next(r["output"] for r in self.context(payload)["toolOutputs"] if r["capability"]["capabilityId"]=="xgen.tools/describe")
                self.assertEqual({k:v for k,v in observation.items() if k!="request"},view)
                return self.plan("read","xgen.http/read",arguments)
            def complete(payload):
                result=next(r["output"] for r in self.context(payload)["toolOutputs"] if r["capability"]["capabilityId"]=="xgen.http/read")
                self.assertTrue(result["ok"],result)
                return {"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":"Verified large-schema response."}
            self.proposals=[self.plan("describe","xgen.tools/describe",{"collection":name,"tool":tool}),read,complete]
            result=self.run_cli();self.assertEqual(result.returncode,0,result.stderr.decode())
            self.assertEqual(len(self.calls),1)
            self.assertTrue(all(r["status"]=="succeeded" for r in self.receipts()))
            self.cli("resume",self.manifest()["record"]["runId"])
            self.assertEqual(len(self.calls),1)
            # The enum is absent from the model view but authoritative on the host.
            arguments["parameters"]["query"]["mode"]="forbidden"
            self.proposals=[self.plan("read","xgen.http/read",arguments)]
            self.calls.clear();self.requests.clear()
            result=self.run_cli();self.assertNotEqual(result.returncode,0)
            self.assertEqual(len(self.calls),0)
            arguments["parameters"]["query"]["mode"]="mode42"
            arguments["contractDigest"]="0"*64
            self.requests.clear();result=self.run_cli();self.assertNotEqual(result.returncode,0)
            self.assertEqual(len(self.calls),0)
            arguments["contractDigest"]=view["http_read"]["contract_digest"]
            self.requests.clear();self.response=invalid_body
            result=self.run_cli();self.assertNotEqual(result.returncode,0)
            self.assertEqual(len(self.calls),1)
            self.assertNotIn(b'"body"',result.stdout)
