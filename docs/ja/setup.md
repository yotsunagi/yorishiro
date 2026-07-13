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
| `http://localhost:8080/whoami` | 認証確認（テナントとscopeを返す） |

## テナントとAPIキーの発行

APIキーはDBにSHA-256ハッシュでのみ保存されるため、発行は管理CLIで行います
（手作業のSQLでは発行できません）:

```console
$ make admin ARGS="create-tenant my-team"
tenant created
  id:   019f565d-f1e3-7afb-b876-b7003e43c230
  name: my-team

$ make admin ARGS="create-api-key 019f565d-f1e3-7afb-b876-b7003e43c230 write"
api key created (the plaintext key is shown ONLY once — store it now)
  key:       ysr_928e48292888_ef72...
  ...

$ make admin ARGS="list-tenants"
```

平文キーは発行時に一度だけ表示されます。管理コマンドは`DATABASE_URL`の接続ロール
（マイグレーションと同じ管理ロール）で直接DBへアクセスします。

その他の管理コマンド:

| コマンド | 内容 |
|---|---|
| `admin list-api-keys <tenant-id>` | キーの一覧（ID・scope・prefix・最終使用日時） |
| `admin revoke-api-key <key-id>` | キーの即時失効（漏洩時など） |
| `admin resync-embeddings <tenant-id>` | embedding未生成のentityを再同期（同期失敗からの回復） |

## 認証とscope

すべてのAPIは`Authorization: Bearer <APIキー>`で認証します。キーは`ysr_`で始まる文字列で、
発行時に一度だけ表示されます（DBにはSHA-256ハッシュのみ保存）。

scopeは3段階の包含関係: `read` < `write` < `schema`。
`write`キーは読み取りもでき、`schema`キーはスキーマ登録を含む全操作ができます。
