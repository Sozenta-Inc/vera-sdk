# Guide: Vera Search

Agent-friendly web search through Vera, with swappable backends. The
public API stays constant across providers — write your client once
against `POST /v1/search`, switch backend (Anthropic, Perplexity,
Brave, future sovereign providers) via Vera config without touching
caller code.

Companion to [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) and
[GUIDE-STREAMING-LOCAL.md](GUIDE-STREAMING-LOCAL.md).

## TL;DR

```bash
curl -X POST https://localhost:8443/v1/search \
  -H "Authorization: Bearer $VERA_BEARER" \
  -H "Content-Type: application/json" \
  -d '{"query": "What is the NVIDIA Blackwell GPU release date?"}'
```

Response:

```json
{
  "query": "What is the NVIDIA Blackwell GPU release date?",
  "results": [
    {
      "title": "...",
      "url": "https://...",
      "snippet": "..."
    },
    ...
  ],
  "synthesized_answer": "The NVIDIA Blackwell B200 GPU was announced ...",
  "backend": "anthropic",
  "cached": false,
  "latency_ms": 4320,
  "request_id": "..."
}
```

## What this is — and isn't

**Is:** a Vera-native HTTP endpoint that takes a query and returns
ranked search results plus an optional LLM-synthesized answer. The
backend doing the actual searching is opaque to your client; Vera
routes to whichever provider is configured.

**Is not:** scraping, browser automation, or RAG over your own
documents. For RAG over your own corpus, build that on top of `/v1/search`
(retrieve from your docs first, then optionally use search to fill
gaps) or use a separate vector-store endpoint we'll document later.

## Why use Vera Search over hitting the search vendor directly

1. **Vendor neutrality.** Your client only knows about `vera.sozenta.ai`.
   We can swap from Anthropic to Perplexity to a sovereign in-region
   provider without your client doing anything.
2. **One bearer, all surfaces.** Same `Authorization` header that gets
   you `/v1/chat/completions` gets you `/v1/search`.
3. **Vault and policy still apply.** Queries flow through Vera's
   per-principal vault and ACL. A vault-block principal can't
   accidentally search the web with a customer's SSN in the query.
4. **Audit chain.** Every search is logged with a query hash + result
   count + backend + latency. Tamper-evident, like every other Vera
   request.
5. **Cost protection.** Search backends charge per call (Anthropic
   web_search is ~$0.02/call). Per-principal QoS limits prevent
   runaway agents from burning budget.

## Authentication

Standard Vera bearer:

```
Authorization: Bearer <token>
```

The token's principal must have `search` in its connectors ACL.
Default policy in the Vera image grants this to the `veya` principal
only — demo principals (`app`, `app-mask`, `app-trusted`) get 403 on
`/v1/search` to keep our Anthropic budget safe from public traffic.

```bash
# Internal app — search allowed
curl ... -H "Authorization: Bearer $VEYA_BEARER" .../v1/search

# Demo bearer — 403 forbidden
curl ... -H "Authorization: Bearer $DEMO_BEARER" .../v1/search
```

To enable search for additional principals, add `"search"` to their
connectors list in `policy-global.toml`.

## Request

```ts
type SearchRequest = {
  /** Required. The query string. <= 4096 chars. */
  query: string;

  /** Hint about WHAT KIND of search.
   *  - 'integrated' (default): backend does search + LLM synthesis,
   *    returns `synthesized_answer` + supporting `results`.
   *  - 'raw': backend returns search results only, no synthesis.
   *    Cheaper / faster — agents do their own LLM pass with the
   *    results in-context.
   *  Note: some backends (Anthropic) always synthesize. Asking for
   *  raw mode there just suppresses `synthesized_answer` in the
   *  response — Anthropic still does the underlying call. */
  mode?: 'integrated' | 'raw';

  /** Cap on results. Default 10. Hard ceiling 50. */
  max_results?: number;

  /** Freshness window. Backends that don't support it ignore. */
  freshness?: 'day' | 'week' | 'month' | 'year' | 'any';

  /** Region (ISO 3166-1 alpha-2, e.g. 'us', 'fr', 'in'). */
  region?: string;

  /** Language (ISO 639-1, e.g. 'en', 'fr'). */
  language?: string;

  /** Source-type filter. Best-effort; not all backends classify. */
  source_type?: 'web' | 'news' | 'academic' | 'docs';

  /** Override Vera's default backend pick. List options via
   *  GET /v1/search/providers. */
  preferred_backend?: string;
};
```

