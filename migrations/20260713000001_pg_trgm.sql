CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX entities_data_trgm_idx ON content.entities USING gin ((data::text) gin_trgm_ops);
