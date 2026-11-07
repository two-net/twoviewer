//! Proof-of-work challenges for sign-up (AUTH-15).
//!
//! When a network has signed up more often than `sign_up.challenge_after` within
//! `sign_up.window` ([`super::limits`]), its next sign-up must carry a solved challenge. Solving
//! one costs the browser a second or two of computing, which makes signing up in bulk expensive,
//! without a puzzle for people to solve, and without third-party services (PRIV-05).
//!
//! The puzzle: find a `nonce` such that the SHA-256 of the challenge ID's 16 bytes followed by the
//! nonce as 8 big-endian bytes starts with at least `difficulty` zero bits. The web client solves
//! it in `web/src/app/accounts/proof-of-work-solver.ts`, and both sides test the same example.
//!
//! Challenges are stored in the database, so that one instance can check a challenge that
//! another issued (CAP-03), and each can be used only once. They hold no personal data.

use std::time::Duration;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set, entity::prelude::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sign_up_challenge")]
pub struct Model {
    /// Random, so it is also the salt of the puzzle.
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    /// The leading zero bits a solution needs.
    pub difficulty: i32,
    #[sea_orm(indexed)]
    pub expires_at: DateTimeUtc,
}

impl ActiveModelBehavior for ActiveModel {}

/// A challenge as the browser receives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    pub id: Uuid,
    pub difficulty: u32,
}

/// A solved challenge, as the browser sends it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    pub id: Uuid,
    pub nonce: u64,
}

/// Stores a new challenge, valid for `lifetime`, and deletes the expired ones.
pub async fn issue<C: ConnectionTrait>(
    db: &C,
    difficulty: u32,
    lifetime: Duration,
) -> Result<Challenge, DbErr> {
    let now = Utc::now();
    Entity::delete_many()
        .filter(Column::ExpiresAt.lte(now))
        .exec(db)
        .await?;
    let lifetime = chrono::Duration::from_std(lifetime).unwrap_or(chrono::Duration::MAX);
    let stored = ActiveModel {
        difficulty: Set(i32::try_from(difficulty).unwrap_or(i32::MAX)),
        expires_at: Set(now + lifetime),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(Challenge {
        id: stored.id,
        difficulty,
    })
}

/// Whether `solution` solves a challenge that was issued and hasn't expired. Either way, the
/// challenge cannot be used again.
pub async fn redeem<C: ConnectionTrait>(db: &C, solution: Solution) -> Result<bool, DbErr> {
    let deleted = Entity::delete_many()
        .filter(Column::Id.eq(solution.id))
        .filter(Column::ExpiresAt.gt(Utc::now()))
        .exec_with_returning(db)
        .await?;
    Ok(deleted.first().is_some_and(|challenge| {
        solves(
            challenge.id,
            solution.nonce,
            u32::try_from(challenge.difficulty).unwrap_or(u32::MAX),
        )
    }))
}

/// Whether `nonce` solves the challenge `id` at `difficulty`.
pub fn solves(id: Uuid, nonce: u64, difficulty: u32) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    hasher.update(nonce.to_be_bytes());
    leading_zero_bits(&hasher.finalize()) >= difficulty
}

fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        bits += byte.leading_zeros();
        if *byte != 0 {
            break;
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same example is in `web/src/app/accounts/proof-of-work-solver.spec.ts`, so that the
    /// server and the web client agree on the puzzle.
    const EXAMPLE_ID: &str = "6f1c2a7e-3b4d-4c5e-8f90-1a2b3c4d5e6f";
    /// The smallest nonce for difficulty 12. Its hash starts with 0002b5d8, 14 zero bits.
    const EXAMPLE_NONCE: u64 = 46;

    #[test]
    fn the_example_is_solved_by_its_nonce_only() {
        let id = EXAMPLE_ID.parse().unwrap();

        assert!(solves(id, EXAMPLE_NONCE, 12));
        assert!(solves(id, EXAMPLE_NONCE, 14));
        assert!(!solves(id, EXAMPLE_NONCE, 15));
        assert!((0..EXAMPLE_NONCE).all(|nonce| !solves(id, nonce, 12)));
        // Another challenge has other solutions.
        assert!(!solves(Uuid::nil(), EXAMPLE_NONCE, 12));
    }

    #[test]
    fn difficulty_zero_is_always_solved() {
        assert!(solves(Uuid::new_v4(), 0, 0));
    }

    #[test]
    fn zero_bits_are_counted_across_bytes() {
        assert_eq!(leading_zero_bits(&[0xff]), 0);
        assert_eq!(leading_zero_bits(&[0x01, 0x00]), 7);
        assert_eq!(leading_zero_bits(&[0x00, 0x00, 0x10]), 19);
        assert_eq!(leading_zero_bits(&[0x00, 0x00]), 16);
    }

    #[test]
    fn solutions_are_read_from_camel_case() {
        let solution: Solution =
            serde_json::from_str(&format!(r#"{{"id":"{EXAMPLE_ID}","nonce":46}}"#)).unwrap();

        assert_eq!(solution.nonce, 46);
        assert_eq!(
            serde_json::to_value(Challenge {
                id: solution.id,
                difficulty: 18
            })
            .unwrap(),
            serde_json::json!({ "id": EXAMPLE_ID, "difficulty": 18 })
        );
    }
}
