-- Basic audit trail: who created/last updated an entity. Nullable because unattributed
-- (service/automation) API keys have no user_id to record. `ON DELETE SET NULL` means
-- deleting a user account doesn't cascade into deleting the entities they touched -- it
-- just anonymizes the attribution, matching how `identity.api_keys.user_id` already behaves.
-- A full tamper-evident, exportable audit log is a separate, hosted-only concern; this is
-- just enough for basic team accountability in both editions.
ALTER TABLE content.entities
  ADD COLUMN created_by UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  ADD COLUMN updated_by UUID REFERENCES identity.users(id) ON DELETE SET NULL;
