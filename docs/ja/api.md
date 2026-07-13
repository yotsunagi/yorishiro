# REST API と MCPツール

[English](../api.md) | **日本語**

## REST API

主なエンドポイント（全一覧と詳細は`/docs`のSwagger UIを参照）:

```console
# スキーマ登録（schema scope）
$ curl -X POST localhost:8080/api/schemas \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d @templates/task-management.json

# エンティティ作成（write scope）
$ curl -X POST localhost:8080/api/entities \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d '{"schema_name":"task-management","entity_type":"task","data":{"title":"牛乳を買う"}}'

# ベクトル類似検索（read scope）
$ curl "localhost:8080/api/search?query_text=買い物" -H "Authorization: Bearer $YSR_KEY"
```

## MCPツール

`/mcp`（Streamable HTTP）に接続すると15のツールが使えます。Claude Codeでの接続例:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| ツール | scope | 内容 |
|---|---|---|
| `create_schema` | schema | メタスキーマの登録（新バージョン追加） |
| `list_schemas` | read | 登録済みスキーマのサマリ一覧（発見用） |
| `get_active_schema` | read | アクティブなスキーマ定義の取得 |
| `get_schema_by_id` | read | 特定バージョンのスキーマ取得 |
| `get_entity_type_json_schema` | read | entity_typeのJSON Schema投影 |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` / `list_entities` | write/read | エンティティCRUD |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | リレーションCRUD |
| `search_entities` | read | 自然文クエリによるベクトル類似検索 |
