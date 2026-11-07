//! The signed-in user's MFA settings (AUTH-06):
//!
//! - `GET /api/mfa` says whether MFA is on, since when, and how many recovery codes are left.
//! - `POST /api/mfa/setup` starts setting up an authenticator app.
//! - `POST /api/mfa/confirm` turns MFA on with a code from the app, and issues recovery codes.
//! - `DELETE /api/mfa` turns MFA off.
//!
//! Changing them needs a recent authentication (AUTH-10, [`RecentlyAuthenticated`]).

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QuerySelect, Set,
    TransactionTrait, prelude::DateTimeUtc, sea_query::OnConflict,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{changed, factor, qr::QrCode, recovery_code, second_factor::TypedCode, totp::Secret};
use crate::{
    PRODUCT_NAME,
    accounts::{Accounts, RecentlyAuthenticated, SignedIn, signed_in::NOT_SIGNED_IN, user},
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Outcome, Target},
    http::{ClientOrigin, CorrelationId},
};

/// The answer to setting up or confirming an app while MFA is on.
const MFA_ENABLED: ApiError = ApiError::Conflict {
    code: "mfa_enabled",
};

/// The answer to confirming an app that isn't being set up: it was never set up, or MFA was
/// turned off since.
const SETUP_EXPIRED: ApiError = ApiError::Gone {
    code: "mfa_setup_expired",
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::accounts) struct Status {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled_at: Option<DateTimeUtc>,
    /// The recovery codes not used yet.
    recovery_codes_left: u64,
}

/// Answers whether MFA is on for the signed-in user.
pub(in crate::accounts) async fn status(
    State(accounts): State<Arc<Accounts>>,
    signed_in: SignedIn,
) -> Result<Json<Status>, ApiError> {
    let factor = super::second_factor::enabled(&accounts.db, signed_in.user_id).await?;
    let recovery_codes_left = match factor {
        Some(_) => recovery_code::unused(&accounts.db, signed_in.user_id).await?,
        None => 0,
    };
    Ok(Json(Status {
        enabled: factor.is_some(),
        enabled_at: factor.and_then(|factor| factor.confirmed_at),
        recovery_codes_left,
    }))
}

/// Starts setting up an authenticator app: stores a new secret, encrypted and unconfirmed, and
/// answers it as base32 for typing, as an `otpauth://` URI, and as a QR code of that URI. A setup
/// that wasn't confirmed is replaced. `409` (`mfa_enabled`) while MFA is on.
pub(in crate::accounts) async fn setup(
    State(accounts): State<Arc<Accounts>>,
    RecentlyAuthenticated(signed_in): RecentlyAuthenticated,
) -> Result<Json<Value>, ApiError> {
    let user = user::Entity::find_by_id(signed_in.user_id)
        .one(&accounts.db)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    let secret = Secret::generate();
    let stored = factor::ActiveModel {
        user_id: Set(user.id),
        secret_encrypted: Set(accounts.mfa_cipher.encrypt(user.id, &secret)),
        confirmed_at: Set(None),
        last_used_step: Set(None),
        created_at: Set(Utc::now()),
    };
    // Replaces an unconfirmed setup, but never an app that is on, even if one was confirmed
    // concurrently.
    let written = factor::Entity::insert(stored)
        .on_conflict(
            OnConflict::column(factor::Column::UserId)
                .update_columns([
                    factor::Column::SecretEncrypted,
                    factor::Column::ConfirmedAt,
                    factor::Column::LastUsedStep,
                    factor::Column::CreatedAt,
                ])
                .action_and_where(factor::Column::ConfirmedAt.is_null())
                .to_owned(),
        )
        .exec_without_returning(&accounts.db)
        .await?;
    if written == 0 {
        return Err(MFA_ENABLED);
    }

    let uri = secret.uri(PRODUCT_NAME, user.email.as_str());
    let qr_code = QrCode::encode(&uri).ok_or_else(|| {
        ApiError::internal(std::io::Error::other(
            "the otpauth URI is too long for a QR code",
        ))
    })?;
    Ok(Json(json!({
        "secret": &*secret.base32(),
        "uri": &*uri,
        "qrCode": qr_code,
    })))
}

