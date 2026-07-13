# Setup

**English** | [日本語](ja/setup.md)

## Prerequisites and startup

Prerequisites: Docker / Docker Compose. `docker compose up` starts PostgreSQL and a
development container (`app`, with the Rust toolchain); the server itself is run with
`cargo run` inside that container.

Configuring an embedding provider is required before the server will start. Here is a
fully local example that needs no external service:

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro

# Place a 768-dimensional BERT-family ONNX model (see embedding-providers.md)
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json

$ docker compose up -d --build
$ docker compose exec \
    -e YSR_EMBEDDING_PROVIDER=local \
    -e YSR_ONNX_MODEL_PATH=models/model.onnx \
    -e YSR_ONNX_TOKENIZER_PATH=models/tokenizer.json \
    app cargo run -p yorishiro-server
```

Migrations are applied automatically on startup. Endpoints:

| Path | Description |
|---|---|
| `http://localhost:8080/health` | Health check |
| `http://localhost:8080/docs` | Swagger UI (REST API documentation) |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI specification |
| `http://localhost:8080/mcp` | MCP endpoint (Streamable HTTP) |
| `http://localhost:8080/whoami` | Authentication check (returns tenant and scope) |

## Provisioning tenants and API keys

API keys are stored in the database only as SHA-256 hashes, so they must be issued through
the admin CLI (there is no way to issue one via manual SQL):

```console
$ docker compose exec app cargo run -q -p yorishiro-server -- admin create-tenant my-team
tenant created
  id:   019f565d-f1e3-7afb-b876-b7003e43c230
  name: my-team

$ docker compose exec app cargo run -q -p yorishiro-server -- admin create-api-key \
    019f565d-f1e3-7afb-b876-b7003e43c230 write
api key created (the plaintext key is shown ONLY once — store it now)
  key:       ysr_928e48292888_ef72...
  ...

$ docker compose exec app cargo run -q -p yorishiro-server -- admin list-tenants
```

The plaintext key is shown only once, at issuance time. Admin commands access the database
directly using the connection role from `DATABASE_URL` (the same administrative role used
for migrations).

Other admin commands:

| Command | Description |
|---|---|
| `admin list-api-keys <tenant-id>` | List keys (ID, scope, prefix, last used) |
| `admin revoke-api-key <key-id>` | Immediately revoke a key (e.g. on leakage) |
| `admin resync-embeddings <tenant-id>` | Re-sync entities missing an embedding (recovery from a failed sync) |

## Authentication and scopes

All APIs authenticate via `Authorization: Bearer <api-key>`. Keys are strings starting with
`ysr_`, shown only once at issuance time (only a SHA-256 hash is stored in the database).

Scopes form a three-level hierarchy: `read` < `write` < `schema`. A `write` key can also
read, and a `schema` key can perform every operation, including schema registration.
