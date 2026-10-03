#!/usr/bin/env python3
"""Offline stdio MCP echo server for disposable AHEAD smoke projects."""

import json
import sys

form_enabled = "--form" in sys.argv[2:]
url_enabled = "--url" in sys.argv[2:]
pending_elicitation_call_id = None
elicitation_request_id = "ahead-smoke-url-1" if url_enabled else "ahead-smoke-form-1"

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if (form_enabled or url_enabled) and request.get("id") == elicitation_request_id and not method:
        if pending_elicitation_call_id is None:
            raise ValueError("unexpected MCP elicitation response")
        response = request.get("result", {"action": "error", "error": request.get("error")})
        if len(sys.argv) > 1:
            with open(sys.argv[1], "a", encoding="utf-8") as trace:
                trace.write(json.dumps({"method": "elicitation/result", "params": response}) + "\n")
        print(json.dumps({"jsonrpc": "2.0", "id": pending_elicitation_call_id, "result": {
            "content": [{"type": "text", "text": json.dumps(response)}],
        }}), flush=True)
        pending_elicitation_call_id = None
        continue
    if method in ("tools/call", "resources/list", "resources/templates/list", "resources/read") and len(sys.argv) > 1:
        params = {key: value for key, value in request.get("params", {}).items() if key != "_meta"}
        with open(sys.argv[1], "a", encoding="utf-8") as trace:
            trace.write(json.dumps({"method": method, "params": params}) + "\n")
    if method == "initialize":
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
        arguments = request["params"].get("arguments", {})
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

    if "id" in request:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
