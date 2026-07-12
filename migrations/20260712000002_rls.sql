ALTER TABLE tenants   ENABLE ROW LEVEL SECURITY;
ALTER TABLE api_keys  ENABLE ROW LEVEL SECURITY;
ALTER TABLE schemas   ENABLE ROW LEVEL SECURITY;
ALTER TABLE entities  ENABLE ROW LEVEL SECURITY;
ALTER TABLE relations ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON tenants
  USING (id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON api_keys
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON schemas
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON entities
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON relations
  USING (tenant_id = current_setting('app.current_tenant')::uuid);
