#!/usr/bin/env python3
"""Compare the model requests two xgen builds send for the same scripted sessions.

Each case runs against a local fake model that returns the same scripted replies to both
builds, so any difference in what reaches the model comes from the build, not the model.
Every build runs each case twice; fields that differ between those two runs (random ids,
digests over them, ports) are noise and are compared by shape only. Remaining differences
are reported with a fingerprint (field pattern, kind, length change) so repeated effects of
one change collapse into one group, and with the first differing byte of each request,
which is where a provider prompt cache stops matching.

Exit status: 0 when the requests match, 1 when they differ, 2 when a case could not run.
This checks request bytes, not model quality. All state lives under a temporary root.
"""

import argparse
from collections import OrderedDict, deque
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading

TIMEOUT = 90
DEFAULT_CASES = Path(__file__).resolve().parent / "fixtures" / "model-request-diff" / "cases.json"
HEX = re.compile(r"[0-9a-f]{16,}")
PORT = re.compile(r"127\.0\.0\.1:\d+")
EXCERPT = 40
RUN_ID = re.compile(r"XGEN_STARTED run_id=(\S+)")


class CaseError(Exception):
    pass


def reply_content(reply):
    """Expand a compact case reply into the planner proposal the fake model returns."""
    if "plan" in reply:
        key, capability, arguments = reply["plan"]
        return {"formatVersion": 1, "kind": "plan", "summary": "", "steps": [{
            "key": key, "objective": key, "dependsOn": [],
            "capability": {"capabilityId": capability, "contractVersion": reply.get("version", "1.0.0")},
            "arguments": arguments,
        }]}
    if "completion" in reply:
        return {"formatVersion": 1, "kind": "completion_candidate", "steps": [], "summary": reply["completion"]}
    if "response" in reply:
        return {"formatVersion": 1, "kind": "response_candidate", "steps": [], "summary": reply["response"]}
    if "raw" in reply:
        # Any proposal as written, including invalid ones the host must reject.
        return reply["raw"]
    raise CaseError(f"unknown reply {sorted(reply)}")


class FakeModel:
    """OpenAI-compatible chat endpoint that records raw request bodies."""

    def __init__(self, replies):
        self.replies = deque(reply_content(reply) for reply in replies)
        self.requests = []
        model = self

        class Handler(http.server.BaseHTTPRequestHandler):
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
                self.send({"data": [{"id": "request-diff-model"}]})

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                model.requests.append({"path": self.path, "body": body})
                # Past the script, finish politely so a build that asks more still ends.
                content = model.replies.popleft() if model.replies else reply_content({"completion": "Done."})
                self.send({"id": "request-diff", "model": "request-diff-model", "choices": [{
                    "index": 0, "message": {"role": "assistant", "content": json.dumps(content)},
                    "finish_reason": "stop"}]})

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    @property
    def base_url(self):
        return f"http://127.0.0.1:{self.server.server_port}/v1"

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def run_case(binary, case, root):
    """Run one case in ``root`` (recreated, same path for every build) and return its requests."""
    if root.exists():
        shutil.rmtree(root)
    workspace = root / "workspace"
    workspace.mkdir(parents=True)
    for name, text in case.get("files", {}).items():
        path = workspace / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    model = FakeModel(case.get("replies", []))
    env = {
        "PATH": os.environ.get("PATH", ""), "HOME": str(root / "home"), "LANG": "C.UTF-8", "TZ": "UTC",
        "XGEN_STATE_HOME": str(root / "state"), "XGEN_CONFIG_HOME": str(root / "config"),
        "XGEN_OPENAI_BASE_URL": model.base_url, "XGEN_OPENAI_MODEL": "request-diff-model",
        "XGEN_OPENAI_TOKENIZER": "request-diff-tokenizer",
    }
    env.update(case.get("env", {}))
    # A case is one process, or several that share state (e.g. a Run and its resume).
    # "{run_id}" in later arguments or input is the Run the previous sessions started.
    sessions = case.get("sessions") or [{"args": case.get("args", []), "stdin": case.get("stdin", "")}]
    exits, stderr, stdout, run_id = [], "", "", None
    try:
        for session in sessions:
            fill = (lambda text: text.replace("{run_id}", run_id or "")) if run_id else (lambda text: text)
            try:
                process = subprocess.run([str(binary), *map(fill, session.get("args", []))], cwd=workspace,
                                         env=env, input=fill(session.get("stdin", "")).encode(),
                                         capture_output=True, timeout=TIMEOUT)
            except subprocess.TimeoutExpired as error:
                raise CaseError(f"{case['name']}: {binary} timed out after {TIMEOUT}s") from error
            exits.append(process.returncode)
            stderr = process.stderr.decode(errors="replace")
            stdout = process.stdout.decode(errors="replace")
            started = RUN_ID.findall(stderr)
            run_id = started[-1] if started else run_id
    finally:
        model.close()
    return {"exit": exits if len(exits) > 1 else exits[0], "requests": model.requests,
            "unused": len(model.replies), "stderr": stderr[-2000:], "stdout": stdout[-2000:]}


