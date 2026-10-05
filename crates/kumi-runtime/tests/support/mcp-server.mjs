// Synthetic MCP protocol fixture. Never connects to Ableton or claims real-Live provenance.
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ErrorCode, ListToolsRequestSchema, McpError } from "@modelcontextprotocol/sdk/types.js";
import { setTimeout as delay } from "node:timers/promises";
import { writeFileSync } from "node:fs";
const mode = process.argv[2] ?? "normal";
const server = new Server({ name: "kumi-synthetic-fixture", version: "1" }, { capabilities: { tools: { listChanged: true } } });
let changed = false;
let cancelled = 0;
const calls = [];
const descriptor = (name) => ({ name, description: `Synthetic ${name}`, inputSchema: { type: "object", properties: { action: { type: "string" } }, additionalProperties: true } });
server.setRequestHandler(ListToolsRequestSchema, async (request) => {
  if (mode === "repeat-cursor") return { tools: [], nextCursor: "same" };
  if (mode === "excessive") return { tools: Array.from({ length: 600 }, (_, i) => descriptor(`extra_${i}`)) };
  if (mode === "duplicate") return { tools: [descriptor("live_status"), descriptor("live_status")] };
  if (mode === "missing") return { tools: [descriptor("server_status")] };
  if (mode === "catalog-bytes") return { tools: [{ ...descriptor("server_status"), description: "x".repeat(1024 * 1024) }] };
  const all = ["server_status", "live_status", "live_discover", ...(changed ? ["new_unsafe_tool"] : ["live_note_read"]), ...Array.from({ length: 100 }, (_, i) => `mutation_${i}`)].map(descriptor);
  const offset = Number(request.params?.cursor ?? 0);
  return { tools: all.slice(offset, offset + 40), ...(offset + 40 < all.length ? { nextCursor: String(offset + 40) } : {}) };
});
server.setRequestHandler(CallToolRequestSchema, async (request, extra) => {
  const { name, arguments: args = {} } = request.params;
  calls.push(name);
  if (args.action === "delay") {
    extra.signal.addEventListener("abort", () => { cancelled++; }, { once: true });
    await delay(60_000, undefined, { signal: extra.signal });
  }
  if (args.action === "exit") { process.exit(0); }
  // An answer that crosses the client's cancel: written after it, as a bridge finishing its work does.
  if (args.action === "late") {
    const id = extra.requestId;
    setTimeout(() => process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id, result: { content: [{ type: "text", text: "{}" }] } })}\n`), 150);
    await delay(60_000, undefined, { signal: extra.signal }).catch(() => {});
    await delay(60_000);
  }
  if (args.action === "notify") { changed = true; await server.sendToolListChanged(); }
  if (args.action === "error") return { isError: true, content: [{ type: "text", text: "fixture failure" }], structuredContent: { fixture: true, reason: "expected-error" } };
  if (args.action === "invalid-params") throw new McpError(ErrorCode.InvalidParams, "trackRef is required");
  if (args.action === "oversized") return { content: [{ type: "text", text: "x".repeat(70 * 1024) }] };
  if (args.action === "frame") return { content: [{ type: "text", text: "x".repeat(65 * 1024 * 1024) }] };
  if (args.action === "large") return { content: [{ type: "text", text: "x".repeat(5 * 1024 * 1024) }] };
  const value = { fixture: true, calls: [...calls], cancelled, provenance: "synthetic-fixture", toolPolicy: process.env.ABLETON_MCP_TOOL_POLICY ?? null, toolAllow: process.env.ABLETON_MCP_TOOL_ALLOW ?? null, secretPresent: ["AI_GATEWAY_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENCODE_API_KEY", "KUMI_AUTH_FILE", "NODE_OPTIONS"].some((key) => Boolean(process.env[key])) };
  return { content: [{ type: "text", text: JSON.stringify(value) }], structuredContent: value };
});
if (mode === "stderr") process.stderr.write("must-not-be-retained-secret".repeat(20_000));
if (mode === "stubborn") { setInterval(() => {}, 1000); process.on("SIGTERM", () => {}); }
if (mode === "no-init") {
  writeFileSync(process.argv[3], String(process.pid));
  process.stdin.resume();
  await new Promise(() => {});
}
await server.connect(new StdioServerTransport());
