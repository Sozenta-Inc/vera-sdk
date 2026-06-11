# Guide — Temporary Token Provisioning (`/v1/getVeyaAuthTmpForDemo`)

**Audience:** the Veya app team + anyone wiring a first-party client that
needs a Vera bearer without making the user paste one.

**Status:** temporary / dev + demo convenience. Real customers manage the
token they were issued (see [Token tiers](#token-tiers)). This endpoint
exists so the Veya app can self-serve the current `veya` token during
development and distribution instead of scraping it out of the Vera
landing page HTML.

---

## TL;DR

- **Endpoint:** `GET https://vera.sozenta.ai/v1/getVeyaAuthTmpForDemo`
- **Auth:** header `X-Veya-Provision-Secret: <shared-secret>`
- **Returns:** `{"token":"<veya bearer>","principal":"veya"}`
- **Off by default.** Only exists where the operator added a
  `[provisioning]` block + set the secret env. A Vera with no
  `[provisioning]` returns `404`/`501` here — that's intended.
- **It is not strong auth.** The shared secret ships inside the
  distributed client, so treat it as drive-by-discovery protection, not
  a security boundary. The only thing it serves is a token the operator
  already treats as a shared demo credential.

---

## Why this replaces homepage scraping

The Veya app previously fetched `vera.sozenta.ai/` and regex-matched the
HTML for a `veya:<64hex>` token (see `config.ts → fetchVeraToken`). That
is brittle (breaks on page redesign, Cloudflare challenge, format drift)
and couples the app to the landing-page markup. This endpoint gives a
stable JSON contract that survives page changes and **picks up key
rotation automatically** (it reads the live token file each call).

---

## Token tiers

| Tier | Principal | How the client gets it | Can do |
|---|---|---|---|
| **Demo / public** | `app-trusted` | scraped/served publicly (rate-limited) | echo, claude, llm — **no** image/KB/writes |
| **Provisioned (this endpoint)** | `veya` | `GET /v1/getVeyaAuthTmpForDemo` + secret | image, KB, search, `kb:write`, `agents:write`, 20 rps |
| **Real customer** | per-customer | operator mints → user pastes in Settings | whatever that principal's ACL grants |

The provisioning endpoint serves the **`veya`** token — the privileged
internal one. That is exactly why it is secret-gated and opt-in: an
unauthenticated endpoint serving `veya` would hand Bedrock image spend +
write access to anyone who learns the URL.

---

## Endpoint contract

### Request
```http
GET /v1/getVeyaAuthTmpForDemo HTTP/1.1
Host: vera.sozenta.ai
X-Veya-Provision-Secret: <shared-secret>
```

### Responses

| Status | Meaning | Body |
|---|---|---|
| `200` | secret matched | `{"token":"<64hex>","principal":"veya"}` |
| `401` | secret missing/wrong | `unauthorized` |
| `503` | endpoint enabled but token file unreadable/empty | `token unavailable` |
| `404`/`501` | endpoint not enabled on this Vera | (fallback) |

`200` responses carry `Cache-Control: no-store` so the token never lands
in an intermediary cache.

The secret check is constant-time at the BLAKE3-digest level (same
approach as the keystore) — a wrong secret can't be recovered by timing.

---

## Veya integration

> **This is an atomic migration — do it in one commit.** The hardcoded
> `veya` token is the fallback in **three** files. If you change one
> without the others (e.g. set the MCP entry to `auth:{kind:'none'}` but
> leave `veraKbs`/`veraAgents` falling back to the stale constant), KB
> and agent **writes** break, because they need the `veya` ACL that only
> a properly-resolved token carries. Land steps 1–4 together.
>
> The three hardcoded sites (search for `6b72128e8899302c…`):
> - `src/ai/ensureVeraMcp.ts:41` — `VERA_VEYA_BEARER_DEFAULT`
> - `src/ai/veraKbs.ts:51` — `endpoint()` fallback
> - `src/ai/veraAgents.ts:84` — `endpoint()` fallback

### 1. Token resolution order (replace the scrape)

In `config.ts`, resolve the bearer in this priority:

1. **User-set token** (Settings → Vera Hub token) — *always wins*. This
   is how real customers run: they paste the token you issued them.
2. **Provisioned token** — `GET /v1/getVeyaAuthTmpForDemo` with the
   shared secret. Dev/demo only. Cache it; clear-and-refetch on `401`.
3. **Nothing** — fall back to anonymous/demo behaviour (image/KB tools
   simply won't appear until a token with that ACL is present).

```ts
// Sketch — replaces the HTML-scraping body of fetchVeraToken().
const PROVISION_SECRET = import.meta.env.VITE_VERA_PROVISION_SECRET; // build-time

async function fetchProvisionedToken(hubUrl: string): Promise<string | null> {
  if (!PROVISION_SECRET) return null; // no secret baked in → skip
  const resp = await tauriFetch(`${hubUrl}/v1/getVeyaAuthTmpForDemo`, {
    headers: { 'X-Veya-Provision-Secret': PROVISION_SECRET },
  });
  if (!resp.ok) return null;          // 401/404/503 → no provisioned token
  const { token } = await resp.json();
  return typeof token === 'string' && token.length >= 16 ? token : null;
}
```

### 2. Remove the three hardcoded bearers

Delete `VERA_VEYA_BEARER_DEFAULT` in `ai/ensureVeraMcp.ts` and the two
inline `6b72128e…` fallbacks in `ai/veraKbs.ts` + `ai/veraAgents.ts`.
They go stale on every key rotation. Each site should resolve through
the shared token layer instead:

- **`ensureVeraMcp.ts`** — register the MCP server with `auth:{kind:'none'}`
  (the MCP client resolves auth via the shared layer at call time).
- **`veraKbs.ts` / `veraAgents.ts`** — make `endpoint()` async and
  resolve via `await fetchVeraToken()` (which walks Settings → provisioned
  → demo). Drop the hardcoded `else`. Because KB/agent writes need the
  `veya` ACL, `fetchVeraToken` **must** be able to return a `veya`-scoped
  token — that's what step 1's provisioned path provides. Without it,
  `fetchVeraToken` resolves only the demo `app-trusted` token and writes
  `403` (see step 3).

### 3. Fix the retry semantics — distinguish 401 from 403

The current `vera.ts` retry refetches on `401`. Keep that, but **cap it
and never refetch on `403`**:

- **`401` (unauthenticated):** token invalid/rotated/expired →
  `clearVeraToken()` + refetch **once** + retry. (Already implemented;
  just ensure it can't loop more than once.)
- **`403` (authenticated, ACL-denied):** refetching can *never* help —
  the demo/provisioned token simply lacks that connector's ACL. Surface
  a clear message: *"This feature needs an upgraded token — paste your
  Vera token in Settings."* Do **not** refetch.

This is the actual cause of the "auth fails, refetch doesn't help" loop:
image generation on a demo token returns `403`, not `401`.

---

## Enabling it on a Vera deployment (operator)

The endpoint is **opt-in**. To turn it on:

1. Add a `[provisioning]` block to the Vera config (`vera.toml`):
   ```toml
   [provisioning]
   # Shared secret the client must send. $ENV is resolved at boot.
   secret     = "$VEYA_PROVISION_SECRET"
   # Plaintext token file to serve (the keystore only holds hashes, so
   # the plaintext must come from a file written at mint time).
   token_file = "/vera/data/demo-key-veya.txt"
   # Principal label echoed in the response (informational).
   principal  = "veya"
   ```
2. Set the secret env on the task (ECS task definition env, or the
   compose `environment:` block):
   ```
   VEYA_PROVISION_SECRET=<a long random string>
   ```
3. Deploy. On boot you'll see
   `provisioning endpoint /v1/getVeyaAuthTmpForDemo registered` in the
   logs.

**Safety:** if the secret env is missing or resolves empty, Vera logs an
error and **skips** registering the endpoint — it never crashes the
gateway and never registers an unguarded token endpoint. A Vera without a
`[provisioning]` block doesn't have the endpoint at all.

### Verify
```bash
# wrong/no secret → 401
curl -sk -o /dev/null -w '%{http_code}\n' https://vera.sozenta.ai/v1/getVeyaAuthTmpForDemo
# correct secret → 200 + token
curl -sk -H "X-Veya-Provision-Secret: $VEYA_PROVISION_SECRET" \
  https://vera.sozenta.ai/v1/getVeyaAuthTmpForDemo
```

---

## Rotation

The endpoint reads `token_file` on **every** request, so rotating the
`veya` key (e.g. EFS recreate → entrypoint re-mints `demo-key-veya.txt`)
is picked up with no restart and no client change. Rotate the **shared
secret** by changing `VEYA_PROVISION_SECRET` + shipping a new client
build with the matching `VITE_VERA_PROVISION_SECRET`.

---

## When to retire this

This is scaffolding for dev + distribution. The end state is:
**no token is served from any endpoint** — demo traffic uses an
anonymous/rate-limited tier (no credential to leak), and privileged
access is a token the operator mints and the user pastes once. Delete the
`[provisioning]` block (and this endpoint usage) when that lands.
