# Guide: Vera RAG — Retrieval-Augmented Generation

Run RAG over your own documents through Vera. The customer's docs and
embeddings live in their own AWS account (Bedrock Knowledge Bases by
default); Vera signs the retrieval call, runs the PII vault on the
returned chunks, audits the request, and routes through the same
ACL + QoS pipeline as everything else.

Companion to [GUIDE-AGENTS.md](GUIDE-AGENTS.md), [GUIDE-SEARCH.md](GUIDE-SEARCH.md),
and [GUIDE-IMAGE.md](GUIDE-IMAGE.md).

## TL;DR

```bash
# 1. Register a Bedrock KB with Vera (one-time)
curl -X POST https://vera.sozenta.ai/v1/knowledge_bases \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "legal-docs",
    "display_name": "Legal Documents",
    "description": "Policies, contracts, SOC2 controls",
    "backend": "bedrock-kb",
    "backend_id": "kb-abcdef0123456789",
    "region": "us-east-2"
  }'

# 2. Full RAG — retrieve + synthesize + cite
curl -X POST https://vera.sozenta.ai/v1/agent/kb-search \
  -H "Authorization: Bearer $VEYA_BEARER" \
  -H "Content-Type: text/plain" \
  --data-raw 'Question: What is our SOC2 audit cadence?
Knowledge base id: kb-abcdef0123456789'
```

## Architecture

```
Client → POST /v1/agent/kb-search    OR    POST /v1/infer/bedrock-kb
            (full RAG)                       (raw chunks, no synthesis)
   ↓
Vera pipeline:
  • Authenticate, ACL check
  • PII vault scan on the query
  • QoS bucket
  • For agent path: LLM decides to call `bedrock-kb` tool
   ↓
bedrock-kb wasm connector:
  • Host signs the call with SigV4
  • POST to bedrock-agent-runtime.{region}.amazonaws.com/knowledgebases/{kb_id}/retrieve
   ↓
Bedrock KB:
  • Embeds the query
  • Searches the customer's vector store (Aurora pgvector / OpenSearch / Pinecone)
  • Returns top-k chunks with content + source URI + score
   ↓
PII vault scan on each returned chunk
   ↓
For agent path: LLM (bedrock-claude) synthesizes a cited answer
   ↓
Audit append: { kb_id, query_hash, chunks_returned, latency, principal }
   ↓
Response → chunks (raw) OR { answer, citations } (agent)
```

**The customer's docs never leave their AWS account.** Vera operates
on the responses — it doesn't host, ingest, or index documents.

## Two ways to query a KB

| Path | Returns | Use when |
|---|---|---|
| `POST /v1/infer/bedrock-kb` | Raw `retrievalResults` envelope from Bedrock | You're composing your own RAG flow (custom rerank, multi-hop, build your own answer) |
| `POST /v1/agent/kb-search` | `{ answer, citations }` — fully synthesized | You want a turnkey "ask my docs" experience |

The agent is built from the same connector — same audit chain, same
vault, same ACL. It's syntactic sugar plus an LLM step.

## Authentication and ACL

- **Calling a KB**: principal needs `bedrock-kb` in its connectors ACL
- **Managing KBs (POST/PUT/DELETE)**: principal needs `kb:write`

Both are granted to `veya` by default and denied to demo principals.

```bash
# veya — works
curl ... -H "Authorization: Bearer $VEYA" .../v1/infer/bedrock-kb

# demo bearer — 403 forbidden
curl ... -H "Authorization: Bearer $DEMO" .../v1/infer/bedrock-kb
```

## Request shape — raw retrieve

```ts
type RetrieveRequest = {
  /** Bedrock KnowledgeBase id (kb-xxxxxxxxxxxx). Required if
   *  BEDROCK_KB_ID env isn't set on the connector. */
  knowledge_base_id: string;

  /** Search query. Required. */
  query: string;

  /** Top-k chunks to return. Default 5, max 100. */
  max_results?: number;

  /** Region of the KB. Default us-east-2 (or BEDROCK_KB_REGION env). */
  region?: string;
};
```

Plain-text body is also accepted (the body becomes the query); use
this when the connector has `BEDROCK_KB_ID` env set so the kb id
doesn't have to be in every call.

