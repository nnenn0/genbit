use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, SystemTime},
};
use tempfile::TempDir;

struct Workspace(TempDir);

struct DevProcess {
    child: Child,
    log: PathBuf,
}

impl DevProcess {
    fn start(site: &Path, port: u16) -> Result<Self> {
        let log = site.join("dev.log");
        let log_file =
            fs::File::create(&log).with_context(|| format!("cannot create {}", log.display()))?;
        let child = Command::new(env!("CARGO_BIN_EXE_genbit"))
            .args(["dev", "--port", &port.to_string()])
            .current_dir(site)
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .spawn()
            .with_context(|| format!("cannot start dev in {}", site.display()))?;
        Ok(Self { child, log })
    }

    fn logs(&self) -> String {
        fs::read_to_string(&self.log)
            .unwrap_or_else(|error| format!("cannot read {}: {error}", self.log.display()))
    }

    /// Starts dev on a free port and waits until it accepts connections.
    fn start_listening(site: &Path) -> Result<(Self, String)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let mut server = Self::start(site, port)?;
        let address = format!("127.0.0.1:{port}");
        server.wait_until_listening(&address)?;
        Ok((server, address))
    }

    fn wait_for_log(&self, text: &str) -> Result<()> {
        for _ in 0..50 {
            if self.logs().contains(text) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!("missing {text:?} in dev logs:\n{}", self.logs())
    }

    fn wait_until_listening(&mut self, address: &str) -> Result<()> {
        for _ in 0..50 {
            if TcpStream::connect(address).is_ok() {
                return Ok(());
            }
            ensure!(
                self.child.try_wait()?.is_none(),
                "dev exited before listening:\n{}",
                self.logs()
            );
            thread::sleep(Duration::from_millis(100));
        }
        bail!("dev did not start:\n{}", self.logs())
    }

    /// Sends a signal such as `TERM` or `INT` and waits for dev to exit.
    fn stop(&mut self, signal: &str) -> Result<ExitStatus> {
        let sent = Command::new("kill")
            .args([format!("-{signal}"), self.child.id().to_string()])
            .status()
            .context("cannot run kill")?;
        ensure!(sent.success(), "cannot send SIG{signal} to dev");
        for _ in 0..50 {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(100));
        }
        bail!("dev did not stop after SIG{signal}:\n{}", self.logs())
    }
}

impl Drop for DevProcess {
    // Stopping with SIGTERM lets dev exit normally, which also writes coverage data.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) && self.stop("TERM").is_err() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn run_genbit(root: &Path, args: &[&str]) -> Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_genbit"))
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("cannot execute genbit {args:?} in {}", root.display()))
}

fn build_site(site: &Path) -> Result<Output> {
    run_genbit(site, &["build"])
}

fn dry_run_site(site: &Path) -> Result<Output> {
    run_genbit(site, &["build", "--dry-run"])
}

/// Returns every file under `dir` with its bytes and modification time.
fn snapshot(dir: &Path) -> Result<BTreeMap<PathBuf, (Vec<u8>, SystemTime)>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let modified = fs::metadata(&path)?.modified()?;
                files.insert(
                    path.strip_prefix(dir)?.to_path_buf(),
                    (fs::read(&path)?, modified),
                );
            }
        }
    }
    Ok(files)
}

fn assert_no_build_leftovers(site: &Path) -> Result<()> {
    for entry in fs::read_dir(site)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        ensure!(
            !name.starts_with(".genbit-build-") && !name.starts_with(".genbit-backup-"),
            "temporary build directory remains: {name}"
        );
    }
    Ok(())
}

