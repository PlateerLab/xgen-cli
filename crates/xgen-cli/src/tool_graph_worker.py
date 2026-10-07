"""Pinned internal retrieval worker. No business API execution or model calls."""

import collections
import copy
import hashlib
import gzip
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unicodedata
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


def letter_script(char):
    name = unicodedata.name(char, "").split(" ")
    if name[0] in ("FULLWIDTH", "HALFWIDTH") and len(name) > 1:
        name = name[1:]
    return {"CJK": "Han", "HIRAGANA": "Kana", "KATAKANA": "Kana"}.get(name[0], name[0].title()) or None


def summary_script(text):
    """Script of one summary. Latin letters appear in identifiers and acronyms of any
    documentation, so any other script wins; Hangul and Kana win over Han because
    Korean and Japanese text may contain Han."""
    letters = collections.Counter(letter_script(char) for char in text if char.isalpha())
    letters.pop(None, None)
    other = {script: count for script, count in letters.items() if script != "Latin"}
    if not letters:
        return None
    if not other:
        return "Latin"
    for script in ("Hangul", "Kana"):
        if script in other:
            return script
    return min(other, key=lambda script: (-other[script], script))


def documentation_script(artifact):
    """Dominant script of operation summaries in the pinned artifact, or None."""
    counts = collections.Counter()
    for tool in artifact["tools"].values():
        openapi = (tool.get("metadata") or {}).get("openapi") or {}
        script = summary_script(str(openapi.get("summary") or tool.get("description") or ""))
        if script:
            counts[script] += 1
    return min(counts, key=lambda script: (-counts[script], script)) if counts else None


