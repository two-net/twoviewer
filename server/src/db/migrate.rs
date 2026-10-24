//! Applies pending migrations at startup (DB-05 to DB-10).
//!
//! Migrations are SQL files in `server/migrations`, generated from the code model with
//! `cargo xtask new-migration` and embedded at build time. A run holds a PostgreSQL advisory lock
//! from start to finish, so when several instances start together exactly one applies migrations
//! while the others wait (DB-07). Each migration runs in its own transaction together with its
//! history row, so it is applied completely or not at all (DB-06).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use tracing::info;

/// A migration: its identifier (the file name without `.sql`) and its SQL.
#[derive(Clone, Copy, Debug)]
pub struct Migration<'a> {
    pub id: &'a str,
    pub sql: &'a str,
}

/// The migrations embedded in this build, in identifier order.
pub static MIGRATIONS: &[Migration<'static>] = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// Advisory lock key that serialises migration runs across instances: ASCII "twoviewr".
const LOCK_KEY: i64 = 0x7477_6f76_6965_7772;

#[derive(Debug)]
pub struct MigrationReport {
    /// Migrations applied by this run, in order. Empty when the schema was already current.
    pub applied: Vec<AppliedMigration>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct AppliedMigration {
    pub id: String,
    pub elapsed: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(
        "the database has tables but no migration history; \
         refusing to migrate a database that was not created by migrations"
    )]
    NotEmptyWithoutHistory,
    #[error(
        "the database has migration {id}, which this application version does not know; \
         the schema is newer than the application"
    )]
    UnknownMigration { id: String },
    #[error(
        "migration {id} has changed since it was applied; correct it with a new migration instead"
    )]
    ChecksumMismatch { id: String },
    #[error(
        "migration {id} is older than the newest applied migration {latest}; give it a newer identifier"
    )]
    OutOfOrder { id: String, latest: String },
    #[error("migration {id} failed and was rolled back: {source}")]
    Failed {
        id: String,
        #[source]
        source: sqlx::Error,
    },
}

/// Applies the embedded migrations, recording this application's version with each.
pub async fn migrate(database_url: &str) -> Result<MigrationReport, MigrationError> {
    run(database_url, MIGRATIONS, env!("CARGO_PKG_VERSION")).await
}

/// Applies `migrations`, which must be sorted by identifier, recording `app_version` with each.
pub async fn run(
    database_url: &str,
    migrations: &[Migration<'_>],
    app_version: &str,
) -> Result<MigrationReport, MigrationError> {
    debug_assert!(migrations.windows(2).all(|pair| pair[0].id < pair[1].id));
    let started = Instant::now();

    // A dedicated connection, not a pooled one: the advisory lock belongs to this session.
    let mut conn = PgConnection::connect(database_url).await?;
    let result = run_locked(&mut conn, migrations, app_version).await;
    // Ending the session releases the lock, also on error. If closing fails, the server ends the
    // session when the connection drops.
    let _ = conn.close().await;

    Ok(MigrationReport {
        applied: result?,
        elapsed: started.elapsed(),
    })
}

async fn run_locked(
    conn: &mut PgConnection,
    migrations: &[Migration<'_>],
    app_version: &str,
) -> Result<Vec<AppliedMigration>, MigrationError> {
    lock(conn).await?;
    let applied = applied_checksums(conn).await?;
    let pending = pending(migrations, &applied)?;
    if !pending.is_empty() {
        info!(count = pending.len(), "applying migrations");
    }

    let mut report = Vec::with_capacity(pending.len());
    for migration in pending {
        let started = Instant::now();
        apply(conn, migration, app_version)
            .await
            .map_err(|source| MigrationError::Failed {
                id: migration.id.to_owned(),
                source,
            })?;
        let elapsed = started.elapsed();
        info!(
            migration = migration.id,
            elapsed_ms = elapsed.as_millis(),
            "applied migration"
        );
        report.push(AppliedMigration {
            id: migration.id.to_owned(),
            elapsed,
        });
    }
    Ok(report)
}

async fn lock(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(LOCK_KEY)
        .fetch_one(&mut *conn)
        .await?;
    if !acquired {
        info!("another instance is applying migrations; waiting");
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Checksums of the applied migrations, by identifier.
async fn applied_checksums(
    conn: &mut PgConnection,
) -> Result<HashMap<String, Vec<u8>>, MigrationError> {
    let has_history: bool =
        sqlx::query_scalar("SELECT to_regclass('schema_migration') IS NOT NULL")
            .fetch_one(&mut *conn)
            .await?;
    if !has_history {
        // Without history, only an empty database may be migrated: it then gets the complete
        // schema from the migrations (DB-08).
        let has_tables: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
            )",
        )
        .fetch_one(&mut *conn)
        .await?;
        if has_tables {
            return Err(MigrationError::NotEmptyWithoutHistory);
        }
        return Ok(HashMap::new());
    }

    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as("SELECT id, checksum FROM schema_migration")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Checks the history against `migrations` and returns those still to apply, in order.
fn pending<'m, 'a>(
    migrations: &'m [Migration<'a>],
    applied: &HashMap<String, Vec<u8>>,
) -> Result<Vec<&'m Migration<'a>>, MigrationError> {
    let known: HashMap<&str, &Migration> = migrations.iter().map(|m| (m.id, m)).collect();
    let mut applied_ids: Vec<&str> = applied.keys().map(String::as_str).collect();
    applied_ids.sort_unstable();

    for &id in &applied_ids {
        // DB-09: refuse to run against a schema newer than this application.
        let Some(migration) = known.get(id) else {
            return Err(MigrationError::UnknownMigration { id: id.to_owned() });
        };
        // DB-12: applied migrations are immutable; corrections are new migrations.
        if checksum(migration.sql) != applied[id] {
            return Err(MigrationError::ChecksumMismatch { id: id.to_owned() });
        }
    }

    let pending: Vec<_> = migrations
        .iter()
        .filter(|m| !applied.contains_key(m.id))
        .collect();
    if let (Some(&latest), Some(first)) = (applied_ids.last(), pending.first())
        && first.id < latest
    {
        return Err(MigrationError::OutOfOrder {
            id: first.id.to_owned(),
            latest: latest.to_owned(),
        });
    }
    Ok(pending)
}

async fn apply(
    conn: &mut PgConnection,
    migration: &Migration<'_>,
    app_version: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin().await?;
    // Fail rather than wait long for a table lock, so a blocked migration cannot queue live
    // traffic behind it (REL-02). #12 makes this configurable.
    sqlx::raw_sql("SET LOCAL lock_timeout = '10s'")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(AssertSqlSafe(migration.sql))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO schema_migration (id, checksum, applied_at, app_version) VALUES ($1, $2, now(), $3)",
    )
    .bind(migration.id)
    .bind(checksum(migration.sql))
    .bind(app_version)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

fn checksum(sql: &str) -> Vec<u8> {
    Sha256::digest(sql.as_bytes()).to_vec()
}