/// Runs `genbit build` and reports its stderr when the build fails.
fn build_ok(site: &Path) -> Result<()> {
    let output = build_site(site)?;
    ensure!(
        output.status.success(),
        "genbit build failed in {}:\n{}",
        site.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Runs `genbit build`, requires it to fail, and returns its stderr.
fn build_err(site: &Path) -> Result<String> {
    let output = build_site(site)?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    ensure!(
        !output.status.success(),
        "genbit build unexpectedly succeeded in {}:\n{stderr}",
        site.display()
    );
    Ok(stderr)
}

/// Renders TOML lines from `defaults`. Each override replaces a field with a
/// raw TOML value, or removes it when the value is `None`.
fn toml_fields(defaults: &[(&str, &str)], overrides: &[(&str, Option<&str>)]) -> String {
    let mut fields = defaults.to_vec();
    for &(key, value) in overrides {
        fields.retain(|&(name, _)| name != key);
        if let Some(value) = value {
            fields.push((key, value));
        }
    }
    let mut source = String::new();
    for (key, value) in fields {
        source.push_str(key);
        source.push_str(" = ");
        source.push_str(value);
        source.push('\n');
    }
    source
}

const DEFAULT_CONFIG: [(&str, &str); 5] = [
    ("title", "'Blog'"),
    ("description", "'Blog articles'"),
    ("site_url", "'https://example.com/'"),
    ("og_image", "'/assets/img/ogp.png'"),
    ("timezone", "'UTC'"),
];

/// Writes `config.toml` from `DEFAULT_CONFIG` with `toml_fields` overrides.
fn write_config(site: &Path, overrides: &[(&str, Option<&str>)]) -> Result<()> {
    let path = site.join("config.toml");
    fs::write(&path, toml_fields(&DEFAULT_CONFIG, overrides))
        .with_context(|| format!("cannot write {}", path.display()))
}

const DEFAULT_ARTICLE: [(&str, &str); 3] = [
    ("created_at", "2026-09-17 00:00"),
    ("updated_at", "2026-09-17 00:00"),
    ("description", "'Test article'"),
];

/// Returns an article source whose front matter is `DEFAULT_ARTICLE` with
/// `toml_fields` overrides.
fn article_source(overrides: &[(&str, Option<&str>)], body: &str) -> String {
    format!(
        "+++\n{}+++\n{body}",
        toml_fields(&DEFAULT_ARTICLE, overrides)
    )
}

fn assert_sse_silent(stream: &mut TcpStream, timeout: Duration, server: &DevProcess) -> Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    let mut buffer = [0; 4096];
    match stream.read(&mut buffer) {
        Ok(read_count) => bail!(
            "unexpected SSE data {:?}:\n{}",
            String::from_utf8_lossy(buffer.get(..read_count).unwrap_or_default()),
            server.logs()
        ),
        Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Connects to the reload stream and waits until reloads caused by startup have stopped.
fn open_reload_stream(address: &str, server: &DevProcess) -> Result<TcpStream> {
    let mut events = TcpStream::connect(address)?;
    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    events.write_all(
        b"GET /__genbit/reload HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
    )?;
    let mut headers = Vec::new();
    read_sse_until(&mut events, &mut headers, b"\r\n\r\n", server)?;
    ensure!(
        String::from_utf8_lossy(&headers).contains("text/event-stream"),
        "missing SSE content type:\n{}",
        server.logs()
    );
    // macOS FSEvents may report files written just before `dev` started, which causes one
    // extra rebuild. Reloads must still stop, since a rebuild must not trigger itself.
    wait_for_sse_quiet(&mut events, server)?;
    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    Ok(events)
}

fn wait_for_sse_quiet(stream: &mut TcpStream, server: &DevProcess) -> Result<()> {
    for _ in 0..3 {
        if assert_sse_silent(stream, Duration::from_secs(1), server).is_ok() {
            return Ok(());
        }
    }
    bail!("SSE kept sending reloads without edits:\n{}", server.logs())
}

fn read_sse_until(
    stream: &mut TcpStream,
    data: &mut Vec<u8>,
    marker: &[u8],
    server: &DevProcess,
) -> Result<()> {
    let mut buffer = [0; 4096];
    while !data.windows(marker.len()).any(|bytes| bytes == marker) {
        let read_count = stream.read(&mut buffer).with_context(|| {
            format!(
                "SSE marker {:?} did not arrive:\n{}",
                String::from_utf8_lossy(marker),
                server.logs()
            )
        })?;
        if read_count == 0 {
            bail!("SSE closed before marker:\n{}", server.logs());
        }
        data.extend_from_slice(
            buffer
                .get(..read_count)
                .context("invalid SSE read length")?,
        );
    }
    Ok(())
}

impl Workspace {
    fn new() -> Result<Self> {
        Ok(Self(
            tempfile::tempdir().context("cannot create test workspace")?,
        ))
    }

    fn run(&self, args: &[&str], expected_success: bool) -> Result<Output> {
        let output = run_genbit(self.0.path(), args)?;
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

    fn new_site(&self, name: &str) -> Result<PathBuf> {
        self.run(&["new", name], true)?;
        Ok(self.0.path().join(name))
    }
}

#[test]
fn creates_site_and_refuses_overwrite() -> Result<()> {
    let workspace = Workspace::new()?;
    let root = workspace.new_site("test-blog")?;
    for file in [
        "config.toml",
        "content/entries/hello-world.md",
        "templates/base.html",
        "templates/page.html",
        "templates/root.html",
        "templates/tags.html",
        "templates/tag.html",
        "templates/404.html",
        "templates/entry-list.html",
        "styles/common.css",
        "styles/page.css",
        "styles/root.css",
        "styles/tags.css",
        "styles/tag.css",
        "static/assets/img/favicon.svg",
        "static/assets/img/favicon.png",
        "static/assets/img/ogp.png",
        ".gitignore",
    ] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    let article = fs::read_to_string(root.join("content/entries/hello-world.md"))?;
    assert!(article.contains("```rust"));
    assert!(article.contains("title = \"はじめての記事\""));
    assert!(article.contains("created_at = "));
    assert!(article.contains("description = "));
    let config = fs::read_to_string(root.join("config.toml"))?;
    assert!(config.contains("site_url = "));
    assert!(config.contains("og_image = \"/assets/img/ogp.png\""));
    assert!(config.contains("timezone = \"Asia/Tokyo\""));
    let og_image = fs::read(root.join("static/assets/img/ogp.png"))?;
    assert!(og_image.starts_with(b"\x89PNG\r\n\x1a\n"));
    let width = og_image.get(16..20).context("missing PNG width")?;
    let height = og_image.get(20..24).context("missing PNG height")?;
    assert_eq!(u32::from_be_bytes(width.try_into()?), 1200);
    assert_eq!(u32::from_be_bytes(height.try_into()?), 630);
    let favicon = fs::read(root.join("static/assets/img/favicon.png"))?;
    assert!(favicon.starts_with(b"\x89PNG\r\n\x1a\n"));
    let width = favicon.get(16..20).context("missing favicon PNG width")?;
    let height = favicon.get(20..24).context("missing favicon PNG height")?;
    assert_eq!(u32::from_be_bytes(width.try_into()?), 80);
    assert_eq!(u32::from_be_bytes(height.try_into()?), 80);
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
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(
            &[
                ("title", Some("\"<Hello & world>\"")),
                ("description", Some("'A post'")),
                ("updated_at", Some("2026-09-22 00:00")),
            ],
            "\n**A post**\n",
        ),
    )?;
    fs::create_dir(site.join("content/entries/posts"))?;
    fs::write(
        site.join("content/entries/posts/another.md"),
        article_source(
            &[
                ("created_at", Some("2026-09-16 00:00")),
                ("updated_at", Some("2026-09-16 00:00")),
            ],
            "# Another\n",
        ),
    )?;
    fs::write(site.join("static/logo.png"), [0, 1, 2, 255])?;
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("<style>"));
    assert!(home.contains("prefers-color-scheme"), "{home}");
    assert!(home.contains("href=/assets/img/favicon.png"), "{home}");
    assert!(home.contains("sizes=80x80"), "{home}");
    assert!(home.contains("type=image/png"), "{home}");
    assert!(home.contains("href=/assets/img/favicon.svg"), "{home}");
    assert!(home.contains("rel=icon"), "{home}");
    assert!(home.contains("type=image/svg+xml"), "{home}");
    assert!(home.contains("<h1>blog</h1>"), "{home}");
    assert!(home.contains("name=description"), "{home}");
    assert!(home.contains("property=og:title"), "{home}");
    assert!(home.contains("content=website property=og:type"), "{home}");
    assert!(home.contains("property=og:image"), "{home}");
    assert!(home.contains("property=og:image:alt"), "{home}");
    assert!(
        home.contains("http://127.0.0.1:3000/assets/img/ogp.png"),
        "{home}"
    );
    assert!(home.contains("\"@type\":\"WebSite\""), "{home}");
    assert!(!home.contains("<strong>A post</strong>"), "{home}");
    assert!(home.contains("/entries/hello-world"));
    assert!(home.contains("/entries/posts/another"));
    assert!(!home.contains("/entries/hello-world.html"));
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(article.contains("prefers-color-scheme"), "{article}");
    assert!(article.contains("<h1>&lt;Hello & world></h1>"), "{article}");
    for expected in [
        "<dt>created_at",
        "<dt>updated_at",
        "<time datetime=2026-09-17T00:00:00+09:00>2026-09-17</time>",
        "<time datetime=2026-09-22T00:00:00+09:00>2026-09-22</time>",
    ] {
        assert!(article.contains(expected), "{article}");
    }
    assert!(article.contains("<strong>A post</strong>"));
    assert!(article.contains("content=\"A post\""), "{article}");
    assert!(
        article.contains("content=article property=og:type"),
        "{article}"
    );
    assert!(article.contains("\"@type\":\"BlogPosting\""), "{article}");
    assert!(
        article.contains("\"datePublished\":\"2026-09-17T00:00:00+09:00\""),
        "{article}"
    );
    assert!(
        article.contains("\"dateModified\":\"2026-09-22T00:00:00+09:00\""),
        "{article}"
    );
    assert!(
        article.contains("\\u003cHello \\u0026 world\\u003e"),
        "{article}"
    );
    assert!(site.join("dist/entries/posts/another.html").is_file());
    let unmodified = fs::read_to_string(site.join("dist/entries/posts/another.html"))?;
    let date = "<time datetime=2026-09-16T00:00:00+09:00>2026-09-16</time>";
    assert_eq!(unmodified.matches(date).count(), 2, "{unmodified}");
    assert_eq!(fs::read(site.join("dist/logo.png"))?, [0, 1, 2, 255]);
    assert!(fs::read_to_string(site.join("dist/assets/img/favicon.svg"))?.contains("<svg"));
    assert!(site.join("dist/assets/img/favicon.png").is_file());
    assert!(site.join("dist/assets/img/ogp.png").is_file());
    Ok(())
}

#[test]
fn builds_not_found_page_without_indexing_metadata() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let not_found = fs::read_to_string(site.join("dist/404.html"))?;
    assert!(not_found.contains("<h1>404</h1>"), "{not_found}");
    assert!(not_found.contains("<p>Not Found"), "{not_found}");
    assert!(not_found.contains("name=robots"), "{not_found}");
    assert!(not_found.contains("content=noindex"), "{not_found}");
    assert!(!not_found.contains("rel=canonical"), "{not_found}");
    assert!(!not_found.contains("application/ld+json"), "{not_found}");
    Ok(())
}

#[test]
fn missing_404_template_fails_without_replacing_output() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let previous = fs::read(site.join("dist/404.html"))?;
    fs::remove_file(site.join("templates/404.html"))?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("404.html"), "{stderr}");
    assert_eq!(fs::read(site.join("dist/404.html"))?, previous);
    Ok(())
}

