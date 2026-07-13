-- The table owner and superuser always bypass RLS even with `FORCE ROW LEVEL SECURITY`. So
-- to make RLS actually apply, the app's runtime connection has to run as a non-superuser,
-- non-owner, NOBYPASSRLS role distinct from the owner (yorishiro). yorishiro_app has no
-- LOGIN privilege (`SET ROLE` can be used by a superuser without membership, so the
-- operational model is: log in as yorishiro, then `SET ROLE yorishiro_app` after the
-- connection is established).
-- Roles are shared cluster-wide, while `sqlx::test` applies this migration concurrently
-- across many ephemeral databases, so a check-then-create would lose the race and hit a
-- duplicate-creation error. Instead this tries CREATE ROLE directly and swallows only the
-- unique_violation from losing that race to a concurrent run.
DO $$
BEGIN
  CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOLOGIN;
EXCEPTION
  WHEN duplicate_object OR unique_violation THEN
    NULL;
END
$$;

ALTER TABLE tenants   FORCE ROW LEVEL SECURITY;
ALTER TABLE api_keys  FORCE ROW LEVEL SECURITY;
ALTER TABLE schemas   FORCE ROW LEVEL SECURITY;
ALTER TABLE entities  FORCE ROW LEVEL SECURITY;
ALTER TABLE relations FORCE ROW LEVEL SECURITY;

GRANT USAGE ON SCHEMA public TO yorishiro_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO yorishiro_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO yorishiro_app;
