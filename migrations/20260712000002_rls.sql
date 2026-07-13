-- Two isolation levels: `app.current_tenant` scopes tenant-level control-plane rows
-- (the tenant itself, its memberships, its workspaces); `app.current_workspace` scopes
-- everything an API key actually touches (the key row itself, plus all content). A single
-- API key is always scoped to one workspace, so content isolation keys on workspace_id
-- rather than tenant_id.
ALTER TABLE identity.tenants             ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.tenant_memberships  ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.workspaces          ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.api_keys            ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.schemas              ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.entities             ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.relations            ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON identity.tenants
  USING (id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON identity.tenant_memberships
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON identity.workspaces
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY workspace_isolation ON identity.api_keys
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY workspace_isolation ON content.schemas
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY workspace_isolation ON content.entities
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY workspace_isolation ON content.relations
  USING (workspace_id = current_setting('app.current_workspace')::uuid);
