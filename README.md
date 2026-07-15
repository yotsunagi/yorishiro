# Yorishiro (依り代)

**English** | [日本語](docs/ja/README.md)

An MCP-native, multi-tenant knowledge store with user-defined schemas.

Users define entity "types" (fields, constraints, relations) as JSON meta-schemas, and data validated against those schemas can be read and written through both a REST API and MCP (Model Context Protocol). Fields marked `x-embed` are automatically vector-embedded, enabling similarity search over natural-language queries.

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

    DB[("PostgreSQL 18 + pgvector<br/>(identity + content schemas, RLS isolation)")]

    MCPClient -->|"/mcp"| MCPAdapter
    RESTClient -->|"/api/*"| RESTAdapter
    Core --> DB
```

- Cargo workspace
  - `yorishiro-core` (domain logic) and `yorishiro-server` (HTTP server and adapter layer).
  - Only `yorishiro-server` accesses the database directly.
- Two-tier tenancy
  - A **tenant** is an organization/account, with human **users** attached via roles: owner/admin/member/viewer. A tenant owns one or more **workspaces**.
  - All content (schemas/entities/relations) and API keys belong to exactly one workspace, not the tenant directly.
  - This lets one organization run several isolated projects (e.g. prod/staging, or one workspace per team) without separate tenants, and lets several people share administrative access to the same tenant.
- Isolation via RLS
  - PostgreSQL Row Level Security is applied to every table.
  - On each request, the workspace (and its owning tenant) are resolved from the API key.
  - Data can only be reached through a connection that has set the `app.current_tenant`/`app.current_workspace` session variables.
  - The application runs as a dedicated role (`yorishiro_app`, without `BYPASSRLS`). Control-plane tables (`identity.tenants`/`identity.users`/`identity.tenant_memberships`) aren't reachable by that role at all -- only the admin CLI, running as the migration role, can manage them.
- Quotas
  - A tenant's `max_workspaces` and a workspace's `max_entities` are enforced at creation time (workspace creation / entity creation, respectively).
  - Both default to `NULL` (unlimited). An operator can set explicit caps per tenant/workspace.
- Schema versioning
  - Re-registering a schema with the same name adds a new version.
  - Breaking changes (removed fields, type changes, newly required fields, etc.) are reported as a diff.
  - Existing entities continue to be validated against the schema version that was active when they were created.
- Single binary
  - Everything above ships in the single `yorishiro-server` binary.
  - Defaults to a single-tenant deployment (`YORISHIRO_MAX_TENANTS=1`; set it to `0` for unlimited tenants).
  - That same cap also enables a first-run setup wizard (browser UI at `/`, or `POST /setup`) that creates the tenant, workspace, and owner account in one step, no admin CLI needed.
  - Beyond that first account, further account creation is invite-only (`admin create-invite` → `POST /auth/signup` → `POST /auth/login`).
  - Tenant owners/admins can then manage members (`/api/members`) and workspaces (`/api/workspaces`) over REST, or through the same browser UI, without touching the admin CLI.

## Quick start

See [docs/setup.md](docs/setup.md) for the full guide, including the prebuilt binary and background/systemd operation. The fastest path, with Docker:

1. Fetch the embedding model (the default local ONNX provider needs no external service):

   ```console
   $ mkdir -p models
   $ curl -L -o models/model.onnx \
       https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
   $ curl -L -o models/tokenizer.json \
       https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json
   ```

2. Start the server:

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -v "$(pwd)/models:/app/models:ro" \
       -e DATABASE_URL=postgres://... \
       ghcr.io/yotsunagi/yorishiro:latest
   ```

   This is a complete single-tenant deployment as-is.
3. Visit `http://localhost:8080/` and create the owner account through the setup wizard.

Prefer building from source? Clone the repo, place the model files as in step 1, then `make init` (needs Docker Compose and `make`) builds and starts PostgreSQL plus the app:

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro
$ make init
```

## Documentation

| Document | Contents |
|---|---|
| [docs/setup.md](docs/setup.md) | Full setup guide: startup, endpoints, tenant/workspace/user/API key provisioning, auth & scopes |
| [docs/schema.md](docs/schema.md) | Meta-schema guide for defining entity types and relations |
| [docs/api.md](docs/api.md) | REST API and MCP tool reference |
| [docs/embedding-providers.md](docs/embedding-providers.md) | Configuring embedding providers (`local` ONNX / `openai`-compatible) |
| [docs/configuration.md](docs/configuration.md) | Environment variable / `config.yml` reference |
| [docs/deployment.md](docs/deployment.md) | Production deployment guide |
| [docs/operations.md](docs/operations.md) | Operational notes: backups, rate limiting, observability |

## Development

Day-to-day development commands run through a separate `dev` service (Rust toolchain, started on demand rather than as part of `make up`):

```console
$ make fmt-check
$ make clippy
$ make test
$ make shell   # ad-hoc cargo/psql/sqlx-cli access
```

Placing an ONNX model under `models/` enables embedding integration tests against the real model (they're skipped automatically otherwise).

## License

Licensed under the [Business Source License 1.1](LICENSE). Self-hosting (including for internal/commercial use) is permitted; the only restriction is offering Yorishiro itself as a competing hosted/managed service. On 2030-07-14 this version automatically converts to the GNU General Public License, Version 2.0 or later.
