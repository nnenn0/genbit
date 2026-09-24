use anyhow::{Context, Result, bail};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
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
}

impl Drop for DevProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        "styles/common.css",
        "styles/page.css",
        "styles/root.css",
        "static/assets/img/favicon.svg",
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
    let og_image = fs::read(root.join("static/assets/img/ogp.png"))?;
    assert!(og_image.starts_with(b"\x89PNG\r\n\x1a\n"));
    let width = og_image.get(16..20).context("missing PNG width")?;
    let height = og_image.get(20..24).context("missing PNG height")?;
    assert_eq!(u32::from_be_bytes(width.try_into()?), 1200);
    assert_eq!(u32::from_be_bytes(height.try_into()?), 630);
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
        "+++\ntitle = \"<Hello & world>\"\ncreated_at = 2026-09-17\ndescription = 'A post'\nupdated_at = 2026-09-22\n+++\n\n**A post**\n",
    )?;
    fs::create_dir(site.join("content/entries/posts"))?;
    fs::write(
        site.join("content/entries/posts/another.md"),
        "+++\ncreated_at = 2026-09-16\ndescription = 'Test article'\n+++\n# Another\n",
    )?;
    fs::write(site.join("static/logo.png"), [0, 1, 2, 255])?;
    let output = build_site(&site)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("<style>"));
    assert!(home.contains("prefers-color-scheme"), "{home}");
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
    assert!(article.contains("2026-09-17</time>"), "{article}");
    assert!(article.contains("<strong>A post</strong>"));
    assert!(article.contains("content=\"A post\""), "{article}");
    assert!(
        article.contains("content=article property=og:type"),
        "{article}"
    );
    assert!(article.contains("\"@type\":\"BlogPosting\""), "{article}");
    assert!(
        article.contains("\"datePublished\":\"2026-09-17\""),
        "{article}"
    );
    assert!(
        article.contains("\"dateModified\":\"2026-09-22\""),
        "{article}"
    );
    assert!(
        article.contains("\\u003cHello \\u0026 world\\u003e"),
        "{article}"
    );
    assert!(site.join("dist/entries/posts/another.html").is_file());
    assert_eq!(fs::read(site.join("dist/logo.png"))?, [0, 1, 2, 255]);
    assert!(fs::read_to_string(site.join("dist/assets/img/favicon.svg"))?.contains("<svg"));
    assert!(site.join("dist/assets/img/ogp.png").is_file());
    Ok(())
}

#[test]
fn article_description_uses_front_matter_instead_of_body() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Chosen summary with quotes & friends'\n+++\n# Heading\n\nShort [linked](other.md) intro.\n\n- Skip this item\n\nMore detail with \"quotes\" & friends.\n",
    )?;
    fs::write(
        site.join("content/override.md"),
        "+++\ncreated_at = 2026-09-18\ndescription = 'Chosen summary'\n+++\nBody text.\n",
    )?;
    let output = build_site(&site)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# Heading\n\n- List only\n",
    )?;
    let build = build_site(&site)?;
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    let article = fs::read_to_string(site.join("dist/entries/hello-world.html"))?;
    assert!(home.contains("name=description"), "{home}");
    assert!(article.contains("name=description"), "{article}");
    assert!(article.contains("Test article"), "{article}");

    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\n+++\n# Heading\n",
    )?;
    let missing_article_description = build_site(&site)?;
    assert!(!missing_article_description.status.success());
    assert!(
        String::from_utf8_lossy(&missing_article_description.stderr)
            .contains("description is required")
    );
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# Heading\n",
    )?;

    for invalid_config in [
        "title = 'Blog'\nsite_url = 'https://example.com/'\nog_image = '/assets/img/ogp.png'\n",
        "title = 'Blog'\ndescription = '  '\nsite_url = 'https://example.com/'\nog_image = '/assets/img/ogp.png'\n",
    ] {
        fs::write(site.join("config.toml"), invalid_config)?;
        let invalid = build_site(&site)?;
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("description"));
        assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, home);
    }
    Ok(())
}

