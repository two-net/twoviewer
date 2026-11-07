//! `GET /api/account`: the signed-in user's account.

use std::sync::Arc;

use axum::{Json, extract::State};
use sea_orm::{EntityTrait, prelude::Uuid};
use serde::Serialize;

use super::{
    Accounts,
    signed_in::{NOT_SIGNED_IN, SignedIn},
    user,
};
use crate::api::ApiError;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    id: Uuid,
    email: String,
    display_name: String,
    /// Until it is, the user cannot pair devices or start sessions (AUTH-02).
    email_verified: bool,
}

/// Answers the account of the signed-in user, or `401` without a sign-in.
pub(super) async fn account(
    State(accounts): State<Arc<Accounts>>,
    signed_in: SignedIn,
) -> Result<Json<Account>, ApiError> {
    // Deleting an account deletes its sign-ins, so this finds one but for a concurrent deletion.
    let user = user::Entity::find_by_id(signed_in.user_id)
        .one(&accounts.db)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    Ok(Json(Account {
        id: user.id,
        email_verified: user.is_verified(),
        email: user.email.as_str().to_owned(),
        display_name: user.display_name,
    }))
}
