//! Passwords: the rules for new ones (AUTH-03, AUTH-04) and how they are stored (SEC-07).
//!
//! # Normalization
//!
//! A [`Password`] is normalized to Unicode NFKC as soon as it is received (NIST SP 800-63B-4
//! §3.1.1.2). That way the same characters give the same hash however a keyboard or operating
//! system encodes them, such as `é` as one code point or as `e` and an accent. Nothing else is
//! changed: spaces, including leading, trailing and repeated ones, and case count, and nothing is
//! cut off (ASVS 5.0 V6.2.8). Lengths count code points of the normalized password.
//!
//! # Hashing
//!
//! Passwords are stored only as Argon2id hashes, a slow, memory-hard function, each with its own
//! random salt. A [`PasswordHash`] is a PHC string, which records the parameters it was made with,
//! so the settings can change without breaking existing hashes, which are renewed at their next
//! sign-in.
//!
//! Hashing keeps a CPU core busy for tens of milliseconds and takes `passwords.hash_memory_kib` of
//! memory, so it runs on the blocking thread pool, at most `passwords.max_concurrent_hashes` at
//! once. Requests beyond that wait, so a flood of sign-in attempts can slow sign-in down but not
//! exhaust memory.

use std::{fmt, sync::Arc};

use argon2::{
    Algorithm, Argon2, Params, PasswordHasher as _, PasswordVerifier as _, Version,
    password_hash::{self, phc},
};
use sea_orm::DeriveValueType;
use serde::Deserialize;
use tokio::sync::Semaphore;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use super::compromised;
use crate::config;

/// The salt for the hash computed in place of a verification when there is no stored hash. The
/// result is thrown away, so the salt doesn't need to be secret or unique.
const UNUSED_SALT: [u8; argon2::RECOMMENDED_SALT_LEN] = [0; argon2::RECOMMENDED_SALT_LEN];

/// A password as typed, normalized (see the [module documentation](self)).
///
/// Its memory is cleared when it is dropped. It never appears in logs: `Debug` hides it, and there
/// is no `Display`. Request bodies can deserialize it directly from a string.
#[derive(Clone, Deserialize)]
#[serde(from = "String")]
pub struct Password(Zeroizing<String>);

impl Password {
    pub fn new(typed: &str) -> Self {
        let mut normalized = Zeroizing::new(String::with_capacity(typed.len()));
        normalized.extend(typed.nfkc());
        Self(normalized)
    }

    /// The number of characters, meaning Unicode code points, that the length rules count.
    pub fn char_count(&self) -> usize {
        self.0.chars().count()
    }

    /// The password itself. Never log it.
    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Password {
    fn from(typed: String) -> Self {
        Self::new(&Zeroizing::new(typed))
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(..)")
    }
}

/// A password as stored: a PHC string, `$argon2id$v=19$m=<KiB>,t=<passes>,p=<lanes>$<salt>$<hash>`.
///
/// Only [`Passwords`] makes one, and it never appears in logs: `Debug` hides it, and there is no
/// `Display`. Declare the column as a plain `password_hash: PasswordHash`.
#[derive(Clone, PartialEq, Eq, DeriveValueType)]
pub struct PasswordHash(String);

impl fmt::Debug for PasswordHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PasswordHash(..)")
    }
}

/// Why a new password is not allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejected {
    /// Fewer characters than `passwords.min_length` (AUTH-03).
    #[error("the password has fewer than {min} characters")]
    TooShort { min: usize },
    /// More characters than `passwords.max_length`.
    #[error("the password has more than {max} characters")]
    TooLong { max: usize },
    /// The password is on the list of compromised passwords (AUTH-04).
    #[error("the password is on a list of compromised passwords")]
    Compromised,
}

