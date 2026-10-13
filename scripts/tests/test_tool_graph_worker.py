"""Actual graph-tool-call integration and optional compiled CLI checks."""

import http.server
import threading
import importlib.util
import gzip
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("tool_graph_worker", ROOT / "crates/xgen-cli/src/tool_graph_worker.py")
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)


def asset_spec():
    return {
        "openapi": "3.1.0", "info": {"title": "Asset registry", "version": "1"},
        "paths": {
            "/assets": {"get": {"operationId": "listAssets", "summary": "List assets in inventory",
                "responses": {"200": {"description": "Assets", "content": {"application/json": {
                    "schema": {"type": "object", "properties": {"asset_id": {"type": "string"}}}}}}}}},
            "/assets/{asset_id}": {"get": {"operationId": "getAssetDetail", "summary": "Read asset detail",
                "parameters": [{"name": "asset_id", "in": "path", "required": True,
                                "schema": {"type": "string", "minLength": 1}}],
                "responses": {"200": {"description": "Asset detail", "content": {"application/json": {
                    "schema": {"type": "object", "properties": {"label": {"type": "string"}}}}}}}}},
        },
    }


def calendar_spec():
    return {
        "swagger": "2.0", "info": {"title": "Calendar", "version": "1"}, "basePath": "/v1",
        "paths": {
            "/events": {"get": {"operationId": "listCalendarEvents", "summary": "List calendar events",
                "responses": {"200": {"description": "Events", "schema": {"type": "array", "items": {
                    "type": "object", "properties": {"eventId": {"type": "integer"}}}}}}}},
            "/events/{eventId}": {"get": {"operationId": "readEventDetails", "summary": "Read event details",
                "parameters": [{"name": "eventId", "in": "path", "required": True, "type": "integer"}],
                "responses": {"200": {"description": "Event", "schema": {"type": "object",
                    "properties": {"start": {"type": "string", "format": "date-time"}}}}}}},
        },
    }