#[test]
fn article_description_uses_front_matter_instead_of_body() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(
            &[(
                "description",
                Some("'Chosen summary with quotes & friends'"),
            )],
            "# Heading\n\nShort [linked](../override.md) intro.\n\n- Skip this item\n\nMore detail with \"quotes\" & friends.\n",
        ),
    )?;
    fs::write(
        site.join("content/override.md"),
        article_source(
            &[
                ("created_at", Some("2026-09-18 00:00")),
                ("updated_at", Some("2026-09-18 00:00")),
                ("description", Some("'Chosen summary'")),
            ],
            "Body text.\n",
        ),
    )?;
    build_ok(&site)?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    let article_head = article.split("<style>").next().context("missing head")?;
    assert!(article_head.contains("name=description"), "{article}");
    assert!(
        article_head.contains("Chosen summary with quotes"),
        "{article}"
    );
    assert!(!article_head.contains("Short linked intro"), "{article}");
    assert!(!article_head.contains("Skip this item"), "{article}");
    let override_html = fs::read_to_string(site.join("dist/override.html"))?;
    let override_head = override_html
        .split("<style>")
        .next()
        .context("missing head")?;
    assert!(override_head.contains("Chosen summary"), "{override_html}");
    assert!(!override_head.contains("Body text"), "{override_html}");
    Ok(())
}

#[test]
fn descriptions_are_always_present_and_site_description_is_required() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "# Heading\n\n- List only\n"),
    )?;
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(home.contains("name=description"), "{home}");
    assert!(article.contains("name=description"), "{article}");
    assert!(article.contains("Test article"), "{article}");

    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[("description", None)], "# Heading\n"),
    )?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("description is required"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "# Heading\n"),
    )?;

    for description in [None, Some("'  '")] {
        write_config(&site, &[("description", description)])?;
        let stderr = build_err(&site)?;
        assert!(stderr.contains("description"), "{stderr}");
        assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
    }
    Ok(())
}