Minimum:

```bash
curl -X POST https://localhost:8443/v1/search \
  -H "Authorization: Bearer $VERA_BEARER" \
  -H "Content-Type: application/json" \
  -d '{"query":"What is QUIC and how does it compare to TCP?"}'
```

## Response

```ts
type SearchResponse = {
  /** Echoed from request for convenience. */
  query: string;

  /** Ordered results. May be empty. */
  results: Array<{
    title: string;
    snippet: string;
    url: string;
    published_at?: string;      // ISO 8601 when backend exposes it
    score?: number;             // 0-1, backend-defined
    source_type?: 'web' | 'news' | 'academic' | 'docs';
  }>;

  /** Synthesized answer (integrated-mode backends only). */
  synthesized_answer?: string;

  /** Indices into `results` cited by the synthesized answer.
   *  Optional; not all backends expose citation indices. */
  citations_for_answer?: number[];

  /** Backend id that served the request. */
  backend: string;

  /** Always false in v1; reserved for future cache layer. */
  cached: boolean;

  /** Wall-clock latency for the backend call (ms). */
  latency_ms: number;

  /** Vera-issued correlation id. Use to join against /admin/audit. */
  request_id: string;
};
```

## Errors

| Status | Code | Meaning |
|---|---|---|
| 400 | `missing_query` | `query` empty or missing |
| 400 | `query_too_long` | `query` exceeds 4096 bytes |
| 401 | (no body) | Missing/invalid bearer |
| 403 | `forbidden` | Principal lacks `search` connector ACL |
| 502 | `backend_error` | Upstream search provider failed |
| 503 | `no_backend` | No backend configured / ANTHROPIC_API_KEY missing |
| 504 | `backend_timeout` | Backend took >30s |

Error body shape:

```json
{
  "error": "backend_error",
  "message": "HTTP 429: rate limit exceeded",
  "request_id": "..."
}
```

## Discovery — finding available backends

```bash
curl -H "Authorization: Bearer $VERA_BEARER" \
  https://localhost:8443/v1/search/providers
```

Returns the list of configured backends + their modes + cost estimates:

```json
[
  {
    "id": "anthropic",
    "supports": ["integrated"],
    "estimated_cost_per_1k_usd": 13.0,
    "is_default": true
  }
]
```

`/v1/search` also surfaces in `/v1/services` (top-level service
inventory) and `/llms.txt` (machine-readable manifest of the running
Vera) — so agents can discover that search is available without
prior knowledge.

## Using search as an agent tool

If your agent is an LLM with tool-calling support (most modern ones),
expose `vera_search` as a tool:

```python
tools = [{
    "name": "vera_search",
    "description": "Search the web for current information. Returns ranked results and an optional synthesized answer.",
    "input_schema": {
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Search query"},
            "mode": {"type": "string", "enum": ["integrated", "raw"]},
        },
        "required": ["query"]
    }
}]

# When the agent calls vera_search(query=...), your tool runner
# forwards it to Vera:
def call_vera_search(query, mode="integrated"):
    import requests
    r = requests.post(
        "https://vera.sozenta.ai/v1/search",
        headers={"Authorization": f"Bearer {VERA_BEARER}"},
        json={"query": query, "mode": mode},
        timeout=35,
    )
    r.raise_for_status()
    return r.json()
```

In `integrated` mode the response already contains a synthesized
answer — pass it back to the agent as the tool result and the agent
gets both the answer and the citations.

In `raw` mode the agent gets results without synthesis and is
responsible for whatever next step (summarize, follow-up search,
combine with internal docs, etc.). Cheaper if you're paying per
synthesis and don't want it.

