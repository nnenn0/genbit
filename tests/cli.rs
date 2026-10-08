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

    /// Starts dev on a port chosen by the OS and returns the address it reports.
    fn start_listening(site: &Path) -> Result<(Self, String)> {
        let mut server = Self::start(site, 0)?;
        let address = server.wait_for_address()?;
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

    /// Reads the address from this process's own log. Picking a free port in the test and
    /// passing it to dev lets a parallel test take the same port before dev binds it, and a
    /// connection check would then succeed against the other test's listener.
    fn wait_for_address(&mut self) -> Result<String> {
        const PREFIX: &str = "Server running at http://";
        for _ in 0..80 {
            let logs = self.logs();
            if let Some(address) = logs
                .split_inclusive('\n')
                .find_map(|line| line.strip_prefix(PREFIX)?.strip_suffix('\n'))
            {
                return Ok(address.to_owned());
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

/// Writes a file, creating its parent directories, as a page directory needs.
fn write_file(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))
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

/// Returns the hash that genbit added to the first image whose `src` starts with `source`.
fn image_version(html: &str, source: &str) -> Result<String> {
    // The minifier quotes the value because the query contains `=`.
    let start = html
        .find(&format!("src=\"{source}"))
        .with_context(|| format!("no image {source}: {html}"))?;
    let attribute = html
        .get(start + "src=\"".len()..)
        .and_then(|rest| rest.split('"').next())
        .context("unterminated src")?;
    let (_, after) = attribute
        .split_once("v=")
        .with_context(|| format!("no hash in {attribute}"))?;
    let version = after
        .get(..16)
        .with_context(|| format!("short hash in {attribute}"))?;
    ensure!(
        version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid hash in {attribute}"
    );
    Ok(version.to_owned())
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
    ("og_image", "'/assets/site/ogp.png'"),
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
        "content/entries/hello-world/index.md",
        "views/pages/root.bv",
        "views/pages/root.css",
        "views/pages/page.bv",
        "views/pages/page.css",
        "views/pages/tags.bv",
        "views/pages/tags.css",
        "views/pages/tag.bv",
        "views/pages/tag.css",
        "views/pages/not-found.bv",
        "views/components/document.bv",
        "views/components/document.css",
        "views/components/layout.bv",
        "views/components/home-link.bv",
        "views/components/entry-list.bv",
        "views/components/entry-list.css",
        "views/components/draft-badge.bv",
        "views/components/draft-badge.css",
        "views/components/timestamp.bv",
        "static/assets/site/favicon.svg",
        "static/assets/site/favicon.png",
        "static/assets/site/ogp.png",
        "drafts/entries/.gitkeep",
        ".gitignore",
    ] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    // Images shared by several articles go here; files with fixed URLs are kept apart in static/assets/site.
    assert!(
        fs::read_dir(root.join("static/assets/img"))?
            .next()
            .is_none()
    );
    let article = fs::read_to_string(root.join("content/entries/hello-world/index.md"))?;
    assert!(article.contains("```rust"));
    assert!(article.contains("title = \"はじめての記事\""));
    assert!(article.contains("created_at = "));
    assert!(article.contains("description = "));
    let config = fs::read_to_string(root.join("config.toml"))?;
    assert!(config.contains("site_url = "));
    assert!(config.contains("og_image = \"/assets/site/ogp.png\""));
    assert!(config.contains("timezone = \"Asia/Tokyo\""));
    let og_image = fs::read(root.join("static/assets/site/ogp.png"))?;
    assert!(og_image.starts_with(b"\x89PNG\r\n\x1a\n"));
    let width = og_image.get(16..20).context("missing PNG width")?;
    let height = og_image.get(20..24).context("missing PNG height")?;
    assert_eq!(u32::from_be_bytes(width.try_into()?), 1200);
    assert_eq!(u32::from_be_bytes(height.try_into()?), 630);
    let favicon = fs::read(root.join("static/assets/site/favicon.png"))?;
    assert!(favicon.starts_with(b"\x89PNG\r\n\x1a\n"));
    let width = favicon.get(16..20).context("missing favicon PNG width")?;
    let height = favicon.get(20..24).context("missing favicon PNG height")?;
    assert_eq!(u32::from_be_bytes(width.try_into()?), 96);
    assert_eq!(u32::from_be_bytes(height.try_into()?), 96);
    assert!(!root.join("content/index.md").exists());
    assert!(!root.join("content/root/index.md").exists());
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
    write_file(
        site.join("content/entries/hello-world/index.md"),
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
    write_file(
        site.join("content/entries/posts/another/index.md"),
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
    assert!(home.contains("href=/assets/site/favicon.png"), "{home}");
    assert!(home.contains("sizes=96x96"), "{home}");
    assert!(home.contains("type=image/png"), "{home}");
    assert!(home.contains("href=/assets/site/favicon.svg"), "{home}");
    assert!(home.contains("rel=icon"), "{home}");
    assert!(home.contains("type=image/svg+xml"), "{home}");
    assert!(home.contains("<h1>blog</h1>"), "{home}");
    assert!(home.contains("name=description"), "{home}");
    assert!(home.contains("property=og:title"), "{home}");
    assert!(home.contains("content=website property=og:type"), "{home}");
    assert!(home.contains("property=og:image"), "{home}");
    assert!(home.contains("property=og:image:alt"), "{home}");
    assert!(
        home.contains("http://127.0.0.1:3000/assets/site/ogp.png"),
        "{home}"
    );
    assert!(home.contains("\"@type\":\"WebSite\""), "{home}");
    assert!(!home.contains("<strong>A post</strong>"), "{home}");
    assert!(home.contains("href=/entries/hello-world/"), "{home}");
    assert!(home.contains("href=/entries/posts/another/"), "{home}");
    let article = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
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
    assert!(site.join("dist/entries/posts/another/index.html").is_file());
    let unmodified = fs::read_to_string(site.join("dist/entries/posts/another/index.html"))?;
    let date = "<time datetime=2026-09-16T00:00:00+09:00>2026-09-16</time>";
    assert_eq!(unmodified.matches(date).count(), 2, "{unmodified}");
    assert_eq!(fs::read(site.join("dist/logo.png"))?, [0, 1, 2, 255]);
    assert!(fs::read_to_string(site.join("dist/assets/site/favicon.svg"))?.contains("<svg"));
    assert!(site.join("dist/assets/site/favicon.png").is_file());
    assert!(site.join("dist/assets/site/ogp.png").is_file());
    Ok(())
}

#[test]
fn tool_files_are_not_copied_or_treated_as_page_assets() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for name in [".gitkeep", ".DS_Store"] {
        for directory in [
            "static",
            "static/assets/img",
            "content",
            "content/entries",
            "content/entries/hello-world",
            "drafts/entries",
        ] {
            fs::write(site.join(directory).join(name), "written by a tool")?;
        }
    }
    build_ok(&site)?;
    for name in [".gitkeep", ".DS_Store"] {
        for directory in [
            "dist",
            "dist/assets/img",
            "dist/entries",
            "dist/entries/hello-world",
        ] {
            let path = site.join(directory).join(name);
            assert!(!path.exists(), "{}", path.display());
        }
    }
    assert!(site.join("dist/assets/site/favicon.svg").is_file());
    let (_server, address) = DevProcess::start_listening(&site)?;
    assert!(http_get(&address, "/.DS_Store")?.starts_with("HTTP/1.1 404"));
    Ok(())
}

#[test]
fn copies_page_assets_next_to_their_page() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "[notes](notes.txt)\n"),
    )?;
    fs::write(site.join("content/entries/hello-world/notes.txt"), "notes")?;
    build_ok(&site)?;
    assert_eq!(
        fs::read_to_string(site.join("dist/entries/hello-world/notes.txt"))?,
        "notes"
    );
    let html = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    assert!(html.contains("href=notes.txt"), "{html}");
    Ok(())
}

