# 埋め込みプロバイダ

[English](../embedding-providers.md) | **日本語**

`x-embed`フィールドの埋め込み生成は`YSR_EMBEDDING_PROVIDER`で切り替えます（次元は768固定）。
埋め込みはエンティティ書き込み後にバックグラウンドで非同期生成されるため、書き込みAPIの
レイテンシには影響しません。

## `local` — ローカルONNXモデル（デフォルト）

外部サービスもAPIキーも不要で、必要なのは下記のモデルファイルだけなので、これがデフォルトになっており、セルフホスト環境では通常そのままで構いません。768次元のBERT系ONNXエクスポートが必要で、`YSR_ONNX_MODEL_PATH`/`YSR_ONNX_TOKENIZER_PATH`は既に`models/model.onnx`/`models/tokenizer.json`をデフォルト値としています:

```console
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json
```

この2ファイルをデフォルトのパスに置くだけでよく、環境変数は一切不要です。注意: 「外部サービス不要」は実行時の話で、**ビルド時**にはortクレートがonnxruntimeのプリビルドバイナリをダウンロードします（cdn.pyke.io）。ビルド環境まで閉域の場合は、事前に配置したonnxruntimeを`ORT_LIB_LOCATION`環境変数で指定してビルドしてください。

## `openai` — OpenAI互換API

Ollama / LM Studio / OpenAIなどの`/v1/embeddings`互換エンドポイントを使います。ローカルONNXのデフォルトから切り替えるには`YSR_EMBEDDING_PROVIDER=openai`を明示的に設定します:

```dotenv
YSR_EMBEDDING_PROVIDER=openai
YSR_EMBEDDING_BASE_URL=http://localhost:11434/v1
YSR_EMBEDDING_MODEL=nomic-embed-text
```
