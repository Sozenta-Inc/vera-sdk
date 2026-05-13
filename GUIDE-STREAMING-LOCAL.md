# Guide: Streaming voice on a local Vera

How to call Vera's WebSocket streaming endpoint from a client app
against a locally running Vera + Moonshine sidecar. Companion to
[`GUIDE-VOICE-PLUGIN.md`](GUIDE-VOICE-PLUGIN.md), which covers the
broader voice plugin/connector model.

## What you get

- **WebSocket transport** at `wss://<vera-host>:8443/v1/stream/moonshine`
- **Live partial transcripts** emitted as the user speaks — TTFT
  ~360 ms on M3 Pro vs ~120 ms total compute time for a 1 s utterance
- **Same Vera pipeline guarantees** as the REST path: auth at
  upgrade, policy ACL, audit on session close
- **No client framework lock-in** — anything that speaks WebSockets
  + raw 16-bit PCM works (browsers, Python, Go, etc.)

## When to use streaming vs the REST endpoint

| Pick this | If you want |
|---|---|
| **REST** `POST /v1/infer/moonshine` | Send a full audio clip, get one transcript back. Simpler. Same Vera vault scan applies. Latency: 130-600 ms warm depending on audio length. |
| **WebSocket** `wss://…/v1/stream/moonshine` | See words appear as they're spoken (dictation, live captioning, sub-1s voice command UX). Latency: ~360 ms time-to-first-text, progressive updates the whole way through audio capture. |

You can use both against the same Vera with the same bearer token —
pick per request.

## Local stack setup

The hybrid shape ([`deploy/docker/docker-compose.gemma-voice-hybrid.yml`](../deploy/docker/docker-compose.gemma-voice-hybrid.yml))
brings up:

- Moonshine sidecar (`docker-moonshine-1`) — Python + `moonshine-voice`
  with the MEDIUM_STREAMING model
- Vera Hub (`docker-vera-hub-1`) bound to `localhost:8443`
- Native `llama-server` on the Mac host for Gemma multimodal (only
  needed if you also want `/v1/chat/completions` against Gemma —
  optional for streaming-only workloads)

```bash
cd deploy/docker
REMOTE_VERA_TOKEN=dummy docker compose -f docker-compose.gemma-voice-hybrid.yml up --build
```

On first boot the sidecar downloads ~80 MB of Moonshine model weights
to its container volume. Subsequent restarts skip the download.

Grab the bearer token (the container prints three on first boot — one
per vault mode). Use the `block` one for production-shaped traffic:

```bash
docker exec docker-vera-hub-1 cat /vera/data/demo-key.txt
```

Smoke-test the WS endpoint is up before wiring the client:

```bash
curl -sk --max-time 3 --http1.1 \
  -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  "https://localhost:8443/v1/stream/moonshine?token=$KEY"
# expect: HTTP/1.1 101 Switching Protocols
```

## Wire format

### Inbound (client → server)

**Binary frames only**, each carrying a chunk of signed 16-bit
little-endian PCM audio at **16 kHz mono**. Frame size is arbitrary;
typical browser MediaRecorder chunks are 100-250 ms (3200-8000 samples
= 6400-16000 bytes). Smaller is fine and lowers TTFB.

The Vera bridge does not interpret the audio — it passes frames
through to the Moonshine sidecar verbatim, which runs the streaming
inference loop. **The sidecar expects this exact format**; sending
raw WebM/Opus/MP3 will not decode.

### Outbound (server → client)

**Text frames only**, one JSON object per event:

```json
{
  "type": "started" | "updated" | "completed" | "final" | "error",
  "line_id": 0,
  "text": "Tell me everything you know about",
  "timestamp_ms": 1410
}
```

Event semantics:

| `type` | When emitted | What `text` contains |
|---|---|---|
| `started` | New transcript line begins (sentence boundary, prior line completed) | First tokens of the new line |
| `updated` | More audio processed; line text revised | Current best-guess for the in-progress line |
| `completed` | Line is final from the model's POV (sentence boundary detected) | Settled text for that line |
| `final` | Sent once on socket close, after a force-update of any in-flight buffer | The full assembled transcript across all lines |
| `error` | Decode/inference failed | Brief error string |

