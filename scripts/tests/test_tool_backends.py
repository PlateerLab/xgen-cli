import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest


def load(name):
    path = Path(__file__).resolve().parents[1] / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


search = load("experiment-tool-backends")
smoke = load("smoke-search-process")


class SearchObservationTests(unittest.TestCase):
    def test_both_schemas_preserve_source_order_and_unicode(self):
        expected = [{"title": "공식 문서", "url": "https://docs.example.org/ko", "snippet": "한글 설명"}]
        self.assertEqual(search.normalize_results("ddgs", [{"title": "공식 문서", "href": "https://docs.example.org/ko", "body": "한글 설명"}]), expected)
        self.assertEqual(search.normalize_results("openserp", {"results": expected}), expected)

    def test_empty_response_is_not_schema_failure(self):
        self.assertEqual(search.normalize_results("ddgs", []), [])
        self.assertEqual(search.normalize_results("openserp", {"results": []}), [])

    def test_malformed_schemas_are_rejected(self):
        for backend, payload in (("ddgs", {}), ("openserp", []), ("openserp", {"results": None}), ("ddgs", [None]), ("ddgs", [{"href": "https://example.org", "title": 123}])):
            with self.subTest(backend=backend, payload=payload), self.assertRaises(ValueError):
                search.normalize_results(backend, payload)

    def test_non_web_and_credential_urls_are_rejected(self):
        for url in ("javascript:alert(1)", "file:///etc/passwd", "https://user:pass@example.org", "https:///missing-host"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                search.normalize_results("ddgs", [{"title": "source", "href": url}])

    def test_domain_match_uses_hostname_boundaries(self):
        rows = [{"url": "https://example.org.evil.invalid"}, {"url": "https://evil.invalid/?next=example.org"}, {"url": "https://docs.example.org/page"}]
        self.assertEqual(search.official_rank(rows, ["example.org"]), 3)
        self.assertIsNone(search.official_rank(rows[:2], ["example.org"]))

    def test_result_limit_is_applied(self):
        rows = [{"title": str(i), "href": f"https://example.org/{i}"} for i in range(8)]
        self.assertEqual(len(search.normalize_results("ddgs", rows)), 5)

    def test_response_claims_require_exact_shape(self):
        rows = [{"title": "one", "url": "https://example.org/1"}, {"title": "two", "url": "https://example.org/2"}]
        self.assertEqual(smoke.claimed_sources(json.dumps({"sources": rows})), rows)
        for claim in ({"sources": rows[:1]}, {"sources": rows, "extra": True}, {"sources": [dict(row, snippet="extra") for row in rows]}, {"sources": [None, None]}):
            with self.subTest(claim=claim), self.assertRaises(ValueError):
                smoke.claimed_sources(json.dumps(claim))

    def test_prose_and_fenced_json_are_not_silently_repaired(self):
        for claim in ("done", '```json\n{"sources": []}\n```'):
            with self.subTest(claim=claim), self.assertRaises(ValueError):
                smoke.claimed_sources(claim)

    def test_approval_pause_uses_documented_reason_and_exit_status(self):
        self.assertTrue(smoke.execute_approval_pause(10, "XGEN_PAUSED run_id=run-example reason=execute_approval_required"))
        self.assertFalse(smoke.execute_approval_pause(10, "XGEN_PAUSED reason=read_approval_required"))
        self.assertFalse(smoke.execute_approval_pause(20, "XGEN_REJECTED reason=execute_approval_required"))

    @unittest.skipUnless(os.name == "posix" and hasattr(os, "waitid"), "requires POSIX waitid")
    def test_group_leader_remains_unreaped_until_cleanup(self):
        process = subprocess.Popen([sys.executable, "-c", "raise SystemExit(7)"], start_new_session=True)
        try:
            search.wait_unreaped(process, 5)
            status = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            self.assertIsNotNone(status)
            self.assertEqual(status.si_status, 7)
        finally:
            search.stop_group(process)
            self.assertEqual(process.wait(timeout=5), 7)

    @unittest.skipUnless(os.name == "posix" and hasattr(os, "waitid"), "requires POSIX waitid")
    def test_wait_deadline_leaves_owned_group_available_for_cleanup(self):
        process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"], start_new_session=True)
        try:
            with self.assertRaises(subprocess.TimeoutExpired):
                search.wait_unreaped(process, 0.02)
        finally:
            search.stop_group(process)
            self.assertEqual(process.wait(timeout=5), -9)


if __name__ == "__main__":
    unittest.main()
