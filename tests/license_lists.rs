//! Checks that cargo-deny and cargo-about accept the same licenses.
//!
//! `deny.toml` and `about.toml` each keep their own list, so updating only one
//! of them would let CI and the release disagree about which dependencies are
//! allowed.

use anyhow::{Context, Result};
use std::{collections::BTreeSet, fs, path::Path};

/// Reads the string array at `keys` from a TOML file in the repository root.
fn string_array(file: &str, keys: &[&str]) -> Result<BTreeSet<String>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let source =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let root = toml::Value::Table(
        toml::from_str(&source).with_context(|| format!("cannot parse {}", path.display()))?,
    );
    let mut value = &root;
    for key in keys {
        value = value
            .get(key)
            .with_context(|| format!("{} has no {}", path.display(), keys.join(".")))?;
    }
    value
        .as_array()
        .with_context(|| format!("{}.{} is not an array", path.display(), keys.join(".")))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .with_context(|| format!("{} lists a non-string license", path.display()))
        })
        .collect()
}

#[test]
fn cargo_deny_and_cargo_about_accept_the_same_licenses() -> Result<()> {
    let deny = string_array("deny.toml", &["licenses", "allow"])?;
    let about = string_array("about.toml", &["accepted"])?;
    assert_eq!(
        deny,
        about,
        "only in deny.toml: {:?}, only in about.toml: {:?}",
        deny.difference(&about).collect::<Vec<_>>(),
        about.difference(&deny).collect::<Vec<_>>()
    );
    Ok(())
}
