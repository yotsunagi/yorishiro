# Production Deployment

**English** | [日本語](ja/deployment.md)

## Docker (prebuilt image)

Every `vX.Y.Z` tag publishes `ghcr.io/yotsunagi/yorishiro:vX.Y.Z` (and `:latest`) — see
[Releasing](#releasing) below. The setup-wizard SPA (`web/`) is compiled into the binary, so
nothing needs to be built or mounted locally for it:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -v "$(pwd)/models:/app/models:ro" \
    -e DATABASE_URL=postgres://... \
    ghcr.io/yotsunagi/yorishiro:latest
```

The embedding provider defaults to the local ONNX model at `models/model.onnx`/`models/tokenizer.json` (mounted above) — see [setup.md](setup.md#prerequisites-and-startup) for fetching those files, or [embedding-providers.md](embedding-providers.md) to use an OpenAI-compatible endpoint instead.

`-d --restart unless-stopped` runs it detached and brings it back up after a reboot or
crash; `docker logs -f yorishiro` follows its output, `docker stop yorishiro` shuts it down
gracefully. Migrations are embedded in the binary and applied automatically on startup (safe
to start multiple replicas concurrently, thanks to an advisory lock). The server shuts down
gracefully on SIGTERM/Ctrl-C, waiting for in-flight requests and background embedding syncs
to finish (up to 30 seconds) before exiting. If an embedding sync is still lost, it can be
recovered with `admin resync-embeddings`.

The admin CLI can be run from the same image:

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

To build the image from source instead (e.g. to test an unreleased change), the same
multi-stage `Dockerfile` is at the repository root: `docker build -t yorishiro .`.

## Prebuilt binary (without Docker)

Each release also attaches Linux binaries (`yorishiro-server-vX.Y.Z-linux-amd64.tar.gz` / `-linux-arm64.tar.gz`) to its [GitHub Release](https://github.com/yotsunagi/yorishiro/releases). The archive contains only the `yorishiro-server` binary -- the setup wizard's `web/` is compiled in, so nothing needs to be fetched for it; only `models/` (if using the local ONNX provider) needs to be placed next to the binary, since model weights aren't compiled in:

```console
$ mkdir -p /opt/yorishiro && cd /opt/yorishiro

# The binary itself
$ curl -L -o yorishiro.tar.gz https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/yorishiro-server-vX.Y.Z-linux-amd64.tar.gz
$ tar -xzf yorishiro.tar.gz && rm yorishiro.tar.gz

# models/ (local ONNX embedding provider, the default -- see embedding-providers.md to use
# an OpenAI-compatible endpoint instead)
$ mkdir -p models
$ curl -L -o models/model.onnx https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json https://huggingface.co/Xenova/all-mpnet-base-v2/resolve/main/tokenizer.json

# .env.example, the full variable reference with comments, from the same tag
$ curl -L -o .env https://raw.githubusercontent.com/yotsunagi/yorishiro/vX.Y.Z/.env.example
```

Edit `.env` to set at least `DATABASE_URL`. Everything else can be left commented out: `YORISHIRO_MAX_TENANTS` and `YSR_EMBEDDING_PROVIDER` (plus the ONNX model/tokenizer paths) all already default to the single-tenant, web-UI-enabled, local-ONNX-embedding values a self-hosted deployment normally wants, matching the files fetched above — see [configuration.md](configuration.md) for the full reference and how to change any of them.

The binary reads configuration only from the real process environment — it never reads a `.env` file itself — so running it directly means loading `.env` into the shell first. A `config.yml` file, on the other hand, *is* read directly by the binary (see [configuration.md](configuration.md#configyml) and [`config.example.yml`](../config.example.yml)) — for a bare-metal/systemd deployment like this one, dropping a `config.yml` next to the binary is often simpler than either of the two `.env`-loading mechanisms below, since it needs no shell sourcing and no `EnvironmentFile=`:

```console
$ set -a; source .env; set +a
$ ./yorishiro-server
```

### Running in the background

For a bare-metal/VM deployment, a systemd unit keeps the process running across reboots and restarts it on failure. Unlike a plain shell, systemd's `EnvironmentFile=` loads `.env` directly — no `source`/`set -a` needed:

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
EnvironmentFile=/opt/yorishiro/.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro
$ journalctl -u yorishiro -f
```

## Releasing

Pushing a `vX.Y.Z` tag triggers `.github/workflows/release.yml`, which builds
`yorishiro-server` binaries for `x86_64`/`aarch64` Linux (glibc, packaged as
`linux-amd64`/`linux-arm64`) and attaches them to a GitHub Release, and builds+pushes a
multi-arch Docker image to `ghcr.io/yotsunagi/yorishiro:vX.Y.Z` (and `:latest`). Both
architectures build natively (no QEMU), matching the `ort`/onnxruntime build requirements
above.

```console
$ git tag vX.Y.Z && git push origin vX.Y.Z
```

## Single-tenant mode

`YORISHIRO_MAX_TENANTS=1` and `YSR_EMBEDDING_PROVIDER=local` (see [configuration.md](configuration.md)) are both defaults, so a deployment that leaves them unset already serves the [`web/`](../web) SPA (compiled into the binary), whose setup wizard (see [setup.md](setup.md#first-run-setup)) is enough to onboard the deployment's one tenant, and embeds using the local ONNX model. Set `YORISHIRO_MAX_TENANTS=0` to lift the tenant cap instead.
