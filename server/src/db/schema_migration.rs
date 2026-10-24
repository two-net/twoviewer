//! Migration history (DB-10): one row per applied migration.
//!
//! The first migration creates this table; the runner in [`super::migrate`] reads and writes it.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "schema_migration")]
pub struct Model {
    /// Migration identifier: the file name without `.sql`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// SHA-256 of the migration file, so applied migrations cannot be edited (DB-12).
    pub checksum: Vec<u8>,
    pub applied_at: DateTimeUtc,
    /// Version of the server application that applied the migration.
    pub app_version: String,
}

impl ActiveModelBehavior for ActiveModel {}
