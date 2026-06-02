# Guide: Vera Image Generation

Generate, edit, and enhance images through Vera. Powered by Stability
AI on AWS Bedrock by default; the wire format is connector-shaped so
you can swap providers later without touching client code.

Companion to [GUIDE-SEARCH.md](GUIDE-SEARCH.md),
[GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md), and
[GUIDE-STREAMING-LOCAL.md](GUIDE-STREAMING-LOCAL.md).

## TL;DR

```bash
curl -X POST https://vera.sozenta.ai/v1/infer/stability-image \
  -H "Authorization: Bearer $VERA_BEARER" \
  -H "Content-Type: text/plain" \
  -d 'a photorealistic red apple on a wooden table, soft window light' \
  | jq -r '.images[0]' | base64 -d > apple.png
```

The response is a JSON envelope with one or more base64-encoded images:

```json
{
  "images": ["iVBORw0KGgoAAAANSUhEUgAA..."],
  "seeds": [42],
  "finish_reasons": ["SUCCESS"]
}
```

## What this is — and isn't

**Is:** a Vera-native inference endpoint (`POST /v1/infer/stability-image`)
that routes to Stability AI's image models on AWS Bedrock. Every call
flows through the vault, policy, audit, and QoS pipeline. SigV4 is
signed by the gateway — no AWS credentials client-side.

**Is not:** GPT-Image, Imagen, Midjourney, or DALL-E. The default backend
is Stability; we can add additional backends behind the same endpoint
later (or expose a new model alias).

## Authentication

Standard Vera bearer:

```
Authorization: Bearer <token>
```

The token's principal must have `stability-image` in its connectors
ACL. Default policy in the Vera image grants this to the `veya`
principal only — demo principals (`app`, `app-mask`, `app-trusted`)
get 403 on `/v1/infer/stability-image` to keep our Bedrock spend safe
from public traffic.

```bash
# Internal app — image allowed
curl ... -H "Authorization: Bearer $VEYA_BEARER" \
  .../v1/infer/stability-image

# Demo bearer — 403 forbidden
curl ... -H "Authorization: Bearer $DEMO_BEARER" \
  .../v1/infer/stability-image
```

## Model selection

`stability-image` is one Vera connector that routes to many underlying
Stability Bedrock models. Pass `"model"` in the request body to pick
one. Default: `core` (text-to-image, balanced cost).

| Alias | Bedrock model | Use case | Approx cost/call |
|---|---|---|---|
| `core` (default) | `stable-image-core-v1:1` | Balanced text-to-image | $0.04 |
| `ultra` | `stable-image-ultra-v1:1` | Premium text-to-image | $0.08 |
| `sd3-5-large` | `sd3-5-large-v1:0` | SD3.5 Large (open model) | $0.065 |
| `inpaint` | `stable-image-inpaint-v1:0` | Mask-based edit | $0.06 |
| `outpaint` | `stable-outpaint-v1:0` | Extend image edges | $0.06 |
| `upscale` | `stable-fast-upscale-v1:0` | Fast upscale | $0.01 |
| `upscale-creative` | `stable-creative-upscale-v1:0` | Creative upscale | $0.25 |
| `upscale-conservative` | `stable-conservative-upscale-v1:0` | Conservative upscale | $0.25 |
| `remove-bg` | `stable-image-remove-background-v1:0` | Background removal | $0.02 |
| `erase-object` | `stable-image-erase-object-v1:0` | Erase object | $0.03 |
| `style-transfer` | `stable-style-transfer-v1:0` | Apply style image | $0.07 |

A full Bedrock model id (e.g. `stability.stable-image-core-v1:1`) is
also accepted verbatim.

## Request shapes per model

### Text-to-image (`core`, `ultra`, `sd3-5-large`)

```ts
type TextToImageRequest = {
  model?: string;          // alias or full Bedrock id (optional)
  prompt: string;          // required
  aspect_ratio?: string;   // "1:1" (default), "16:9", "21:9", "2:3",
                           // "3:2", "4:5", "5:4", "9:16", "9:21"
  output_format?: string;  // "png" (default), "jpeg", "webp"
  seed?: number;           // 0..4294967294; 0 = random
  negative_prompt?: string;
};
```

Plain text in the body is auto-wrapped:

```bash
# These two are equivalent:
curl ... -d 'a sunset over a mountain'
curl ... -H "Content-Type: application/json" \
  -d '{"prompt":"a sunset over a mountain","aspect_ratio":"1:1","output_format":"png"}'
```

### Inpaint (`inpaint`)

Mask-based edit — replace the masked region of an image.

```ts
type InpaintRequest = {
  model: "inpaint";
  image: string;            // base64 PNG/JPEG of the source
  prompt: string;           // what to put in the masked region
  mask_source: "MASK_IMAGE_WHITE" | "MASK_IMAGE_BLACK" | "INPAINT_MODE_PRECISE";
  mask_image?: string;      // base64 PNG; required if mask_source is MASK_IMAGE_*
  negative_prompt?: string;
  output_format?: string;
  seed?: number;
};
```

### Outpaint (`outpaint`)

```ts
type OutpaintRequest = {
  model: "outpaint";
  image: string;
  left?: number;   // pixels to extend (0..2000)
  right?: number;
  up?: number;
  down?: number;
  prompt?: string;
  creativity?: number;     // 0..1
  output_format?: string;
  seed?: number;
};
```

### Upscale (`upscale`, `upscale-creative`, `upscale-conservative`)

```ts
type UpscaleRequest = {
  model: "upscale" | "upscale-creative" | "upscale-conservative";
  image: string;             // base64
  prompt?: string;           // creative-only
  output_format?: string;
  seed?: number;
};
```