#[test]
fn content_files_outside_the_page_rule_fail_and_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (relative, expected) in [
        (
            "content/index.md",
            "content/index.md is not supported; the home page is generated",
        ),
        (
            "content/entries/post.md",
            "content/entries/post.md is not supported; write each page as index.md",
        ),
        (
            "content/entries/hello-world/notes.md",
            "content/entries/hello-world/notes.md is not supported",
        ),
        (
            "content/entries/photo.png",
            "content/entries/photo.png is not in a page directory",
        ),
        (
            "content/entries/hello-world/images/a.png",
            "content/entries/hello-world/images/a.png is in a subdirectory of the page content/entries/hello-world",
        ),
    ] {
        let path = site.join(relative);
        write_file(&path, article_source(&[], "Body"))?;
        let stderr = build_err(&site)?;
        assert!(stderr.contains(expected), "{relative}: {stderr}");
        assert_eq!(snapshot(&site.join("dist"))?, before);
        fs::remove_file(&path)?;
    }
    let images = site.join("content/entries/hello-world/images");
    let expected = "content/entries/hello-world/images/ is a subdirectory of the page content/entries/hello-world";
    for keep in [false, true] {
        if keep {
            write_file(images.join(".gitkeep"), "")?;
        }
        let stderr = build_err(&site)?;
        assert!(stderr.contains(expected), "gitkeep {keep}: {stderr}");
        let output = dry_run_site(&site)?;
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "dry run, gitkeep {keep}: {stderr}"
        );
        assert_eq!(snapshot(&site.join("dist"))?, before);
    }
    fs::remove_dir_all(images)?;
    build_ok(&site)?;
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
    fs::remove_file(site.join("views/pages/not-found.bv"))?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains(
            "views/pages/not-found.bv must define fn not-found(ctx), which renders 404.html"
        ),
        "{stderr}"
    );
    assert_eq!(fs::read(site.join("dist/404.html"))?, previous);
    Ok(())
}

