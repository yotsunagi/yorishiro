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
| `http://localhost:8080/whoami` | Authentication check (returns workspace, tenant, and scope) |

## Tenants, workspaces, and users

Yorishiro's control plane is two-tiered:

- A **tenant** is an organization/account. It can have `max_workspaces` set (a billing cap;
  `NULL`, the default, means unlimited — appropriate for self-hosted deployments) and can
  have any number of human **users** attached to it, each with a role (`owner` / `admin` /
  `member` / `viewer`) recorded in a membership. A user can belong to multiple tenants.
- A **workspace** belongs to exactly one tenant and is the actual operational container:
  schemas, entities, relations, and API keys all scope to a workspace, not directly to the
  tenant. A workspace can have `max_entities` set (also `NULL`/unlimited by default).

Splitting tenant from workspace lets one organization run several isolated projects (e.g.
separate workspaces per environment or team) without provisioning a whole new tenant for
each, and lets several people share administrative access to the same tenant via
memberships. None of this is exposed over the REST/MCP API yet — it's managed entirely
through the admin CLI below, by whoever holds `DATABASE_URL`.

## Provisioning tenants, workspaces, and API keys

API keys are stored in the database only as SHA-256 hashes and user passwords only as
argon2 hashes, so neither can be provisioned by hand in SQL — both go through the admin CLI:

```console
$ make admin ARGS="create-tenant my-team"
tenant created
  id:            019f565d-f1e3-7afb-b876-b7003e43c230
  name:          my-team
  max_workspaces: unlimited
default workspace created
  id:   019f565d-f204-7f3e-9a1e-2b6b6e2b6b6e
  name: default

$ make admin ARGS="create-api-key 019f565d-f204-7f3e-9a1e-2b6b6e2b6b6e write"
api key created (the plaintext key is shown ONLY once — store it now)
  key:          ysr_928e48292888_ef72...
  ...

$ make admin ARGS="list-tenants"
```

`create-tenant` also creates a `default` workspace under the new tenant, since most
deployments only need one workspace per tenant; use `create-workspace` for additional ones.
The plaintext API key is shown only once, at issuance time. Admin commands access the
database directly using the connection role from `DATABASE_URL` (the same administrative
role used for migrations, and the only role permitted to touch
`identity.tenants`/`identity.users`/`identity.tenant_memberships` at all — the application's
own `yorishiro_app` role cannot).

Other admin commands:

| Command | Description |
|---|---|
| `admin list-tenants` | List all tenants |
| `admin create-workspace <tenant-id> <name> [--max-entities <n>]` | Create an additional workspace under a tenant |
| `admin list-workspaces <tenant-id>` | List a tenant's workspaces |
| `admin create-user <email> <password> [--display-name <name>]` | Create a human user account |
| `admin add-member <tenant-id> <user-id> <role>` | Add (or change the role of) a user's membership in a tenant (`owner`/`admin`/`member`/`viewer`) |
| `admin list-members <tenant-id>` | List a tenant's members and their roles |
| `admin create-api-key <workspace-id> <scope> [--user <user-id>]` | Issue an API key, optionally attributed to a member |
| `admin list-api-keys <workspace-id>` | List keys (ID, scope, prefix, attributed user, last used) |
| `admin revoke-api-key <key-id>` | Immediately revoke a key (e.g. on leakage) |
| `admin resync-embeddings <workspace-id>` | Re-sync entities missing an embedding (recovery from a failed sync) |

## Authentication and scopes

All APIs authenticate via `Authorization: Bearer <api-key>`. Keys are strings starting with
`ysr_`, shown only once at issuance time (only a SHA-256 hash is stored in the database).

Scopes form a three-level hierarchy: `read` < `write` < `schema`. A `write` key can also
read, and a `schema` key can perform every operation, including schema registration.

### Attributing keys to users

Since Yorishiro is API-first (there's no login/session flow — every request, human or
automated, goes through an API key), multi-user access control works by tying a key to a
member's role rather than by a session. Passing `--user <user-id>` to `create-api-key`
attributes the key to that member and caps the requested scope at
`MembershipRole::max_scope()`: `owner`/`admin` may be issued up to `schema`, `member` up to
`write`, and `viewer` up to `read`. Requesting a scope above that cap, or attributing a key
to someone who isn't a member of the workspace's tenant, is rejected at issuance time. This
check runs once, when the key is created — like a key's scope itself, it isn't re-evaluated
on every request, so revoking a user's membership doesn't retroactively narrow keys already
issued to them (revoke the key instead). Omit `--user` for unattributed service/automation
keys, which aren't capped by any role. `GET /whoami` echoes the attributed `user_id` (or
`null`) alongside the workspace, tenant, and scope.
