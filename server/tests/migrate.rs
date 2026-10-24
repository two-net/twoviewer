//! Startup migrations against a real PostgreSQL (DB-05 to DB-10), including the upgrade from the
//! previous release (MNT-05). Needs Docker.

mod common;

use std::{process::Command, time::Duration};

use common::{LOCK_TIMEOUT, execute, postgres};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use tokio::task::JoinSet;
use twoviewer_server::db::migrate::{self, MIGRATIONS, Migration, MigrationError};

const OK: Migration = Migration {
    id: "99990101000000_ok",
    sql: "CREATE TABLE ok_table (id integer)",
};

const LATER: Migration = Migration {
    id: "99990101000002_later",
    sql: "CREATE TABLE later_table (id integer)",
};

/// The embedded migrations followed by `extra`.
fn with(extra: &[Migration<'static>]) -> Vec<Migration<'static>> {
    MIGRATIONS.iter().chain(extra).copied().collect()
}

fn ids(migrations: &[Migration]) -> Vec<String> {
    migrations.iter().map(|m| m.id.to_owned()).collect()
}

async fn applied_ids(url: &str) -> Vec<String> {
    let mut conn = PgConnection::connect(url).await.unwrap();
    sqlx::query_scalar("SELECT id FROM schema_migration ORDER BY id")
        .fetch_all(&mut conn)
        .await
        .unwrap()
}

fn git(args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The newest release before this commit, from its `v<major>.<minor>.<patch>` tag: its version
/// and `(id, sql)` of its migrations, in order. `None` before the first release.
fn previous_release() -> Option<(String, Vec<(String, String)>)> {
    assert_eq!(
        git(&["rev-parse", "--is-shallow-repository"]).trim(),
        "false",
        "a shallow clone hides the release tags; fetch the full history"
    );
    // A build of a release commit upgrades from the release before it. Pre-releases don't count.
    let tags = git(&[
        "tag",
        "--list",
        "v[0-9]*",
        "--merged",
        "HEAD",
        "--no-contains",
        "HEAD",
        "--sort=-v:refname",
    ]);
    let tag = tags.lines().find(|tag| !tag.contains('-'))?;
    let migrations = git(&["ls-tree", "--name-only", tag, "migrations/"])
        .lines()
        .filter_map(|path| {
            let id = path.strip_prefix("migrations/")?.strip_suffix(".sql")?;
            Some((id.to_owned(), git(&["show", &format!("{tag}:./{path}")])))
        })
        .collect();
    Some((tag.trim_start_matches('v').to_owned(), migrations))
}

async fn table_exists(url: &str, name: &str) -> bool {
    let mut conn = PgConnection::connect(url).await.unwrap();
    sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(name)
        .fetch_one(&mut conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn empty_database_gets_the_complete_schema_and_no_data() {
    let (_postgres, url) = postgres().await;

    let report = migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap();

    let applied: Vec<_> = report.applied.into_iter().map(|m| m.id).collect();
    assert!(!applied.is_empty());
    assert_eq!(applied, ids(MIGRATIONS));

    // DB-10: identifier, time applied and application version.
    let mut conn = PgConnection::connect(&url).await.unwrap();
    let history: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT id, app_version, applied_at > now() - interval '1 minute' FROM schema_migration ORDER BY id",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    for (id, app_version, recent) in &history {
        assert_eq!(app_version, env!("CARGO_PKG_VERSION"), "{id}");
        assert!(recent, "{id}");
    }
    assert_eq!(
        history.into_iter().map(|row| row.0).collect::<Vec<_>>(),
        applied
    );

    // DB-08: no demo data or default accounts.
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = current_schema() AND c.relkind = 'r' AND c.relname <> 'schema_migration'",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    for table in tables {
        #[expect(
            clippy::disallowed_methods,
            reason = "the table names come from the catalog"
        )]
        let rows: i64 =
            sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM \"{table}\"")))
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
}

#[tokio::test]
async fn current_schema_applies_nothing() {
    let (_postgres, url) = postgres().await;
    migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap();

    let report = migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap();

    assert!(report.applied.is_empty());
    assert_eq!(applied_ids(&url).await, ids(MIGRATIONS));
}

#[tokio::test]
async fn upgrades_from_the_previous_release() {
    let Some((version, released)) = previous_release() else {
        eprintln!("no release yet; nothing to upgrade from");
        return;
    };
    let released: Vec<_> = released
        .iter()
        .map(|(id, sql)| Migration { id, sql })
        .collect();
    let (_postgres, url) = postgres().await;
    migrate::run(&url, &released, &version, LOCK_TIMEOUT)
        .await
        .unwrap();

    // Fails if a released migration was edited, removed or renamed, or if a new one sorts before
    // it (DB-09, DB-12).
    let report = migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap();

    let applied: Vec<_> = report.applied.into_iter().map(|m| m.id).collect();
    let new: Vec<_> = MIGRATIONS
        .iter()
        .filter(|m| released.iter().all(|r| r.id != m.id))
        .map(|m| m.id.to_owned())
        .collect();
    assert_eq!(applied, new, "upgrading from {version}");
    assert_eq!(applied_ids(&url).await, ids(MIGRATIONS));
}

