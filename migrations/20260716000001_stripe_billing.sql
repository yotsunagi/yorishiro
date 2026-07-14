-- Links a tenant to its Stripe customer once a subscription is created, so that later webhook
-- events (which only carry the Stripe customer/subscription id, never our tenant_id) can be
-- routed back to the right tenant. NULL until the hosted checkout flow completes; self-hosted
-- deployments never populate this.
ALTER TABLE identity.tenants ADD COLUMN stripe_customer_id TEXT UNIQUE;
