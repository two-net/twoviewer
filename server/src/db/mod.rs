//! PostgreSQL access: the code-first model (DB-02) and startup migrations (DB-05).
//!
//! Every entity follows the conventions in `server/migrations/README.md`: random UUID keys, UTC
//! timestamps, integrity declared in the model, and no partitioning (DB-11, DB-13 to DB-16).

mod email_address;
pub mod migrate;
pub mod schema_migration;

pub use email_address::EmailAddress;

use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DbBackend, DbErr, Schema, SchemaBuilder,
};

/// The code model: every entity in the database schema.
///
/// Migrations are generated from this list (`cargo xtask new-migration`), so a new entity must be
/// registered here.
pub fn schema_builder() -> SchemaBuilder {
    Schema::new(DbBackend::Postgres)
        .builder()
        .register(schema_migration::Entity)
        .register(crate::audit::event::Entity)
}

/// Opens the connection pool. Call only after [`migrate::migrate`] has succeeded.
pub async fn connect(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(database_url);
    // Statement logging would put query parameters, which can be secrets, into the logs (MNT-03).
    options.sqlx_logging(false);
    Database::connect(options).await
}