#[test]
fn article_description_uses_front_matter_instead_of_body() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[(
                "description",
                Some("'Chosen summary with quotes & friends'"),
            )],
            "# Heading\n\nShort [linked](../../override/) intro.\n\n- Skip this item\n\nMore detail with \"quotes\" & friends.\n",
        ),
    )?;
    write_file(
        site.join("content/override/index.md"),
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
    let article = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    let article_head = article.split("<style>").next().context("missing head")?;
    assert!(article_head.contains("name=description"), "{article}");
    assert!(
        article_head.contains("Chosen summary with quotes"),
        "{article}"
    );
    assert!(!article_head.contains("Short linked intro"), "{article}");
    assert!(!article_head.contains("Skip this item"), "{article}");
    let override_html = fs::read_to_string(site.join("dist/override/index.html"))?;
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
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "# Heading\n\n- List only\n"),
    )?;
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    assert!(home.contains("name=description"), "{home}");
    assert!(article.contains("name=description"), "{article}");
    assert!(article.contains("Test article"), "{article}");

    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[("description", None)], "# Heading\n"),
    )?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("description is required"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
    write_file(
        site.join("content/entries/hello-world/index.md"),
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
    write_file(
        site.join("content/entries/posts/another/index.md"),
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
            "dist/entries/hello-world/index.html",
            "https://example.com/entries/hello-world/",
        ),
        (
            "dist/entries/posts/another/index.html",
            "https://example.com/entries/posts/another/",
        ),
    ] {
        let html = fs::read_to_string(site.join(file))?;
        assert!(html.contains("rel=canonical"), "{file}: {html}");
        assert!(html.contains(canonical), "{file}: {html}");
        assert!(
            html.contains("https://example.com/assets/site/ogp.png"),
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
        "https://example.com/entries/hello-world/",
        "https://example.com/entries/posts/another/",
        "https://example.com/tags/",
        "https://example.com/tags/untagged/",
    ] {
        assert!(sitemap.contains(&format!("<loc>{url}</loc>")), "{sitemap}");
    }
    assert_eq!(sitemap.matches("<url>").count(), 5);
    assert!(
        sitemap.contains(
            "<loc>https://example.com/entries/hello-world/</loc><lastmod>2026-09-17T09:00:00+00:00</lastmod>"
        ),
        "{sitemap}"
    );
    assert!(
        sitemap.contains(
            "<loc>https://example.com/entries/posts/another/</loc><lastmod>2026-09-22T00:00:00+00:00</lastmod>"
        ),
        "{sitemap}"
    );
    assert!(!sitemap.contains(".html"), "{sitemap}");
    Ok(())
}

#[test]
fn invalid_ipv4_configuration_preserves_dist_in_build_and_dry_run() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for field in ["site_url", "og_image"] {
        for host in ["999.999.999.999", "127.0.0.01", "127.1"] {
            let value = format!("'https://{host}/'");
            write_config(&site, &[(field, Some(&value))])?;
            let error = build_err(&site)?;
            assert!(error.contains(field) && error.contains(host), "{error}");
            let dry_run = dry_run_site(&site)?;
            assert!(!dry_run.status.success());
            let error = String::from_utf8(dry_run.stderr)?;
            assert!(error.contains(field) && error.contains(host), "{error}");
            assert_eq!(snapshot(&site.join("dist"))?, before);
            assert_no_build_leftovers(&site)?;
        }
    }
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
        "https://example.com:abc/",
        "https://example.com:99999/",
        "https://exa_mple.com/",
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
        "assets/site/ogp.png",
        "//example.com/ogp.png",
        "ftp://example.com/ogp.png",
        "https://user@example.com/ogp.png",
        "/assets/site/ogp.png#fragment",
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
    write_file(
        site.join("content/entries/older/index.md"),
        article_source(
            &[
                ("title", Some("'Older <post>'")),
                ("created_at", Some("2026-09-16 23:30")),
                ("updated_at", Some("2026-09-30 12:00")),
                (
                    "description",
                    Some("'''Use Vec<T>, &copy;, A & B, \"quotes\" and 'apostrophes'.'''"),
                ),
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
        feed.contains("<description>Use Vec&amp;lt;T&amp;gt;, &amp;amp;copy;, A &amp;amp; B, &amp;quot;quotes&amp;quot; and &amp;apos;apostrophes&amp;apos;.</description>"),
        "{feed}"
    );
    assert!(
        feed.contains("<guid>https://example.com/entries/older/</guid>"),
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
        "dist/entries/older/index.html",
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
fn links_between_pages_keep_their_urls() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[],
            "[Next](../next/?view=full#details)\n\n[External](https://example.com/next.md)\n\n<person@example.md> [Mail](mailto:person@example.md)\n",
        ),
    )?;
    write_file(
        site.join("content/entries/next/index.md"),
        article_source(
            &[
                ("created_at", Some("2026-09-18 00:00")),
                ("updated_at", Some("2026-09-18 00:00")),
            ],
            "# Next\n",
        ),
    )?;

    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    assert!(
        html.contains("href=\"../next/?view=full#details\""),
        "{html}"
    );
    assert!(html.contains("href=https://example.com/next.md"), "{html}");
    assert_eq!(
        html.matches("href=mailto:person@example.md").count(),
        2,
        "{html}"
    );
    assert!(html.contains("target=_blank"), "{html}");
    assert!(html.contains("rel=\"noopener noreferrer\""), "{html}");
    assert!(site.join("dist/entries/next/index.html").is_file());
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
        write_file(
            site.join(format!("content/entries/{name}/index.md")),
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
        write_file(
            site.join(format!("content/entries/{name}/index.md")),
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
    let article = fs::read_to_string(site.join("dist/entries/newer/index.html"))?;
    assert!(article.contains("href=/tags/rust/"), "{article}");
    let untagged = fs::read_to_string(site.join("dist/entries/untagged/index.html"))?;
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
    let article_path = site.join("content/entries/invalid/index.md");
    for (tags, reason) in [
        ("['React']", "invalid tag \"React\""),
        ("['react', 'react']", "duplicate tag \"react\""),
        ("['untagged']", "tag \"untagged\" is reserved"),
    ] {
        write_file(
            &article_path,
            article_source(&[("tags", Some(tags))], "Body"),
        )?;
        let stderr = build_err(&site).with_context(|| format!("accepted {tags}"))?;
        assert!(
            stderr.contains(reason) && stderr.contains("content/entries/invalid/index.md"),
            "{tags}: {stderr}"
        );
        assert_eq!(
            fs::read_to_string(site.join("dist/tags/index.html"))?,
            original
        );
    }
    fs::remove_file(&article_path)?;
    write_file(
        site.join("content/tags/index.md"),
        article_source(&[], "Body"),
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("output collision at tags/index.html"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(site.join("dist/tags/index.html"))?,
        original
    );
    fs::remove_dir_all(site.join("content/tags"))?;
    write_file(
        &article_path,
        article_source(&[("tags", Some("['index']"))], "Body"),
    )?;
    build_ok(&site)?;
    assert!(site.join("dist/tags/index/index.html").is_file());
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
        write_file(
            site.join(format!("content/entries/{name}/index.md")),
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
    fs::write(
        site.join("views/components/document.css"),
        "h1 { color: red; }\n",
    )?;
    fs::remove_file(site.join("views/pages/page.css"))?;
    fs::remove_file(site.join("views/components/draft-badge.css"))?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[],
            "# Hello\n\n![A & B](photo.png \"Photo\")\n\n```\n  keep spacing\n```\n",
        ),
    )?;
    fs::write(
        site.join("content/entries/hello-world/photo.png"),
        include_bytes!("fixtures/images/basic.png"),
    )?;
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    assert!(html.contains("<style>h1{color:red}</style>"), "{html}");
    image_version(&html, "photo.png?v=")?;
    assert!(html.contains("width=300"), "{html}");
    assert_eq!(
        fs::read(site.join("dist/entries/hello-world/photo.png"))?,
        include_bytes!("fixtures/images/basic.png")
    );
    assert!(html.contains("alt=\"A & B\""), "{html}");
    assert!(html.contains("loading=lazy"), "{html}");
    assert!(html.contains("decoding=async"), "{html}");
    assert!(html.contains("  keep spacing"), "{html}");
    assert!(!html.contains("\n  <header>"), "{html}");
    Ok(())
}

#[test]
fn image_dimensions_follow_local_urls_and_refresh_on_rebuild() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("images")?;
    let image = site.join("static/photo.data");
    let original = include_bytes!("fixtures/images/basic.png");
    fs::write(&image, original)?;
    fs::write(site.join("static/notes.data"), b"notes")?;
    fs::write(site.join("static/icon.svg"), "<svg/>")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[],
            "![local](../../%70hoto.data#part)\n\n![again](/photo.data)\n\n![data](/notes.data)\n\n![svg](/icon.svg)\n\n![remote](https://example.invalid/photo.png)",
        ),
    )?;
    build_ok(&site)?;
    let output = site.join("dist/entries/hello-world/index.html");
    let html = fs::read_to_string(&output)?;
    assert_eq!(html.matches("width=300").count(), 2, "{html}");
    assert_eq!(html.matches("height=200").count(), 2, "{html}");
    assert_eq!(html.matches("loading=lazy").count(), 5, "{html}");
    let version = image_version(&html, "../../%70hoto.data?v=")?;
    assert_eq!(image_version(&html, "/photo.data?v=")?, version);
    assert!(html.contains(&format!("?v={version}#part")), "{html}");
    image_version(&html, "/notes.data?v=")?;
    image_version(&html, "/icon.svg?v=")?;
    assert!(
        html.contains("src=https://example.invalid/photo.png>"),
        "{html}"
    );
    assert_eq!(fs::read(site.join("dist/photo.data"))?, original);
    let before = snapshot(&site.join("dist"))?;
    let rotated = include_bytes!("fixtures/images/exif.jpg");
    fs::write(&image, rotated)?;
    assert!(dry_run_site(&site)?.status.success());
    assert_eq!(snapshot(&site.join("dist"))?, before);
    build_ok(&site)?;
    let html = fs::read_to_string(&output)?;
    assert_eq!(html.matches("width=200").count(), 2, "{html}");
    assert_eq!(html.matches("height=300").count(), 2, "{html}");
    assert_ne!(image_version(&html, "/photo.data?v=")?, version);
    assert_eq!(fs::read(site.join("dist/photo.data"))?, rotated);
    let before = snapshot(&site.join("dist"))?;
    fs::remove_file(image)?;
    assert!(build_err(&site)?.contains("photo.data"));
    assert_eq!(snapshot(&site.join("dist"))?, before);
    assert_no_build_leftovers(&site)?;
    Ok(())
}

#[test]
fn image_hashes_depend_only_on_content_and_reserve_the_v_query() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("images")?;
    let png = include_bytes!("fixtures/images/basic.png");
    fs::write(site.join("static/a.png"), png)?;
    fs::create_dir(site.join("static/copies"))?;
    fs::write(site.join("static/copies/b.png"), png)?;
    let article = site.join("content/entries/hello-world/index.md");
    fs::write(
        &article,
        article_source(&[], "![a](/a.png)\n\n![b](/copies/b.png?x=1#top)\n"),
    )?;
    build_ok(&site)?;
    let output = site.join("dist/entries/hello-world/index.html");
    let html = fs::read_to_string(&output)?;
    let version = image_version(&html, "/a.png?v=")?;
    assert_eq!(image_version(&html, "/copies/b.png?x=1")?, version);
    assert!(html.contains(&format!("v={version}#top")), "{html}");
    build_ok(&site)?;
    assert_eq!(fs::read_to_string(&output)?, html);

    let before = snapshot(&site.join("dist"))?;
    fs::write(&article, article_source(&[], "![a](/a.png?v=2)\n"))?;
    let error = build_err(&site)?;
    assert!(error.contains("has the query parameter v"), "{error}");
    assert!(error.contains("hello-world/index.md"), "{error}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    assert_no_build_leftovers(&site)?;
    Ok(())
}

