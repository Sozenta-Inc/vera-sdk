# Guide: Voice Apps on Vera

Companion to [REQUIREMENTS.md](REQUIREMENTS.md), [INFRA.md](INFRA.md), and [DEPLOY.md](DEPLOY.md).
Audience: developers building voice-capable apps (STT → LLM → TTS, conversational assistants,
live-dictation tools) that route their AI traffic through Vera.

## TL;DR

Vera is an **enabler**, not a voice framework. You bring:

1. Signed `.wasm` **connectors** for each external service (STT, LLM, TTS).
2. A signed `.wasm` **plugin** that composes them via `dispatch.call`.
3. A **policy bundle** granting your app principal the needed connectors + hosts.

Vera enforces the same invariants it enforces on every request — auth, policy, rate limit,
egress allow-list, audit, bounded dispatch depth — automatically, across the whole voice
pipeline.

## Why Vera for voice

Voice workflows are inherently multi-service: speech-to-text from one vendor, inference from
another, text-to-speech from a third. Without a gateway each app re-implements auth, rate
limiting, cost visibility, audit, and the "don't let the model exfiltrate data" property
from scratch. Vera provides all of them once, at the gateway layer:

- **Per-principal policy** gates every sub-call in the pipeline, not just the first request.
  A plugin that calls `dispatch.call("llm-claude")` re-authorizes the parent principal
  before the sub-call runs (Phase 5 INV-PLUGIN-001, proven in Lean 4).
- **Egress allow-list** gates every outbound HTTP, including the connector's own calls
  out to the vendor. A compromised STT connector cannot exfiltrate transcripts to an
  unexpected host.
- **Rate limit shared across nested calls**. A voice turn is one parent request that
  may fan out into three sub-calls; throttling applies at the parent principal level,
  so a single bucket covers the whole turn.
- **Audit chain covers every hop**. Each `dispatch.call` lands an audit record, which
  Phase 7's sealed segments make tamper-evident end-to-end.
- **Bounded dispatch depth** (proven in Lean 4): a recursion in your pipeline can't
  DOS the Hub.
- **Signed bundles**: every connector and plugin is Ed25519-verified at load time.
  A malicious binary in `plugins_dir` fails fast at startup.

## Architecture: one voice turn

```
┌─────────────┐   TLS (8443)   ┌──────────────────────────────────────┐
│  Your App   │ ─────────────▶ │               vera-hub               │
│ (mobile /   │                │                                      │
│  desktop /  │                │  ingress → authn → authz → route     │
│  web)       │                │           │                          │
└─────────────┘                │           ▼                          │
                               │  ┌─────────────────────────┐         │
                               │  │ Plugin: voice-pipeline  │         │
                               │  │                         │         │
                               │  │  dispatch.call(stt-*) ──┼── 1 ──┐ │
                               │  │  dispatch.call(llm-*) ──┼── 2 ──┼─│──▶ wasmtime
                               │  │  dispatch.call(tts-*) ──┼── 3 ──┘ │    sandboxed
                               │  │                         │         │    connectors,
                               │  │  each sub-call re-      │         │    each making
                               │  │  authorized + audited   │         │    bounded
                               │  └─────────────────────────┘         │    outbound
                               │                                      │    HTTPS
                               └──────────────────────────────────────┘
```

All three sub-calls run in the same wasmtime sandbox chain, carrying the original caller's
principal. Each produces its own audit record; the app tags the parent request and all
sub-calls with the same `session_id` (see below) so forensics groups them as one turn.

## Step 1: pick or build connectors

A connector is a signed `.wasm` component implementing the `connector` world
(see `wit/connector/`). For voice you'll typically want:

| Service kind | Example vendors | What the connector does |
|---|---|---|
| STT (speech→text) | OpenAI Whisper, AssemblyAI, Deepgram | Accepts audio bytes, returns JSON transcript |
| LLM (text→text)   | Anthropic, OpenAI, local vLLM | Accepts JSON messages, returns JSON response |
| TTS (text→speech) | ElevenLabs, OpenAI TTS, Polly | Accepts text, returns audio bytes |

If the vendor ships a Vera connector, use theirs. Otherwise build one: any WASI-p2 HTTP
component that talks HTTPS to the vendor's API and shapes the response to match the
`dispatch` interface will work. `wasi:http/outgoing-handler` calls automatically go
through Vera's egress allow-list — no per-connector escape.

