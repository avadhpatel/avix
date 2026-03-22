# LLM Service Spec

← [Back to Schema Index](./README.md)

**Kind:** `LlmConfig`  
**Service:** `llm.svc`  
**Config location:** `/etc/avix/llm.yaml`  
**Runtime state:** `/proc/services/llm/`  
**Secrets:** `/secrets/services/llm/<provider-name>.enc`

-----

## Overview

`llm.svc` is a built-in core service that owns all AI model provider connections in
Avix. It acts as a proxy and multiplexer for every AI inference call — text completion,
image generation, speech synthesis, transcription, and embeddings — regardless of which
provider or model fulfils the request.

Every AI tool call from a `RuntimeExecutor` goes through `llm.svc`. Agents never call
provider APIs directly.

**Why a dedicated service, not a library call inside the kernel:**

- Credential isolation — API keys and OAuth tokens are held only in `llm.svc`’s process
  memory, injected at boot from `/secrets/`. No other process ever holds them.
- Connection pooling — HTTP clients, retry state, token-refresh state, and rate limit
  windows are owned by one process, not duplicated per-agent.
- Provider routing — the decision of which provider fulfils a given request is
  deterministic policy, not LLM-decided.
- Modality routing — each tool call is dispatched to the correct provider based on the
  modality the provider supports and the system’s `defaultProviders` map.
- Dynamic tool availability — `llm.svc` marks tools `degraded` or `unavailable` per
  provider via `ipc.tool-add` / `ipc.tool-remove` in response to real outage state.
- Observability — all token usage, latency, file output, and error events are emitted
  from one place.

`llm.svc` is analogous to a kernel-mode device driver: it presents a stable, uniform
tool surface regardless of which underlying provider or modality is in use.

-----

## Modalities

A **modality** is the input/output shape of an AI call. Each modality maps to exactly
one tool in `llm.svc`’s tool surface.

|Modality       |Tool                 |Input                |Output               |
|---------------|---------------------|---------------------|---------------------|
|`text`         |`llm/complete`       |messages array       |text content blocks  |
|`image`        |`llm/generate-image` |text prompt          |image file → VFS path|
|`speech`       |`llm/generate-speech`|text                 |audio file → VFS path|
|`transcription`|`llm/transcribe`     |audio file (VFS path)|text                 |
|`embedding`    |`llm/embed`          |text                 |float vector         |

**Binary output rule:** tools that produce binary output (`image`, `speech`) never return
raw bytes over IPC. `llm.svc` writes the output file to the calling agent’s scratch
directory (`/proc/<pid>/scratch/`) and returns the VFS path. The agent then decides what
to do with the file — move it to workspace, attach it to a response, pass the path to
another tool.

**File creation is not a modality.** When an agent needs to produce a PDF, DOCX, or
spreadsheet, that is a deterministic formatting operation handled by `exec.svc` or an
installed service — not `llm.svc`. The pattern is: LLM generates content via
`llm/complete`, a service formats it into the target file type via `exec/run` or a
purpose-built tool. This keeps AI generation and file rendering cleanly separated.

-----

## Architecture

```
RuntimeExecutor (per agent)
        │
        │  llm/complete | llm/generate-image | llm/generate-speech
        │  llm/transcribe | llm/embed
        ▼
   router.svc  (IPC)
        │
        ▼
   llm.svc
     ├── provider registry (in-memory)
     │     ├── anthropic     ← text                        — API key
     │     ├── openai        ← text, image, speech,        — OAuth2
     │     │                    transcription, embedding
     │     ├── stability-ai  ← image                       — API key
     │     ├── elevenlabs    ← speech                      — API key
     │     └── ollama        ← text, embedding             — none (local)
     │
     ├── routing engine
     │     ├── resolve modality from tool name
     │     ├── if provider specified → validate it supports the modality
     │     └── else → look up defaultProviders[modality]
     │
     ├── credential layer
     │     └── kernel-injected at boot, never re-read from disk
     │
     ├── binary output handler
     │     └── writes image/audio to /proc/<pid>/scratch/, returns VFS path
     │
     └── response / stream relay
           └── text streams via jobs.svc; binary streams written to scratch file
```

-----

## Config Schema — /etc/avix/llm.yaml

