"""Pinned internal retrieval worker. No business API execution or model calls."""

import hashlib
import gzip
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import warnings

VERSION = "0.46.0"
LIMIT = 256 * 1024 * 1024
COMPRESSED_LIMIT = 64 * 1024 * 1024


class WorkerError(Exception):
    pass


def canonical(value):
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def read_json(path, limit=LIMIT):
    path = Path(path)
    if path.suffix == ".gz" and path.stat().st_size > COMPRESSED_LIMIT:
        raise WorkerError("document_size_limit")
    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rb") as stream:
        raw = stream.read(limit + 1)
    if len(raw) > limit:
        raise WorkerError("document_size_limit")
    return json.loads(raw, object_pairs_hook=unique_object, parse_constant=invalid_constant)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise WorkerError("duplicate_json_key")
        result[key] = value
    return result


def invalid_constant(_value):
    raise WorkerError("invalid_json_number")


def root_directory(root):
    path = Path(root)
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode):
        raise WorkerError("invalid_collection_directory")
    if os.name == "posix" and (info.st_uid != os.getuid() or info.st_mode & 0o077):
        raise WorkerError("collection_directory_not_private")
    return path


def store(root, name, artifact):
    envelope = {"format_version": 1, "collection": name, "backend": "graph-tool-call",
                "backend_version": VERSION, "artifact_digest": digest(artifact), "artifact": artifact}
    encoded = canonical(envelope)
    if len(encoded) > LIMIT:
        raise WorkerError("document_size_limit")
    compressed = gzip.compress(encoded, compresslevel=1, mtime=0)
    if len(compressed) > COMPRESSED_LIMIT:
        raise WorkerError("document_size_limit")
    target = root / (name + ".json.gz")
    fd, temporary = tempfile.mkstemp(prefix=".import-", dir=root)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(compressed)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            # Publish only complete bytes, and never replace an existing collection.
            os.link(temporary, target)
        except FileExistsError:
            raise WorkerError("collection_already_exists") from None
        except OSError:
            raise WorkerError("collection_publish_unavailable") from None
    finally:
        os.unlink(temporary)
    return envelope


def load(root, name):
    path = root / (name + ".json.gz")
    if path.is_symlink():
        raise WorkerError("invalid_collection_file")
    try:
        envelope = read_json(path)
    except FileNotFoundError:
        raise WorkerError("collection_not_found") from None
    if (envelope.get("format_version") != 1 or envelope.get("collection") != name
            or envelope.get("backend") != "graph-tool-call"
            or envelope.get("backend_version") != VERSION
            or envelope.get("artifact_digest") != digest(envelope.get("artifact"))):
        raise WorkerError("collection_integrity_failure")
    return envelope


def graph_from_artifact(artifact):
    from graph_tool_call import ToolGraph
    # Use the public persisted-graph loader without modifying the saved collection.
    with tempfile.TemporaryDirectory(prefix="xgen-graph-") as directory:
        path = Path(directory) / "graph.json"
        path.write_bytes(canonical(artifact))
        return ToolGraph.load(path)


def summarize(envelope):
    artifact = envelope["artifact"]
    return {"collection": envelope["collection"], "artifact_digest": envelope["artifact_digest"],
            "backend_version": VERSION, "tool_count": len(artifact["tools"]),
            "source_count": artifact["metadata"]["source_count"],
            "execution_enabled": False}


def dispatch(request):
    from graph_tool_call import __version__
    from graph_tool_call.graphify import build_openapi_collection_artifact, expand_candidates_with_producers
    if __version__ != VERSION:
        raise WorkerError("backend_version_mismatch")
    name = request.get("name")
    if name is not None and (not isinstance(name, str) or not 1 <= len(name) <= 64
                             or not all(c.isascii() and (c.isalnum() or c in "-_") for c in name)):
        raise WorkerError("invalid_collection_name")
    root = root_directory(request["root"])
    operation = request["operation"]
    if operation == "list":
        return {"ok": True, "collections": [summarize(load(root, path.name[:-8]))
                                                for path in sorted(root.glob("*.json.gz"))]}
    if operation == "import":
        if (root / (name + ".json.gz")).exists():
            raise WorkerError("collection_already_exists")
        sources = request["sources"]
        if not 1 <= len(sources) <= 16:
            raise WorkerError("invalid_sources")
        # JSON only for local files; no YAML extra or credentials are needed.
        inputs = [source if source.startswith("https://") else read_json(source, 5_000_000)
                  for source in sources]
        artifact = build_openapi_collection_artifact(inputs, max_response_bytes=5_000_000)
        artifact["metadata"]["xgen_source_inputs"] = [
            {"kind": "https" if isinstance(value, str) else "json",
             "label": source if isinstance(value, str) else Path(source).name,
             "document_digest": None if isinstance(value, str) else digest(value)}
            for source, value in zip(sources, inputs)
        ]
        envelope = store(root, name, artifact)
        return {"ok": True, **summarize(envelope)}
    envelope = load(root, name)
    graph = graph_from_artifact(envelope["artifact"])
    if operation == "describe":
        tool = graph.tools.get(request["tool"])
        if tool is None:
            raise WorkerError("tool_not_found")
        return {"ok": True, **summarize(envelope), "tool": tool.to_dict(),
                "contract_kind": "discovery_candidate", "effect_class": "unclassified"}
    if operation == "search":
        query, top_k = request["query"], request["top_k"]
        if not isinstance(query, str) or not query.strip() or len(query.encode()) > 4096:
            raise WorkerError("invalid_query")
        if type(top_k) is not int or not 1 <= top_k <= 20:
            raise WorkerError("invalid_top_k")
        hits = graph.retrieve_with_scores(query, top_k=top_k)
        names = [hit.tool.name for hit in hits]
        tools = {name: tool.to_dict() for name, tool in graph.tools.items()}
        expanded = expand_candidates_with_producers(names, tools, max_hops=1)
        return {"ok": True, **summarize(envelope),
                "candidates": [{"tool": hit.tool.name, "description": hit.tool.description,
                                "score": hit.score} for hit in hits],
                "possible_producers": [name for name in expanded if name not in names][:40],
                "omitted_producers": max(0, len(set(expanded) - set(names)) - 40),
                "relations_verified_by_execution": False}
    raise WorkerError("unknown_operation")


def main():
    warnings.simplefilter("ignore")
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            raise WorkerError("request_size_limit")
        response = dispatch(json.loads(raw, object_pairs_hook=unique_object, parse_constant=invalid_constant))
    except WorkerError as error:
        response = {"ok": False, "error": str(error)}
    except Exception:
        # Source documents, paths, tokens and upstream exception bodies stay private.
        response = {"ok": False, "error": "collection_operation_failed"}
    sys.stdout.buffer.write(canonical(response) + b"\n")


if __name__ == "__main__":
    main()
