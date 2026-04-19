# Vera SDK

Rust SDK, demos, and guides for building apps on the [Vera Secure AI Wasm Gateway](https://github.com/bssingh/vera).

## Quick start

### 1. Add the SDK to your app

```toml
# Cargo.toml
[dependencies]
vera-client = { git = "https://github.com/bssingh/vera-sdk.git", path = "sdk" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

### 2. Connect and call

```rust
use vera_client::VeraClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = VeraClient::builder()
        .url(std::env::var("VERA_URL")?)
        .bearer_from_env("VERA_TOKEN")
        .danger_accept_invalid_certs(std::env::var("VERA_INSECURE").is_ok())
        .build()?;

    // Call an LLM connector
    let resp = client.infer("llm-local", b"Explain Rust in one sentence").await?;
    println!("{}", String::from_utf8_lossy(&resp.body));

    Ok(())
}
```

### 3. Run it

```bash
export VERA_URL="https://your-vera-hub:8443"
export VERA_TOKEN="your_bearer_token_here"
export VERA_INSECURE=1  # only for self-signed certs in dev
cargo run
```

## Three transport modes

Vera supports three ways to communicate, all authenticated and policy-gated:

### Mode 1 — Request/Response (HTTP POST)

```rust
// Text prompt → LLM → complete response
let resp = client.infer("llm-local", b"What is Rust?").await?;
println!("{}", String::from_utf8_lossy(&resp.body));

// Plugin composition (e.g. voice pipeline)
let resp = client.plugin("voice-pipeline", audio_bytes).await?;
```

### Mode 2 — Streaming response (chunked HTTP)

For LLM token streaming — tokens arrive as they generate.

Requires a **proxy connector** on the Hub with `stream_response = true`:
```toml
# vera.toml on the Hub
[[proxy_connectors]]
id = "llm-anthropic"
upstream_url = "https://api.anthropic.com/v1/messages"
stream_response = true
[proxy_connectors.upstream_headers]
x-api-key = "$ANTHROPIC_API_KEY"
content-type = "application/json"
```

Client code:
```rust
// The response body arrives as chunked HTTP — process chunks as they arrive
let resp = client.infer("llm-anthropic", prompt_json).await?;
// resp.body contains the full streamed response collected by the SDK
// For true chunk-by-chunk processing, use reqwest directly against the same URL
```

### Mode 3 — WebSocket bidirectional (real-time voice)

For real-time streaming — send audio chunks, receive text chunks simultaneously.

```rust
// Get the authenticated WebSocket URL
let ws_url = client.ws_url("moonshine");
// → "wss://vera:8443/v1/stream/moonshine?token=vk_..."

// Connect with any WebSocket client (e.g. tokio-tungstenite)
// Auth fires once on upgrade; each message runs through QoS + vault
```

Example with `tokio-tungstenite`:
```rust
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

let (mut ws, _) = connect_async(&client.ws_url("moonshine")).await?;

// Send audio chunk
ws.send(Message::Binary(audio_chunk.into())).await?;

// Receive transcription
if let Some(Ok(Message::Binary(text))) = ws.next().await {
    println!("Transcript: {}", String::from_utf8_lossy(&text));
}

ws.close(None).await?;
```

## SDK reference

### `VeraClient::builder()`

| Method | Required | Description |
|---|---|---|
| `.url("https://...")` | Yes | Hub base URL |
| `.bearer("vk_...")` | Yes | Bearer token from `vera-hub keys create` |
| `.bearer_from_env("VERA_TOKEN")` | Alt | Read token from env var |
| `.session_id("session-123")` | No | Attach `X-Vera-Session` for multi-turn correlation |
| `.max_retries(3)` | No | Max 429 retries (default 3) |
| `.connect_timeout(Duration)` | No | TCP connect timeout (default 10s) |
| `.danger_accept_invalid_certs(true)` | No | Dev only — accept self-signed TLS certs |

### Methods

| Method | Transport | Description |
|---|---|---|
| `client.infer(connector, body)` | Mode 1 (POST) | Call a connector, get complete response |
| `client.plugin(plugin_id, body)` | Mode 1 (POST) | Call a plugin |
| `client.ws_url(connector)` | Mode 3 (WS) | Build authenticated WebSocket URL |
| `client.with_session(id)` | All | Clone with session id attached |

### Error handling

```rust
use vera_client::VeraError;

