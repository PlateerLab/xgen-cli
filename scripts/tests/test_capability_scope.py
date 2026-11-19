"""Capability selection on held-out HTTP fixtures; only loopback endpoints are used."""
import json
import os
import unittest

import test_http_read_tools as http


@unittest.skipUnless(os.environ.get("XGEN_TOOL_TEST_BINARY"), "compiled CLI opt-in")
class CapabilityScopeHttpTests(unittest.TestCase):
    setUp = http.HttpCliTests.setUp
    server = http.HttpCliTests.server
    cli = http.HttpCliTests.cli
    install = http.HttpCliTests.install
    plan = staticmethod(http.HttpCliTests.plan)
    context = staticmethod(http.HttpCliTests.context)
    run_cli = http.HttpCliTests.run_cli

    def test_only_offered_discovery_and_http_hints_reach_the_model(self):
        for name in ("shipments", "sensors"):
            arguments = self.install(name)
            self.response = {"tracking":"A42", "delivered":True} if name == "shipments" else {"values":[1.5]}
            for capability in ("xgen.http/read", "xgen.tools/describe"):
                self.requests.clear(); self.calls.clear()
                args = arguments if capability == "xgen.http/read" else {"collection":name, "tool":arguments["tool"]}
                self.proposals = [self.plan("inspect", capability, args)]
                result = self.run_cli(extra=("--capability", capability))
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                for request in self.requests:
                    context = self.context(request)
                    self.assertEqual([item["capability"]["capabilityId"] for item in context["capabilities"]], [capability])
                    constraints = {item["constraintId"]:item["description"] for item in context["planningConstraints"]}
                    self.assertNotIn("workspace.fs-scope", constraints)
                    self.assertNotIn("process.executable-catalog", constraints)
                    if capability == "xgen.http/read":
                        self.assertNotIn("tools.discovery-snapshots", constraints)
                        self.assertNotIn("First describe", constraints["http.read-connections"])
                    else:
                        self.assertNotIn("http.read-connections", constraints)
                        self.assertNotIn("Search with", constraints["tools.discovery-snapshots"])
                self.assertEqual(len(self.calls), int(capability == "xgen.http/read"))

    def test_smaller_requests_preserve_actual_api_input_and_response(self):
        for name in ("shipments", "sensors"):
            arguments = self.install(name)
            self.response = {"tracking":"A42", "delivered":True} if name == "shipments" else {"values":[2.5]}
            sizes, calls, bodies = [], [], []
            for extra in ((), ("--capability", "xgen.http/read")):
                self.requests.clear(); self.calls.clear()
                self.proposals = [self.plan("inspect", "xgen.http/read", arguments)]
                result = self.run_cli(extra=extra)
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                sizes.append(len(json.dumps(self.requests[0], ensure_ascii=False).encode()))
                calls.append(self.calls.copy())
                outputs = self.context(self.requests[-1])["toolOutputs"]
                bodies.append(outputs[0]["output"]["body"])
                self.assertEqual(outputs[0]["output"]["request"], arguments)
            self.assertLess(sizes[1], sizes[0])
            self.assertTrue(calls[0] == calls[1], "API requests changed with capability scope")
            self.assertEqual(bodies, [self.response, self.response])

    @unittest.skipUnless(os.environ.get("XGEN_SCOPE_PREVIOUS_BINARY"), "previous unscoped binary opt-in")
    def test_previous_unscoped_runs_resume_with_the_same_catalog_and_api_contract(self):
        for name in ("shipments", "sensors"):
            arguments = self.install(name)
            self.response = {"tracking":"A42", "delivered":True} if name == "shipments" else {"values":[3.5]}
            self.requests.clear(); self.calls.clear()
            self.proposals = [self.plan("inspect", "xgen.http/read", arguments)]
            paused = self.run_cli(allow=False, binary=os.environ["XGEN_SCOPE_PREVIOUS_BINARY"])
            self.assertEqual(paused.returncode, 10, paused.stderr.decode())
            run_id = next(line.split("run_id=", 1)[1].split()[0] for line in paused.stderr.decode().splitlines() if line.startswith("XGEN_STARTED "))
            first = self.context(self.requests[0])
            resumed = self.cli("resume", run_id, "--workspace", str(self.workspace), "--allow-dir", ".", "--allow-read", "--allow-remote-model-egress", "--base-url", f"http://127.0.0.1:{self.model.server_port}/v1", check=False)
            self.assertEqual(resumed.returncode, 0, resumed.stderr.decode())
            final = self.context(self.requests[-1])
            self.assertEqual(first["catalogDigest"], final["catalogDigest"])
            self.assertEqual(final["toolOutputs"][0]["output"]["body"], self.response)
            self.assertEqual(len(self.calls), 1)