def language_hints(root, request, snapshots):
    """Per pinned collection: documentation script and whether the request uses another script.

    The planner writes search queries. Measured on held-out requests, a query that keeps
    the request's terms and adds documentation-language terms recovers cross-language
    misses; same-script requests keep the unchanged prompt.
    """
    if (not isinstance(request, str) or not isinstance(snapshots, dict)
            or not 1 <= len(snapshots) <= 16):
        raise WorkerError("invalid_language_hint_request")
    request_script = summary_script(request)
    result = {}
    for name, expected in sorted(snapshots.items()):
        if not isinstance(name, str) or not 1 <= len(name) <= 64 or not all(
                c.isascii() and (c.isalnum() or c in "-_") for c in name):
            raise WorkerError("invalid_collection_name")
        envelope = load(root, name)
        if envelope["artifact_digest"] != expected:
            raise WorkerError("collection_snapshot_changed")
        script = documentation_script(envelope["artifact"])
        result[name] = {"documentation_script": script,
                        "script_differs": bool(request_script and script and request_script != script)}
    return result


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
    if operation == "language_hints":
        return {"ok": True, "collections": language_hints(root, request.get("request"), request.get("snapshots"))}
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
        artifact["metadata"]["xgen_http_sources"] = {f"inline:{index}": copy.deepcopy(value) for index, value in enumerate(inputs, 1)} if all(isinstance(value, dict) for value in inputs) else {}
        artifact["metadata"]["xgen_source_inputs"] = [
            {"kind": "https" if isinstance(value, str) else "json",
             "label": source if isinstance(value, str) else Path(source).name,
             "document_digest": None if isinstance(value, str) else digest(value)}
            for source, value in zip(sources, inputs)
        ]
        envelope = store(root, name, artifact)
        return {"ok": True, **summarize(envelope)}
    envelope = load(root, name)
    if request.get("expected_digest") is not None and request["expected_digest"] != envelope["artifact_digest"]:
        raise WorkerError("collection_snapshot_changed")
    graph = graph_from_artifact(envelope["artifact"])
    if operation in ("describe", "describe_view", "execution_contract"):
        tool = graph.tools.get(request["tool"])
        if tool is None:
            raise WorkerError("tool_not_found")
        contract = http_read_contract(envelope["artifact"], tool.to_dict())
        if operation == "execution_contract":
            return {"ok": True, **summarize(envelope), "tool_name": tool.name,
                    "http_read": contract, "contract_kind": "host_http_contract"}
        if operation == "describe_view":
            return describe_view(envelope, tool.to_dict(), contract, request.get("parameter_offset", 0))
        return {"ok": True, **summarize(envelope), "tool": tool.to_dict(), "http_read": contract,
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


def dereference(value, document, stack=(), depth=0):
    if depth > 48:
        raise WorkerError("http_schema_depth_limit")
    if isinstance(value, list):
        return [dereference(item, document, stack, depth+1) for item in value]
    if not isinstance(value, dict):
        return value
    if "$ref" in value:
        reference = value["$ref"]
        if not isinstance(reference, str) or not reference.startswith("#/") or reference in stack:
            raise WorkerError("http_schema_reference_unsupported")
        target = document
        try:
            for part in reference[2:].split("/"):
                target = target[part.replace("~1", "/").replace("~0", "~")]
        except (KeyError, TypeError):
            raise WorkerError("http_schema_reference_missing") from None
        resolved = dereference(target, document, (*stack,reference), depth+1)
        if len(value) != 1:
            raise WorkerError("http_schema_ref_siblings_unsupported")
        return resolved
    return {key:dereference(item, document, stack, depth+1) for key,item in value.items()}


def check_http_schema(value):
    # Visit schema positions, never instance examples or maps of property names.
    if not isinstance(value, dict):
        return
    if "nullable" in value or any(isinstance(value.get(k),bool) for k in ("exclusiveMinimum","exclusiveMaximum")) or any(k in value for k in ("$dynamicRef","$recursiveRef")):
        raise WorkerError("http_schema_dialect_unsupported")
    for key in ("properties","patternProperties","$defs","definitions","dependentSchemas"):
        for schema in value.get(key,{}).values():
            check_http_schema(schema)
    for key in ("allOf","anyOf","oneOf","prefixItems"):
        for schema in value.get(key,[]):
            check_http_schema(schema)
    for key in ("items","additionalProperties","unevaluatedProperties","unevaluatedItems","contains","propertyNames","not","if","then","else"):
        check_http_schema(value.get(key))


# Presentation is a projection, never an executable contract. All digests refer to
# full values under this worker's canonical JSON encoding, including omitted data.
def preview(value, limit=512):
    if not isinstance(value, str):
        return {"text": "", "omitted": False}
    raw = value.encode("utf-8")
    return {"text": raw[:limit].decode("utf-8", errors="ignore"), "omitted": len(raw) > limit}


def schema_view(schema):
    # Small schemas are complete. Large schemas are explicitly opaque references;
    # the host validates the entire original schema even when this view omits it.
    encoded = canonical(schema)
    if len(encoded) <= 512:
        return {"schema": schema, "schema_digest": digest(schema), "complete": True}
    result = {"schema_digest": digest(schema), "complete": False, "schema_bytes": len(encoded)}
    if isinstance(schema, dict):
        for key in ("type", "minimum", "maximum", "minLength", "maxLength", "format"):
            if key in schema and len(canonical(schema[key])) <= 128:
                result[key] = schema[key]
        if isinstance(schema.get("enum"), list):
            result["enum_count"] = len(schema["enum"])
    return result


def describe_view(envelope, tool, http, offset):
    if type(offset) is not int or offset < 0:
        raise WorkerError("invalid_parameter_offset")
    # Prefer executable original parameters when supported, else discovery inputs.
    parameters = http["contract"]["parameters"] if http["supported"] else tool.get("parameters", [])
    if offset > len(parameters):
        raise WorkerError("invalid_parameter_offset")
    schema_origin = "http_original" if http["supported"] else "discovery_normalized"
    rows = []
    end = offset
    # Bound both count and bytes, including hostile parameter names/descriptions.
    for parameter in parameters[offset:offset + 32]:
        schema = parameter.get("schema")
        if schema_origin == "discovery_normalized":
            schema = {key: parameter[key] for key in ("type", "enum") if parameter.get(key) is not None}
        row = {"index": end, "name": preview(parameter.get("name"), 1024),
               "required": bool(parameter.get("required")),
               "description": preview(parameter.get("description")),
               "schema_origin": schema_origin, **schema_view(schema)}
        if parameter.get("location") in ("path", "query"):
            row["location"] = parameter["location"]
        if len(canonical(rows + [row])) > 40 * 1024:
            break
        rows.append(row)
        end += 1
    metadata = tool.get("metadata", {})
    http_view = {key: value for key, value in http.items() if key != "contract"}
    if http["supported"]:
        contract = http["contract"]
        http_view.update({"method": contract["method"], "path": preview(contract["path"], 1024),
                          "bearer_required": contract["bearer_required"],
                          "response_schema_digests": {status: digest(schema) for status, schema in list(contract["responses"].items())[:32] if len(status) <= 3},
                          "response_schema_count": len(contract["responses"]), "response_schemas_complete": False})
    result = {"ok": True, **summarize(envelope), "view_version": 2,
            "contract_kind": "discovery_view", "effect_class": "unclassified",
            "full_tool_digest": digest(tool), "complete": False,
            "tool": {"name": tool["name"], "description": preview(tool.get("description"), 1024),
                     "parameters": rows, "metadata": {"api_contract": {
                         "method": preview(metadata.get("method"), 16),
                         "path": preview(metadata.get("path"), 1024)}}},
            "parameter_page": {"offset": offset, "total": len(parameters),
                               "next_offset": end if end < len(parameters) else None},
            "http_read": http_view}
    # Reserve space for the request material added by the Rust receipt adapter.
    while len(canonical(result)) > 48 * 1024 and rows:
        rows.pop()
        end -= 1
        result["parameter_page"]["next_offset"] = end if end < len(parameters) else None
    if len(canonical(result)) > 48 * 1024 or (end < len(parameters) and not rows):
        raise WorkerError("tool_discovery_output_limit")
    return result


def http_read_contract(artifact, tool):
    # The normalized graph is discovery data. Invoke only against retained source schemas.
    metadata = tool.get("metadata", {})
    source = artifact.get("metadata", {}).get("xgen_http_sources", {}).get(metadata.get("source_url"))
    try:
        if not isinstance(source, dict):
            raise WorkerError("http_original_source_unavailable")
        method, path = metadata.get("method"), metadata.get("path")
        if method != "get":
            raise WorkerError("http_method_not_read_approved")
        item = dereference(source["paths"][path], source)
        operation = item[method]
        if operation.get("requestBody") or any(p.get("in") == "body" for p in operation.get("parameters", [])):
            raise WorkerError("http_get_body_unsupported")
        merged = {}
        for parameter in [*item.get("parameters", []), *operation.get("parameters", [])]:
            parameter = dereference(parameter, source)
            location, name = parameter.get("in"), parameter.get("name")
            if location not in ("path", "query") or not isinstance(name, str) or not name:
                raise WorkerError("http_parameter_location_unsupported")
            schema = parameter.get("schema")
            if schema is None and source.get("swagger") == "2.0":
                schema = {key:value for key,value in parameter.items() if key in ("type","enum","minimum","maximum","exclusiveMinimum","exclusiveMaximum","minLength","maxLength","pattern","format","default","multipleOf")}
            if not isinstance(schema, dict) or schema.get("type") not in ("string","integer","number","boolean"):
                raise WorkerError("http_parameter_type_unsupported")
            if parameter.get("style", "simple" if location == "path" else "form") != ("simple" if location == "path" else "form") or parameter.get("allowReserved",False) or parameter.get("content"):
                raise WorkerError("http_parameter_serialization_unsupported")
            merged[(location,name)] = {"location":location,"name":name,"required":bool(parameter.get("required")) or location=="path","schema":dereference(schema,source)}
        responses = {}
        for status,response in operation.get("responses", {}).items():
            if not str(status).isdigit() or not 200 <= int(status) <= 299:
                continue
            if source.get("swagger") == "2.0":
                produces = operation.get("produces", source.get("produces", []))
                if produces and "application/json" not in produces:
                    raise WorkerError("http_response_media_unsupported")
                schema = response.get("schema")
            else:
                schema = response.get("content",{}).get("application/json",{}).get("schema")
            if not isinstance(schema,(dict,bool)):
                raise WorkerError("http_response_schema_missing")
            responses[str(status)] = dereference(schema,source)
        if not responses:
            raise WorkerError("http_success_schema_missing")
        security = operation.get("security",source.get("security",[]))
        schemes = source.get("components",{}).get("securitySchemes",source.get("securityDefinitions",{}))
        for requirement in security:
            if not isinstance(requirement,dict) or len(requirement)>1:
                raise WorkerError("http_security_unsupported")
            for name,scopes in requirement.items():
                scheme = dereference(schemes.get(name,{}),source)
                if scheme.get("type") != "http" or scheme.get("scheme", "").lower() != "bearer" or scopes:
                    raise WorkerError("http_security_unsupported")
        for parameter in merged.values():
            check_http_schema(parameter["schema"])
        for schema in responses.values():
            check_http_schema(schema)
        contract = {"format_version":1,"method":"GET","path":path,"base_path":source.get("basePath", ""),"parameters":list(merged.values()),"responses":responses,"bearer_required":bool(security) and not any(not requirement for requirement in security),"source_digest":digest(source)}
        if len(canonical(contract)) > 8 * 1024 * 1024:
            raise WorkerError("http_contract_size_limit")
        return {"supported":True,"contract_digest":digest(contract),"contract":contract}
    except WorkerError as error:
        return {"supported":False,"error":str(error)}
    except (KeyError,TypeError,ValueError):
        return {"supported":False,"error":"http_original_contract_invalid"}


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
