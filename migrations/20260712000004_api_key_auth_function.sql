-- At the API key authentication entry point, neither workspace_id nor tenant_id is known
-- yet (there's nothing to set `app.current_workspace`/`app.current_tenant` to), so the
-- normal RLS path can't look up api_keys by key_hash. This function runs SECURITY DEFINER
-- as the migration role (the table owner), scoping the RLS bypass to this one function and
-- purpose. It never returns key_hash itself — only the columns needed for the
-- authentication result, plus tenant_id (joined from the workspace) so callers can set both
-- session variables without a second round trip.
CREATE FUNCTION identity.authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, identity
AS $$
  SELECT k.id, k.workspace_id, w.tenant_id, k.scope
  FROM identity.api_keys k
  JOIN identity.workspaces w ON w.id = k.workspace_id
  WHERE k.key_hash = p_key_hash
$$;

REVOKE ALL ON FUNCTION identity.authenticate_api_key(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION identity.authenticate_api_key(bytea) TO yorishiro_app;