def parse(value):
    """Ordered JSON tree; strings holding a JSON object or array are expanded."""
    if isinstance(value, (bytes, str)):
        text = value.decode() if isinstance(value, bytes) else value
        stripped = text.lstrip()
        if stripped[:1] in "{[":
            try:
                return ("json", parse_tree(json.loads(text, object_pairs_hook=lambda pairs: ("obj", pairs))))
            except ValueError:
                pass
        return text
    return value


def parse_tree(value):
    if isinstance(value, tuple) and value[0] == "obj":
        return ("obj", [(key, parse_tree(item)) for key, item in value[1]])
    if isinstance(value, list):
        return ("arr", [parse_tree(item) for item in value])
    return parse(value) if isinstance(value, str) else value


def flatten(tree, pointer="", out=None):
    """Ordered ``{pointer: leaf}``; each object's key order is a leaf at ``pointer#keys``."""
    if out is None:
        out = OrderedDict()
    if isinstance(tree, tuple) and tree[0] == "json":
        return flatten(tree[1], pointer + "~json", out)
    if isinstance(tree, tuple) and tree[0] == "obj":
        out[pointer + "#keys"] = [key for key, _ in tree[1]]
        for key, item in tree[1]:
            flatten(item, f"{pointer}/{key}", out)
    elif isinstance(tree, tuple) and tree[0] == "arr":
        out[pointer + "#len"] = len(tree[1])
        for index, item in enumerate(tree[1]):
            flatten(item, f"{pointer}/{index}", out)
    else:
        out[pointer] = tree
    return out


def request_fields(request):
    try:
        tree = parse_tree(json.loads(request["body"], object_pairs_hook=lambda pairs: ("obj", pairs)))
    except ValueError:
        tree = request["body"].decode(errors="replace")
    fields = flatten(tree)
    fields["#path"] = request["path"]
    return fields


def shape(value):
    return PORT.sub("127.0.0.1:<port>", HEX.sub("<hex>", json.dumps(value, ensure_ascii=False)))


def noise_pointers(first, second):
    """Pointers whose values differ between two runs of the same build."""
    noisy = set()
    for left, right in zip(first, second):
        for pointer in set(left) | set(right):
            if left.get(pointer) != right.get(pointer):
                noisy.add(pointer)
    return noisy


def first_difference(left, right):
    limit = min(len(left), len(right))
    return next((index for index in range(limit) if left[index] != right[index]), limit)


