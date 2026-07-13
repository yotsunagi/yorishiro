# Embedding Providers

**English** | [日本語](ja/embedding-providers.md)

Embedding generation for `x-embed` fields is switched with `YSR_EMBEDDING_PROVIDER`
(dimensions are fixed at 768). Embeddings are generated asynchronously in the background
after an entity is written, so write API latency is unaffected.

## `openai` — OpenAI-compatible API (default)

Uses an `/v1/embeddings`-compatible endpoint such as Ollama, LM Studio, or OpenAI.

```dotenv
YSR_EMBEDDING_PROVIDER=openai
YSR_EMBEDDING_BASE_URL=http://localhost:11434/v1
YSR_EMBEDDING_MODEL=nomic-embed-text
```

## `local` — Local ONNX model

Requires no external service; suited to air-gapped environments. Requires a
768-dimensional BERT-family ONNX export.

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

Note: "requires no external service" applies at runtime only. **At build time**, the `ort`
crate downloads a prebuilt onnxruntime binary (from cdn.pyke.io). If your build environment
is also air-gapped, provide a pre-placed onnxruntime and point the build at it with the
`ORT_LIB_LOCATION` environment variable.
