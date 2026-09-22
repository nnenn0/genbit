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
    let output = workspace.run(&["dev"], false)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not implemented yet"), "{stderr}");
    Ok(())
}

#[test]
fn builds_pages_and_assets_from_generated_site() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(
        site.join("content/hello-world.md"),
        "+++\ntitle = \"<Hello & world>\"\n+++\n\n**A post**\n",
    )?;
    fs::create_dir(site.join("content/posts"))?;
    fs::write(site.join("content/posts/another.md"), "# Another\n")?;
    fs::write(site.join("static/logo.png"), [0, 1, 2, 255])?;
    let output = Command::new(env!("CARGO_BIN_EXE_genbit"))
        .arg("build")
        .current_dir(&site)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("<style>"));
    assert!(home.contains("href=\"/hello-world/\""));
    assert!(home.contains("href=\"/posts/another/\""));
    let article = fs::read_to_string(site.join("dist/hello-world/index.html"))?;
    assert!(article.contains("&lt;Hello &amp; world&gt;"));
    assert!(article.contains("<strong>A post</strong>"));
    assert!(site.join("dist/posts/another/index.html").is_file());
    assert_eq!(fs::read(site.join("dist/logo.png"))?, [0, 1, 2, 255]);
    Ok(())
}

#[test]
fn bad_input_preserves_previous_output_and_rebuild_removes_stale_pages() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    let build = || -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_genbit"))
            .arg("build")
            .current_dir(&site)
            .output()?)
    };
    assert!(build()?.status.success());
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(
        site.join("content/index.md"),
        "+++\ntitle = [\n+++\nInvalid",
    )?;
    let failed = build()?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("content/index.md"));
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    fs::write(site.join("content/index.md"), "# Working again\n")?;
    fs::write(site.join("content/stale.md"), "# Old\n")?;
    assert!(build()?.status.success());
    assert!(site.join("dist/stale/index.html").is_file());
    fs::remove_file(site.join("content/stale.md"))?;
    assert!(build()?.status.success());
    assert!(!site.join("dist/stale/index.html").exists());
    Ok(())
}

#[test]
fn rejects_output_collisions_without_touching_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    let build = || -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_genbit"))
            .arg("build")
            .current_dir(&site)
            .output()?)
    };
    assert!(build()?.status.success());
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(site.join("content/about.md"), "# First\n")?;
    fs::create_dir(site.join("content/about"))?;
    fs::write(site.join("content/about/index.md"), "# Second\n")?;
    let failed = build()?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("output collision"));
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    Ok(())
}

#[test]
fn protects_unrecognized_dist_and_detects_static_file_conflicts() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    let build = || -> Result<Output> {
        Ok(Command::new(env!("CARGO_BIN_EXE_genbit"))
            .arg("build")
            .current_dir(&site)
            .output()?)
    };

    fs::create_dir(site.join("dist"))?;
    fs::write(site.join("dist/keep.txt"), "user file")?;
    let failed = build()?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("unrecognized dist"));
    assert_eq!(fs::read_to_string(site.join("dist/keep.txt"))?, "user file");

    fs::remove_dir_all(site.join("dist"))?;
    assert!(build()?.status.success());
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(site.join("static/index.html"), "conflict")?;
    let failed = build()?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("output collision"));
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    Ok(())
}
