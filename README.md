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

### Calling connectors and plugins

```rust
// Call a connector: POST /v1/infer/{connector}
let resp = client.infer("llm-local", b"your prompt").await?;
let resp = client.infer("echo", b"ping").await?;

// Call a plugin: POST /v1/plugin/{plugin_id}
let resp = client.plugin("voice-pipeline", audio_bytes).await?;
```

### Multi-turn sessions

```rust
// All calls share a session id for audit correlation
let session = VeraClient::builder()
    .url("https://vera:8443")
    .bearer_from_env("VERA_TOKEN")
    .session_id("conversation-abc-123")
    .build()?;

let turn1 = session.infer("llm-local", b"What is Rust?").await?;
let turn2 = session.infer("llm-local", b"Give me an example").await?;
// Both turns grouped in the audit chain under session "conversation-abc-123"
```

### Error handling

```rust
use vera_client::VeraError;

match client.infer("llm-local", prompt).await {
    Ok(resp) => {
        println!("{}", String::from_utf8_lossy(&resp.body));
    }
    Err(VeraError::Unauthorized) => {
        eprintln!("Bad token — check VERA_TOKEN");
    }
    Err(VeraError::Forbidden) => {
        eprintln!("Policy denied — principal not authorized for this connector");
    }
    Err(VeraError::Throttled { retry_after_secs, .. }) => {
        eprintln!("Rate limited — retry in {retry_after_secs}s");
    }
    Err(VeraError::VaultBlocked { rules, .. }) => {
        eprintln!("PII detected — rules violated: {rules:?}");
    }
    Err(VeraError::Server { status, message }) => {
        eprintln!("Server error {status}: {message}");
    }
    Err(VeraError::Transport(e)) => {
        eprintln!("Network error: {e}");
    }
    Err(VeraError::Config(msg)) => {
        eprintln!("Client config error: {msg}");
    }
}
```

## Available connectors

| Connector | Type | What it does |
|---|---|---|
| `echo` | Text | Returns input unchanged (testing) |
| `llm-local` | Text | POSTs prompt to local Ollama (Qwen 3 0.6B) and returns generated text |
| `proxy` | HTTP | GETs a URL from the body, returns response |
| `moonshine` | Audio→Text | Sends audio to Moonshine ASR, returns transcript |

### Using `llm-local`

```rust
// Simple prompt
let resp = client.infer("llm-local", b"What is 2+2?").await?;

// The connector sends to Ollama's /api/generate endpoint
// Model: Qwen 3 0.6B (configurable at connector compile time)
// Response: generated text only (not JSON)
```

## Demo apps

### `demos/echo/`

Simplest possible client — calls the echo connector and prints the response.

```bash
cd demos/echo
export VERA_URL="https://your-vera-hub:8443"
export VERA_TOKEN="your_token"
export VERA_INSECURE=1
cargo run -- "Hello, Vera!"
# → POST /v1/infer/echo: Hello, Vera!
# ← 200 (12): Hello, Vera!
```

## What Vera handles server-side

Your app doesn't need to implement any of these — the gateway enforces them on every call:

- **Authentication** — token → principal resolution
- **Policy** — which connectors each principal can call
- **Rate limiting** — multi-dimensional QoS (per principal, connector, upstream host)
- **PII scanning** — regex-based vault in observe/block mode
- **Audit chain** — BLAKE3 hash-chained, Ed25519-signed sealed segments
- **Egress control** — connectors can only reach allow-listed hosts
- **Metrics** — Prometheus scrape endpoint for dashboards

## Setup: getting a token

Ask your Vera operator to create a principal for your app:

```bash
# On the vera-hub host:
vera-hub keys create --keystore /path/to/keystore.redb --principal my-app
# Prints: vk_64hexchars...  ← this is your VERA_TOKEN
```

Or via the admin API:

```bash
curl -X POST https://vera-hub:9090/admin/keys \
  -H "Authorization: Bearer <operator-token>" \
  -d '{"principal": "my-app"}'
# Returns: {"id":"...","principal":"my-app","secret":"vk_..."}
```

## Building voice apps

See [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) for how to compose STT → LLM → TTS pipelines using Vera plugins with `dispatch.call`, session-id correlation, and per-pipeline audit.

## Links

- [Vera Gateway](https://github.com/bssingh/vera) — the gateway itself
- [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) — voice app developer guide