#[derive(Deserialize)]
pub(in crate::accounts) struct ConfirmRequest {
    code: TypedCode,
}

/// Turns MFA on, once a code shows that the app set up with [`setup`] makes the right codes, and
/// answers the new recovery codes, the only time they are shown (AUTH-06). `422` for a code that
/// isn't right, `410` (`mfa_setup_expired`) without a setup, `409` (`mfa_enabled`) while MFA is
/// already on. Records `mfa_enabled`.
pub(in crate::accounts) async fn confirm(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    RecentlyAuthenticated(signed_in): RecentlyAuthenticated,
    request: Result<Json<ConfirmRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) = request?;
    let user_id = signed_in.user_id;
    if request.code.is_blank() {
        return Err(invalid_code("required"));
    }
    let txn = accounts.db.begin().await?;
    let factor = factor::Entity::find_by_id(user_id)
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or(SETUP_EXPIRED)?;
    if factor.is_enabled() {
        return Err(MFA_ENABLED);
    }
    let secret = accounts
        .mfa_cipher
        .decrypt(user_id, &factor.secret_encrypted)
        .map_err(ApiError::internal)?;
    let step = secret
        .verify(
            request.code.expose(),
            Utc::now(),
            accounts.mfa.clock_skew_steps,
            None,
        )
        .ok_or_else(|| invalid_code("invalid"))?;

    let mut factor = factor.into_active_model();
    factor.confirmed_at = Set(Some(Utc::now()));
    factor.last_used_step = Set(Some(step));
    factor.update(&txn).await?;
    let codes = recovery_code::issue(&txn, user_id).await?;
    audit::record(
        &txn,
        Event {
            actor: Actor::User {
                id: user_id,
                origin,
            },
            action: Action::MfaEnabled,
            target: Some(Target::User(user_id)),
            outcome: Outcome::Success,
            correlation_id,
            details: json!({}),
        },
    )
    .await?;
    changed(&txn, user_id).await?;
    // #32: email the user that MFA was turned on (AUTH-12).
    txn.commit().await?;

    let codes: Vec<&str> = codes.iter().map(|code| code.expose()).collect();
    Ok(Json(json!({ "recoveryCodes": codes })))
}

/// Turns MFA off: deletes the app's secret and the recovery codes, and answers `204`. Records
/// `mfa_disabled` if MFA was on; a setup that wasn't confirmed goes too, without an event.
pub(in crate::accounts) async fn disable(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    RecentlyAuthenticated(signed_in): RecentlyAuthenticated,
) -> Result<StatusCode, ApiError> {
    let user_id = signed_in.user_id;
    let txn = accounts.db.begin().await?;
    let deleted = factor::Entity::delete_many()
        .filter(factor::Column::UserId.eq(user_id))
        .exec_with_returning(&txn)
        .await?;
    if deleted.iter().any(factor::Model::is_enabled) {
        // #43: turn off unattended access on all of the user's devices here, after the web
        // client has warned about it (DEV-09).
        audit::record(
            &txn,
            Event {
                actor: Actor::User {
                    id: user_id,
                    origin,
                },
                action: Action::MfaDisabled,
                target: Some(Target::User(user_id)),
                outcome: Outcome::Success,
                correlation_id,
                details: json!({}),
            },
        )
        .await?;
        changed(&txn, user_id).await?;
        // #32: email the user that MFA was turned off (AUTH-12).
    }
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `422` for the field `code`.
fn invalid_code(reason: &'static str) -> ApiError {
    let mut fields = FieldErrors::new();
    fields.insert("code", FieldError::new(reason));
    ApiError::Invalid(fields)
}