Sign the bundle:

```bash
vera-hub plugin keygen --out /etc/vera/plugin-signing.key   # if you don't have one
vera-hub plugin sign connectors/stt-whisper.wasm --key /etc/vera/plugin-signing.key
# produces connectors/stt-whisper.wasm.sig
```

(The `plugin keygen` / `sign` CLI is the same one used for plugin signing; same Ed25519
key shape.)

## Step 2: write a voice-pipeline plugin

A plugin is also a signed `.wasm` component, but it runs with the `dispatch.call` host
import available so it can compose connectors. See `wit/plugin/` for the world.

Minimum manifest (`voice-pipeline.wasm.toml`):

```toml
name = "voice-pipeline"
version = "0.1.0"
author = "your-team"

[[routes]]
method = "POST"
path = "/v1/voice/turn"

[dependencies]
connectors = ["stt-whisper", "llm-claude", "tts-elevenlabs"]

[budget]
max_memory_mb      = 64
fuel_per_request   = 2_000_000_000
epoch_deadline_ms  = 30_000

[egress]
# Plugin-level host allow-list that unions into the global policy. Keep
# this empty for plugins that only dispatch to connectors — the
# connectors' own egress lists cover the vendor hosts.
allow_hosts = []
```

Plugin code (pseudocode — real code targets the `plugin` WIT world):

```rust
// Inside the plugin's exported handler:
let audio = read_request_body()?;
let transcript = dispatch_call("stt-whisper", audio)?;          // sub-call 1
let completion = dispatch_call("llm-claude", transcript)?;      // sub-call 2
let speech     = dispatch_call("tts-elevenlabs", completion)?;  // sub-call 3
respond(speech)
```

Each `dispatch_call` re-enters Vera's policy gate under the caller's principal. If any
one is denied, the whole turn returns a descriptive 403 — the plugin does not need to
encode the policy itself.

Sign the plugin bundle the same way as connectors.

## Step 3: policy bundle for your voice principal

The signed policy bundle (`policy-global.toml`) grants a principal the right to invoke
specific connectors and plugins. For a voice principal:

```toml
[[principal]]
id = "voice-app-principal"

[[principal.connector]]
id = "stt-whisper"

[[principal.connector]]
id = "llm-claude"

[[principal.connector]]
id = "tts-elevenlabs"

[[principal.plugin]]
id = "voice-pipeline"
```

Local policy (operator-authored overrides) can tighten this further at runtime without
a bundle re-sign — for example, disabling a connector during an outage.

Plugin `dispatch.call` is gated by the **intersection** of the caller's allowed
connectors and the plugin's `dependencies.connectors`. A plugin cannot escalate its
reach beyond what policy grants to the caller.

## Step 4: session_id for multi-turn audit correlation

Voice is conversational. A single user session produces many audit events — one per
turn, plus three per pipeline sub-call. For forensics, replay, or billing, operators
want to group events by session.

**From the app side** (opaque to Vera): generate a session id when the voice session
starts (UUID, ULID, BLAKE3 of the session handle — shape is your choice). Send it
with every request for that session as a header:

```
X-Vera-Session: voice-session-7d1e9c-2026-04-14T04:20:00Z
```

**From the plugin side**: the plugin propagates the header into each `dispatch.call`
so sub-calls inherit it.

**From Vera's side**: `AuditEvent::with_session_id(...)` attaches the value to the
audit record. The `StoredEvent` preserves the string byte-for-byte through the hash
chain. The session id appears on the pull endpoints
(`GET /admin/audit?from_seq=...`) so operators can filter post-hoc.

Length is capped at 256 bytes. Treat it as opaque — the Hub never parses, transforms,
or echoes it beyond the audit chain.

> Wiring the header into `AuditEvent` from the ingress path is the application
> plumbing between your plugin code and Vera's `audit::append`. The gateway ships
> the storage layer; the plugin (or a Vera ingress-side helper in a later cycle)
> decides when to attach.

## Step 5: observability

Vera's Phase-7 admin listener surfaces everything you need to watch a voice
workload:

