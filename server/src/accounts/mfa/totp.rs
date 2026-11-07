//! Time-based one-time codes (RFC 6238), as authenticator apps make them (AUTH-06).
//!
//! The settings are the ones that every authenticator app supports: HMAC-SHA1, 6 digits and
//! 30-second steps. Apps that are told other settings often ignore them, and then show codes that
//! never work. A code is accepted up to `mfa.clock_skew_steps` steps early or late, for clocks
//! that are off, and only once (RFC 6238 §5.2): the step of the last accepted code is stored, and
//! codes of that step or earlier are refused, so a code that someone saw cannot be used again.

use std::fmt::{self, Write as _};

use chrono::{DateTime, Utc};
use ring::hmac;
use subtle::{ConditionallySelectable, ConstantTimeEq};
use zeroize::Zeroizing;

/// The length of a secret: 160 bits, the length of an HMAC-SHA1 output, as RFC 4226 recommends.
pub const SECRET_BYTES: usize = 20;
/// The number of digits in a code.
pub const DIGITS: usize = 6;
/// The length of a step in seconds.
pub const STEP_SECONDS: i64 = 30;

/// The secret that a user's authenticator app and the server share.
///
/// It never appears in logs or audit events (MNT-03, AUD-06): `Debug` hides it, there is no
/// `Display`, and its memory is cleared when it is dropped. The database holds it only encrypted
/// ([`super::cipher`]).
#[derive(Clone)]
pub struct Secret(Zeroizing<[u8; SECRET_BYTES]>);

