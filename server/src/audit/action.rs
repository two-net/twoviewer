//! The audited actions: exactly those in Table 3.8-A of the SRS (AUD-01).
//!
//! Changes to appearance preferences (theme, motion), Vim mode settings and keyboard shortcuts are
//! not audited (AUD-13), so there is deliberately no action for them. A test pins this list to the
//! table.

use sea_orm::entity::prelude::*;

/// The categories of Table 3.8-A, which users filter their audit events by (AUD-07).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Authentication,
    Account,
    Devices,
    Sessions,
    FileTransfer,
    Vpn,
    Operations,
    System,
}

/// An audited action, stored as its snake_case name. Parentheses in Table 3.8-A name the details
/// an event carries; they are repeated on the variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum Action {
    // Authentication
    SignUp,
    EmailVerified,
    SignInSucceeded,
    SignInFailed,
    SignInBlocked,
    SignOut,
    SignInSessionRevoked,
    Reauthenticated,
    PasswordChanged,
    PasswordResetRequested,
    PasswordResetCompleted,
    MfaEnabled,
    MfaDisabled,
    RecoveryCodeUsed,
    // Account
    DisplayNameChanged,
    EmailChangeRequested,
    EmailChangeCompleted,
    TermsAccepted,
    PersonalDataDownloaded,
    AccountDeleted,
    // Devices
    PairingStarted,
    DevicePaired,
    DeviceRenamed,
    UnattendedAccessTurnedOn,
    UnattendedAccessTurnedOff,
    DeviceRemoved,
    DeviceCredentialRevoked,
    HostAgentVersionChanged,
    // Sessions
    SessionCodeGenerated,
    ConnectionRequested,
    ConsentGranted,
    ConsentDenied,
    ConsentExpired,
    /// Details: type, mode, access, transport.
    SessionStarted,
    /// Details: the new access, view-only or full control.
    AccessChanged,
    SessionReconnected,
    /// Details: reason, duration.
    SessionEnded,
    AbuseReported,
    // File transfer; details: direction, file name, size.
    TransferStarted,
    TransferCompleted,
    TransferFailed,
    TransferCancelled,
    // VPN
    /// Details: devices, virtual addresses, transport.
    VpnStarted,
    /// Details: reason, duration, data transferred.
    VpnEnded,
    // Operations
    UserDetailsViewed,
    AuditEventsSearched,
    /// Details: reason.
    UserSuspended,
    /// Details: reason.
    UserReactivated,
    AbuseReportResolved,
    MaintenanceMessagePublished,
    MaintenanceMessageRemoved,
    // System
    /// Details: version.
    ServiceStarted,
    /// Details: migration identifiers.
    MigrationsApplied,
    MigrationFailed,
    /// Details: rows removed.
    RetentionJobRun,
}

