# Yorishiro（依り代）

ユーザー定義スキーマを持つ、MCPネイティブなマルチテナント・ナレッジストア。

エンティティの「型」（フィールド・制約・リレーション）を利用者がJSONメタスキーマとして定義し、
そのスキーマで検証されたデータをREST APIとMCP（Model Context Protocol）の両方から読み書きできます。
`x-embed`を付けたフィールドは自動でベクトル埋め込みされ、自然文クエリによる類似検索ができます。

## アーキテクチャ

```
┌──────────────┐   ┌──────────────┐
│ MCPクライアント │   │ RESTクライアント │
│ (Claude等)    │   │ (curl/SDK)    │
└──────┬───────┘   └──────┬───────┘
       │ /mcp             │ /api/*
       ▼                  ▼
┌─────────────────────────────────┐
│ yorishiro-server (axum)          │
│  MCPアダプタ ──┬── RESTアダプタ    │
│               ▼                  │
│        yorishiro-core            │
│  (schemas/entities/relations/    │
│   search/auth/embedding)         │
└──────────────┬──────────────────┘
               ▼
   PostgreSQL 18 + pgvector
   （RLSによるテナント分離）
```

- **cargo workspace**: `yorishiro-core`（ドメインロジック）と`yorishiro-server`（HTTPサーバ・アダプタ層）。
  DBへ直接アクセスするのは`yorishiro-server`プロセスのみ。
- **テナント分離**: 全テーブルにPostgreSQLのRow Level Securityを適用。リクエストごとに
  APIキーからテナントを解決し、セッション変数`app.current_tenant`を設定したコネクションでのみ
  データへ到達できる。アプリは専用ロール（`yorishiro_app`、`BYPASSRLS`なし）で動作する。
- **スキーマバージョニング**: 同名スキーマの再登録は新バージョンとして追加され、破壊的変更
  （フィールド削除・型変更・必須化など）は差分として報告される。既存エンティティは
  作成時点のスキーマバージョンに対して検証され続ける。

## セットアップ

必要なもの: Docker / Docker Compose。`docker compose up`で起動するのはPostgreSQLと
開発コンテナ（`app`、Rustツールチェーン入り）で、サーバ自体は開発コンテナ内で
`cargo run`します。

埋め込みプロバイダの設定が起動に必須です。外部サービス不要で完結するローカルONNXの例:

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro

# 768次元のBERT系ONNXモデルを配置（後述の「埋め込みプロバイダ」参照）
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

起動時にマイグレーションが自動適用されます。エンドポイント:

| パス | 内容 |
|---|---|
| `http://localhost:8080/health` | ヘルスチェック |
| `http://localhost:8080/docs` | Swagger UI（REST APIドキュメント） |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI仕様 |
| `http://localhost:8080/mcp` | MCPエンドポイント（Streamable HTTP） |
| `http://localhost:8080/whoami` | 認証確認（テナントとscopeを返す） |

### テナントとAPIキーの発行

APIキーはDBにSHA-256ハッシュでのみ保存されるため、発行は管理CLIで行います
（手作業のSQLでは発行できません）:

```console
$ docker compose exec app cargo run -q -p yorishiro-server -- admin create-tenant my-team
tenant created
  id:   019f565d-f1e3-7afb-b876-b7003e43c230
  name: my-team

$ docker compose exec app cargo run -q -p yorishiro-server -- admin create-api-key \
    019f565d-f1e3-7afb-b876-b7003e43c230 write
api key created (the plaintext key is shown ONLY once — store it now)
  key:       ysr_928e48292888_ef72...
  ...

$ docker compose exec app cargo run -q -p yorishiro-server -- admin list-tenants
```

平文キーは発行時に一度だけ表示されます。管理コマンドは`DATABASE_URL`の接続ロール
（マイグレーションと同じ管理ロール）で直接DBへアクセスします。

その他の管理コマンド:

| コマンド | 内容 |
|---|---|
| `admin list-api-keys <tenant-id>` | キーの一覧（ID・scope・prefix・最終使用日時） |
| `admin revoke-api-key <key-id>` | キーの即時失効（漏洩時など） |
| `admin resync-embeddings <tenant-id>` | embedding未生成のentityを再同期（同期失敗からの回復） |

### 認証とscope

すべてのAPIは`Authorization: Bearer <APIキー>`で認証します。キーは`ysr_`で始まる文字列で、
発行時に一度だけ表示されます（DBにはSHA-256ハッシュのみ保存）。

scopeは3段階の包含関係: `read` < `write` < `schema`。
`write`キーは読み取りもでき、`schema`キーはスキーマ登録を含む全操作ができます。

## スキーマ定義

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

## 埋め込みプロバイダ

`x-embed`フィールドの埋め込み生成は`YSR_EMBEDDING_PROVIDER`で切り替えます（次元は768固定）。
埋め込みはエンティティ書き込み後にバックグラウンドで非同期生成されるため、書き込みAPIの
レイテンシには影響しません。

