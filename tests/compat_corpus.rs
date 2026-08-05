//! Compat corpus enforcement.
//!
//! Walks `compat/s3-ferry/config/*.env` and asserts that every
//! env-var name in the corpus is on an EXPLICITLY-CURATED list of
//! S3-Ferry fields that have been reviewed against the FileFerry
//! target contract. The curated list is a build-time constant
//! here — no docs are read at test time, so the result is stable
//! across environments.
//!
//! **When adding a new field to a compat file**: run an
//! S3-Ferry-vs-FileFerry review that produces a target action
//! for it (preserve / rename / drop / extend), then add the field
//! name to `COVERED_FIELDS` below. The test fails until the
//! review happens — this is the intended coverage gate.
//!
//! **When removing a field from a compat file**: leave it in
//! `COVERED_FIELDS`. A removed-from-upstream field is still a
//! valid historical row in the coverage record; the test only
//! enforces "every corpus field is covered", not "every covered
//! field is still in the corpus".
//!
//! Kept dependency-free on purpose — this test runs on
//! `cargo test` in a fresh checkout, no fixtures beyond the repo
//! itself.

use std::fs;
use std::path::{Path, PathBuf};

/// Every S3-Ferry env-var that has been reviewed and mapped to a
/// FileFerry outcome. Sorted alphabetically for diff-friendliness.
///
/// **Do not edit this list to make a failing test pass.** The list
/// is the human-reviewed output of an S3-Ferry-vs-FileFerry
/// contract-comparison pass. If a new corpus field appears here
/// without that pass having happened, the §7.3 guarantee is void.
const COVERED_FIELDS: &[&str] = &[
    "API_CORS_ORIGIN",
    "API_DOCUMENTATION_ENABLED",
    "FS_DATA_DIRECTORY_PATH",
    "S3_ACCESS_KEY_ID",
    "S3_DATA_BUCKET_NAME",
    "S3_DATA_BUCKET_PATH",
    "S3_ENDPOINT_URL",
    "S3_REGION",
    "S3_SECRET_ACCESS_KEY",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Extract every env-var name from a `.env` file.
/// Accepts both `KEY=value` (development.env / test.env) and bare
/// `KEY` template form (production.env). Blank lines and `#` comments
/// are skipped.
fn extract_env_keys(path: &Path) -> Vec<String> {
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let mut keys = Vec::new();
    for (lineno, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = line.split('=').next().unwrap().trim();
        assert!(
            key.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
            "{}:{}: unexpected env-var name shape: {key:?}",
            path.display(),
            lineno + 1,
        );
        keys.push(key.to_string());
    }
    keys
}

#[test]
fn s3_ferry_env_fields_are_all_covered() {
    let root = repo_root();
    let corpus_dir = root.join("compat").join("s3-ferry").join("config");

    let mut missing: Vec<(PathBuf, String)> = Vec::new();
    let mut checked = 0usize;

    for entry in fs::read_dir(&corpus_dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", corpus_dir.display()))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("env") {
            continue;
        }
        for key in extract_env_keys(&path) {
            checked += 1;
            if !COVERED_FIELDS.iter().any(|c| *c == key) {
                missing.push((path.clone(), key));
            }
        }
    }

    assert!(
        checked > 0,
        "compat/s3-ferry/config/ contained no .env files — the corpus \
         cannot be empty or the guarantee is vacuous"
    );

    if !missing.is_empty() {
        let mut msg = String::from("\ncompat corpus fields with NO entry in COVERED_FIELDS:\n");
        for (p, k) in &missing {
            msg.push_str(&format!("  {}: {}\n", p.display(), k));
        }
        msg.push_str(
            "\nEvery field in compat/ MUST be human-reviewed against the \
             FileFerry target contract and added to COVERED_FIELDS in \
             this file. Do NOT add a field to the list without doing the \
             review.\n",
        );
        panic!("{msg}");
    }
}

#[test]
fn extract_env_keys_matches_expected_s3_ferry_set() {
    // Belt-and-suspenders: pin the exact set we expect S3-Ferry's
    // production.env to expose. If S3-Ferry adds a field upstream,
    // updating `compat/s3-ferry/config/production.env` fails this
    // test until the expected list here (and COVERED_FIELDS above)
    // is updated — forcing a conscious decision about the new field.
    let path = repo_root()
        .join("compat")
        .join("s3-ferry")
        .join("config")
        .join("production.env");
    let mut keys = extract_env_keys(&path);
    keys.sort();
    keys.dedup();

    let mut expected: Vec<&'static str> = COVERED_FIELDS.to_vec();
    expected.sort();

    assert_eq!(
        keys, expected,
        "s3-ferry production.env field set has drifted from COVERED_FIELDS; \
         review the new/removed field against the FileFerry target contract \
         before updating either list."
    );
}
