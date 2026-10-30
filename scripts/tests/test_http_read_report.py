"""Offline HTTP support report agrees with individual describe contracts."""
import copy
import importlib.util
import json
import os
import unittest

import test_tool_graph_worker as base
from test_tool_graph_worker import WORKER, asset_spec, calendar_spec
import test_http_read_tools as http


def envelope(source):
    return {"collection":"fixture","artifact_digest":"a"*64,"artifact":{"tools":{},
        "metadata":{"source_count":1,"xgen_http_sources":{"fixture":source}}}}


def tools_for(source):
    return [{"name":operation["operationId"],"metadata":{"source_url":"fixture","path":path,"method":method}}
            for path,item in source["paths"].items() for method,operation in item.items()
            if method in ("get","post","delete")]


class ReportTests(unittest.TestCase):
    def test_two_contract_formats_match_individual_classifier_and_keep_summary_across_pages(self):
        for source,path in ((asset_spec(),"/assets"),(calendar_spec(),"/events")):
            with self.subTest(path=path):
                operation=source["paths"][path]["get"]
                operation["parameters"]=[{"name":"trace","in":"header","schema":{"type":"string"}}]
                post=copy.deepcopy(operation);post["operationId"]="inspectRecords"
                source["paths"][path]["post"]=post
                tools=tools_for(source);saved=envelope(source)
                policy={"allow_get":True,"read_operations":[]}
                first=WORKER.http_read_report(saved,tools,policy,0,1)
                self.assertEqual(first["summary"]["supported"],2)
                self.assertEqual(first["summary"]["unsupported"],1)
                self.assertEqual(first["summary"]["excluded_parameter_count"],1)
                self.assertFalse(first["execution_enabled"])
                rows=[];offset=0
                while offset is not None:
                    page=WORKER.http_read_report(saved,tools,policy,offset,1)
                    self.assertEqual(page["summary"],first["summary"])
                    rows+=page["tools"];offset=page["page"]["next_offset"]
                self.assertEqual([row["tool"]["text"] for row in rows],sorted(tool["name"] for tool in tools))
                terminal=WORKER.http_read_report(saved,tools,policy,len(tools),1)
                self.assertEqual(terminal["tools"],[])
                self.assertIsNone(terminal["page"]["next_offset"])
                self.assertEqual(terminal["summary"],first["summary"])
                for row in rows:
                    tool=next(tool for tool in tools if tool["name"]==row["tool"]["text"])
                    original=WORKER.http_read_contract(saved["artifact"],tool,policy)
                    self.assertEqual(row["supported"],original["supported"])
                    if original["supported"]: self.assertEqual(row["contract_digest"],original["contract_digest"])
                    else: self.assertEqual(row["error"],original["error"])
                policy["read_operations"]=["inspectRecords"]
                selected=WORKER.http_read_report(saved,tools,policy,0,100)
                self.assertNotEqual(selected["policy_digest"],first["policy_digest"])
                self.assertEqual(selected["summary"]["supported"],3)
                self.assertEqual(selected["summary"]["excluded_parameter_count"],2)

    def test_page_bounds_empty_collection_and_large_exclusions(self):
        saved=envelope(asset_spec())
        for offset,limit in ((-1,1),(True,1),(0,0),(0,501),(0,True),(1,1)):
            with self.assertRaises(WORKER.WorkerError): WORKER.http_read_report(saved,[],None,offset,limit)
        empty=WORKER.http_read_report(saved,[],None,0,100)
        self.assertEqual(empty["tools"],[]);self.assertIsNone(empty["page"]["next_offset"])
        self.assertFalse(empty["connection_present"])
        source=asset_spec()
        source["paths"]["/assets"]["get"]["parameters"]=[{"name":("filter"+str(i))*100,"in":"header","schema":{"type":"string"}} for i in range(40)]
        result=WORKER.http_read_report(envelope(source),tools_for(source),None,0,100)
        row=next(row for row in result["tools"] if row["tool"]["text"]=="listAssets")
        self.assertEqual(row["excluded_parameter_count"],40)
        self.assertEqual(len(row["excluded_parameters"]),32)
        self.assertFalse(row["excluded_parameters_complete"])
        self.assertTrue(row["excluded_parameters"][0]["name"]["omitted"])
        self.assertLess(len(WORKER.canonical(result)),1024*1024)
        aliases=[{"name":f"alias{i:04}","metadata":{"source_url":"fixture","path":"/assets","method":"get"}} for i in range(200)]
        page=WORKER.http_read_report(envelope(source),aliases,None,0,500)
        self.assertLess(len(page["tools"]),200)
        self.assertEqual(page["page"]["next_offset"],len(page["tools"]))
        self.assertEqual(page["summary"]["total"],200)
        self.assertLess(len(WORKER.canonical(page)),1024*1024+4096)