#[test]
fn default_css_stacks_centers_and_only_shrinks_images() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "![a](/a.png) ![b](/b.png)\n"),
    )?;
    for name in ["a.png", "b.png"] {
        fs::write(
            site.join("static").join(name),
            include_bytes!("fixtures/images/basic.png"),
        )?;
    }
    build_ok(&site)?;
    for page in ["dist/entries/hello-world/index.html", "dist/index.html"] {
        let html = fs::read_to_string(site.join(page))?;
        let image_css = html
            .split("img{")
            .nth(1)
            .and_then(|css| css.split('}').next())
            .context("missing image CSS")?;
        for declaration in [
            "display:block",
            "max-width:100%",
            "height:auto",
            "margin-inline:auto",
        ] {
            assert!(image_css.contains(declaration), "{page}: {image_css}");
        }
    }
    Ok(())
}

#[test]
fn broken_images_fail_the_build_without_touching_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("images")?;
    let article = site.join("content/entries/hello-world/index.md");
    fs::write(&article, article_source(&[], "![photo](/photo.png)"))?;
    fs::write(
        site.join("static/photo.png"),
        include_bytes!("fixtures/images/basic.png"),
    )?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (bytes, expected) in [
        (
            b"not an image".as_slice(),
            "the content is not a PNG, JPEG, GIF, or WebP image",
        ),
        (
            include_bytes!("fixtures/images/exif.webp").as_slice(),
            "remove the EXIF metadata or convert the image to JPEG",
        ),
    ] {
        fs::write(site.join("static/photo.png"), bytes)?;
        let error = build_err(&site)?;
        assert!(error.contains("hello-world/index.md"), "{error}");
        assert!(error.contains("static/photo.png"), "{error}");
        assert!(error.contains(expected), "{error}");
        assert_eq!(snapshot(&site.join("dist"))?, before);
    }
    fs::write(&article, article_source(&[], "text"))?;
    build_ok(&site)?;
    assert_no_build_leftovers(&site)?;
    Ok(())
}

#[test]
fn inlines_the_css_of_the_functions_each_page_uses() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for (css, marker) in [
        ("components/document.css", "--document-marker"),
        ("components/entry-list.css", "--list-marker"),
        ("pages/root.css", "--root-marker"),
        ("pages/page.css", "--page-marker"),
    ] {
        fs::write(
            site.join("views").join(css),
            format!(":root {{ {marker}: yes; }}"),
        )?;
    }
    build_ok(&site)?;

    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    let tag = fs::read_to_string(site.join("dist/tags/untagged/index.html"))?;
    let position = |html: &str, marker: &str| html.find(marker).unwrap_or(usize::MAX);
    assert!(
        position(&home, "--document-marker") < position(&home, "--list-marker")
            && position(&home, "--list-marker") < position(&home, "--root-marker")
            && home.contains("--root-marker"),
        "{home}"
    );
    assert!(!home.contains("--page-marker"), "{home}");
    assert!(
        position(&article, "--document-marker") < position(&article, "--page-marker")
            && article.contains("--page-marker"),
        "{article}"
    );
    assert!(!article.contains("--list-marker"), "{article}");
    assert!(!article.contains("--root-marker"), "{article}");
    assert!(tag.contains("--list-marker"), "{tag}");
    assert!(!tag.contains("--root-marker"), "{tag}");
    Ok(())
}

#[test]
fn views_hold_pages_components_and_their_css() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (path, expected) in [
        (
            "views/notes.txt",
            "views/notes.txt is outside views/pages/ and views/components/",
        ),
        (
            "views/parts/list.bv",
            "views/parts is not a views directory; views/ holds pages/ and components/",
        ),
        (
            "views/components/parts/list.bv",
            "views/components/parts is not a views directory",
        ),
        (
            "views/components/notes.txt",
            "views/components/notes.txt is neither a .bv nor a .css file",
        ),
        (
            "views/pages/about.bv",
            "views/pages/about.bv is not a page; views/pages/ holds views/pages/root.bv",
        ),
        (
            "views/components/entry_list.css",
            "views/components/entry_list.css does not belong to a function defined in views/components/",
        ),
        (
            "views/pages/document.css",
            "views/pages/document.css does not belong to a function defined in views/pages/",
        ),
    ] {
        write_file(site.join(path), "")?;
        let stderr = build_err(&site)?;
        assert!(stderr.contains(expected), "{path}: {stderr}");
        assert_eq!(snapshot(&site.join("dist"))?, before);
        fs::remove_file(site.join(path))?;
        if let Some(parent) = Path::new(path).parent()
            && parent.ends_with("parts")
        {
            fs::remove_dir(site.join(parent))?;
        }
    }

    let page = site.join("views/pages/root.bv");
    let moved = site.join("views/components/root.bv");
    fs::rename(&page, &moved)?;
    let stderr = build_err(&site)?;
    assert!(
        stderr
            .contains("views/pages/root.bv must define fn root(ctx), which renders the home page"),
        "{stderr}"
    );
    fs::rename(&moved, &page)?;

    for hidden in [
        "views/.DS_Store",
        "views/pages/.DS_Store",
        "views/components/.cache/notes.txt",
    ] {
        write_file(site.join(hidden), "written by a tool")?;
    }
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    fs::write(
        site.join("views/pages/page.css"),
        "a { color: red; } </style>",
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("invalid CSS in views/pages/page.css"),
        "{stderr}"
    );
    assert!(stderr.contains("must not contain </style"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn article_template_receives_documented_fields() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("views/pages/page.bv"),
        concat!(
            "fn page(ctx) => html(head(ctx.style, ctx.json-ld), body(\n",
            "  p(ctx.site.title), p(ctx.canonical-url),\n",
            "  p(concat(ctx.article.title, \"|\", ctx.article.description, \"|\", ctx.article.url, \"|\",\n",
            "    ctx.article.created-at.datetime, \"|\", ctx.article.created-at.date, \"|\",\n",
            "    ctx.article.updated-at.datetime)),\n",
            "  ctx.content))\n",
        ),
    )?;
    fs::write(
        site.join("views/pages/page.css"),
        "body { --custom-marker: yes; }",
    )?;
    write_file(
        site.join("content/custom/index.md"),
        article_source(
            &[
                ("title", Some("'Custom Post'")),
                ("description", Some("'Custom summary'")),
                ("created_at", Some("2026-09-17 10:30")),
                ("updated_at", Some("2026-09-22 00:00")),
            ],
            "**Custom body**\n",
        ),
    )?;
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/custom/index.html"))?;
    assert!(html.contains("--custom-marker"), "{html}");
    assert!(html.contains("http://127.0.0.1:3000/custom/"), "{html}");
    assert!(
        html.contains("Custom Post|Custom summary|/custom/|2026-09-17T10:30:00+09:00|2026-09-17|2026-09-22T00:00:00+09:00"),
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
fn dev_serves_page_directories_static_files_and_not_found_page() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/about/index.md"),
        article_source(&[], "# About page\n"),
    )?;
    fs::create_dir_all(site.join("content/entries/posts"))?;
    write_file(
        site.join("content/entries/posts/nested/index.md"),
        article_source(&[], "# Nested page\n"),
    )?;
    fs::write(site.join("static/asset.txt"), "static asset")?;
    let (_server, address) = DevProcess::start_listening(&site)?;

    let page = http_get(&address, "/")?;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("EventSource"), "{page}");
    assert!(http_get(&address, "/about/?view=full")?.contains("About page"));
    let redirect = http_get(&address, "/about")?;
    assert!(redirect.starts_with("HTTP/1.1 307"), "{redirect}");
    assert!(
        redirect
            .to_ascii_lowercase()
            .contains("\r\nlocation: /about/\r\n"),
        "{redirect}"
    );
    assert!(http_get(&address, "/entries/posts/nested/")?.contains("Nested page"));
    assert!(http_get(&address, "/entries/posts/nested/index.html")?.contains("Nested page"));
    assert!(http_get(&address, "/entries/posts/%6Eested/")?.contains("Nested page"));
    assert_not_found_page(&address, "/entries/posts/nested.html")?;
    assert!(http_get(&address, "/asset.txt")?.contains("static asset"));
    assert_not_found_page(&address, "/missing/path")?;
    Ok(())
}

