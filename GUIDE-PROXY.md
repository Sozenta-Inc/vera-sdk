# Proxy Models through Vera

Vera's second role: a **governed proxy** in front of model servers you
already run (Ollama, llama.cpp, whisper.cpp) or SaaS APIs you already
use (Anthropic, OpenAI, Gemini). Requests pass through byte-faithful —
same SDKs, same wire shapes — but every one is authenticated, ACL'd,
PII-scanned, rate-limited, and written to the audit chain on the way.

Use proxy mode when the upstream owns the models; use
[hosted models](GUIDE-HOSTED-MODELS.md) when Vera's connectors are the
model surface. One Vera can do both.

---

## 1. Transparent prefix proxies (`[[proxies]]`)

Operator config maps URL prefixes to upstreams:

```toml
[[proxies]]
id            = "ollama"
prefix        = "/v1"                          # longest prefix wins
upstream      = "http://127.0.0.1:11434/v1"
vera_auth     = true                           # Vera bearer required
upstream_auth = "none"                         # none | passthrough | override
```

`upstream_auth` controls the credential story:

| Mode | Behavior | Example |
|---|---|---|
| `none` | nothing forwarded | local Ollama |
| `passthrough` | client's own upstream key forwarded | bring-your-own Anthropic key via `/anthropic/*` |
| `override` | Vera injects a configured token (`$ENV`) | edge → central Vera federation |

### What runs on every proxied request

auth → policy ACL → vault (block/mask per principal) → QoS → forward
(wasm-sandboxed, egress allow-listed) → audit. A request carrying an
SSN is blocked with `422` **before it reaches the upstream**. Bodies in
the audit chain are the post-vault redacted copies.

### Live examples

**SaaS passthrough (cloud Vera):**
```bash
# your Anthropic key, Vera's governance:
curl https://<your-vera>/anthropic/v1/messages \
  -H "Authorization: Bearer $VERA_KEY" -H "x-api-key: $ANTHROPIC_KEY" \
  -H "anthropic-version: 2023-06-01" -H "Content-Type: application/json" \
  -d '{"model":"claude-haiku-4-5","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}'
# /openai/* and /gemini/* work the same way
```

**Local model servers (metal/edge Vera, see [GUIDE-METAL.md](GUIDE-METAL.md)):**
```bash
curl -k https://localhost:8443/v1/chat/completions \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"model":"qwen3:8b","messages":[{"role":"user","content":"hello"}]}'
```

## 2. Proxy-role discovery (`models_from_proxy`)

In proxy deployments, set:

```toml
[server]
models_from_proxy = true
```

`GET /v1/models` then forwards to the upstream, so clients get the
**upstream's live model list** (e.g. exactly what `ollama list` shows)
via the standard OpenAI convention — pull a model, it appears. Vera's
internal connector inventory stays an admin-console concern, never the
client model list. Default `false` (provider role) keeps the built-in
connector listing.

## 3. Model lifecycle through the proxy

Expose the upstream's management API under its own prefix and the whole
model lifecycle flows through Vera — authed and audited:

```toml
[[proxies]]
id = "ollama-api"
prefix = "/ollama"
upstream = "http://127.0.0.1:11434"
vera_auth = true
upstream_auth = "none"
```

```bash
GET    /ollama/api/tags              # list installed (sizes, digests)
POST   /ollama/api/pull              # install — {"model":"llama3.2"}
DELETE /ollama/api/delete            # remove
```

Note: the passthrough is buffered — a multi-GB pull returns on
completion; poll `/ollama/api/tags` for arrival.

## 4. Forward proxy (HTTPS_PROXY mode)

For traffic you can't re-point at a prefix, Vera can run as an HTTPS
forward proxy (compile feature `forward-proxy`): clients set
`HTTPS_PROXY=http://vera:8080`, Vera intercepts TLS with its own CA,
and the vault scans what passes. Opt-in via `[forward_proxy]` config +
trusting Vera's CA cert. Use prefix proxies when you can — they need no
client CA trust.

## 5. Choosing a surface

| You have… | Use |
|---|---|
| An OpenAI-compatible local server (Ollama, llama.cpp, vLLM) | prefix proxy on `/v1` + `models_from_proxy = true` |
| SaaS APIs with team-held keys | prefix proxies (`passthrough` auth) — one audit chain across providers |
| A second Vera (edge → central) | prefix proxy with `override` auth — federation with two audit chains |
| Vera-managed backends (Bedrock) | [hosted models](GUIDE-HOSTED-MODELS.md) — provider role |