#[test]
fn site_url_generates_matching_canonicals_and_sitemap() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    assert!(site.join("dist/sitemap.xml").exists());
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("rel=canonical"), "{home}");
    assert!(home.contains("http://127.0.0.1:3000/"), "{home}");

    write_config(&site, &[])?;
    fs::create_dir(site.join("content/entries/posts"))?;
    fs::write(
        site.join("content/entries/posts/another.md"),
        article_source(
            &[
                ("updated_at", Some("2026-09-22 00:00")),
                ("description", Some("'Another article'")),
            ],
            "Another article.\n",
        ),
    )?;
    build_ok(&site)?;
    for (file, canonical) in [
        ("dist/index.html", "https://example.com/"),
        (
            "dist/entries/hello-world.html",
            "https://example.com/entries/hello-world",
        ),
        (
            "dist/entries/posts/another.html",
            "https://example.com/entries/posts/another",
        ),
    ] {
        let html = fs::read_to_string(site.join(file))?;
        assert!(html.contains("rel=canonical"), "{file}: {html}");
        assert!(html.contains(canonical), "{file}: {html}");
        assert!(
            html.contains("https://example.com/assets/img/ogp.png"),
            "{file}: {html}"
        );
    }
    let sitemap = fs::read_to_string(site.join("dist/sitemap.xml"))?;
    let robots = fs::read_to_string(site.join("dist/robots.txt"))?;
    assert_eq!(
        robots,
        "User-agent: *\nAllow: /\n\nSitemap: https://example.com/sitemap.xml\n"
    );
    assert!(sitemap.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    for url in [
        "https://example.com/",
        "https://example.com/entries/hello-world",
        "https://example.com/entries/posts/another",
        "https://example.com/tags/",
        "https://example.com/tags/untagged/",
    ] {
        assert!(sitemap.contains(&format!("<loc>{url}</loc>")), "{sitemap}");
    }
    assert_eq!(sitemap.matches("<url>").count(), 5);
    assert!(
        sitemap.contains(
            "<loc>https://example.com/entries/hello-world</loc><lastmod>2026-09-17T09:00:00+00:00</lastmod>"
        ),
        "{sitemap}"
    );
    assert!(
        sitemap.contains(
            "<loc>https://example.com/entries/posts/another</loc><lastmod>2026-09-22T00:00:00+00:00</lastmod>"
        ),
        "{sitemap}"
    );
    assert!(!sitemap.contains(".html"), "{sitemap}");
    Ok(())
}

#[test]
fn invalid_urls_and_sitemap_collision_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_config(&site, &[])?;
    build_ok(&site)?;
    let original = fs::read_to_string(site.join("dist/sitemap.xml"))?;
    write_config(&site, &[("site_url", None)])?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("site_url"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    for invalid_url in [
        "example.com",
        "ftp://example.com/",
        "https://user@example.com/",
        "https://example.com/blog/",
        "https://example.com/?q=1",
        "https://example.com/#fragment",
    ] {
        write_config(&site, &[("site_url", Some(&format!("'{invalid_url}'")))])?;
        let stderr = build_err(&site).with_context(|| format!("accepted {invalid_url}"))?;
        assert!(stderr.contains("site_url"), "{stderr}");
        assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    }
    write_config(&site, &[("og_image", None)])?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("og_image"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    for invalid_og_image in [
        "assets/img/ogp.png",
        "//example.com/ogp.png",
        "ftp://example.com/ogp.png",
        "https://user@example.com/ogp.png",
        "/assets/img/ogp.png#fragment",
    ] {
        write_config(
            &site,
            &[("og_image", Some(&format!("'{invalid_og_image}'")))],
        )?;
        let stderr = build_err(&site).with_context(|| format!("accepted {invalid_og_image}"))?;
        assert!(stderr.contains("og_image"), "{stderr}");
        assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    }
    write_config(
        &site,
        &[(
            "og_image",
            Some("'https://cdn.example.com/social/card.png'"),
        )],
    )?;
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("https://cdn.example.com/social/card.png"));
    write_config(&site, &[])?;
    fs::write(site.join("static/sitemap.xml"), "conflict")?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("output collision"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    Ok(())
}

#[test]
fn generates_rss_feed_with_autodiscovery_outside_sitemap() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_config(
        &site,
        &[
            ("title", Some("'Tom & Jerry <Blog>'")),
            ("timezone", Some("'Asia/Tokyo'")),
        ],
    )?;
    fs::write(
        site.join("content/entries/older.md"),
        article_source(
            &[
                ("title", Some("'Older <post>'")),
                ("created_at", Some("2026-09-16 23:30")),
                ("updated_at", Some("2026-09-30 12:00")),
                ("description", Some("'Fish & chips'")),
            ],
            "Older.\n",
        ),
    )?;
    build_ok(&site)?;
    let feed = fs::read_to_string(site.join("dist/feed.xml"))?;
    assert!(
        feed.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\""),
        "{feed}"
    );
    assert!(feed.contains("<title>Tom &amp; Jerry &lt;Blog&gt;</title>\n    <link>https://example.com/</link>\n    <description>Blog articles</description>"), "{feed}");
    assert!(feed.contains("<title>Older &lt;post&gt;</title>"), "{feed}");
    assert!(
        feed.contains("<description>Fish &amp; chips</description>"),
        "{feed}"
    );
    assert!(
        feed.contains("<guid>https://example.com/entries/older</guid>"),
        "{feed}"
    );
    assert!(
        feed.contains("<pubDate>Wed, 16 Sep 2026 23:30:00 +0900</pubDate>"),
        "{feed}"
    );
    assert!(!feed.contains("30 Sep 2026"), "{feed}");
    let newest = feed
        .find("/entries/hello-world")
        .context("missing hello-world")?;
    let older = feed.find("/entries/older").context("missing older")?;
    assert!(newest < older, "{feed}");
    let sitemap = fs::read_to_string(site.join("dist/sitemap.xml"))?;
    assert!(!sitemap.contains("feed.xml"), "{sitemap}");
    for file in [
        "dist/index.html",
        "dist/entries/older.html",
        "dist/tags/index.html",
        "dist/404.html",
    ] {
        let html = fs::read_to_string(site.join(file))?;
        assert!(
            html.contains(
                "<link title=\"Tom & Jerry <Blog>\" href=/feed.xml rel=alternate type=application/rss+xml>"
            ),
            "{file}: {html}"
        );
    }
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("<a href=/feed.xml>RSS</a>"), "{home}");

    let original = feed;
    for timezone in [Some("'+09:00'"), None] {
        write_config(&site, &[("timezone", timezone)])?;
        let stderr = build_err(&site)?;
        assert!(stderr.contains("timezone"), "{stderr}");
        assert_eq!(fs::read_to_string(site.join("dist/feed.xml"))?, original);
    }
    write_config(&site, &[])?;
    fs::write(site.join("static/feed.xml"), "conflict")?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("output collision"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/feed.xml"))?, original);
    fs::remove_file(site.join("static/feed.xml"))?;
    build_ok(&site)?;
    let utc = fs::read_to_string(site.join("dist/feed.xml"))?;
    assert!(
        utc.contains("<pubDate>Wed, 16 Sep 2026 23:30:00 +0000</pubDate>"),
        "{utc}"
    );
    Ok(())
}