```yaml
apiVersion: avix/v1
kind: LlmConfig
metadata:
  lastUpdated: 2026-03-21T00:00:00Z

spec:

  # ── Default providers per modality ────────────────────────────────────────
  # Used when an agent does not specify a provider on a tool call.
  # Each value must match the `name` of a provider below that declares
  # the matching modality in its `modalities` list.
  defaultProviders:
    text: anthropic
    image: openai
    speech: elevenlabs
    transcription: openai
    embedding: openai

  # ── Providers ─────────────────────────────────────────────────────────────
  providers:

    # ── Anthropic — text only, API key ────────────────────────────────────
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth:
        type: api_key
        secretName: llm-anthropic-key       # resolved from /secrets/services/llm/
        header: x-api-key
      models:
        - id: claude-opus-4
          modality: text
          contextWindow: 200000
          tier: premium
        - id: claude-sonnet-4
          modality: text
          contextWindow: 200000
          tier: standard
        - id: claude-haiku-4
          modality: text
          contextWindow: 200000
          tier: economy
      limits:
        requestsPerMinute: 50
        tokensPerMinute: 100000
      timeout:
        connectMs: 3000
        readMs: 120000
      retryPolicy:
        maxAttempts: 3
        backoffMs: 1000
        retryOn: [429, 529, 503]
      healthCheck:
        enabled: true
        intervalSec: 30
        endpoint: /v1/models

    # ── OpenAI — multi-modal, OAuth2 ─────────────────────────────────────
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: oauth2
        secretName: llm-openai-oauth        # encrypted blob: { access_token, refresh_token, expiry }
        tokenUrl: https://auth.openai.com/oauth/token
        clientId: avix-client
        clientSecretName: llm-openai-client-secret
        scopes: [model.read, completions.write]
        refreshBeforeExpiryMin: 5
      models:
        - id: gpt-4o
          modality: text
          contextWindow: 128000
          tier: premium
        - id: gpt-4o-mini
          modality: text
          contextWindow: 128000
          tier: economy
        - id: dall-e-3
          modality: image
          tier: standard
        - id: tts-1
          modality: speech
          tier: standard
        - id: tts-1-hd
          modality: speech
          tier: premium
        - id: whisper-1
          modality: transcription
          tier: standard
        - id: text-embedding-3-small
          modality: embedding
          dimensions: 1536
          tier: economy
        - id: text-embedding-3-large
          modality: embedding
          dimensions: 3072
          tier: premium
      limits:
        requestsPerMinute: 60
        tokensPerMinute: 200000
      timeout:
        connectMs: 3000
        readMs: 120000
      retryPolicy:
        maxAttempts: 3
        backoffMs: 1000
        retryOn: [429, 503]
      healthCheck:
        enabled: true
        intervalSec: 60
        endpoint: /v1/models

    # ── Stability AI — image only, API key ───────────────────────────────
    - name: stability-ai
      baseUrl: https://api.stability.ai
      modalities: [image]
      auth:
        type: api_key
        secretName: llm-stability-key
        header: Authorization
        prefix: "Bearer "
      models:
        - id: stable-diffusion-3
          modality: image
          tier: standard
        - id: stable-image-ultra
          modality: image
          tier: premium
      limits:
        requestsPerMinute: 20
        tokensPerMinute: 0                  # image providers don't use token limits
      timeout:
        connectMs: 3000
        readMs: 60000
      retryPolicy:
        maxAttempts: 2
        backoffMs: 2000
        retryOn: [429, 503]
      healthCheck:
        enabled: true
        intervalSec: 60
        endpoint: /v1/engines/list

    # ── ElevenLabs — speech only, API key ────────────────────────────────
    - name: elevenlabs
      baseUrl: https://api.elevenlabs.io
      modalities: [speech]
      auth:
        type: api_key
        secretName: llm-elevenlabs-key
        header: xi-api-key
      models:
        - id: eleven_multilingual_v2
          modality: speech
          tier: standard
        - id: eleven_turbo_v2
          modality: speech
          tier: economy
      limits:
        requestsPerMinute: 10
        tokensPerMinute: 0
      timeout:
        connectMs: 3000
        readMs: 120000
      retryPolicy:
        maxAttempts: 2
        backoffMs: 1000
        retryOn: [429, 503]
      healthCheck:
        enabled: true
        intervalSec: 60
        endpoint: /v1/models

    # ── Ollama — local, no auth ───────────────────────────────────────────
    - name: ollama
      baseUrl: http://localhost:11434
      modalities: [text, embedding]
      auth:
        type: none
      models:
        - id: llama3.2
          modality: text
          contextWindow: 131072
          tier: local
        - id: mistral
          modality: text
          contextWindow: 32768
          tier: local
        - id: nomic-embed-text
          modality: embedding
          dimensions: 768
          tier: local
      limits:
        requestsPerMinute: 0                # 0 = unlimited
        tokensPerMinute: 0
      timeout:
        connectMs: 1000
        readMs: 300000
      retryPolicy:
        maxAttempts: 1
        backoffMs: 0
        retryOn: []
      healthCheck:
        enabled: true
        intervalSec: 15
        endpoint: /api/tags
```

-----

## Provider Auth Types

### `api_key`

```
Fields:
  secretName   string   Name of the secret in /secrets/services/llm/
  header       string   HTTP header name (default: Authorization)
  prefix       string   Optional value prefix. Default: "Bearer " when header is
                        Authorization; empty for custom headers (x-api-key, xi-api-key)

Wire behaviour:
  Every request: HTTP header `<header>: <prefix><key>`
  No expiry. Key is rotated by operator via `avix llm rotate <provider>`.
```

### `oauth2`