/// Hashing failed. It never depends on the password, only on the system or the stored hash.
#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("password hashing failed: {0}")]
    Argon2(#[from] password_hash::Error),
    #[error("password hashing stopped: {0}")]
    Task(#[from] tokio::task::JoinError),
}

#[derive(Debug, thiserror::Error)]
pub enum NewPasswordError {
    #[error(transparent)]
    Rejected(#[from] Rejected),
    #[error(transparent)]
    Hashing(#[from] HashError),
}

/// The result of checking a password against a stored hash.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum Verification {
    /// The password is right. `rehashed` is a new hash of it when the stored one was made with
    /// other parameters than the current settings: store it in place of the old one.
    Match { rehashed: Option<PasswordHash> },
    /// The password is wrong, or there was no hash to check it against.
    Mismatch,
}

/// The rules for new passwords, and the hashing of passwords, as configured in `[passwords]`.
///
/// It is cheap to clone, and clones share the limit on concurrent hashes.
#[derive(Clone)]
pub struct Passwords(Arc<Inner>);

struct Inner {
    min_length: usize,
    max_length: usize,
    argon2: Argon2<'static>,
    permits: Arc<Semaphore>,
}

impl fmt::Debug for Passwords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Passwords")
            .field("min_length", &self.0.min_length)
            .field("max_length", &self.0.max_length)
            .field("argon2", &self.0.argon2)
            .finish_non_exhaustive()
    }
}

/// The Argon2 parameters in `config`, or why they aren't valid.
pub(crate) fn argon2_params(config: &config::Passwords) -> Result<Params, argon2::Error> {
    Params::new(
        config.hash_memory_kib,
        config.hash_iterations,
        config.hash_parallelism,
        None,
    )
}

impl Passwords {
    /// Fails if the Argon2 parameters aren't valid, which [`config::Config::load`] already checks.
    pub fn new(config: &config::Passwords) -> Result<Self, argon2::Error> {
        let params = argon2_params(config)?;
        compromised::load();
        Ok(Self(Arc::new(Inner {
            min_length: config.min_length,
            max_length: config.max_length,
            argon2: Argon2::new(Algorithm::Argon2id, Version::V0x13, params),
            permits: Arc::new(Semaphore::new(config.max_concurrent_hashes)),
        })))
    }

    /// Checks a new password against the rules: its length (AUTH-03), then the list of compromised
    /// passwords (AUTH-04). [`hash_new`](Self::hash_new) does this too; call it on its own to
    /// report every problem with a form before doing anything else.
    pub fn check_new(&self, password: &Password) -> Result<(), Rejected> {
        let length = password.char_count();
        if length < self.0.min_length {
            return Err(Rejected::TooShort {
                min: self.0.min_length,
            });
        }
        if length > self.0.max_length {
            return Err(Rejected::TooLong {
                max: self.0.max_length,
            });
        }
        if compromised::contains(password) {
            return Err(Rejected::Compromised);
        }
        Ok(())
    }

    /// Hashes a new password, for sign-up, password change and password reset, if it meets the
    /// rules ([`check_new`](Self::check_new)).
    pub async fn hash_new(&self, password: &Password) -> Result<PasswordHash, NewPasswordError> {
        self.check_new(password)?;
        let password = password.clone();
        Ok(self.run(move |argon2| hash(argon2, &password)).await?)
    }

    /// Checks a password against the stored hash, for sign-in and re-authentication. The rules
    /// for new passwords don't apply, so passwords set under earlier settings keep working.
    ///
    /// Without a stored hash, such as when no account has the email address, it does as much work
    /// as a check and returns [`Verification::Mismatch`], so that the response takes as long and
    /// doesn't reveal whether the address is registered (SEC-14).
    pub async fn verify(
        &self,
        password: &Password,
        stored: Option<&PasswordHash>,
    ) -> Result<Verification, HashError> {
        let password = password.clone();
        let stored = stored.cloned();
        self.run(move |argon2| verify(argon2, &password, stored.as_ref()))
            .await
    }

    /// Runs `work` on the blocking thread pool once a permit is free. The permit is held until
    /// `work` is done, even if the caller stops waiting, so the limit holds for the memory in use.
    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Argon2<'static>) -> Result<T, password_hash::Error> + Send + 'static,
    ) -> Result<T, HashError> {
        let permit = self
            .0
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        let inner = self.0.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work(&inner.argon2)
        })
        .await??;
        Ok(result)
    }
}

fn hash(argon2: &Argon2<'_>, password: &Password) -> Result<PasswordHash, password_hash::Error> {
    let hash = argon2.hash_password(password.expose().as_bytes())?;
    Ok(PasswordHash(hash.to_string()))
}

