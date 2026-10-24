//! The audit event store against a real PostgreSQL (AUD-02 to AUD-06, AUD-11). Needs Docker.

mod common;

use std::net::{Ipv4Addr, Ipv6Addr};

use common::{execute, postgres};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait, prelude::Uuid,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use testcontainers_modules::{postgres::Postgres, testcontainers::ContainerAsync};
use twoviewer_server::{
    audit::{self, Action, Actor, Event, Origin, Outcome, Target},
    db::{self, migrate},
};

const USER: Uuid = Uuid::from_u128(1);
const DEVICE: Uuid = Uuid::from_u128(2);
const AGENT_INSTANCE: Uuid = Uuid::from_u128(3);
const SESSION: Uuid = Uuid::from_u128(4);
const REQUEST: Uuid = Uuid::from_u128(5);

fn browser() -> Origin {
    Origin {
        ip_address: Ipv4Addr::new(203, 0, 113, 7).into(),
        client_description: "Firefox 140 on macOS".to_owned(),
    }
}

fn device_renamed() -> Event {
    Event {
        actor: Actor::User {
            id: USER,
            origin: browser(),
        },
        action: Action::DeviceRenamed,
        target: Some(Target::Device(DEVICE)),
        outcome: Outcome::Success,
        correlation_id: REQUEST,
        details: json!({}),
    }
}

/// A migrated database and a connection pool to it.
async fn database() -> (ContainerAsync<Postgres>, String, DatabaseConnection) {
    let (postgres, url) = postgres().await;
    migrate::migrate(&url).await.unwrap();
    let db = db::connect(&url).await.unwrap();
    (postgres, url, db)
}