### Background removal (`remove-bg`)

```ts
type RemoveBgRequest = {
  model: "remove-bg";
  image: string;
  output_format?: string;
};
```

For other variants (`style-transfer`, `erase-object`, etc.), see the
matching parameter set in AWS's Stability docs — Vera passes the body
through to Bedrock verbatim.

## Response

```ts
type ImageResponse = {
  images: string[];          // base64-encoded image(s)
  seeds: number[];           // seed used per image
  finish_reasons: string[];  // "SUCCESS" or filtered reason
};
```

To save the first image to disk:

```bash
curl -X POST .../v1/infer/stability-image \
  -H "Authorization: Bearer $VEYA" \
  -H "Content-Type: text/plain" \
  -d 'a serene mountain lake at dawn' \
  | jq -r '.images[0]' | base64 -d > out.png
```

In Python:

```python
import base64, requests
r = requests.post(
    "https://vera.sozenta.ai/v1/infer/stability-image",
    headers={"Authorization": f"Bearer {VERA_BEARER}", "Content-Type": "application/json"},
    json={"model": "ultra", "prompt": "isometric pixel-art castle at sunset",
          "aspect_ratio": "16:9", "output_format": "png"},
    timeout=60,
).json()
with open("castle.png", "wb") as f:
    f.write(base64.b64decode(r["images"][0]))
```

## Errors

| Status | Cause |
|---|---|
| 400 | Body empty, malformed JSON, invalid `aspect_ratio` / `output_format`, missing required field for the chosen model |
| 401 | Missing / invalid bearer |
| 403 | Principal lacks `stability-image` ACL |
| 502 | Bedrock returned 5xx |
| 504 | Bedrock timeout |

Vera surfaces Bedrock's `"message"` field in the error body where
available, so the most common 400s come back with a clear cause.

## Discovery

`stability-image` shows up in:

- `GET /v1/models` — with `capabilities.modalities = ["text-to-image","image-to-image"]`
- `GET /llms.txt` — under Connectors with modality tags
- `GET /v1/services` — in the live service inventory

To list every model and pick one programmatically:

```bash
curl -sk -H "Authorization: Bearer $VEYA" https://vera.sozenta.ai/v1/models \
  | jq '.[] | select(.capabilities.modalities[] | contains("text-to-image"))'
```

## Using image generation as an agent tool

```python
tools = [{
    "name": "vera_image",
    "description": "Generate or edit an image. Returns base64-encoded PNG.",
    "input_schema": {
        "type": "object",
        "properties": {
            "prompt": {"type": "string"},
            "model": {"type": "string", "enum": ["core", "ultra", "sd3-5-large"]},
            "aspect_ratio": {"type": "string"},
        },
        "required": ["prompt"]
    }
}]

def call_vera_image(prompt, model="core", aspect_ratio="1:1"):
    import requests, base64
    r = requests.post(
        "https://vera.sozenta.ai/v1/infer/stability-image",
        headers={"Authorization": f"Bearer {VERA_BEARER}"},
        json={"prompt": prompt, "model": model, "aspect_ratio": aspect_ratio,
              "output_format": "png"},
        timeout=60,
    ).json()
    return base64.b64decode(r["images"][0])  # raw PNG bytes
```

## Cost model

Cost depends on the model alias (see the table above). All Stability
image calls are paid for on the Vera operator's Bedrock account. We
absorb the cost in the demo / free tier; higher-volume customers will
eventually meter image generation separately (not in v1).

Access is gated by the `stability-image` connector ACL. Only `veya`
and operator-blessed principals get image generation by default; demo
principals are 403'd.

## Operational notes

- **Backoff on 5xx.** Bedrock occasionally returns 503 under regional
  load. Exponential backoff (1s, 2s, 4s) before retrying.
- **Don't retry on 4xx.** A 400 means the request is malformed (bad
  aspect ratio, missing `image` for an edit model, etc.). Fix the
  request and resubmit.
- **PNG sizes.** Default Stable Image Core PNGs are ~1.5MB each
  (1024×1024). Set `output_format: "webp"` for smaller responses.
- **Audit.** Every call is logged with a BLAKE3 hash of the prompt and
  the selected model. Image bytes are NOT logged.
- **Stability latency.** Typical 6–15s for `core`, 10–25s for `ultra`.
  Set your client timeout to ≥60s.

## Quickstart

```bash
# 1. Get a bearer with stability-image enabled.
KEY=$(docker exec docker-vera-hub-1 cat /vera/data/demo-key-veya.txt)
#    Production: grab the veya bearer from https://vera.sozenta.ai/

# 2. Verify the connector is loaded
curl -sk -H "Authorization: Bearer $KEY" \
  https://vera.sozenta.ai/v1/models \
  | jq '.[] | select(.id == "stability-image") | .capabilities'

# 3. Generate an image
curl -sk -X POST \
  -H "Authorization: Bearer $KEY" \
  -H "Content-Type: text/plain" \
  -d 'a tiny robot wearing a wizard hat, claymation style' \
  https://vera.sozenta.ai/v1/infer/stability-image \
  | jq -r '.images[0]' | base64 -d > robot.png
open robot.png
```

If you get 403, your bearer lacks the `stability-image` ACL — request
a `veya`-tier bearer from the operator.

---

## Related guides

- [GUIDE-SEARCH.md](GUIDE-SEARCH.md) — web search with swappable backends
- [GUIDE-VOICE-PLUGIN.md](GUIDE-VOICE-PLUGIN.md) — STT → LLM → TTS pipelines
- [GUIDE-STREAMING-LOCAL.md](GUIDE-STREAMING-LOCAL.md) — WebSocket streaming endpoints
- [README.md](README.md) — top-level SDK quickstart
