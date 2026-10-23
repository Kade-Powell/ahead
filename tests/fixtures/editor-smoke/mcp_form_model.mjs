import { createServer } from "node:http";
import { appendFileSync, writeFileSync } from "node:fs";

const [, , portFile, traceFile, mode = "form"] = process.argv;
const catalogMode = mode === "catalog-a" || mode === "catalog-b";
if (!portFile || !traceFile || (!["form", "url", "text"].includes(mode) && !catalogMode)) throw new Error("usage: node mcp_form_model.mjs PORT_FILE TRACE_FILE [form|url|text|catalog-a|catalog-b]");
const toolName = mode === "url" ? "ask_url" : "ask_form";
const modelIds = catalogMode
  ? ["shared-model", ...Array.from({ length: 120 }, (_, index) => `${mode}/model-${String(index + 1).padStart(3, "0")}`)]
  : ["gpt-5.5"];

let turn = 0;
const server = createServer(async (request, response) => {
  if (request.method === "GET" && request.url === "/v1") {
    response.writeHead(200, { "content-type": "application/json" }).end("{}");
    return;
  }
  if (request.method === "GET" && request.url === "/v1/models") {
    if (catalogMode) {
      appendFileSync(traceFile, JSON.stringify({ type: "model_catalog_request", method: request.method, url: request.url }) + "\n");
    }
    response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({
      object: "list",
      data: modelIds.map((id) => {
        const reasoning = mode === "catalog-a" && id === "shared-model"
          ? { default_effort: "medium", supported_efforts: ["low", "medium", "high"] }
          : undefined;
        return {
          id,
          object: "model",
          name: id === "shared-model" ? "Shared Model" : undefined,
          ...(reasoning ? { reasoning } : {}),
        };
      }),
    }));
    return;
  }
  if (request.method !== "POST" || request.url !== "/v1/responses") {
    response.writeHead(404).end();
    return;
  }
  let body = "";
  for await (const chunk of request) body += chunk;
  appendFileSync(traceFile, JSON.stringify({ turn, request: JSON.parse(body) }) + "\n");
  const toolSearch = { type: "tool_search_call", id: "form-search", call_id: "form-search", execution: "client", arguments: { query: toolName, limit: 1 } };
  const toolCall = { type: "function_call", id: "form-call", call_id: "form-call", namespace: "mcp__echo", name: toolName, arguments: "{}" };
  const plainReply = mode === "text" || catalogMode;
  const message = { type: "message", id: "form-message", role: "assistant", content: [{ type: "output_text", text: mode === "text" ? "AHEAD_TEXT_SMOKE_OK" : catalogMode ? "AHEAD_MODEL_CATALOG_SMOKE_OK" : `MCP ${mode} complete` }] };
  const phase = turn++ % 3;
  const events = plainReply
    ? [
      { type: "response.output_item.added", output_index: 0, item: { ...message, content: [] } },
      { type: "response.output_item.done", output_index: 0, item: message },
      { type: "response.completed", response: { id: "text-response-1", end_turn: true } },
    ]
    : phase === 0
    ? [
      { type: "response.output_item.added", output_index: 0, item: toolSearch },
      { type: "response.output_item.done", output_index: 0, item: toolSearch },
      { type: "response.completed", response: { id: "form-response-1", end_turn: false } },
    ]
    : phase === 1
      ? [
        { type: "response.output_item.added", output_index: 0, item: toolCall },
        { type: "response.output_item.done", output_index: 0, item: toolCall },
        { type: "response.completed", response: { id: "form-response-2", end_turn: false } },
      ]
      : [
        { type: "response.output_item.added", output_index: 0, item: { ...message, content: [] } },
        { type: "response.output_item.done", output_index: 0, item: message },
        { type: "response.completed", response: { id: "form-response-3", end_turn: true } },
      ];
  response.writeHead(200, { "content-type": "text/event-stream" });
  for (const event of events) response.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`);
  response.end();
});
server.listen(0, "127.0.0.1", () => writeFileSync(portFile, String(server.address().port)));