## Response shape — raw retrieve

Bedrock's envelope, passed through verbatim:

```ts
type RetrieveResponse = {
  retrievalResults: Array<{
    content: { text: string };
    location: { s3Location?: { uri: string }; type: string };
    score: number;
    metadata?: Record<string, unknown>;
  }>;
};
```

## Full RAG via the `kb-search` agent

The agent's system prompt tells it to call `bedrock-kb` with the kb
id and a search query paraphrased from the user's question, then
synthesize a 2-5 sentence answer that **cites the retrieved chunks**.
Refuses to speculate when retrieval returns nothing.

Request:

```bash
curl -X POST https://vera.sozenta.ai/v1/agent/kb-search \
  -H "Authorization: Bearer $VEYA" \
  -H "Content-Type: text/plain" \
  --data-raw 'Question: What is our incident response SLA?
Knowledge base id: kb-abcdef0123456789'
```

Response: an `AgentResult` JSON with `answer` (the synthesized text
prefixed with `DONE:`) and `steps[]` (the tool calls the agent
made, so you can audit the retrieval).

## Knowledge Base management

| Method | Path | Purpose | ACL | Status |
|---|---|---|---|---|
| GET    | `/v1/knowledge_bases`        | List registered KBs | bearer | 200 / 401 |
| POST   | `/v1/knowledge_bases`        | Register a KB | `kb:write` | 201 / 400 / 403 / 409 |
| GET    | `/v1/knowledge_bases/{id}`   | Get one | bearer | 200 / 401 / 404 |
| PUT    | `/v1/knowledge_bases/{id}`   | Create-or-replace | `kb:write` | 200 / 201 / 400 / 403 |
| DELETE | `/v1/knowledge_bases/{id}`   | Remove | `kb:write` | 204 / 403 / 404 |

### `KnowledgeBase` schema

```ts
type KnowledgeBase = {
  /** Stable friendly id. ASCII alphanumeric + dash + underscore, ≤ 64 chars. */
  id: string;

  /** Human-readable name. Required. */
  display_name: string;

  /** Free-text description. */
  description?: string;

  /** Backend connector id. v1 supports "bedrock-kb"; future versions
   *  will add "pgvector" and "sqlite-vec". */
  backend: "bedrock-kb";

  /** Backend-specific id. For bedrock-kb: the Bedrock KnowledgeBase id
   *  (kb-xxxxxxxxxxxx). */
  backend_id: string;

  /** AWS region of the KB. Optional; defaults to the connector's
   *  configured region (us-east-2). */
  region?: string;
};
```

## Picking a vector store backend (operator setup)

**Vera does not store vectors.** The customer creates a Bedrock
Knowledge Base in their AWS account; that KB owns the vector store.
Choices, ranked for the regulated mid-cap target:

| Backend | Cost (idle) | Sovereignty | Use when |
|---|---|---|---|
| **Aurora PostgreSQL + pgvector** ⭐ | ~$50/mo | High (customer VPC, open-source extension) | **Default recommendation** |
| RDS Postgres + pgvector | ~$15/mo (t3.micro) | Highest | Cheapest sovereign option |
| OpenSearch Serverless | ~$700/mo minimum | Medium | High-QPS workloads |
| Pinecone / MongoDB Atlas | External pricing | **Low — data leaves AWS** | Avoid for regulated customers |

Setup happens once in the AWS Console (Bedrock → Knowledge Bases →
Create). Bedrock handles document chunking + embedding + writes to
the chosen vector store. Vera is invisible to that flow — it only
calls `Retrieve` at query time.

## Errors

| Status | Cause |
|---|---|
| 400 | Missing `knowledge_base_id` or `query`; invalid JSON; validation failed |
| 401 | Missing/invalid bearer |
| 403 | Principal lacks `bedrock-kb` (queries) or `kb:write` (management) ACL |
| 404 | Bedrock returned `ResourceNotFoundException` — KB id doesn't exist or task role lacks permission |
| 502 | Bedrock returned 5xx |
| 504 | Bedrock timeout |

## Ingestion: where Vera is NOT in the loop

Bedrock KB handles ingestion natively — point it at an S3 bucket, it
chunks + embeds + writes the vector store on a schedule. **This
happens out-of-band; Vera does not proxy ingestion.** The implication:

