# Vera SDK

Rust SDK, demos, and guides for building apps on the [Vera Secure AI Wasm Gateway](https://github.com/Sozenta-Inc/vera).

## Quick start

### 1. Add the SDK to your app

```toml
# Cargo.toml
[dependencies]
vera-client = { git = "https://github.com/Sozenta-Inc/vera-sdk.git", path = "sdk" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

### 2. Get a token

On the Vera hub host:
```bash
vera-hub keys --keystore ~/vera/data/keystore.redb create --principal my-app
# Prints: ef2cbfdd...56c  ← this is your VERA_TOKEN (shown once, store it now)
```

The operator must also authorize your principal in the policy:
```toml
# policy-global.toml on the hub
[principals.my-app]
connectors = ["echo", "bedrock-claude", "llm-local", "moonshine"]
```

### 3. Connect and call

```rust
use vera_client::VeraClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = VeraClient::builder()
        .url("https://18.190.188.99:8443")     // Vera hub Elastic IP
        .bearer_from_env("VERA_TOKEN")
        .danger_accept_invalid_certs(true)      // self-signed certs in dev
        .build()?;

    // Call Claude via Bedrock
    let resp = client.infer("bedrock-claude",
        b"What is Rust? Answer in one sentence.").await?;
    println!("{}", String::from_utf8_lossy(&resp.body));

    Ok(())
}
```

### 4. Run it

```bash
export VERA_URL="https://18.190.188.99:8443"
export VERA_TOKEN="ef2cbfdde0d1b0cb55d0610fea24754531436a761a35db942e5414cab9f9656c"
export VERA_INSECURE=1  # only for self-signed certs in dev
cargo run
```

## Authentication

Every request to Vera requires a bearer token. The token maps to a **principal** —
an identity with a policy (which connectors it can access, rate limits, vault rules).

| Step | Who | How |
|------|-----|-----|
| 1. Create key | Operator | `vera-hub keys create --principal <name>` |
| 2. Authorize | Operator | Add principal to `policy-global.toml` |
| 3. Use token | App | `Authorization: Bearer <token>` header |

The SDK handles the header automatically via `.bearer()` or `.bearer_from_env()`.

### Default test credentials (dev only)

The deployed Vera hub at `18.190.188.99:8443` has a test principal:

```bash
export VERA_URL="https://18.190.188.99:8443"
export VERA_TOKEN="ef2cbfdde0d1b0cb55d0610fea24754531436a761a35db942e5414cab9f9656c"
```

This principal (`test`) has access to: `echo`, `llm-local`, `bedrock-claude`, `llm-fallback`.

## Available connectors

| Connector | Type | Default | What it does |
|---|---|---|---|
| `bedrock-claude` | Proxy | **Yes** | Claude models via AWS Bedrock — pick per-request via `"model"` field: `haiku` (default, Haiku 4.5), `sonnet` (Sonnet 4.6), `opus` (Opus 4.7), or any full Bedrock inference profile ID |
| `llm-local` | Proxy | | Qwen 3 0.6B via local Ollama |
| `echo` | Wasm | | Returns input unchanged (testing) |
| `moonshine` | Wasm | | Audio → transcript via Moonshine ONNX |
| `llm-fallback` | Fallback chain | | Tries bedrock-claude, falls back to llm-local |

### Model aliases

Operators can define aliases so apps don't hardcode connector names:

```toml
# vera.toml
[[models]]
name = "claude"
connector = "bedrock-claude"

[[models]]
name = "default-llm"
connector = "llm-local"
```

```rust
// App calls the alias — operator controls which backend it maps to
let resp = client.infer("claude", b"Hello").await?;
```

### Picking a Bedrock Claude model (SDK 0.2+)

The `bedrock-claude` connector accepts a `"model"` field per request — friendly aliases (`haiku`, `sonnet`, `opus`) or full Bedrock inference profile IDs. The SDK exposes a typed helper:

```rust
use vera_client::{BedrockModel, VeraClient};

let client = VeraClient::builder()
    .url("https://vera.sozenta.ai")
    .bearer(&token)
    .build()?;

// Cheap + fast (default)
let r1 = client.infer_claude(BedrockModel::Haiku, "Summarize this report.").await?;
// Higher quality
let r2 = client.infer_claude(BedrockModel::Sonnet, "Draft the legal review.").await?;
// Top-tier reasoning
let r3 = client.infer_claude(BedrockModel::Opus, "Audit this architecture.").await?;

