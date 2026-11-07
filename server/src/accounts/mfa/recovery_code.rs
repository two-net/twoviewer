//! Recovery codes: single-use codes that stand in for the authenticator app, for when the phone
//! is lost (AUTH-05, AUTH-06). Part of MfaFactor in §5.2.
//!
//! Turning MFA on issues [`COUNT`] of them, shown once. Each is 16 characters from Crockford's
//! base32 alphabet, which leaves out letters that look like digits, shown in groups of four:
//! `7k2m-9xqa-...`. That is 80 random bits, so the database stores only their SHA-256 (SEC-07),
//! like the [tokens](crate::accounts::token): a slow hash isn't needed for what cannot be guessed.
//! Typed codes are read generously: case doesn't matter, spaces and hyphens are ignored, and the
//! letters `o`, `i` and `l` are read as the digits they look like.
//!
//! A code is used up by setting `used_at`, in one statement that only matches an unused code, so
//! that two concurrent attempts cannot both use it.

use std::fmt;

use chrono::Utc;
use sea_orm::{ConnectionTrait, DbErr, QueryFilter, Set, entity::prelude::*};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "mfa_recovery_code")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    #[sea_orm(unique_key = "user_code")]
    pub user_id: Uuid,
    /// SHA-256 of the normalized code, never the code itself (SEC-07).
    #[sea_orm(unique_key = "user_code")]
    pub code_hash: Vec<u8>,
    /// When the code was used; `None` while it can be.
    pub used_at: Option<DateTimeUtc>,
    #[sea_orm(belongs_to, from = "user_id", to = "user_id", on_delete = "Cascade")]
    pub factor: BelongsTo<super::factor::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// How many codes turning MFA on issues (AUTH-06).
pub const COUNT: usize = 10;

/// The number of characters in a code, without hyphens: 16 × 5 = 80 random bits.
const LENGTH: usize = 16;

/// Crockford's base32 alphabet, in lowercase: no `i`, `l`, `o` or `u`.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A recovery code as shown to the user, such as `7k2m-9xqa-4b0d-ztw1`.
///
/// It never appears in logs or audit events (MNT-03, AUD-06): `Debug` hides it, there is no
/// `Display`, and its memory is cleared when it is dropped.
pub struct RecoveryCode(Zeroizing<String>);

impl RecoveryCode {
    /// A new code from a cryptographically secure generator, seeded by the operating system.
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; LENGTH]);
        rand::fill(&mut bytes[..]);
        let mut code = Zeroizing::new(String::with_capacity(LENGTH + LENGTH / 4));
        for (index, byte) in bytes.iter().enumerate() {
            if index > 0 && index % 4 == 0 {
                code.push('-');
            }
            // 256 is a multiple of 32, so every character is equally likely.
            code.push(char::from(ALPHABET[usize::from(byte % 32)]));
        }
        Self(code)
    }

    /// The code itself, to show to the user once. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The hash that the database stores for this code.
    pub fn hash(&self) -> Vec<u8> {
        hash(&normalize(&self.0).expect("generated codes are valid"))
    }
}

impl fmt::Debug for RecoveryCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryCode(..)")
    }
}

/// `typed` as stored before hashing: lowercase, without spaces and hyphens, with look-alike
/// letters read as digits. `None` if it cannot be a recovery code.
pub fn normalize(typed: &str) -> Option<Zeroizing<String>> {
    let mut normalized = Zeroizing::new(String::with_capacity(LENGTH));
    for c in typed.chars() {
        let c = match c.to_ascii_lowercase() {
            ' ' | '-' => continue,
            'o' => '0',
            'i' | 'l' => '1',
            c if c.is_ascii() && ALPHABET.contains(&(c as u8)) => c,
            _ => return None,
        };
        if normalized.len() == LENGTH {
            return None;
        }
        normalized.push(c);
    }
    (normalized.len() == LENGTH).then_some(normalized)
}

fn hash(normalized: &str) -> Vec<u8> {
    Sha256::digest(normalized.as_bytes()).to_vec()
}

/// Replaces `user_id`'s recovery codes with [`COUNT`] new ones, and returns them, to show once.
pub async fn issue<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<Vec<RecoveryCode>, DbErr> {
    Entity::delete_many()
        .filter(Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    let codes: Vec<RecoveryCode> = (0..COUNT).map(|_| RecoveryCode::generate()).collect();
    Entity::insert_many(codes.iter().map(|code| ActiveModel {
        user_id: Set(user_id),
        code_hash: Set(code.hash()),
        used_at: Set(None),
        ..Default::default()
    }))
    .exec(db)
    .await?;
    Ok(codes)
}

/// Uses up `user_id`'s unused recovery code `typed`, and returns whether there was one.
pub async fn use_up<C: ConnectionTrait>(db: &C, user_id: Uuid, typed: &str) -> Result<bool, DbErr> {
    let Some(normalized) = normalize(typed) else {
        return Ok(false);
    };
    let used = Entity::update_many()
        .set(ActiveModel {
            used_at: Set(Some(Utc::now())),
            ..Default::default()
        })
        .filter(Column::UserId.eq(user_id))
        .filter(Column::CodeHash.eq(hash(&normalized)))
        .filter(Column::UsedAt.is_null())
        .exec(db)
        .await?;
    Ok(used.rows_affected == 1)
}

/// The number of `user_id`'s recovery codes that are still unused.
pub async fn unused<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<u64, DbErr> {
    Entity::find()
        .filter(Column::UserId.eq(user_id))
        .filter(Column::UsedAt.is_null())
        .count(db)
        .await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn codes_are_random_groups_of_four_from_the_alphabet() {
        let codes: Vec<_> = (0..100).map(|_| RecoveryCode::generate()).collect();

        for code in &codes {
            let groups: Vec<_> = code.expose().split('-').collect();
            assert_eq!(groups.len(), 4, "{}", code.expose());
            for group in groups {
                assert_eq!(group.len(), 4);
                assert!(group.bytes().all(|b| ALPHABET.contains(&b)));
            }
        }
        let distinct: HashSet<_> = codes.iter().map(|code| code.expose().to_owned()).collect();
        assert_eq!(distinct.len(), codes.len());
    }

    #[test]
    fn typed_codes_are_read_generously() {
        let code = RecoveryCode::generate();
        let normalized = normalize(code.expose()).unwrap();

        for typed in [
            code.expose().to_owned(),
            code.expose().to_uppercase(),
            code.expose().replace('-', " "),
            code.expose().replace('-', ""),
            format!("  {}  ", code.expose()),
        ] {
            assert_eq!(normalize(&typed).as_deref(), Some(&*normalized), "{typed}");
        }
        assert_eq!(
            normalize("O1IL-0000-0000-0000")
                .as_deref()
                .map(String::as_str),
            Some("0111000000000000")
        );
    }

    #[test]
    fn other_input_is_not_a_code() {
        for typed in [
            "",
            "123456",
            "0000-0000-0000-000",
            "0000-0000-0000-00000",
            "0000-0000-0000-000u",
            "0000-0000-0000-000é",
            "0000_0000_0000_0000",
        ] {
            assert!(normalize(typed).is_none(), "{typed}");
        }
    }

    #[test]
    fn the_hash_is_of_the_normalized_code() {
        let code = RecoveryCode::generate();

        assert_eq!(
            code.hash(),
            hash(&normalize(&code.expose().to_uppercase()).unwrap())
        );
        assert_eq!(code.hash().len(), 32);
        assert!(!format!("{code:?}").contains(code.expose()));
    }
}
