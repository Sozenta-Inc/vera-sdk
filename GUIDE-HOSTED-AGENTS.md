# Hosted Agents on Vera

Vera hosts agents: small, auditable LLM + tool loops that run **inside
your gateway**, behind the same pipeline as every other request —
authenticate → policy ACL → PII vault → QoS → hash-chained audit. You
publish an agent once; anyone with access invokes it over REST,
WebSocket, or MCP. The agent's instructions and documents live on Vera;
its every step is in the audit chain.

```
publisher ──PUT bundle──▶ Vera ◀──POST prompt── consumers (apps, Claude Code via MCP, curl)
                           │
                           └─ LLM ⇄ tools loop, each call audited with agent_id
```

---

## 1. Concepts

| Term | Meaning |
|---|---|
| **Agent** | A manifest: id, display name, driving LLM connector, allowed tools, system prompt, limits. Static (shipped on disk) or dynamic (created via API). |
| **Bundle** | A `.bot.tar.gz` artifact carrying `bot.json` + `instructions.md` (+ optional document corpus). Publishing a bundle creates/updates the agent atomically. Format: [FORMAT-bot-bundle.md](FORMAT-bot-bundle.md). |
| **Sealed bundle** | `"distribution": "sealed"` in `bot.json` — Vera runs the agent but the bundle can **never be downloaded again**, by anyone. For confidential corpora (see §5). |
| **Consumer** | Any bearer whose ACL covers the agent's tools. Can invoke; cannot publish or download. |
| **Publisher** | Bearer with the `agents:write` ACL. Can create, publish, download (unsealed), and delete. |

## 2. Lifecycle

### Publish (create or update)

```bash
# Layout:
#   my-agent/
#   ├── bot.json            # {"version":2, "kind":"veya-bot", "name":"…", …}
#   ├── instructions.md     # the system prompt
#   └── corpus/…            # optional documents (text)
COPYFILE_DISABLE=1 tar -czf my-agent.bot.tar.gz -C parent-dir my-agent/

curl -X PUT https://<your-vera>/v1/agents/my-agent/bundle \
  -H "Authorization: Bearer $PUBLISHER_KEY" \
  -H "Content-Type: application/gzip" \
  --data-binary @my-agent.bot.tar.gz
```

| Response | Meaning |
|---|---|
| `202` | accepted — validated, extracted (atomic swap), live immediately, no restart |
| `200` | identical bytes already published (SHA-256 idempotency) — no-op |
| `400` | validation: `corpus.mode:"linked"`, unsafe paths, dotfiles, >50 MB/file, >100 MB total, bad `distribution` value |
| `403` | bearer lacks `agents:write` |

The `202` body returns the translated manifest, `sha256`, `sealed`
flag, and a `knowledge_base` report when a corpus is present.

### List

```bash
curl -H "Authorization: Bearer $KEY" https://<your-vera>/v1/agents
```
Every entry carries bundle metadata: `has_bundle`, `bundle_size`,
`bundle_sha256`, `bundle_schema_version`, `bundle_sealed`.

### Download / delete (publisher-only)

```bash
curl -H "Authorization: Bearer $PUBLISHER_KEY" -o my-agent.bot.tar.gz \
  https://<your-vera>/v1/agents/my-agent/bundle          # byte-identical round-trip
curl -X DELETE -H "Authorization: Bearer $PUBLISHER_KEY" \
  https://<your-vera>/v1/agents/my-agent/bundle           # 204; manifest remains
```

Sealed bundles return `403 {"error":"sealed"}` on download — for
everyone, including the publisher (keep your source of truth outside
Vera).

### Manifest-only CRUD (no bundle)

`POST /v1/agents` (JSON manifest), `PUT /v1/agents/{id}`,
`DELETE /v1/agents/{id}` — same `agents:write` gate. Static agents
(shipped on disk) cannot be deleted via API (`409`).

## 3. Invoking an agent

### REST — one shot or sessioned

```bash
curl -X POST https://<your-vera>/v1/agent/my-agent \
  -H "Authorization: Bearer $KEY" \
  -H "X-Vera-Session: ticket-4821" \
  -d "Summarize the refund policy for an angry customer"
```

Response: `{"answer": "...", "steps": [...], "iterations": N}` — the
`steps` array is the full tool-call trace. `X-Vera-Session` (optional,
any opaque string ≤256B) gives the agent memory across calls.

### WebSocket — streaming

`GET /v1/agent/my-agent/stream` upgrades to a WS session: auth + policy
at upgrade, per-message QoS + vault, audit on close.

### MCP — agents as tools

Bundles with `"exposeMcp": true` surface as tool `agent_<id>` on Vera's
MCP server (`POST /mcp`, Streamable HTTP). Claude Code, Cursor, and
Claude Desktop can then call your agent like any tool, with Vera's
pipeline underneath.

## 4. What the agent can do (and can't)

- The driving LLM and every tool are **connectors** — the agent can
  only call tools in its manifest's allowlist, and the *caller's* ACL
  must also cover them. Two gates, both enforced server-side.
- Iterations are capped (`limits.max_iterations`), calls per step are
  capped, and a server-authoritative per-trace ceiling backstops
  runaway loops — an agent cannot spend unbounded tokens.
- Every LLM call and tool call inside a run is a separate audit event
  carrying `agent_id`, latency, tokens, and vault verdict — the ops
  console's Agents view aggregates them per agent.

## 5. Confidential RAG agents — sealed bundles

The pattern: a company-wide policy agent should answer questions about
documents **without ever surrendering the documents**.

```json
{ "version": 2, "kind": "veya-bot", "name": "Policy Assistant",
  "instructions_file": "instructions.md",
  "corpus": { "mode": "owned" },
  "distribution": "sealed" }
```

- Consumers invoke it like any agent; answers flow normally.
- `GET /v1/agents/policy/bundle` → `403 sealed` for **all** principals.
  The archive bytes never leave disk after publish.
- The flag is validated at publish: anything other than `"open"` /
  `"sealed"` is rejected with `400` (a typo must never silently expose
  a corpus).
- Listing shows `"bundle_sealed": true` so the state is discoverable.

## 6. Corpus / RAG status

Corpus files are vault-scanned at publish (PII in documents blocks the
publish in `block` mode) and stored under the agent. The `202` response
includes a `knowledge_base` report; automatic indexing of corpus into a
queryable KB is the active roadmap item — today the corpus rides the
bundle and the agent's instructions, while registered Knowledge Bases
(`/v1/knowledge_bases`, e.g. Bedrock KB) serve retrieval.

## 7. Audit & compliance

Per agent run you can answer, from the chain: who invoked it, which
tools fired, what the vault verdict was on every body, per-stage
latency, token counts, and cost. `GET /admin/usage?group_by=agent` and
the ops console's Agents + Traces views are built on exactly this.
