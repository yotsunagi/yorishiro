-- Invite-only signup: a tenant admin (via the `admin create-invite` CLI command, or later a
-- hosted dashboard endpoint) mints a token for a specific email/role pair, and the signup flow
-- consumes it to create a user and tenant_membership together. Only the token's SHA-256 hash
-- is stored, mirroring `identity.api_keys.key_hash` -- the plaintext is shown once, at creation
-- time, and never persisted.
CREATE TABLE identity.invites (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id   UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  email       TEXT NOT NULL,
  role        TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
  token_hash  BYTEA NOT NULL UNIQUE,
  expires_at  TIMESTAMPTZ NOT NULL,
  used_at     TIMESTAMPTZ,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX invites_tenant_id_idx ON identity.invites (tenant_id);

-- Same tenant-scoped isolation as the rest of the control plane. In practice this table is
-- only ever touched through the admin/migration role (CLI invite creation, and the eventual
-- signup endpoint's token redemption both connect that way, exactly like
-- `identity.tenants`/`identity.users`/`identity.tenant_memberships` -- see the role-separation
-- migration's comment on why `yorishiro_app` gets no grant on those tables), so this is
-- defense in depth rather than a load-bearing check.
ALTER TABLE identity.invites ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.invites FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON identity.invites
  USING (tenant_id = current_setting('app.current_tenant')::uuid);