```
Fields:
  secretName             string    Name of secret storing { access_token, refresh_token, expiry }
  tokenUrl               string    OAuth token endpoint
  clientId               string    OAuth client ID (non-sensitive, lives in config)
  clientSecretName       string    Name of secret storing the client secret
  scopes                 []string  OAuth scopes to request
  refreshBeforeExpiryMin int       Refresh this many minutes before token expiry

Wire behaviour:
  Every request: HTTP header `Authorization: Bearer <access_token>`
  llm.svc runs a background refresh task per oauth2 provider.
  On refresh: POST to tokenUrl with refresh_token → update in-memory token →
              re-encrypt and write back to /secrets/ via kernel syscall (never direct
              disk write by llm.svc).
  On refresh failure: mark provider degraded, emit tool.changed, log error, retry with backoff.
```

### `none`

```
Fields: (none)

Wire behaviour:
  No Authorization header sent.
  Used for local providers (Ollama, LM Studio, vLLM on localhost).
```

-----

## Provider Adapters

Every provider in `llm.yaml` is backed by a **provider adapter** inside `llm.svc`. The
adapter is the single place responsible for all translation between Avix’s internal
formats and the provider’s wire API. Nothing outside `llm.svc` ever contains
provider-specific serialisation logic — not `RuntimeExecutor`, not the kernel, not any
agent.

### Responsibility Split

```
RuntimeExecutor                         llm.svc (ProviderAdapter)
───────────────                         ─────────────────────────
Owns: conversation state                Owns: provider wire format
      tool grant validation                   auth headers
      HIL gating                              retry + backoff
      tool dispatch via IPC                   descriptor → provider schema
      result injection into context           provider response → Avix format
                                              tool call parse + name unmangle
Speaks: Avix-native types               Speaks: Anthropic API, OpenAI API, etc.
        never knows provider name              never knows agent state
```

`RuntimeExecutor` calls `llm/complete` with an Avix-native payload. It receives an
Avix-native response. Provider wire formats never cross the IPC boundary.

-----

### The Three Translation Responsibilities

Every adapter handles exactly three translations per `llm/complete` call:

#### 1. Outbound — Tool Descriptors → Provider Schema

The `llm/complete` call arrives with a `tools` array of Avix tool descriptors. The
adapter converts them to the provider’s native function/tool schema before making the
HTTP request.

**Avix tool descriptor (internal):**

```json
{
  "name": "fs/write",
  "description": "Write content to a file in the VFS.",
  "input": {
    "path":    { "type": "string",  "required": true,  "desc": "Absolute VFS path" },
    "content": { "type": "string",  "required": true,  "desc": "UTF-8 content" },
    "append":  { "type": "bool",    "required": false, "default": false }
  },
  "output": {
    "bytesWritten": { "type": "integer" }
  }
}
```

**After `AnthropicAdapter.translate_tool()`:**

```json
{
  "name": "fs__write",
  "description": "Write content to a file in the VFS.",
  "input_schema": {
    "type": "object",
    "properties": {
      "path":    { "type": "string",  "description": "Absolute VFS path" },
      "content": { "type": "string",  "description": "UTF-8 content" },
      "append":  { "type": "boolean", "description": "Append instead of overwrite" }
    },
    "required": ["path", "content"]
  }
}
```

**After `OpenAiAdapter.translate_tool()` (also used by Ollama):**

```json
{
  "type": "function",
  "function": {
    "name": "fs__write",
    "description": "Write content to a file in the VFS.",
    "parameters": {
      "type": "object",
      "properties": {
        "path":    { "type": "string" },
        "content": { "type": "string" },
        "append":  { "type": "boolean" }
      },
      "required": ["path", "content"]
    }
  }
}
```

#### 2. Inbound — Provider Tool Call → Avix Tool Call

The LLM response contains a tool call in the provider’s native format. The adapter
normalises it back to Avix’s format before returning to `RuntimeExecutor`.

**Anthropic raw tool call:**

```json
{
  "type": "tool_use",
  "id": "toolu_01ABC",
  "name": "fs__write",
  "input": { "path": "/users/alice/out.txt", "content": "hello" }
}
```

**OpenAI raw tool call:**

```json
{
  "id": "call_01ABC",
  "type": "function",
  "function": {
    "name": "fs__write",
    "arguments": "{\"path\":\"/users/alice/out.txt\",\"content\":\"hello\"}"
  }
}
```

**Both normalise to the same `AvixToolCall`:**

```json
{
  "callId": "toolu_01ABC",
  "name": "fs/write",
  "args": { "path": "/users/alice/out.txt", "content": "hello" }
}
```

`RuntimeExecutor` only ever sees `AvixToolCall`. It dispatches `fs/write` via IPC,
gets an `AvixToolResult` back, then asks the adapter to format the result for
re-injection into the conversation.

#### 3. Result Injection — Avix Result → Provider Message Format

After `RuntimeExecutor` executes the tool and receives a result, it passes the result
back through the adapter to produce the correctly-shaped message for the next LLM turn.

**Avix tool result (internal):**

```json
{
  "callId": "toolu_01ABC",
  "output": { "bytesWritten": 5 },
  "error": null
}
```

**After `AnthropicAdapter.format_tool_result()`:**

```json
{
  "role": "user",
  "content": [{
    "type": "tool_result",
    "tool_use_id": "toolu_01ABC",
    "content": "{\"bytesWritten\":5}"
  }]
}
```

