//! Single-use links sent by email (EmailLink in §5.2), such as the one that verifies an address
//! (AUTH-02).
//!
//! A link carries a random [`LinkToken`]. The database stores only its SHA-256 (SEC-07), so a
//! copy of the database cannot be used to open links. A slow hash isn't needed: the token is 256
//! random bits, which cannot be guessed, unlike a password. Links are kept for 24 hours after use
//! or expiry, then deleted by the retention job (Table 5.3-A, #66).

use std::fmt::{self, Write as _};

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set, entity::prelude::*};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The number of random bytes in a token.
const TOKEN_BYTES: usize = 32;

/// The longest token that is worth looking up: longer ones were never issued.
pub const MAX_TOKEN_LENGTH: usize = 2 * TOKEN_BYTES;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "email_link")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub user_id: Uuid,
    pub purpose: Purpose,
    /// SHA-256 of the token, never the token itself (SEC-07).
    #[sea_orm(unique)]
    pub token_hash: Vec<u8>,
    pub expires_at: DateTimeUtc,
    /// When the link was used; `None` until then.
    pub used_at: Option<DateTimeUtc>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(belongs_to, from = "user_id", to = "id", on_delete = "Cascade")]
    pub user: BelongsTo<super::user::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// What a link does. Stored as text: schema sync cannot add values to a PostgreSQL enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum Purpose {
    /// Verifies the email address of a new account (AUTH-02).
    VerifyEmail,
}

/// The secret part of a link: 32 random bytes, as 64 hexadecimal digits.
///
/// It never appears in logs or audit events (MNT-03, AUD-06): `Debug` hides it, there is no
/// `Display`, and its memory is cleared when it is dropped.
pub struct LinkToken(Zeroizing<String>);

impl LinkToken {
    /// A new token from a cryptographically secure generator, seeded by the operating system.
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
        rand::fill(&mut bytes[..]);
        let mut token = Zeroizing::new(String::with_capacity(MAX_TOKEN_LENGTH));
        for byte in bytes.iter() {
            write!(token, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(token)
    }

    /// The token itself, to put in the link. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The hash that the database stores for this token.
    pub fn hash(&self) -> Vec<u8> {
        hash(&self.0)
    }
}

impl fmt::Debug for LinkToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkToken(..)")
    }
}

/// The hash under which the database stores the token `token`, as received in a request.
pub fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Stores a new link for `purpose` to `user_id`, valid for `lifetime`, and returns its token.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    purpose: Purpose,
    lifetime: std::time::Duration,
) -> Result<LinkToken, DbErr> {
    let token = LinkToken::generate();
    let lifetime = chrono::Duration::from_std(lifetime).unwrap_or(chrono::Duration::MAX);
    ActiveModel {
        user_id: Set(user_id),
        purpose: Set(purpose),
        token_hash: Set(token.hash()),
        expires_at: Set(Utc::now() + lifetime),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (LinkToken::generate(), LinkToken::generate());

        assert_ne!(a.expose(), b.expose());
        assert_eq!(a.expose().len(), MAX_TOKEN_LENGTH);
        assert!(
            a.expose()
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn the_hash_is_the_sha256_of_the_token_as_received() {
        let token = LinkToken::generate();

        assert_eq!(token.hash(), hash(token.expose()));
        assert_eq!(token.hash().len(), 32);
        assert_ne!(token.hash(), token.expose().as_bytes());
        // SHA-256 of "abc" (FIPS 180-2, appendix B.1).
        assert_eq!(
            hash("abc")[..4],
            [0xba, 0x78, 0x16, 0xbf],
            "{:02x?}",
            hash("abc")
        );
    }

    #[test]
    fn tokens_are_not_formatted() {
        let token = LinkToken::generate();

        assert!(!format!("{token:?}").contains(token.expose()));
    }
}