#[test]
fn links_to_markdown_articles_use_clean_urls() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(
            &[],
            "[Next](next.md?view=full#details)\n\n[External](https://example.com/next.md)\n",
        ),
    )?;
    fs::write(
        site.join("content/entries/next.md"),
        article_source(
            &[
                ("created_at", Some("2026-09-18 00:00")),
                ("updated_at", Some("2026-09-18 00:00")),
            ],
            "# Next\n",
        ),
    )?;

    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(html.contains("href=\"next?view=full#details\""), "{html}");
    assert!(html.contains("href=https://example.com/next.md"), "{html}");
    assert!(html.contains("target=_blank"), "{html}");
    assert!(html.contains("rel=\"noopener noreferrer\""), "{html}");
    assert!(site.join("dist/entries/next.html").is_file());
    Ok(())
}

#[test]
fn homepage_lists_articles_by_creation_date() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for (name, date) in [
        ("old", "2026-09-05 00:00"),
        ("new", "2026-09-17 00:00"),
        ("middle", "2026-09-16 00:00"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            article_source(
                &[
                    ("title", Some(&format!("'{name}'"))),
                    ("created_at", Some(date)),
                    ("updated_at", Some(date)),
                ],
                &format!("# {name}\n"),
            ),
        )?;
    }
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let newest = home
        .find("/entries/new")
        .context("newest article missing")?;
    let middle = home
        .find("/entries/middle")
        .context("middle article missing")?;
    let oldest = home
        .find("/entries/old")
        .context("oldest article missing")?;
    assert!(newest < middle && middle < oldest, "{home}");
    for date in ["2026-09-17", "2026-09-16", "2026-09-05"] {
        assert!(home.contains(&format!("datetime={date}")), "{home}");
    }
    Ok(())
}

#[test]
fn generates_tag_pages_with_sorted_articles_and_sitemap_entries() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for (name, date, tags) in [
        ("older", "2026-09-17 00:00", "['rust', 'web-security']"),
        ("newer", "2026-09-19 00:00", "['rust']"),
        ("untagged", "2026-09-20 00:00", "[]"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            article_source(
                &[
                    ("created_at", Some(date)),
                    ("updated_at", Some(date)),
                    ("tags", Some(tags)),
                ],
                name,
            ),
        )?;
    }
    build_ok(&site)?;
    let tags_index = fs::read_to_string(site.join("dist/tags/index.html"))?;
    assert!(tags_index.contains("href=/tags/rust/"), "{tags_index}");
    assert!(tags_index.contains("(2)"), "{tags_index}");
    assert!(tags_index.contains("href=/tags/untagged/"), "{tags_index}");
    assert!(
        tags_index.contains("href=/tags/web-security/"),
        "{tags_index}"
    );
    assert!(
        tags_index
            .find("/tags/rust/")
            .context("rust link missing")?
            < tags_index
                .find("/tags/web-security/")
                .context("web-security link missing")?
    );
    let rust = fs::read_to_string(site.join("dist/tags/rust/index.html"))?;
    assert!(
        rust.find("/entries/newer").context("newer missing")?
            < rust.find("/entries/older").context("older missing")?,
        "{rust}"
    );
    assert!(!rust.contains("/entries/untagged"), "{rust}");
    let untagged_page = fs::read_to_string(site.join("dist/tags/untagged/index.html"))?;
    assert!(
        untagged_page.contains("/entries/untagged"),
        "{untagged_page}"
    );
    assert!(!untagged_page.contains("/entries/newer"), "{untagged_page}");
    assert!(rust.contains("http://127.0.0.1:3000/tags/rust/"), "{rust}");
    let article = fs::read_to_string(site.join("dist/entries/newer.html"))?;
    assert!(article.contains("href=/tags/rust/"), "{article}");
    let untagged = fs::read_to_string(site.join("dist/entries/untagged.html"))?;
    assert!(!untagged.contains("<nav"), "{untagged}");
    let sitemap = fs::read_to_string(site.join("dist/sitemap.xml"))?;
    for path in [
        "/tags/",
        "/tags/rust/",
        "/tags/web-security/",
        "/tags/untagged/",
    ] {
        assert!(
            sitemap.contains(&format!("<loc>http://127.0.0.1:3000{path}</loc>")),
            "{sitemap}"
        );
    }
    Ok(())
}

#[test]
fn invalid_tags_and_generated_tag_url_collisions_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let original = fs::read_to_string(site.join("dist/tags/index.html"))?;
    let article_path = site.join("content/entries/invalid.md");
    for (tags, reason) in [
        ("['React']", "invalid tag \"React\""),
        ("['react', 'react']", "duplicate tag \"react\""),
        ("['untagged']", "tag \"untagged\" is reserved"),
    ] {
        fs::write(
            &article_path,
            article_source(&[("tags", Some(tags))], "Body"),
        )?;
        let stderr = build_err(&site).with_context(|| format!("accepted {tags}"))?;
        assert!(stderr.contains(reason), "{tags}: {stderr}");
        assert_eq!(
            fs::read_to_string(site.join("dist/tags/index.html"))?,
            original
        );
    }
    fs::remove_file(&article_path)?;
    fs::write(site.join("content/tags.md"), article_source(&[], "Body"))?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("collision at /tags"), "{stderr}");
    assert_eq!(
        fs::read_to_string(site.join("dist/tags/index.html"))?,
        original
    );
    Ok(())
}

#[test]
fn homepage_orders_same_day_articles_by_creation_time_but_shows_date() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for (name, created_at) in [
        ("a-early", "2026-09-17 08:00"),
        ("b-same", "2026-09-17T08:01"),
        ("z-late", "2026-09-17T08:01"),
        ("m-midnight", "2026-09-17 00:00"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            article_source(
                &[
                    ("created_at", Some(created_at)),
                    ("updated_at", Some(created_at)),
                ],
                &format!("# {name}\n"),
            ),
        )?;
    }
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let same = home
        .find("/entries/b-same")
        .context("same-time article missing")?;
    let late = home
        .find("/entries/z-late")
        .context("late article missing")?;
    let early = home
        .find("/entries/a-early")
        .context("early article missing")?;
    let midnight = home
        .find("/entries/m-midnight")
        .context("midnight article missing")?;
    assert!(same < late && late < early && early < midnight, "{home}");
    assert!(
        home.contains("<time datetime=2026-09-17T08:00:00+09:00>2026-09-17</time>"),
        "{home}"
    );
    Ok(())
}

