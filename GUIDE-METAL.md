# Guide — Metal Edge Vera (native, no Docker)

**Audience:** the Veya app team + anyone deploying Vera as an OS-native
service on a workstation or edge box.

The `metal-local` shape runs Vera as a **single Rust binary under
launchd (macOS) or systemd (Linux)** — no Docker — fronting two
host-local model servers in **proxy mode**: clients talk only to Vera,
and every request passes auth → policy ACL → PII vault scan → QoS →
hash-chained audit before any model sees a byte.

```
Veya ──https://localhost:8443──▶ Vera ──▶ 127.0.0.1:11434  Ollama   (chat, any pulled model)
        (auth/ACL/vault/QoS/audit)  └──▶ 127.0.0.1:8090   whisper  (ASR, whisper.cpp medium)
```

---

## Install

**From the DMG (no toolchain):** mount `VeraHub-metal-<sha>-<arch>.dmg`
and run `./VeraHub/install.sh` — prebuilt binary, connectors, vera-ctl,
and the installer are bundled. Operators produce the DMG with
`make metal-dmg` (lands in `dist/`).

**From source:**
```bash
git clone git@github.com:Sozenta-Inc/vera.git && cd vera
make metal          # build → ~/.vera → service unit → started
make metal-smoke    # verification (14 checks)
```

Idempotent: re-running upgrades the binary + service units and
**preserves** `~/.vera/data` (keystore, audit chain, hub identity) and
any operator-edited config.

Prereqs: `rustup` (until prebuilt binaries ship), `ollama serve`
running, and optionally whisper-server for ASR. Dev installs:
`vera-ctl asr-setup` (brews whisper-cpp + unit + default model) — a
client may spawn it after `409 runtime_missing` from the install API.
App-bundle distributions ship whisper-server inside the app and never
need it. Without ASR, `/v1/audio/*` 502s cleanly; everything else works.
ASR catalog is curated: **whisper-medium (default) and whisper-large-v3
only** — smaller variants are a quality cut we don't ship.

## Lifecycle

```bash
vera-ctl status     # service state + vera/ollama/whisper probes + hub_id
vera-ctl restart    # stop + start (data dir untouched)
vera-ctl update     # git pull → rebuild → atomic binary swap → restart → health check
vera-ctl logs       # tail the structured JSONL log
```

`~/.vera/bin/vera-ctl` — add `~/.vera/bin` to PATH. The data dir is
single-writer (keystore + audit are locked redb files): lifecycle is
always stop-then-start, never two hubs on one data dir.

## Extensions (federated MCP tool servers)

An **ext** is a local MCP server that vera-metal supervises and federates
into its governed `/mcp` — the way you add a custom capability (a 3D
printer, a CRM, an internal API) without modifying Vera. It reuses the
whole sidecar lifecycle: signed channel artifact, sha256 + minisign
verify, supervised service, register live (no hub restart).

```bash
vera-ctl ext list                 # installed exts + server up/down
vera-ctl ext install printer      # fetch (channel or tarball) → verify → supervise → federate
vera-ctl ext update printer       # re-install the channel artifact
vera-ctl ext uninstall printer    # deregister + stop + remove (keeps data/ unless --purge)
```

Install lays the ext under `~/.vera/ext/<id>/`, runs it as a per-user
service, waits for its `/healthz`, then registers it with the hub via
`POST /admin/mcp/upstreams` — so its tools appear in `/mcp` as
`<id>.<tool>`, ACL-gated (a principal needs the ext's `acl`) and audited.
See [GUIDE-MCP.md](GUIDE-MCP.md) → "Federating upstream MCP servers".

The reference ext is **`printer`** — a 3D-printer MCP server
(slice/send/status) with FlashForge (TCP 8899) + Moonraker (Klipper REST)
backends and **LAN auto-discovery** (installs, finds your printer, no IP
to type).

> **macOS:** a launchd-supervised ext that reaches LAN devices needs the
> **Local Network** grant — System Settings → Privacy & Security → Local
> Network → enable the ext binary. Without it, discovery + device control
> silently fail (the binary works fine from a Terminal/SSH context, which
> already has the grant). `vera-ctl ext install` prints this and opens the
> pane when a GUI session is present. Linux/systemd has no equivalent block.

## Auth

Two keys are minted at install into `~/.vera/data/`:

| File | Principal | ACL |
|---|---|---|
| `key-veya.txt` | `veya` | echo, passthrough (all proxied model traffic), `admin:read`, `admin:write` |
| `key-demo.txt` | `demo` | echo only — exists to prove the ACL gate |

Every endpoint requires `Authorization: Bearer <key>`. TLS is
self-signed (`curl -k` / trust-on-first-use).

## The client contract (what Veya calls)

| Intent | Call |
|---|---|
| **List installed models** | `GET /v1/models` (OpenAI shape — proxied to Ollama, so it IS the live pulled-model list) or `GET /ollama/api/tags` for sizes/digests |
| **Chat** | `POST /v1/chat/completions` (OpenAI shape) with `"model": "<name from tags>"` |
| Embeddings | `POST /v1/embeddings` |
| **Transcribe (ASR)** | `POST /v1/audio/inference` — multipart `file=@audio.wav` → whisper.cpp medium |
| **Unified model catalog** | `GET /admin/models` — curated ASR rows (installed/active) + Ollama's installed set + `asr_runtime_present`, one call for the whole picker (`admin:read`) |
| **Install a model** | `POST /admin/models/install {"backend":"llm"\|"asr","model":"…"}` (`admin:write`, audited). ASR: curated catalog only — `whisper-medium` (default), `whisper-large-v3`; sha-pinned download + activation symlink swap + service bounce. LLM: forwarded to Ollama pull (any registry name). Raw `/ollama/api/pull` also works |
| Remove a model | `DELETE /admin/models/{backend}/{name}` — active ASR model refuses deletion (activate another first) |
| Ops console | `GET /admin/console` (paste the veya key once) |

Notes:
- The metal config sets `[server] models_from_proxy = true` (proxy-role
  discovery): `GET /v1/models` forwards to Ollama, so clients use the
  plain OpenAI convention. Vera's connector inventory is an admin
  concern (visible in the ops console), never the client model list.
  Provider-role deployments (e.g. prod with the bedrock-claude
  translator) keep the default `false` and list Vera's own surface.
- The passthrough is buffered: a multi-GB `pull` returns on completion
  rather than streaming progress — fire it, poll tags.
- Vault runs on every body: a request containing e.g. an SSN is blocked
  with `422` **before** it reaches Ollama or whisper.

## Model-support policy

- **LLMs (GGUF via Ollama): any model, dynamically.** Ollama normalizes
  the API; `ollama pull` (or `/ollama/api/pull` through Vera) is the
  whole story. No Vera config change per model.
- **ASR (ONNX/GGML): curated + preseeded — never "any file".** ONNX is
  a file format, not an API contract; each ASR model needs an execution
  recipe (tokenizer, sample rate, chunking). Shipped: whisper.cpp
  `medium`. Additional sizes/models land as curated additions.

## Port map (all loopback)

| Port | Service | Exposed to clients? |
|---|---|---|
| 8443 | Vera ingress (TLS) | **yes — the only front door** |
| 9090 | Vera admin (loopback) | host-local only |
| 11434 | Ollama | no — reached via Vera |
| 8090 | whisper-server | no — reached via Vera |