@unittest.skipUnless(importlib.util.find_spec("graph_tool_call"),"pinned backend opt-in")
class WorkerReportTests(unittest.TestCase):
    setUp=base.WorkerTests.setUp
    call=base.WorkerTests.call
    install=base.WorkerTests.install

    def test_saved_collection_report_and_snapshot_check(self):
        self.install("assets",asset_spec())
        result=self.call("check_http",name="assets",offset=0,limit=1)
        self.assertEqual(result["summary"]["total"],2)
        self.assertEqual(len(result["tools"]),1)
        self.assertEqual(result["page"]["next_offset"],1)
        with self.assertRaises(WORKER.WorkerError):
            self.call("check_http",name="assets",expected_digest="0"*64)


@unittest.skipUnless(os.environ.get("XGEN_TOOL_TEST_BINARY"),"compiled CLI opt-in")
class CliReportTests(unittest.TestCase):
    # Reuse transport fixtures without inheriting and rerunning the HTTP execution suite.
    setUp=http.HttpCliTests.setUp
    server=http.HttpCliTests.server
    cli=http.HttpCliTests.cli
    install=http.HttpCliTests.install
    install_post=http.HttpCliTests.install_post

    def test_held_out_reports_match_describe_without_credentials_or_network(self):
        for name in ("shipments","sensors"):
            arguments=self.install(name)
            self.env.pop("XGEN_FIXTURE_BEARER",None)
            report=json.loads(self.cli("tools","check-http","--name",name).stdout)
            self.assertEqual(report["scope"],"imported_tools")
            self.assertTrue(report["connection_present"])
            self.assertFalse(report["credentials_checked"]);self.assertFalse(report["network_checked"])
            self.assertEqual(report["summary"]["supported"],1)
            self.assertEqual(report["tools"][0]["contract_digest"],arguments["contractDigest"])
            self.assertEqual(self.calls,[]);self.assertEqual(self.requests,[])
            self.assertNotIn(self.secret,json.dumps(report))

    def test_held_out_optional_exclusions_are_visible(self):
        for name in ("shipments","sensors"):
            source=http.held_out_spec(name)
            operation=next(iter(source["paths"].values()))["get"]
            operation["parameters"].append({"name":"trace","in":"header","type":"string","schema":{"type":"string"}})
            self.install(name,source)
            report=json.loads(self.cli("tools","check-http","--name",name).stdout)
            self.assertEqual(report["summary"]["excluded_parameter_count"],1)
            self.assertEqual(report["tools"][0]["excluded_parameters"][0]["name"]["text"],"trace")
            self.assertEqual(report["summary"]["excluded_parameter_reasons"],{"http_parameter_location_unsupported":1})
        self.assertEqual(self.calls,[])

    def test_selected_post_policy_is_reported(self):
        for name in ("shipments","sensors"):
            arguments,description=self.install_post(name)
            report=json.loads(self.cli("tools","check-http","--name",name,"--limit","1").stdout)
            self.assertEqual(report["summary"]["supported"],1)
            self.assertEqual(report["tools"][0]["contract_digest"],description["http_read"]["contract_digest"])
            self.assertEqual(report["tools"][0]["method"]["text"],"post")
        arguments,description=self.install_post("unapproved",approve=False)
        report=json.loads(self.cli("tools","check-http","--name","unapproved").stdout)
        self.assertEqual(report["summary"]["unsupported_reasons"],{"http_method_not_read_approved":1})
        self.assertEqual(self.calls,[]);self.assertEqual(self.requests,[])