**After `OpenAiAdapter.format_tool_result()`:**

```json
{
  "role": "tool",
  "tool_call_id": "call_01ABC",
  "content": "{\"bytesWritten\":5}"
}
```

-----

### Tool Name Mangling

Anthropic and OpenAI both restrict tool names to alphanumeric characters plus `_` and
`-`. Avix tool names use `/` as a namespace separator (`fs/write`,
`llm/generate-image`, `mcp/github/list-prs`). Every adapter applies a reversible mangle
on the way out and unmangle on the way in.

**Rule:** replace every `/` with `__` (double underscore).

```
Avix name            Provider name
─────────────────    ─────────────────────
fs/write          →  fs__write
llm/generate-image → llm__generate-image
mcp/github/list-prs→ mcp__github__list-prs
```

The unmangle is the exact reverse: replace every `__` with `/`. This is applied
by the adapter before returning an `AvixToolCall` to `RuntimeExecutor`. The
`RuntimeExecutor` always sees and uses unmangled Avix names.

**Invariant:** no Avix tool name contains `__` (double underscore). The tool registry
rejects registration of any name containing `__`. This makes the mangle lossless.

-----

### Built-in Adapters

|Adapter            |Provider names|Wire format                                          |
|-------------------|--------------|-----------------------------------------------------|
|`AnthropicAdapter` |`anthropic`   |Anthropic Messages API                               |
|`OpenAiAdapter`    |`openai`      |OpenAI Chat Completions API                          |
|`OllamaAdapter`    |`ollama`      |OpenAI-compatible (thin wrapper over `OpenAiAdapter`)|
|`StabilityAdapter` |`stability-ai`|Stability AI REST API (image only)                   |
|`ElevenLabsAdapter`|`elevenlabs`  |ElevenLabs API (speech only)                         |

`OllamaAdapter` reuses `OpenAiAdapter` for all translation logic; it differs only in
base URL and auth (none). It is not a separate code path, just a configured instance.

-----

### Rust Trait

```rust
/// Implemented by every provider adapter inside llm.svc.
/// All methods are pure translation — no I/O, no async.
pub trait ProviderAdapter: Send + Sync {
    /// Provider name as declared in llm.yaml (e.g. "anthropic")
    fn provider_name(&self) -> &str;

    /// Supported modalities for this adapter
    fn modalities(&self) -> &[Modality];

    // ── Text (llm/complete) ──────────────────────────────────────────────

    /// Translate a slice of Avix tool descriptors into the provider's
    /// native tools/functions array (JSON). Called once per llm/complete request.
    fn translate_tools(&self, tools: &[AvixToolDescriptor]) -> serde_json::Value;

    /// Build the complete HTTP request body for a text completion call.
    /// Includes translated tools, messages, model, temperature, etc.
    fn build_complete_request(&self, req: &AvixCompleteRequest) -> serde_json::Value;

    /// Parse the raw HTTP response body into a normalised AvixCompleteResponse.
    /// Extracts content blocks, usage, stop reason.
    fn parse_complete_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<AvixCompleteResponse, AdapterError>;

    /// Parse a tool call from a response content block into an AvixToolCall.
    /// Applies name unmangling (provider name → Avix name).
    fn parse_tool_call(
        &self,
        raw: &serde_json::Value,
    ) -> Result<AvixToolCall, AdapterError>;

    /// Format an AvixToolResult as a provider-native message for re-injection
    /// into the conversation history.
    fn format_tool_result(&self, result: &AvixToolResult) -> serde_json::Value;

    // ── Image (llm/generate-image) ───────────────────────────────────────

    /// Build the HTTP request body for an image generation call.
    /// Returns Err if this adapter does not support the image modality.
    fn build_image_request(
        &self,
        req: &AvixImageRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    /// Parse the raw image generation response.
    /// Returns image bytes; llm.svc writes them to scratch.
    fn parse_image_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<Vec<ImageOutput>, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    // ── Speech (llm/generate-speech) ────────────────────────────────────

    fn build_speech_request(
        &self,
        req: &AvixSpeechRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

    /// Speech responses are streaming bytes, not JSON.
    /// The adapter returns the HTTP endpoint + headers; llm.svc streams the body.
    fn speech_endpoint(&self, req: &AvixSpeechRequest) -> Result<SpeechEndpoint, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

    // ── Transcription (llm/transcribe) ───────────────────────────────────

    fn build_transcription_request(
        &self,
        req: &AvixTranscribeRequest,
        audio_bytes: &[u8],
    ) -> Result<MultipartRequest, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Transcription))
    }

    fn parse_transcription_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<AvixTranscribeResponse, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Transcription))
    }

    // ── Embedding (llm/embed) ────────────────────────────────────────────

    fn build_embed_request(
        &self,
        req: &AvixEmbedRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Embedding))
    }

    fn parse_embed_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<AvixEmbedResponse, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Embedding))
    }
}
```

Default implementations return `AdapterError::UnsupportedModality` for all non-text
modalities. Adapters only override the methods for modalities they actually support. The
compiler enforces that every adapter declares `modalities()` — if it claims to support
`Modality::Image` but hasn’t overridden `build_image_request`, the routing engine will
panic at boot during adapter self-check.

