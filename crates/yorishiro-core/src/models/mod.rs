//! Data shapes: the structs and enums persisted to or read from the database, plus the input
//! DTOs the matching `repositories` module accepts. Adding or changing a field lives here;
//! changing how a record is queried lives in the matching file under `repositories`.

pub mod entities;
pub mod export;
pub mod recall;
pub mod relations;
pub mod schemas;
pub mod search;
pub mod tenancy;
