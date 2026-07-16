# Environment Variable Reference

**English** | [日本語](ja/configuration.md)

The full list of variables, with comments, lives in [`.env.example`](../.env.example). Variables are passed to the server **as process environment variables** -- there is no mechanism that automatically reads a `.env` file. Set them via `environment:` in docker compose, `docker compose exec -e`, `Environment=` in systemd, or similar.

## config.yml

Every setting below can also go in a `config.yml` file instead. See [`config.example.yml`](../config.example.yml) for the full key list (nested under `embedding:`, `logging:`, and `auth_rate_limit:` for those groups). By default the server looks for `config.yml` in its working directory; set `YSR_CONFIG_PATH` to point elsewhere.

A missing file, or a missing key within it, is not an error -- that setting just falls back to its usual default. **A set environment variable always wins over the equivalent `config.yml` key.**

This makes `config.yml` convenient as the base configuration for a deployment, with environment variables reserved for one-off overrides (e.g. a Docker `-e` flag for a single run) rather than the only way to configure anything.

## Core

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string (required) |
| `YSR_BIND` | Listen address (default: `0.0.0.0:8080`) |
| `YSR_CORS_ORIGINS` | Comma-separated list of allowed origins for browser access (e.g. so a browser-based dashboard on a different origin can call `/auth/login`/`/api/members`). Cross-origin reads are disabled if unset. In debug builds only, leaving this unset also auto-allows any `http://localhost:*`/`http://127.0.0.1:*` origin (for browser-based dev tools like the MCP Inspector) -- release builds never do this |
| `YORISHIRO_MAX_TENANTS` | Deployment-wide cap on tenants `admin create-tenant` may create. Defaults to `1` (single-tenant). Set `0` for unlimited, or a higher number for that many. `POST /auth/signup` never creates a tenant (it just redeems an invite), so it's unaffected. Also gates the first-run setup wizard (see [setup.md](setup.md#first-run-setup)), enabled only when the cap isn't `0` |
| `YSR_WEB_DIR` | The setup/login/workspace-management web UI's static files are compiled into the binary and served at `/` by default. Set this to serve them from a real directory on disk instead (e.g. to iterate on `web/` without rebuilding) |
| `YSR_AUTH_RATE_LIMIT_MAX` / `YSR_AUTH_RATE_LIMIT_WINDOW_SECS` | Per-client-IP rate limit on `/auth/signup`, `/auth/login`, and `/setup` — the endpoints reachable without a bearer token, and therefore the only ones an unauthenticated caller can brute-force. Defaults: 10 requests per 60 seconds |
| `RUST_LOG` | Log level (e.g. `info`) |

## Request correlation

Every response carries an `x-request-id` header -- a UUID the server generates if the request didn't already have one, otherwise the caller's own value is echoed back unchanged. The same value tags the tracing span for that request, so any `warn`/`error` line logged while handling it (an authentication rejection, a rate-limit hit, an internal error) carries the same `request_id` field as the access log line for that request. Useful for tying a specific failed request to its server-side log lines when following up on an incident report.

Rejected requests (bad/missing API key, insufficient scope, rate limit exceeded) are logged at `warn` with the caller's IP and the request path, but never the presented credential -- previously these surfaced only as an anonymous 401/403/429 in the access log.

## Logging

Every log line, including the HTTP access log (method, path, status, latency), is a JSON object. `YSR_LOG_TARGET` selects where those lines go:

| Variable | Description |
|---|---|
| `YSR_LOG_TARGET` | `stdout` (default, for a container runtime's log driver), `single` (one file, never rotated), `daily` (one file per day), or `syslog` |

### When `YSR_LOG_TARGET=single` or `daily`

| Variable | Description |
|---|---|
| `YSR_LOG_DIR` | Directory the log file is written under (default: `.`). The file is named `yorishiro.log`, with the date appended for `daily` (e.g. `yorishiro.log.2026-07-13`) |

### When `YSR_LOG_TARGET=syslog`

| Variable | Description |
|---|---|
| `YSR_SYSLOG_SOCKET` | Unix domain socket to send RFC 3164-framed messages to (default: `/dev/log`). Linux/Unix only |

## Embedding provider

| Variable | Description |
|---|---|
| `YSR_EMBEDDING_PROVIDER` | `local` (default) or `openai` |
| `YSR_EMBEDDING_DIMENSIONS` | Since `entities.embedding` is fixed at `vector(768)`, any value other than 768 causes a startup error |

### When `YSR_EMBEDDING_PROVIDER=local` (768-dimensional BERT-family ONNX export, the default)

| Variable | Description |
|---|---|
| `YSR_ONNX_MODEL_PATH` | Path to the ONNX model (default: `models/model.onnx`) |
| `YSR_ONNX_TOKENIZER_PATH` | Path to the tokenizer (default: `models/tokenizer.json`) |
| `YSR_ONNX_MAX_SEQUENCE_LENGTH` | Maximum sequence length (default: `512`) |

### When `YSR_EMBEDDING_PROVIDER=openai` (e.g. Ollama, LM Studio, OpenAI)

| Variable | Description |
|---|---|
| `YSR_EMBEDDING_BASE_URL` | Base URL of the `/v1/embeddings`-compatible endpoint (required) |
| `YSR_EMBEDDING_MODEL` | Model name (required) |
| `YSR_EMBEDDING_API_KEY` | API key, if required by the endpoint |
| `YSR_EMBEDDING_SEND_DIMENSIONS_PARAM` | Whether to include a `dimensions` parameter in the request body (set `false` for servers that don't support it) |

See [docs/embedding-providers.md](embedding-providers.md) for a worked example, e.g. `https://huggingface.co/Xenova/all-mpnet-base-v2` (`onnx/model_quantized.onnx` and `tokenizer.json`).