#[test]
fn dev_ignores_conditional_requests_and_disables_caching() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(site.join("static/asset.txt"), "static asset")?;
    let (_server, address) = DevProcess::start_listening(&site)?;
    // Each header would otherwise turn the response into 304 or 412 for an existing file.
    for header in [
        "",
        "If-Modified-Since: Fri, 01 Jan 2100 00:00:00 GMT\r\n",
        "If-None-Match: *\r\n",
        "If-Match: \"stale\"\r\n",
        "If-Unmodified-Since: Thu, 01 Jan 1970 00:00:00 GMT\r\n",
    ] {
        for (path, status, body) in [
            ("/entries/hello-world/", "200", "EventSource"),
            ("/asset.txt", "200", "static asset"),
            ("/missing/path", "404", "<p>Not Found"),
        ] {
            let response = http_get_with_header(&address, path, header)?;
            let (head, _) = response
                .split_once("\r\n\r\n")
                .with_context(|| format!("no header end: {response}"))?;
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status}"))
                    && response.contains(body)
                    && head
                        .to_ascii_lowercase()
                        .contains("\r\ncache-control: no-store\r\n"),
                "{path} with {header:?}: {response}"
            );
            // Only HTML responses take the reload script.
            assert_eq!(
                response.contains("EventSource"),
                path != "/asset.txt",
                "{path}: {response}"
            );
        }
    }
    Ok(())
}

#[test]
fn dev_reloads_after_an_article_is_edited() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (server, address) = DevProcess::start_listening(&site)?;
    let mut events = open_reload_stream(&address, &server)?;

    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "# Changed in dev\n"),
    )?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    let updated = http_get(&address, "/entries/hello-world/")?;
    assert!(updated.contains("Changed in dev"), "{updated}");
    Ok(())
}

#[test]
fn dev_keeps_serving_after_failed_rebuild_and_reloads_after_fix() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (server, address) = DevProcess::start_listening(&site)?;
    let mut events = open_reload_stream(&address, &server)?;
    let before = http_body(&address, "/")?;

    write_file(
        site.join("content/entries/hello-world/index.md"),
        "+++\ntitle = [\n+++\n",
    )?;
    assert_sse_silent(&mut events, Duration::from_millis(500), &server)?;
    server.wait_for_log("Rebuild failed")?;
    assert_eq!(http_body(&address, "/")?, before);

    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "# Fixed in dev\n"),
    )?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    let fixed = http_get(&address, "/entries/hello-world/")?;
    assert!(fixed.contains("Fixed in dev"), "{fixed}");
    Ok(())
}

#[test]
fn dev_keeps_the_last_output_when_raw_html_is_added() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let (server, address) = DevProcess::start_listening(&site)?;
    let mut events = open_reload_stream(&address, &server)?;
    let before = http_body(&address, "/entries/hello-world/")?;

    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "Text\n\n<div>raw</div>\n"),
    )?;
    assert_sse_silent(&mut events, Duration::from_millis(500), &server)?;
    server.wait_for_log("raw HTML is not allowed in Markdown at line 8")?;
    assert_eq!(http_body(&address, "/entries/hello-world/")?, before);
    assert!(!site.join("dist").exists());
    Ok(())
}

#[test]
fn dev_reloads_after_an_article_is_removed() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/about/index.md"),
        article_source(&[], "# About page\n"),
    )?;
    let (server, address) = DevProcess::start_listening(&site)?;
    assert!(http_get(&address, "/about/")?.contains("About page"));
    let mut events = open_reload_stream(&address, &server)?;

    fs::remove_file(site.join("content/about/index.md"))?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    assert_not_found_page(&address, "/about/")?;
    Ok(())
}

