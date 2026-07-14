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

## Releasing

Pushing a `vX.Y.Z` tag triggers `.github/workflows/release.yml`, which builds
`yorishiro-server` binaries for `x86_64`/`aarch64` Linux (glibc) and attaches them to a
GitHub Release, and builds+pushes a multi-arch Docker image to
`ghcr.io/yotsunagi/yorishiro:vX.Y.Z` (and `:latest`). Both architectures build natively (no
QEMU), matching the `ort`/onnxruntime build requirements above.

```console
$ git tag vX.Y.Z && git push origin vX.Y.Z
```

## Single-tenant mode

Set `YORISHIRO_MAX_TENANTS=1` and `YSR_WEB_DIR=web` (see [configuration.md](configuration.md))
to serve the [`web/`](../web) SPA, whose setup wizard (see
[setup.md](setup.md#first-run-setup)) is enough to onboard the deployment's one tenant.
