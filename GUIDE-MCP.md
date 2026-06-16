# Guide: Vera MCP Server

Vera exposes its capabilities to Claude Code, Cursor, Claude Desktop,
and any MCP-compatible client through a single Streamable HTTP
endpoint. Same bearer, same audit chain, same vault and ACL pipeline
as the rest of Vera.

Companion to [GUIDE-AGENTS.md](GUIDE-AGENTS.md), [GUIDE-RAG.md](GUIDE-RAG.md),
[GUIDE-SEARCH.md](GUIDE-SEARCH.md), and [GUIDE-IMAGE.md](GUIDE-IMAGE.md).

## TL;DR

```bash
# In Claude Code (one-time install):
claude mcp add vera https://vera.sozenta.ai/mcp \
  --header "Authorization: Bearer $VEYA_BEARER"

# Claude Code now sees vera_scan, vera_search, vera_image,
# kb_search_<id>, agent_<id> as tools.
```

## Architecture

```
Claude Code / Cursor / Claude Desktop
        │
        │ JSON-RPC over HTTPS (Streamable HTTP transport)
        ▼
POST https://vera.sozenta.ai/mcp
   • initialize           — handshake
   • tools/list           — ACL-filtered tool catalog
   • tools/call           — dispatch to existing Vera pipeline
        ▼
Vera pipeline:
   auth → ACL → vault → QoS → connector dispatch → audit chain
```

Spec target: **2026-07-28 RC** (stateless, gateway-friendly). We don't
issue `Mcp-Session-Id`, don't depend on initialize ordering, and any
replica answers any request. Older clients (2025-11-25 / 2025-06-18)
also work — the protocol version is negotiated in `initialize`.

## Authentication

Standard Vera bearer in the `Authorization: Bearer <token>` header.

```bash
# Authenticated as veya
curl -X POST https://vera.sozenta.ai/mcp \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# No bearer → JSON-RPC unauthorized error, HTTP 401
curl -X POST https://vera.sozenta.ai/mcp -d '...'
```

The tool catalog returned by `tools/list` is filtered by the
authenticated principal's ACL — a demo bearer only sees `vera_scan`
(no ACL needed), while veya sees the full catalog including search,
image, KBs, and opted-in agents. **Industry pattern**: explicit
curation, never auto-expose (Glean, Salesforce, Cloudflare).

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| POST | `/mcp` | JSON-RPC entry — `initialize`, `tools/list`, `tools/call`, `ping` |

## JSON-RPC methods

### `initialize`

Handshake. Echoes a supported protocol version and the tools
capability. Stateless — calling it is optional under the 2026-07-28
RC, but most clients send it first.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize",
 "params":{"protocolVersion":"2025-11-25","capabilities":{},
           "clientInfo":{"name":"my-client","version":"1.0"}}}
```

Response:
```json
{"jsonrpc":"2.0","id":1,"result":{
  "protocolVersion":"2025-11-25",
  "capabilities":{"tools":{"listChanged":false}},
  "serverInfo":{"name":"vera","version":"<git-sha>"},
  "instructions":"Vera MCP server. Tools are filtered by your principal's ACL..."
}}
```

### `tools/list`

Returns the tools this principal can call. Includes the JSON Schema
for each tool's input so the MCP client can build function-call
envelopes for the LLM driving it.

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
```

### `tools/call`

Dispatch a tool. The call flows through Vera's normal pipeline (vault
scan on the arguments, ACL check, QoS bucket, audit chain entry).

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call",
 "params":{"name":"vera_search","arguments":{"query":"EU AI Act deadline"}}}
```

Response:
```json
{"jsonrpc":"2.0","id":3,"result":{
  "content":[{"type":"text","text":"<JSON-encoded search result>"}],
  "isError":false
}}
```

### `ping`

Liveness check. Returns `{}`.

## Tools surfaced

All names sanitized to `[a-zA-Z0-9_]+` (dashes become underscores).

| Name | ACL needed | Inputs | Purpose |
|---|---|---|---|
| `vera_scan` | none (bearer only) | `{ text }` | PII scan — returns `{verdict, matches[]}` (rule IDs + offsets, never matched text) |
| `vera_search` | `search` | `{ query, mode?, max_results? }` | Vendor-neutral web search with optional synthesis |
| `vera_image` | `stability-image` | `{ prompt, model?, aspect_ratio?, output_format? }` | Stability image gen on Bedrock |
| `kb_search_<id>` | `bedrock-kb` | `{ query, max_results? }` | One tool per registered Knowledge Base. `<id>` is the sanitized KB id from `/v1/knowledge_bases`. |
| `agent_<id>` | none (per-agent gate via `expose_mcp`) | `{ input }` | One tool per agent manifest with `expose_mcp = true`. Surfaces agents you've explicitly opted in. |

**Add a custom tool**: ship a wasm connector + give it a friendly
schema. Backed-in via `manifest.toml` so it appears in `tools/list`
the next time the wasm dir is loaded.

## Errors

JSON-RPC error codes per the spec:

| Code | Meaning | Vera-side cause |
|---|---|---|
| `-32700` | Parse error | Body isn't valid JSON |
| `-32601` | Method not found | Unknown method or unknown tool |
| `-32602` | Invalid params | Missing required argument (e.g. `name` on `tools/call`) |
| `-32603` | Internal error | Downstream service failure (Bedrock 5xx, etc.) |
| `-32001` | unauthorized (Vera-specific) | Missing/invalid bearer |
| `-32002` | forbidden (Vera-specific) | Principal lacks the tool's ACL |

The HTTP status is `200` for protocol errors (per JSON-RPC convention)
and `401` for missing auth.

## Install in Claude Code

```bash
claude mcp add vera https://vera.sozenta.ai/mcp \
  --header "Authorization: Bearer $VEYA_BEARER"
