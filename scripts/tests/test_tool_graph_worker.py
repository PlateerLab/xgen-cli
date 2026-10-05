"""Actual graph-tool-call integration and optional compiled CLI checks."""

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


if __name__ == "__main__":
    unittest.main()
