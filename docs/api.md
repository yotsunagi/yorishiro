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

# Vector similarity search (read scope)
$ curl "localhost:8080/api/search?query_text=shopping" -H "Authorization: Bearer $YSR_KEY"
```

## MCP Tools

Connecting to `/mcp` (Streamable HTTP) gives you access to 15 tools. Example connection
from Claude Code:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| Tool | Scope | Description |
|---|---|---|
| `create_schema` | schema | Register a meta-schema (adds a new version) |
| `list_schemas` | read | List a summary of registered schemas (for discovery) |
| `get_active_schema` | read | Fetch the active schema definition |
| `get_schema_by_id` | read | Fetch a specific schema version |
| `get_entity_type_json_schema` | read | Project an entity_type as a JSON Schema |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` / `list_entities` | write/read | Entity CRUD |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | Relation CRUD |
| `search_entities` | read | Vector similarity search over a natural-language query |