match client.infer("llm-local", prompt).await {
    Ok(resp) => println!("{}", String::from_utf8_lossy(&resp.body)),
    Err(VeraError::Unauthorized) => eprintln!("Bad token"),
    Err(VeraError::Forbidden) => eprintln!("Policy denied"),
    Err(VeraError::Throttled { retry_after_secs, .. }) => eprintln!("Rate limited — retry in {retry_after_secs}s"),
    Err(VeraError::VaultBlocked { rules, .. }) => eprintln!("PII detected: {rules:?}"),
    Err(VeraError::Server { status, message }) => eprintln!("Server error {status}: {message}"),
    Err(VeraError::Transport(e)) => eprintln!("Network: {e}"),
    Err(VeraError::Config(msg)) => eprintln!("Config: {msg}"),
}
```

## Available connectors

| Connector | Type | Mode | What it does |
|---|---|---|---|
| `echo` | Wasm | 1 | Returns input unchanged (testing) |
| `llm-local` | Wasm | 1 | Text prompt → Qwen 3 0.6B via Ollama |
| `moonshine` | Wasm | 1, 3 | Audio bytes → transcript via Whisper |
| `proxy` | Wasm | 1 | GETs a URL, returns response |
| Proxy connectors | HTTP proxy | 1, 2 | Operator-declared upstream URL with optional streaming |

## Multi-turn sessions

```rust
let session = VeraClient::builder()
    .url("https://vera:8443")
    .bearer_from_env("VERA_TOKEN")
    .session_id("conversation-abc-123")
    .build()?;

let turn1 = session.infer("llm-local", b"What is Rust?").await?;
let turn2 = session.infer("llm-local", b"Give me an example").await?;
// Both turns grouped in the audit chain under session "conversation-abc-123"
```

## What Vera handles server-side

Your app doesn't implement any of these — the gateway enforces them on every call:

- **Authentication** — token → principal resolution
- **Policy** — which connectors each principal can call + content deny patterns
- **Rate limiting** — multi-dimensional QoS (per principal, connector, upstream host)
- **PII scanning** — regex-based vault in observe/block mode
- **Audit chain** — BLAKE3 hash-chained, Ed25519-signed sealed segments
- **Egress control** — connectors can only reach allow-listed hosts
- **Upstream backpressure** — exponential cooldown on failing upstreams
- **Metrics** — Prometheus scrape endpoint for dashboards

## Demo apps

### `demos/echo/`

```bash
cd demos/echo
export VERA_URL="https://your-vera-hub:8443"
export VERA_TOKEN="your_token"
export VERA_INSECURE=1
cargo run -- "Hello, Vera!"
```

## Setup: getting a token

```bash
# On the vera-hub host
vera-hub keys create --keystore /path/to/keystore.redb --principal my-app
# Prints: vk_64hexchars...  ← this is your VERA_TOKEN
```

## Links

- [Vera Gateway](https://github.com/bssingh/vera) — the gateway itself
- [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) — voice app developer guide
- [Architecture](https://github.com/bssingh/vera/blob/main/docs/ARCHITECTURE.md) — pipeline, streaming, transport modes

## Running agents

Agents are multi-step AI workflows: LLM decides → tools execute → repeat until done.

### One-shot (complete result)

```rust
let resp = client.run_agent("assistant", "What is 2+2? Think step by step.").await?;
let result: serde_json::Value = serde_json::from_slice(&resp.body)?;
println!("Answer: {}", result["answer"]);
println!("Steps: {}", result["steps"]);
```

### Streaming (live steps via WebSocket)

```rust
let ws_url = client.agent_ws_url("assistant");
// Connect with tokio-tungstenite, send prompt as first message,
// receive AgentStep JSON objects as each step completes:
// {"step":1,"action":"llm","content":"thinking..."}
// {"step":1,"action":"tool","tool":"echo","content":"result"}
// {"step":2,"action":"done","content":"The answer is 4"}
```

### Multi-turn (session memory)

```rust
let session = VeraClient::builder()
    .url("https://vera:8443")
    .bearer_from_env("VERA_TOKEN")
    .session_id("conversation-123")
    .build()?;

// First turn — agent remembers
let r1 = session.run_agent("assistant", "My name is Alice").await?;
// Second turn — agent recalls from context
let r2 = session.run_agent("assistant", "What is my name?").await?;
```

### Agent discovery

```rust
let models = client.discover().await?;
// Also check GET /v1/agents for available agents:
// curl https://vera:8443/v1/agents -H "Authorization: Bearer $TOKEN"
```