#[test]
fn build_and_dry_run_ignore_drafts_even_when_they_are_broken() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(site.join("drafts/entries/wip/index.md"), "+++\ntitle = [\n")?;
    write_file(site.join("drafts/entries/wip/photo.png"), "not an image")?;
    write_file(site.join("drafts/notes.txt"), "outside any page")?;
    assert!(dry_run_site(&site)?.status.success());
    build_ok(&site)?;
    assert!(!site.join("dist/entries/wip").exists());
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(!home.contains("/entries/wip/"), "{home}");
    assert!(!home.contains("class=draft-badge"), "{home}");

    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "[draft](../wip/)\n"),
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("broken internal link in content/entries/hello-world/index.md: ../wip/"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn dev_shows_drafts_with_marks_and_leaves_dist_unchanged() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    write_file(
        site.join("drafts/entries/wip/index.md"),
        article_source(
            &[
                ("title", Some("'Work in progress'")),
                ("created_at", Some("2026-09-20 00:00")),
                ("updated_at", Some("2026-09-20 00:00")),
                ("tags", Some("['draft-only']")),
            ],
            "![photo](photo.png)\n\n[published](../hello-world/)\n",
        ),
    )?;
    fs::write(
        site.join("drafts/entries/wip/photo.png"),
        include_bytes!("fixtures/images/basic.png"),
    )?;
    let (server, address) = DevProcess::start_listening(&site)?;

    let home = http_body(&address, "/")?;
    assert!(home.contains("href=/entries/wip/"), "{home}");
    assert_eq!(home.matches("class=draft-badge").count(), 1, "{home}");
    assert!(home.contains(".draft-badge{"), "{home}");
    let draft = http_body(&address, "/entries/wip/")?;
    assert!(draft.contains("class=draft-badge"), "{draft}");
    assert!(draft.contains("width=300"), "{draft}");
    image_version(&draft, "photo.png?v=")?;
    assert!(http_get(&address, "/entries/wip/photo.png")?.starts_with("HTTP/1.1 200"));
    assert!(http_body(&address, "/tags/draft-only/")?.contains("href=/entries/wip/"));
    let published = http_body(&address, "/entries/hello-world/")?;
    assert!(!published.contains("class=draft-badge"), "{published}");

    let mut events = open_reload_stream(&address, &server)?;
    write_file(
        site.join("drafts/entries/wip/index.md"),
        article_source(&[], "# Edited draft\n"),
    )?;
    read_sse_until(&mut events, &mut Vec::new(), b"data: reload", &server)?;
    assert!(http_body(&address, "/entries/wip/")?.contains("Edited draft"));
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn views_are_checked_against_their_variables_before_any_page_renders() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    // Only drafts take this branch, and the site has none, so rendering alone would never meet it.
    write_file(
        site.join("views/components/draft-badge.bv"),
        "fn draft-badge(entry) =>\n  if entry.draft then span({class: \"draft-badge\"}, entry.titel) else []\n",
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("views/pages/root.bv does not fit the variables genbit passes"),
        "{stderr}"
    );
    assert!(
        stderr.contains("views/components/draft-badge.bv:2:58: unknown field \"titel\""),
        "{stderr}"
    );
    assert!(stderr.contains("in draft-badge"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);

    fs::write(
        site.join("views/components/draft-badge.bv"),
        "fn draft-badge(entry) =>\n  if entry.draft then span({class: \"draft-badge\"}, \"draft\") else []\n",
    )?;
    fs::write(
        site.join("views/components/home-link.bv"),
        "fn home-link(ctx) => a({href: \"/\"}, ctx.site)\n",
    )?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("cannot be a child of <a>"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn render_errors_name_the_article_by_its_site_path() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("views/pages/page.bv"),
        "fn page(ctx) => html(body(a({href: concat(\"javascript:\", ctx.article.title)}, \"x\")))\n",
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains(
            "cannot render content/entries/hello-world/index.md with views/pages/page.bv"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("views/pages/page.bv:1:27: href has the URL scheme javascript:"),
        "{stderr}"
    );

    fs::create_dir_all(site.join("drafts/entries"))?;
    fs::rename(
        site.join("content/entries/hello-world"),
        site.join("drafts/entries/wip"),
    )?;
    let error = DevProcess::start_listening(&site)
        .err()
        .context("dev accepted a draft that cannot render")?;
    assert!(
        format!("{error:#}")
            .contains("cannot render drafts/entries/wip/index.md with views/pages/page.bv"),
        "{error:#}"
    );
    Ok(())
}

#[test]
fn dev_rejects_drafts_that_share_or_nest_with_published_pages() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for (draft, expected) in [
        (
            "drafts/entries/hello-world/index.md",
            "drafts/entries/hello-world/ is already published as content/entries/hello-world/",
        ),
        (
            "drafts/entries/hello-world/part/index.md",
            "the draft drafts/entries/hello-world/part/ and the page content/entries/hello-world/ would nest",
        ),
        (
            "drafts/entries/x.png",
            "drafts/entries/x.png is not in a page directory",
        ),
    ] {
        write_file(site.join(draft), article_source(&[], "Draft\n"))?;
        let error = DevProcess::start_listening(&site)
            .err()
            .with_context(|| format!("dev accepted {draft}"))?;
        assert!(
            format!("{error:#}").contains(expected),
            "{draft}: {error:#}"
        );
        fs::remove_dir_all(site.join("drafts/entries"))?;
        fs::create_dir(site.join("drafts/entries"))?;
    }
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
    assert!(!site.join("dist").exists());
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
    write_file(site.join("content/entries/hello-world/index.md"), "invalid")?;
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
    http_get_with_header(address, path, "")
}

/// Returns the body of a successful response, leaving out headers such as `Date`.
fn http_body(address: &str, path: &str) -> Result<String> {
    let response = http_get(address, path)?;
    ensure!(response.starts_with("HTTP/1.1 200"), "{response}");
    let (_, body) = response
        .split_once("\r\n\r\n")
        .with_context(|| format!("no header end: {response}"))?;
    Ok(body.to_owned())
}

/// `header` is a complete header line such as `If-None-Match: *\r\n`, or empty.
fn http_get_with_header(address: &str, path: &str, header: &str) -> Result<String> {
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{header}Connection: close\r\n\r\n")
            .as_bytes(),
    )?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    // Images are not UTF-8; their responses are only checked for the status line.
    Ok(String::from_utf8_lossy(&response).into_owned())
}

#[test]
fn bad_input_preserves_previous_output_and_rebuild_removes_stale_pages() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        "+++\ntitle = [\n+++\nInvalid",
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("content/entries/hello-world/index.md"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "# Working again\n"),
    )?;
    write_file(
        site.join("content/stale/index.md"),
        article_source(&[], "# Old\n"),
    )?;
    build_ok(&site)?;
    assert!(site.join("dist/stale/index.html").is_file());
    fs::remove_file(site.join("content/stale/index.md"))?;
    build_ok(&site)?;
    assert!(!site.join("dist/stale/index.html").exists());
    Ok(())
}

#[test]
fn control_characters_in_feed_text_fail_and_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (field, value) in [("title", "\"a\\u0001b\""), ("description", "\"a\\u001Fb\"")] {
        write_file(
            site.join("content/entries/hello-world/index.md"),
            article_source(&[(field, Some(value))], "# Body\n"),
        )?;
        let stderr = build_err(&site)?;
        assert!(
            stderr.contains("content/entries/hello-world/index.md")
                && stderr.contains(&format!("{field} must not contain control characters")),
            "{stderr}"
        );
        assert_eq!(snapshot(&site.join("dist"))?, before);
    }
    Ok(())
}

#[test]
fn invalid_article_dates_report_source_and_preserve_previous_output() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    let article = site.join("content/entries/hello-world/index.md");
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
            stderr.contains("content/entries/hello-world/index.md"),
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
    write_file(
        site.join("content/about/index.md"),
        article_source(&[], "# First\n"),
    )?;
    write_file(site.join("static/about/index.html"), "conflict")?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("output collision at about/index.html"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);
    Ok(())
}

