# Schema Definition

**English** | [日本語](ja/schema.md)

Entity types are defined with a JSON meta-schema (example: `templates/task-management.json`):

```json
{
  "name": "task-management",
  "entity_types": {
    "task": {
      "fields": {
        "title": { "type": "string", "required": true, "x-embed": true },
        "done":  { "type": "boolean", "default": false }
      }
    },
    "project": {
      "fields": { "title": { "type": "string", "required": true } }
    }
  },
  "relation_types": {
    "belongs_to": { "source": "task", "target": "project" }
  }
}
```

- `type`: one of `string` / `integer` / `number` / `boolean` / `array`
- Constraints such as `required` / `enum` / `minimum` / `maximum` / `format`
- A `string` field marked `x-embed: true` becomes a target for vector embedding
- `relation_types` defines directed relations between entity types
