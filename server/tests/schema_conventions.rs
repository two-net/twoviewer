//! The conventions every entity follows (DB-11, DB-13 to DB-15), checked against a real
//! PostgreSQL. Needs Docker.

mod common;

use chrono::Utc;
use common::{execute, postgres};
use sea_orm::{ActiveModelTrait, DbBackend, Schema, Set, SqlErr};
use sqlx::{Connection, PgConnection};
use twoviewer_server::db::{self, EmailAddress, migrate};

/// One line per schema object that breaks a convention, prefixed with the requirement.
const VIOLATIONS: &str = r"
WITH rel AS (
    SELECT c.oid, c.relname, c.relkind
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = current_schema()
)
SELECT format('DB-11 partitioned table %s', relname) FROM rel WHERE relkind = 'p'
UNION ALL
SELECT format('DB-11 table %s inherits from %s', c.relname, p.relname)
FROM pg_inherits i
JOIN rel c ON c.oid = i.inhrelid
JOIN pg_class p ON p.oid = i.inhparent
WHERE c.relkind IN ('r', 'p', 'f')
UNION ALL
SELECT format('DB-11 foreign table %s', relname) FROM rel WHERE relkind = 'f'
UNION ALL
-- Without an index, every delete of a referenced row scans the referencing table.
SELECT format('DB-11 foreign key %s(%s) has no index', c.relname,
              (SELECT string_agg(a.attname, ', ' ORDER BY array_position(k.conkey, a.attnum))
               FROM pg_attribute a WHERE a.attrelid = k.conrelid AND a.attnum = ANY (k.conkey)))
FROM pg_constraint k
JOIN rel c ON c.oid = k.conrelid
WHERE k.contype = 'f' AND NOT EXISTS (
    SELECT FROM pg_index i
    WHERE i.indrelid = k.conrelid AND i.indpred IS NULL
      AND (string_to_array(i.indkey::text, ' ')::int2[])[1:cardinality(k.conkey)] @> k.conkey
)
UNION ALL
SELECT format('DB-13 table %s has no primary key', c.relname)
FROM rel c
WHERE c.relkind IN ('r', 'p')
  AND NOT EXISTS (SELECT FROM pg_constraint k WHERE k.conrelid = c.oid AND k.contype = 'p')
UNION ALL
SELECT format('DB-14 column %s.%s is %s', c.relname, a.attname, format_type(a.atttypid, a.atttypmod))
FROM pg_attribute a
JOIN rel c ON c.oid = a.attrelid
WHERE c.relkind IN ('r', 'p', 'f') AND a.attnum > 0 AND NOT a.attisdropped
  AND a.atttypid IN ('timestamp'::regtype, 'timestamp[]'::regtype)
UNION ALL
-- Covers serial and identity columns, which own a sequence.
SELECT format('DB-15 sequence %s', relname) FROM rel WHERE relkind = 'S'
UNION ALL
SELECT format('DB-15 primary key %s.%s is %s', c.relname, a.attname, format_type(a.atttypid, a.atttypmod))
FROM pg_constraint k
JOIN rel c ON c.oid = k.conrelid
JOIN pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = k.conkey[1]
WHERE k.contype = 'p' AND cardinality(k.conkey) = 1
  AND a.atttypid IN ('smallint'::regtype, 'integer'::regtype, 'bigint'::regtype, 'numeric'::regtype)
UNION ALL
-- A key that is also a foreign key takes its value from the referenced row.
SELECT format('DB-15 primary key %s.%s has no gen_random_uuid() default', c.relname, a.attname)
FROM pg_constraint k
JOIN rel c ON c.oid = k.conrelid
JOIN pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = k.conkey[1]
LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
WHERE k.contype = 'p' AND cardinality(k.conkey) = 1 AND a.atttypid = 'uuid'::regtype
  AND pg_get_expr(d.adbin, d.adrelid) IS DISTINCT FROM 'gen_random_uuid()'
  AND NOT EXISTS (
      SELECT FROM pg_constraint f WHERE f.conrelid = k.conrelid AND f.contype = 'f' AND f.conkey = k.conkey
  )
";

async fn violations(url: &str) -> Vec<String> {
    let mut conn = PgConnection::connect(url).await.unwrap();
    let mut lines: Vec<String> = sqlx::query_scalar(VIOLATIONS)
        .fetch_all(&mut conn)
        .await
        .unwrap();
    lines.sort();
    lines
}

