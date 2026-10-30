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
import unicodedata

TIMEOUT = 30


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/xgen"))
    parser.add_argument("--results", action="store_true", help="Also check folding and the full-result viewer")
    parser.add_argument("--rapid-resize", action="store_true", help="Stress consecutive resizes without settling; known inline-pane edge case")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="xgen-inline-tui-"))
    config, state = root / "config", root / "state"
    config.mkdir(mode=0o700)
    state.mkdir(mode=0o700)
    (root / "note.txt").write_text("READ_FIXTURE\n", encoding="utf-8")
    socket = root / "tmux.sock"
    entered, release = threading.Event(), threading.Event()
    background_entered, background_release = threading.Event(), threading.Event()
    completion_entered, completion_release = threading.Event(), threading.Event()
    requests = []
    change_text = "".join(f"DIFF_ROW_{n:02}\n" for n in range(60))
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
            background = "Inspect viewer background" in context["goal"]
            if (context["goal"].startswith("Inspect fixture") or background) and not context["toolOutputs"]:
                ready, proceed = (background_entered, background_release) if background else (entered, release)
                ready.set()
                if not proceed.wait(TIMEOUT):
                    mark("fixture_wait_expired")
                    self.send_error(500)
                    return
                mark("fixture_released")
                writing = args.results and not background
                proposal = {
                    "formatVersion": 1, "kind": "plan", "summary": "", "steps": [{
                        "key": "read", "objective": "Read the fixture", "dependsOn": [],
                        "capability": {"capabilityId": "xgeny.fs/write-atomic" if writing else "xgeny.fs/read-text", "contractVersion": "1.0.0"},
                        "arguments": {"path": "created.txt", "content": change_text, "expectedDigest": None} if writing else {"path": "note.txt"},
                    }],
                }
            else:
                read = bool(context["toolOutputs"])
                answer = "READ_DONE\n" + "".join(f"ANSWER_ROW_{n:02}\n" for n in range(40)) + "ANSWER_LAST" if args.results and read else "READ_DONE" if read else "QUEUED_DONE"
                if background:
                    completion_entered.set()
                    if not completion_release.wait(TIMEOUT):
                        self.send_error(500)
                        return
                    answer = "BACKGROUND_DONE"
                proposal = {
                    "formatVersion": 1, "kind": "completion_candidate" if read else "response_candidate",
                    "steps": [], "summary": answer,
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
        text = ""
        stable_since = None

        def ready():
            nonlocal stable_since
            nonlocal text
            snapshot = tmux("capture-pane", "-p", "-S", "-", "-t", "fixture", ";",
                            "display-message", "-p", "-t", "fixture", "__CURSOR__#{cursor_x},#{cursor_y}")
            text, cursor = snapshot.rsplit("__CURSOR__", 1)
            x, y = map(int, cursor.strip().split(","))
            rows = text.splitlines()[-height:]
            if y >= len(rows) or not rows[y].startswith("│ ") or not rows[y].endswith(" │"):
                return False
            # Wait for the completed frame's caret, not just its already-written borders.
            content = rows[y][4:-2].rstrip(" ")
            caret = 4 + sum(0 if unicodedata.combining(c) else 2 if unicodedata.east_asian_width(c) in ("W", "F") else 1 for c in content)
            box_width = width
            complete = (x == caret and text.count("╭") == 1 and text.count("╰") == 1
                    and "╭" + "─" * (box_width - 2) + "╮" in text
                    and "╰" + "─" * (box_width - 2) + "╯" in text)
            if not complete:
                stable_since = None
                return False
            if stable_since is None:
                stable_since = time.monotonic()
            # A completed caret does not mean tmux has settled its resize reflow.
            return time.monotonic() - stable_since >= (0 if args.rapid_resize else 0.25)

        wait_for(ready, f"resized pane: {label}")
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
        permission = "Allow write" if args.results else "Allow read"
        wait_for(lambda: permission in capture(), "tool approval")
        send("y")
        resize(80, 24, "approval-draft")
        assert "│ > y" in capture(), "approval draft lost on resize"
        enter()
        wait_for(lambda: "QUEUED_DONE" in capture(history=True), "queued goal completion")
        text = capture(history=True)
        assert "READ_DONE" in text, "read did not complete"
        assert len(requests) == 3, f"unexpected model request count: {len(requests)}"
        if args.results:
            assert (root / "created.txt").read_text() == change_text
        else:
            assert requests[1]["toolOutputs"][0]["output"]["content"] == "READ_FIXTURE\n"
        assert "queued after resize" in requests[2]["goal"], "queued goal changed"
        resize(55, 18, "heldout-after-completion")
        if args.results:
            assert "Ctrl+O:" in capture(history=True), "long results were not folded"
            assert "ANSWER_ROW_20" not in capture(history=True), "answer middle leaked into preview"
            assert "DIFF_ROW_30" not in capture(history=True), "change middle leaked into preview"
            send("unsent draft 한국어")
            tmux("send-keys", "-t", "fixture", "C-o")
            wait_for(lambda: "Answer #3" in capture(), "latest result viewer")
            tmux("send-keys", "-t", "fixture", "Left", "End")
            wait_for(lambda: "DIFF_ROW_59" in capture(), "full change tail")
            tmux("send-keys", "-t", "fixture", "Home")
            tmux("send-keys", "-t", "fixture", "-N", "20", "Down")
            wait_for(lambda: "DIFF_ROW_30" in capture(), "full change middle beyond old cap")
            tmux("send-keys", "-t", "fixture", "Left", "End")
            wait_for(lambda: "ANSWER_LAST" in capture(), "full answer tail")
            tmux("resize-window", "-t", "fixture", "-x", "70", "-y", "20")
            tmux("send-keys", "-t", "fixture", "Home")
            wait_for(lambda: "Answer #1" in capture() and "READ_DONE" in capture(), "viewer resize and Home")
            tmux("send-keys", "-t", "fixture", "End")
            wait_for(lambda: "ANSWER_LAST" in capture(), "viewer End after resize")
            tmux("send-keys", "-t", "fixture", "Enter", "Escape")
            wait_for(lambda: "│ > unsent draft 한국어" in capture(), "draft restored after viewer")
            assert "ANSWER_ROW_20" not in capture(history=True), "viewer replaced terminal scrollback"
            assert len(requests) == 3, "viewer input started another model call"
            tmux("send-keys", "-t", "fixture", "C-c")
            send("Inspect viewer background")
            enter()
            assert background_entered.wait(TIMEOUT), "background model call did not begin"
            send("preserved draft")
            tmux("send-keys", "-t", "fixture", "C-o")
            wait_for(lambda: "Answer #3" in capture(), "viewer during a Run")
            background_release.set()
            wait_for(lambda: "Approval pending" in capture(), "approval notice inside viewer")
            tmux("send-keys", "-t", "fixture", "Enter", "Left")
            wait_for(lambda: "Changes #2" in capture(), "viewer navigation after ignored Enter")
            assert len(requests) == 4, "viewer Enter approved a tool"
            tmux("send-keys", "-t", "fixture", "Escape")
            wait_for(lambda: "│ > preserved draft" in capture(), "pending draft on approval return")
            tmux("send-keys", "-t", "fixture", "C-c")
            send("y")
            enter()
            assert completion_entered.wait(TIMEOUT), "approved read did not finish"
            tmux("send-keys", "-t", "fixture", "C-o")
            wait_for(lambda: "Answer #3" in capture(), "viewer before background result")
            completion_release.set()
            wait_for(lambda: "New output" in capture(), "deferred result notice")
            tmux("send-keys", "-t", "fixture", "Escape")
            wait_for(lambda: "BACKGROUND_DONE" in capture(history=True), "deferred result restored")
            assert len(requests) == 5, "unexpected background model request count"
        tmux("send-keys", "-t", "fixture", "Up")
        recalled = "│ > Inspect viewer background" if args.results else "│ > queued after resize"
        wait_for(lambda: recalled in capture(), "history after resize")
        tmux("send-keys", "-t", "fixture", "C-c", "C-d")
        wait_for(lambda: (root / "stty.txt").exists(), "terminal restoration")
        modes = (root / "stty.txt").read_text().split()
        assert "icanon" in modes and "-icanon" not in modes, "raw mode not restored"
        print(json.dumps({"status": "passed", "results": args.results, "modelCalls": len(requests), "artifacts": str(root)}))
    finally:
        release.set()
        background_release.set()
        completion_release.set()
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