@unittest.skipUnless(importlib.util.find_spec("graph_tool_call"), "run with pinned graph-tool-call environment")
class WorkerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.root = self.directory / "catalogs"

    def call(self, operation, **fields):
        return WORKER.dispatch({"root": str(self.root), "operation": operation, **fields})

    def install(self, name="assets", spec=None):
        path = self.directory / (name + "-source.json")
        path.write_text(json.dumps(asset_spec() if spec is None else spec))
        return self.call("import", name=name, sources=[str(path)])

    def test_two_formats_search_and_exact_schema(self):
        cases = [
            ("assets", asset_spec(), "read asset detail", "getAssetDetail", "asset_id"),
            ("calendar", calendar_spec(), "read event details", "readEventDetails", "eventId"),
        ]
        for name, spec, query, expected, parameter in cases:
            with self.subTest(system=name):
                built = self.install(name, spec)
                self.assertEqual(built["tool_count"], 2)
                self.assertFalse(built["execution_enabled"])
                result = self.call("search", name=name, query=query, top_k=1)
                self.assertEqual(result["candidates"][0]["tool"], expected)
                schema = self.call("describe", name=name, tool=expected)
                self.assertEqual(schema["effect_class"], "unclassified")
                self.assertTrue(any(p["name"] == parameter and p["required"] for p in schema["tool"]["parameters"]))
                self.assertIn("api_contract", schema["tool"]["metadata"])
                self.assertEqual(built["artifact_digest"], schema["artifact_digest"])
        self.assertEqual(len(self.call("list")["collections"]), 2)

    def test_language_hints_compare_request_and_documentation_scripts(self):
        korean = asset_spec()
        korean["paths"]["/assets"]["get"]["summary"] = "자산 목록 조회 (API v2)"
        korean["paths"]["/labels"] = {"get": {"operationId": "listLabels", "summary": "라벨 목록 조회",
                                              "responses": {"200": {"description": "OK"}}}}
        snapshots = {name: self.install(name, spec)["artifact_digest"]
                     for name, spec in (("assets", asset_spec()), ("korean", korean))}
        english = self.call("language_hints", request="show every asset label", snapshots=snapshots)
        self.assertEqual(english["collections"], {
            "assets": {"documentation_script": "Latin", "script_differs": False},
            "korean": {"documentation_script": "Hangul", "script_differs": True}})
        hangul = self.call("language_hints", request="자산 라벨 보여줘", snapshots=snapshots)
        self.assertEqual([row["script_differs"] for row in hangul["collections"].values()], [True, False])
        neutral = self.call("language_hints", request="12345 ?", snapshots=snapshots)
        self.assertFalse(any(row["script_differs"] for row in neutral["collections"].values()))
        with self.assertRaisesRegex(WORKER.WorkerError, "snapshot_changed"):
            self.call("language_hints", request="x", snapshots={"assets": "0" * 64})
        self.assertNotIn("documentation_script", self.call("search", name="korean", query="자산", top_k=1))
        self.assertEqual(WORKER.summary_script("プロジェクト一覧を取得"), "Kana")
        self.assertEqual(WORKER.summary_script("获取项目列表"), "Han")
        self.assertIsNone(WORKER.summary_script("123 / {}"))

    def test_required_producers_and_source_identity(self):
        self.install()
        result = self.call("search", name="assets", query="read asset detail", top_k=1)
        self.assertIn("listAssets", result["possible_producers"])
        self.assertFalse(result["relations_verified_by_execution"])
        saved = WORKER.load(self.root, "assets")
        row = saved["artifact"]["metadata"]["xgen_source_inputs"][0]
        self.assertEqual(row["document_digest"], WORKER.digest(asset_spec()))

    def test_immutable_and_tampered_collections(self):
        self.install()
        path = self.root / "assets.json.gz"
        original = path.read_bytes()
        with self.assertRaisesRegex(WORKER.WorkerError, "already_exists"):
            self.install()
        self.assertEqual(original, path.read_bytes())
        saved = json.loads(gzip.decompress(original))
        saved["artifact"]["tools"]["listAssets"]["description"] = "altered"
        path.write_bytes(gzip.compress(json.dumps(saved).encode()))
        with self.assertRaisesRegex(WORKER.WorkerError, "integrity_failure"):
            self.call("search", name="assets", query="inventory", top_k=2)

    def rewrite(self, name, change):
        path = self.root / (name + ".json.gz")
        saved = json.loads(gzip.decompress(path.read_bytes()))
        change(saved)
        path.write_bytes(gzip.compress(WORKER.canonical(saved)))
        return path

    def test_collection_built_by_another_backend_version_is_reported_as_such(self):
        self.install()
        self.rewrite("assets", lambda saved: saved.update(backend_version="0.0.1"))
        with self.assertRaisesRegex(WORKER.WorkerError, "collection_backend_version_mismatch"):
            self.call("search", name="assets", query="inventory", top_k=2)
        self.assertEqual(self.call("list")["collections"],
                         [{"collection": "assets", "status": "rebuild_required", "backend_version": "0.0.1"}])

    def test_rebuild_restores_old_backend_collections_from_retained_sources(self):
        built = {}
        for name, spec in [("assets", asset_spec()), ("calendar", calendar_spec())]:
            self.install(name, spec)
            built[name] = WORKER.load(self.root, name)["artifact"]
        for name in built:
            self.rewrite(name, lambda saved: saved.update(backend_version="0.0.1"))
        listed = self.call("list")["collections"]
        self.assertEqual([row["status"] for row in listed], ["rebuild_required", "rebuild_required"])
        for name, original in built.items():
            with self.subTest(collection=name):
                result = self.call("rebuild", name=name)
                self.assertTrue(result["rebuilt"])
                self.assertEqual(result["previous"]["backend_version"], "0.0.1")
                self.assertEqual(result["previous"]["tool_count"], 2)
                self.assertEqual(result["refetched_https_sources"], 0)
                self.assertEqual(result["backend_version"], WORKER.VERSION)
                self.assertEqual(result["tool_count"], 2)
                # Same retained inputs and labels give the same tools and source records as the
                # original import; only the build times recorded by the backend differ.
                rebuilt = WORKER.load(self.root, name)["artifact"]
                self.assertEqual(rebuilt["tools"], original["tools"])
                for key in ("xgen_source_inputs", "xgen_http_sources"):
                    self.assertEqual(rebuilt["metadata"][key], original["metadata"][key])
        self.assertEqual([row["status"] for row in self.call("list")["collections"]], ["ready", "ready"])
        self.assertEqual(self.call("search", name="calendar", query="read event details", top_k=1)["candidates"][0]["tool"], "readEventDetails")

    def test_rebuild_of_current_collection_keeps_the_file(self):
        built = self.install()
        path = self.root / "assets.json.gz"
        original = path.read_bytes()
        result = self.call("rebuild", name="assets")
        self.assertFalse(result["rebuilt"])
        self.assertEqual(result["artifact_digest"], built["artifact_digest"])
        self.assertEqual(original, path.read_bytes())

    def test_rebuild_replaces_only_the_snapshot_it_read(self):
        built = self.install()
        path = self.root / "assets.json.gz"
        original = path.read_bytes()
        artifact = WORKER.read_stored(self.root, "assets")["artifact"]
        with self.assertRaisesRegex(WORKER.WorkerError, "collection_changed_during_rebuild"):
            WORKER.store(self.root, "assets", artifact, replaces="0" * 64)
        self.assertEqual(original, path.read_bytes())
        path.unlink()
        with self.assertRaisesRegex(WORKER.WorkerError, "collection_changed_during_rebuild"):
            WORKER.store(self.root, "assets", artifact, replaces=built["artifact_digest"])
        self.assertFalse(path.exists())
        self.assertEqual([p.name for p in self.root.iterdir()], [])

    def test_rebuild_needs_verified_retained_sources(self):
        def drop_copy(saved):
            saved["artifact"]["metadata"]["xgen_http_sources"] = {}
            saved.update(backend_version="0.0.1", artifact_digest=WORKER.digest(saved["artifact"]))
        def alter_copy(saved):
            saved["artifact"]["metadata"]["xgen_http_sources"]["inline:1"]["info"]["title"] = "altered"
            saved.update(backend_version="0.0.1", artifact_digest=WORKER.digest(saved["artifact"]))
        for change in (drop_copy, alter_copy):
            with self.subTest(change=change.__name__):
                self.install()
                self.rewrite("assets", change)
                with self.assertRaisesRegex(WORKER.WorkerError, "collection_sources_unavailable"):
                    self.call("rebuild", name="assets")
                self.call("remove", name="assets")
        rows = [{"kind": "https", "label": "https://example.test/openapi.json", "document_digest": None}]
        self.assertEqual(WORKER.retained_inputs({"metadata": {"xgen_source_inputs": rows, "xgen_http_sources": {}}}),
                         (["https://example.test/openapi.json"], ["https://example.test/openapi.json"]))
        with self.assertRaisesRegex(WORKER.WorkerError, "collection_not_found"):
            self.call("rebuild", name="missing")

    def test_remove_frees_the_name_even_for_unusable_collections(self):
        self.install()
        self.install("calendar", calendar_spec())
        # Damaged deflate data raises zlib.error, which is not an OSError.
        for damaged in [b"not a collection", b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\xff\x07" + b"\x00" * 8,
                        gzip.compress(WORKER.canonical({"format_version": 1}))]:
            (self.root / "assets.json.gz").write_bytes(damaged)
            listed = self.call("list")["collections"]
            self.assertEqual(listed[0], {"collection": "assets", "status": "unreadable", "error": "collection_integrity_failure"})
            self.assertEqual(listed[1]["status"], "ready")
        self.assertEqual(self.call("remove", name="assets"), {"ok": True, "collection": "assets", "removed": True})
        with self.assertRaisesRegex(WORKER.WorkerError, "collection_not_found"):
            self.call("remove", name="assets")
        self.assertEqual(self.install()["tool_count"], 2)
        if os.name == "posix":
            target = self.directory / "outside.json.gz"
            target.write_bytes(b"kept")
            (self.root / "linked.json.gz").symlink_to(target)
            with self.assertRaisesRegex(WORKER.WorkerError, "invalid_collection_file"):
                self.call("remove", name="linked")
            self.assertEqual(target.read_bytes(), b"kept")

    def test_host_pinned_snapshot_rejects_valid_replacement(self):
        for name, spec, target in [("assets", asset_spec(), "getAssetDetail"), ("calendar", calendar_spec(), "readEventDetails")]:
            built = self.install(name, spec)
            self.assertEqual(self.call("describe", name=name, tool=target, expected_digest=built["artifact_digest"])["tool"]["name"], target)
            path = self.root / (name + ".json.gz")
            saved = json.loads(gzip.decompress(path.read_bytes()))
            saved["artifact"]["tools"][target]["description"] = "valid newer snapshot"
            saved["artifact_digest"] = WORKER.digest(saved["artifact"])
            path.write_bytes(gzip.compress(WORKER.canonical(saved)))
            for operation, fields in [("search", {"query": "details", "top_k": 1}), ("describe", {"tool": target})]:
                with self.assertRaisesRegex(WORKER.WorkerError, "collection_snapshot_changed"):
                    self.call(operation, name=name, expected_digest=built["artifact_digest"], **fields)

    def test_invalid_names_and_missing_tools(self):
        for name in ["../escape", "", "x/y"]:
            with self.assertRaisesRegex(WORKER.WorkerError, "invalid_collection_name"):
                self.call("import", name=name, sources=[])
        self.install()
        with self.assertRaisesRegex(WORKER.WorkerError, "tool_not_found"):
            self.call("describe", name="assets", tool="invented")
        with self.assertRaisesRegex(WORKER.WorkerError, "collection_not_found"):
            self.call("describe", name="missing", tool="listAssets")

    def test_strict_json_and_private_directory(self):
        path = self.directory / "duplicate.json"
        path.write_text('{"paths":{},"paths":{}}')
        with self.assertRaisesRegex(WORKER.WorkerError, "duplicate_json_key"):
            self.call("import", name="bad", sources=[str(path)])
        if os.name == "posix":
            self.root.chmod(0o755)
            with self.assertRaisesRegex(WORKER.WorkerError, "not_private"):
                self.call("list")

    def test_query_limits(self):
        self.install()
        for query, top_k in [("", 1), ("x", 0), ("x", 21), ("x", True)]:
            with self.assertRaises(WORKER.WorkerError):
                self.call("search", name="assets", query=query, top_k=top_k)

    def test_compressed_document_inflation_is_bounded(self):
        path = self.directory / "oversized.json.gz"
        path.write_bytes(gzip.compress(b'"' + b'x' * 256 + b'"'))
        with self.assertRaisesRegex(WORKER.WorkerError, "document_size_limit"):
            WORKER.read_json(path, limit=128)


