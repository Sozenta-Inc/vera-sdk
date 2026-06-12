# Guide: Vera Agent Bundles

Publish portable agent recipes — system prompt, configuration, and
(optionally) the RAG corpus — as a single `.bot.tar.gz` file. Anyone
can build a bundle that Vera installs. Format spec:
[**FORMAT-bot-bundle.md**](FORMAT-bot-bundle.md).

Companion to [GUIDE-AGENTS.md](GUIDE-AGENTS.md) (REST CRUD on
manifests), [GUIDE-RAG.md](GUIDE-RAG.md) (knowledge bases), and
[GUIDE-MCP.md](GUIDE-MCP.md) (MCP tool surface).

## TL;DR

```bash
# Publish (or update — idempotent on SHA256)
curl -X PUT https://vera.sozenta.ai/v1/agents/compliance/bundle \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -H "Content-Type: application/gzip" \
  --data-binary @compliance.bot.tar.gz

# Download
curl -X GET https://vera.sozenta.ai/v1/agents/compliance/bundle \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -o compliance.bot.tar.gz

# Remove the bundle (manifest stays — agent remains callable)
curl -X DELETE https://vera.sozenta.ai/v1/agents/compliance/bundle \
  -H "Authorization: Bearer $VEYA_BEARER"
```

## Two roles, two access tiers

| Role | What they do | ACL |
|---|---|---|
| **Publisher** (small team — CI tokens, AI/platform lead) | `PUT` / `GET` / `DELETE` bundles | `agents:write` |
| **User** (everyone with a bearer) | `POST /v1/agent/{id}` — invoke the published bot | bearer only |

Same ACL as cycle 1's agent CRUD. No new permission name. The veya
principal has `agents:write` by default; demo principals don't.

## Sealed bundles (confidential corpora)

Add `"distribution": "sealed"` to `bot.json` and the published bundle
becomes a **write-only artifact**: everyone with access can still
*invoke* the agent, but `GET /v1/agents/{id}/bundle` returns
`403 {"error":"sealed"}` for all principals — publisher included. Use
it for company-wide RAG agents whose documents must answer questions
without ever being extractable. Keep your source of truth outside Vera;
re-publishing (PUT) and deleting still work. Listings show
`"bundle_sealed": true`. See [GUIDE-HOSTED-AGENTS.md §5](GUIDE-HOSTED-AGENTS.md).

## Build a bundle (anyone can)

The format is documented in [FORMAT-bot-bundle.md](FORMAT-bot-bundle.md).
Minimal layout:

```
my-bot/
├── bot.json         (required)
└── instructions.md  (required)
```

`bot.json`:
```json
{
  "version": 2,
  "kind": "veya-bot",
  "name": "My Bot",
  "instructions_file": "instructions.md",
  "exposeMcp": false
}
```

Pack:
```bash
tar -czf my-bot.bot.tar.gz my-bot/
```

That's a complete prompt-only bundle ready to publish.

### With RAG corpus

```
my-bot/
├── bot.json
├── instructions.md
└── corpus/
    ├── policies/
    │   └── data-handling.md
    └── runbooks.md
```

Set `bot.json`:
```json
{
  "version": 2,
  "kind": "veya-bot",
  "name": "My Bot",
  "instructions_file": "instructions.md",
  "corpus": { "mode": "owned" },
  "exposeMcp": false
}
```

> **Cycle 7 caveat:** Vera v1 stores the corpus on disk but does NOT
> yet index it for retrieval. The bot is callable today and works with
> the system prompt; corpus-grounded answers go live in cycle 7.5
> (sqlite-vec + Bedrock Titan embeddings). The bundle wire format is
> stable — your `.bot.tar.gz` won't need to change.

## Endpoints

| Method | Path | ACL | Status |
|---|---|---|---|
| PUT    | `/v1/agents/{id}/bundle`   | `agents:write` | 202 / 200 (idempotent) / 400 / 403 / 413 |
| GET    | `/v1/agents/{id}/bundle`   | `agents:write` | 200 (application/gzip) / 401 / 403 / 404 |
| DELETE | `/v1/agents/{id}/bundle`   | `agents:write` | 204 / 401 / 403 / 404 |

`GET /v1/agents` also gained per-entry fields:
- `has_bundle: bool`
- `bundle_size: int`
- `bundle_sha256: string`
- `bundle_schema_version: int`

Use these to decide whether to download.

## PUT response shape

```ts
type PublishResult = {
  id: string;
  name: string;                  // from bot.json
  schema_version: number;        // currently 2
  size: number;                  // compressed byte count
  sha256: string;                // hex digest — use for idempotency
  file_count: number;            // files inside the tar (sans dirs)
  has_corpus: boolean;           // bundle ships a corpus/ folder
  manifest: AgentManifest;       // the translated Vera manifest
  knowledge_base?: {             // present iff has_corpus
    id: string;                  // "<agent_id>-kb"
    backend: "sqlite-vec";
    status: "deferred";          // cycle 7 v1; cycle 7.5 = "ready"
    note: string;
    corpus_file_count: number;
  };
  idempotent_noop: boolean;      // true if PUT'd the exact same bytes again
};
```

Status semantics:
- `202 Accepted` — bundle ingested, manifest upserted, KB ingestion
  is "deferred" pending cycle 7.5
- `200 OK` with `idempotent_noop: true` — same bytes, already installed

## Errors

