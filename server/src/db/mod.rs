//! PostgreSQL access: the code-first model (DB-02) and startup migrations (DB-05).

pub mod migrate;
pub mod schema_migration;

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
}

/// Opens the connection pool. Call only after [`migrate::migrate`] has succeeded.
pub async fn connect(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(database_url);
    // Statement logging would put query parameters, which can be secrets, into the logs (MNT-03).
    options.sqlx_logging(false);
    Database::connect(options).await
}
