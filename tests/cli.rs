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
        "content/entries/hello-world.md",
        "templates/base.html",
        "templates/page.html",
        "templates/root.html",
        "styles/common.css",
        "styles/page.css",
        "styles/root.css",
        "static/assets/img/favicon.svg",
        ".gitignore",
    ] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    let article = fs::read_to_string(root.join("content/entries/hello-world.md"))?;
    assert!(article.contains("```rust"));
    assert!(article.contains("title = \"はじめての記事\""));
    assert!(article.contains("created_at = "));
    assert!(!root.join("content/index.md").exists());
    assert!(!root.join("content/root.md").exists());
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
        site.join("content/entries/hello-world.md"),
        "+++\ntitle = \"<Hello & world>\"\ncreated_at = 2026-09-17\nupdated_at = 2026-09-22\n+++\n\n**A post**\n",
    )?;
    fs::create_dir(site.join("content/entries/posts"))?;
    fs::write(
        site.join("content/entries/posts/another.md"),
        "+++\ncreated_at = 2026-09-16\n+++\n# Another\n",
    )?;
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
    assert!(home.contains("href=/assets/img/favicon.svg"), "{home}");
    assert!(home.contains("rel=icon"), "{home}");
    assert!(home.contains("type=image/svg+xml"), "{home}");
    assert!(home.contains("<h1>blog</h1>"), "{home}");
    assert!(!home.contains("<strong>A post</strong>"), "{home}");
    assert!(home.contains("/entries/hello-world.html"));
    assert!(home.contains("/entries/posts/another.html"));
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(article.contains("<h1>&lt;Hello & world></h1>"), "{article}");
    assert!(article.contains("<strong>A post</strong>"));
    assert!(site.join("dist/entries/posts/another.html").is_file());
    assert_eq!(fs::read(site.join("dist/logo.png"))?, [0, 1, 2, 255]);
    assert!(fs::read_to_string(site.join("dist/assets/img/favicon.svg"))?.contains("<svg"));
    Ok(())
}

#[test]
fn links_to_markdown_articles_use_generated_html_paths() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n[Next](next.md?view=full#details)\n\n[External](https://example.com/next.md)\n",
    )?;
    fs::write(
        site.join("content/entries/next.md"),
        "+++\ncreated_at = 2026-09-18\n+++\n# Next\n",
    )?;

    let build = Command::new(env!("CARGO_BIN_EXE_genbit"))
        .arg("build")
        .current_dir(&site)
        .output()?;
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let html = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(
        html.contains("href=\"next.html?view=full#details\""),
        "{html}"
    );
    assert!(html.contains("href=https://example.com/next.md"), "{html}");
    assert!(site.join("dist/entries/next.html").is_file());
    Ok(())
}

#[test]
fn homepage_lists_articles_by_creation_date() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    for (name, date) in [
        ("old", "2026-09-05"),
        ("new", "2026-09-17"),
        ("middle", "2026-09-16"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            format!("+++\ntitle = \"{name}\"\ncreated_at = {date}\n+++\n# {name}\n"),
        )?;
    }
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
    let newest = home
        .find("/entries/new.html")
        .context("newest article missing")?;
    let middle = home
        .find("/entries/middle.html")
        .context("middle article missing")?;
    let oldest = home
        .find("/entries/old.html")
        .context("oldest article missing")?;
    assert!(newest < middle && middle < oldest, "{home}");
    for date in ["2026-09-17", "2026-09-16", "2026-09-05"] {
        assert!(home.contains(&format!("datetime={date}")), "{home}");
    }
    Ok(())
}

#[test]
fn homepage_orders_same_day_articles_by_creation_time_but_shows_date() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    for (name, created_at) in [
        ("a-early", "2026-09-17 08:00"),
        ("z-late", "2026-09-17T08:00:40"),
        ("m-legacy", "2026-09-17"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            format!("+++\ncreated_at = {created_at}\n+++\n# {name}\n"),
        )?;
    }
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
    let late = home
        .find("/entries/z-late.html")
        .context("late article missing")?;
    let early = home
        .find("/entries/a-early.html")
        .context("early article missing")?;
    let legacy = home
        .find("/entries/m-legacy.html")
        .context("legacy article missing")?;
    assert!(late < early && early < legacy, "{home}");
    assert!(home.contains("datetime=2026-09-17"), "{home}");
    assert!(
        !home.contains("08:00") && !home.contains("08:00:40"),
        "{home}"
    );
    Ok(())
}

#[test]
fn builds_minified_html_with_lazy_images_and_inline_css() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(site.join("styles/common.css"), "h1 { color: red; }\n")?;
    fs::remove_file(site.join("styles/page.css"))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# Hello\n\n![A & B](photo.png \"Photo\")\n\n```\n  keep spacing\n```\n",
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
    let html = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
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
fn inlines_common_and_template_specific_css() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(
        site.join("styles/common.css"),
        "body { --common-marker: yes; }",
    )?;
    fs::write(
        site.join("styles/root.css"),
        ":root { --root-marker: yes; }",
    )?;
    fs::write(
        site.join("styles/page.css"),
        ":root { --page-marker: yes; }",
    )?;
    let build = Command::new(env!("CARGO_BIN_EXE_genbit"))
        .arg("build")
        .current_dir(&site)
        .output()?;
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(home.contains("--common-marker"), "{home}");
    assert!(home.contains("--root-marker"), "{home}");
    assert!(!home.contains("--page-marker"), "{home}");
    assert!(article.contains("--common-marker"), "{article}");
    assert!(article.contains("--page-marker"), "{article}");
    assert!(!article.contains("--root-marker"), "{article}");
    Ok(())
}

#[test]
fn dev_serves_pages_and_pushes_reloads_after_source_changes() -> Result<()> {
    let workspace = Workspace::new()?;
    workspace.run(&["new", "blog"], true)?;
    let site = workspace.0.path().join("blog");
    fs::write(
        site.join("content/about.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# About page\n",
    )?;
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
    assert!(http_get(&address, "/about.html")?.contains("About page"));
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
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ntitle = [\n+++\n",
    )?;
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
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# Changed in dev\n",
    )?;
    while !String::from_utf8_lossy(&data).contains("data: reload") {
        let read_count = events.read(&mut buffer)?;
        assert!(read_count > 0, "SSE connection closed before reload");
        data.extend(buffer.iter().take(read_count).copied());
    }
    let updated = http_get(&address, "/entries/hello-world.html")?;
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
    assert!(http_get(&address, "/about.html")?.starts_with("HTTP/1.1 404"));

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
        site.join("content/entries/hello-world.md"),
        "+++\ntitle = [\n+++\nInvalid",
    )?;
    let failed = build()?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("content/entries/hello-world.md"));
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# Working again\n",
    )?;
    fs::write(
        site.join("content/stale.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# Old\n",
    )?;
    assert!(build()?.status.success());
    assert!(site.join("dist/stale.html").is_file());
    fs::remove_file(site.join("content/stale.md"))?;
    assert!(build()?.status.success());
    assert!(!site.join("dist/stale.html").exists());
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
    fs::write(
        site.join("content/about.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# First\n",
    )?;
    fs::write(site.join("static/about.html"), "conflict")?;
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
