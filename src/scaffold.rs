use anyhow::{Context, Result, ensure};
use std::{fs, io::Write, path::Path};

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
        "styles/main.css",
        include_str!("../scaffold/styles/main.css"),
    ),
    (
        "content/index.md",
        include_str!("../scaffold/content/index.md"),
    ),
    ("static/.gitkeep", ""),
    (".gitignore", "/dist/\n"),
];

pub fn create(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && name.as_bytes()[0].is_ascii_alphanumeric(),
        "invalid site name: use ASCII letters, numbers, hyphens or underscores; start with a letter or number"
    );
    let root = Path::new(name);
    // Atomic no-overwrite check, including existing empty directories.
    fs::create_dir(root)
        .with_context(|| format!("cannot create {name}; choose a new directory name"))?;
    write_site(root, name).with_context(|| {
        format!("scaffolding failed in {name}; partial files were retained for inspection")
    })
}

fn write_site(root: &Path, name: &str) -> Result<()> {
    for directory in ["templates", "styles", "content", "static"] {
        fs::create_dir(root.join(directory))
            .with_context(|| format!("cannot create {directory}"))?;
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