#[test]
fn site_url_generates_matching_canonicals_and_sitemap() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let build = || build_site(&site);
    assert!(build()?.status.success());
    assert!(site.join("dist/sitemap.xml").exists());
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("rel=canonical"), "{home}");
    assert!(home.contains("http://127.0.0.1:3000/"), "{home}");

    fs::write(
        site.join("config.toml"),
        "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'https://example.com/'\nog_image = '/assets/img/ogp.png'\n",
    )?;
    fs::create_dir(site.join("content/entries/posts"))?;
    fs::write(
        site.join("content/entries/posts/another.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Another article'\n+++\nAnother article.\n",
    )?;
    let output = build()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    ] {
        assert!(sitemap.contains(&format!("<loc>{url}</loc>")), "{sitemap}");
    }
    assert_eq!(sitemap.matches("<url>").count(), 3);
    assert!(!sitemap.contains(".html"), "{sitemap}");
    Ok(())
}

#[test]
fn invalid_urls_and_sitemap_collision_preserve_dist() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let build = || build_site(&site);
    fs::write(
        site.join("config.toml"),
        "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'https://example.com/'\nog_image = '/assets/img/ogp.png'\n",
    )?;
    assert!(build()?.status.success());
    let original = fs::read_to_string(site.join("dist/sitemap.xml"))?;
    fs::write(
        site.join("config.toml"),
        "title = 'Blog'\ndescription = 'Blog articles'\nog_image = '/assets/img/ogp.png'\n",
    )?;
    let missing = build()?;
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("site_url"));
    assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    for invalid_url in [
        "example.com",
        "ftp://example.com/",
        "https://user@example.com/",
        "https://example.com/blog/",
        "https://example.com/?q=1",
        "https://example.com/#fragment",
    ] {
        fs::write(
            site.join("config.toml"),
            format!(
                "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = '{invalid_url}'\nog_image = '/assets/img/ogp.png'\n"
            ),
        )?;
        let output = build()?;
        assert!(!output.status.success(), "accepted {invalid_url}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("site_url"));
        assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    }
    fs::write(
        site.join("config.toml"),
        "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'https://example.com/'\n",
    )?;
    let missing_og_image = build()?;
    assert!(!missing_og_image.status.success());
    assert!(String::from_utf8_lossy(&missing_og_image.stderr).contains("og_image"));
    assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    for invalid_og_image in [
        "assets/img/ogp.png",
        "//example.com/ogp.png",
        "ftp://example.com/ogp.png",
        "https://user@example.com/ogp.png",
        "/assets/img/ogp.png#fragment",
    ] {
        fs::write(
            site.join("config.toml"),
            format!(
                "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'https://example.com/'\nog_image = '{invalid_og_image}'\n"
            ),
        )?;
        let output = build()?;
        assert!(!output.status.success(), "accepted {invalid_og_image}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("og_image"));
        assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    }
    fs::write(
        site.join("config.toml"),
        "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'https://example.com/'\nog_image = 'https://cdn.example.com/social/card.png'\n",
    )?;
    assert!(build()?.status.success());
    let home = fs::read_to_string(site.join("dist/index.html"))?;
    assert!(home.contains("https://cdn.example.com/social/card.png"));
    fs::write(
        site.join("config.toml"),
        "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'https://example.com/'\nog_image = '/assets/img/ogp.png'\n",
    )?;
    fs::write(site.join("static/sitemap.xml"), "conflict")?;
    let output = build()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("output collision"));
    assert_eq!(fs::read_to_string(site.join("dist/sitemap.xml"))?, original);
    Ok(())
}

