# スキーマ定義

[English](../schema.md) | **日本語**

エンティティの型はJSONメタスキーマで定義します（例: `templates/task-management.json`）:

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

- `type`: `string` / `integer` / `number` / `boolean` / `array`
- `required` / `enum` / `minimum` / `maximum` / `format`などの制約
- `x-embed: true`を付けたstringフィールドはベクトル埋め込みの対象になる
- `relation_types`はエンティティ型どうしの有向リレーションを定義する
