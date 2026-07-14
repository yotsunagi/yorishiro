# Embedding Providers

**English** | [日本語](ja/embedding-providers.md)

Embedding generation for `x-embed` fields is switched with `YSR_EMBEDDING_PROVIDER`
(dimensions are fixed at 768). Embeddings are generated asynchronously in the background
after an entity is written, so write API latency is unaffected.

## `local` — Local ONNX model (default)

Requires no external service or API key — just the model files below — so it's the default
and what a self-hosted deployment normally wants. Requires a 768-dimensional BERT-family ONNX
export at `YSR_ONNX_MODEL_PATH`/`YSR_ONNX_TOKENIZER_PATH`, which already default to
`models/model.onnx`/`models/tokenizer.json`:

```console
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json
```

Placing the two files at those default paths is enough — no environment variables are
required at all. Note: "requires no external service" applies at runtime only. **At build
time**, the `ort` crate downloads a prebuilt onnxruntime binary (from cdn.pyke.io). If your
build environment is also air-gapped, provide a pre-placed onnxruntime and point the build at
it with the `ORT_LIB_LOCATION` environment variable.

## `openai` — OpenAI-compatible API

Uses an `/v1/embeddings`-compatible endpoint such as Ollama, LM Studio, or OpenAI. Set
`YSR_EMBEDDING_PROVIDER=openai` explicitly to opt into this instead of the local ONNX default:

```dotenv
YSR_EMBEDDING_PROVIDER=openai
YSR_EMBEDDING_BASE_URL=http://localhost:11434/v1
YSR_EMBEDDING_MODEL=nomic-embed-text
```
