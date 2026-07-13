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

ALTER TABLE identity.tenants            FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.tenant_memberships  FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.workspaces          FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.api_keys            FORCE ROW LEVEL SECURITY;
ALTER TABLE content.schemas              FORCE ROW LEVEL SECURITY;
ALTER TABLE content.entities             FORCE ROW LEVEL SECURITY;
ALTER TABLE content.relations            FORCE ROW LEVEL SECURITY;

-- Least privilege: the request-serving role only gets access to what request handling
-- actually touches today. `identity.tenants`/`tenant_memberships`/`users` are managed
-- exclusively by the admin CLI, which connects as the owning role (bypassing RLS and these
-- grants entirely) -- so yorishiro_app gets no grant on them at all yet. Widen this
-- deliberately (not by default) once a user-facing tenant-management endpoint exists.
-- `identity.workspaces` is the one exception: it gets a read-only grant because entity
-- creation reads `max_entities` from it to enforce the per-workspace billing cap.
GRANT USAGE ON SCHEMA identity TO yorishiro_app;
GRANT USAGE ON SCHEMA content  TO yorishiro_app;

GRANT SELECT ON identity.workspaces TO yorishiro_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON identity.api_keys TO yorishiro_app;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA content TO yorishiro_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA content TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA content GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA content GRANT USAGE, SELECT ON SEQUENCES TO yorishiro_app;