/// An entity that follows every convention, as the reference for new ones.
mod example {
    use sea_orm::entity::prelude::*;
    use twoviewer_server::db::EmailAddress;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "convention_example")]
    pub struct Model {
        #[sea_orm(
            primary_key,
            auto_increment = false,
            default_expr = "Expr::cust(\"gen_random_uuid()\")"
        )]
        pub id: Uuid,
        #[sea_orm(unique, extra = "CHECK (email = lower(email))")]
        pub email: EmailAddress,
        pub created_at: DateTimeUtc,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

#[tokio::test]
async fn migrated_schema_follows_the_conventions() {
    let (_postgres, url) = postgres().await;

    migrate::migrate(&url).await.unwrap();

    assert_eq!(violations(&url).await, Vec::<String>::new());
}

#[tokio::test]
async fn violations_are_detected() {
    let (_postgres, url) = postgres().await;
    execute(
        &url,
        "CREATE TABLE event (id uuid NOT NULL DEFAULT gen_random_uuid(), at timestamptz NOT NULL,
                             PRIMARY KEY (id, at)) PARTITION BY RANGE (at);
         CREATE TABLE event_2026 PARTITION OF event FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
         CREATE TABLE base (id uuid PRIMARY KEY DEFAULT gen_random_uuid());
         CREATE TABLE derived (PRIMARY KEY (id)) INHERITS (base);
         CREATE FOREIGN DATA WRAPPER elsewhere;
         CREATE SERVER shard FOREIGN DATA WRAPPER elsewhere;
         CREATE FOREIGN TABLE remote (id uuid) SERVER shard;
         CREATE TABLE child (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                             base_id uuid NOT NULL REFERENCES base (id));
         CREATE TABLE note (body text, written_at timestamp);
         CREATE TABLE counter (id serial PRIMARY KEY);
         CREATE TABLE token (id uuid PRIMARY KEY);
         -- Conforms: a key that is also a foreign key needs no default, and its index covers it.
         CREATE TABLE profile (id uuid PRIMARY KEY REFERENCES base (id));",
    )
    .await;

    assert_eq!(
        violations(&url).await,
        [
            "DB-11 foreign key child(base_id) has no index",
            "DB-11 foreign table remote",
            "DB-11 partitioned table event",
            "DB-11 table derived inherits from base",
            "DB-11 table event_2026 inherits from event",
            "DB-13 table note has no primary key",
            "DB-14 column note.written_at is timestamp without time zone",
            "DB-15 primary key counter.id is integer",
            "DB-15 primary key token.id has no gen_random_uuid() default",
            "DB-15 sequence counter_id_seq",
        ]
    );
}

#[tokio::test]
async fn connections_use_utc() {
    let (_postgres, url) = postgres().await;
    execute(
        &url,
        "ALTER DATABASE postgres SET timezone = 'Asia/Bangkok'",
    )
    .await;

    let db = db::connect(&url).await.unwrap();
    let timezone: String = sqlx::query_scalar("SHOW TimeZone")
        .fetch_one(db.get_postgres_connection_pool())
        .await
        .unwrap();

    // DB-14: timestamps are read and written in UTC whatever the server's default.
    assert_eq!(timezone, "UTC");
}

#[tokio::test]
async fn email_addresses_are_unique_regardless_of_case() {
    let (_postgres, url) = postgres().await;
    let db = db::connect(&url).await.unwrap();
    Schema::new(DbBackend::Postgres)
        .builder()
        .register(example::Entity)
        .apply(&db)
        .await
        .unwrap();
    let insert = |email: &str| example::ActiveModel {
        email: Set(EmailAddress::new(email)),
        created_at: Set(Utc::now()),
        ..Default::default()
    };

    let ann = insert("Ann@Example.com").insert(&db).await.unwrap();
    let err = insert(" ann@example.COM ").insert(&db).await.unwrap_err();

    // DB-15: the database generated a random identifier.
    assert_eq!(ann.id.get_version_num(), 4);
    assert_eq!(ann.email.as_str(), "ann@example.com");
    assert!(
        matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))),
        "{err}"
    );

    // An address that bypasses `EmailAddress` is rejected by the database.
    let err = sqlx::query(
        "INSERT INTO convention_example (email, created_at) VALUES ('Bob@Example.com', now())",
    )
    .execute(db.get_postgres_connection_pool())
    .await
    .unwrap_err();
    assert_eq!(
        err.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514"),
        "{err}"
    );

    assert_eq!(violations(&url).await, Vec::<String>::new());
}