### `openai` — OpenAI互換API（デフォルト）

Ollama / LM Studio / OpenAIなどの`/v1/embeddings`互換エンドポイントを使います。

```dotenv
YSR_EMBEDDING_PROVIDER=openai
YSR_EMBEDDING_BASE_URL=http://localhost:11434/v1
YSR_EMBEDDING_MODEL=nomic-embed-text
```

### `local` — ローカルONNXモデル

外部サービス不要。閉域環境向け。768次元のBERT系ONNXエクスポートが必要です。

```console
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json
```

```dotenv
YSR_EMBEDDING_PROVIDER=local
YSR_ONNX_MODEL_PATH=models/model.onnx
YSR_ONNX_TOKENIZER_PATH=models/tokenizer.json
```

注意: 「外部サービス不要」は実行時の話で、**ビルド時**にはortクレートがonnxruntimeの
プリビルドバイナリをダウンロードします（cdn.pyke.io）。ビルド環境まで閉域の場合は、
事前に配置したonnxruntimeを`ORT_LIB_LOCATION`環境変数で指定してビルドしてください。

## 環境変数リファレンス

全変数の一覧と説明は[`.env.example`](.env.example)を参照してください。変数は
**プロセス環境変数として**サーバへ渡します（`.env`ファイルを自動で読む仕組みはありません。
docker composeの`environment:`や`docker compose exec -e`、systemdの`Environment=`などで
設定します）。

主要なもの:

| 変数 | 内容 |
|---|---|
| `DATABASE_URL` | PostgreSQL接続文字列（必須） |
| `YSR_BIND` | リッスンアドレス（既定: `0.0.0.0:8080`） |
| `YSR_EMBEDDING_PROVIDER` | `openai`（既定）または`local` |
| `YSR_CORS_ORIGINS` | ブラウザからアクセスする場合の許可オリジン（カンマ区切り）。未設定時はクロスオリジン読み取り不可 |
| `RUST_LOG` | ログレベル（例: `info`） |

## 本番デプロイ

リポジトリ直下の`Dockerfile`（マルチステージ）で自己完結の実行イメージをビルドできます:

```console
$ docker build -t yorishiro .
$ docker run --rm -p 8080:8080 \
    -e DATABASE_URL=postgres://... \
    -e YSR_EMBEDDING_BASE_URL=... -e YSR_EMBEDDING_MODEL=... \
    yorishiro
```

マイグレーションはバイナリに埋め込まれており起動時に自動適用されます（複数レプリカの
同時起動もadvisory lockで安全）。SIGTERM/Ctrl-Cでgraceful shutdownし、処理中のリクエストと
バックグラウンドのembedding同期の完了（最大30秒）を待ってから終了します。それでも
embedding同期が失われた場合は`admin resync-embeddings`で回復できます。

管理CLIは同じイメージで実行できます:

```console
$ docker run --rm -e DATABASE_URL=postgres://... yorishiro admin list-tenants
```

## 運用上の注意

本プロジェクト自体は以下を自動化する機構を持たないため、運用者側で別途整備してください。

### バックアップ/リストア

データはPostgreSQL（開発環境では`docker-compose.yml`の named volume `pgdata`、実体は
`yorishiro_pgdata`）にのみ保持されます。本プロジェクトはバックアップの自動化機構を
持たないため、標準的な`pg_dump`/`pg_restore`によるスケジュールバックアップ、または
WALアーカイブ+PITR（Point-in-Time Recovery）構成を運用者側で用意してください。
ボリュームのスナップショットのみに頼ると、整合性のないバックアップになる場合があります。

### レート制限

APIキー単位・テナント単位のレート制限やクォータ管理機構は現状実装していません。
単一のAPIキーが埋め込み生成や検索を大量に呼び出すと、他のリクエストが遅延しえます。
特に`YSR_EMBEDDING_PROVIDER=local`（ONNXローカル推論）は単一Mutexで推論を直列化して
いるため、同一テナントはもちろん他テナントの埋め込み生成も待たされます。必要に応じて
リバースプロキシ層（nginx/Envoyなど）でAPIキー単位のレート制限を導入してください。

### 可観測性

embedding同期（entity書き込み後のバックグラウンド処理）の失敗は現状`tracing`ログ
（`RUST_LOG`）にのみ出力され、メトリクス基盤には接続していません。継続的に監視したい
場合は、ログ収集基盤（Loki/CloudWatch Logsなど）でのアラート設定に加えて、
`admin resync-embeddings`を定期実行し取りこぼしがないか確認することを推奨します。

## 開発

```console
# フォーマット・lint・テスト（要: 起動中のPostgreSQL、DATABASE_URL）
$ cargo fmt --check
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo test --workspace
```

`models/`にONNXモデルを置くと、実モデルでの埋め込み統合テストが有効になります
（無い場合は自動スキップ）。

## ライセンス

[MIT](LICENSE-MIT) または [Apache-2.0](LICENSE-APACHE) のいずれか（利用者の選択）。
