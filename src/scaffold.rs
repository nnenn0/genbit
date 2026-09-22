use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

const FILES: &[(&str, &str)] = &[
    (
        "templates/base.html",
        include_str!("../scaffold/templates/base.html"),
    ),
    (
        "templates/page.html",
        include_str!("../scaffold/templates/page.html"),
    ),
    (
        "templates/root.html",
        include_str!("../scaffold/templates/root.html"),
    ),
    (
        "styles/main.css",
        include_str!("../scaffold/styles/main.css"),
    ),
    (
        "content/hello-world.md",
        include_str!("../scaffold/content/hello-world.md"),
    ),
    ("static/.gitkeep", ""),
    (".gitignore", "/dist/\n"),
];

pub(crate) fn create(name: &str) -> Result<()> {
    validate_name(name)?;
    let root = Path::new(name);
    // create_dir refuses existing paths without a separate existence check.
    fs::create_dir(root).map_err(|error| {
        let context = if error.kind() == ErrorKind::AlreadyExists {
            format!("cannot create {name}: destination already exists; choose a new directory name")
        } else {
            format!("cannot create site directory {}", root.display())
        };
        anyhow::Error::new(error).context(context)
    })?;
    write_site(root, name).with_context(|| {
        format!("scaffolding failed in {name}; partial files were retained for inspection")
    })
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        name.bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid site name: use ASCII letters, numbers, hyphens or underscores; start with a letter or number"
    );
    Ok(())
}

fn write_site(root: &Path, name: &str) -> Result<()> {
    for directory in ["templates", "styles", "content", "static"] {
        let path = root.join(directory);
        fs::create_dir(&path).with_context(|| format!("cannot create {}", path.display()))?;
    }
    // The validated name contains only TOML-safe ASCII.
    let config = format!("title = \"{name}\"\n");
    write_new(&root.join("config.toml"), &config)?;
    for &(relative, content) in FILES {
        write_new(&root.join(relative), content)?;
    }
    Ok(())
}

fn write_new(path: &Path, content: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn accepts_supported_names() {
        for name in ["a", "0", "my-blog", "my_blog", "Blog2026"] {
            assert!(validate_name(name).is_ok(), "rejected {name:?}");
        }
    }

    #[test]
    fn rejects_invalid_names() {
        for name in [
            "",
            ".",
            "..",
            "../blog",
            "/blog",
            "a/b",
            "a\\b",
            "-blog",
            "_blog",
            "my blog",
            "ブログ",
            "a\"b",
            "a\nb",
        ] {
            assert!(validate_name(name).is_err(), "accepted {name:?}");
        }
    }
}
