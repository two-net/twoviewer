//! Random secrets that the database stores only as hashes: the tokens of email links and of
//! sign-in sessions.
//!
//! A token is 32 random bytes, as 64 hexadecimal digits. The database stores only its SHA-256
//! (SEC-07), so a copy of the database can be used neither to open links nor to sign in. A slow
//! hash isn't needed: the token is 256 random bits, which cannot be guessed, unlike a password.

use std::fmt::{self, Write as _};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The number of random bytes in a token.
const TOKEN_BYTES: usize = 32;

/// The length of every token issued. Longer ones are not worth looking up.
pub const TOKEN_LENGTH: usize = 2 * TOKEN_BYTES;

/// A token: 32 random bytes, as 64 hexadecimal digits.
///
/// It never appears in logs or audit events (MNT-03, AUD-06): `Debug` hides it, there is no
/// `Display`, and its memory is cleared when it is dropped.
pub struct Token(Zeroizing<String>);

impl Token {
    /// A new token from a cryptographically secure generator, seeded by the operating system.
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
        rand::fill(&mut bytes[..]);
        let mut token = Zeroizing::new(String::with_capacity(TOKEN_LENGTH));
        for byte in bytes.iter() {
            write!(token, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(token)
    }

    /// The token itself, to put in a link or a cookie. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The hash that the database stores for this token.
    pub fn hash(&self) -> Vec<u8> {
        hash(&self.0)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// The hash under which the database stores the token `token`, as received in a request.
pub fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (Token::generate(), Token::generate());

        assert_ne!(a.expose(), b.expose());
        assert_eq!(a.expose().len(), TOKEN_LENGTH);
        assert!(
            a.expose()
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn the_hash_is_the_sha256_of_the_token_as_received() {
        let token = Token::generate();

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
        let token = Token::generate();

        assert!(!format!("{token:?}").contains(token.expose()));
    }
}
