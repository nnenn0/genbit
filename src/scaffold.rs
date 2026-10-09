use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

const FILES: &[(&str, &str)] = &[
    (
        "views/pages/home.bv",
        include_str!("../scaffold/views/pages/home.bv"),
    ),
    (
        "views/pages/home.css",
        include_str!("../scaffold/views/pages/home.css"),
    ),
    (
        "views/pages/entry.bv",
        include_str!("../scaffold/views/pages/entry.bv"),
    ),
    (
        "views/pages/entry.css",
        include_str!("../scaffold/views/pages/entry.css"),
    ),
    (
        "views/pages/tags.bv",
        include_str!("../scaffold/views/pages/tags.bv"),
    ),
    (
        "views/pages/tags.css",
        include_str!("../scaffold/views/pages/tags.css"),
    ),
    (
        "views/pages/tag.bv",
        include_str!("../scaffold/views/pages/tag.bv"),
    ),
    (
        "views/pages/tag.css",
        include_str!("../scaffold/views/pages/tag.css"),
    ),
    (
        "views/pages/not-found.bv",
        include_str!("../scaffold/views/pages/not-found.bv"),
    ),
    (
        "views/components/document.bv",
        include_str!("../scaffold/views/components/document.bv"),
    ),
    (
        "views/components/document.css",
        include_str!("../scaffold/views/components/document.css"),
    ),
    (
        "views/components/layout.bv",
        include_str!("../scaffold/views/components/layout.bv"),
    ),
    (
        "views/components/home-link.bv",
        include_str!("../scaffold/views/components/home-link.bv"),
    ),
    (
        "views/components/entry-list.bv",
        include_str!("../scaffold/views/components/entry-list.bv"),
    ),
    (
        "views/components/entry-list.css",
        include_str!("../scaffold/views/components/entry-list.css"),
    ),
    (
        "views/components/draft-badge.bv",
        include_str!("../scaffold/views/components/draft-badge.bv"),
    ),
    (
        "views/components/draft-badge.css",
        include_str!("../scaffold/views/components/draft-badge.css"),
    ),
    (
        "views/components/timestamp.bv",
        include_str!("../scaffold/views/components/timestamp.bv"),
    ),
    (
        "content/entries/hello-world/index.md",
        include_str!("../scaffold/content/entries/hello-world/index.md"),
    ),
    (
        "static/assets/site/favicon.svg",
        include_str!("../scaffold/static/assets/site/favicon.svg"),
    ),
    ("drafts/entries/.gitkeep", ""),
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
    for directory in [
        "views",
        "views/pages",
        "views/components",
        "content",
        "content/entries",
        "content/entries/hello-world",
        "drafts",
        "drafts/entries",
        "static",
        "static/assets",
        "static/assets/img",
        "static/assets/site",
    ] {
        let path = root.join(directory);
        fs::create_dir(&path).with_context(|| format!("cannot create {}", path.display()))?;
    }
    // The validated name contains only TOML-safe ASCII.
    let config = format!(
        "title = \"{name}\"\ndescription = \"{name} で公開している記事の一覧です。\"\nsite_url = \"http://127.0.0.1:3000/\"\nog_image = \"/assets/site/ogp.png\"\ntimezone = \"Asia/Tokyo\"\n"
    );
    write_new(&root.join("config.toml"), &config)?;
    for &(relative, content) in FILES {
        write_new(&root.join(relative), content)?;
    }
    write_new(
        &root.join("static/assets/site/ogp.png"),
        include_bytes!("../scaffold/static/assets/site/ogp.png"),
    )?;
    write_new(
        &root.join("static/assets/site/favicon.png"),
        include_bytes!("../scaffold/static/assets/site/favicon.png"),
    )?;
    Ok(())
}

fn write_new(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(content.as_ref())
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