## Cost model

Cost depends on backend. As of writing, the default backend
(Anthropic via Anthropic API direct) costs **~$0.02 per search**:
$10 per 1k searches for the web_search tool plus ~$3-5/1k for Claude
tokens (Haiku 4.5 default).

The cost is on the Vera operator's account, not yours. We absorb it
in the demo / free tier; higher-volume customers will eventually meter
search separately (not in v1 — billing pass-through is on the roadmap).

To stay inside our budget, the `search` connector ACL gates which
principals can call this endpoint. Only `veya` and operator-blessed
principals get search by default; demo principals are 403'd.

## Operational notes (for SDK readers building production clients)

**Backoff on 5xx.** If Vera returns 502/503/504, the backend is having
a bad time. Exponential backoff (1s, 2s, 4s up to 30s) before retrying.

**Don't retry on 4xx.** A 400 means your request is malformed; retrying
won't help. A 403 means policy denies you; retrying won't help.

**Honor `Retry-After`.** Future versions will return 429 with
`Retry-After` when rate limits kick in. Respect it.

**Audit your queries.** Vera logs a BLAKE3 hash of each query
(not plaintext) for cache hit-rate analysis and abuse detection.
If your client needs to correlate Vera's audit chain with your own
request logs, use the `request_id` Vera returns and the principal
id from your bearer.

**Web_search returns can be slow.** Typical: 3-8s. Hard timeout: 30s
(Vera returns 504). Your client timeout should be >35s to allow
Vera's own timeout to fire first.

## Switching backends later

If we add Perplexity as a backend, your client doesn't change unless
you want to:

- Default keeps working — Vera's default backend selection serves your
  requests transparently.
- Per-request override: pass `"preferred_backend": "perplexity"` in
  the request to force a specific backend.

Backends to expect in future Vera releases (rough order of priority):

| Backend | Mode | Strength |
|---|---|---|
| `anthropic` (default today) | integrated | High-quality synthesis, expensive |
| `perplexity` | integrated | Alternative integrated mode |
| `brave` | raw | Cheap, no synthesis, fast |
| `tavily` | raw + integrated | Agent-optimized |
| In-region sovereign | varies | EU / India / Saudi sovereign deployments |

## Limits in v1

These are explicit non-features in v1:

- **No caching.** Every search hits the backend. (Adding cache is a
  separate cycle; the response shape already has `cached: bool` for
  forward-compat.)
- **No streaming.** Response is one JSON body. Anthropic supports
  streaming, but agent UX is fine without it for now.
- **No fallback.** If the configured backend fails, you get a 502.
  Multi-backend fallback ships when we add backend #2.
- **No per-request billing.** Cost protection is via Vera's per-principal
  QoS and the `search` connector ACL.
- **No request body for `safety_filter`, `image_url`, etc.** Add when
  customers ask.

## Quickstart for new SDK users

```bash
# 1. Get a bearer with search enabled.
#    Local hybrid stack:
KEY=$(docker exec docker-vera-hub-1 cat /vera/data/demo-key-veya.txt)

#    Production:
#    grab the veya bearer from https://vera.sozenta.ai/ (landing page)

# 2. Verify search is available
curl -sk -H "Authorization: Bearer $KEY" \
  https://localhost:8443/v1/search/providers

# 3. Run a search
curl -sk -X POST \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: application/json" \
  -d '{"query":"What is the latest Claude model?"}' \
  https://localhost:8443/v1/search | jq .
```

If `/v1/search/providers` returns `[]`, the backend isn't configured
yet — likely `ANTHROPIC_API_KEY` env isn't set in the Vera deployment.
Local hybrid: add `ANTHROPIC_API_KEY` to your shell before
`docker compose up`. Production: contact the Vera operator.

---

## Related guides

- [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) — STT → LLM → TTS pipelines
- [GUIDE-STREAMING-LOCAL.md](GUIDE-STREAMING-LOCAL.md) — WebSocket streaming endpoints
- [README.md](README.md) — top-level SDK quickstart
