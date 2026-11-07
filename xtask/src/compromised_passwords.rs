//! `cargo xtask compromised-passwords <word list>...` writes the list of compromised passwords that
//! the server checks new passwords against (AUTH-04), `server/data/compromised-passwords.txt`.
//!
//! It keeps the passwords in the word lists that are long enough to be allowed otherwise, in the
//! form the server looks them up in, sorted and without duplicates. Both come from the server's
//! own code, so the list always matches how the server checks it. `server/README.md` says which
//! word lists the file is made from.

use std::{collections::BTreeSet, fs, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result, bail};
use twoviewer_server::{
    accounts::{compromised::lookup_form, password::Password},
    config,
};

pub fn write(sources: &[&str]) -> Result<ExitCode> {
    if sources.is_empty() {
        bail!("name at least one word list");
    }
    let min_length = config::Passwords::default().min_length;
    let mut entries = BTreeSet::new();
    let mut read = 0;
    for source in sources {
        let text = fs::read(source).with_context(|| format!("read {source}"))?;
        // Word lists are mostly, but not always, UTF-8. A line that isn't can't be typed into a
        // web form as it is, so it is skipped.
        for line in text.split(|&byte| byte == b'\n') {
            let Ok(line) = std::str::from_utf8(line) else {
                continue;
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            read += 1;
            // Control characters can't be typed, and would break the one-per-line format.
            if line.chars().any(char::is_control) {
                continue;
            }
            let password = Password::new(line);
            if password.char_count() < min_length {
                continue;
            }
            let form = lookup_form(&password);
            // Lowercasing can, rarely, give text that normalizes differently. Such an entry could
            // never match, so it is left out rather than listed in a form the server won't use.
            if *lookup_form(&Password::new(&form)) == *form {
                entries.insert(form.to_string());
            }
        }
    }

    let path = list_path();
    let mut list = String::new();
    for entry in &entries {
        list.push_str(entry);
        list.push('\n');
    }
    fs::write(&path, list).with_context(|| format!("write {}", path.display()))?;
    println!(
        "Wrote {} of {read} passwords to {}.",
        entries.len(),
        path.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn list_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("server/data/compromised-passwords.txt")
}
