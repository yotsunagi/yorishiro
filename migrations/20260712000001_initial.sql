CREATE EXTENSION IF NOT EXISTS vector;

-- `identity` holds the control plane (accounts, memberships, workspaces, API keys).
-- `content` holds tenant-authored data (schemas, entities, relations). Keeping the two
-- apart in dedicated schemas -- rather than the default `public` -- means every table
-- reference in application code is schema-qualified and never depends on `search_path`
-- resolution order (the class of issue behind CVE-2018-1058, "schema squatting").
CREATE SCHEMA identity;
CREATE SCHEMA content;

-- No first-party table ever lives in `public`; extensions (vector, pg_trgm) stay there.
-- Revoking CREATE from PUBLIC on it is Postgres 15's own default for new databases, but is
-- restated explicitly here so the guarantee doesn't depend on which Postgres version this
-- runs against.
REVOKE CREATE ON SCHEMA public FROM PUBLIC;

-- A tenant is a billing/ownership account. `plan`/`max_workspaces` are NULL by default,
-- meaning unlimited -- self-hosted deployments never set them; a hosted offering would.
CREATE TABLE identity.tenants (
  id             UUID PRIMARY KEY DEFAULT uuidv7(),
  name           TEXT NOT NULL,
  plan           TEXT,
  max_workspaces INTEGER,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A human account. Not tenant-scoped by itself -- a user can belong to multiple tenants
-- through `tenant_memberships`.
CREATE TABLE identity.users (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  email         TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  display_name  TEXT,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Many-to-many between users and tenants, carrying the user's role within that tenant.
CREATE TABLE identity.tenant_memberships (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id   UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  user_id     UUID NOT NULL REFERENCES identity.users(id) ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, user_id)
);

-- A workspace is the operational container nested under a tenant: schemas, entities,
-- relations, and API keys all scope to a workspace, not directly to a tenant.
-- `max_entities` mirrors `tenants.max_workspaces`: NULL means unlimited.
CREATE TABLE identity.workspaces (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id    UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  max_entities INTEGER,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, name)
);

CREATE TABLE identity.api_keys (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  key_hash     BYTEA NOT NULL UNIQUE,
  key_prefix   TEXT  NOT NULL,
  scope        TEXT  NOT NULL CHECK (scope IN ('read', 'write', 'schema')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_used_at TIMESTAMPTZ
);

CREATE TABLE content.schemas (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  version      INTEGER NOT NULL DEFAULT 1,
  definition   JSONB NOT NULL,
  status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, name, version)
);

CREATE TABLE content.entities (
  id             UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id   UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  schema_id      UUID NOT NULL REFERENCES content.schemas(id),
  schema_version INTEGER NOT NULL,
  entity_type    TEXT NOT NULL,
  data           JSONB NOT NULL,
  embedding      vector(768),
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX entities_workspace_type_idx ON content.entities (workspace_id, entity_type, created_at);
CREATE INDEX entities_data_gin          ON content.entities USING GIN (data jsonb_path_ops);
CREATE INDEX entities_embedding_hnsw    ON content.entities USING hnsw (embedding vector_cosine_ops);

CREATE TABLE content.relations (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  source_id     UUID NOT NULL REFERENCES content.entities(id) ON DELETE CASCADE,
  target_id     UUID NOT NULL REFERENCES content.entities(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL,
  properties    JSONB NOT NULL DEFAULT '{}',
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, source_id, target_id, relation_type)
);
CREATE INDEX relations_source_idx ON content.relations (workspace_id, source_id);
CREATE INDEX relations_target_idx ON content.relations (workspace_id, target_id);
