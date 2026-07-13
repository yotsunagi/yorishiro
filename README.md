# Yorishiro (依り代)

**English** | [日本語](docs/ja/README.md)

An MCP-native, multi-tenant knowledge store with user-defined schemas.

Users define entity "types" (fields, constraints, relations) as JSON meta-schemas, and
data validated against those schemas can be read and written through both a REST API and
MCP (Model Context Protocol). Fields marked `x-embed` are automatically vector-embedded,
enabling similarity search over natural-language queries.

## Architecture

```mermaid
flowchart TD
    MCPClient["MCP client<br/>(Claude, etc.)"]
    RESTClient["REST client<br/>(curl/SDK)"]

    subgraph Server["yorishiro-server (axum)"]
        MCPAdapter["MCP adapter"]
        RESTAdapter["REST adapter"]
        Core["yorishiro-core<br/>(schemas / entities / relations /<br/>search / auth / embedding)"]
        MCPAdapter --> Core
        RESTAdapter --> Core
    end

    DB[("PostgreSQL 18 + pgvector<br/>(tenant isolation via RLS)")]

    MCPClient -->|"/mcp"| MCPAdapter
    RESTClient -->|"/api/*"| RESTAdapter
    Core --> DB
```

- **Cargo workspace**: `yorishiro-core` (domain logic) and `yorishiro-server` (HTTP server
  and adapter layer). Only the `yorishiro-server` process accesses the database directly.
- **Tenant isolation**: PostgreSQL Row Level Security is applied to every table. On each
  request, the tenant is resolved from the API key, and data can only be reached through a
  connection that has set the `app.current_tenant` session variable. The application runs
  as a dedicated role (`yorishiro_app`, without `BYPASSRLS`).
- **Schema versioning**: Re-registering a schema with the same name adds a new version;
  breaking changes (removed fields, type changes, newly required fields, etc.) are reported
  as a diff. Existing entities continue to be validated against the schema version that was
  active when they were created.

## Quick start

Prerequisites: Docker / Docker Compose. `docker compose up` starts PostgreSQL and a
development container (`app`, with the Rust toolchain); the server itself is run with
`cargo run` inside that container.

An embedding provider must be configured for the server to start. Here's a fully local
example using ONNX that needs no external service:

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro

# Place a 768-dimensional BERT-family ONNX model (see docs/embedding-providers.md)
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

Migrations are applied automatically on startup. See [docs/setup.md](docs/setup.md) for
the full setup guide, endpoint list, tenant/API key provisioning, and auth model.

## Documentation

| Document | Contents |
|---|---|
| [docs/setup.md](docs/setup.md) | Full setup guide: startup, endpoints, tenant/API key provisioning, auth & scopes |
| [docs/schema.md](docs/schema.md) | Meta-schema guide for defining entity types and relations |
| [docs/api.md](docs/api.md) | REST API and MCP tool reference |
| [docs/embedding-providers.md](docs/embedding-providers.md) | Configuring embedding providers (`openai`-compatible / `local` ONNX) |
| [docs/configuration.md](docs/configuration.md) | Environment variable reference |
| [docs/deployment.md](docs/deployment.md) | Production deployment guide |
| [docs/operations.md](docs/operations.md) | Operational notes: backups, rate limiting, observability |

## Development

```console
# Format, lint, test (requires a running PostgreSQL and DATABASE_URL)
$ cargo fmt --check
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo test --workspace
```

Placing an ONNX model under `models/` enables embedding integration tests against the real
model (they're skipped automatically otherwise).

## License

Licensed under the [MIT License](LICENSE).
