# 埋め込みプロバイダ

[English](../embedding-providers.md) | **日本語**

`x-embed`フィールドの埋め込み生成は`YSR_EMBEDDING_PROVIDER`で切り替えます（次元は768固定）。
埋め込みはエンティティ書き込み後にバックグラウンドで非同期生成されるため、書き込みAPIの
レイテンシには影響しません。

## `openai` — OpenAI互換API（デフォルト）

Ollama / LM Studio / OpenAIなどの`/v1/embeddings`互換エンドポイントを使います。

```dotenv
YSR_EMBEDDING_PROVIDER=openai
YSR_EMBEDDING_BASE_URL=http://localhost:11434/v1
YSR_EMBEDDING_MODEL=nomic-embed-text
```

## `local` — ローカルONNXモデル

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
