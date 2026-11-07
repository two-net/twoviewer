//! The list of compromised passwords that new passwords are checked against (AUTH-04).
//!
//! `server/data/compromised-passwords.txt` holds common passwords from public breach collections,
//! long enough to be allowed otherwise: shorter ones are rejected anyway. It is embedded in the
//! server, so the check needs no external service, and passwords never leave the server.
//! `cargo xtask compromised-passwords` generates it, and `server/README.md` says from what.
//!
//! The list holds passwords in their [`lookup_form`], one per line, sorted.

use std::{collections::HashSet, sync::LazyLock};

use zeroize::Zeroizing;

use super::password::Password;

static LIST: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("../../data/compromised-passwords.txt")
        .lines()
        .collect()
});

/// The form in which passwords are listed and looked up: normalized, like every [`Password`], and
/// lowercased, so that `Password1234` is as compromised as `password1234`.
pub fn lookup_form(password: &Password) -> Zeroizing<String> {
    Zeroizing::new(password.expose().to_lowercase())
}

/// Whether `password` is on the list.
pub(super) fn contains(password: &Password) -> bool {
    LIST.contains(lookup_form(password).as_str())
}

/// Reads the list now rather than at the first lookup.
pub(super) fn load() {
    LazyLock::force(&LIST);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    const FILE: &str = include_str!("../../data/compromised-passwords.txt");

    #[test]
    fn every_entry_is_long_enough_and_in_lookup_form() {
        let min_length = config::Passwords::default().min_length;
        let entries: Vec<&str> = FILE.lines().collect();

        assert!(entries.len() > 1000, "only {} entries", entries.len());
        assert!(FILE.ends_with('\n'));
        for entry in &entries {
            let password = Password::new(entry);
            assert!(
                password.char_count() >= min_length,
                "{entry:?} is too short"
            );
            assert_eq!(lookup_form(&password).as_str(), *entry);
            assert!(!entry.chars().any(char::is_control), "{entry:?}");
        }
        assert!(
            entries.windows(2).all(|pair| pair[0] < pair[1]),
            "the entries are not sorted, or not unique"
        );
    }

    #[test]
    fn listed_passwords_are_found_in_any_case() {
        let listed = FILE.lines().next().unwrap();

        assert!(contains(&Password::new(listed)));
        assert!(contains(&Password::new(&listed.to_uppercase())));
        assert!(!contains(&Password::new(
            "correct horse battery staple 7%Qz"
        )));
    }
}
