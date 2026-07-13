-- At the API key authentication entry point, tenant_id isn't known yet (there's nothing to
-- set `app.current_tenant` to), so the normal RLS path can't look up api_keys by key_hash.
-- This function runs SECURITY DEFINER as the migration role (the table owner), scoping the
-- RLS bypass to this one function and purpose. It never returns key_hash itself — only the
-- columns needed for the authentication result (id/tenant_id/scope).
CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, tenant_id uuid, scope text)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
  SELECT id, tenant_id, scope FROM api_keys WHERE key_hash = p_key_hash
$$;

REVOKE ALL ON FUNCTION authenticate_api_key(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;