**Important**: the model **revises early guesses** as more context
arrives. A 5-second utterance may emit 10-15 `updated` events, with
each one potentially replacing the previous text entirely (not
appending). Render the latest text, not a concatenation.

`timestamp_ms` is wall-clock milliseconds since the WS session
opened, on the server side. Useful for measuring end-to-end TTFT.

## Browser client (JavaScript)

Browser `MediaRecorder` produces WebM/Opus by default — that won't
work. Use the Web Audio API to capture PCM samples directly and
downsample to 16 kHz. Skeleton:

```javascript
const KEY = "<vera-bearer-token>";
const ws = new WebSocket(`wss://localhost:8443/v1/stream/moonshine?token=${KEY}`);
ws.binaryType = "arraybuffer";

ws.onmessage = (ev) => {
  const e = JSON.parse(ev.data);
  if (e.type === "updated" || e.type === "completed") {
    document.getElementById("transcript").textContent = e.text;
  } else if (e.type === "final") {
    console.log("session done:", e.text);
  }
};

// Capture audio at 16 kHz mono — most browsers default to 48 kHz, so
// downsample with an AudioWorklet or a single-rate AudioContext.
const ctx = new AudioContext({ sampleRate: 16000 });
const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
const src = ctx.createMediaStreamSource(stream);