- `GET /metrics` — Prometheus text. Useful labels:
  - `vera_requests_total{connector="voice-pipeline",outcome=...}` — turn count.
  - `vera_request_latency_seconds{connector="voice-pipeline"}` — end-to-end
    turn latency. Compare against the per-sub-call latency by filtering on
    `connector="stt-whisper"` etc.
  - `vera_egress_requests_total{host=...,outcome=...}` — vendor-side egress by
    host. Denied egress on a voice host should page.
  - `vera_rate_limit_throttles_total` — if voice turns get throttled, tune the
    principal's QoS budget (Phase 8).
- `GET /admin/logs?since=...&level=warn&target=vera_hub::plugin` — plugin errors.
- `GET /admin/audit?from_seq=N&limit=100` — audit chain JSONL, including
  `session_id` on every voice-related record.
- `GET /admin/audit/segments` — sealed-segment metadata for long-range archive.

All four endpoints are loopback-only by default and bearer-guarded; TLS lands
in Phase 9 for non-loopback binds.

## Security notes

- **Transcripts can contain PII.** Names, addresses, credit-card numbers spoken aloud
  will appear in the STT output and flow into the LLM prompt. Phase 8's Vault subsystem
  adds a `mask` mode that can run inside the pipeline **before** the LLM sees
  untokenized PII. Shape when Vault lands: STT → Vault.mask → LLM → Vault.unmask → TTS.
- **Connector egress allow-lists are bounded.** Vera's wasmtime egress override denies
  any host not declared in the global policy + the connector's per-bundle allow-list.
  A vendor connector cannot silently ship transcripts to `evil.example.com`.
- **Plugin depth is bounded.** A buggy plugin that calls itself transitively (or
  forms a cycle with another plugin) is rejected at `max_dispatch_depth` (proven in
  Lean 4).
- **Audit chain is tamper-evident.** Sealed segments carry Ed25519 tail signatures
  (Phase 7 Cycle 60); if a voice turn's records need to be presented as evidence,
  `verify_sealed_segment` gives an external verifier a zero-trust way to confirm
  the chain.
- **Auth failures are audited.** A 401 from `POST /v1/voice/turn` appends an
  `AuthFailure` record (Phase 7 Cycle 62) with the `ingress.plugin_dispatch`
  surface, not just a counter bump.

## What Vera deliberately does NOT do

- **No voice-specific transport.** Vera speaks HTTPS; voice audio is bytes in the
  body. Apps that want WebRTC/RTP/WebSocket streaming terminate at the app server
  and call Vera over HTTP.
- **No UX opinion.** "Should the app use voice-only approval, visual preview, or
  push-to-talk?" is an app-layer decision. Vera enforces the same invariants
  regardless.
- **No voice model hosting.** Vera doesn't run STT/LLM/TTS models itself. Connectors
  wrap vendor APIs or your own inference service; Vera is the signed, audited,
  policy-gated glue between them.
- **No browser-side runtime.** Vera serves no HTML and no JavaScript. An operator
  console (Phase 10, `vera-console`) is a separate sidecar binary; a voice *app*
  ships its own frontend outside of Vera entirely.

## Checklist for a production voice app on Vera

- [ ] Connectors signed and placed in `paths.connectors_dir`.
- [ ] Plugin signed and placed in `paths.plugins_dir` with a companion `.toml`
      manifest.
- [ ] Signed policy bundle grants the voice principal the needed connectors + plugin.
- [ ] Egress allow-list in the bundle covers every vendor host the connectors reach.
- [ ] Rate-limit burst/refill tuned for voice volume (10–20× text).
- [ ] App attaches `X-Vera-Session` header for every turn; plugin propagates it.
- [ ] Vault mode decided (off / observe / block / mask) when Phase 8 Vault lands.
- [ ] `/metrics`, `/admin/logs`, `/admin/audit` scraped continuously.
- [ ] Audit sealing enabled (`audit.signing_key_path` set) if long-range archive
      matters.
- [ ] App's own logging redacts the raw transcript before it hits your sink; Vera's
      audit chain retains only the structured metadata by default.

## Further reading

- `specs/SPEC-POLICY.md` — how policy merge + evaluate works.
- `specs/SPEC-PLUGIN.md` — plugin world, `dispatch.call` semantics, bounded depth.
- `specs/SPEC-AUDIT.md` — hash chain, sealed segments, pull endpoints, AUDIT-005
  session correlation.
- `specs/SPEC-METRICS.md` — what `/metrics` exposes.
- `REQUIREMENTS.md §A.13` — Vault PII redaction (Phase 8).
