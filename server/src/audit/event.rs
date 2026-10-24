//! Stored audit events (AUD-02).
//!
//! Only [`super::record`] writes this table, and nothing updates it: events are append-only
//! (AUD-04), and the migration that creates the table adds a trigger that rejects updates
//! (AUD-05). There are no foreign keys, because events outlive the users, devices and sessions
//! they refer to (PRIV-03, RET-01).

use sea_orm::entity::prelude::*;

use super::Action;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_event")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    /// Set by the database. In a transaction, this is the transaction's time, so an event carries
    /// the same time as the change it records.
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub occurred_at: DateTimeUtc,
    pub actor_type: ActorType,
    /// `None` only for the system.
    pub actor_id: Option<Uuid>,
    /// `None` only for the system.
    pub ip_address: Option<IpNetwork>,
    /// Browser and operating system, or Host Agent version. `None` only for the system.
    pub client_description: Option<String>,
    pub action: Action,
    pub target_type: Option<TargetType>,
    pub target_id: Option<Uuid>,
    pub outcome: Outcome,
    /// Set only on failure.
    pub reason_code: Option<String>,
    /// The request or session the event belongs to.
    pub correlation_id: Uuid,
    #[sea_orm(column_type = "JsonBinary")]
    pub details: Json,
}

impl ActiveModelBehavior for ActiveModel {}

// Text rather than PostgreSQL enums: schema sync cannot add values to an existing enum type.

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum ActorType {
    User,
    RemoteParticipant,
    Operator,
    HostAgent,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum TargetType {
    User,
    Device,
    AgentInstance,
    Session,
    AbuseReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum Outcome {
    Success,
    Failure,
}