#[test]
fn directory_case_collisions_fail_and_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    fs::create_dir_all(site.join("content/Docs"))?;
    write_file(
        site.join("content/Docs/post/index.md"),
        article_source(&[], "![image](/docs/image.svg)\n"),
    )?;
    fs::create_dir_all(site.join("static/docs"))?;
    fs::write(site.join("static/docs/image.svg"), "<svg/>")?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("output directory case collision")
            && stderr.contains("Docs")
            && stderr.contains("docs"),
        "{stderr}"
    );
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn rejects_file_directory_collisions_reserved_urls_and_nested_pages() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    let post = article_source(&[], "# Post\n");
    let post = post.as_str();

    for (files, reason) in [
        (
            &["content/foo/index.md", "static/foo"][..],
            "creates file foo, required as directory",
        ),
        (
            &["static/entries/hello-world"][..],
            "creates file entries/hello-world, required as directory",
        ),
        (&["content/__genbit/reload/index.md"][..], "reserved URL"),
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
            fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?,
            article
        );
    }

    write_file(site.join("content/foo/index.md"), post)?;
    write_file(site.join("content/foo/bar/index.md"), post)?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("content/foo/bar/index.md is in a subdirectory of the page content/foo"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
    fs::remove_dir_all(site.join("content/foo/bar"))?;
    write_file(site.join("content/foo-bar/index.md"), post)?;
    build_ok(&site)?;
    assert!(site.join("dist/foo/index.html").is_file());
    assert!(site.join("dist/foo-bar/index.html").is_file());
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
    let original_css = fs::read_to_string(site.join("views/components/document.css"))?;

    for relative in ["config.toml", "content", "views", "static"] {
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
        fs::read_to_string(site.join("views/components/document.css"))?,
        original_css
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn views_reject_file_and_directory_symlinks() -> Result<()> {
    use std::os::unix::fs::symlink;

    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;

    let css = site.join("views/pages/page.css");
    let outside_css = workspace.0.path().join("outside-page.css");
    fs::rename(&css, &outside_css)?;
    symlink(&outside_css, &css)?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("symlinks are not supported"), "{stderr}");
    assert!(stderr.contains("page.css"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    fs::remove_file(&css)?;
    fs::rename(&outside_css, &css)?;

    let components = site.join("views/components");
    let outside_dir = workspace.0.path().join("outside-components");
    fs::rename(&components, &outside_dir)?;
    symlink(&outside_dir, &components)?;
    let stderr = build_err(&site)?;
    assert!(stderr.contains("symlinks are not supported"), "{stderr}");
    assert!(stderr.contains("components"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn colon_paths_and_encoded_segments_work_in_images_and_dev() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::create_dir(site.join("static/files"))?;
    let png = include_bytes!("fixtures/images/basic.png");
    fs::write(site.join("static/files/a:b.png"), png)?;
    fs::write(site.join("static/files/図 a%2f.png"), png)?;
    fs::write(site.join("static/files/a:b.txt"), "colon asset")?;
    fs::write(site.join("static/files/図 a%2f.txt"), "encoded asset")?;
    write_file(
        site.join("content/about/index.md"),
        article_source(&[], "About page"),
    )?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[],
            concat!(
                "[about](%2e%2E/%2e%2e/about/?x=1#top) [home](.%2e/.%2e/) ",
                "[self](/entries/%68ello-world/) [web](HTTPS://example.com/a.md)\n\n",
                "[colon](/files/a:b.txt) [encoded](/files/%E5%9B%B3%20a%252f.txt)\n\n",
                "![colon](/files/a:b.png) ![relative](%2e%2e/%2e%2e/files/a:b.png?x=1#part) ",
                "![encoded](/files/%E5%9B%B3%20a%252f.png)\n",
            ),
        ),
    )?;
    assert!(dry_run_site(&site)?.status.success());
    assert!(!site.join("dist").exists());
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    let hash = image_version(&html, "/files/a:b.png?v=")?;
    assert_eq!(
        image_version(&html, "%2e%2e/%2e%2e/files/a:b.png?x=1")?,
        hash
    );
    assert_eq!(
        image_version(&html, "/files/%E5%9B%B3%20a%252f.png?v=")?,
        hash
    );
    assert_eq!(html.matches("width=300").count(), 3, "{html}");
    assert_eq!(html.matches("height=200").count(), 3, "{html}");
    assert!(
        html.contains("href=\"%2e%2E/%2e%2e/about/?x=1#top\""),
        "{html}"
    );
    assert!(html.contains("href=HTTPS://example.com/a.md"), "{html}");
    assert!(html.contains("target=_blank"), "{html}");

    let (_server, address) = DevProcess::start_listening(&site)?;
    for (path, expected) in [
        ("/about/?x=1", "About page"),
        ("/entries/%68ello-world/", "width=300"),
        ("/files/a:b.txt", "colon asset"),
        ("/files/%E5%9B%B3%20a%252f.txt", "encoded asset"),
    ] {
        let response = http_get(&address, path)?;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains(expected), "{response}");
    }
    Ok(())
}

#[test]
fn invalid_site_links_fail_build_and_dry_run_without_replacing_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (body, reason) in [
        ("[missing](/files/missing:name.txt)", "broken internal link"),
        ("![missing](./missing:name.png)", "broken internal link"),
        ("[encoded](/entries%2Fhello-world)", "path separators"),
        ("[encoded](/entries%2fhello-world)", "path separators"),
        ("[encoded](/entries%5Chello-world)", "path separators"),
        ("![encoded](/assets%2fsite/ogp.png)", "path separators"),
        ("![encoded](/assets%5csite/ogp.png)", "path separators"),
    ] {
        write_file(
            site.join("content/entries/hello-world/index.md"),
            article_source(&[], body),
        )?;
        let error = build_err(&site)?;
        assert!(error.contains(reason), "{body}: {error}");
        assert!(
            error.contains("content/entries/hello-world/index.md"),
            "{error}"
        );
        let dry_run = dry_run_site(&site)?;
        assert!(!dry_run.status.success(), "{body}");
        let dry_error = String::from_utf8(dry_run.stderr)?;
        assert!(dry_error.contains(reason), "{dry_error}");
        assert_eq!(snapshot(&site.join("dist"))?, before);
        assert_no_build_leftovers(&site)?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn static_backslash_names_fail_without_replacing_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for name in [r"a\b.txt", r"a\b/file.txt"] {
        let path = site.join("static").join(name);
        fs::create_dir_all(path.parent().context("missing static parent")?)?;
        fs::write(&path, "asset")?;
        write_file(
            site.join("content/entries/hello-world/index.md"),
            article_source(&[], &format!("[asset](/{})", name.replace('\\', "/"))),
        )?;
        let error = build_err(&site)?;
        assert!(error.contains("backslash"), "{error}");
        assert!(error.contains(name), "{error}");
        let dry_run = dry_run_site(&site)?;
        assert!(!dry_run.status.success());
        let error = String::from_utf8(dry_run.stderr)?;
        assert!(
            error.contains("backslash") && error.contains(name),
            "{error}"
        );
        assert_eq!(snapshot(&site.join("dist"))?, before);
        assert_no_build_leftovers(&site)?;
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn view_backslash_names_fail_without_replacing_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    let name = r"pages\page.bv";
    fs::write(site.join("views").join(name), "")?;
    let error = build_err(&site)?;
    assert!(
        error.contains("backslash") && error.contains(&format!("views/{name}")),
        "{error}"
    );
    assert_eq!(snapshot(&site.join("dist"))?, before);
    assert_no_build_leftovers(&site)?;
    Ok(())
}

#[test]
fn internal_links_to_generated_pages_and_static_files_build() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[],
            concat!(
                "[next](../next/#details) [abs](/entries/next/) [redirected](/entries/next) ",
                "[html](/entries/next/index.html?x=1)\n\n",
                "[about](../../about/) [home](/) [tags](/tags/) [tag](/tags/untagged) [feed](/feed.xml)\n\n",
                "![ogp](/assets/site/ogp.png) ![relative](../../assets/site/ogp.png) ",
                "![encoded](/files/%E7%94%BB%E5%83%8F.png) ![raw](/files/画像.png)\n\n",
                "[web](https://example.com/missing) [plain](http://example.com/missing) ",
                "[mail](mailto:someone@example.com) ",
                "[top](#top) <someone@example.com>\n\n",
                "## [unwrapped](missing/)\n",
            ),
        ),
    )?;
    write_file(
        site.join("content/entries/next/index.md"),
        article_source(&[], "## Details\n"),
    )?;
    write_file(
        site.join("content/about/index.md"),
        article_source(&[], "About\n"),
    )?;
    fs::create_dir(site.join("static/files"))?;
    fs::write(
        site.join("static/files/画像.png"),
        include_bytes!("fixtures/images/basic.png"),
    )?;
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    assert!(html.contains("href=../next/#details"), "{html}");
    assert!(html.contains("href=../../about/"), "{html}");
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
            "[missing](/entries/missing/)",
            "/entries/missing/ resolves to /entries/missing/",
        ),
        (
            "[markdown](../hello-world/index.md#x)",
            "../hello-world/index.md#x resolves to /entries/hello-world/index.md",
        ),
        (
            "![missing](/assets/img/missing.png)",
            "/assets/img/missing.png resolves to /assets/img/missing.png",
        ),
        (
            "![missing](../../missing.png)",
            "../../missing.png resolves to /missing.png",
        ),
        (
            "[case](/Entries/Hello-World/)",
            "/Entries/Hello-World/ resolves to /Entries/Hello-World/",
        ),
        (
            "[index](/entries/hello-world/index)",
            "/entries/hello-world/index resolves to /entries/hello-world/index",
        ),
    ] {
        write_file(
            site.join("content/entries/hello-world/index.md"),
            article_source(&[], body),
        )?;
        let stderr = build_err(&site)?;
        assert!(
            stderr.contains(&format!(
                "broken internal link in content/entries/hello-world/index.md: {expected}, which is not generated"
            )),
            "{stderr}"
        );
        assert_eq!(snapshot(&site.join("dist"))?, before);
    }
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "[bad](a%zz)"),
    )?;
    let stderr = build_err(&site)?;
    assert!(
        stderr.contains("invalid internal link in content/entries/hello-world/index.md: a%zz"),
        "{stderr}"
    );
    assert!(stderr.contains("invalid percent-encoding"), "{stderr}");
    assert_eq!(snapshot(&site.join("dist"))?, before);
    Ok(())
}

