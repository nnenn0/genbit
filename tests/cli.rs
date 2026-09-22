#![allow(
    clippy::unwrap_used,
    reason = "Test setup and fixture assertions should fail immediately on unexpected errors"
)]

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "genbit-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_genbit"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn creates_site_and_refuses_overwrite() {
    let workspace = Workspace::new();
    assert!(workspace.run(&["new", "test-blog"]).status.success());
    let root = workspace.0.join("test-blog");
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
    assert!(
        fs::read_to_string(root.join("content/index.md"))
            .unwrap()
            .contains("```rust")
    );
    fs::write(root.join("config.toml"), "user content").unwrap();
    assert!(!workspace.run(&["new", "test-blog"]).status.success());
    assert_eq!(
        fs::read_to_string(root.join("config.toml")).unwrap(),
        "user content"
    );
    fs::create_dir(workspace.0.join("empty")).unwrap();
    assert!(!workspace.run(&["new", "empty"]).status.success());
}

#[test]
fn rejects_paths_and_unsafe_names_without_writing() {
    let workspace = Workspace::new();
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
        assert!(
            !workspace.run(&["new", name]).status.success(),
            "accepted {name}"
        );
    }
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn help_and_unimplemented_commands_are_explicit() {
    let workspace = Workspace::new();
    assert!(workspace.run(&["--help"]).status.success());
    for command in ["build", "dev"] {
        let output = workspace.run(&[command]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not implemented yet"));
    }
}
