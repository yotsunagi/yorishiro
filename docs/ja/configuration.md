# 環境変数リファレンス

[English](../configuration.md) | **日本語**

全変数の一覧と説明は[`.env.example`](../../.env.example)を参照してください。変数は
**プロセス環境変数として**サーバへ渡します（`.env`ファイルを自動で読む仕組みはありません。
docker composeの`environment:`や`docker compose exec -e`、systemdの`Environment=`などで
設定します）。

## 基本

| 変数 | 内容 |
|---|---|
| `DATABASE_URL` | PostgreSQL接続文字列（必須） |
| `YSR_BIND` | リッスンアドレス（既定: `0.0.0.0:8080`） |
| `YSR_CORS_ORIGINS` | ブラウザからアクセスする場合の許可オリジン（カンマ区切り。例: 別オリジンで動くダッシュボードが`/auth/login`/`/api/members`を呼べるようにする）。未設定時はクロスオリジン読み取り不可 |
| `YORISHIRO_MAX_TENANTS` | `admin create-tenant`が作成できるテナント数のデプロイ全体での上限。未設定（既定）は無制限。セルフホスト（コミュニティ）版では`1`を設定し、ホスティング版では未設定のままにする。`POST /auth/signup`はテナントを作成しない（既存のテナントへ招待を引き換えるだけ）ため影響を受けない。初回セットアップウィザード（`GET`/`POST /setup`、[setup.md](setup.md#初回セットアップコミュニティ版)参照）もこの変数で有効/無効が決まり、未設定のデプロイでは無効化される |
| `YSR_WEB_DIR` | セットアップ・ログイン用Web UIの静的ファイルを`/`で配信するディレクトリ。未設定（既定）ではWeb UI自体が無効になり、`/api/*`・`/mcp`・`/docs`のみが配信される。`docker-compose.yml`の`app`サービスはこれを`web`に設定済み |
| `YSR_AUTH_RATE_LIMIT_MAX` / `YSR_AUTH_RATE_LIMIT_WINDOW_SECS` | `/auth/signup`・`/auth/login`・`/setup`（bearerトークン不要なエンドポイントであり、未認証の呼び出し元が総当たりできる唯一の経路）に対する、呼び出し元IPごとのレート制限。既定値: 60秒あたり10リクエスト |
| `RUST_LOG` | ログレベル（例: `info`） |

## ログ出力

HTTPアクセスログ（method・path・status・latency）を含む全てのログ行はJSON形式で出力され、
`YSR_LOG_TARGET`で出力先を選択できます。

| 変数 | 内容 |
|---|---|
| `YSR_LOG_TARGET` | `stdout`（既定、コンテナランタイムのログドライバ向け）、`single`（単一ファイルへ追記、ローテーションなし）、`daily`（日次ローテーションするファイル）、`syslog` |

### `YSR_LOG_TARGET=single`または`daily`の場合

| 変数 | 内容 |
|---|---|
| `YSR_LOG_DIR` | ログファイルの出力先ディレクトリ（既定: `.`）。ファイル名は`yorishiro.log`固定で、`daily`の場合は日付が付与される（例: `yorishiro.log.2026-07-13`） |

### `YSR_LOG_TARGET=syslog`の場合

| 変数 | 内容 |
|---|---|
| `YSR_SYSLOG_SOCKET` | RFC 3164形式のメッセージを送信するUnixドメインソケット（既定: `/dev/log`）。Linux/Unix系OS限定 |

## 埋め込みプロバイダ

| 変数 | 内容 |
|---|---|
| `YSR_EMBEDDING_PROVIDER` | `openai`（既定）または`local` |
| `YSR_EMBEDDING_DIMENSIONS` | `entities.embedding`はvector(768)固定のため、768以外は起動時エラー |

### `YSR_EMBEDDING_PROVIDER=openai`の場合（例: Ollama, LM Studio, OpenAI）

| 変数 | 内容 |
|---|---|
| `YSR_EMBEDDING_BASE_URL` | `/v1/embeddings`互換エンドポイントのベースURL |
| `YSR_EMBEDDING_MODEL` | モデル名 |
| `YSR_EMBEDDING_API_KEY` | エンドポイントが要求する場合のAPIキー |
| `YSR_EMBEDDING_SEND_DIMENSIONS_PARAM` | リクエストボディに`dimensions`パラメータを含めるか（非対応サーバーでは`false`） |

### `YSR_EMBEDDING_PROVIDER=local`の場合（768次元のBERT系ONNXエクスポート）

| 変数 | 内容 |
|---|---|
| `YSR_ONNX_MODEL_PATH` | ONNXモデルのパス（例: `models/model.onnx`） |
| `YSR_ONNX_TOKENIZER_PATH` | tokenizerのパス（例: `models/tokenizer.json`） |
| `YSR_ONNX_MAX_SEQUENCE_LENGTH` | 最大シーケンス長（既定: `512`） |

具体的な取得例（`https://huggingface.co/Xenova/all-mpnet-base-v2`の
`onnx/model_quantized.onnx`と`tokenizer.json`）は
[docs/ja/embedding-providers.md](embedding-providers.md)を参照してください。

## ホスティング版限定（`yorishiro-hosted-server`）

別プロセスであるホスティング版（Stripe課金・使用量計測・管理ダッシュボードSPA）は、
このリポジトリではなく非公開の`yotsunagi/yorishiro-enterprise`リポジトリで開発されています
— [deployment.md](deployment.md#ホスティング版のデプロイ)参照。環境変数もそちらの
ドキュメントに記載されています。セルフホスト版はこのプロセスを一切実行しないため、
ここでは無関係です。