#[test]
fn builds_minified_html_with_lazy_images_and_inline_css() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(site.join("styles/common.css"), "h1 { color: red; }\n")?;
    fs::remove_file(site.join("styles/page.css"))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(
            &[],
            "# Hello\n\n![A & B](photo.png \"Photo\")\n\n```\n  keep spacing\n```\n",
        ),
    )?;
    fs::create_dir_all(site.join("static/entries"))?;
    fs::write(site.join("static/entries/photo.png"), [0])?;
    build_ok(&site)?;
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
    let site = workspace.new_site("blog")?;
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
    build_ok(&site)?;

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
fn custom_article_template_receives_documented_fields() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("templates/custom.html"),
        "<html><head><style>{{ css | safe }}</style><script type=\"application/ld+json\">{{ json_ld | safe }}</script></head><body><p>{{ site.title }}</p><p>{{ canonical_url }}</p><p>{{ article.title }}|{{ article.description }}|{{ article.url }}|{{ article.created_at.datetime }}|{{ article.created_at.date }}|{{ article.created_at.time }}|{{ article.updated_at.datetime }}</p>{{ content | safe }}</body></html>",
    )?;
    fs::write(
        site.join("styles/custom.css"),
        "body { --custom-marker: yes; }",
    )?;
    fs::write(
        site.join("content/custom.md"),
        article_source(
            &[
                ("title", Some("'Custom Post'")),
                ("description", Some("'Custom summary'")),
                ("template", Some("'custom.html'")),
                ("created_at", Some("2026-09-17 10:30")),
                ("updated_at", Some("2026-09-22 00:00")),
            ],
            "**Custom body**\n",
        ),
    )?;
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/custom.html"))?;
    assert!(html.contains("--custom-marker"), "{html}");
    assert!(html.contains("http://127.0.0.1:3000/custom"), "{html}");
    assert!(
        html.contains("Custom Post|Custom summary|/custom|2026-09-17T10:30:00+09:00|2026-09-17|10:30|2026-09-22T00:00:00+09:00"),
        "{html}"
    );
    assert!(html.contains("<strong>Custom body</strong>"), "{html}");
    assert!(
        html.contains("\"datePublished\":\"2026-09-17T10:30:00+09:00\""),
        "{html}"
    );
    assert!(!html.contains("EventSource"), "{html}");
    Ok(())
}

#[test]
fn dev_serves_clean_urls_static_files_and_not_found_page() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/about.md"),
        article_source(&[], "# About page\n"),
    )?;
    fs::create_dir_all(site.join("content/entries/posts"))?;
    fs::write(
        site.join("content/entries/posts/nested.md"),
        article_source(&[], "# Nested page\n"),
    )?;
    fs::write(site.join("static/asset.txt"), "static asset")?;
    let (_server, address) = DevProcess::start_listening(&site)?;

    let page = http_get(&address, "/")?;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("EventSource"), "{page}");
    assert!(http_get(&address, "/about?view=full")?.contains("About page"));
    assert!(http_get(&address, "/entries/posts/nested?view=full")?.contains("Nested page"));
    assert!(http_get(&address, "/entries/posts/nested.html")?.contains("Nested page"));
    assert!(http_get(&address, "/asset.txt")?.contains("static asset"));
    assert_not_found_page(&address, "/missing/path")?;
    Ok(())
}

#[test]
fn dev_reloads_after_an_article_is_edited() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (server, address) = DevProcess::start_listening(&site)?;
    let mut events = open_reload_stream(&address, &server)?;

    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "# Changed in dev\n"),
    )?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    let updated = http_get(&address, "/entries/hello-world")?;
    assert!(updated.contains("Changed in dev"), "{updated}");
    Ok(())
}

#[test]
fn dev_keeps_serving_after_failed_rebuild_and_reloads_after_fix() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (server, address) = DevProcess::start_listening(&site)?;
    let mut events = open_reload_stream(&address, &server)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;

    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ntitle = [\n+++\n",
    )?;
    assert_sse_silent(&mut events, Duration::from_millis(500), &server)?;
    server.wait_for_log("Rebuild failed")?;
    assert!(http_get(&address, "/")?.contains(&before));

    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "# Fixed in dev\n"),
    )?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    let fixed = http_get(&address, "/entries/hello-world")?;
    assert!(fixed.contains("Fixed in dev"), "{fixed}");
    Ok(())
}

#[test]
fn dev_reloads_after_an_article_is_removed() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/about.md"),
        article_source(&[], "# About page\n"),
    )?;
    let (server, address) = DevProcess::start_listening(&site)?;
    assert!(http_get(&address, "/about")?.contains("About page"));
    let mut events = open_reload_stream(&address, &server)?;

    fs::remove_file(site.join("content/about.md"))?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    assert_not_found_page(&address, "/about")?;
    Ok(())
}

#[test]
fn dev_stops_cleanly_on_sigterm_with_an_open_reload_stream() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (mut server, address) = DevProcess::start_listening(&site)?;
    let _events = open_reload_stream(&address, &server)?;

    let status = server.stop("TERM")?;
    assert!(status.success(), "{status}\n{}", server.logs());
    assert!(
        server.logs().contains("Stopping server"),
        "{}",
        server.logs()
    );
    Ok(())
}

#[test]
fn dev_stops_cleanly_on_ctrl_c() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (mut server, _address) = DevProcess::start_listening(&site)?;

    let status = server.stop("INT")?;
    assert!(status.success(), "{status}\n{}", server.logs());
    assert!(site.join("dist/index.html").is_file());
    assert!(!fs::read_dir(&site)?.any(|entry| {
        entry.is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with(".genbit-"))
    }));
    Ok(())
}