| Status | Code | Cause |
|---|---|---|
| 400 | `bad_request` | Agent id invalid; body empty; bot.json parse error |
| 400 | `validation_failed` | Missing required file; `kind` mismatch; `corpus.mode: "linked"` (must normalize to `owned` before publish); allowlist failure; path traversal |
| 401 | — | Missing/invalid bearer |
| 403 | `forbidden` | Principal lacks `agents:write` ACL |
| 404 | `not_found` | No bundle for this agent (GET / DELETE) |
| 413 | `payload_too_large` | Compressed body > 100 MB, or any uncompressed file > 50 MB, or total uncompressed > 100 MB |

## Idempotency

PUTting the same `.bot.tar.gz` twice is safe. The server SHA256s the
body and short-circuits: returns `200 OK` with `idempotent_noop: true`
without re-extracting. This makes CI publish loops cheap — re-run them
on every build, only changed bundles incur work.

Different bytes for the same `{id}` → atomic replace (the staging
directory swap means an interrupted PUT leaves the previous bundle
intact).

## What happens on the server

```
PUT /v1/agents/compliance/bundle
   │
   1. ACL check (agents:write) → 403 if not
   2. Body ≤100 MB compressed
   3. SHA256 → if matches existing, 200 idempotent_noop
   4. Stream-validate tar:
      • allowlist (spec §4)
      • path safety: no /, no .., no dotfiles, no symlinks
      • file ≤50 MB, total ≤100 MB uncompressed
      • bot.json: kind == "veya-bot", version ≤2, name non-empty
      • instructions.md present
      • reject corpus.mode = "linked"
      • vault PII scan on every text file in corpus/
   5. Normalize bot.json: source = null; mode = owned/none
   6. Persist verbatim → data/bundles/compliance/bundle.bot.tar.gz
   7. Extract → data/bundles/compliance/extracted/
   8. Translate bot.json → AgentManifest:
      name → display_name
      instructions.md content → prompt.system
      exposeMcp → expose_mcp
      llm.connector defaults to "bedrock-claude"
      tools.allowed = ["echo", "bedrock-claude"]
   9. agent_store.put() — upserts via cycle 1 plumbing
  10. rebuild_agents_view → ArcSwap (hot-swap; no restart)
  11. Audit-chain entry
  12. Return 202 with the manifest + kb stub
```

A user then `POST /v1/agent/compliance` (no extra ACL beyond bearer)
runs the published bot using cycle 1's agent runtime.

## Publish from CI

```bash
#!/bin/sh
# .github/workflows/publish-bots.sh
set -e
for dir in bots/*/; do
  id=$(basename "$dir")
  tar -czf "${id}.bot.tar.gz" -C "bots" "${id}/"
  curl -fsk -X PUT "https://vera.sozenta.ai/v1/agents/${id}/bundle" \
    -H "Authorization: Bearer ${VERA_BEARER}" \
    -H "Content-Type: application/gzip" \
    --data-binary "@${id}.bot.tar.gz" | jq '.id, .sha256, .idempotent_noop'
done
```

Idempotency makes this safe to run on every push. The `.bot.tar.gz`
files can be committed (small) or built fresh each time (clean).

## Discover what's published

```bash
curl -sk -H "Authorization: Bearer $VEYA_BEARER" \
  https://vera.sozenta.ai/v1/agents \
  | jq '.[] | select(.has_bundle) | {id, display_name, bundle_size, bundle_sha256}'
```

```bash
# Just the bundle metadata for one agent (no body download)
curl -sk -H "Authorization: Bearer $VEYA_BEARER" \
  https://vera.sozenta.ai/v1/agents/compliance \
  | jq '. + (.|getpath(["has_bundle"]))'
```

## Round-trip via download

```bash
# Pull the current bundle
curl -fsk -H "Authorization: Bearer $VEYA_BEARER" \
  https://vera.sozenta.ai/v1/agents/compliance/bundle \
  -o /tmp/compliance.bot.tar.gz

# Inspect it
tar -tzf /tmp/compliance.bot.tar.gz
tar -xOzf /tmp/compliance.bot.tar.gz compliance/bot.json | jq .

# Re-publish to a different Vera (or back to the same one — idempotent)
curl -fsk -X PUT https://other-vera.example.com/v1/agents/compliance/bundle \
  -H "Authorization: Bearer $OTHER_BEARER" \
  -H "Content-Type: application/gzip" \
  --data-binary @/tmp/compliance.bot.tar.gz
```

## Forward path (what's coming)

- **Cycle 7.5: corpus indexing.** sqlite-vec embedded vector store +
  Bedrock Titan v2 embeddings. Bundled corpus auto-indexes on PUT;
  `kb_search_<id>` MCP tool lights up; `/v1/agent/{id}` runs as a real
  RAG agent. No bundle format change.
- **Cycle 7.6: Ed25519 bundle signatures** — same key model as plugin
  signing.
- **Future: cross-Vera distribution registry** — pull bundles by name
  from a central index. Until then, bundles are file-based artifacts
  + a one-shot `PUT` per Vera instance.

## Related guides

- [FORMAT-bot-bundle.md](FORMAT-bot-bundle.md) — the wire-format spec
- [GUIDE-AGENTS.md](GUIDE-AGENTS.md) — REST CRUD on manifests (no bundle)
- [GUIDE-RAG.md](GUIDE-RAG.md) — knowledge bases (the cycle 7.5 target backend)
- [GUIDE-MCP.md](GUIDE-MCP.md) — `agent_<id>` MCP tools (gated by `expose_mcp`)
- [README.md](README.md) — top-level SDK quickstart
