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

# ベクトル類似検索（構造化フィルタとの組み合わせ、read scope）
$ curl "localhost:8080/api/search?query_text=買い物&filter=%7B%22status%22%3A%22active%22%7D" \
    -H "Authorization: Bearer $YSR_KEY"

# エンティティとそのリレーション・隣接エンティティを一括取得（read scope）
$ curl "localhost:8080/api/entities/$ENTITY_ID/context" -H "Authorization: Bearer $YSR_KEY"

# ワークスペース全体のJSON Linesエクスポート（read scope）
$ curl "localhost:8080/api/export.jsonl" -H "Authorization: Bearer $YSR_KEY"
```

`GET /api/entities`は`filter`クエリパラメータ（JSONBの包含条件でマッチするJSONオブジェクト、例:
`filter={"status":"active"}`）も受け付けます。また`POST /api/schemas`はインラインの定義に加えて
`{"template_id": "..."}`を渡すことで、`GET /api/templates`で一覧取得できる組み込みテンプレートから
スキーマを登録できます。

### 認証とメンバー管理

他の全エンドポイントと異なり、`/auth/signup`と`/auth/login`はbearerトークンを必要としません
— これらの目的自体がトークンを発行することだからです。招待からサインアップ・ログインまでの
一連の流れは[setup.md](setup.md#サインアップログインメンバー管理)を参照してください。

```console
# 招待（`admin create-invite`参照）を引き換えてアカウントを作成
$ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
    -d '{"invite_token":"...","password":"...","display_name":"..."}'

# メールアドレス/パスワードを、新しく発行されたrole上限付きのAPIキーと交換
$ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
    -d '{"email":"...","password":"...","workspace_id":"..."}'

# 呼び出し元自身のテナントのメンバーを一覧・追加（owner/adminのみ）
$ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/json" -d '{"email":"...","role":"member"}'
```

`POST /api/members`は**既存の**アカウントを呼び出し元のテナントに追加します — 新規作成は
しません（それはサインアップの役割です）。両メンバー管理エンドポイントとも、キー自身のscope
とは独立に、呼び出し元のテナントrole（owner/admin）で制御されます。

## MCPツール

`/mcp`（Streamable HTTP）に接続すると17のツールが使えます。Claude Codeでの接続例:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| ツール | scope | 内容 |
|---|---|---|
| `create_schema` | schema | メタスキーマの登録（新バージョン追加）。インラインの`definition`または`template_id`から作成可能 |
| `list_templates` | read | `create_schema`の`template_id`に指定できる組み込みスキーマテンプレートの一覧 |
| `list_schemas` | read | 登録済みスキーマのサマリ一覧（発見用） |
| `get_active_schema` | read | アクティブなスキーマ定義の取得 |
| `get_schema_by_id` | read | 特定バージョンのスキーマ取得 |
| `get_entity_type_json_schema` | read | entity_typeのJSON Schema投影 |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` | write/read | エンティティCRUD |
| `list_entities` | read | エンティティ一覧。`entity_type`および/または`filter`（JSONB包含マッチ）で絞り込み可能 |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | リレーションCRUD |
| `search_entities` | read | 自然文クエリによるベクトル類似検索。`entity_type`/`filter`で絞り込み可能。埋め込みを持たないエンティティも trigram によるあいまい検索でヒットし得る |
| `recall_context` | read | エンティティとそのリレーション・隣接エンティティを一括取得 |

REST専用の`GET /api/export.jsonl`エンドポイント（ワークスペース全体のJSON Linesエクスポート）に対応する
MCPツールはありません。