-----

### Adding a New Provider

To add a provider not covered by a built-in adapter:

1. Implement `ProviderAdapter` for the new provider’s wire format.
1. Register the adapter in `llm.svc`’s adapter registry at boot (keyed by provider
   name).
1. Add the provider entry to `llm.yaml`.

The adapter registry is a `HashMap<String, Box<dyn ProviderAdapter>>` populated at
`llm.svc` startup. The registry key is the `name` field from `llm.yaml`. If a provider
in `llm.yaml` has no registered adapter, `llm.svc` logs an error and marks that
provider `unavailable` at boot.

Built-in adapters are registered unconditionally. Installed service packages can ship
additional adapters as Rust dylibs loaded at boot — this is the extension path for
enterprise or community providers.

-----

## Tool Surface

`llm.svc` registers five inference tools and two introspection tools.

-----

### `llm/complete` — Text Completion

All agents with `llm:inference` capability.

**Input:**

```json
{
  "provider": "anthropic",        // optional — omit to use defaultProviders.text
  "model": "claude-sonnet-4",     // required
  "messages": [...],              // required — OpenAI-style messages array
  "system": "You are...",         // optional system prompt
  "maxTokens": 4096,              // optional — provider default if omitted
  "temperature": 0.7,             // optional — kernel.yaml models.temperature if omitted
  "stream": true,                 // optional — default false
  "stopSequences": [],            // optional
  "metadata": {                   // injected by RuntimeExecutor, not the agent
    "agentPid": 42,
    "sessionId": "sess-abc123"
  }
}
```

**Output (non-streaming):**

```json
{
  "provider": "anthropic",
  "model": "claude-sonnet-4",
  "content": [...],               // normalised Avix content blocks (provider-agnostic)
  "usage": {
    "inputTokens": 512,
    "outputTokens": 128,
    "totalTokens": 640
  },
  "stopReason": "end_turn",       // end_turn | max_tokens | stop_sequence | tool_use
  "latencyMs": 842
}
```

**Streaming:** when `stream: true`, returns `{ "job_id": "..." }` immediately. Content
delta events are emitted via `jobs.svc`. The RuntimeExecutor subscribes via `jobs/watch`.

-----

### `llm/generate-image` — Image Generation

Requires `llm:image` capability.

**Input:**

```json
{
  "provider": "stability-ai",     // optional — omit to use defaultProviders.image
  "model": "stable-diffusion-3",  // required
  "prompt": "A fox in a forest at dusk, oil painting style",
  "negativePrompt": "blurry, low quality",  // optional
  "size": "1024x1024",            // optional — provider default if omitted
  "style": "vivid",               // optional — provider-specific style hint
  "n": 1,                         // optional — number of images, default 1
  "metadata": { "agentPid": 42, "sessionId": "sess-abc123" }
}
```

**Output:**

```json
{
  "provider": "stability-ai",
  "model": "stable-diffusion-3",
  "images": [
    {
      "filePath": "/proc/42/scratch/img-001.png",
      "mimeType": "image/png",
      "size": "1024x1024",
      "bytes": 984320
    }
  ],
  "latencyMs": 4210
}
```

Output files land in the agent’s scratch directory (`/proc/<pid>/scratch/`). The agent
must copy them to a persistent path if they should survive the session.

-----

### `llm/generate-speech` — Speech Synthesis

Requires `llm:speech` capability.

**Input:**

```json
{
  "provider": "elevenlabs",       // optional — omit to use defaultProviders.speech
  "model": "eleven_multilingual_v2",
  "text": "Hello, this is the generated audio.",
  "voice": "Rachel",              // required — provider voice ID or name
  "format": "mp3",                // optional — mp3 | opus | wav, default mp3
  "speed": 1.0,                   // optional — 0.5–2.0, default 1.0
  "stream": false,                // optional — stream audio as it generates
  "metadata": { "agentPid": 42, "sessionId": "sess-abc123" }
}
```

**Output (non-streaming):**

```json
{
  "provider": "elevenlabs",
  "model": "eleven_multilingual_v2",
  "filePath": "/proc/42/scratch/speech-001.mp3",
  "mimeType": "audio/mpeg",
  "durationSec": 3.4,
  "bytes": 54272,
  "latencyMs": 1820
}
```

**Streaming:** when `stream: true`, returns `{ "job_id": "..." }` immediately. The
output file at the returned VFS path grows as audio data arrives. The agent can begin
reading the partial file before generation is complete.

-----

### `llm/transcribe` — Speech to Text

Requires `llm:transcription` capability. Input is a VFS file path; the agent must
already have the audio file on the filesystem before calling this tool.

**Input:**

```json
{
  "provider": "openai",           // optional — omit to use defaultProviders.transcription
  "model": "whisper-1",
  "filePath": "/users/alice/workspace/meeting.mp3",  // required — VFS path to audio file
  "language": "en",               // optional — ISO 639-1 code; auto-detect if omitted
  "prompt": "Avix, RuntimeExecutor, llm.svc",  // optional — hints for proper nouns
  "granularity": "segment",       // optional — word | segment | none, default segment
  "metadata": { "agentPid": 42, "sessionId": "sess-abc123" }
}
```

