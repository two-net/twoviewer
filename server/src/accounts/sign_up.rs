//! `POST /api/sign-up`: creating an account (AUTH-01), protected against automated abuse
//! (AUTH-15), without revealing whether an address is registered (SEC-14).

use std::{net::IpAddr, sync::Arc, time::Instant};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use lettre::Address;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait, TryInsertResult};
use serde::Deserialize;
use serde_json::{Map, json};

use super::{
    Accounts, RESET_PASSWORD_PAGE, SIGN_IN_PAGE, VERIFY_EMAIL_PAGE,
    challenge::{self, Solution},
    email_link::{self, LinkToken, Purpose},
    legal,
    limits::Admission,
    password::{NewPasswordError, Password, Rejected},
    user,
};
use crate::{
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Outcome, Target},
    db::EmailAddress,
    email::{AccountExists, VerifyEmail},
    http::{ClientOrigin, CorrelationId},
};

/// The longest email address that can be delivered (RFC 5321: a 256-octet path, less the angle
/// brackets).
const MAX_EMAIL_LENGTH: usize = 254;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Request {
    email: String,
    display_name: String,
    password: Password,
    /// The versions of the Terms of Service and Privacy Notice the user accepted (AUTH-01).
    terms_version: String,
    privacy_version: String,
    /// A solved challenge, when one was asked for.
    #[serde(default)]
    challenge: Option<Solution>,
}

/// A request that meets the rules.
struct Form {
    email: EmailAddress,
    display_name: String,
    password: Password,
}

/// Creates an account, and answers `202 Accepted` with no body, the same whether or not the
/// address was already registered (SEC-14):
///
/// - A new address gets an account, unverified, and an email with a link that verifies it.
/// - An address with an unverified account gets a new link. The account keeps its password and
///   display name: overwriting them would let someone who knows the address set the password of
///   an account that its owner then verifies.
/// - An address with a verified account gets an email that says so, with links to sign in and to
///   reset the password.
///
/// The password is hashed in every case, before the address is looked up, so that every answer
/// takes about as long.
pub(super) async fn sign_up(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    request: Result<Json<Request>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    let form = accounts.validate(&request)?;
    accounts.admit(origin.ip_address, request.challenge).await?;

    let password_hash = match accounts.passwords.hash_new(&form.password).await {
        Ok(hash) => hash,
        // `validate` has already checked the rules.
        Err(NewPasswordError::Rejected(rejected)) => {
            return Err(ApiError::Invalid(password_error(rejected)));
        }
        Err(NewPasswordError::Hashing(err)) => return Err(ApiError::internal(err)),
    };
    let notify = accounts.limits.may_notify(&form.email, Instant::now());

    let txn = accounts.db.begin().await?;
    let now = Utc::now();
    let inserted = user::Entity::insert(user::ActiveModel {
        email: Set(form.email.clone()),
        password_hash: Set(password_hash),
        display_name: Set(form.display_name),
        accepted_terms_version: Set(legal::TERMS_VERSION.to_owned()),
        accepted_privacy_version: Set(legal::PRIVACY_VERSION.to_owned()),
        terms_accepted_at: Set(now),
        created_at: Set(now),
        ..Default::default()
    })
    // A concurrent sign-up with the same address waits for this one, then finds the account.
    // `exec`, since `exec_with_returning` reports a conflict as an error rather than `Conflicted`.
    .on_conflict_do_nothing_on([user::Column::Email])
    .exec(&txn)
    .await?;

    match inserted {
        TryInsertResult::Inserted(inserted) => {
            let user_id = inserted.last_insert_id;
            let token = accounts.verification_link(&txn, user_id).await?;
            for (action, details) in [
                (Action::SignUp, json!({})),
                (
                    Action::TermsAccepted,
                    json!({
                        "terms_version": legal::TERMS_VERSION,
                        "privacy_version": legal::PRIVACY_VERSION,
                    }),
                ),
            ] {
                audit::record(
                    &txn,
                    Event {
                        actor: Actor::User {
                            id: user_id,
                            origin: origin.clone(),
                        },
                        action,
                        target: Some(Target::User(user_id)),
                        outcome: Outcome::Success,
                        correlation_id,
                        details,
                    },
                )
                .await?;
            }
            txn.commit().await?;
            // Always: the address has had no email from sign-up yet.
            accounts.send_verification(&form.email, &token, false);
        }
        TryInsertResult::Conflicted | TryInsertResult::Empty => {
            let existing = user::Entity::find()
                .filter(user::Column::Email.eq(form.email.clone()))
                .one(&txn)
                .await?;
            match existing {
                Some(user) if notify && !user.is_verified() => {
                    let token = accounts.verification_link(&txn, user.id).await?;
                    txn.commit().await?;
                    accounts.send_verification(&form.email, &token, true);
                }
                Some(user) if notify => {
                    debug_assert!(user.is_verified());
                    txn.commit().await?;
                    let (sign_in_url, reset_url) = (
                        accounts.public_url.join(SIGN_IN_PAGE),
                        accounts.public_url.join(RESET_PASSWORD_PAGE),
                    );
                    accounts.mailer.send(
                        &form.email,
                        &AccountExists {
                            sign_in_url: &sign_in_url,
                            reset_url: &reset_url,
                        },
                    );
                }
                // The address had an email from sign-up recently, or its account was deleted in
                // the meantime.
                _ => txn.commit().await?,
            }
        }
    }
    Ok(StatusCode::ACCEPTED)
}