- A document with PII landing in S3 will be indexed without Vera's
  vault seeing it. If you need ingestion-side PII redaction, set up
  an S3 event handler that scans + redacts before Bedrock sync (out
  of scope for v1; planned for a follow-up cycle).
- Audit visibility on ingestion is via CloudTrail (Bedrock-side),
  not Vera's BLAKE3 chain. Vera's chain captures the query side.

## Cycle 6: alternative backends (`pgvector`, `sqlite-vec`)

**Status:** schema reserved + dispatch wired; the connectors
themselves are not yet shipped (separate cycles — each requires a
Postgres or embedded vector client plus an embedding pipeline, real
subsystems on their own). Registering a KB against one of these names
**succeeds**; calling it returns a clear "not yet implemented" error
rather than silently no-op'ing.

The same `KnowledgeBase` schema works against the upcoming backends —
forward-declare your KBs today so callers can write code against the
final wire shape:

```ts
// pgvector — customer runs Postgres in their VPC
{ "id": "internal-runbooks", "backend": "pgvector",
  "backend_id": "postgresql://...", "region": "" }

// sqlite-vec — embedded for laptop / airgap demos
{ "id": "dev-docs", "backend": "sqlite-vec",
  "backend_id": "/vera/data/kbs/dev-docs.sqlite", "region": "" }
```

What the dispatch layer does *today*:

- `POST /v1/knowledge_bases` accepts these backends and persists the
  record (validation pass — they're in `RESERVED_BACKENDS`)
- `GET /v1/knowledge_bases` returns them in the catalog
- `/llms.txt` and `/v1/services` list them
- MCP `tools/list` surfaces `kb_search_<id>` for them **if the caller's
  ACL grants the backend name** (veya has `pgvector` and `sqlite-vec`
  in its connectors list by default)
- MCP `tools/call` on those tools returns:
  `{"error":{"code":-32603,"message":"backend 'pgvector' is reserved but not yet implemented (cycle 6+); register against 'bedrock-kb' for now"}}`

When the connectors land:

- **`pgvector`**: a host-native module (not wasm — Postgres clients
  don't compile to wasm32-wasip2 cleanly) that takes a query, calls
  Bedrock Titan Embeddings to embed, runs SQL against the customer's
  pgvector table with cosine distance. Customer pre-creates the table
  and populates it themselves (no Vera ingestion).
- **`sqlite-vec`**: an embedded vector store using sqlite-vec, with a
  local embedding model (likely Bedrock Titan or a small ONNX model).
  For laptop / airgap demos. Vera handles ingestion in this case.

Both work against the same `kb-search` agent — agents see the
`bedrock-kb`-style request body; the dispatch layer routes by the KB's
`backend` field.

## Quickstart

```bash
# 1. Get a veya bearer
KEY=$(docker exec docker-vera-hub-1 cat /vera/data/demo-key-veya.txt)
#    Production: grab from https://vera.sozenta.ai/

# 2. Create a Bedrock KB in your AWS account (one-time, AWS Console).
#    https://console.aws.amazon.com/bedrock/home → Knowledge Bases → Create
#    Choose Aurora pgvector for the vector store.
#    Sync from an S3 bucket of your docs.
#    Note the kb-xxxxxxxxxxxx id.

# 3. Register the KB with Vera
curl -sk -X POST https://vera.sozenta.ai/v1/knowledge_bases \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: application/json" \
  -d '{"id":"my-kb","display_name":"My KB","backend":"bedrock-kb",
       "backend_id":"kb-xxxxxxxxxxxx","region":"us-east-2"}'

# 4. Ask a question
curl -sk -X POST https://vera.sozenta.ai/v1/agent/kb-search \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: text/plain" \
  --data-raw 'Question: <your question>
Knowledge base id: kb-xxxxxxxxxxxx'
```

---

## Related guides

- [GUIDE-AGENTS.md](GUIDE-AGENTS.md) — agent CRUD (the kb-search agent shape)
- [GUIDE-SEARCH.md](GUIDE-SEARCH.md) — web search (different from RAG)
- [GUIDE-IMAGE.md](GUIDE-IMAGE.md) — image generation
- [README.md](README.md) — top-level SDK quickstart
