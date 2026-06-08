# Veya Bot Bundle — Format Specification

This is the canonical wire format for a Veya bot. The same bytes power
local creation, file-based sharing (`.bot.tar.gz`), and remote publish
via Vera (`PUT /v1/agents/{id}/bundle`). Vera mirrors this spec so a
bot created in Veya, published to Vera, and installed by a third user
round-trips losslessly.

**Schema version:** 2
**MIME type:** `application/gzip`
**File extension:** `.bot.tar.gz`

> **Spec ownership.** This file is mirrored from the Veya repo
> (`docs/FORMAT-bot-bundle.md`). The Vera-side server implementation
> lives in `vera-hub/src/bundle.rs` and conforms to this document.
> If behavior diverges, the spec is the source of truth — file a bug
> against either side.

---

## 1. Folder layout

Every bot is a folder with this shape:

```
<bot_id>/
├── bot.json              REQUIRED  manifest (see §2)
├── instructions.md       REQUIRED  system prompt (long-form)
├── corpus/               OPTIONAL  documents to index for RAG
│   └── …                            see §3 for extension allowlist
├── prompts/              OPTIONAL
│   ├── welcome.md                   first-message text in chat
│   └── suggested.json               array of suggested question strings
└── examples/             OPTIONAL   eval / regression artifacts
    └── eval.jsonl                   one Q/A per line: {q, expected}
```