// Or pass any Bedrock inference profile ID directly:
let r4 = client.infer_claude_with(
    "us.anthropic.claude-opus-4-5-20251101-v1:0",
    "Hello",
).await?;
```

If no `"model"` is supplied, the connector defaults to **Haiku 4.5**.

### Verifying which build is running

`/version` returns the build SHA + timestamp (no auth required). Useful when rolling out new images:

```rust
let v = client.version().await?;
println!("running: {} @ {}", v.git_sha, v.build_time);
```

### Fallback chains

Operator-declared retry paths — if the primary connector fails, Vera tries the next:

```toml
# vera.toml
[[fallback_chains]]
name = "llm-fallback"
connectors = ["bedrock-claude", "llm-local"]
```

```rust
// App calls the chain name — Vera handles failover transparently
let resp = client.infer("llm-fallback", b"Hello").await?;
```

## Three transport modes

### Mode 1 — Request/Response (HTTP POST)

```rust
// Text prompt → LLM → complete response
let resp = client.infer("bedrock-claude", b"What is Rust?").await?;
println!("{}", String::from_utf8_lossy(&resp.body));
```

### Mode 2 — Streaming response (chunked HTTP)

For LLM token streaming — tokens arrive as they generate:

```rust
let resp = client.infer("bedrock-claude", prompt_json).await?;
// resp.body contains the full streamed response collected by the SDK
```

### Mode 3 — WebSocket bidirectional (real-time voice)

For real-time streaming — send audio chunks, receive text chunks:

```rust
let ws_url = client.ws_url("moonshine");
// Connect with tokio-tungstenite, send audio, receive transcripts
```

Full setup + wire format + browser/Python examples for the Moonshine
streaming endpoint on a local Vera: see
[**GUIDE-STREAMING-LOCAL.md**](GUIDE-STREAMING-LOCAL.md).

## Search

Vendor-neutral web search through Vera. Same bearer, swappable
backends (Anthropic today, Perplexity / Brave / sovereign providers
swappable later — your client doesn't change).

```bash
curl -X POST https://vera.sozenta.ai/v1/search \
  -H "Authorization: Bearer $VERA_BEARER" \
  -H "Content-Type: application/json" \
  -d '{"query":"What is the current ECB interest rate?"}'
```

Returns ranked results + an optional LLM-synthesized answer with
citations. Full request/response schema, agent tool-calling pattern,
error handling, and backend selection: see
[**GUIDE-SEARCH.md**](GUIDE-SEARCH.md).

## Image generation

Generate and edit images through Stability AI on AWS Bedrock — same
bearer, same audit chain, same vault/policy/QoS pipeline. SigV4 signed
by the gateway; no AWS credentials client-side.

```bash
curl -X POST https://vera.sozenta.ai/v1/infer/stability-image \
  -H "Authorization: Bearer $VERA_BEARER" \
  -H "Content-Type: text/plain" \
  -d 'a photorealistic red apple on a wooden table, soft window light' \
  | jq -r '.images[0]' | base64 -d > apple.png
```

One connector, many models: pass `"model"` in the body to pick `core`
(default), `ultra`, `sd3-5-large`, `inpaint`, `outpaint`, `upscale`,
`remove-bg`, `style-transfer`, etc. Full per-model request shapes,
edit examples, error codes, and agent tool-call pattern: see
[**GUIDE-IMAGE.md**](GUIDE-IMAGE.md).

## Discovery

Auto-discover available models and their capabilities:

```rust
let models = client.discover().await?;
for m in &models {
    println!("{}: {} ({}) — {:?}",
        m.id, m.display_name, m.provider, m.capabilities.modalities);
}
```

Smart dispatch uses discovery to pick the best transport:

```rust
// Discovers models, picks transport, dispatches
let resp = client.call("bedrock-claude", b"Hello").await?;
```

## Agents

Run hosted agents that use LLMs + tools in a loop:

```rust
// Run the admin agent
let result = client.run_agent("admin", "show me all API keys").await?;
println!("{}", String::from_utf8_lossy(&result.body));

// Agent streaming via WebSocket
let ws_url = client.agent_ws_url("admin");
// Connect, send prompt as first message, receive AgentStep JSON objects
```

### Available agents

| Agent | Brain | Tools | What it does |
|---|---|---|---|
| `assistant` | llm-local | echo, llm-local | General assistant (sample) |
| `admin` | llm-local | admin-api | Manage Vera via natural language |

## SDK reference

### `VeraClient::builder()`

| Method | Required | Description |
|---|---|---|
| `.url("https://...")` | Yes | Hub base URL |
| `.bearer("vk_...")` | Yes | Bearer token |
| `.bearer_from_env("VERA_TOKEN")` | Alt | Read token from env var |
| `.session_id("session-123")` | No | Multi-turn session correlation |
| `.max_retries(3)` | No | Max 429 retries (default 3) |
| `.connect_timeout(Duration)` | No | TCP connect timeout (default 10s) |
| `.danger_accept_invalid_certs(true)` | No | Dev only — self-signed TLS |

### Methods

| Method | Description |
|---|---|
| `client.infer(connector, body)` | Call a connector (Mode 1 POST) |
| `client.call(model, body)` | Smart dispatch with auto-negotiation |
| `client.discover()` | Fetch + cache available models |
| `client.plugin(plugin_id, body)` | Call a plugin |
| `client.run_agent(agent_id, prompt)` | Run a hosted agent |
| `client.ws_url(connector)` | Build WebSocket URL (Mode 3) |
| `client.agent_ws_url(agent_id)` | Build agent streaming WebSocket URL |
| `client.with_session(id)` | Clone with session id |

### Error handling

```rust
use vera_client::VeraError;

