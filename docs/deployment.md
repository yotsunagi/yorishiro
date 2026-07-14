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

## Hosted deployment

Everything above is all a self-hosted (community) deployment needs — set
`YORISHIRO_MAX_TENANTS=1` (see [configuration.md](configuration.md)) and stop there. A
*hosted* deployment additionally runs a second process, `yorishiro-hosted-server`, built
from the separate `Dockerfile.hosted` (kept out of the community image's dependency tree
and attack surface on purpose). It serves:

- `POST /hosted/stripe/webhook` — Stripe subscription webhook receiver, mapping a Stripe
  Price id to a plan (`free`/`pro`/`team`) and writing the resulting caps onto
  `identity.tenants`.
- `GET /hosted/tenant/overview` — the data backing the admin dashboard (plan, usage, member
  list), restricted to the caller's own tenant.
- The admin dashboard SPA itself (login, usage/billing, member management — a
  framework-free static site under [`web/`](../web), served directly via `ServeDir`).

### Via Docker Compose (`hosted` profile)

`docker-compose.yml` defines `hosted` under an opt-in `"hosted"` Compose profile, so plain
`docker compose up`/`make up` (self-hosted) never starts it:

```console
$ make up-hosted   # docker compose --profile hosted up -d db app hosted
```

This starts `db` + `app` (`yorishiro-server`, port 8080) + `hosted`
(`yorishiro-hosted-server`, port 8081, dashboard at `http://localhost:8081/`). Because the
dashboard's login/member-management calls go to `app`'s origin while the dashboard page
itself is served from `hosted`'s origin, `app`'s `YSR_CORS_ORIGINS` includes
`http://localhost:8081` by default in `docker-compose.yml` — adjust it if the hosted service
is exposed under a different origin. Edit [`web/config.js`](../web/config.js) (or bind-mount
a replacement over it) to point the dashboard's `apiBase` at wherever `yorishiro-server` is
actually reachable.

### Standalone image

```console
$ docker build -f Dockerfile.hosted -t yorishiro-hosted .
$ docker run --rm -p 8081:8081 -e DATABASE_URL=postgres://... yorishiro-hosted
```

### Real Stripe/email credentials

Real Stripe credentials and a real transactional-email provider are deliberately deferred:
`YORISHIRO_STRIPE_WEBHOOK_SECRET` unset makes the webhook endpoint respond `501` rather than
accept unverifiable requests, and the built-in `EmailProvider` only logs the message it would
have sent. Wire up real ones by setting the Stripe env vars (see
[configuration.md](configuration.md#hosted-only-yorishiro-hosted-server)) and by implementing
`yorishiro_hosted::email::EmailProvider` against a real provider (SES, Postmark, etc.) in
place of `NoopEmailProvider`.