#[tokio::test]
async fn failed_migration_is_rolled_back() {
    let (_postgres, url) = postgres().await;
    let bad = Migration {
        id: "99990101000001_bad",
        sql: "CREATE TABLE bad_table (id integer); SELECT no_such_column",
    };

    let err = migrate::run(&url, &with(&[OK, bad]), "test", LOCK_TIMEOUT)
        .await
        .unwrap_err();

    assert!(
        matches!(&err, MigrationError::Failed { id, .. } if id == bad.id),
        "{err}"
    );
    assert!(table_exists(&url, "ok_table").await);
    assert!(!table_exists(&url, "bad_table").await);
    assert_eq!(applied_ids(&url).await, ids(&with(&[OK])));
}

#[tokio::test]
async fn migration_blocked_by_a_table_lock_fails_after_the_lock_timeout() {
    let (_postgres, url) = postgres().await;
    migrate::run(&url, &with(&[OK]), "test", LOCK_TIMEOUT)
        .await
        .unwrap();
    // A long transaction holds a lock that the next migration needs.
    let mut blocker = PgConnection::connect(&url).await.unwrap();
    sqlx::raw_sql("BEGIN; LOCK TABLE ok_table IN ACCESS EXCLUSIVE MODE")
        .execute(&mut blocker)
        .await
        .unwrap();
    let blocked = Migration {
        id: "99990101000001_blocked",
        sql: "ALTER TABLE ok_table ADD COLUMN name text",
    };

    let err = migrate::run(
        &url,
        &with(&[OK, blocked]),
        "test",
        Duration::from_millis(100),
    )
    .await
    .unwrap_err();

    // REL-02: the migration gave up instead of queueing traffic behind it.
    let MigrationError::Failed { id, source } = &err else {
        panic!("{err}");
    };
    assert_eq!(id, blocked.id);
    let code = source.as_database_error().and_then(|e| e.code());
    assert_eq!(code.as_deref(), Some("55P03"), "{source}"); // lock_not_available
    assert_eq!(applied_ids(&url).await, ids(&with(&[OK])));
}

#[tokio::test]
async fn unknown_applied_migration_is_refused() {
    let (_postgres, url) = postgres().await;
    // A newer application version applied a migration this one does not have.
    migrate::run(&url, &with(&[OK]), "newer", LOCK_TIMEOUT)
        .await
        .unwrap();

    let err = migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap_err();

    assert!(
        matches!(&err, MigrationError::UnknownMigration { id } if id == OK.id),
        "{err}"
    );
}

#[tokio::test]
async fn changed_applied_migration_is_refused() {
    let (_postgres, url) = postgres().await;
    migrate::run(&url, &with(&[OK]), "test", LOCK_TIMEOUT)
        .await
        .unwrap();
    let changed = Migration {
        sql: "CREATE TABLE ok_table (id bigint)",
        ..OK
    };

    let err = migrate::run(&url, &with(&[changed, LATER]), "test", LOCK_TIMEOUT)
        .await
        .unwrap_err();

    assert!(
        matches!(&err, MigrationError::ChecksumMismatch { id } if id == OK.id),
        "{err}"
    );
    assert!(!table_exists(&url, "later_table").await);
}

#[tokio::test]
async fn pending_migration_older_than_the_newest_applied_is_refused() {
    let (_postgres, url) = postgres().await;
    migrate::run(&url, &with(&[LATER]), "test", LOCK_TIMEOUT)
        .await
        .unwrap();

    let err = migrate::run(&url, &with(&[OK, LATER]), "test", LOCK_TIMEOUT)
        .await
        .unwrap_err();

    assert!(
        matches!(&err, MigrationError::OutOfOrder { id, latest } if id == OK.id && latest == LATER.id),
        "{err}"
    );
    assert!(!table_exists(&url, "ok_table").await);
}

#[tokio::test]
async fn database_with_tables_but_no_history_is_refused() {
    let (_postgres, url) = postgres().await;
    execute(&url, "CREATE TABLE stray (id integer)").await;

    let err = migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap_err();

    assert!(
        matches!(err, MigrationError::NotEmptyWithoutHistory),
        "{err}"
    );
    assert!(!table_exists(&url, "schema_migration").await);
}

#[tokio::test]
async fn concurrent_instances_apply_each_migration_once() {
    let (_postgres, url) = postgres().await;
    // Slow, so the instances overlap; and it fails if applied twice.
    let slow = Migration {
        id: "99990101000000_slow",
        sql: "SELECT pg_sleep(1); CREATE TABLE slow_table (id integer)",
    };
    let migrations = with(&[slow]);

    let mut instances = JoinSet::new();
    for _ in 0..4 {
        let (url, migrations) = (url.clone(), migrations.clone());
        instances.spawn(async move { migrate::run(&url, &migrations, "test", LOCK_TIMEOUT).await });
    }
    let reports: Vec<_> = instances
        .join_all()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    // DB-07: exactly one instance applied the migrations; the others waited and found none.
    let applying: Vec<_> = reports
        .iter()
        .filter(|report| !report.applied.is_empty())
        .collect();
    assert_eq!(applying.len(), 1);
    assert_eq!(applying[0].applied.len(), migrations.len());
    assert_eq!(applied_ids(&url).await, ids(&migrations));
}
