//! Email addresses as stored: unique regardless of case (DB-13).

use sea_orm::DeriveValueType;

/// An email address in the form it is stored and compared: trimmed and lowercased, so that
/// `Ann@Example.com` and `ann@example.com` are the same account.
///
/// Declare the column as `#[sea_orm(unique, extra = "CHECK (email = lower(email))")]`, so the
/// database also rejects addresses that bypass this type. The rare characters that Rust and
/// PostgreSQL lowercase differently then fail the check instead of slipping past the unique
/// constraint. Whether the address is well-formed is for the caller to check.
#[derive(Clone, Debug, PartialEq, Eq, Hash, DeriveValueType)]
pub struct EmailAddress(String);

impl EmailAddress {
    pub fn new(address: &str) -> Self {
        Self(address.trim().to_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_and_surrounding_whitespace_are_ignored() {
        let address = EmailAddress::new(" Ann.Lee@Example.COM\n");

        assert_eq!(address.as_str(), "ann.lee@example.com");
        assert_eq!(address, EmailAddress::new("ann.lee@example.com"));
    }
}