def excerpt(text, offset):
    start = max(0, offset - EXCERPT // 2)
    return text[start:start + EXCERPT]


def first_line(text):
    return next((line for line in text.splitlines() if line.strip()), "")


def describe(case, call, pointer, base, head):
    kind = "added" if base is None else "removed" if head is None else "changed"
    if pointer.endswith("#keys") and kind == "changed" and sorted(base) == sorted(head):
        kind = "key order"
    entry = {"case": case, "call": call, "pointer": pointer, "kind": kind}
    if kind == "key order":
        offset = first_difference(base, head)
        entry.update(offset=offset, base=base[offset:offset + 3], head=head[offset:offset + 3])
    elif isinstance(base, str) and isinstance(head, str):
        offset = first_difference(base, head)
        entry.update(offset=offset, baseLength=len(base), headLength=len(head),
                     base=excerpt(base, offset), head=excerpt(head, offset))
    else:
        entry.update(base=base, head=head)
    return entry


def fingerprint(entry):
    pattern = re.sub(r"/\d+(?=/|#|~|$)", "/*", entry["pointer"])
    delta = ""
    if "baseLength" in entry:
        change = entry["headLength"] - entry["baseLength"]
        delta = f"{change:+d} chars"
    return f"{pattern} | {entry['kind']}{' | ' + delta if delta else ''}"


def masked_body(request):
    return PORT.sub("127.0.0.1:<port>", HEX.sub("<hex>", request["body"].decode(errors="replace")))


def compare_case(case, base_runs, head_runs):
    """Differences between builds for one case, ignoring each build's own run-to-run noise."""
    name = case["name"]
    for label, runs in (("base", base_runs), ("head", head_runs)):
        if len(runs[0]["requests"]) != len(runs[1]["requests"]) or runs[0]["exit"] != runs[1]["exit"]:
            raise CaseError(f"{name}: the {label} build is not repeatable "
                            f"({len(runs[0]['requests'])} vs {len(runs[1]['requests'])} calls)")
    base_fields = [[request_fields(r) for r in run["requests"]] for run in base_runs]
    head_fields = [[request_fields(r) for r in run["requests"]] for run in head_runs]
    noisy = noise_pointers(*base_fields) | noise_pointers(*head_fields)
    differences, prefixes = [], []
    if base_runs[0]["exit"] != head_runs[0]["exit"]:
        differences.append({"case": name, "call": None, "pointer": "#exit", "kind": "changed",
                            "base": base_runs[0]["exit"], "head": head_runs[0]["exit"]})
    base_requests, head_requests = base_runs[0]["requests"], head_runs[0]["requests"]
    if len(base_requests) != len(head_requests):
        differences.append({"case": name, "call": None, "pointer": "#calls", "kind": "changed",
                            "base": len(base_requests), "head": len(head_requests)})
    for call, (base, head) in enumerate(zip(base_fields[0], head_fields[0])):
        for pointer in list(base) + [p for p in head if p not in base]:
            left, right = base.get(pointer), head.get(pointer)
            same = shape(left) == shape(right) if pointer in noisy else left == right
            if not same and pointer.endswith("#keys") and (left is None or right is None):
                same = True  # a whole object added or removed: its fields are reported instead
            elif not same and pointer.endswith("#keys") and left and right and set(left) != set(right):
                # Added or removed keys are reported at their own pointers; only a changed
                # order of the keys both sides share is a separate difference here.
                same = [k for k in left if k in right] == [k for k in right if k in left]
            if not same:
                differences.append(describe(name, call, pointer, left, right))
        left, right = masked_body(base_requests[call]), masked_body(head_requests[call])
        if left != right:
            prefixes.append({"case": name, "call": call, "offset": first_difference(left, right),
                             "baseLength": len(left), "headLength": len(right)})
    return differences, prefixes, len(noisy)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", type=Path, required=True, help="xgen binary to compare against (e.g. main)")
    parser.add_argument("--head", type=Path, required=True, help="xgen binary under review")
    parser.add_argument("--cases", type=Path, default=DEFAULT_CASES)
    parser.add_argument("--case", action="append", help="run only the named case (repeatable)")
    parser.add_argument("--json", type=Path, help="write the full report here")
    args = parser.parse_args(argv)
    base, head = args.base.resolve(strict=True), args.head.resolve(strict=True)
    cases = json.loads(args.cases.read_text())["cases"]
    if args.case:
        unknown = set(args.case) - {case["name"] for case in cases}
        if unknown:
            parser.error(f"unknown case: {', '.join(sorted(unknown))}")
        cases = [case for case in cases if case["name"] in args.case]

    report = {"base": str(base), "head": str(head), "cases": [], "differences": [], "prefixes": []}
    with tempfile.TemporaryDirectory(prefix="xgen-request-diff-") as temporary:
        for case in cases:
            root = Path(temporary) / "case"
            try:
                base_runs = [run_case(base, case, root) for _ in range(2)]
                head_runs = [run_case(head, case, root) for _ in range(2)]
                differences, prefixes, noisy = compare_case(case, base_runs, head_runs)
            except CaseError as error:
                print(f"error: {error}", file=sys.stderr)
                return 2
            if base_runs[0]["unused"]:
                # The case no longer drives the base build through its script; say so.
                print(f"warning: {case['name']}: base left {base_runs[0]['unused']} scripted replies "
                      f"unused (exit {base_runs[0]['exit']}): {first_line(base_runs[0]['stderr'])}",
                      file=sys.stderr)
            report["cases"].append({"name": case["name"], "calls": len(base_runs[0]["requests"]),
                                    "headCalls": len(head_runs[0]["requests"]), "noiseFields": noisy,
                                    "differences": len(differences)})
            report["differences"] += differences
            report["prefixes"] += prefixes

    groups = OrderedDict()
    for entry in report["differences"]:
        groups.setdefault(fingerprint(entry), []).append(entry)
    report["groups"] = [{"fingerprint": key, "count": len(items), "example": items[0]}
                        for key, items in groups.items()]
    if args.json:
        args.json.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")

    for case in report["cases"]:
        calls = case["calls"] if case["calls"] == case["headCalls"] else f"{case['calls']}->{case['headCalls']}"
        print(f"{case['name']}: calls {calls}, noise fields {case['noiseFields']}, "
              f"differences {case['differences']}")
    if not groups:
        print("model requests match")
        return 0
    print(f"\n{len(report['differences'])} differences in {len(groups)} groups:")
    for group in report["groups"]:
        example = group["example"]
        where = f"{example['case']} call {example['call']}"
        unit = "key" if example["kind"] == "key order" else "char"
        detail = (f"{unit} {example['offset']}: {example['base']!r} -> {example['head']!r}"
                  if "offset" in example else f"{example['base']!r} -> {example['head']!r}")
        print(f"- [{group['count']}x] {group['fingerprint']}\n    e.g. {where}, {detail[:240]}")
    print("\nfirst differing byte per request (prompt cache stops matching here):")
    for prefix in report["prefixes"]:
        print(f"- {prefix['case']} call {prefix['call']}: byte {prefix['offset']} "
              f"of {prefix['baseLength']} -> {prefix['headLength']}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