/// The stored events without their timestamps, in the order they were written, read on a
/// connection of their own.
async fn stored(url: &str) -> Vec<Value> {
    let mut conn = PgConnection::connect(url).await.unwrap();
    sqlx::query_scalar("SELECT to_jsonb(e) - 'occurred_at' FROM audit_event e ORDER BY occurred_at")
        .fetch_all(&mut conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn events_are_stored_with_every_field() {
    let (_postgres, url, db) = database().await;
    let event = Event {
        action: Action::SignInFailed,
        target: Some(Target::User(USER)),
        outcome: Outcome::Failure {
            reason_code: "invalid_credentials",
        },
        details: json!({ "attempts": 3 }),
        ..device_renamed()
    };

    // Recorded on the pool, as an event without a state change: it is stored once `record`
    // returns, before any response (AUD-03).
    let id = audit::record(&db, event).await.unwrap();

    // AUD-02.
    assert_eq!(
        stored(&url).await,
        [json!({
            "id": id,
            "actor_type": "user",
            "actor_id": USER,
            "ip_address": "203.0.113.7",
            "client_description": "Firefox 140 on macOS",
            "action": "sign_in_failed",
            "target_type": "user",
            "target_id": USER,
            "outcome": "failure",
            "reason_code": "invalid_credentials",
            "correlation_id": REQUEST,
            "details": { "attempts": 3 },
        })]
    );
    assert_eq!(id.get_version_num(), 4);
    let mut conn = PgConnection::connect(&url).await.unwrap();
    let just_now: bool = sqlx::query_scalar(
        "SELECT occurred_at BETWEEN now() - interval '1 minute' AND now() FROM audit_event",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(just_now);
}

#[tokio::test]
async fn remote_participants_and_the_system_are_actors() {
    let (_postgres, url, db) = database().await;

    let denied = audit::record(
        &db,
        Event {
            actor: Actor::RemoteParticipant {
                agent_instance_id: AGENT_INSTANCE,
                origin: Origin {
                    ip_address: Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 7).into(),
                    client_description: "TwoViewer QuickShare 0.1.0".to_owned(),
                },
            },
            action: Action::ConsentDenied,
            target: Some(Target::Session(SESSION)),
            outcome: Outcome::Success,
            correlation_id: SESSION,
            details: json!({}),
        },
    )
    .await
    .unwrap();
    let migrated = audit::record(
        &db,
        Event {
            actor: Actor::System,
            action: Action::MigrationsApplied,
            target: None,
            outcome: Outcome::Success,
            correlation_id: REQUEST,
            details: json!({ "migrations": ["20261024112041_create_schema_migration"] }),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        stored(&url).await,
        [
            // AUD-11: without an account, the participant is their Host Agent instance and IP.
            json!({
                "id": denied,
                "actor_type": "remote_participant",
                "actor_id": AGENT_INSTANCE,
                "ip_address": "2001:db8::7",
                "client_description": "TwoViewer QuickShare 0.1.0",
                "action": "consent_denied",
                "target_type": "session",
                "target_id": SESSION,
                "outcome": "success",
                "reason_code": null,
                "correlation_id": SESSION,
                "details": {},
            }),
            json!({
                "id": migrated,
                "actor_type": "system",
                "actor_id": null,
                "ip_address": null,
                "client_description": null,
                "action": "migrations_applied",
                "target_type": null,
                "target_id": null,
                "outcome": "success",
                "reason_code": null,
                "correlation_id": REQUEST,
                "details": { "migrations": ["20261024112041_create_schema_migration"] },
            }),
        ]
    );
}

/// A state-changing operation written as features write them: the change and its event in one
/// transaction.
async fn add_widget(db: &DatabaseConnection, name: &str) -> Result<(), audit::Error> {
    let txn = db.begin().await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO widget (name) VALUES ($1)",
        [name.into()],
    ))
    .await?;
    audit::record(&txn, device_renamed()).await?;
    txn.commit().await?;
    Ok(())
}

#[tokio::test]
async fn a_change_is_stored_only_together_with_its_event() {
    let (_postgres, url, db) = database().await;
    execute(&url, "CREATE TABLE widget (name text PRIMARY KEY)").await;
    let widgets = || async {
        let mut conn = PgConnection::connect(&url).await.unwrap();
        sqlx::query_scalar::<_, String>("SELECT name FROM widget ORDER BY name")
            .fetch_all(&mut conn)
            .await
            .unwrap()
    };

    add_widget(&db, "kept").await.unwrap();
    assert_eq!(widgets().await, ["kept"]);
    assert_eq!(stored(&url).await.len(), 1);

    // AUD-03: when the event cannot be written, the operation fails and its change is rolled back.
    execute(
        &url,
        "ALTER TABLE audit_event ADD CONSTRAINT unwritable CHECK (false) NOT VALID",
    )
    .await;
    let err = add_widget(&db, "lost").await.unwrap_err();

    assert!(matches!(err, audit::Error::Database(_)), "{err}");
    assert_eq!(widgets().await, ["kept"]);
    assert_eq!(stored(&url).await.len(), 1);
}

#[tokio::test]
async fn stored_events_can_only_be_deleted() {
    let (_postgres, url, db) = database().await;
    let id = audit::record(&db, device_renamed()).await.unwrap();
    let original = stored(&url).await;
    let mut conn = PgConnection::connect(&url).await.unwrap();

    // AUD-05: the database rejects changes, even those that match no rows.
    for sql in [
        "UPDATE audit_event SET reason_code = 'tampered'",
        "UPDATE audit_event SET reason_code = 'tampered' WHERE false",
        "INSERT INTO audit_event SELECT * FROM audit_event
         ON CONFLICT (id) DO UPDATE SET reason_code = 'tampered'",
        "TRUNCATE audit_event",
    ] {
        let err = sqlx::raw_sql(sql).execute(&mut conn).await.unwrap_err();
        assert_eq!(
            err.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23001"),
            "{sql}: {err}"
        );
    }
    assert_eq!(stored(&url).await, original);

    // The retention job removes events after their retention period (AUD-04, RET-02).
    sqlx::query("DELETE FROM audit_event WHERE id = $1")
        .bind(id)
        .execute(&mut conn)
        .await
        .unwrap();
    assert!(stored(&url).await.is_empty());
}

#[tokio::test]
async fn events_whose_details_name_secrets_are_not_stored() {
    let (_postgres, url, db) = database().await;
    let event = Event {
        action: Action::SessionCodeGenerated,
        target: Some(Target::AgentInstance(AGENT_INSTANCE)),
        details: json!({ "session_code": "123456789" }),
        ..device_renamed()
    };

    let err = audit::record(&db, event).await.unwrap_err();

    // AUD-06.
    assert!(
        matches!(&err, audit::Error::ForbiddenDetail { key } if key == "session_code"),
        "{err}"
    );
    assert!(stored(&url).await.is_empty());
}