match client.infer("bedrock-claude", prompt).await {
    Ok(resp) => println!("{}", String::from_utf8_lossy(&resp.body)),
    Err(VeraError::Unauthorized) => eprintln!("Bad token"),
    Err(VeraError::Forbidden) => eprintln!("Not authorized for this connector"),
    Err(VeraError::Throttled { retry_after_secs, .. }) => {
        eprintln!("Rate limited — retry in {retry_after_secs}s")
    }
    Err(VeraError::VaultBlocked { rules, .. }) => {
        eprintln!("PII detected: {rules:?}")
    }
    Err(VeraError::Server { status, message }) => {
        eprintln!("Server error {status}: {message}")
    }
    Err(VeraError::Transport(e)) => eprintln!("Network: {e}"),
    Err(VeraError::Config(msg)) => eprintln!("Config: {msg}"),
}
```

## What Vera handles server-side

Your app doesn't implement any of these — the gateway enforces them on every call:

- **Authentication** — token → principal resolution
- **Policy** — which connectors each principal can call (Lean 4 proven monotonicity)
- **Content rules** — PII detection with observe/block/mask actions (Lean 4 proven non-bypass)
- **Rate limiting** — 7-dimension QoS (principal, connector, group, agent, upstream, pairs, cooldown)
- **Fallback chains** — automatic retry across connector chains on failure
- **Audit chain** — BLAKE3 hash-chained, Ed25519-signed sealed segments
- **Egress control** — connectors can only reach allow-listed hosts
- **Metrics** — Prometheus scrape endpoint

## Demo apps

### `demos/echo/`

```bash
cd demos/echo
export VERA_URL="https://18.190.188.99:8443"
export VERA_TOKEN="ef2cbfdde0d1b0cb55d0610fea24754531436a761a35db942e5414cab9f9656c"
export VERA_INSECURE=1
cargo run -- "Hello, Vera!"
```

## Deployment Profiles

Vera ships three deployment profiles. Your SDK code doesn't change — only the server-side config differs.

| Profile | Installer | Use case | Models |
|---|---|---|---|
| **Local** | `./deploy/install-local.sh` | On-device AI, no cloud. Data never leaves the machine. | Ollama (Qwen, Llama, etc.), Moonshine STT |
| **Network** | `./deploy/install.sh` | Full deployment. Local + cloud models, forward proxy, all features. | Everything — Ollama + Bedrock + Anthropic + OpenAI |
| **Policy Only** | `./deploy/install-policy.sh` | Security layer for cloud AI. No local inference. PII scanning, audit, compliance. | Bedrock Claude, Anthropic (proxy), OpenAI (proxy) |

### Local Mode
```bash
# On-device — no cloud, no data leaves the machine
./deploy/install-local.sh
# App connects to http://127.0.0.1:8443
```

### Network Mode (full)
```bash
# All features — local models + cloud + proxy + forward proxy
./deploy/install.sh
# Edit vera.env with AWS creds
sudo systemctl start vera-hub
```

### Policy Only Mode
```bash
# Security/compliance layer for cloud AI
./deploy/install-policy.sh
# Edit vera.env with AWS creds
sudo systemctl start vera-hub
# Apps use:
#   HTTPS_PROXY=http://vera:8080 (transparent, no code changes)
#   ANTHROPIC_BASE_URL=https://vera:8443/anthropic (reverse proxy)
#   https://vera:8443/v1/infer/bedrock-claude (Vera API)
```

### What all profiles share
- Pipeline: auth → policy → QoS → vault → dispatch → audit
- Vault PII scanning (SSN + credit card with Luhn validation)
- BLAKE3 hash-chained audit (EU AI Act Art. 12 compliant)
- Lean 4 formal proofs (policy monotonicity, ingress non-bypass)
- Wasm sandboxed connectors
- Admin agent + chat UI
- Same SDK — your app code doesn't change between profiles

## Links

- [Vera Gateway](https://github.com/Sozenta-Inc/vera) — the gateway itself
- [Architecture](https://github.com/Sozenta-Inc/vera/blob/main/docs/ARCHITECTURE.md) — pipeline, streaming, discovery, agents
- [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) — voice app developer guide
- [Connector Guide](https://github.com/Sozenta-Inc/vera/blob/main/connectors/README.md) — build your own connector
