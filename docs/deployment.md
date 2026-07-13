# Production Deployment

**English** | [日本語](ja/deployment.md)

The multi-stage `Dockerfile` at the repository root builds a self-contained runtime image:

```console
$ docker build -t yorishiro .
$ docker run --rm -p 8080:8080 \
    -e DATABASE_URL=postgres://... \
    -e YSR_EMBEDDING_BASE_URL=... -e YSR_EMBEDDING_MODEL=... \
    yorishiro
```

Migrations are embedded in the binary and applied automatically on startup (safe to start
multiple replicas concurrently, thanks to an advisory lock). The server shuts down
gracefully on SIGTERM/Ctrl-C, waiting for in-flight requests and background embedding syncs
to finish (up to 30 seconds) before exiting. If an embedding sync is still lost, it can be
recovered with `admin resync-embeddings`.

The admin CLI can be run from the same image:

```console
$ docker run --rm -e DATABASE_URL=postgres://... yorishiro admin list-tenants
```