**Output:**

```json
{
  "provider": "openai",
  "model": "whisper-1",
  "text": "Hello everyone, let's start the meeting...",
  "language": "en",
  "durationSec": 183.4,
  "segments": [
    { "start": 0.0, "end": 2.1, "text": "Hello everyone," },
    { "start": 2.1, "end": 4.8, "text": "let's start the meeting." }
  ],
  "latencyMs": 2340
}
```

-----

### `llm/embed` — Embeddings

Requires `llm:embedding` capability. Text in, float vector out. Fits in a normal IPC
response — no file output needed.

**Input:**

```json
{
  "provider": "openai",           // optional — omit to use defaultProviders.embedding
  "model": "text-embedding-3-small",
  "input": "The quick brown fox", // required — string or array of strings
  "metadata": { "agentPid": 42, "sessionId": "sess-abc123" }
}
```

**Output:**

```json
{
  "provider": "openai",
  "model": "text-embedding-3-small",
  "embeddings": [
    [0.0123, -0.4567, 0.8901, ...]   // one vector per input string
  ],
  "dimensions": 1536,
  "usage": { "inputTokens": 6 },
  "latencyMs": 88
}
```

-----

### `llm/providers` — Provider Introspection

Returns the current list of configured providers, their supported modalities, and live
health status. Available to all agents with any `llm:*` capability.

**Output:**

```json
{
  "providers": [
    {
      "name": "anthropic",
      "status": "available",
      "modalities": ["text"],
      "models": ["claude-opus-4", "claude-sonnet-4", "claude-haiku-4"],
      "authType": "api_key",
      "lastHealthCheckMs": 310,
      "lastError": null
    },
    {
      "name": "stability-ai",
      "status": "degraded",
      "modalities": ["image"],
      "models": ["stable-diffusion-3", "stable-image-ultra"],
      "authType": "api_key",
      "lastHealthCheckMs": 8200,
      "lastError": "503 on /v1/engines/list — retrying"
    },
    {
      "name": "ollama",
      "status": "unavailable",
      "modalities": ["text", "embedding"],
      "models": [],
      "authType": "none",
      "lastHealthCheckMs": null,
      "lastError": "connection refused on localhost:11434"
    }
  ],
  "defaultProviders": {
    "text": "anthropic",
    "image": "openai",
    "speech": "elevenlabs",
    "transcription": "openai",
    "embedding": "openai"
  }
}
```

-----

### `llm/usage` — Usage Stats

Returns aggregated usage stats for the current session, broken down by provider and
modality. Available to all agents with any `llm:*` capability.

**Output:**

```json
{
  "sessionId": "sess-abc123",
  "byProvider": {
    "anthropic": {
      "text": { "inputTokens": 12000, "outputTokens": 3400, "requests": 18 }
    },
    "openai": {
      "image":     { "requests": 3 },
      "embedding": { "inputTokens": 840, "requests": 12 }
    },
    "elevenlabs": {
      "speech": { "charactersGenerated": 4200, "requests": 5 }
    }
  }
}
```

-----

## Provider Routing

Resolution order for every tool call:

```
1. Provider explicitly specified in tool call params?
     → validate it supports the required modality
     → validate agent holds llm:<modality>::<provider> or unscoped llm:<modality>
     → use it

2. No provider specified:
     → look up defaultProviders[modality] in llm.yaml
     → use that provider

3. Resolved provider is `degraded`:
     → check kernel.yaml models.fallback provider (only applies to text modality)
     → use fallback if available and healthy
     → else proceed with degraded provider (it may still succeed)

4. Resolved provider is `unavailable`:
     → return error -32010 "No provider available for modality: <modality>"
```

Fallback currently applies to `text` modality only, since `kernel.yaml models.fallback`
is a text model name. For other modalities, operators configure redundancy by having
multiple providers declare the same modality and choosing which is the `defaultProviders`
entry.

-----

## Agent Manifest Integration

An agent declares modality needs and provider preferences in its manifest:

```yaml
spec:
  entrypoint:
    type: llm-loop
    modelPreference: claude-sonnet-4
    providerPreference: anthropic        # optional — overrides defaultProviders.text

  capabilities:
    required:
      - llm:inference                    # text completion
      - llm:image                        # image generation
    optional:
      - llm:speech                       # speech synthesis (granted if available)
```

`providerPreference` applies to `llm:inference` (text) only. For other modalities the
agent passes `provider` explicitly on the tool call, or the system default is used.

**Resolution chain at spawn time (text provider):**

```
agent manifest: providerPreference
  → if absent: kernel.yaml models.defaultProvider
  → if absent: llm.yaml spec.defaultProviders.text
```

-----

## kernel.yaml Changes

The `models` block gains a `defaultProvider` field pointing into `llm.yaml`:

```yaml
models:
  default: claude-sonnet-4
  kernel: claude-opus-4
  fallback: claude-haiku-4
  defaultProvider: anthropic             # NEW — must match a provider name in llm.yaml
  temperature: 0.7
```

