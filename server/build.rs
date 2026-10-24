//! Embeds `migrations/*.sql` into the server binary, in identifier order.

use std::{env, fs, path::Path};

fn main() {
    println!("cargo::rerun-if-changed=migrations");

    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("migrations");
    let mut ids = Vec::new();
    for entry in fs::read_dir(&dir).expect("read server/migrations") {
        let path = entry.expect("read server/migrations").path();
        if path.extension().is_none_or(|ext| ext != "sql") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        assert!(
            is_valid_id(id),
            "invalid migration file name {}: expected <yyyymmddHHMMSS>_<snake_case_name>.sql",
            path.display()
        );
        ids.push(id.to_owned());
    }
    ids.sort();

    let mut out = String::from("&[\n");
    for id in &ids {
        let path = dir.join(format!("{id}.sql"));
        out += &format!("    Migration {{ id: {id:?}, sql: include_str!({path:?}) }},\n");
    }
    out += "]\n";
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("migrations.rs"),
        out,
    )
    .expect("write migrations.rs");
}

/// `<14-digit UTC timestamp>_<snake_case_name>`, so identifiers sort in creation order.
fn is_valid_id(id: &str) -> bool {
    let Some((timestamp, name)) = id.split_once('_') else {
        return false;
    };
    timestamp.len() == 14
        && timestamp.bytes().all(|b| b.is_ascii_digit())
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
