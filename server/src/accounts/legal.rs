//! The current versions of the Terms of Service and the Privacy Notice, which sign-up records as
//! accepted (AUTH-01, PRIV-04).
//!
//! The texts are in the web client, in `web/src/app/legal/`, and so are their versions, in
//! `versions.ts`. A version is the date the text last changed. Change it here and there together
//! with the text: a test checks that both say the same. Sign-up is refused with a version other
//! than the current one, so that the version recorded is the one the user read.

/// The date the Terms of Service last changed.
pub const TERMS_VERSION: &str = "2026-11-08";

/// The date the Privacy Notice last changed.
pub const PRIVACY_VERSION: &str = "2026-11-08";

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_VERSIONS: &str = include_str!("../../../web/src/app/legal/versions.ts");

    #[test]
    fn the_versions_are_the_web_clients() {
        for declaration in [
            format!("export const TERMS_VERSION = '{TERMS_VERSION}';"),
            format!("export const PRIVACY_VERSION = '{PRIVACY_VERSION}';"),
        ] {
            assert!(WEB_VERSIONS.contains(&declaration), "{declaration}");
        }
    }

    #[test]
    fn versions_are_dates() {
        for version in [TERMS_VERSION, PRIVACY_VERSION] {
            assert!(
                chrono::NaiveDate::parse_from_str(version, "%Y-%m-%d").is_ok(),
                "{version}"
            );
        }
    }
}