fn verify(
    argon2: &Argon2<'_>,
    password: &Password,
    stored: Option<&PasswordHash>,
) -> Result<Verification, password_hash::Error> {
    let password_bytes = password.expose().as_bytes();
    let Some(stored) = stored else {
        argon2.hash_password_with_salt(password_bytes, &UNUSED_SALT)?;
        return Ok(Verification::Mismatch);
    };
    let stored = phc::PasswordHash::new(&stored.0)?;
    match argon2.verify_password(password_bytes, &stored) {
        Ok(()) => {
            let rehashed = if is_current(argon2, &stored) {
                None
            } else {
                Some(hash(argon2, password)?)
            };
            Ok(Verification::Match { rehashed })
        }
        Err(password_hash::Error::PasswordInvalid) => Ok(Verification::Mismatch),
        Err(err) => Err(err),
    }
}

/// Whether `stored` was made with the algorithm and parameters that `argon2` uses.
fn is_current(argon2: &Argon2<'_>, stored: &phc::PasswordHash) -> bool {
    let current = argon2.params();
    stored.algorithm == argon2::ARGON2ID_IDENT
        && stored.version == Some(Version::V0x13.into())
        && Params::try_from(stored).is_ok_and(|params| {
            params.m_cost() == current.m_cost()
                && params.t_cost() == current.t_cost()
                && params.p_cost() == current.p_cost()
                && params.output_len() == Some(Params::DEFAULT_OUTPUT_LEN)
        })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Settings that hash in well under a millisecond, for the tests that aren't about cost.
    fn cheap() -> config::Passwords {
        config::Passwords {
            hash_memory_kib: 64,
            hash_iterations: 1,
            ..config::Passwords::default()
        }
    }

    fn passwords(config: &config::Passwords) -> Passwords {
        Passwords::new(config).unwrap()
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let passwords = passwords(&cheap());

        assert_eq!(
            passwords.check_new(&Password::new("tr0ub4dor&3")),
            Err(Rejected::TooShort { min: 12 })
        );
        assert_eq!(passwords.check_new(&Password::new("tr0ub4dor&3x")), Ok(()));
        // Twelve characters, 48 bytes.
        let emoji = Password::new(&"🐢".repeat(12));
        assert_eq!(emoji.char_count(), 12);
        assert_eq!(passwords.check_new(&emoji), Ok(()));
        assert_eq!(
            passwords.check_new(&Password::new(&"🐢".repeat(11))),
            Err(Rejected::TooShort { min: 12 })
        );
    }

    #[tokio::test]
    async fn long_passwords_with_any_characters_are_accepted() {
        let passwords = passwords(&cheap());
        let typed: String = "Über 9000 ¿spaces? «quotes» \t ✓ 日本語 "
            .chars()
            .cycle()
            .take(128)
            .collect();
        let password = Password::new(&typed);
        assert_eq!(password.char_count(), 128);

        let hash = passwords.hash_new(&password).await.unwrap();

        assert_eq!(
            passwords.verify(&password, Some(&hash)).await.unwrap(),
            Verification::Match { rehashed: None }
        );
        assert_eq!(
            passwords.check_new(&Password::new(&"a".repeat(1025))),
            Err(Rejected::TooLong { max: 1024 })
        );
        assert_eq!(
            passwords.check_new(&Password::new(&"a".repeat(1024))),
            Ok(())
        );
    }

    #[tokio::test]
    async fn spaces_and_case_count() {
        let passwords = passwords(&cheap());
        let hash = passwords
            .hash_new(&Password::new("  two  spaces, twice  "))
            .await
            .unwrap();

        for other in [
            "two  spaces, twice",
            "  two spaces, twice  ",
            "  Two  spaces, twice  ",
        ] {
            assert_eq!(
                passwords
                    .verify(&Password::new(other), Some(&hash))
                    .await
                    .unwrap(),
                Verification::Mismatch,
                "{other:?}"
            );
        }
    }

    #[tokio::test]
    async fn equivalent_encodings_are_the_same_password() {
        let passwords = passwords(&cheap());
        // `é` as one code point, and as `e` followed by a combining accent.
        let composed = Password::new("caf\u{e9} au lait, s'il vous pla\u{ee}t");
        let decomposed = Password::new("cafe\u{301} au lait, s'il vous plai\u{302}t");
        // A full-width `Ａ` is an `A`.
        let full_width = Password::new("\u{ff21}ccess granted, finally");

        let hash = passwords.hash_new(&composed).await.unwrap();

        assert_eq!(
            passwords.verify(&decomposed, Some(&hash)).await.unwrap(),
            Verification::Match { rehashed: None }
        );
        assert_eq!(full_width.expose(), "Access granted, finally");
    }

    #[tokio::test]
    async fn compromised_passwords_are_rejected() {
        let passwords = passwords(&cheap());
        let listed = include_str!("../../data/compromised-passwords.txt")
            .lines()
            .next()
            .unwrap();

        for typed in [listed.to_owned(), listed.to_uppercase()] {
            let password = Password::new(&typed);
            assert_eq!(
                passwords.check_new(&password),
                Err(Rejected::Compromised),
                "{typed:?}"
            );
            assert!(matches!(
                passwords.hash_new(&password).await,
                Err(NewPasswordError::Rejected(Rejected::Compromised))
            ));
        }
    }

    #[tokio::test]
    async fn hashes_are_argon2id_with_the_configured_parameters_and_a_salt() {
        let passwords = passwords(&config::Passwords::default());
        let password = Password::new("a long and unremarkable sentence");

        let started = Instant::now();
        let first = passwords.hash_new(&password).await.unwrap();
        let elapsed = started.elapsed();
        let second = passwords.hash_new(&password).await.unwrap();

        // Run with `--nocapture` to see how long a hash takes with the default settings.
        println!("one hash with the default settings took {elapsed:?}");
        assert!(
            first.0.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{}",
            first.0
        );
        assert_ne!(first, second);
        for hash in [&first, &second] {
            assert_eq!(
                passwords.verify(&password, Some(hash)).await.unwrap(),
                Verification::Match { rehashed: None }
            );
        }
    }

    #[tokio::test]
    async fn verification() {
        let passwords = passwords(&cheap());
        let password = Password::new("a long and unremarkable sentence");
        let hash = passwords.hash_new(&password).await.unwrap();

        assert_eq!(
            passwords.verify(&password, Some(&hash)).await.unwrap(),
            Verification::Match { rehashed: None }
        );
        assert_eq!(
            passwords
                .verify(
                    &Password::new("a long and unremarkable sentence."),
                    Some(&hash)
                )
                .await
                .unwrap(),
            Verification::Mismatch
        );
        assert_eq!(
            passwords.verify(&password, None).await.unwrap(),
            Verification::Mismatch
        );
        assert!(
            passwords
                .verify(&password, Some(&PasswordHash("not a hash".to_owned())))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn hashes_with_other_parameters_are_renewed() {
        let password = Password::new("a long and unremarkable sentence");
        let old = passwords(&cheap()).hash_new(&password).await.unwrap();
        let raised = config::Passwords {
            hash_iterations: 2,
            ..cheap()
        };
        let passwords = passwords(&raised);

        let Verification::Match {
            rehashed: Some(new),
        } = passwords.verify(&password, Some(&old)).await.unwrap()
        else {
            panic!("the old hash was not renewed");
        };

        assert!(
            new.0.starts_with("$argon2id$v=19$m=64,t=2,p=1$"),
            "{}",
            new.0
        );
        assert_eq!(
            passwords.verify(&password, Some(&new)).await.unwrap(),
            Verification::Match { rehashed: None }
        );
        // A wrong password gets no new hash.
        assert_eq!(
            passwords
                .verify(&Password::new("something else entirely"), Some(&old))
                .await
                .unwrap(),
            Verification::Mismatch
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hashes_beyond_the_limit_wait_their_turn() {
        let passwords = passwords(&config::Passwords {
            max_concurrent_hashes: 1,
            ..cheap()
        });

        let hashes = futures_util::future::join_all((0..8).map(|i| {
            let passwords = passwords.clone();
            async move {
                passwords
                    .hash_new(&Password::new(&format!("concurrent password {i}")))
                    .await
            }
        }))
        .await;

        assert!(hashes.iter().all(Result::is_ok));
        assert_eq!(passwords.0.permits.available_permits(), 1);
    }

    #[tokio::test]
    async fn secrets_are_not_formatted() {
        let passwords = passwords(&cheap());
        let password = Password::new("hunter2 hunter2 hunter2");
        let hash = passwords.hash_new(&password).await.unwrap();
        let salt_and_hash = hash.0.rsplit('$').next().unwrap().to_owned();

        let formatted = format!("{password:?} {hash:?} {passwords:?}");

        assert!(!formatted.contains("hunter2"), "{formatted}");
        assert!(!formatted.contains(&salt_and_hash), "{formatted}");
    }

    #[test]
    fn passwords_deserialize_normalized() {
        let password: Password = serde_json::from_str("\"cafe\\u0301 au lait\"").unwrap();

        assert_eq!(password.expose(), "caf\u{e9} au lait");
    }
}
