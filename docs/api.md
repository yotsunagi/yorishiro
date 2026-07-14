# REST API & MCP Tools

**English** | [日本語](ja/api.md)

## REST API

Key endpoints (see the Swagger UI at `/docs` for the full list and details):

```console
# Register a schema (schema scope)
$ curl -X POST localhost:8080/api/schemas \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d @templates/task-management.json

# Create an entity (write scope)
$ curl -X POST localhost:8080/api/entities \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d '{"schema_name":"task-management","entity_type":"task","data":{"title":"Buy milk"}}'

# Vector similarity search, combined with a structured filter (read scope)
$ curl "localhost:8080/api/search?query_text=shopping&filter=%7B%22status%22%3A%22active%22%7D" \
    -H "Authorization: Bearer $YSR_KEY"

# Entity plus its relations and connected neighbors in one call (read scope)
$ curl "localhost:8080/api/entities/$ENTITY_ID/context" -H "Authorization: Bearer $YSR_KEY"

# Full-workspace JSON Lines export (read scope)
$ curl "localhost:8080/api/export.jsonl" -H "Authorization: Bearer $YSR_KEY"
```

`GET /api/entities` also accepts a `filter` query parameter (a JSON object matched with
JSONB containment, e.g. `filter={"status":"active"}`), and `POST /api/schemas` accepts
either an inline definition or `{"template_id": "..."}` to register one of the built-in
templates listed at `GET /api/templates`.

### Auth & member management

Unlike every other endpoint, `/auth/signup` and `/auth/login` take no bearer token — their
entire purpose is to hand one out. See
[setup.md](setup.md#signup-login-and-member-management) for the full invite → signup →
login flow.

```console
# Redeem an invite (see `admin create-invite`) to create an account
$ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
    -d '{"invite_token":"...","password":"...","display_name":"..."}'

# Exchange email/password for a freshly issued, role-capped API key
$ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
    -d '{"email":"...","password":"...","workspace_id":"..."}'

# List / add members of the caller's own tenant (owner/admin only)
$ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/json" -d '{"email":"...","role":"member"}'
```

`POST /api/members` attaches an *existing* account to the caller's tenant — it never creates
one (that's what signup does). Both member-management endpoints are gated on the caller's
tenant role (owner/admin), independent of their key's own scope.

### Hosted-only endpoints

A hosted deployment runs a second process (`yorishiro-hosted-server`, see
[deployment.md](deployment.md#hosted-deployment)) exposing `POST /hosted/stripe/webhook`
(Stripe subscription webhook) and `GET /hosted/tenant/overview` (billing/usage/member data
backing the admin dashboard SPA). Self-hosted deployments never run this process, so these
paths don't exist there.

## MCP Tools

Connecting to `/mcp` (Streamable HTTP) gives you access to 17 tools. Example connection
from Claude Code:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| Tool | Scope | Description |
|---|---|---|
| `create_schema` | schema | Register a meta-schema (adds a new version), from an inline `definition` or a `template_id` |
| `list_templates` | read | List built-in schema templates usable as `create_schema`'s `template_id` |
| `list_schemas` | read | List a summary of registered schemas (for discovery) |
| `get_active_schema` | read | Fetch the active schema definition |
| `get_schema_by_id` | read | Fetch a specific schema version |
| `get_entity_type_json_schema` | read | Project an entity_type as a JSON Schema |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` | write/read | Entity CRUD |
| `list_entities` | read | List entities, optionally filtered by `entity_type` and/or a `filter` JSONB containment match |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | Relation CRUD |
| `search_entities` | read | Vector similarity search over a natural-language query, optionally narrowed by `entity_type`/`filter`; entities without an embedding can still surface via trigram fuzzy matching |
| `recall_context` | read | Fetch an entity plus its relations and connected neighbors in one call |

The REST-only `GET /api/export.jsonl` endpoint (full-workspace JSON Lines export) has no MCP
tool equivalent.