#[test]
fn links_to_markdown_articles_use_clean_urls() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n[Next](next.md?view=full#details)\n\n[External](https://example.com/next.md)\n",
    )?;
    fs::write(
        site.join("content/entries/next.md"),
        "+++\ncreated_at = 2026-09-18\ndescription = 'Test article'\n+++\n# Next\n",
    )?;

    let build = build_site(&site)?;
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
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
        ("old", "2026-09-05"),
        ("new", "2026-09-17"),
        ("middle", "2026-09-16"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            format!(
                "+++\ntitle = \"{name}\"\ncreated_at = {date}\ndescription = 'Test article'\n+++\n# {name}\n"
            ),
        )?;
    }
    let output = build_site(&site)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
fn homepage_orders_same_day_articles_by_creation_time_but_shows_date() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    for (name, created_at) in [
        ("a-early", "2026-09-17 08:00"),
        ("b-same", "2026-09-17T08:00:40"),
        ("z-late", "2026-09-17T08:00:40"),
        ("m-legacy", "2026-09-17"),
    ] {
        fs::write(
            site.join(format!("content/entries/{name}.md")),
            format!(
                "+++\ncreated_at = {created_at}\ndescription = 'Test article'\n+++\n# {name}\n"
            ),
        )?;
    }
    let output = build_site(&site)?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    let legacy = home
        .find("/entries/m-legacy")
        .context("legacy article missing")?;
    assert!(same < late && late < early && early < legacy, "{home}");
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
    let site = workspace.new_site("blog")?;
    fs::write(site.join("styles/common.css"), "h1 { color: red; }\n")?;
    fs::remove_file(site.join("styles/page.css"))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# Hello\n\n![A & B](photo.png \"Photo\")\n\n```\n  keep spacing\n```\n",
    )?;
    let output = build_site(&site)?;
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
    let build = build_site(&site)?;
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
    let site = workspace.new_site("blog")?;
    fs::write(
        site.join("content/about.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# About page\n",
    )?;
    fs::write(site.join("static/asset.txt"), "static asset")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut server = DevProcess::start(&site, port)?;
    let address = format!("127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..50 {
        if TcpStream::connect(&address).is_ok() {
            ready = true;
            break;
        }
        assert!(
            server.child.try_wait()?.is_none(),
            "dev exited before listening:\n{}",
            server.logs()
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "dev did not start:\n{}", server.logs());

    let page = http_get(&address, "/")?;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("EventSource"), "{page}");
    assert!(http_get(&address, "/about?view=full")?.contains("About page"));
    assert!(http_get(&address, "/asset.txt")?.contains("static asset"));

    let mut events = TcpStream::connect(&address)?;
    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    events.write_all(
        b"GET /__genbit/reload HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
    )?;
    let mut data = Vec::new();
    let mut buffer = [0; 4096];
    read_sse_until(&mut events, &mut data, b"\r\n\r\n", &server)?;
    assert!(
        String::from_utf8_lossy(&data).contains("text/event-stream"),
        "missing SSE content type:\n{}",
        server.logs()
    );

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
    let mut failure_logged = false;
    for _ in 0..50 {
        if server.logs().contains("Rebuild failed") {
            failure_logged = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        failure_logged,
        "missing build failure in dev logs:\n{}",
        server.logs()
    );
    assert_eq!(fs::read_to_string(site.join("dist/index.html"))?, before);

    events.set_read_timeout(Some(Duration::from_secs(8)))?;
    fs::write(
        site.join("content/entries/hello-world.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# Changed in dev\n",
    )?;
    read_sse_until(&mut events, &mut data, b"data: reload", &server)?;
    let updated = http_get(&address, "/entries/hello-world")?;
    assert!(updated.contains("Changed in dev"), "{updated}");

    data.clear();
    fs::remove_file(site.join("content/about.md"))?;
    read_sse_until(&mut events, &mut data, b"data: reload", &server)?;
    assert!(http_get(&address, "/about")?.starts_with("HTTP/1.1 404"));

    let build = build_site(&site)?;
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
    let site = workspace.new_site("blog")?;
    let build = || build_site(&site);
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
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# Working again\n",
    )?;
    fs::write(
        site.join("content/stale.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# Old\n",
    )?;
    assert!(build()?.status.success());
    assert!(site.join("dist/stale.html").is_file());
    fs::remove_file(site.join("content/stale.md"))?;
    assert!(build()?.status.success());
    assert!(!site.join("dist/stale.html").exists());
    Ok(())
}

#[test]
fn invalid_article_dates_report_source_and_preserve_previous_output() -> Result<()> {
    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    assert!(build_site(&site)?.status.success());
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    let article = site.join("content/entries/hello-world.md");
    for (dates, reason) in [
        (
            "created_at = \"2026-09-17\"",
            "created_at must be a TOML local date",
        ),
        (
            "created_at = 2026-09-17T10:00:00Z",
            "created_at must be a TOML local date",
        ),
        (
            "created_at = 2026-09-17T10:00:00.5",
            "created_at must be a TOML local date",
        ),
        (
            "created_at = 2026-09-17\nupdated_at = 2026-09-16",
            "updated_at must not precede created_at",
        ),
    ] {
        fs::write(
            &article,
            format!("+++\n{dates}\ndescription = 'Test article'\n+++\n# Post\n"),
        )?;
        let failed = build_site(&site)?;
        assert!(!failed.status.success(), "accepted {dates}");
        let stderr = String::from_utf8_lossy(&failed.stderr);
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
    let build = || build_site(&site);
    assert!(build()?.status.success());
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    fs::write(
        site.join("content/about.md"),
        "+++\ncreated_at = 2026-09-17\ndescription = 'Test article'\n+++\n# First\n",
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
    let site = workspace.new_site("blog")?;
    let build = || build_site(&site);

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

#[cfg(unix)]
#[test]
fn rejects_symlinks_in_site_inputs_without_touching_dist() -> Result<()> {
    use std::os::unix::fs::symlink;

    let workspace = Workspace::new()?;
    let site = workspace.new_site("blog")?;
    let built = build_site(&site)?;
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let before = fs::read_to_string(site.join("dist/index.html"))?;
    let original_css = fs::read_to_string(site.join("styles/common.css"))?;

    for relative in ["config.toml", "content", "templates", "styles", "static"] {
        let input = site.join(relative);
        let outside = workspace.0.path().join(format!("outside-{relative}"));
        fs::rename(&input, &outside)?;
        symlink(&outside, &input)?;

        let failed = build_site(&site)?;
        let stderr = String::from_utf8_lossy(&failed.stderr);
        assert!(!failed.status.success(), "accepted {relative}");
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
        "+++\ncreated_at = 2026-09-17\ndescription = 'Nested article'\ntemplate = 'deep/page.html'\n+++\n# Deep\n",
    )?;

    let built = build_site(&site)?;
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let before = fs::read_to_string(site.join("dist/deep.html"))?;
    assert!(before.contains("Nested template") && before.contains("--nested-css"));

    let parent = site.join("styles/deep");
    let outside_dir = workspace.0.path().join("outside-deep");
    fs::rename(&parent, &outside_dir)?;
    symlink(&outside_dir, &parent)?;
    let failed = build_site(&site)?;
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        !failed.status.success() && stderr.contains("symlinks are not supported"),
        "{stderr}"
    );
    assert!(stderr.contains("styles/deep"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/deep.html"))?, before);
    fs::remove_file(&parent)?;
    fs::rename(&outside_dir, &parent)?;

    let css = parent.join("page.css");
    let outside_css = workspace.0.path().join("outside-page.css");
    fs::rename(&css, &outside_css)?;
    symlink(&outside_css, &css)?;
    let failed = build_site(&site)?;
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        !failed.status.success() && stderr.contains("symlinks are not supported"),
        "{stderr}"
    );
    assert!(stderr.contains("page.css"), "{stderr}");
    assert_eq!(fs::read_to_string(site.join("dist/deep.html"))?, before);
    assert_eq!(
        fs::read_to_string(&outside_css)?,
        "body { --nested-css: yes; }"
    );
    Ok(())
}
