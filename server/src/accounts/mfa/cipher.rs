//! Encryption of the secrets of authenticator apps at rest (SEC-07).
//!
//! Each secret is encrypted with AES-256-GCM under the key `MFA_ENCRYPTION_KEY`
//! ([`crate::config::MfaEncryptionKey`]), with a random nonce. The user's ID is authenticated
//! along with it, so a secret copied to another user's row doesn't decrypt. A copy of the
//! database alone therefore reveals no secret, and lets nobody make codes.
//!
//! Stored: a version byte, the 12-byte nonce, then the ciphertext and its 16-byte tag. The version
//! names the key and algorithm, so that a later key rotation can tell old secrets from new ones.

use std::{fmt, sync::Arc};

use ring::aead::{self, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use sea_orm::prelude::Uuid;
use zeroize::Zeroizing;

use super::totp::{SECRET_BYTES, Secret};
use crate::config::MfaEncryptionKey;

/// The format of stored secrets: AES-256-GCM under `MFA_ENCRYPTION_KEY`.
const VERSION: u8 = 1;

/// The length of a stored secret.
pub const STORED_LENGTH: usize = 1 + NONCE_LEN + SECRET_BYTES + aead::MAX_TAG_LEN;

/// Encrypts and decrypts the secrets of authenticator apps.
#[derive(Clone)]
pub struct Cipher(Arc<LessSafeKey>);

#[derive(Debug, thiserror::Error)]
#[error(
    "cannot decrypt the secret of an authenticator app: it was stored for another user, has been \
     changed, or MFA_ENCRYPTION_KEY isn't the key it was encrypted with"
)]
pub struct DecryptError;

impl Cipher {
    pub fn new(key: &MfaEncryptionKey) -> Self {
        let key = UnboundKey::new(&aead::AES_256_GCM, key.expose())
            .expect("an MfaEncryptionKey has the length of an AES-256 key");
        Self(Arc::new(LessSafeKey::new(key)))
    }

    /// `secret`, encrypted for storage in the row of `user_id`.
    pub fn encrypt(&self, user_id: Uuid, secret: &Secret) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        // 96 random bits: a repeat is negligible for the number of secrets one key encrypts.
        rand::fill(&mut nonce[..]);
        let mut sealed = Vec::with_capacity(STORED_LENGTH);
        sealed.push(VERSION);
        sealed.extend_from_slice(&nonce);
        let mut in_out = secret.expose().to_vec();
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(associated_data(user_id)),
                &mut in_out,
            )
            .expect("a secret is far shorter than AES-GCM's limit");
        sealed.extend_from_slice(&in_out);
        sealed
    }

    /// The secret that [`encrypt`](Self::encrypt) stored for `user_id` as `stored`.
    pub fn decrypt(&self, user_id: Uuid, stored: &[u8]) -> Result<Secret, DecryptError> {
        let (&version, rest) = stored.split_first().ok_or(DecryptError)?;
        if version != VERSION || rest.len() < NONCE_LEN {
            return Err(DecryptError);
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| DecryptError)?;
        let mut in_out = Zeroizing::new(sealed.to_vec());
        let plain = self
            .0
            .open_in_place(nonce, Aad::from(associated_data(user_id)), &mut in_out)
            .map_err(|_| DecryptError)?;
        Secret::from_bytes(plain).ok_or(DecryptError)
    }
}

impl fmt::Debug for Cipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cipher(..)")
    }
}

/// What a stored secret is bound to: its format and its user.
fn associated_data(user_id: Uuid) -> [u8; 17] {
    let mut data = [0u8; 17];
    data[0] = VERSION;
    data[1..].copy_from_slice(user_id.as_bytes());
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> Cipher {
        Cipher::new(&MfaEncryptionKey::new([byte; MfaEncryptionKey::LENGTH]))
    }

    #[test]
    fn secrets_decrypt_for_their_user_with_their_key() {
        let cipher = cipher(7);
        let (ann, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let secret = Secret::generate();

        let stored = cipher.encrypt(ann, &secret);

        assert_eq!(stored.len(), STORED_LENGTH);
        assert_eq!(stored[0], VERSION);
        assert_eq!(
            cipher.decrypt(ann, &stored).unwrap().expose(),
            secret.expose()
        );
        assert!(
            !stored
                .windows(SECRET_BYTES)
                .any(|window| window == secret.expose())
        );

        assert!(cipher.decrypt(bob, &stored).is_err());
        assert!(self::cipher(8).decrypt(ann, &stored).is_err());
    }

    #[test]
    fn changed_or_truncated_secrets_do_not_decrypt() {
        let cipher = cipher(7);
        let user = Uuid::new_v4();
        let stored = cipher.encrypt(user, &Secret::generate());

        for index in 0..stored.len() {
            let mut changed = stored.clone();
            changed[index] ^= 1;
            assert!(cipher.decrypt(user, &changed).is_err(), "{index}");
        }
        for length in 0..stored.len() {
            assert!(cipher.decrypt(user, &stored[..length]).is_err(), "{length}");
        }
    }

    #[test]
    fn each_encryption_is_different() {
        let cipher = cipher(7);
        let user = Uuid::new_v4();
        let secret = Secret::generate();

        assert_ne!(cipher.encrypt(user, &secret), cipher.encrypt(user, &secret));
    }
}
