CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE tenants (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  name        TEXT NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE api_keys (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id    UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  key_hash     BYTEA NOT NULL UNIQUE,
  key_prefix   TEXT  NOT NULL,
  scope        TEXT  NOT NULL CHECK (scope IN ('read','write','schema')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_used_at TIMESTAMPTZ
);

CREATE TABLE schemas (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id   UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  version     INTEGER NOT NULL DEFAULT 1,
  definition  JSONB NOT NULL,
  status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, name, version)
);

CREATE TABLE entities (
  id             UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id      UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  schema_id      UUID NOT NULL REFERENCES schemas(id),
  schema_version INTEGER NOT NULL,
  entity_type    TEXT NOT NULL,
  data           JSONB NOT NULL,
  embedding      vector(768),
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX entities_tenant_type_idx ON entities (tenant_id, entity_type, created_at);
CREATE INDEX entities_data_gin        ON entities USING GIN (data jsonb_path_ops);
CREATE INDEX entities_embedding_hnsw  ON entities USING hnsw (embedding vector_cosine_ops);

CREATE TABLE relations (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id     UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  source_id     UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
  target_id     UUID NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL,
  properties    JSONB NOT NULL DEFAULT '{}',
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, source_id, target_id, relation_type)
);
CREATE INDEX relations_source_idx ON relations (tenant_id, source_id);
CREATE INDEX relations_target_idx ON relations (tenant_id, target_id);
