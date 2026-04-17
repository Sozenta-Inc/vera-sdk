# Vera Client SDK & Demos

Everything an app developer needs to integrate with the Vera Secure AI Wasm Gateway.

## `vera-client` — Rust SDK

A typed HTTP client library for calling connectors and plugins hosted by `vera-hub`.

```bash
cargo add vera-client
```

```rust
use vera_client::VeraClient;

let client = VeraClient::builder()
    .url("https://vera.internal:8443")
    .bearer("vk_a1b2c3d4...")
    .build()?;

// Call a connector
let resp = client.infer("echo", b"hello").await?;

// Call a plugin (e.g. voice pipeline)
let resp = client.plugin("voice-pipeline", audio_bytes).await?;

// Multi-turn session
let session_client = client.with_session("session-abc-123");
let resp = session_client.plugin("voice-pipeline", turn_1_audio).await?;
let resp = session_client.plugin("voice-pipeline", turn_2_audio).await?;
```

### Features

- **Typed errors**: `Unauthorized`, `Forbidden`, `Throttled { retry_after }`, `VaultBlocked { rules }`, `Server`
- **Auto-retry on 429**: respects `Retry-After` header, configurable max retries
- **Session management**: `with_session(id)` attaches `X-Vera-Session` header
- **TLS-only**: `reqwest` with `rustls-tls`, HTTPS enforced
- **Connection pooling**: shares a single HTTP/2 connection pool

### Configuration

The client needs two things:

| Setting | Source | Example |
|---|---|---|
| Hub URL | Code or env var | `https://vera.internal:8443` |
| Bearer token | `vera-hub keys create` output | `vk_a1b2c3d4e5f6...` |

```rust
// From env vars (production)
let client = VeraClient::builder()
    .url(std::env::var("VERA_URL")?)
    .bearer_from_env("VERA_TOKEN")
    .build()?;
```

The bearer token is the **only auth the client needs**. Vera handles principal resolution, policy evaluation, QoS, vault scanning, and audit attribution server-side.

## Demo apps

### `demo-echo/`

Simplest possible client — calls the `echo` connector and prints the response.

```bash
export VERA_URL="https://localhost:8443"
export VERA_TOKEN="vk_your_token"
cargo run --manifest-path client/demo-echo/Cargo.toml -- "Hello, Vera!"
```

### Future demos

- `demo-voice-turn/` — STT → LLM → TTS via the voice-pipeline plugin
- `demo-observe-vault/` — send PII-laden body, observe vault block

## Further reading

- [GUIDE-VOICE-PLUGIN.md](../GUIDE-VOICE-PLUGIN.md) — building voice apps on Vera
- [DEPLOY.md](../DEPLOY.md) — deploying and configuring `vera-hub`
- [REQUIREMENTS.md](../REQUIREMENTS.md) — full system requirements
