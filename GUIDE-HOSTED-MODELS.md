# Hosted Models on Vera

Vera serves models in one of two roles per deployment — understanding
which role your Vera plays tells you exactly what `/v1/models` returns
and how to invoke things.

| Role | Who owns the models | `/v1/models` returns | Typical deployment |
|---|---|---|---|
| **Provider** (this doc) | Vera — its connectors translate to backends | Vera's connector manifests (capabilities, transport) | Cloud/VPC Vera with Bedrock |
| **Proxy** ([GUIDE-PROXY.md](GUIDE-PROXY.md)) | An upstream (Ollama, Anthropic…) | the upstream's live model list (`models_from_proxy = true`) | Metal/edge Vera |

In provider role, every model call passes the full pipeline —
authenticate → policy ACL → PII vault → QoS → dispatch → audit. Most
connectors run **sandboxed in wasm** (SigV4 signing happens outside the
sandbox; the connector never sees keys); a few are **host-side proxy
forwards** to an OpenAI-compatible upstream (e.g. Meta Muse Spark 1.1),
with the upstream API key injected host-side. Either way credentials
stay on the host and never reach the client.

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

### Meta Muse Spark 1.1 — `POST /v1/infer/muse-spark-1.1`

A hosted **text + vision → text** reasoning model (1M-token context).
Vera holds the Meta API key server-side and injects it per call, so —
like every hosted model — clients carry only their Vera bearer. It
appears in `/v1/models` on its own (`id: "meta"`, `model:
"muse-spark-1.1"`) and is called by the `muse-spark-1.1` alias (or the
`meta` connector id).

**Same wire format as Claude.** Spark's connector speaks **Anthropic
Messages** — top-level `system`, `max_tokens`, `messages` in; content
blocks out — so a client already calling `claude` reaches Spark by
**changing only the model name**. No OpenAI-shape translation.

```bash
curl -X POST https://<your-vera>/v1/infer/muse-spark-1.1 \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{ "model": "muse-spark-1.1", "max_tokens": 3000,
        "system": "You are a terse assistant.",
        "messages": [{"role":"user","content":"What is 17 × 23? Reply with only the number."}] }'
```

The response is a `content` array. As a **reasoning** model, Spark
returns its reasoning as an encrypted `redacted_thinking` block next to
the `text` block — **read `text`, ignore `redacted_thinking`** (Claude
parsers already do).

- **Send a generous `max_tokens`.** Reasoning tokens draw from the same
  budget, so a small `max_tokens` can be consumed entirely by reasoning,
  leaving **no `text` block** (`stop_reason: "max_tokens"`) with no
  error. A few thousand is a safe floor for short answers.
- **Streaming (recommended):** send `"stream": true` for Anthropic SSE
  (`message_start` / `content_block_delta` / …). The model can pause for
  seconds emitting reasoning before the first visible token; Vera's
  keep-alive holds the connection open through it.
- **Prompt caching:** Spark honors Anthropic `cache_control` breakpoints
  — a repeated large prefix is read from cache (large input-token
  savings). Vera can also place breakpoints server-side via
  `[[cache.provider_injection]]`, same as `bedrock-claude`.
- **Vision (image → text):** pass an Anthropic image block; Spark answers
  in text. It does **not** generate images — for that use
  `stability-image` (text → image).

```bash
curl -X POST https://<your-vera>/v1/infer/muse-spark-1.1 \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{ "model":"muse-spark-1.1", "max_tokens":3000, "messages":[{"role":"user","content":[
        {"type":"text","text":"What is in this image?"},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"<...>"}}]}] }'
```

Discovery advertises `modalities: ["text-to-text","image-to-text"]`,
`input: "text+image"`, `output: "text"`.

### Meta Muse Spark 1.3 — `POST /v1/infer/muse-spark-1.3`

The newer Spark checkpoint, alongside 1.1 (both are live; 1.1 is
unchanged). Same server-held key, same **Anthropic Messages** wire
format, same reasoning-model caveats as 1.1 above — generous
`max_tokens`, `redacted_thinking` next to `text`, streaming
recommended. It appears in `/v1/models` as `id: "meta-spark-13"`,
`model: "muse-spark-1.3"`.

```bash
curl -X POST https://<your-vera>/v1/infer/muse-spark-1.3 \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{ "model": "muse-spark-1.3", "max_tokens": 3000,
        "messages": [{"role":"user","content":"Summarise this repo in three bullets."}] }'
```

**Wider input than 1.1.** Context is 1,048,576 tokens, and it reads
video and PDF documents in addition to text and images. Discovery
advertises `modalities: ["text-to-text","image-to-text",
"video-to-text","pdf-to-text"]`, `input: "text+image+video+pdf"`,
`output: "text"`.

> **Audio is not offered on 1.3, deliberately.** Meta's model reference
> lists audio with an asterisk, and the footnote is a negative one:
> audio understanding on this checkpoint is *"currently not fully
> supported"*, response quality *"may be degraded"*, and it points you
> at **Muse Spark 1.2** or **Muse Voice Transcribe** instead. Vera
> therefore does not advertise `audio-to-text` for 1.3 — sending audio
> is unsupported, not merely lower quality. For speech-to-text use the
> `moonshine` connector.

### Meta Muse Image 1.0 — `POST /v1/infer/muse-image-1.0`

**Text → image.** A separate connector from Spark (`id: "meta-image"`),
because it is a different API family: Spark posts to Meta's Anthropic
Messages endpoint, while Muse Image is served by Meta's
OpenAI-compatible **images** endpoint. Same server-held Meta key, so
clients still carry only their Vera bearer.

```bash
curl -X POST https://<your-vera>/v1/infer/muse-image-1.0 \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{ "model": "muse-image-1.0", "n": 1,
        "prompt": "a red fox trotting through fresh snow, golden hour" }'
```

**The body is forwarded verbatim**, so send the shape Meta's images
endpoint expects (`model`, `prompt`, `n`, …) — *not* Anthropic
`messages`. The response is one buffered JSON body carrying
base64-encoded images; decode `data[0].b64_json` to get the bytes.
There is **no streaming** on this connector (`stream_response: false`),
so `GET /v1/stream/meta-image` is not available.

Discovery advertises `modalities: ["text-to-image"]`, `input: "text"`,
`output: "image"`.

> Meta also serves Muse Image conversationally on the Responses API
> (`/v1/responses`), for interleaving reference images and refining
> across turns. This connector points at the **one-off generations**
> path only. A conversational image connector would be a second
> `[[proxy_connectors]]` entry with its own URL — ask your operator.

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
