#!/usr/bin/env python3
"""Exercise the real inline TUI in isolated tmux with a loopback model; no API key needed."""

import argparse
import http.server
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import threading
import time

TIMEOUT = 30


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/xgen"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="xgen-inline-tui-"))
    config, state = root / "config", root / "state"
    config.mkdir(mode=0o700)
    state.mkdir(mode=0o700)
    (root / "note.txt").write_text("READ_FIXTURE\n", encoding="utf-8")
    socket = root / "tmux.sock"
    entered, release = threading.Event(), threading.Event()
    requests = []
    events = []
    started_at = time.monotonic()

    def mark(label):
        events.append({"seconds": round(time.monotonic() - started_at, 3), "event": label})

    class Model(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            context = json.loads(request["messages"][1]["content"])["planningContext"]
            requests.append(context)
            mark("model_request")
            if context["goal"].startswith("Inspect fixture") and not context["toolOutputs"]:
                entered.set()
                if not release.wait(TIMEOUT):
                    mark("fixture_wait_expired")
                    self.send_error(500)
                    return
                mark("fixture_released")
                proposal = {
                    "formatVersion": 1, "kind": "plan", "summary": "", "steps": [{
                        "key": "read", "objective": "Read the fixture", "dependsOn": [],
                        "capability": {"capabilityId": "xgeny.fs/read-text", "contractVersion": "1.0.0"},
                        "arguments": {"path": "note.txt"},
                    }],
                }
            else:
                read = bool(context["toolOutputs"])
                proposal = {
                    "formatVersion": 1, "kind": "completion_candidate" if read else "response_candidate",
                    "steps": [], "summary": "READ_DONE" if read else "QUEUED_DONE",
                }
            body = json.dumps({
                "id": "tui-fixture", "model": "fixture", "choices": [{
                    "index": 0, "message": {"role": "assistant", "content": json.dumps(proposal)},
                    "finish_reason": "stop",
                }],
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
                mark("model_response")
            except BrokenPipeError:
                mark("fixture_client_disconnected")

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    profile = config / "model-profiles.json"
    profile.write_text(json.dumps({
        "formatVersion": 1, "activeProfile": "fixture", "profiles": [{
            "name": "fixture", "baseUrl": f"http://127.0.0.1:{server.server_port}/v1",
            "model": "fixture", "tokenizer": "fixture", "credentialRef": None,
        }],
    }))
    profile.chmod(0o600)

    def tmux(*parts):
        return subprocess.check_output(["tmux", "-S", str(socket), *parts], text=True, timeout=5)

    def capture(history=False):
        return tmux("capture-pane", "-p", "-S", "-" if history else "0", "-t", "fixture")

    def wait_for(predicate, label):
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.05)
        (root / "failure.txt").write_text(capture(history=True))
        raise AssertionError(f"timed out: {label}; captures in {root}")

    def send(text):
        tmux("send-keys", "-t", "fixture", "-l", text)

    def enter():
        tmux("send-keys", "-t", "fixture", "Enter")

    def resize(width, height, label):
        mark(label)
        tmux("resize-window", "-t", "fixture", "-x", str(width), "-y", str(height))
        time.sleep(0.25)
        text = capture(history=True)
        (root / f"{label}.txt").write_text(text)
        assert text.count("╭") == 1 and text.count("╰") == 1, f"stale pane: {label}; {root}"
        assert text.count("XGEN Developer Preview") == 1, f"lost or duplicated output: {label}"

    started = False
    try:
        # The fixture never inherits real provider configuration or credentials.
        unsets = [f"-u {shlex.quote(key)}" for key in os.environ if key.startswith("XGEN_")]
        launch = f"cd {shlex.quote(str(root))} && env " + " ".join(unsets + [
            shlex.quote(f"XGEN_CONFIG_HOME={config}"), shlex.quote(f"XGEN_STATE_HOME={state}"),
            shlex.quote(str(binary)),
        ]) + f"; stty -a > {shlex.quote(str(root / 'stty.txt'))}"
        tmux("new-session", "-d", "-s", "fixture", "-x", "100", "-y", "30", launch)
        started = True
        tmux("set-option", "-t", "fixture", "status", "off")
        tmux("set-option", "-t", "fixture", "remain-on-exit", "on")
        wait_for(lambda: "Enter send" in capture(), "initial pane")
        send("한글 draft that wraps across the terminal and remains editable")
        for width, height in [(40, 12), (100, 30), (100, 12), (80, 24)]:
            resize(width, height, f"draft-{width}x{height}")
        tmux("send-keys", "-t", "fixture", "C-c")
        send("Inspect fixture")
        enter()
        wait_for(lambda: "Allow sending" in capture(), "model approval")
        send("a")
        enter()
        assert entered.wait(TIMEOUT), "model call did not begin"
        mark("model_entered")
        send("queued after resize")
        enter()
        wait_for(lambda: "queued:" in capture(), "queued goal")
        for width, height in [(40, 18), (100, 30), (60, 18)]:
            resize(width, height, f"running-{width}x{height}")
        release.set()
        mark("release_model")
        wait_for(lambda: "Allow read" in capture(), "read approval")
        send("y")
        resize(80, 24, "approval-draft")
        assert "│ > y" in capture(), "approval draft lost on resize"
        enter()
        wait_for(lambda: "QUEUED_DONE" in capture(history=True), "queued goal completion")
        text = capture(history=True)
        assert "READ_DONE" in text, "read did not complete"
        assert len(requests) == 3, f"unexpected model request count: {len(requests)}"
        assert requests[1]["toolOutputs"][0]["output"]["content"] == "READ_FIXTURE\n"
        assert "queued after resize" in requests[2]["goal"], "queued goal changed"
        resize(55, 18, "heldout-after-completion")
        tmux("send-keys", "-t", "fixture", "Up")
        wait_for(lambda: "│ > queued after resize" in capture(), "history after resize")
        tmux("send-keys", "-t", "fixture", "C-c", "C-d")
        wait_for(lambda: (root / "stty.txt").exists(), "terminal restoration")
        modes = (root / "stty.txt").read_text().split()
        assert "icanon" in modes and "-icanon" not in modes, "raw mode not restored"
        print(json.dumps({"status": "passed", "modelCalls": len(requests), "artifacts": str(root)}))
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        (root / "events.json").write_text(json.dumps(events, indent=2))
        if started:
            # This socket and its only session were created by this invocation.
            sessions = tmux("list-sessions", "-F", "#{session_name}").splitlines()
            if sessions == ["fixture"]:
                tmux("kill-server")


if __name__ == "__main__":
    main()
