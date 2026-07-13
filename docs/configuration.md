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
| `YSR_CORS_ORIGINS` | Comma-separated list of allowed origins for browser access. Cross-origin reads are disabled if unset |
| `RUST_LOG` | Log level (e.g. `info`) |

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