#[test]
fn dev_port_conflict_does_not_build_or_replace_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let occupied = TcpListener::bind("127.0.0.1:0")?;
    let port = occupied.local_addr()?.port();
    let args = ["dev", "--port", &port.to_string()];
    let failed = run_genbit(&site, &args)?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("cannot bind"));
    assert!(!site.join("dist").exists());

    assert!(build_site(&site)?.status.success());
    let previous = fs::read(site.join("dist/index.html"))?;
    fs::write(site.join("content/entries/hello-world.md"), "invalid")?;
    let failed = run_genbit(&site, &args)?;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("cannot bind"));
    assert_eq!(fs::read(site.join("dist/index.html"))?, previous);
    Ok(())
}

fn assert_not_found_page(address: &str, path: &str) -> Result<()> {
    let response = http_get(address, path)?;
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    assert!(response.contains("<p>Not Found"), "{response}");
    assert!(response.contains("EventSource"), "{response}");
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
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ntitle = [\n+++\nInvalid",
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("content/entries/hello-world.md"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "# Working again\n"),
    )?;
    fs::write(
        site.join("content/stale.md"),
        article_source(&[], "# Old\n"),
    )?;
    build_ok(&site)?;
    assert!(site.join("dist/stale.html").is_file());
    fs::remove_file(site.join("content/stale.md"))?;
    build_ok(&site)?;
    assert!(!site.join("dist/stale.html").exists());
    Ok(())
}

#[test]
fn invalid_article_dates_report_source_and_preserve_previous_output() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    let article = site.join("content/entries/hello-world.md");
    for (dates, reason) in [
        (
            "created_at = \"2026-09-17\"",
            "created_at must be a TOML local date-time",
        ),
        (
            "created_at = 2026-09-17",
            "created_at must be a TOML local date-time",
        ),
        (
            "created_at = 2026-09-17 10:00:00",
            "created_at must be a TOML local date-time",
        ),
        (
            "created_at = 2026-09-17T10:00:00Z",
            "created_at must be a TOML local date-time",
        ),
        (
            "created_at = 2026-09-17T10:00:00.5",
            "created_at must be a TOML local date-time",
        ),
        (
            "created_at = 2026-09-17 00:00\nupdated_at = 2026-09-16 23:59",
            "updated_at must not precede created_at",
        ),
        (
            "created_at = 2026-09-17 00:00\nupdated_at = 2026-09-18",
            "updated_at must be a TOML local date-time",
        ),
        (
            "created_at = 2026-09-17 00:00\nupdated_at = 2026-09-18 10:00:00",
            "updated_at must be a TOML local date-time",
        ),
    ] {
        fs::write(
            &article,
            format!("+++\n{dates}\ndescription = 'Test article'\n+++\n# Post\n"),
        )?;
        let stderr = build_err(&site).with_context(|| format!("accepted {dates}"))?;
        assert!(
            stderr.contains("content/entries/hello-world.md"),
            "{stderr}"
        );
        assert!(stderr.contains(reason), "{stderr}");
        assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    }
    Ok(())
}

#[test]
fn rejects_output_collisions_without_touching_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(
        site.join("content/about.md"),
        article_source(&[], "# First\n"),
    )?;
    fs::write(site.join("static/about.html"), "conflict")?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("output collision"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    Ok(())
}

#[test]
fn rejects_served_url_collisions_and_reserved_paths_without_touching_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    let post = article_source(&[], "# Post\n");
    let post = post.as_str();

    for (files, reason) in [
        (
            &["content/foo.md", "static/foo"][..],
            "URL collision at /foo",
        ),
        (
            &["content/foo.md", "static/foo/index.html"][..],
            "URL collision at /foo",
        ),
        (
            &["static/entries/hello-world"][..],
            "URL collision at /entries/hello-world",
        ),
        (&["static/index"][..], "URL collision at /index"),
        (&["content/__genbit/reload.md"][..], "reserved URL"),
        (&["static/__genbit/asset.txt"][..], "reserved URL"),
    ] {
        for relative in files {
            let path = site.join(relative);
            fs::create_dir_all(path.parent().context("test input has no parent")?)?;
            fs::write(
                &path,
                if Path::new(relative)
                    .extension()
                    .is_some_and(|ext| ext == "md")
                {
                    post
                } else {
                    "asset"
                },
            )?;
        }
        let stderr = build_err(&site).with_context(|| format!("accepted {files:?}"))?;
        assert!(stderr.contains(reason), "{files:?}: {stderr}");
        for relative in files {
            assert!(stderr.contains(relative), "{files:?}: {stderr}");
            fs::remove_file(site.join(relative))?;
        }
        assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
        assert_eq!(
            fs::read_to_string(site.join("dist/entries/hello-world.html"))?,
            article
        );
    }

    fs::create_dir_all(site.join("content/foo"))?;
    fs::write(site.join("content/foo.md"), post)?;
    fs::write(site.join("content/foo/bar.md"), post)?;
    build_ok(&site)?;
    assert!(site.join("dist/foo.html").is_file());
    assert!(site.join("dist/foo/bar.html").is_file());
    Ok(())
}

