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
| `http://localhost:8080/` | Setup/login web UI (only when `YSR_WEB_DIR` is set — see below) |
| `http://localhost:8080/docs` | Swagger UI (REST API documentation) |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI specification |
| `http://localhost:8080/mcp` | MCP endpoint (Streamable HTTP) |
| `http://localhost:8080/whoami` | Authentication check (returns workspace, tenant, and scope) |

## First-run setup

Deployments with `YORISHIRO_MAX_TENANTS` set (`docker-compose.yml`'s `app`
service does this and also sets `YSR_WEB_DIR=web`) serve a setup wizard at
`http://localhost:8080/` — no admin CLI needed. On first visit, since no tenant exists yet,
the browser shows a form asking only for an email and password; submitting it creates the
tenant, its `default` workspace, and an owner account in one step, and displays the freshly
issued API key (shown only once, same as every other key in this system). Visiting the same
page afterward shows a login form instead.

The same flow is available without a browser:

```console
$ curl localhost:8080/setup/status
{"setup_required":true}
$ curl -X POST localhost:8080/setup -H "Content-Type: application/json" \
    -d '{"email":"owner@example.com","password":"a strong password"}'
{"user_id":"...","email":"owner@example.com","tenant_id":"...","workspace_id":"...",
 "api_key":"ysr_..."}
```

`POST /setup` returns `404` once a tenant already exists, or on any deployment where
`YORISHIRO_MAX_TENANTS` is unset — hosted deployments onboard tenants via signup/invite
instead (see [Signup, login, and member management](#signup-login-and-member-management)).
The admin CLI below remains available for anything the wizard doesn't cover: additional
workspaces/tenants, invites, and key rotation.

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
memberships. Tenant/workspace *creation* is only available through the admin CLI below (by
whoever holds `DATABASE_URL`); day-to-day *membership* management (inviting/adding/listing
members) is available to tenant owners/admins over REST — see
[Signup, login, and member management](#signup-login-and-member-management).

By default, a deployment may create any number of tenants. Single-tenant deployments
should set `YORISHIRO_MAX_TENANTS=1` (see
[configuration.md](configuration.md)) so `admin create-tenant` and the signup flow below
can never create a second one; leave it unset to allow multiple tenants.

## Provisioning tenants, workspaces, and API keys

Deployments that used the setup wizard above can skip this section for their first (and, per
`YORISHIRO_MAX_TENANTS=1`, only) tenant. It remains the only way to provision *additional*
tenants/workspaces, and the only way to provision anything at all when
`YORISHIRO_MAX_TENANTS` is unset.

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
| `admin create-invite <tenant-id> <email> <role> [--ttl-hours <n>]` | Issue an invite token for an email to join a tenant (default TTL: 7 days) — see below |
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

Every request, human or automated, is ultimately authenticated by an API key (there's no
cookie/session state on the server) — but a key can be *attributed* to a human user, and
multi-user access control works by tying that attribution to the user's tenant role rather
than by a session. Passing `--user <user-id>` to `create-api-key` attributes the key to that
member and caps the requested scope at `MembershipRole::max_scope()`: `owner`/`admin` may be
issued up to `schema`, `member` up to `write`, and `viewer` up to `read`. Requesting a scope
above that cap, or attributing a key to someone who isn't a member of the workspace's
tenant, is rejected at issuance time. This check runs once, when the key is created — like a
key's scope itself, it isn't re-evaluated on every request, so revoking a user's membership
doesn't retroactively narrow keys already issued to them (revoke the key instead). Omit
`--user` for unattributed service/automation keys, which aren't capped by any role.
`GET /whoami` echoes the attributed `user_id` (or `null`) alongside the workspace, tenant,
and scope.

`POST /auth/login` (below) is the self-service equivalent of `admin create-api-key --user`:
it authenticates with a password instead of `DATABASE_URL` access, and issues a key already
capped at the caller's own role.

## Signup, login, and member management

Account creation is invite-only — there is no public, unauthenticated signup. A tenant
owner/admin issues an invite (by CLI or, once they hold an API key, by REST), the invitee
redeems it once to create their account, and from then on they authenticate with
email/password to obtain API keys, rather than being handed one out of band.

1. **Invite** — a tenant owner/admin creates an invite token for an email address and role:

   ```console
   $ make admin ARGS="create-invite 019f565d-f1e3-7afb-b876-b7003e43c230 newperson@example.com member"
   invite created (the plaintext token is shown ONLY once — send it now)
     token:      c8b9ea1f...
     ...
     expires at: 2026-07-20 16:57 UTC
   ```

   Send the plaintext `token` to the invitee out of band (email, chat, etc.) — like an API
   key, it's shown only once and only its hash is persisted. It expires after `--ttl-hours`
   (default 7 days) or immediately upon being redeemed, whichever comes first.

2. **Signup** — the invitee redeems the token to create their account:

   ```console
   $ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
       -d '{"invite_token":"c8b9ea1f...","password":"a strong password","display_name":"New Person"}'
   {"user_id":"...","email":"newperson@example.com","tenant_id":"...","role":"member",
    "workspaces":[{"id":"...","name":"default"}]}
   ```

   This both creates the `identity.users` row and adds the membership the invite specified
   — a second signup attempt with the same (now-consumed) token is rejected (422).

3. **Login** — from then on, the user exchanges their password for a freshly issued API key,
   scoped to one workspace and capped at their role's `max_scope()` (see above):

   ```console
   $ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
       -d '{"email":"newperson@example.com","password":"a strong password","workspace_id":"..."}'
   {"api_key":"ysr_...","api_key_id":"...","workspace_id":"...","scope":"write","user_id":"..."}
   ```

   Every login issues a *new* key rather than reusing one — revoke old ones with
   `admin revoke-api-key` if they're no longer needed.

4. **Member management** — once authenticated, a tenant owner/admin can list and add members
   over REST without needing `DATABASE_URL`/the admin CLI at all:

   ```console
   $ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
   $ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
       -H "Content-Type: application/json" \
       -d '{"email":"existing-user@example.com","role":"admin"}'
   ```

   `POST /api/members` attaches an *existing* account (one that already completed signup) to
   the caller's tenant — it never creates a new account. To bring in someone with no account
   yet, issue them an invite (step 1) instead. Both endpoints require the caller's own key to
   be attributed to an Owner/Admin member — a Member-role key is rejected with 403 regardless
   of its own scope, since membership management is a tenant-role concern, not a scope one.