#[test]
fn links_with_unsafe_schemes_fail_and_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    for (body, target, scheme) in [
        (
            "[x](javascript:alert(1))",
            "link javascript:alert(1)",
            "javascript",
        ),
        (
            "[x](tel:+81-3-0000-0000)",
            "link tel:+81-3-0000-0000",
            "tel",
        ),
        (
            "![x](data:image/png;base64,AAAA)",
            "image data:image/png;base64,AAAA",
            "data",
        ),
    ] {
        write_file(
            site.join("content/entries/hello-world/index.md"),
            article_source(&[], body),
        )?;
        let stderr = build_err(&site)?;
        assert!(
            stderr.contains("cannot parse")
                && stderr.contains("content/entries/hello-world/index.md"),
            "{stderr}"
        );
        assert!(stderr.contains(&format!("invalid {target}")), "{stderr}");
        assert!(
            stderr.contains(&format!("has the URL scheme {scheme}:")),
            "{stderr}"
        );
        assert_eq!(snapshot(&site.join("dist"))?, before);
    }
    Ok(())
}

#[test]
fn raw_html_in_articles_fails_with_its_line_and_preserves_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    build_ok(&site)?;
    let before = snapshot(&site.join("dist"))?;
    let crlf = article_source(
        &[],
        "# 見出し\n\n本文 <kbd>Ctrl</kbd>\n\n<div>second</div>\n",
    )
    .replace('\n', "\r\n");
    for (source, line) in [
        (
            article_source(&[], "<details>\n<summary>More</summary>\n</details>\n"),
            6,
        ),
        (article_source(&[], "Text\n\nPress <kbd>Ctrl</kbd>\n"), 8),
        (article_source(&[], "Text\n\n<!-- draft -->\n"), 8),
        (article_source(&[], "## 見出し <span>x</span>\n"), 6),
        (
            article_source(&[], "段落\n\n![画像 <b>x</b>](/assets/site/ogp.png)\n"),
            8,
        ),
        (format!("\u{feff}{crlf}"), 8),
    ] {
        write_file(site.join("content/entries/hello-world/index.md"), &source)?;
        let stderr = build_err(&site)?;
        assert!(
            stderr.contains("content/entries/hello-world/index.md"),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!(
                "raw HTML is not allowed in Markdown at line {line};"
            )),
            "{source:?}: {stderr}"
        );
        let output = dry_run_site(&site)?;
        assert!(!output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stderr), stderr);
        assert_eq!(snapshot(&site.join("dist"))?, before);
        assert_no_build_leftovers(&site)?;
    }
    Ok(())
}

#[test]
fn articles_show_html_notation_as_text_and_keep_generated_markup() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(
            &[],
            concat!(
                "## `<details>` の使い方\n\n",
                "`<kbd>` と \\<br> と &lt;!-- memo --&gt;\n\n",
                "```html\n<picture></picture>\n```\n\n",
                "<https://example.com/a> <someone@example.com> [外部](https://example.com/b)\n\n",
                "![ロゴ](/assets/site/ogp.png)\n",
            ),
        ),
    )?;
    build_ok(&site)?;
    let html = fs::read_to_string(site.join("dist/entries/hello-world/index.html"))?;
    for expected in [
        "id=details-の使い方",
        "class=heading-anchor href=#details-の使い方",
        "<code>&lt;details></code>",
        "<code>&lt;kbd></code>",
        "&lt;br>",
        "&lt;!-- memo -->",
        "class=code-language>html</span>",
        "&lt;picture>&lt;/picture>",
        "href=https://example.com/a",
        "href=mailto:someone@example.com",
        "href=https://example.com/b",
        "target=_blank",
        "src=\"/assets/site/ogp.png?v=",
        "loading=lazy",
        "decoding=async",
    ] {
        assert!(html.contains(expected), "{expected}: {html}");
    }
    for raw in ["<details>", "<kbd>", "<br>", "<!-- memo", "<picture>"] {
        assert!(!html.contains(raw), "{raw}: {html}");
    }
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
    write_file(
        site.join("content/entries/hello-world/index.md"),
        article_source(&[], "# Changed\n"),
    )?;
    write_file(
        site.join("content/added/index.md"),
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
        (
            article_source(&[], "<div>raw</div>\n"),
            "raw HTML is not allowed in Markdown at line 6",
        ),
    ] {
        write_file(site.join("content/entries/hello-world/index.md"), body)?;
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
