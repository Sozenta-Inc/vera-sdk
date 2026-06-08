# Guide: Vera Agents — Create, Update, List, Delete

How to manage agents on a running Vera over HTTP. Agents are
multi-step LLM-tool loops; this guide is for adding/curating them at
runtime. For the runtime semantics (how `POST /v1/agent/{id}` executes
an agent), see the [SDK README §Agents](README.md#agents).

Companion to [GUIDE-SEARCH.md](GUIDE-SEARCH.md), [GUIDE-IMAGE.md](GUIDE-IMAGE.md),
and [GUIDE-STREAMING-LOCAL.md](GUIDE-STREAMING-LOCAL.md).

## TL;DR

```bash
# Create an agent
curl -X POST https://vera.sozenta.ai/v1/agents \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "support",
    "display_name": "Customer Support Agent",
    "description": "Answers customer questions from FAQ docs",
    "llm": { "connector": "bedrock-claude" },
    "tools": { "allowed": ["echo", "bedrock-claude"] },
    "limits": { "max_iterations": 10, "timeout_seconds": 120, "max_tool_calls_per_step": 3 },
    "prompt": { "system": "You are a helpful customer support agent. Be concise and cite sources." },
    "expose_mcp": false
  }'

# Use it
curl -X POST https://vera.sozenta.ai/v1/agent/support \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -d "How do I reset my password?"
```

## Static vs dynamic

Vera tracks two kinds of agents:

| Source | Origin | Editable via API? | Notes |
|---|---|---|---|
| `static` | TOML files in the Vera image's `agents/` dir | **No** (GET allowed; PUT/DELETE returns 409) | Baked-in operator agents (`admin`, `assistant`, `compliance`) |
| `dynamic` | `POST/PUT /v1/agents` | **Yes** | Persisted in redb; survives restart; hot-swapped without reload |

Dynamic agents override static ones with the same id, **except** when
the static agent has been baked in — in that case the API refuses to
write (returns 409). Pick a fresh id for new agents.

## Authentication and ACL

All endpoints require a Vera bearer. Write operations (POST/PUT/DELETE)
additionally require the synthetic `agents:write` connector ACL —
granted to `veya` by default, denied to demo principals.

```bash
# veya bearer — can manage agents
curl -X POST ... -H "Authorization: Bearer $VEYA_BEARER" .../v1/agents

# app/app-mask/app-trusted bearer — 403 forbidden on writes
curl -X POST ... -H "Authorization: Bearer $DEMO_BEARER" .../v1/agents
# → 403 {"error":"forbidden","message":"principal not authorized for agents:write"}
```

To grant a different principal write access, add `"agents:write"` to
their `connectors` list in `policy-global.toml`.

## Request body (`AgentManifest`)

```ts
type AgentManifest = {
  /** Stable id. ASCII alphanumeric/-/_, ≤ 64 chars. Required. */
  id: string;

  /** Human-readable name. Required. */
  display_name: string;

  /** Free-text description shown in /v1/agents and /llms.txt. */
  description?: string;

  llm: {
    /** Connector id or model alias the agent calls in its inner loop. */
    connector: string;
  };

  tools: {
    /** Connector ids the agent is allowed to dispatch. Validated
     *  against the live connector registry — references to unknown
     *  ids are rejected with 400. The synthetic "agents:write" ACL
     *  is NOT a valid entry (it's a control-plane gate). */
    allowed: string[];
  };

  limits: {
    /** Max LLM → tool → LLM iterations. 1..=100. Default 10. */
    max_iterations?: number;
    /** Overall wall-clock timeout per run. 1..=3600. Default 120. */
    timeout_seconds?: number;
    /** Max parallel tool calls per LLM step. 1..=20. Default 3. */
    max_tool_calls_per_step?: number;
  };

  prompt: {
    /** System message prepended to every run. Required, non-empty. */
    system: string;
  };

  /** Surface this agent as an MCP tool. Default false. Exposure
   *  honored once /mcp ships (a future cycle). Mirrors the explicit
   *  curation pattern used by Glean, Salesforce, Cloudflare. */
  expose_mcp?: boolean;
};
```

## Endpoints

| Method | Path | Purpose | ACL | Status |
|---|---|---|---|---|
| GET    | `/v1/agents`           | List all agents (static + dynamic) | bearer only | 200 / 401 |
| POST   | `/v1/agents`           | Create dynamic agent (409 if id exists) | `agents:write` | 201 / 400 / 403 / 409 |
| GET    | `/v1/agents/{id}`      | Get one manifest | bearer only | 200 / 401 / 404 |
| PUT    | `/v1/agents/{id}`      | Create-or-replace dynamic agent | `agents:write` | 200 / 201 / 400 / 403 / 409 |
| DELETE | `/v1/agents/{id}`      | Remove dynamic agent | `agents:write` | 204 / 403 / 404 / 409 |

## Errors

| Status | Error code | Cause |
|---|---|---|
| 400 | `bad_request` | Body isn't valid JSON; URL id doesn't match body id |
| 400 | `validation_failed` | Manifest fails validation (missing id/display_name/system, bad charset, unknown connector in `tools.allowed`, limits out of range). Response body has `issues[]` listing every problem found. |
| 401 | — | Missing/invalid bearer |
| 403 | `forbidden` | Principal lacks `agents:write` ACL |
| 404 | — | No agent with that id |
| 409 | `conflict` | (POST) agent already exists; (PUT/DELETE) target is static |
| 500 | — | Persistence layer error |

Validation error body example:

```json
{
  "error": "validation_failed",
  "issues": [
    "tools.allowed references unknown connector 'web-search'",
    "limits.max_iterations must be in 1..=100"
  ]
}
```

Fix all listed issues and resubmit.

## Examples

### Create + run

```bash
# 1. Create a "release notes summarizer" agent
curl -X POST https://vera.sozenta.ai/v1/agents \
  -H "Authorization: Bearer $VEYA" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "release-notes",
    "display_name": "Release Notes Summarizer",
    "description": "Distills a long changelog into a 5-bullet summary",
    "llm": { "connector": "bedrock-claude" },
    "tools": { "allowed": ["echo", "bedrock-claude"] },
    "limits": { "max_iterations": 4, "timeout_seconds": 60 },
    "prompt": { "system": "Summarize the changelog the user provides as exactly five bullets. Highlight breaking changes first." }
  }'

# 2. Use it
curl -X POST https://vera.sozenta.ai/v1/agent/release-notes \
  -H "Authorization: Bearer $VEYA" \
  -d "$(cat CHANGELOG.md)"
```

### Update

```bash
curl -X PUT https://vera.sozenta.ai/v1/agents/release-notes \
  -H "Authorization: Bearer $VEYA" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "release-notes",
    "display_name": "Release Notes Summarizer (v2)",
    "description": "Distills a long changelog into a 5-bullet summary",
    "llm": { "connector": "bedrock-claude" },
    "tools": { "allowed": ["echo", "bedrock-claude"] },
    "limits": { "max_iterations": 6, "timeout_seconds": 90 },
    "prompt": { "system": "Summarize as 5 bullets. Lead with breaking changes. Cite the PR number for each bullet if visible in the text." }
  }'
```

### Delete

```bash
curl -X DELETE https://vera.sozenta.ai/v1/agents/release-notes \
  -H "Authorization: Bearer $VEYA"
# → 204 No Content
```

### List + filter to dynamic only

```bash
curl -sk -H "Authorization: Bearer $VEYA" https://vera.sozenta.ai/v1/agents \
  | jq '[.[] | select(.source == "dynamic")]'
```

## Persistence and durability

Dynamic agents are persisted in a redb table (`dynamic_agents`) inside
the Vera data directory (`/vera/data/agents.redb` in the container).
They survive process restart. The static set is re-loaded from
`agents/*.toml` on every boot; the dynamic set is merged on top of it.

If you wipe the data directory you lose dynamic agents (along with the
keystore and audit chain — same blast radius).

## Discovery

Dynamic agents show up everywhere static agents do:

- `GET /v1/agents` — every agent with `source: "static" | "dynamic"`
- `GET /v1/services` — `agents` category count includes both
- `GET /llms.txt` — the Agents table lists every agent with its source
- (future) `/mcp` `tools/list` — agents with `expose_mcp = true`
  surface as MCP tools, filtered by the caller's ACL

## Patterns

### Snapshot-then-edit

```bash
curl -sk -H "Authorization: Bearer $VEYA" \
  https://vera.sozenta.ai/v1/agents/my-agent \
  | jq '.prompt.system = "new system prompt"' \
  | curl -X PUT -H "Authorization: Bearer $VEYA" \
      -H "Content-Type: application/json" \
      --data-binary @- \
      https://vera.sozenta.ai/v1/agents/my-agent
```

### Per-team agent provisioning (GitOps)

Treat the manifest list as code: keep `agents/*.json` in a repo, run a
CI job on merge that `PUT`s each file. Validation is server-side, so a
broken manifest fails CI (400) rather than corrupting Vera.

### Promote a dynamic agent to static

If a dynamic agent has stabilized and you want it baked into the
image: write its manifest to `agents/<id>.toml`, rebuild the image,
then `DELETE /v1/agents/<id>` to remove the redb copy. The static
version takes over on the next deploy.

---

## Related guides

- [GUIDE-SEARCH.md](GUIDE-SEARCH.md) — web search backends
- [GUIDE-IMAGE.md](GUIDE-IMAGE.md) — image generation
- [GUIDE-STREAMING-LOCAL.md](GUIDE-STREAMING-LOCAL.md) — WebSocket streaming
- [README.md](README.md) — top-level SDK quickstart
