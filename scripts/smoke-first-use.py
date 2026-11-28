#!/usr/bin/env python3
"""Check fresh model setup, editing, test execution and approval recovery with a local fixture.

This validates CLI plumbing, not real model quality. All profiles and Run state are isolated.
"""

import argparse
from collections import deque
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading

TIMEOUT = 60


def completion():
    return {"formatVersion": 1, "kind": "completion_candidate", "steps": [], "summary": "Fixture verified."}


def plan(key, capability, arguments, version="1.0.0"):
    return {"formatVersion": 1, "kind": "plan", "summary": "", "steps": [{
        "key": key, "objective": key, "dependsOn": [],
        "capability": {"capabilityId": capability, "contractVersion": version}, "arguments": arguments,
    }]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/xgen"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    responses, requests = deque(), []

    class Model(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send(self, value):
            body = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            assert self.path == "/v1/models"
            self.send({"data": [{"id": name} for name in ["alpha-fixture", "beta-fixture"]]})

        def do_POST(self):
            assert self.path == "/v1/chat/completions"
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(body)
            self.send({"model": body["model"], "choices": [{"index": 0,
                "message": {"role": "assistant", "content": json.dumps(responses.popleft())},
                "finish_reason": "stop"}]})

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="xgen-first-use-") as temporary:
            root = Path(temporary)
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith(("XGEN_", "XGENY_"))}
            environment.update(XGEN_CONFIG_HOME=str(root / "config"), XGEN_STATE_HOME=str(root / "state"),
                               NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
            endpoint = f"http://127.0.0.1:{server.server_port}/v1"

            def run(*arguments, expected=0, cwd=root):
                result = subprocess.run([str(binary), *arguments], cwd=cwd, env=environment,
                                        capture_output=True, text=True, timeout=TIMEOUT)
                assert result.returncode == expected, (arguments[0], result.returncode, result.stderr)
                return result

            responses.append(completion())
            if os.name == "posix":
                import pty
                master, slave = pty.openpty()
                try:
                    # Both provider and model menus recover without restarting setup or refetching.
                    child = subprocess.Popen([str(binary), "model", "setup"], cwd=root, env=environment,
                                             stdin=slave, stdout=slave, stderr=slave)
                    import re
                    import select
                    import time
                    output = bytearray()
                    deadline = time.monotonic() + TIMEOUT
                    # Pick the custom endpoint by its label so new menu entries do not shift it.
                    while b"Provider number" not in output:
                        assert child.poll() is None, output.decode(errors="replace")
                        assert time.monotonic() < deadline, "provider menu timed out"
                        if select.select([master], [], [], 0.1)[0]:
                            output.extend(os.read(master, 65536))
                    custom = re.search(rb"(\d+)\D{0,4}Other OpenAI-compatible endpoint", output)
                    assert custom, output.decode(errors="replace")
                    number = custom.group(1).decode()
                    os.write(master, f"0\n{number}\n{endpoint}\ninvalid\n2\n".encode())
                    while child.poll() is None:
                        assert time.monotonic() < deadline, "interactive setup timed out"
                        if select.select([master], [], [], 0.1)[0]:
                            output.extend(os.read(master, 65536))
                    while select.select([master], [], [], 0.1)[0]:
                        output.extend(os.read(master, 65536))
                    assert child.returncode == 0, output.decode(errors="replace")
                    assert output.count(b"Enter a number") == 2
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=TIMEOUT)
                    os.close(master)
                    os.close(slave)
            else:
                run("model", "setup", "--base-url", endpoint, "--model", "beta-fixture")
            assert len(requests) == 1
            profile = root / "config/model-profiles.json"
            original = profile.read_bytes()
            assert not (root / "state").exists()
            failed = run("model", "setup", "--model", "missing-fixture", expected=65)
            assert "reason=model_not_advertised" in failed.stderr and "recovery:" in failed.stderr
            assert profile.read_bytes() == original
            responses.append(completion())
            run("model", "check", "--compatibility")
            python = str(Path(sys.executable).resolve())

            # Two design layouts and an independent text layout use identical capability contracts.
            for index, (filename, before, after, check) in enumerate([
                ("answer.py", "def answer():\n    return 41\n", "def answer():\n    return 42\n",
                 "from answer import answer; assert answer() == 42"),
                ("settings.json", '{"enabled": false}\n', '{"enabled": true}\n',
                 'import json; assert json.loads(Path("settings.json").read_text())["enabled"] is True'),
                ("note.txt", "draft\n", "reviewed\n", 'assert Path("note.txt").read_text() == "reviewed\\n"'),
            ]):
                workspace = root / f"project-{index}"
                workspace.mkdir()
                source = workspace / filename
                source.write_text(before)
                (workspace / "verify.py").write_text(
                    'from pathlib import Path\n' + check + '\n'
                    'p = Path("verified.txt"); p.write_text(p.read_text() + "ok\\n" if p.exists() else "ok\\n")\n')
                responses.extend([
                    plan("inspect", "xgeny.fs/read-text", {"path": filename}),
                    plan("update", "xgeny.fs/apply-patch", {"path": filename,
                         "expectedDigest": "sha256:" + hashlib.sha256(before.encode()).hexdigest(),
                         "edits": [{"oldText": before, "newText": after}]}, "2.0.0"),
                    plan("test", "xgeny.process/execute", {"executable": "python", "args": ["verify.py"],
                         "cwd": ".", "env": {}, "timeoutMs": 30000, "maxOutputBytes": 4096}),
                    completion(),
                ])
                common = ["--workspace", str(workspace), "--allow-dir", ".",
                          "--allow-executable", f"python={python}", "--allow-remote-model-egress", "--allow-read"]
                first = run("run", *common, "Update the file and run verify.py.", expected=10)
                assert "reason=write_approval_required" in first.stderr
                run_id = re.search(r"run_id=(\S+)", first.stderr)[1]
                assert source.read_text() == before
                second = run("resume", run_id, *common, "--allow-write", expected=10)
                assert "reason=execute_approval_required" in second.stderr
                assert source.read_text() == after
                assert not (workspace / "verified.txt").exists()
                run("resume", run_id, *common, "--allow-write", "--allow-execute")
                assert (workspace / "verified.txt").read_text() == "ok\n"
                calls = len(requests)
                run("resume", run_id)
                assert len(requests) == calls
                assert (workspace / "verified.txt").read_text() == "ok\n"
                run("usage", run_id)
                assert not responses
            assert profile.read_bytes() == original
            print(f"first-use fixture: PASS (fresh setup, failed setup preserved, 3 edit/test/resume layouts; {len(requests)} model fixture calls, no live LLM)")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=TIMEOUT)


if __name__ == "__main__":
    main()