#[test]
fn protects_unrecognized_dist_and_detects_static_file_conflicts() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;

    fs::create_dir(site.join("dist"))?;
    fs::write(site.join("dist/keep.txt"), "user file")?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("unrecognized dist"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/keep.txt"))?, "user file");

    fs::remove_dir_all(site.join("dist"))?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(site.join("static/index.html"), "conflict")?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("output collision"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    Ok(())
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_in_site_inputs_without_touching_dist() -> Result<()> {
    use std::os::unix::fs::symlink;

    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    let original_css = fs::read_to_string(site.join("styles/common.css"))?;

    for relative in ["config.toml", "content", "templates", "styles", "static"] {
        let input = site.join(relative);
        let outside = workspace.0.path().join(format!("outside-{relative}"));
        fs::rename(&input, &outside)?;
        symlink(&outside, &input)?;

        let stderr = build_err(&site).with_context(|| format!("accepted {relative}"))?;
        assert!(
            stderr.contains("symlinks are not supported"),
            "{relative}: {stderr}"
        );
        assert!(stderr.contains(relative), "{relative}: {stderr}");
        assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);

        fs::remove_file(&input)?;
        fs::rename(&outside, &input)?;
    }
    assert_eq!(
        fs::read_to_string(site.join("styles/common.css"))?,
        original_css
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn nested_template_css_rejects_parent_and_file_symlinks() -> Result<()> {
    use std::os::unix::fs::symlink;

    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::create_dir_all(site.join("templates/deep"))?;
    fs::write(
        site.join("templates/deep/page.html"),
        "{% extends \"base.html\" %}{% block main %}Nested template{% endblock main %}",
    )?;
    fs::create_dir_all(site.join("styles/deep"))?;
    fs::write(
        site.join("styles/deep/page.css"),
        "body { --nested-css: yes; }",
    )?;
    fs::write(
        site.join("content/deep.md"),
        article_source(
            &[
                ("description", Some("'Nested article'")),
                ("template", Some("'deep/page.html'")),
            ],
            "# Deep\n",
        ),
    )?;

    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/deep.html"))?;
    assert!(before.contains("Nested template") && before.contains("--nested-css"));

    let parent = site.join("styles/deep");
    let outside_dir = workspace.0.path().join("outside-deep");
    fs::rename(&parent, &outside_dir)?;
    symlink(&outside_dir, &parent)?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("symlinks are not supported"), "{stderr}");
    assert!(stderr.contains("styles/deep"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/deep.html"))?, before);
    fs::remove_file(&parent)?;
    fs::rename(&outside_dir, &parent)?;

    let css = parent.join("page.css");
    let outside_css = workspace.0.path().join("outside-page.css");
    fs::rename(&css, &outside_css)?;
    symlink(&outside_css, &css)?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("symlinks are not supported"), "{stderr}");
    assert!(stderr.contains("page.css"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/deep.html"))?, before);
    assert_eq!(
        fs::read_to_string(&outside_css)?,
        "body { --nested-css: yes; }"
    );
    Ok(())
}

#[test]
fn internal_links_to_generated_pages_and_static_files_build() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(
            &[],
            concat!(
                "[next](next.md#details) [abs](/entries/next) [html](/entries/next.html?x=1)\n\n",
                "[about](../about.md) [home](/) [tags](/tags/) [tag](/tags/untagged) [feed](/feed.xml)\n\n",
                "![ogp](/assets/img/ogp.png) ![relative](../assets/img/ogp.png) ",
                "![encoded](/files/%E7%94%BB%E5%83%8F.png) ![raw](/files/画像.png)\n\n",
                "[web](https://example.com/missing) [plain](http://example.com/missing) ",
                "[mail](mailto:someone@example.com) [tel](tel:+81-3-0000-0000) ",
                "[top](#top) <someone@example.com>\n\n",
                "## [unwrapped](missing.md)\n",
            ),
        ),
    )?;
    fs::write(
        site.join("content/entries/next.md"),
        article_source(&[], "## Details\n"),
    )?;
    fs::write(
        site.join("content/about.md"),
        article_source(&[], "About\n"),
    )?;
    fs::create_dir(site.join("static/files"))?;
    fs::write(site.join("static/files/画像.png"), [0])?;
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(html.contains("href=next#details"), "{html}");
    assert!(html.contains("href=../about"), "{html}");
    Ok(())
}

#[test]
fn broken_internal_links_fail_and_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (body, expected) in [
        (
            "[missing](/entries/missing)",
            "/entries/missing resolves to /entries/missing",
        ),
        (
            "[missing](missing.md#x)",
            "missing#x resolves to /entries/missing",
        ),
        (
            "![missing](/assets/img/missing.png)",
            "/assets/img/missing.png resolves to /assets/img/missing.png",
        ),
        (
            "![missing](../missing.png)",
            "../missing.png resolves to /missing.png",
        ),
        (
            "[case](/Entries/Hello-World)",
            "/Entries/Hello-World resolves to /Entries/Hello-World",
        ),
        (
            "[slash](/entries/hello-world/)",
            "/entries/hello-world/ resolves to /entries/hello-world/",
        ),
    ] {
        fs::write(
            site.join("content/entries/hello-world.md"),
            article_source(&[], body),
        )?;
        let stderr = build_err(&site)?;
        assert!(
            stderr.contains(&format!(
                "broken internal link in content/entries/hello-world.md: {expected}, which is not generated"
            )),
            "{stderr}"
        );
        assert_eq!(snapshot(&site.join("dist"))?, before);
    }
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "[bad](a%zz)"),
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("invalid internal link in content/entries/hello-world.md: a%zz"),
        "{stderr}"
    );
    assert!(stderr.contains("invalid percent-encoding"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn dry_run_builds_without_creating_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let output = dry_run_site(&site)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("dist/ was not changed"), "{stdout}");
    assert!(!site.join("dist").exists());
    assert_no_build_leftovers(&site)
}

#[test]
fn dry_run_leaves_existing_dist_unchanged() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        article_source(&[], "# Changed\n"),
    )?;
    fs::write(
        site.join("content/added.md"),
        article_source(&[], "Added\n"),
    )?;
    let output = dry_run_site(&site)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(snapshot(&site.join("dist"))?, before);
    assert_no_build_leftovers(&site)
}

#[test]
fn dry_run_fails_with_the_same_errors_as_build() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (body, expected) in [
        ("+++\ntitle = [\n+++\nInvalid".to_owned(), "cannot parse"),
        (
            article_source(&[], "[missing](/entries/missing)"),
            "broken internal link",
        ),
    ] {
        fs::write(site.join("content/entries/hello-world.md"), body)?;
        let output = dry_run_site(&site)?;
        assert!(!output.status.success());
        let dry_run_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(dry_run_stderr.contains(expected), "{dry_run_stderr}");
        assert_eq!(dry_run_stderr, build_err(&site)?);
        assert_eq!(snapshot(&site.join("dist"))?, before);
        assert_no_build_leftovers(&site)?;
    }
    Ok(())
}

#[test]
fn dry_run_refuses_unrecognized_dist_like_build() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::create_dir(site.join("dist"))?;
    fs::write(site.join("dist/keep.txt"), "user file")?;
    let before = snapshot(&site.join("dist"))?;
    let output = dry_run_site(&site)?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to replace unrecognized dist directory"),
        "{stderr}"
    );
    assert_eq!(snapshot(&site.join("dist"))?, before);
    assert_no_build_leftovers(&site)
}