`defaultProvider` in `kernel.yaml` is used only for the kernel’s own LLM calls
(`models.kernel`, `models.fallback`). Agent-facing defaults are resolved from
`llm.yaml spec.defaultProviders`.

The kernel holds `llm:inference::kernel` — a privileged scoped capability that bypasses
per-agent quotas. It is never grantable to user agents.

-----

## Boot Sequence

`llm.svc` starts in Phase 3, after `auth.svc` and `memfs.svc` but before `kernel.agent`:

```
Phase 3 boot order (excerpt):
  ...
  5. tool-registry.svc
  6. llm.svc              ← new position
  7. exec.svc
  8. mcp-bridge.svc
  9. gateway.svc
  10. kernel.agent        ← depends on llm.svc being ready
```

**`llm.svc` startup sequence:**

```
1. Read /etc/avix/llm.yaml
2. Validate defaultProviders — each modality entry must name a provider that declares
   that modality in its modalities list. Error on mismatch → abort boot.
3. For each provider with auth.type != none:
     a. Request secret injection from kernel (kernel/secrets/inject)
     b. Kernel decrypts /secrets/services/llm/<secretName>.enc
     c. Kernel injects plaintext into llm.svc memory via IPC response
     d. llm.svc stores in-memory credential map — never logged, never written to disk
4. For oauth2 providers: start background token-refresh task
5. Run initial health checks against all providers (parallel, non-blocking)
6. Register tools: llm/complete, llm/generate-image, llm/generate-speech,
                   llm/transcribe, llm/embed, llm/providers, llm/usage
7. ipc.register with kernel
8. Mark tools available / degraded / unavailable per health check results
9. Emit tool.changed for any provider that is not available
```

If `/etc/avix/llm.yaml` does not exist or has no providers, `llm.svc` starts with all
tools `unavailable`. `kernel.agent` starts in LLM-optional mode.

-----

## Scratch Directory — Binary Output Files

Tools that produce binary output (`llm/generate-image`, `llm/generate-speech`) write
files to the calling agent’s scratch directory.

**Path pattern:** `/proc/<pid>/scratch/<tool>-<seq>.<ext>`

```
/proc/42/scratch/img-001.png
/proc/42/scratch/img-002.png
/proc/42/scratch/speech-001.mp3
```

**Lifetime:** scratch files are deleted when the agent session ends — on `SIGKILL`,
`SIGSTOP`, or clean session close. If the file should survive the session, the agent
must copy it to a persistent path before the session closes (e.g., via `fs/copy`
to `/users/alice/workspace/`).

**Ownership:** scratch files are owned by the agent’s UID. Other agents cannot read them
without an explicit `fs/share` grant from the owning agent.

-----

## Runtime State — /proc/services/llm/

`llm.svc` writes read-only VFS files generated on-demand from live in-memory state.

```
/proc/services/llm/
  status.yaml                    — service health summary
  providers/
    anthropic.yaml
    openai.yaml
    stability-ai.yaml
    elevenlabs.yaml
    ollama.yaml
  usage/
    total.yaml                   — system-wide usage since boot, by provider + modality
```

Example `/proc/services/llm/providers/openai.yaml`:

```yaml
name: openai
status: available
authType: oauth2
modalities: [text, image, speech, transcription, embedding]
models:
  text:          [gpt-4o, gpt-4o-mini]
  image:         [dall-e-3]
  speech:        [tts-1, tts-1-hd]
  transcription: [whisper-1]
  embedding:     [text-embedding-3-small, text-embedding-3-large]
health:
  lastCheckedAt: 2026-03-21T10:00:00Z
  lastLatencyMs: 210
  consecutiveFailures: 0
  lastError: null
  tokenExpiresAt: 2026-03-21T11:00:00Z   # oauth2 only
usage:
  byModality:
    text:          { requests: 44,  inputTokens: 58000, outputTokens: 12000, errors: 0 }
    image:         { requests: 3,   errors: 0 }
    speech:        { requests: 5,   charactersGenerated: 4200, errors: 0 }
    transcription: { requests: 2,   audioSecondsProcessed: 386, errors: 0 }
    embedding:     { requests: 12,  inputTokens: 840, errors: 0 }
rateLimit:
  requestsPerMinute: 60
  currentWindowRequests: 7
  tokensPerMinute: 200000
  currentWindowTokens: 9400
```

-----

## Capability Token Changes

Each modality has its own capability scope, with an optional provider sub-scope.

|Capability                |Grants                                                               |
|--------------------------|---------------------------------------------------------------------|
|`llm:inference`           |`llm/complete` on any provider; routing follows defaultProviders.text|
|`llm:inference::anthropic`|`llm/complete` on Anthropic only                                     |
|`llm:inference::ollama`   |`llm/complete` on local Ollama only                                  |
|`llm:inference::kernel`   |Reserved for kernel-internal use; bypasses per-agent quotas          |
|`llm:image`               |`llm/generate-image` on defaultProviders.image                       |
|`llm:image::stability-ai` |`llm/generate-image` on Stability AI only                            |
|`llm:speech`              |`llm/generate-speech` on defaultProviders.speech                     |
|`llm:speech::elevenlabs`  |`llm/generate-speech` on ElevenLabs only                             |
|`llm:transcription`       |`llm/transcribe` on defaultProviders.transcription                   |
|`llm:embedding`           |`llm/embed` on defaultProviders.embedding                            |

