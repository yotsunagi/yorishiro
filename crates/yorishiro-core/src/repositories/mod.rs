//! CRUD/query functions, one file per `models` counterpart, each re-exporting its models
//! (`pub use crate::models::X::*`) so `repositories::X` is the single import path for both a
//! record's shape and its persistence operations.

pub mod entities;
pub mod export;
pub mod recall;
pub mod relations;
pub mod schemas;
pub mod search;
pub mod tenancy;
