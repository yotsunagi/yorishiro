//! Control-plane CRUD for tenants, workspaces, users, and tenant memberships. Everything here
//! operates on a raw `&PgPool` rather than an RLS-scoped connection: callers (the admin CLI)
//! connect using the migration/admin role, which is the only role permitted to touch
//! `identity.tenants`/`identity.users`/`identity.tenant_memberships` at all (see the
//! role-separation migration).

mod invites;
mod memberships;
mod tenants;
mod users;
mod workspaces;

pub use invites::*;
pub use memberships::*;
pub use tenants::*;
pub use users::*;
pub use workspaces::*;

pub use crate::models::tenancy::*;
