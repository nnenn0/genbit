use anyhow::{Context, Result};
use std::{
    fs,
    process::{Command, Output},
};
use tempfile::TempDir;

struct Workspace(TempDir);

impl Workspace {
    fn new() -> Result<Self> {
        Ok(Self(
            tempfile::tempdir().context("cannot create test workspace")?,
        ))
    }

    fn run(&self, args: &[&str], expected_success: bool) -> Result<Output> {
        let output = Command::new(env!("CARGO_BIN_EXE_genbit"))
            .args(args)
            .current_dir(self.0.path())
            .output()
            .with_context(|| format!("cannot execute genbit {args:?}"))?;
        assert_eq!(
            output.status.success(),
            expected_success,
            "genbit {args:?}: unexpected status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        Ok(output)
    }
}

#[test]
fn creates_site_and_refuses_overwrite() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "test-blog"], true)?;
    let root = workspace.0.path().join("test-blog");
    for file in [
        "config.toml",
        "content/index.md",
        "templates/base.html",
        "templates/page.html",
        "styles/main.css",
        "static/.gitkeep",
        ".gitignore",
    ] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    assert!(fs::read_to_string(root.join("content/index.md"))?.contains("```rust"));
    fs::write(root.join("config.toml"), "user content")?;
    let output = workspace.run(&["new", "test-blog"], false)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("destination already exists"), "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join("config.toml"))?,
        "user content"
    );
    fs::create_dir(workspace.0.path().join("empty"))?;
    workspace.run(&["new", "empty"], false)?;
    fs::write(workspace.0.path().join("existing-file"), "keep me")?;
    workspace.run(&["new", "existing-file"], false)?;
    assert_eq!(
        fs::read_to_string(workspace.0.path().join("existing-file"))?,
        "keep me"
    );
    Ok(())
}

#[test]
fn rejects_paths_and_unsafe_names_without_writing() -> Result<()> {
    let workspace = Workspace::new()?;
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "/absolute",
        "nested/site",
        "quote\"",
        "a\\b",
    ] {
        let output = workspace.run(&["new", name], false)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("invalid site name"), "{name:?}: {stderr}");
    }
    assert_eq!(fs::read_dir(workspace.0.path())?.count(), 0);
    Ok(())
}

#[test]
fn help_and_unimplemented_commands_are_explicit() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["--help"], true)?;
    for command in ["build", "dev"] {
        let output = workspace.run(&[command], false)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not implemented yet"), "{stderr}");
    }
    Ok(())
}
