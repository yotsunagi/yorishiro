# セットアップ

[English](../setup.md) | **日本語**

## 起動手順

必要なもの: Docker / Docker Compose / make。`make init`でイメージをビルドし、PostgreSQLと
`app`（リポジトリルートのマルチステージ`Dockerfile`が生成する、本番と同じreleaseバイナリを
実行するコンテナ）を起動します。

埋め込みプロバイダの設定が起動に必須です。`docker-compose.yml`は既に`app`をローカルONNX
プロバイダに向けているので、あとはモデルを配置するだけです（外部サービス不要）:

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro

# 768次元のBERT系ONNXモデルを配置（embedding-providers.md参照）
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json

$ make init
```

起動時にマイグレーションが自動適用されます。エンドポイント:

| パス | 内容 |
|---|---|
| `http://localhost:8080/up` | Liveness probe（プロセスが起動していれば依存関係を見ず常に200） |
| `http://localhost:8080/health` | Readiness check（DB接続も確認し、障害時は503） |
| `http://localhost:8080/docs` | Swagger UI（REST APIドキュメント） |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI仕様 |
| `http://localhost:8080/mcp` | MCPエンドポイント（Streamable HTTP） |
| `http://localhost:8080/whoami` | 認証確認（ワークスペース・テナント・scopeを返す） |

## テナント・ワークスペース・ユーザー

Yorishiroの制御プレーンは2階層構造です。

- **テナント**は組織/アカウント。`max_workspaces`を設定でき（課金上限。デフォルトの
  `NULL`は無制限で、セルフホスト運用に適する）、任意数の人間の**ユーザー**を
  ロール（`owner`/`admin`/`member`/`viewer`）付きのメンバーシップとして紐付けられる。
  1人のユーザーが複数のテナントに所属することもできる。
- **ワークスペース**はちょうど1つのテナントに属し、実際の操作対象コンテナ。
  スキーマ・エンティティ・リレーション・APIキーはすべてテナントではなくワークスペースに
  紐付く。ワークスペースは`max_entities`を設定できる（これもデフォルト`NULL`/無制限）。

テナントとワークスペースを分けることで、1つの組織が複数の独立したプロジェクト
（環境別・チーム別のワークスペースなど）を新規テナントを都度作らずに運用でき、
複数人でメンバーシップを介して同一テナントの管理権限を共有できます。この階層は
まだREST/MCP APIには公開されておらず、`DATABASE_URL`を持つ者が下記の管理CLIから
完全に管理します。

## テナント・ワークスペース・APIキーの発行

APIキーはDBにSHA-256ハッシュ、ユーザーパスワードはargon2ハッシュでのみ保存されるため、
どちらも手作業のSQLでは発行できず、管理CLIで行います:

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

`create-tenant`は新規テナントの下に`default`ワークスペースも自動作成します
（多くの運用ではテナントごとに1ワークスペースで十分なため）。追加のワークスペースが
必要な場合は`create-workspace`を使ってください。平文キーは発行時に一度だけ表示されます。
管理コマンドは`DATABASE_URL`の接続ロール（マイグレーションと同じ管理ロールで、
`identity.tenants`/`identity.users`/`identity.tenant_memberships`に書き込める唯一の
ロール。アプリ自身の`yorishiro_app`ロールにはこの権限がない）で直接DBへアクセスします。

その他の管理コマンド:

| コマンド | 内容 |
|---|---|
| `admin list-tenants` | 全テナントの一覧 |
| `admin create-workspace <tenant-id> <name> [--max-entities <n>]` | テナント配下に追加のワークスペースを作成 |
| `admin list-workspaces <tenant-id>` | テナントのワークスペース一覧 |
| `admin create-user <email> <password> [--display-name <name>]` | 人間のユーザーアカウントを作成 |
| `admin add-member <tenant-id> <user-id> <role>` | ユーザーのテナントへのメンバーシップを追加（または既存のroleを変更）（`owner`/`admin`/`member`/`viewer`） |
| `admin list-members <tenant-id>` | テナントのメンバーとそのroleの一覧 |
| `admin list-api-keys <workspace-id>` | キーの一覧（ID・scope・prefix・最終使用日時） |
| `admin revoke-api-key <key-id>` | キーの即時失効（漏洩時など） |
| `admin resync-embeddings <workspace-id>` | embedding未生成のentityを再同期（同期失敗からの回復） |

## 認証とscope

すべてのAPIは`Authorization: Bearer <APIキー>`で認証します。キーは`ysr_`で始まる文字列で、
発行時に一度だけ表示されます（DBにはSHA-256ハッシュのみ保存）。

scopeは3段階の包含関係: `read` < `write` < `schema`。
`write`キーは読み取りもでき、`schema`キーはスキーマ登録を含む全操作ができます。