impl Action {
    pub fn category(self) -> Category {
        use Action::*;
        match self {
            SignUp
            | EmailVerified
            | SignInSucceeded
            | SignInFailed
            | SignInBlocked
            | SignOut
            | SignInSessionRevoked
            | Reauthenticated
            | PasswordChanged
            | PasswordResetRequested
            | PasswordResetCompleted
            | MfaEnabled
            | MfaDisabled
            | RecoveryCodeUsed => Category::Authentication,
            DisplayNameChanged
            | EmailChangeRequested
            | EmailChangeCompleted
            | TermsAccepted
            | PersonalDataDownloaded
            | AccountDeleted => Category::Account,
            PairingStarted
            | DevicePaired
            | DeviceRenamed
            | UnattendedAccessTurnedOn
            | UnattendedAccessTurnedOff
            | DeviceRemoved
            | DeviceCredentialRevoked
            | HostAgentVersionChanged => Category::Devices,
            SessionCodeGenerated | ConnectionRequested | ConsentGranted | ConsentDenied
            | ConsentExpired | SessionStarted | AccessChanged | SessionReconnected
            | SessionEnded | AbuseReported => Category::Sessions,
            TransferStarted | TransferCompleted | TransferFailed | TransferCancelled => {
                Category::FileTransfer
            }
            VpnStarted | VpnEnded => Category::Vpn,
            UserDetailsViewed
            | AuditEventsSearched
            | UserSuspended
            | UserReactivated
            | AbuseReportResolved
            | MaintenanceMessagePublished
            | MaintenanceMessageRemoved => Category::Operations,
            ServiceStarted | MigrationsApplied | MigrationFailed | RetentionJobRun => {
                Category::System
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use sea_orm::Iterable;

    use super::*;

    /// Table 3.8-A, one line per action. A new action belongs in the table first (AUD-01, AUD-13).
    const TABLE_3_8_A: &[(Category, &str)] = &[
        (Category::Authentication, "sign_up"),
        (Category::Authentication, "email_verified"),
        (Category::Authentication, "sign_in_succeeded"),
        (Category::Authentication, "sign_in_failed"),
        (Category::Authentication, "sign_in_blocked"),
        (Category::Authentication, "sign_out"),
        (Category::Authentication, "sign_in_session_revoked"),
        (Category::Authentication, "reauthenticated"),
        (Category::Authentication, "password_changed"),
        (Category::Authentication, "password_reset_requested"),
        (Category::Authentication, "password_reset_completed"),
        (Category::Authentication, "mfa_enabled"),
        (Category::Authentication, "mfa_disabled"),
        (Category::Authentication, "recovery_code_used"),
        (Category::Account, "display_name_changed"),
        (Category::Account, "email_change_requested"),
        (Category::Account, "email_change_completed"),
        (Category::Account, "terms_accepted"),
        (Category::Account, "personal_data_downloaded"),
        (Category::Account, "account_deleted"),
        (Category::Devices, "pairing_started"),
        (Category::Devices, "device_paired"),
        (Category::Devices, "device_renamed"),
        (Category::Devices, "unattended_access_turned_on"),
        (Category::Devices, "unattended_access_turned_off"),
        (Category::Devices, "device_removed"),
        (Category::Devices, "device_credential_revoked"),
        (Category::Devices, "host_agent_version_changed"),
        (Category::Sessions, "session_code_generated"),
        (Category::Sessions, "connection_requested"),
        (Category::Sessions, "consent_granted"),
        (Category::Sessions, "consent_denied"),
        (Category::Sessions, "consent_expired"),
        (Category::Sessions, "session_started"),
        (Category::Sessions, "access_changed"),
        (Category::Sessions, "session_reconnected"),
        (Category::Sessions, "session_ended"),
        (Category::Sessions, "abuse_reported"),
        (Category::FileTransfer, "transfer_started"),
        (Category::FileTransfer, "transfer_completed"),
        (Category::FileTransfer, "transfer_failed"),
        (Category::FileTransfer, "transfer_cancelled"),
        (Category::Vpn, "vpn_started"),
        (Category::Vpn, "vpn_ended"),
        (Category::Operations, "user_details_viewed"),
        (Category::Operations, "audit_events_searched"),
        (Category::Operations, "user_suspended"),
        (Category::Operations, "user_reactivated"),
        (Category::Operations, "abuse_report_resolved"),
        (Category::Operations, "maintenance_message_published"),
        (Category::Operations, "maintenance_message_removed"),
        (Category::System, "service_started"),
        (Category::System, "migrations_applied"),
        (Category::System, "migration_failed"),
        (Category::System, "retention_job_run"),
    ];

    #[test]
    fn actions_are_exactly_table_3_8_a() {
        let actions: Vec<_> = Action::iter()
            .map(|action| (action.category(), action.to_value()))
            .collect();
        let table: Vec<_> = TABLE_3_8_A
            .iter()
            .map(|&(category, name)| (category, name.to_owned()))
            .collect();

        assert_eq!(actions, table);
    }

    #[test]
    fn stored_names_are_unique_and_read_back() {
        let names: HashSet<_> = Action::iter().map(|action| action.to_value()).collect();
        assert_eq!(names.len(), Action::iter().count());

        for action in Action::iter() {
            assert_eq!(Action::try_from_value(&action.to_value()).unwrap(), action);
        }
    }
}