```

After install, every Claude Code session has the Vera tools available.
Each `tools/call` is audited in your Vera audit chain with the dev's
principal id, vault-scanned, and rate-limited under that principal's
QoS bucket.

**The compliance pitch made tangible:** Claude Code keeps working as
expected; every AI action flows through your audit chain. Devs don't
write any HTTP code; the CISO gets a complete record of who called
what AI capability when.

## Use as an agent tool (server-to-server)

If you're building your own agent runtime and want Vera as a tool
backend:

```python
import requests
def vera_mcp(method, params=None, *, bearer):
    r = requests.post(
        "https://vera.sozenta.ai/mcp",
        headers={"Authorization": f"Bearer {bearer}",
                 "Content-Type": "application/json"},
        json={"jsonrpc":"2.0","id":1,"method":method,"params":params or {}},
        timeout=60,
    )
    return r.json()

tools = vera_mcp("tools/list", bearer=VERA_BEARER)["result"]["tools"]
result = vera_mcp("tools/call",
    {"name": "vera_search",
     "arguments": {"query": "What's the EU AI Act enforcement deadline?"}},
    bearer=VERA_BEARER)
```

## Discovery

The MCP server appears in:

- `GET /v1/services` — under `service_type: "mcp"`
- `GET /llms.txt` — the MCP section lists the install command and the
  surfaced tools

No tool re-list needed when KBs or agents change — `tools/list` is
recomputed from the live registry on every call. (Future: the
`tools/listChanged` capability could be turned on once we wire a
notification channel.)

## What's deliberately out of scope (v1)

- **stdio transport** — disqualified for shared infra (spec exempts
  stdio from auth, no centralized audit interception)
- **OAuth 2.1 / DCR** — bearer auth only for v1; OAuth wrapper planned
  for a follow-up
- **Resources, prompts, sampling** — the spec is deprecating Sampling
  in the 2026-07-28 RC; we only expose tools
- **`tools/listChanged` notifications** — list is recomputed on call
  instead
- **Agent CRUD over MCP** — universally invocation-only across the
  industry; manage agents via the REST endpoints in
  [GUIDE-AGENTS.md](GUIDE-AGENTS.md)
- **MCP-as-management-surface** — same reason

## Federating upstream MCP servers (the gateway)

Vera is also an **MCP gateway**: register an external MCP server and its
tools surface inside Vera's own `/mcp` — ACL-gated, audited, egress-checked
— so a customer adds a custom capability (a 3D-printer server, a CRM MCP,
an internal-API MCP) **without Vera reimplementing it**. One governed
endpoint, many capabilities, filtered per principal.

Register in config:

```toml
[[mcp_upstreams]]
id   = "printer"                    # tools surface as printer.slice, printer.send, …
url  = "http://127.0.0.1:7777/mcp"  # host must be in [egress] allow_hosts
acl  = "printer"                    # principals need this ACL to see/call them
auth = "none"                       # or "bearer" + token = "$PRINTER_MCP_TOKEN"
```

…or live, with **no hub restart** (what the ext installer uses):

```
GET    /admin/mcp/upstreams         # list (token redacted)
POST   /admin/mcp/upstreams         # { id, url, acl, auth, enabled } — upsert
DELETE /admin/mcp/upstreams/{id}    # remove
```

How it behaves:

- **`tools/list`** fetches each enabled upstream's tools, prefixes them
  `<id>.<tool>`, and merges them in — filtered by the caller's ACL. A down
  upstream is **skipped, never fatal**.
- **`tools/call`** on a `<id>.<tool>` name forwards the JSON-RPC to the
  upstream (optional bearer) and audits the dispatch — it bypasses the wasm
  pipeline, so it's logged explicitly.
- The upstream server is **language-agnostic** (Rust/Python/TS) — Vera
  speaks to it over HTTP and governs it identically.

The upstream server itself is shipped as a **vera-metal extension** (see
[GUIDE-METAL.md](GUIDE-METAL.md) → Extensions): `vera-ctl ext install <id>`
fetches a signed artifact, supervises it, and registers it via the admin
endpoint above. The reference ext is a 3D-printer server (slice/send/status,
FlashForge + Moonraker backends, LAN auto-discovery).

## Forward path

The same `/mcp` endpoint will pick up new tools automatically as:

1. New static agents land in `agents/*.toml` with `expose_mcp = true`
2. New KBs are registered via `POST /v1/knowledge_bases`
3. Customer wasm-MCP connectors land (cycle 5.5+) — a wasm component
   that conforms to a Vera-internal tool interface gets surfaced under
   a derived MCP name
4. Cycle 6 alt RAG backends (`pgvector`, `sqlite-vec`) — each gets a
   matching set of `kb_search_<id>` tools

---

## Related guides

- [GUIDE-AGENTS.md](GUIDE-AGENTS.md) — agent CRUD; `expose_mcp` flag
- [GUIDE-RAG.md](GUIDE-RAG.md) — KB registration; `kb_search_<id>` derivation
- [GUIDE-SEARCH.md](GUIDE-SEARCH.md) — `vera_search` underlying API
- [GUIDE-IMAGE.md](GUIDE-IMAGE.md) — `vera_image` underlying API
- [README.md](README.md) — top-level SDK quickstart