**Rules:**

- An unscoped grant (e.g., `llm:image`) allows the agent to use the system default
  provider for that modality only. It cannot name an arbitrary provider.
- A provider-scoped grant (e.g., `llm:image::stability-ai`) allows naming that specific
  provider. Naming any other provider is rejected with `-32018`.
- `llm:inference::kernel` is never grantable to user agents.
- Agents that do not hold a modality capability cannot call that modality’s tool. The
  router rejects the call before it reaches `llm.svc`.

-----

## service.unit

```yaml
name: llm
binary: avix --service=llm

[unit]
description: AI model provider connection manager — text, image, speech, transcription, embedding
requires: [router, auth, memfs, logger]
after: [auth, memfs]

[service]
restart: on-failure
restart_delay: 3s

[capabilities]
required: [fs:read, fs:write, secrets:read, llm:inference::kernel]
scope: /etc/avix/llm.yaml, /proc/services/llm/, /secrets/services/llm/, /proc/*/scratch/

[network]
# llm.svc makes outbound HTTPS calls to provider APIs.
# No inbound ports — all access is via IPC only.
outbound:
  - https://api.anthropic.com
  - https://api.openai.com
  - https://auth.openai.com
  - https://api.stability.ai
  - https://api.elevenlabs.io
  - http://localhost:11434           # ollama — localhost only

[concurrency]
max_concurrent: 100
queue_max: 200
queue_timeout: 30s
```

-----

## CLI

```bash
# View all provider status across all modalities
avix llm status

# Test a specific provider (runs a minimal real call per declared modality)
avix llm test anthropic
avix llm test stability-ai

# Rotate an API key (re-encrypts in /secrets/, SIGHUP reloads credential in llm.svc)
avix llm rotate anthropic

# List all models, grouped by modality
avix llm models

# List models for a specific modality
avix llm models --modality image

# View usage since boot
avix llm usage

# Add a new provider interactively
avix llm provider add

# Disable a provider without removing it from config
avix llm provider disable stability-ai

# Re-enable
avix llm provider enable stability-ai

# Change the default provider for a modality
avix llm default set image stability-ai
```

-----

## Error Codes

|Code  |Message                           |Meaning                                                      |
|------|----------------------------------|-------------------------------------------------------------|
|-32010|No provider available for modality|All providers for this modality are unavailable              |
|-32011|Provider not found                |Specified `provider` name not in `llm.yaml`                  |
|-32012|Model not found on provider       |Requested model not in provider’s model list                 |
|-32013|Modality not supported by provider|Provider exists but doesn’t support this modality            |
|-32014|Provider auth failed              |Credential rejected by provider (401/403)                    |
|-32015|Provider rate limited             |429 received; `retry_after` in error `data`                  |
|-32016|Inference timeout                 |Provider did not respond within `readMs`                     |
|-32017|Token quota exceeded              |Agent or system token quota reached                          |
|-32018|Capability denied                 |Agent lacks required `llm:<modality>` capability             |
|-32019|Scratch write failed              |Could not write binary output to `/proc/<pid>/scratch/`      |
|-32020|Input file not found              |`filePath` supplied to `llm/transcribe` does not exist in VFS|

-----

## Open Questions

1. **Per-user provider budgets** — should `llm.yaml` support per-provider token budgets
   per user, or is that handled by the existing `quota.tokens` in `users.yaml`? Current
   stance: defer to `users.yaml`; `llm.svc` enforces it at call time by checking
   `/proc/<pid>/resolved.yaml`.
1. **Multi-modal input (vision)** — `llm/complete` currently takes a text messages array.
   Supporting image inputs (vision models like `gpt-4o` or `claude-3`) requires messages
   to carry image content blocks. Vision is still `llm/complete`, just with richer message
   content — no new tool needed. Spec update deferred to v1.1.
1. **Azure OpenAI** — uses a different base URL scheme and requires an `api-version`
   query param. Add as a v1 provider or handle as an `openai` variant with extra config
   fields? Deferred to v1.1.
1. **Voice-to-voice pipeline** — combining `llm/transcribe` + `llm/complete` +
   `llm/generate-speech` covers a full voice pipeline today (three tool calls). A future
   `llm/voice-chat` tool could pipeline these internally for lower latency. Deferred.

-----

## Related Documents

- [KernelConfig](./kernel-config.md) — `models.defaultProvider` field
- [AgentManifest](./agent-manifest.md) — `providerPreference` and modality capability fields
- [Capability Token](./capability-token.md) — `llm:<modality>::<provider>` scoped caps
- [Secrets Store](../filesystem.md#6-secrets-store) — credential encryption model
- [Jobs](./jobs.md) — streaming inference and streaming audio via `jobs.svc`
- [Service Authoring Guide](../service-authoring.md) — IPC startup contract
