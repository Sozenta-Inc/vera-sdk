# Hosted Models on Vera

Vera serves models in one of two roles per deployment — understanding
which role your Vera plays tells you exactly what `/v1/models` returns
and how to invoke things.

| Role | Who owns the models | `/v1/models` returns | Typical deployment |
|---|---|---|---|
| **Provider** (this doc) | Vera — its connectors translate to backends | Vera's connector manifests (capabilities, transport) | Cloud/VPC Vera with Bedrock |
| **Proxy** ([GUIDE-PROXY.md](GUIDE-PROXY.md)) | An upstream (Ollama, Anthropic…) | the upstream's live model list (`models_from_proxy = true`) | Metal/edge Vera |

In provider role, every model call passes the full pipeline —
authenticate → policy ACL → PII vault → QoS → dispatch → audit — and
the connector runs **sandboxed in wasm** with credentials held by the
host (SigV4 signing happens outside the sandbox; the connector never
sees keys).

---

## 1. Discovery

```bash
curl -H "Authorization: Bearer $KEY" https://<your-vera>/v1/models
```

Returns one manifest per connector **filtered by your ACL** — you only
see what you can call. Each entry:

```json
{ "id": "bedrock-claude", "display_name": "Bedrock Claude",
  "provider": "bedrock", "model": "claude-haiku-4-5",
  "capabilities": { "input": "text", "output": "text",
                    "modalities": ["text-to-text"], "streaming_response": false },
  "transport": { "modes": ["post"], "post": { "path": "/v1/infer/bedrock-claude" } },
  "healthy": true }
```

`GET /v1/services` is the wider index (models, tools, agents, KBs,
MCP). `GET /llms.txt` is the human/LLM-readable routing manifest
generated from live config — including model aliases.

## 2. Invocation

### Text models — `POST /v1/infer/{connector-or-alias}`

```bash
# alias "claude" → bedrock-claude; per-call model selection in the body:
curl -X POST https://<your-vera>/v1/infer/claude \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{ "model": "sonnet", "max_tokens": 512,
        "messages": [{"role":"user","content":"three bullet points on PII risk"}] }'
```

- `model`: `haiku` (default) | `sonnet` | `opus`, or any full Bedrock
  inference-profile id verbatim.
- Aliases come from the operator's `[[models]]` config (`claude` →
  `bedrock-claude`); apps decouple from backend names.
- Tool use: pass Anthropic-style `tools`; the response carries
  `tool_use` blocks for your client loop.

### Image models — `POST /v1/infer/stability-image`

```bash
curl -X POST https://<your-vera>/v1/infer/stability-image \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"prompt":"isometric data center, blueprint style","model":"stable-image-core"}'
```
One connector fronts the Stability family on Bedrock (core / ultra /
sd3.5 / inpaint / outpaint / upscale / style…) — variant picked per
call via `model`.

### RAG retrieve — `POST /v1/infer/bedrock-kb`

Signs a Bedrock Knowledge-Base `Retrieve` with the host's IAM role;
pair with `/v1/knowledge_bases` registration and `kb_search_<id>` MCP
tools.

### Streaming — `GET /v1/stream/{connector}`

Where the operator configured a streaming upstream, the WS bridge
authenticates + authorizes at upgrade and audits the session.

## 3. What every call gets (the point of hosting models behind Vera)

| Stage | Effect |
|---|---|
| Auth | bearer → principal; no anonymous dispatch |
| Policy ACL | per-principal connector allowlist — discovery AND dispatch both filtered |
| Vault | PII scan of the body (block / mask / observe per principal); Tier-2 NER where deployed |
| QoS | token buckets per principal, per (principal, connector), per upstream host |
| Dispatch | wasm-sandboxed connector; host-side credential signing |
| Audit | hash-chained event: latency, stage timings, tokens in/out, vault verdict, redacted bodies |

Token usage reported by the backend is captured per request, so
`GET /admin/usage` (and the ops console) shows per-customer / per-model
tokens and **cost** — priced by the operator's `[[cost]]` rules.

## 4. Client-credential model

Clients hold exactly **one** secret: their Vera bearer. AWS/provider
credentials live host-side (IAM task role, SigV4 at egress) and never
reach clients or the wasm sandbox. Revoking a bearer revokes
everything; rate limits and spend attribution follow the bearer's
principal.

## 5. Operating notes

- Health per connector rides `/v1/models` (`healthy`) and `/admin/status`.
- Hot-reload: dropping a new connector `.wasm` + `POST
  /admin/connectors/reload` swaps the registry without restart.
- Fallback chains (`[[fallback_chains]]`) retry an ordered connector
  list on failure — the pipeline runs once, only dispatch retries.
