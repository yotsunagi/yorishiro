# Production Deployment

**English** | [日本語](ja/deployment.md)

## Docker (prebuilt image)

Every `vX.Y.Z` tag publishes `ghcr.io/yotsunagi/yorishiro:vX.Y.Z` (and `:latest`) — see
[Releasing](#releasing) below. The image already bundles the setup-wizard SPA (`web/`);
nothing needs to be built locally:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e DATABASE_URL=postgres://... \
    -e YSR_EMBEDDING_BASE_URL=... -e YSR_EMBEDDING_MODEL=... \
    ghcr.io/yotsunagi/yorishiro:latest
```

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

Each release also attaches Linux binaries (`yorishiro-server-vX.Y.Z-linux-amd64.tar.gz` /
`-linux-arm64.tar.gz`) to its [GitHub Release](https://github.com/yotsunagi/yorishiro/releases).
Unlike the Docker image, the archive contains only the `yorishiro-server` binary — the setup
wizard's `web/` directory and (if using the local ONNX provider) `models/` are not inside it,
since they aren't compiled in. Fetch them from the tag they were released from and place them
next to the binary (relative paths like `YSR_WEB_DIR=web` resolve against the process's
working directory):

```console
$ mkdir -p /opt/yorishiro && cd /opt/yorishiro
$ curl -L -o yorishiro.tar.gz \
    https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/yorishiro-server-vX.Y.Z-linux-amd64.tar.gz
$ tar -xzf yorishiro.tar.gz && rm yorishiro.tar.gz

# web/ (setup wizard) and, if applicable, models/ (local ONNX provider) come from the same tag
$ curl -L https://github.com/yotsunagi/yorishiro/archive/refs/tags/vX.Y.Z.tar.gz \
    | tar -xz --strip-components=1 "yorishiro-*/web"
```

### Running in the background

For a bare-metal/VM deployment, a systemd unit keeps the process running across reboots and
restarts it on failure:

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
EnvironmentFile=/opt/yorishiro/yorishiro.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

`yorishiro.env` holds `DATABASE_URL`, `YSR_WEB_DIR=web`, `YORISHIRO_MAX_TENANTS=1`, and the
rest of [configuration.md](configuration.md)'s variables, one `KEY=value` per line. Then:

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

Set `YORISHIRO_MAX_TENANTS=1` and `YSR_WEB_DIR=web` (see [configuration.md](configuration.md))
to serve the [`web/`](../web) SPA, whose setup wizard (see
[setup.md](setup.md#first-run-setup)) is enough to onboard the deployment's one tenant.