impl Accounts {
    /// Checks every field, and reports every problem at once.
    fn validate(&self, request: &Request) -> Result<Form, ApiError> {
        let mut fields = FieldErrors::new();

        let email = request.email.trim();
        if email.is_empty() {
            fields.insert("email", FieldError::new("required"));
        } else if email.chars().count() > MAX_EMAIL_LENGTH || email.parse::<Address>().is_err() {
            fields.insert("email", FieldError::new("invalid"));
        }

        let display_name = request.display_name.trim();
        let max = self.config.display_name_max_length;
        if display_name.is_empty() {
            fields.insert("displayName", FieldError::new("required"));
        } else if display_name.chars().count() > max {
            fields.insert("displayName", FieldError::max("too_long", max));
        } else if display_name.chars().any(char::is_control) {
            fields.insert("displayName", FieldError::new("invalid"));
        }

        if let Err(rejected) = self.passwords.check_new(&request.password) {
            fields.extend(password_error(rejected));
        }

        if request.terms_version.is_empty() || request.privacy_version.is_empty() {
            fields.insert("terms", FieldError::new("required"));
        }
        if !fields.is_empty() {
            return Err(ApiError::Invalid(fields));
        }
        // The user accepted texts that have changed since: they must read the new ones.
        if request.terms_version != legal::TERMS_VERSION
            || request.privacy_version != legal::PRIVACY_VERSION
        {
            return Err(ApiError::Conflict {
                code: "terms_changed",
            });
        }

        Ok(Form {
            email: EmailAddress::new(email),
            display_name: display_name.to_owned(),
            password: request.password.clone(),
        })
    }

    /// Counts a sign-up from `address`, and lets it through if the network may sign up, if
    /// necessary with a solved challenge (AUTH-15).
    async fn admit(&self, address: IpAddr, solution: Option<Solution>) -> Result<(), ApiError> {
        match self.limits.admit(address, Instant::now()) {
            Admission::Free => Ok(()),
            Admission::Refused { retry_after } => Err(ApiError::TooManyRequests { retry_after }),
            Admission::Challenge => {
                if let Some(solution) = solution
                    && challenge::redeem(&self.db, solution).await?
                {
                    return Ok(());
                }
                let challenge = challenge::issue(
                    &self.db,
                    self.sign_up.challenge_difficulty,
                    self.sign_up.challenge_lifetime,
                )
                .await?;
                let mut details = Map::new();
                details.insert("challenge".to_owned(), json!(challenge));
                Err(ApiError::PreconditionRequired {
                    code: "challenge_required",
                    details,
                })
            }
        }
    }

    async fn verification_link<C: sea_orm::ConnectionTrait>(
        &self,
        db: &C,
        user_id: sea_orm::prelude::Uuid,
    ) -> Result<LinkToken, sea_orm::DbErr> {
        email_link::create(
            db,
            user_id,
            Purpose::VerifyEmail,
            self.config.verification_link_lifetime,
        )
        .await
    }

    fn send_verification(&self, to: &EmailAddress, token: &LinkToken, repeated: bool) {
        // In the fragment, which browsers never send to a server, so the token doesn't reach
        // server or proxy logs, or other sites in the `Referer` header.
        let link = format!(
            "{}#{}",
            self.public_url.join(VERIFY_EMAIL_PAGE),
            token.expose()
        );
        self.mailer.send(
            to,
            &VerifyEmail {
                link: &link,
                valid_for: self.config.verification_link_lifetime,
                repeated,
            },
        );
    }
}

fn password_error(rejected: Rejected) -> FieldErrors {
    let error = match rejected {
        Rejected::TooShort { min } => FieldError::min("too_short", min),
        Rejected::TooLong { max } => FieldError::max("too_long", max),
        Rejected::Compromised => FieldError::new("compromised"),
    };
    FieldErrors::from([("password", error)])
}
