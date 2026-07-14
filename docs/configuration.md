# Environment Variable Reference

**English** | [日本語](ja/configuration.md)

The full list of variables, with comments, lives in [`.env.example`](../.env.example).
Variables are passed to the server **as process environment variables** — there is no
mechanism that automatically reads a `.env` file. Set them via `environment:` in
docker compose, `docker compose exec -e`, `Environment=` in systemd, or similar.

## Core

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string (required) |
| `YSR_BIND` | Listen address (default: `0.0.0.0:8080`) |
| `YSR_CORS_ORIGINS` | Comma-separated list of allowed origins for browser access (e.g. so a browser-based dashboard on a different origin can call `/auth/login`/`/api/members`). Cross-origin reads are disabled if unset |
| `YORISHIRO_MAX_TENANTS` | Deployment-wide cap on the number of tenants `admin create-tenant` may create. Unset (default) means unlimited. Set this to `1` for a single-tenant deployment; leave it unset to allow multiple tenants. `POST /auth/signup` never creates a tenant (it only redeems an invite into an *existing* one), so it is unaffected. This is also what gates the first-run setup wizard (`GET`/`POST /setup`, see [setup.md](setup.md#first-run-setup)) — it's disabled on any deployment where this is unset |
| `YSR_WEB_DIR` | Directory the setup/login web UI's static files are served from at `/`. Unset (default) disables the web UI entirely; falls back to serving `/api/*`, `/mcp`, and `/docs` only. `docker-compose.yml`'s `app` service sets this to `web` |
| `YSR_AUTH_RATE_LIMIT_MAX` / `YSR_AUTH_RATE_LIMIT_WINDOW_SECS` | Per-client-IP rate limit on `/auth/signup`, `/auth/login`, and `/setup` — the endpoints reachable without a bearer token, and therefore the only ones an unauthenticated caller can brute-force. Defaults: 10 requests per 60 seconds |
| `RUST_LOG` | Log level (e.g. `info`) |

## Logging

Every log line, including the HTTP access log (method, path, status, latency), is a JSON
object. `YSR_LOG_TARGET` selects where those lines go:

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
| `YSR_EMBEDDING_PROVIDER` | `openai` (default) or `local` |
| `YSR_EMBEDDING_DIMENSIONS` | Since `entities.embedding` is fixed at `vector(768)`, any value other than 768 causes a startup error |

### When `YSR_EMBEDDING_PROVIDER=openai` (e.g. Ollama, LM Studio, OpenAI)

| Variable | Description |
|---|---|
| `YSR_EMBEDDING_BASE_URL` | Base URL of the `/v1/embeddings`-compatible endpoint |
| `YSR_EMBEDDING_MODEL` | Model name |
| `YSR_EMBEDDING_API_KEY` | API key, if required by the endpoint |
| `YSR_EMBEDDING_SEND_DIMENSIONS_PARAM` | Whether to include a `dimensions` parameter in the request body (set `false` for servers that don't support it) |

### When `YSR_EMBEDDING_PROVIDER=local` (768-dimensional BERT-family ONNX export)

| Variable | Description |
|---|---|
| `YSR_ONNX_MODEL_PATH` | Path to the ONNX model, e.g. `models/model.onnx` |
| `YSR_ONNX_TOKENIZER_PATH` | Path to the tokenizer, e.g. `models/tokenizer.json` |
| `YSR_ONNX_MAX_SEQUENCE_LENGTH` | Maximum sequence length (default: `512`) |

See [docs/embedding-providers.md](embedding-providers.md) for a worked example, e.g.
`https://huggingface.co/Xenova/all-mpnet-base-v2` (`onnx/model_quantized.onnx` and
`tokenizer.json`).