Locally, this folder lives at:
- **macOS:** `~/Library/Application Support/app.veya.editor/bots/<bot_id>/`
- **Linux:** `$XDG_DATA_HOME/app.veya.editor/bots/<bot_id>/` (planned)
- **Windows:** `%APPDATA%\app.veya.editor\bots\<bot_id>\` (planned)

The folder name IS the bot id (kebab-case, `[a-zA-Z0-9_-]`, ≤80 chars).

---

## 2. `bot.json` schema

```jsonc
{
  "version": 2,                                  // bump on breaking change
  "kind": "veya-bot",                            // discriminator (constant)
  "name": "Handbook Bot",                        // human-readable display name
  "instructions_file": "instructions.md",        // relative path inside the bot folder
  "mode": "chat" | "autonomous",                 // surface mode
  "corpus": {
    "mode": "none" | "linked" | "owned",         // see §3
    "source": "/absolute/path/to/user/folder"    // ONLY for linked mode
  },
  "welcomeMessageFile": "prompts/welcome.md",    // optional, chat mode
  "suggestedQuestionsFile": "prompts/suggested.json",  // optional
  "exposeMcp": false,                            // surface as agent_<id> in Vera MCP
  "exportedAt": "2026-06-08T15:42:00Z"           // provenance, display-only
}
```

### Field semantics

| Field | Required | Notes |
|---|---|---|
| `version` | yes | Importers reject unknown major versions. Current: 2. |
| `kind` | yes | Always `"veya-bot"`. Rejects on mismatch. |
| `name` | yes | Display name. Trimmed; non-empty. |
| `instructions_file` | yes | Always `"instructions.md"` in v2. Indirection preserved for future templating. |
| `mode` | optional | Default `"autonomous"` if omitted. |
| `corpus.mode` | yes (if corpus present) | See §3. Omitting the whole `corpus` object = no RAG. |
| `corpus.source` | conditional | Required for `linked`; absent for `owned` / `none`. |
| `welcomeMessageFile` | optional | Path inside bot folder. Chat mode only. |
| `suggestedQuestionsFile` | optional | Path to a JSON array of strings. |
| `exposeMcp` | optional | When true + published to Vera, surface as `agent_<slug>` MCP tool. |
| `exportedAt` | optional | ISO timestamp written by the exporter. Display-only. |

---

## 3. Corpus modes (local-only — `owned` is the only one on the wire)

The `corpus.mode` discriminator describes how the **local** Veya
instance tracks the bot's corpus. **Published bundles always carry
`mode: "owned"`** — see §6 Normalization. Vera and any other consumer
of the tar only ever sees `owned` (or absent corpus).

### `none`
Prompt-only bot. The `corpus/` directory may not exist. No RAG tools
are exposed to the bot's runtime. Publishes as `mode: "none"`.

### `linked` *(local only)*
The `corpus/` directory inside the bot folder is **empty**. The
indexer reads directly from `corpus.source` (an absolute path to a
user-chosen folder outside the bot home). Files stay where the user
keeps them; no duplication; live with edits.

**On publish:** the producer copies files from `source` into a
staging `corpus/` and writes a normalized `bot.json` (mode `owned`,
no source) into the tar. The recipient never sees `linked`. Vera
rejects bundles arriving with `mode: "linked"` at the publish endpoint.

### `owned`
Files live inside `corpus/` within the bot folder. Snapshot semantics.
The bot owns its copy; the user's other folders won't drift it.
`corpus.source` is unused. Publishes as `mode: "owned"`.

---

## 4. Tar contents — what's allowed

Producers MUST emit only the files below. Consumers MUST reject
entries outside this allowlist.

| Path inside the tar | Required? | Allowed extensions |
|---|---|---|
| `bot.json` | required | exact filename |
| `instructions.md` | required | exact filename |
| `prompts/welcome.md` | optional | `.md` |
| `prompts/suggested.json` | optional | `.json` |
| `examples/*.md` or `examples/*.jsonl` | optional | `.md`, `.jsonl` |
| `corpus/<path>/<file>` | conditional on mode | `.md` `.markdown` `.txt` `.text` `.csv` `.json` `.yaml` `.yml` `.pdf` |

Consumers MUST reject any entry where:
- The path is absolute.
- Any component is `..`, `.`, a dotfile, or in the never-include list:
  `.git/`, `.veya/`, `.env`, `node_modules/`, `target/`, `dist/`,
  `build/`, `.next/`, `.cache/`, `venv/`, `.venv/`, `.DS_Store`
- The tar entry is anything other than `Regular`, `Directory`, or
  `Continuous`. Symlinks, hard links, devices, FIFOs are silently
  dropped (potential security vectors).

### Size limits

- **Per-file maximum:** 50 MB (uncompressed). Larger files are
  skipped by the producer; consumers MUST also enforce this on extract.
- **Total bundle maximum:** 100 MB (uncompressed sum). The producer
  errors if exceeded; the consumer aborts the extract.

These limits keep `/v1/agents/{id}/bundle` uploads simple — no
multipart, no signed URLs in v1. Revisit when a customer hits the cap.

### Format

- `tar.gz` (gzip-compressed POSIX tar). MIME `application/gzip`.
- Entries SHOULD use the `pax` or `ustar` format. `gnu` tar extensions
  are accepted but discouraged (smaller compatibility surface).

---

## 5. Vera HTTP contract

The same bundle bytes flow through Vera via these endpoints. They sit
beside the existing `/v1/agents` CRUD; the manifest still goes there
as JSON. The bundle is a sub-resource:

| Method | Path | Body | Returns |
|---|---|---|---|
| `PUT` | `/v1/agents/{id}/bundle` | `application/gzip` (raw tar.gz) | `{manifest, size, file_count, knowledge_base?, idempotent_noop}` |
| `GET` | `/v1/agents/{id}/bundle` | — | `application/gzip` (tar.gz stream) |
| `DELETE` | `/v1/agents/{id}/bundle` | — | `204 No Content` (manifest stays) |

The existing list endpoint (`GET /v1/agents`) includes `has_bundle: bool`,
`bundle_size: int`, `bundle_sha256: string`, and
`bundle_schema_version: int` per entry so installers know whether a
bundle download is needed and whether the local copy is current.

**ACL:** `agents:write` extends to the bundle sub-resource (no new
permission name). GET also requires `agents:write` because corpus may
contain data that not every bearer should pull.

**Server-side handling (Vera-hub current implementation):**
1. Validate the tar (allowlist, size caps, no path traversal).
2. Run vault PII scan on every text file in `corpus/` before storing.
3. Persist the tar verbatim under `data/bundles/<id>/bundle.bot.tar.gz`.
4. Extract to `data/bundles/<id>/extracted/` with normalized `bot.json`.
5. Translate `bot.json` → Vera's `AgentManifest` and upsert via cycle 1.
6. **Cycle 7.5 (planned):** extract + index `corpus/` into a
   `sqlite-vec` KB linked to this agent so the bot can be called via
   `/v1/agent/{id}` without each client running its own index.
7. Stream the tar back on `GET /v1/agents/{id}/bundle`.

Server-side indexing is not required for v1 — the recipient indexes
locally on install. The bundle endpoint is a pure object-store proxy
in v1 (cycle 7).

### Idempotency

`PUT` is idempotent on SHA256: identical bytes for the same `{id}` →
HTTP 200 with `idempotent_noop: true` and no re-extraction. Different
bytes → HTTP 202 with `idempotent_noop: false` after the swap.

---

## 6. Normalization rules — bundles are ALWAYS self-contained

`linked` vs `owned` is a **local-only distinction**. It describes how
the publisher's running Veya tracks the bot's corpus. It does not
travel in the tar.

**Producers MUST emit bundles in which:**
- `bot.json` carries `corpus.mode = "owned"` (or omits `corpus`
  entirely for prompt-only bots).
- `bot.json` does NOT contain `corpus.source`.
- Every file referenced by the corpus is materialized inside
  `corpus/` — no external references, no symlinks pointing out.

The producer's local `bot.json` on disk MAY say `linked` with a
`source` path; the producer rewrites the manifest in-memory when
emitting the tar. The local file is untouched.

This means: Vera, and any other consumer (peer install via file,
third-party tooling) never sees the publisher's filesystem paths and
never has to interpret `linked` mode. Every bundle is a snapshot.

### Other normalization

1. **Owned-by-default on install.** Consumers MAY additionally
   rewrite `corpus.mode` to `"owned"` after extraction as a
   defense-in-depth measure for hand-crafted bundles. Vera applies
   this rewrite unconditionally.

2. **Empty corpus.** If the bot has no `corpus/` directory after
   unpack and `bot.json` says `owned`, Vera rewrites `corpus.mode` to
   `"none"`.

3. **Reject on publish.** A bundle arriving at `PUT /v1/agents/{id}/bundle`
   with `corpus.mode = "linked"` is **rejected** with HTTP 400
   (`validation_failed`). Producers must normalize before upload.

4. **ID assignment on install.** Recipients pick their own `bot_id`
   (typically a fresh UUID-derived slug). The id in the tar is
   advisory; the recipient is free to ignore it. This prevents two
   installs of the same bot from colliding on disk.

---

## 7. Backward compatibility

- **v1 → v2 migration.** v1 stored `corpus: {folder: "<path>"}`.
  v2 stores `corpus: {mode: "linked", source: "<path>"}`. The Veya
  reader accepts both — v1 shape is mapped to `mode: "linked"` on
  import. Producers MUST emit v2.

- **Unknown top-level fields.** Consumers MUST preserve unknown fields
  (log them as warnings, don't drop). This lets a newer Veya add fields
  without breaking older Vera or peer installers. Vera-side: unknown
  fields land in the `extra` map of the internal `BotJson` and survive
  the normalize-and-re-emit round-trip.

- **Missing optional sub-resources.** Producers MAY omit
  `prompts/`, `examples/`, `corpus/`. Consumers MUST treat absence as
  "feature disabled," never as an error.

---

## 8. Reference test vectors

The Vera-hub `bundle.rs` test suite covers:

- Pack-then-ingest minimal bundle (prompt-only)
- Idempotent re-PUT is a no-op
- Reject `kind` mismatch
- Reject oversized total
- Reject `corpus.mode = "linked"` on publish
- Reject missing required files (no `instructions.md`)
- Path-safety enforcement (absolute, `..`, dotfile, banned dir)
- Allowlist filtering per §4
- Agent id validation

Run: `cargo test --workspace bundle`.

---

## 9. Implementations

| Component | Repo | Path |
|---|---|---|
| Producer (local creation, export) | Veya editor app | `src/services/bot-bundle/*` |
| Server validator + extractor | Vera-hub | [`crates/vera-hub/src/bundle.rs`](https://github.com/Sozenta-Inc/vera/blob/main/crates/vera-hub/src/bundle.rs) |
| HTTP routes | Vera-hub | [`crates/vera-hub/src/ingress.rs`](https://github.com/Sozenta-Inc/vera/blob/main/crates/vera-hub/src/ingress.rs) |
| SDK client examples | This file's sibling | [`GUIDE-BUNDLES.md`](GUIDE-BUNDLES.md) |

Third-party producers and consumers are explicitly supported — this
format is public and stable. Just match §1–§7 and you'll interoperate
with both Veya and Vera.

---

*Spec mirror.* Authoritative copy in the Veya repo. Vera mirrors here
for SDK accessibility. Last sync: 2026-06-08.
