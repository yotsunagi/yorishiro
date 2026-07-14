# 環境変数リファレンス

[English](../configuration.md) | **日本語**

全変数の一覧と説明は[`.env.example`](../../.env.example)を参照してください。変数は
**プロセス環境変数として**サーバへ渡します（`.env`ファイルを自動で読む仕組みはありません。
docker composeの`environment:`や`docker compose exec -e`、systemdの`Environment=`などで
設定します）。

以下の設定はすべて`config.yml`ファイルでも指定できます — キー一覧は
[`config.example.yml`](../../config.example.yml)を参照してください（`embedding:`・`logging:`・
`auth_rate_limit:`はグループごとにネストします）。デフォルトでは作業ディレクトリの
`config.yml`を読み込みます。別の場所を使う場合は`YSR_CONFIG_PATH`で指定してください。
ファイルが存在しない場合や、ファイル内に該当キーがない場合はエラーにならず、通常の
デフォルト値にフォールバックします。**環境変数が設定されている場合は、対応する
`config.yml`のキーより常に優先されます。** これにより、`config.yml`をデプロイの基本設定
として使い、環境変数は（1回限りのDocker `-e`オプションなど）一時的な上書き用途に
限定する、という使い方ができます。

## 基本

| 変数 | 内容 |
|---|---|
| `DATABASE_URL` | PostgreSQL接続文字列（必須） |
| `YSR_BIND` | リッスンアドレス（既定: `0.0.0.0:8080`） |
| `YSR_CORS_ORIGINS` | ブラウザからアクセスする場合の許可オリジン（カンマ区切り。例: 別オリジンで動くダッシュボードが`/auth/login`/`/api/members`を呼べるようにする）。未設定時はクロスオリジン読み取り不可 |
| `YORISHIRO_MAX_TENANTS` | `admin create-tenant`が作成できるテナント数のデプロイ全体での上限。未設定時は既定で`1`（シングルテナント）。無制限にするには`0`を、複数テナントを許可するにはその上限数を設定する。`POST /auth/signup`はテナントを作成しない（既存のテナントへ招待を引き換えるだけ）ため影響を受けない。初回セットアップウィザード（`GET`/`POST /setup`、[setup.md](setup.md#初回セットアップ)参照）もこの変数で有効/無効が決まり、上限が実際に有効（`0`でない）な場合のみ有効化される |
| `YSR_WEB_DIR` | セットアップ・ログイン用Web UIの静的ファイルを`/`で配信するディレクトリ。未設定時は既定でバイナリ/Dockerイメージに同梱された`web`ディレクトリを使う |
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
| `YSR_EMBEDDING_PROVIDER` | `local`（既定）または`openai` |
| `YSR_EMBEDDING_DIMENSIONS` | `entities.embedding`はvector(768)固定のため、768以外は起動時エラー |

### `YSR_EMBEDDING_PROVIDER=local`の場合（768次元のBERT系ONNXエクスポート、既定）

| 変数 | 内容 |
|---|---|
| `YSR_ONNX_MODEL_PATH` | ONNXモデルのパス（既定: `models/model.onnx`） |
| `YSR_ONNX_TOKENIZER_PATH` | tokenizerのパス（既定: `models/tokenizer.json`） |
| `YSR_ONNX_MAX_SEQUENCE_LENGTH` | 最大シーケンス長（既定: `512`） |

### `YSR_EMBEDDING_PROVIDER=openai`の場合（例: Ollama, LM Studio, OpenAI）

| 変数 | 内容 |
|---|---|
| `YSR_EMBEDDING_BASE_URL` | `/v1/embeddings`互換エンドポイントのベースURL（必須） |
| `YSR_EMBEDDING_MODEL` | モデル名（必須） |
| `YSR_EMBEDDING_API_KEY` | エンドポイントが要求する場合のAPIキー |
| `YSR_EMBEDDING_SEND_DIMENSIONS_PARAM` | リクエストボディに`dimensions`パラメータを含めるか（非対応サーバーでは`false`） |

具体的な取得例（`https://huggingface.co/Xenova/all-mpnet-base-v2`の
`onnx/model_quantized.onnx`と`tokenizer.json`）は
[docs/ja/embedding-providers.md](embedding-providers.md)を参照してください。
