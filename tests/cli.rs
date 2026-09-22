use anyhow::{Context, Result, bail};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};
use tempfile::TempDir;

struct Workspace(TempDir);

struct DevProcess(Child);

impl Drop for DevProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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
fn help_is_available() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["--help"], true)?;
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
    assert!(home.contains("/hello-world/"));
    assert!(home.contains("/posts/another/"));
    let article = fs::read_to_string(site.join("dist/hello-world/index.html"))?;
    assert!(article.contains("<h1>&lt;Hello & world></h1>"), "{article}");
    assert!(article.contains("<strong>A post</strong>"));
    assert!(site.join("dist/posts/another/index.html").is_file());
    assert_eq!(fs::read(site.join("dist/logo.png"))?, [0, 1, 2, 255]);
    Ok(())
}

#[test]
fn builds_minified_html_with_lazy_images_and_inline_css() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(site.join("styles/main.css"), "h1 { color: red; }\n")?;
    fs::write(
        site.join("content/index.md"),
        "# Hello\n\n![A & B](photo.png \"Photo\")\n\n```\n  keep spacing\n```\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_genbit"))
        .arg("build")
        .current_dir(&site)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(html.contains("<style>h1{color:red}</style>"), "{html}");
    assert!(html.contains("src=photo.png"), "{html}");
    assert!(html.contains("alt=\"A & B\""), "{html}");
    assert!(html.contains("loading=lazy"), "{html}");
    assert!(html.contains("decoding=async"), "{html}");
    assert!(html.contains("  keep spacing"), "{html}");
    assert!(!html.contains("\n  <header>"), "{html}");
    Ok(())
}

#[test]
fn dev_serves_pages_and_pushes_reloads_after_source_changes() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(site.join("content/about.md"), "# About page\n")?;
    fs::write(site.join("static/asset.txt"), "static asset")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut server = DevProcess(
        Command::new(env!("CARGO_BIN_EXE_genbit"))
            .args(["dev", "--port", &port.to_string()])
            .current_dir(&site)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let address = format!("127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..50 {
        if TcpStream::connect(&address).is_ok() {
            ready = true;
            break;
        }
        assert!(
            server.0.try_wait()?.is_none(),
            "dev exited before listening"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "dev did not start");

    let page = http_get(&address, "/")?;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("EventSource"), "{page}");
    assert!(http_get(&address, "/about/")?.contains("About page"));
    assert!(http_get(&address, "/asset.txt")?.contains("static asset"));

    let mut events = TcpStream::connect(&address)?;
    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    events.write_all(
        b"GET /__genbit/reload HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
    )?;
    let mut data = Vec::new();
    let mut buffer = [0; 4096];
    while !data.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let read_count = events.read(&mut buffer)?;
        assert!(read_count > 0, "SSE connection closed before headers");
        data.extend(buffer.iter().take(read_count).copied());
    }
    assert!(String::from_utf8_lossy(&data).contains("text/event-stream"));

    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(site.join("content/index.md"), "+++\ntitle = [\n+++\n")?;
    events.set_read_timeout(Some(Duration::from_millis(500)))?;
    let Err(error) = events.read(&mut buffer) else {
        bail!("invalid source sent a reload");
    };
    assert!(matches!(
        error.kind(),
        ErrorKind::TimedOut | ErrorKind::WouldBlock
    ));
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);

    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    fs::write(site.join("content/index.md"), "# Changed in dev\n")?;
    while !String::from_utf8_lossy(&data).contains("data: reload") {
        let read_count = events.read(&mut buffer)?;
        assert!(read_count > 0, "SSE connection closed before reload");
        data.extend(buffer.iter().take(read_count).copied());
    }
    let updated = http_get(&address, "/")?;
    assert!(updated.contains("Changed in dev"), "{updated}");

    data.clear();
    fs::remove_file(site.join("content/about.md"))?;
    while !String::from_utf8_lossy(&data).contains("data: reload") {
        let read_count = events.read(&mut buffer)?;
        assert!(
            read_count > 0,
            "SSE connection closed before removal reload"
        );
        data.extend(buffer.iter().take(read_count).copied());
    }
    assert!(http_get(&address, "/about/")?.starts_with("HTTP/1.1 404"));

    let build = Command::new(env!("CARGO_BIN_EXE_genbit"))
        .arg("build")
        .current_dir(&site)
        .output()?;
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let production = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(!production.contains("EventSource"), "{production}");
    Ok(())
}

fn http_get(address: &str, path: &str) -> Result<String> {
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
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
