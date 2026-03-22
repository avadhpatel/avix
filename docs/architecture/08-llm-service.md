# 08 — llm.svc: Multi-Modality LLM Service

## Overview

`llm.svc` is the sole AI inference gateway in Avix. It owns all provider credentials,
routes requests by modality, translates tool names between Avix format (`/`) and wire
format (`__`), and handles binary outputs (images, audio, video). No other component in
Avix calls a provider API directly — all AI inference flows through `llm.svc` via IPC.

This design centralises credential management, enables transparent provider routing,
and provides a single point for observability (token counters, latency, error rates).

---

## Configuration (`/etc/avix/llm.yaml`)

```yaml
kind: LlmConfig
version: 1
providers:
  anthropic:
    kind: Anthropic
    credential:
      type: api_key
      key_env: ANTHROPIC_API_KEY
    models:
      - claude-opus-4-5
      - claude-sonnet-4-5
      - claude-haiku-3-5
    modalities: [text, vision, document]

  openai:
    kind: OpenAI
    credential:
      type: api_key
      key_env: OPENAI_API_KEY
    models:
      - gpt-4o
      - gpt-4-turbo
      - text-embedding-3-large
    modalities: [text, embedding]

  ollama:
    kind: Ollama
    base_url: http://localhost:11434
    credential:
      type: none          # local inference, no auth required
    models:
      - llama3
      - mistral
    modalities: [text]

  stability:
    kind: StabilityAI
    credential:
      type: api_key
      key_env: STABILITY_API_KEY
    models:
      - stable-diffusion-xl-1024-v1-0
    modalities: [image]

  elevenlabs:
    kind: ElevenLabs
    credential:
      type: api_key
      key_env: ELEVENLABS_API_KEY
    models:
      - eleven_multilingual_v2
    modalities: [speech]

defaultProviders:
  text: anthropic
  vision: anthropic
  document: anthropic
  embedding: openai
  image: stability
  speech: elevenlabs
```

### Key fields

- **`providers`**: Named map of provider configs. Each entry specifies kind, credentials,
  available models, and supported modalities.
- **`defaultProviders`**: Maps each modality to a provider name. The routing engine
  consults this map when the caller does not specify an explicit provider.
- **`modalities`**: Enum — `text`, `vision`, `document`, `embedding`, `image`, `speech`.

---

## Providers

### Anthropic
- Modalities: `text`, `vision`, `document`
- Models: `claude-*` family
- Auth: `api_key` via `ANTHROPIC_API_KEY`

### OpenAI
- Modalities: `text`, `embedding`
- Models: `gpt-*`, `text-embedding-*`
- Auth: `api_key` via `OPENAI_API_KEY`

### Ollama
- Modality: `text`
- Models: any locally-served model (`llama3`, `mistral`, etc.)
- Auth: `none` — local inference, no credentials required
- Base URL defaults to `http://localhost:11434`

### Stability AI
- Modality: `image`
- Models: `stable-diffusion-*`
- Auth: `api_key` via `STABILITY_API_KEY`

### ElevenLabs
- Modality: `speech`
- Models: `eleven_multilingual_v2`, `eleven_monolingual_v1`
- Auth: `api_key` via `ELEVENLABS_API_KEY`

---

## Authentication

Three credential types are recognised:

| Type      | Description                                        |
|-----------|----------------------------------------------------|
| `api_key` | Static key loaded from an environment variable     |
| `oauth2`  | Bearer token + refresh URL; background auto-renew  |
| `none`    | No credentials (Ollama and other local providers)  |

`credential.type: none` is valid **only** for providers that do not require auth (e.g.,
Ollama). For all remote provider configs, `api_key` or `oauth2` is mandatory.

Credentials are loaded into `llm.svc` memory at startup. They are never written to VFS
or logged.

---

## Routing Engine

1. Caller invokes `llm/complete` (or other `llm/*` tool) specifying a `modality` field.
2. If the request includes an explicit `provider` field, that provider is used directly
   (after validating it supports the requested modality).
3. Otherwise, `defaultProviders[modality]` is looked up. If no entry exists, error
   `-32010` is returned.
4. The selected provider must be healthy (not rate-limited, credentials valid). If
   unhealthy, `llm.svc` returns error `-32013` or `-32012` as appropriate.

### Routing conflict

If two providers are registered as defaults for the same modality, `llm.svc` fails to
start and returns error `-32019` (routing conflict). The `llm.yaml` must be corrected
and `llm.svc` restarted.

---

## Tool Name Mangling

Avix tool names use `/` as the namespace separator (e.g., `fs/read`). Provider APIs
such as Anthropic and OpenAI require `__` as the separator (e.g., `fs__read`). This
translation happens **exclusively** at the `llm.svc` boundary.

- Outbound (Avix → provider): `/` → `__`
- Inbound (provider → Avix): `__` → `/`

`RuntimeExecutor` always works with unmangled Avix names. No Avix tool name ever
contains `__` — that string is reserved for wire encoding only (ADR-03).

The `ToolName::parse` function rejects any name containing `__`, ensuring the invariant
is enforced at the type level.

---

## Binary Output Handling

Image, audio, and video responses cannot be transported over IPC as raw bytes (IPC uses
JSON-RPC 2.0 with 4-byte framing). Instead:

1. `llm.svc` writes the binary payload to a scratch path:
   `/tmp/llm-outputs/<job-id>.<ext>`
2. The IPC response contains only the VFS path, not the binary data.
3. The caller (e.g., `RuntimeExecutor`) reads the file via `fs/read` when needed.
4. Scratch files are cleaned up by `llm.svc` after a configurable TTL (default: 1 hour).

If the write fails (disk full, permissions), error `-32014` is returned and no VFS path
is included in the response.

---

## OAuth2 Refresh

For providers configured with `credential.type: oauth2`:

1. A background `tokio` task runs inside `llm.svc`.
2. It monitors the token's `expires_at` timestamp.
3. **5 minutes before expiry**, it fires a refresh request to the provider's `token_url`.
4. On success, the new token replaces the old one in memory.
5. On failure, the task retries with exponential back-off (1 s, 2 s, 4 s … up to 60 s).
6. If the token expires without a successful refresh, subsequent calls to that provider
   return error `-32017` (oauth2 refresh failed).

On `SIGHUP`, `llm.svc` re-reads `llm.yaml` and reloads all provider configurations and
credentials without a full restart.

---

## Health Checks

`llm.svc` tracks provider health continuously:

- Each provider is polled at a configurable interval (default: 30 s) via a lightweight
  ping request.
- When a provider becomes **unhealthy** (ping timeout, credential error), `llm.svc`
  emits a `tool.changed` event on the IPC event bus.
- When a provider **recovers**, another `tool.changed` event is emitted.
- `RuntimeExecutor` subscribes to `tool.changed` events and refreshes the tool list it
  presents to the LLM, ensuring the LLM never attempts to call a tool backed by a
  degraded provider.

---

## Capability Scopes

Callers must hold the appropriate capability to invoke `llm/*` tools:

| Scope           | Grants access to                              |
|-----------------|-----------------------------------------------|
| `llm:inference` | `llm/complete`, `llm/embed`, `llm/status`, `llm/models`, `llm/usage` |
| `llm:image`     | `llm/image`                                   |
| `llm:speech`    | `llm/speech`, `llm/transcribe`                |
| `llm:embedding` | `llm/embed`                                   |
| `llm:admin`     | provider add / disable / credential rotation  |

Capabilities are enforced by `auth.svc` when the `CapabilityToken` is validated, and
again by `llm.svc` on every tool call.

---

## Error Codes

All errors are returned as JSON-RPC 2.0 error objects. `llm.svc`-specific codes occupy
the `-32010` to `-32029` range.

| Code     | Name                        | Description                                      |
|----------|-----------------------------|--------------------------------------------------|
| `-32010` | `ProviderNotFound`          | Named provider does not exist in config          |
| `-32011` | `ModalityNotSupported`      | Provider does not support the requested modality |
| `-32012` | `CredentialInvalid`         | API key or token is invalid or expired           |
| `-32013` | `ProviderRateLimited`       | Provider returned 429 / quota exhausted          |
| `-32014` | `BinaryOutputWriteFailed`   | Could not write image/audio to scratch dir       |
| `-32015` | `ContextLengthExceeded`     | Message history exceeds model context window     |
| `-32016` | `ContentPolicyViolation`    | Provider refused content (safety filter)         |
| `-32017` | `OAuth2RefreshFailed`       | Background token refresh failed                  |
| `-32018` | `ProviderTimeout`           | Provider did not respond within timeout          |
| `-32019` | `RoutingConflict`           | Two providers claim the same default modality    |
| `-32020` | `NotInitialized`            | `llm.svc` has not completed initialisation       |

---

## IPC Interface

`llm.svc` exposes the following tools under the `llm/` namespace. All are callable via
IPC (JSON-RPC 2.0, 4-byte length-prefixed framing, fresh connection per call — ADR-05).

| Tool              | Capability scope  | Description                                             |
|-------------------|-------------------|---------------------------------------------------------|
| `llm/complete`    | `llm:inference`   | Text, vision, and document completion                   |
| `llm/embed`       | `llm:embedding`   | Produce a vector embedding for the given input          |
| `llm/image`       | `llm:image`       | Generate an image from a text prompt                    |
| `llm/speech`      | `llm:speech`      | Convert text to speech audio                            |
| `llm/transcribe`  | `llm:speech`      | Transcribe audio to text (speech-to-text)               |
| `llm/status`      | `llm:inference`   | Return health summary for all configured providers      |
| `llm/models`      | `llm:inference`   | List available models, optionally filtered by modality  |
| `llm/usage`       | `llm:inference`   | Return cumulative token and request counters            |

### `llm/complete` request schema (abbreviated)

```json
{
  "modality": "text",
  "provider": "anthropic",          // optional; overrides defaultProviders
  "model": "claude-sonnet-4-5",     // optional; provider default used if absent
  "messages": [...],
  "tools": [...],                   // Avix tool schemas (unmangled names)
  "max_tokens": 4096,
  "temperature": 0.7
}
```

### `llm/complete` response schema (abbreviated)

```json
{
  "stop_reason": "tool_use",        // "end_turn" | "tool_use" | "max_tokens"
  "content": [...],
  "usage": { "input_tokens": 512, "output_tokens": 128 }
}
```

Long-running requests (e.g., large image generation) return a `job_id` immediately.
Progress is emitted via `jobs.svc`. The caller polls using `job/watch`.