impl Secret {
    /// A new secret from a cryptographically secure generator, seeded by the operating system.
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; SECRET_BYTES]);
        rand::fill(&mut bytes[..]);
        Self(bytes)
    }

    /// The secret with these bytes, if there are [`SECRET_BYTES`] of them.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: [u8; SECRET_BYTES] = bytes.try_into().ok()?;
        Some(Self(Zeroizing::new(bytes)))
    }

    /// The secret itself, to encrypt. Never log it.
    pub fn expose(&self) -> &[u8; SECRET_BYTES] {
        &self.0
    }

    /// The secret in base32, as people type it into an authenticator app. Never log it.
    pub fn base32(&self) -> Zeroizing<String> {
        Zeroizing::new(data_encoding::BASE32_NOPAD.encode(&self.0[..]))
    }

    /// The `otpauth://` URI that authenticator apps read from a QR code ("Key Uri Format"), which
    /// names the account `account` of `issuer`. It contains the secret: never log it.
    pub fn uri(&self, issuer: &str, account: &str) -> Zeroizing<String> {
        let mut uri = Zeroizing::new(String::from("otpauth://totp/"));
        percent_encode(&mut uri, issuer);
        uri.push(':');
        percent_encode(&mut uri, account);
        uri.push_str("?secret=");
        uri.push_str(&self.base32());
        uri.push_str("&issuer=");
        percent_encode(&mut uri, issuer);
        write!(uri, "&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}")
            .expect("writing to a String cannot fail");
        uri
    }

    /// The code of `step`, as a number below 10^[`DIGITS`] (RFC 4226 §5.3).
    pub fn code_at(&self, step: i64) -> u32 {
        let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &self.0[..]);
        let tag = hmac::sign(&key, &step.to_be_bytes());
        let hash = tag.as_ref();
        let offset = usize::from(hash[hash.len() - 1] & 0x0f);
        let truncated = u32::from_be_bytes(
            hash[offset..offset + 4]
                .try_into()
                .expect("an HMAC-SHA1 has 20 bytes"),
        ) & 0x7fff_ffff;
        truncated % 10u32.pow(DIGITS as u32)
    }

    /// The step of the code `typed` if it is the code of a step within `skew` steps of `now`, and
    /// later than `last_used_step`, the step of the last code accepted. `None` otherwise.
    ///
    /// Every step in the window is compared, in constant time, so that the time taken doesn't
    /// tell which step a code was close to.
    pub fn verify(
        &self,
        typed: &str,
        now: DateTime<Utc>,
        skew: u32,
        last_used_step: Option<i64>,
    ) -> Option<i64> {
        let typed = normalize(typed)?;
        let current = step(now);
        let skew = i64::from(skew);
        let mut matched = subtle::Choice::from(0);
        let mut matched_step = 0i64;
        for step in current - skew..=current + skew {
            let expected = Zeroizing::new(format!("{:0DIGITS$}", self.code_at(step)));
            let is_match = expected.as_bytes().ct_eq(typed.as_bytes()) & !matched;
            matched_step = i64::conditional_select(&matched_step, &step, is_match);
            matched |= is_match;
        }
        let fresh = last_used_step.is_none_or(|last| matched_step > last);
        (bool::from(matched) && fresh).then_some(matched_step)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// The step that `time` falls in.
pub fn step(time: DateTime<Utc>) -> i64 {
    time.timestamp().div_euclid(STEP_SECONDS)
}

/// Whether `typed` looks like an authenticator code rather than a recovery code: [`DIGITS`]
/// digits, with any spaces that apps show in the middle.
pub fn is_code(typed: &str) -> bool {
    normalize(typed).is_some()
}

/// The digits of `typed` without spaces, if they are exactly [`DIGITS`] digits.
fn normalize(typed: &str) -> Option<Zeroizing<String>> {
    let digits = Zeroizing::new(
        typed
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>(),
    );
    (digits.len() == DIGITS && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// Appends `text` to `out` with everything but unreserved characters percent-encoded (RFC 3986).
fn percent_encode(out: &mut String, text: &str) {
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            write!(out, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SHA-1 secret of the test vectors in RFC 6238, Appendix B.
    fn rfc_secret() -> Secret {
        Secret::from_bytes(b"12345678901234567890").unwrap()
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    #[test]
    fn codes_match_the_test_vectors_of_rfc_6238() {
        let secret = rfc_secret();

        // The RFC's 8-digit codes, of which apps show the last 6.
        for (time, code) in [
            (59, 94287082),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ] {
            assert_eq!(secret.code_at(step(at(time))), code % 1_000_000, "{time}");
        }
    }

    #[test]
    fn codes_are_accepted_within_the_skew_and_only_after_the_last_used_step() {
        let secret = rfc_secret();
        let now = at(1111111111);
        let current = step(now);
        let code = |step| format!("{:06}", secret.code_at(step));

        assert_eq!(secret.verify(&code(current), now, 1, None), Some(current));
        assert_eq!(
            secret.verify(&code(current - 1), now, 1, None),
            Some(current - 1)
        );
        assert_eq!(
            secret.verify(&code(current + 1), now, 1, None),
            Some(current + 1)
        );
        assert_eq!(secret.verify(&code(current - 2), now, 1, None), None);
        assert_eq!(secret.verify(&code(current + 2), now, 1, None), None);
        assert_eq!(secret.verify(&code(current - 1), now, 0, None), None);

        // A code is used once, and so are the codes before it.
        assert_eq!(secret.verify(&code(current), now, 1, Some(current)), None);
        assert_eq!(
            secret.verify(&code(current - 1), now, 1, Some(current)),
            None
        );
        assert_eq!(
            secret.verify(&code(current + 1), now, 1, Some(current)),
            Some(current + 1)
        );
    }

    #[test]
    fn codes_may_be_typed_with_spaces() {
        let secret = rfc_secret();
        let now = at(59);
        let code = format!("{:06}", secret.code_at(step(now)));
        let spaced = format!(" {} {} ", &code[..3], &code[3..]);

        assert!(is_code(&spaced));
        assert_eq!(secret.verify(&spaced, now, 0, None), Some(step(now)));
        for not_a_code in [
            "",
            "12345",
            "1234567",
            "12345a",
            "abcd-efgh-jkmn-pqrs",
            "１２３４５６",
        ] {
            assert!(!is_code(not_a_code), "{not_a_code}");
            assert_eq!(secret.verify(not_a_code, now, 1, None), None);
        }
    }

    #[test]
    fn the_uri_names_the_account_and_carries_the_secret_in_base32() {
        let secret = rfc_secret();

        assert_eq!(&*secret.base32(), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(
            &*secret.uri("TwoViewer", "ann+mfa@example.com"),
            "otpauth://totp/TwoViewer:ann%2Bmfa%40example.com?secret=\
             GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=TwoViewer&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn secrets_are_random_and_not_formatted() {
        let (a, b) = (Secret::generate(), Secret::generate());

        assert_ne!(a.expose(), b.expose());
        assert!(!format!("{a:?}").contains(&*a.base32()));
        assert_eq!(Secret::from_bytes(&[0; 19]).map(|_| ()), None);
    }
}
