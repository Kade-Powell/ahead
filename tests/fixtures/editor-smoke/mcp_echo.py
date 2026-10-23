#!/usr/bin/env python3
"""Offline stdio MCP echo server for disposable AHEAD smoke projects."""

import json
import sys
import threading

form_enabled = "--form" in sys.argv[2:]
url_enabled = "--url" in sys.argv[2:]
modern = False
pending_elicitation_call_id = None
elicitation_request_id = "ahead-smoke-url-1" if url_enabled else "ahead-smoke-form-1"
pending_calls = {}
trace_lock = threading.Lock()
stdout_lock = threading.Lock()


def record(entry):
    if len(sys.argv) > 1:
        with trace_lock, open(sys.argv[1], "a", encoding="utf-8") as trace:
            trace.write(json.dumps(entry) + "\n")


def send(message):
    with stdout_lock:
        print(json.dumps(message), flush=True)


def validate_modern_request(request):
    metadata = request.get("params", {}).get("_meta", {})
    if metadata.get("io.modelcontextprotocol/protocolVersion") != "2026-07-28":
        raise ValueError("MCP request must use protocol 2026-07-28")
    client_info = metadata.get("io.modelcontextprotocol/clientInfo", {})
    if client_info.get("name") != "AHEAD":
        raise ValueError("MCP client must identify as AHEAD")
    if not isinstance(metadata.get("io.modelcontextprotocol/clientCapabilities"), dict):
        raise ValueError("MCP request must include client capabilities")

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if (form_enabled or url_enabled) and request.get("id") == elicitation_request_id and not method:
        if pending_elicitation_call_id is None:
            raise ValueError("unexpected MCP elicitation response")
        response = request.get("result", {"action": "error", "error": request.get("error")})
        record({"method": "elicitation/result", "params": response})
        send({"jsonrpc": "2.0", "id": pending_elicitation_call_id, "result": {
            "content": [{"type": "text", "text": json.dumps(response)}],
        }})
        pending_elicitation_call_id = None
        continue
    if method == "notifications/cancelled":
        params = request.get("params", {})
        record({"method": method, "params": params})
        cancellation = pending_calls.get(params.get("requestId"))
        if cancellation is not None:
            cancellation.set()
        continue
    if method in ("tools/call", "resources/list", "resources/templates/list", "resources/read") and len(sys.argv) > 1:
        params = {key: value for key, value in request.get("params", {}).items() if key != "_meta"}
        record({"method": method, "params": params, "request_id": request.get("id")})
    if method == "server/discover":
        if form_enabled or url_enabled:
            print(json.dumps({
                "jsonrpc": "2.0",
                "id": request["id"],
                "error": {"code": -32601, "message": "Method not found"},
            }), flush=True)
            continue
        validate_modern_request(request)
        modern = True
        result = {
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": {"tools": {}, "resources": {}},
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {
                    "name": "ahead-smoke-echo",
                    "version": "1.0",
                },
            },
            "instructions": "Echo text and expose one smoke resource.",
            "ttlMs": 0,
            "cacheScope": "private",
        }
    elif method == "initialize":
        if not form_enabled and not url_enabled:
            raise ValueError("default MCP smoke must negotiate the modern protocol")
        if request["params"]["clientInfo"]["name"] != "AHEAD":
            raise ValueError("MCP client must identify as AHEAD")
        if url_enabled and "url" not in request["params"].get("capabilities", {}).get("elicitation", {}):
            raise ValueError("MCP client must advertise URL elicitation")
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "ahead-smoke-echo", "version": "1.0"},
        }
    elif method == "tools/list":
        result = {
            "tools": [{
                "name": "echo",
                "description": "Echo text without network or filesystem access",
                "annotations": {"readOnlyHint": True},
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string"},
                        "payload": {"type": "object"},
                    },
                    "required": ["text"],
                },
            }, {
                "name": "stateful",
                "description": "Return an MRTR request-state challenge before echoing text",
                "annotations": {"readOnlyHint": True},
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                },
            }]
        }
        if form_enabled:
            result["tools"].append({
                "name": "ask_form",
                "description": "Ask the user for non-secret sample settings through MCP form elicitation",
                "annotations": {"readOnlyHint": True},
                "inputSchema": {"type": "object", "properties": {}},
            })
        if url_enabled:
            result["tools"].append({
                "name": "ask_url",
                "description": "Ask the user to open a test website through MCP URL elicitation",
                "annotations": {"readOnlyHint": True},
                "inputSchema": {"type": "object", "properties": {}},
            })
    elif method == "tools/call":
        tool_name = request["params"]["name"]
        arguments = request["params"].get("arguments", {})
        if tool_name == "echo" and arguments.get("text") == "AHEAD_MCP_CANCEL_SENTINEL":
            request_id = request["id"]
            cancellation = threading.Event()
            pending_calls[request_id] = cancellation

            def finish_after_cancel_window():
                if not cancellation.wait(30):
                    send({"jsonrpc": "2.0", "id": request_id, "result": {
                        "content": [{"type": "text", "text": arguments["text"]}],
                    }})

            threading.Thread(target=finish_after_cancel_window, daemon=True).start()
            continue
        if (form_enabled and tool_name == "ask_form") or (url_enabled and tool_name == "ask_url"):
            pending_elicitation_call_id = request["id"]
            params = ({
                "mode": "url",
                "message": "Open the test website to continue",
                "url": "https://example.test/connect?state=abc",
                "elicitationId": "ahead-smoke-url-flow",
            } if url_enabled else {
                "mode": "form",
                "message": "Choose sample settings for the smoke test",
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "count": {"type": "integer", "minimum": 1},
                        "color": {"type": "string", "enum": ["red", "blue"]},
                        "nickname": {"type": "string", "minLength": 2},
                    },
                    "required": ["count", "nickname"],
                },
            })
            print(json.dumps({
                "jsonrpc": "2.0", "id": elicitation_request_id,
                "method": "elicitation/create", "params": params,
            }), flush=True)
            continue
        if tool_name == "stateful":
            if request["params"].get("requestState") != "AHEAD_MCP_MRTR_STATE":
                result = {"resultType": "input_required", "requestState": "AHEAD_MCP_MRTR_STATE"}
            else:
                result = {"resultType": "complete", "content": [{"type": "text", "text": arguments["text"]}]}
        else:
            result = {"content": [{"type": "text", "text": arguments["text"]}]}
    elif method == "resources/list":
        result = {
            "resources": [{
                "uri": "memo://ahead-smoke",
                "name": "AHEAD smoke resource",
                "mimeType": "text/plain",
            }]
        }
    elif method == "resources/read":
        if request["params"]["uri"] != "memo://ahead-smoke":
            raise ValueError("unexpected resource URI")
        result = {
            "contents": [{
                "uri": "memo://ahead-smoke",
                "mimeType": "text/plain",
                "text": "MCP_RESOURCE_SENTINEL",
            }]
        }
    elif method == "resources/templates/list":
        result = {
            "resourceTemplates": [{
                "uriTemplate": "memo://{id}",
                "name": "AHEAD smoke template",
            }]
        }
    elif method == "ping":
        result = {}
    else:
        continue

    if modern and method != "server/discover":
        validate_modern_request(request)
        result.setdefault("resultType", "complete")
        if method in (
            "tools/list",
            "resources/list",
            "resources/templates/list",
            "resources/read",
        ):
            result["ttlMs"] = 0
            result["cacheScope"] = "private"

    if "id" in request:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
