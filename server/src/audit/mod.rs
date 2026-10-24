//! The audit trail (§3.8): who did what, when, from where, and with what result.
//!
//! Every feature records its events with [`record`], which only ever appends (AUD-04). Write the
//! event for a state-changing operation in the operation's own transaction, so that the operation
//! fails if the event cannot be written (AUD-03):
//!
//! ```ignore
//! let txn = db.begin().await?;
//! device.update(&txn).await?;
//! audit::record(&txn, event).await?;
//! txn.commit().await?;
//! ```
//!
//! An event without a state change, such as a failed sign-in, is recorded on the connection pool
//! before the response is returned.
//!
//! Events never contain passwords, codes, tokens, keys or session content (AUD-06). Identifiers,
//! addresses and reason codes are typed, so only `client_description` and `details` hold free
//! text. Events refer to people by ID, never by name or email address, so pseudonymizing a deleted
//! account (PRIV-03) leaves stored events untouched. [`record`] rejects `details` keys that name
//! such content. That check is a safety net, not a substitute for review.

mod action;
pub(crate) mod event;

pub use action::{Action, Category};

use std::net::IpAddr;

use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DbErr, Set,
    prelude::{IpNetwork, Uuid},
};
use serde_json::Value;

/// An event to record (AUD-02). Its identifier and timestamp are assigned when it is stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub actor: Actor,
    pub action: Action,
    pub target: Option<Target>,
    pub outcome: Outcome,
    /// The request or session the event belongs to.
    pub correlation_id: Uuid,
    /// Action-specific details as a JSON object with snake_case keys; `{}` when there are none.
    pub details: Value,
}

/// Who performed the action. Everyone but the system acts from somewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    User {
        id: Uuid,
        origin: Origin,
    },
    /// A remote participant without an account, identified by their Host Agent instance (AUD-11).
    RemoteParticipant {
        agent_instance_id: Uuid,
        origin: Origin,
    },
    Operator {
        id: Uuid,
        origin: Origin,
    },
    HostAgent {
        instance_id: Uuid,
        origin: Origin,
    },
    System,
}

/// Where a request came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub ip_address: IpAddr,
    /// Browser and operating system, or Host Agent version.
    pub client_description: String,
}

/// What the action was performed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    User(Uuid),
    Device(Uuid),
    AgentInstance(Uuid),
    Session(Uuid),
    AbuseReport(Uuid),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// The reason code is a snake_case constant such as `invalid_credentials`, never runtime data.
    Failure {
        reason_code: &'static str,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] DbErr),
    #[error("audit event details must be a JSON object")]
    DetailsNotObject,
    #[error(
        "audit event details may not use the key {key:?}: keys are snake_case and never name \
         secrets, session content or email addresses (AUD-06)"
    )]
    ForbiddenDetail { key: String },
}

/// Words that may not appear in a `details` key: content an event must never hold (AUD-06), and
/// email addresses, which would otherwise have to be pseudonymized in stored events (PRIV-03).
const FORBIDDEN_KEY_WORDS: &[&str] = &[
    "password",
    "passwords",
    "passphrase",
    "passcode",
    "code",
    "codes",
    "otp",
    "token",
    "tokens",
    "secret",
    "secrets",
    "key",
    "keys",
    "cookie",
    "cookies",
    "keystroke",
    "keystrokes",
    "clipboard",
    "content",
    "email",
];

