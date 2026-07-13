# Setup

**English** | [日本語](ja/setup.md)

## Prerequisites and startup

Prerequisites: Docker / Docker Compose / make. `make init` builds the images and starts
PostgreSQL plus `app`, a container running the actual release binary from the multi-stage
`Dockerfile` at the repo root (the same image used in production).

Configuring an embedding provider is required before the server will start. `docker-compose.yml`
already points `app` at the local ONNX provider; here's how to fetch a model for it (needs no
external service):

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro

# Place a 768-dimensional BERT-family ONNX model (see embedding-providers.md)
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json

$ make init
```

Migrations are applied automatically on startup. Endpoints:

| Path | Description |
|---|---|
| `http://localhost:8080/up` | Liveness probe (always 200 if the process is running; no dependency checks) |
| `http://localhost:8080/health` | Readiness check (also probes DB connectivity; 503 on outage) |
| `http://localhost:8080/docs` | Swagger UI (REST API documentation) |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI specification |
| `http://localhost:8080/mcp` | MCP endpoint (Streamable HTTP) |
| `http://localhost:8080/whoami` | Authentication check (returns tenant and scope) |

## Provisioning tenants and API keys

API keys are stored in the database only as SHA-256 hashes, so they must be issued through
the admin CLI (there is no way to issue one via manual SQL):

```console
$ make admin ARGS="create-tenant my-team"
tenant created
  id:   019f565d-f1e3-7afb-b876-b7003e43c230
  name: my-team

$ make admin ARGS="create-api-key 019f565d-f1e3-7afb-b876-b7003e43c230 write"
api key created (the plaintext key is shown ONLY once — store it now)
  key:       ysr_928e48292888_ef72...
  ...

$ make admin ARGS="list-tenants"
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
