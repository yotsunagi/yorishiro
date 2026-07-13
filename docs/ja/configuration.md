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
| `YSR_CORS_ORIGINS` | ブラウザからアクセスする場合の許可オリジン（カンマ区切り）。未設定時はクロスオリジン読み取り不可 |
| `RUST_LOG` | ログレベル（例: `info`） |

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