/// Appends `event` to the audit trail and returns its identifier.
///
/// Pass the transaction of the change the event records, or the connection pool for an event
/// without a state change (AUD-03).
pub async fn record<C: ConnectionTrait>(db: &C, event: Event) -> Result<Uuid, Error> {
    check_details(&event.details)?;

    let (actor_type, actor_id, origin) = match event.actor {
        Actor::User { id, origin } => (event::ActorType::User, Some(id), Some(origin)),
        Actor::RemoteParticipant {
            agent_instance_id,
            origin,
        } => (
            event::ActorType::RemoteParticipant,
            Some(agent_instance_id),
            Some(origin),
        ),
        Actor::Operator { id, origin } => (event::ActorType::Operator, Some(id), Some(origin)),
        Actor::HostAgent {
            instance_id,
            origin,
        } => (event::ActorType::HostAgent, Some(instance_id), Some(origin)),
        Actor::System => (event::ActorType::System, None, None),
    };
    let (target_type, target_id) = match event.target {
        Some(Target::User(id)) => (Some(event::TargetType::User), Some(id)),
        Some(Target::Device(id)) => (Some(event::TargetType::Device), Some(id)),
        Some(Target::AgentInstance(id)) => (Some(event::TargetType::AgentInstance), Some(id)),
        Some(Target::Session(id)) => (Some(event::TargetType::Session), Some(id)),
        Some(Target::AbuseReport(id)) => (Some(event::TargetType::AbuseReport), Some(id)),
        None => (None, None),
    };
    let (outcome, reason_code) = match event.outcome {
        Outcome::Success => (event::Outcome::Success, None),
        Outcome::Failure { reason_code } => (event::Outcome::Failure, Some(reason_code.to_owned())),
    };
    let (ip_address, client_description) = match origin {
        Some(origin) => (
            Some(IpNetwork::from(origin.ip_address)),
            Some(origin.client_description),
        ),
        None => (None, None),
    };

    let stored = event::ActiveModel {
        actor_type: Set(actor_type),
        actor_id: Set(actor_id),
        ip_address: Set(ip_address),
        client_description: Set(client_description),
        action: Set(event.action),
        target_type: Set(target_type),
        target_id: Set(target_id),
        outcome: Set(outcome),
        reason_code: Set(reason_code),
        correlation_id: Set(event.correlation_id),
        details: Set(event.details),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(stored.id)
}

fn check_details(details: &Value) -> Result<(), Error> {
    if !details.is_object() {
        return Err(Error::DetailsNotObject);
    }
    check_keys(details)
}

/// Checks the keys of every object in `value`, at any depth.
fn check_keys(value: &Value) -> Result<(), Error> {
    match value {
        Value::Object(map) => map.iter().try_for_each(|(key, value)| {
            if !is_allowed_key(key) {
                return Err(Error::ForbiddenDetail { key: key.clone() });
            }
            check_keys(value)
        }),
        Value::Array(items) => items.iter().try_for_each(check_keys),
        _ => Ok(()),
    }
}

/// A snake_case key, so it can be checked word by word, with no forbidden word.
fn is_allowed_key(key: &str) -> bool {
    let snake_case = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    snake_case
        && !key
            .split('_')
            .any(|word| FORBIDDEN_KEY_WORDS.contains(&word))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn forbidden_key(details: Value) -> Option<String> {
        match check_details(&details) {
            Err(Error::ForbiddenDetail { key }) => Some(key),
            _ => None,
        }
    }

    #[test]
    fn details_that_name_secrets_or_session_content_are_rejected() {
        for (details, key) in [
            (json!({ "password": "hunter2" }), "password"),
            (json!({ "session_code": "123456789" }), "session_code"),
            (json!({ "recovery_codes": [] }), "recovery_codes"),
            (json!({ "api_key": "k" }), "api_key"),
            (json!({ "clipboard": "text" }), "clipboard"),
            (json!({ "file_content": "…" }), "file_content"),
            (json!({ "owner_email": "ann@example.com" }), "owner_email"),
            (json!({ "devices": [{ "id": 1, "token": "t" }] }), "token"),
            (json!({ "session": { "mfa": { "otp": "1" } } }), "otp"),
            // Keys that are not snake_case cannot be checked word by word.
            (json!({ "sessionCode": "1" }), "sessionCode"),
            (json!({ "Password": "1" }), "Password"),
        ] {
            assert_eq!(forbidden_key(details).as_deref(), Some(key));
        }
    }

    #[test]
    fn details_without_secrets_are_accepted() {
        let details = json!({
            "file_name": "report.pdf",
            "size": 1024,
            "direction": "upload",
            "reason": "ended_by_controller",
            "bytes_transferred": 5_000_000,
            "migrations": ["20261024112041_create_schema_migration"],
            "devices": [{ "id": "6f1c…", "virtual_address": "10.77.0.2" }],
            "keyboard_layout": "de",
        });

        assert!(check_details(&details).is_ok());
        assert!(check_details(&json!({})).is_ok());
    }

    #[test]
    fn details_must_be_an_object() {
        for details in [json!(null), json!([]), json!("text"), json!(1)] {
            assert!(matches!(
                check_details(&details),
                Err(Error::DetailsNotObject)
            ));
        }
    }
}
