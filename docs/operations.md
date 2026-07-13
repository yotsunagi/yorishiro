# Operational Notes

**English** | [日本語](ja/operations.md)

Yorishiro itself does not automate the concerns below; operators need to set these up
separately.

## Backup and restore

Data lives entirely in PostgreSQL (in the development environment, the named volume
`pgdata` in `docker-compose.yml`, backed by the `yorishiro_pgdata` volume). Yorishiro has no
built-in backup automation, so set up scheduled backups with standard `pg_dump`/`pg_restore`,
or a WAL-archiving + PITR (Point-in-Time Recovery) setup, on the operator side. Relying on
volume snapshots alone can produce an inconsistent backup.

## Rate limiting

There is currently no per-API-key or per-tenant rate limiting or quota mechanism. A single
API key making heavy use of embedding generation or search can delay other requests. This is
especially true for `YSR_EMBEDDING_PROVIDER=local` (local ONNX inference), which serializes
inference behind a single mutex — so embedding generation for other tenants can be blocked
too, not just the same tenant. Introduce per-API-key rate limiting at a reverse proxy layer
(nginx, Envoy, etc.) if needed.

## Observability

Failures in embedding sync (background processing after an entity write) are currently only
emitted to `tracing` logs (`RUST_LOG`); there is no integration with a metrics backend. If
you need continuous monitoring, set up alerting on your log aggregation platform (Loki,
CloudWatch Logs, etc.) and additionally run `admin resync-embeddings` periodically to check
for anything missed.