await ctx.audioWorklet.addModule("pcm-encoder.js");
const node = new AudioWorkletNode(ctx, "pcm-encoder");
node.port.onmessage = (ev) => {
  // ev.data is an Int16Array — send as a binary frame
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(ev.data.buffer);
  }
};
src.connect(node).connect(ctx.destination);
```

`pcm-encoder.js`:

```javascript
class PCMEncoder extends AudioWorkletProcessor {
  process(inputs) {
    const input = inputs[0]?.[0];
    if (!input) return true;
    const out = new Int16Array(input.length);
    for (let i = 0; i < input.length; i++) {
      const s = Math.max(-1, Math.min(1, input[i]));
      out[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    this.port.postMessage(out);
    return true;
  }
}
registerProcessor("pcm-encoder", PCMEncoder);
```

Buffer size: each AudioWorklet callback gives 128 samples by default
(8 ms at 16 kHz). That's smaller than ideal — accumulate ~10-20
callbacks (~100-200 ms) before sending to avoid WS frame overhead.

## Python client

```python
import json, ssl, threading, time, wave
import websocket

KEY = "<vera-bearer-token>"
URL = f"wss://localhost:8443/v1/stream/moonshine?token={KEY}"

# Read a 16 kHz mono WAV and stream it in 250 ms chunks.
with wave.open("input.wav", "rb") as w:
    assert w.getframerate() == 16000 and w.getnchannels() == 1
    pcm = w.readframes(w.getnframes())

ws = websocket.create_connection(URL, sslopt={"cert_reqs": ssl.CERT_NONE})
start = time.perf_counter()

def reader():
    while True:
        try:
            msg = ws.recv()
        except Exception:
            return
        if not msg:
            return
        evt = json.loads(msg)
        t_ms = (time.perf_counter() - start) * 1000
        print(f"[+{t_ms:7.0f} ms] {evt['type']:>9}: {evt.get('text', '')!r}")
        if evt['type'] == 'final':
            return

threading.Thread(target=reader, daemon=True).start()

chunk = 8000  # 250 ms at 16 kHz mono int16
for i in range(0, len(pcm), chunk):
    ws.send_binary(pcm[i:i+chunk])
    time.sleep(0.25)  # pace at real-time

ws.close()
```

Install: `pip install websocket-client`.

## Latency expectations

Measured end-to-end through Vera against a local stack on M3 Pro
(MEDIUM_STREAMING model, 250 ms chunks):

| Metric | Value |
|---|---|
| Time-to-first-text (TTFT) | **~360 ms** |
| Per-update cadence | every 250-600 ms |
| Settled transcript at audio-end | within 200 ms |
| Total session bytes overhead | <1 KB JSON for a typical 5 s utterance |

For comparison, the buffered REST endpoint (`POST /v1/infer/moonshine`)
on the same audio is ~70-220 ms total — *faster total compute*, but
the user sees nothing until that whole window passes. Streaming wins
on **perceived** latency because words appear during recording.

## Switching model sizes

Three streaming model sizes available via env override:

```yaml
# docker-compose.gemma-voice-hybrid.yml
moonshine:
  environment:
    - MOONSHINE_MODEL_ARCH=medium_streaming  # default
    # or: small_streaming | base_streaming | tiny_streaming
```

| Variant | Batch latency (1 s) | Accuracy |
|---|---|---|
| `tiny_streaming` | ~60 ms | ok for clear speech |
| `base_streaming` | ~80 ms | good general |
| `small_streaming` | ~100 ms | better |
| **`medium_streaming`** (default) | ~140 ms | best |

The streaming-loop overhead is the same across sizes — the difference
is mostly per-token inference cost. For voice commands in a quiet
environment `tiny_streaming` is often enough; for noisy/accented
audio go to medium.

## Pipeline guarantees on the streaming path

Compared to the REST path, the streaming bridge has slightly different
characteristics:

| Invariant | REST `/v1/infer/moonshine` | WS `/v1/stream/moonshine` |
|---|---|---|
| Auth (bearer required) | yes | yes (at upgrade) |
| Policy ACL (`moonshine` connector permitted) | yes | yes (at upgrade) |
| Vault PII scan on audio bytes | yes (no-op on binary) | **no** — frames pass through |
| Audit event | per request | one per WS session (with byte counts) |
| Rate limit | per request | per WS upgrade |

The vault gap is intentional: binary audio bytes never match Vera's
text-shaped PII rules, so the scan returned no matches on the REST
path either. The session-level audit gives operators the byte counts
+ duration for forensics; for line-level transcripts you'd add an
audit hook in the sidecar to log the final text per session.

## Troubleshooting

**Connection drops with no error frame.** Most often a wrong bearer
token. Check `gh run view` or the Vera log for `unauthorized` —
WS handshake fails with HTTP 401 before upgrading.

**`HTTP/2 405 — Request method must be CONNECT`.** Your client used
HTTP/2 for the upgrade. WebSocket over HTTP/2 needs RFC 8441
CONNECT-method extended upgrade, which axum supports but most clients
don't auto-negotiate. Force HTTP/1.1 (browsers do this automatically;
for curl pass `--http1.1`).

**Empty / repeated prompt-echo transcripts.** Symptom: server returns
`"Transcribe this audio."` (your prompt) or repeats it. Means the
audio bytes Moonshine decoded to silence or garbage. Verify:
1. Frames are int16 LE PCM, not float32 / int32 / WebM.
2. Sample rate is exactly 16000.
3. Mono (1 channel), not stereo interleaved.
4. Frame contains real audio (> 100 ms, non-silent).

**No `final` event after closing.** Some WS clients close the socket
abruptly without giving the server time to send the trailing frame.
Use a half-close (send a close frame and wait for the upstream's
close frame) instead of just `socket.close()`. In Python's
`websocket-client` this is `ws.close(timeout=2)`.

## Further reading

- [`GUIDE-VOICE-PLUGIN.md`](GUIDE-VOICE-PLUGIN.md) — the broader
  voice plugin/connector model (STT → LLM → TTS chains).
- [`deploy/docker/docker-compose.gemma-voice-hybrid.yml`](../deploy/docker/docker-compose.gemma-voice-hybrid.yml)
  — the local stack definition.
- [`services/moonshine/server.py`](../services/moonshine/server.py)
  — sidecar wire-format authority (look here when in doubt about
  what the server actually sends).