@unittest.skipUnless(os.environ.get("XGEN_TOOL_TEST_BINARY"), "compiled CLI opt-in")
class CliTests(unittest.TestCase):
    def test_import_search_describe_both_systems(self):
        with tempfile.TemporaryDirectory() as directory:
            env = dict(os.environ, XGEN_STATE_HOME=directory)
            binary = os.environ["XGEN_TOOL_TEST_BINARY"]
            def call(*args):
                result = subprocess.run([binary, "tools", *args], env=env, capture_output=True, timeout=150)
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                return json.loads(result.stdout)
            for name, spec, query, target in [
                ("assets", asset_spec(), "list assets in inventory", "listAssets"),
                ("calendar", calendar_spec(), "list calendar events", "listCalendarEvents"),
            ]:
                path = Path(directory) / (name + ".json")
                path.write_text(json.dumps(spec))
                call("import", "--name", name, "--source", str(path))
                result = call("search", "--name", name, query, "--top-k", "1")
                self.assertEqual(result["candidates"][0]["tool"], target)
                self.assertEqual(call("describe", "--name", name, target)["tool"]["name"], target)
            self.assertEqual(len(call("list")["collections"]), 2)

    def test_agent_loop_with_actual_worker_on_held_out_list_requests(self):
        binary = os.environ["XGEN_TOOL_TEST_BINARY"]
        for collection, spec, query, target in [
            ("assets", asset_spec(), "list assets in inventory", "listAssets"),
            ("calendar", calendar_spec(), "list calendar events", "listCalendarEvents"),
        ]:
            with self.subTest(collection=collection), tempfile.TemporaryDirectory() as directory:
                state = Path(directory) / "state"
                workspace = Path(directory) / "workspace"
                workspace.mkdir()
                source = Path(directory) / "source.json"
                source.write_text(json.dumps(spec))
                env = dict(os.environ, XGEN_STATE_HOME=str(state))
                env.pop("XGEN_OPENAI_API_KEY", None)
                imported = subprocess.run([binary, "tools", "import", "--name", collection, "--source", str(source)], env=env, capture_output=True, timeout=150)
                self.assertEqual(imported.returncode, 0, imported.stderr.decode())
                snapshot = json.loads(imported.stdout)["artifact_digest"]
                requests = []
                proposals = []
                for key, capability, arguments in [
                    ("find", "xgen.tools/search", {"collection":collection,"query":query,"topK":1}),
                    ("inspect", "xgen.tools/describe", {"collection":collection,"tool":target}),
                ]:
                    proposals.append({"formatVersion":1,"kind":"plan","summary":"","steps":[{"key":key,"objective":"Inspect discovery candidates","dependsOn":[],"capability":{"capabilityId":capability,"contractVersion":"2.0.0" if capability=="xgen.tools/describe" else "1.0.0"},"arguments":arguments}]})
                proposals.append({"formatVersion":1,"kind":"completion_candidate","steps":[],"summary":"Tool schema inspected; no API executed."})
                class Handler(http.server.BaseHTTPRequestHandler):
                    def log_message(self, *_args):
                        pass
                    def do_POST(self):
                        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                        requests.append(payload)
                        index = len(requests)-1
                        proposal = proposals[index] if index < len(proposals) else proposals[-1]
                        body = json.dumps({"id":"fixture","model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":json.dumps(proposal)},"finish_reason":"stop"}]}).encode()
                        self.send_response(200)
                        self.send_header("Content-Type", "application/json")
                        self.send_header("Content-Length", str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                server = http.server.ThreadingHTTPServer(("127.0.0.1",0), Handler)
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                try:
                    run = subprocess.run([binary,"run","--workspace",str(workspace),"--base-url",f"http://127.0.0.1:{server.server_port}/v1","--model","fixture-model","--tokenizer","fixture-tokenizer","--allow-dir",".","--allow-read","--allow-remote-model-egress",query], env=env,capture_output=True,timeout=150)
                finally:
                    server.shutdown()
                    server.server_close()
                    thread.join(timeout=5)
                self.assertEqual(run.returncode,0,run.stderr.decode())
                self.assertEqual(len(requests),3)
                def context(request):
                    return json.loads(next(message["content"] for message in request["messages"] if message["role"] == "user"))["planningContext"]
                initial = context(requests[0])
                self.assertIn("xgen.tools/search",[row["capability"]["capabilityId"] for row in initial["capabilities"]])
                final = context(requests[-1])
                rows = {row["capability"]["capabilityId"]:row["output"] for row in final["toolOutputs"]}
                self.assertEqual(rows["xgen.tools/search"]["candidates"][0]["tool"],target)
                self.assertEqual(rows["xgen.tools/describe"]["tool"]["name"],target)
                self.assertIn("api_contract",rows["xgen.tools/describe"]["tool"]["metadata"])
                self.assertEqual(rows["xgen.tools/describe"]["artifact_digest"],snapshot)
                self.assertFalse(rows["xgen.tools/describe"]["execution_enabled"])
                manifest = json.loads(next((state/"runs").glob("*/manifest.json")).read_text())
                self.assertEqual(manifest["record"]["toolDiscoverySnapshots"][collection],snapshot)


if __name__ == "__main__":
    unittest.main()
