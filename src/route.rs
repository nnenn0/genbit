use anyhow::{Context, Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub(crate) const FEED_URL: &str = "/feed.xml";
pub(crate) const TAGS_INDEX_URL: &str = "/tags/";
pub(crate) const UNTAGGED_TAG: &str = "untagged";

pub(crate) fn tag_url(tag: &str) -> String {
    format!("{TAGS_INDEX_URL}{tag}/")
}

pub(crate) struct Route {
    url: String,
    output: PathBuf,
}

impl Route {
    pub(crate) fn from_content_path(relative: &Path) -> Result<Self> {
        ensure!(
            relative.extension().is_some_and(|ext| ext == "md"),
            "expected a .md file"
        );
        ensure!(
            relative != Path::new("index.md"),
            "content/index.md is not supported; the home page is generated from config.toml and templates/root.html"
        );
        let stem = relative.with_extension("");
        let segments = stem
            .components()
            .map(|part| {
                let Component::Normal(segment) = part else {
                    bail!("invalid content path");
                };
                let segment = segment.to_str().context("content path must be UTF-8")?;
                ensure!(
                    valid_segment(segment),
                    "content paths must use ASCII letters, numbers, hyphens or underscores"
                );
                Ok(segment)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::from_segments(&segments))
    }

    /// Decodes each segment as link validation does, so every validated link is also served.
    pub(crate) fn from_request_path(path: &str) -> Option<Self> {
        let relative = path.strip_prefix('/')?;
        if relative.is_empty() {
            return None;
        }
        let segments = relative
            .split('/')
            .map(|segment| percent_decode(segment).ok())
            .collect::<Option<Vec<_>>>()?;
        let segments = segments.iter().map(String::as_str).collect::<Vec<_>>();
        segments
            .iter()
            .all(|segment| valid_segment(segment))
            .then(|| Self::from_segments(&segments))
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn output(&self) -> &Path {
        &self.output
    }

    fn from_segments(segments: &[&str]) -> Self {
        Self {
            url: format!("/{}", segments.join("/")),
            output: segments.iter().collect::<PathBuf>().with_extension("html"),
        }
    }
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Returns the served URLs with their original case: collisions ignore case, but hosts match links exactly.
pub(crate) fn validate_served_urls<'a>(
    files: impl IntoIterator<Item = (&'a Path, &'a str)>,
) -> Result<BTreeSet<String>> {
    let mut claimed = BTreeMap::new();
    let mut urls = BTreeSet::new();
    for (path, source) in files {
        for url in served_urls(path)? {
            let key = url.to_ascii_lowercase();
            ensure!(!is_reserved_url(&url), "reserved URL {url} from {source}");
            if let Some(previous) = claimed.insert(key, source) {
                bail!("URL collision at {url}: {previous} and {source}");
            }
            urls.insert(url);
        }
    }
    Ok(urls)
}

/// Returns `None` for links with a scheme or host, which leave the site.
pub(crate) fn split_site_link(link: &str) -> Option<(&str, &str)> {
    let boundary = link.find(['?', '#']).unwrap_or(link.len());
    let (path, suffix) = link.split_at(boundary);
    (!path.starts_with("//") && !path.contains(':')).then_some((path, suffix))
}

pub(crate) fn validate_links(
    page_url: &str,
    links: &[String],
    source: &Path,
    served: &BTreeSet<String>,
) -> Result<()> {
    for link in links {
        let target = resolve_link(page_url, link).with_context(|| {
            format!(
                "invalid internal link in content/{}: {link}",
                source.display()
            )
        })?;
        if let Some(target) = target {
            ensure!(
                served.contains(&target),
                "broken internal link in content/{}: {link} resolves to {target}, which is not generated",
                source.display()
            );
        }
    }
    Ok(())
}

/// Resolves a link the way a browser would from `page_url`, returning `None` for external and same-page links.
pub(crate) fn resolve_link(page_url: &str, link: &str) -> Result<Option<String>> {
    let Some((path, _)) = split_site_link(link) else {
        return Ok(None);
    };
    if path.is_empty() {
        return Ok(None);
    }
    let joined = if path.starts_with('/') {
        path.to_owned()
    } else {
        let directory = page_url.rfind('/').and_then(|end| page_url.get(..=end));
        format!("{}{path}", directory.unwrap_or("/"))
    };
    let raw = joined.split('/').skip(1).collect::<Vec<_>>();
    let mut segments = Vec::with_capacity(raw.len());
    for (index, segment) in raw.iter().enumerate() {
        let last = index + 1 == raw.len();
        match *segment {
            "." => {}
            ".." => {
                // A `..` at the root stays at the root, as in RFC 3986.
                segments.pop();
            }
            other => {
                segments.push(percent_decode(other)?);
                continue;
            }
        }
        if last {
            segments.push(String::new());
        }
    }
    Ok(Some(format!("/{}", segments.join("/"))))
}

fn percent_decode(segment: &str) -> Result<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let value = bytes
                .get(index + 1..index + 3)
                .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .context("invalid percent-encoding")?;
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    String::from_utf8(decoded).context("percent-encoded path is not UTF-8")
}

pub(crate) fn is_reserved_url(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "/__genbit" || lower.starts_with("/__genbit/")
}

fn served_urls(path: &Path) -> Result<Vec<String>> {
    let path = path
        .to_str()
        .context("output path must be UTF-8")?
        .replace('\\', "/");
    let mut urls = vec![format!("/{path}")];
    if let Some(stem) = path.strip_suffix(".html") {
        let clean = format!("/{stem}");
        if Route::from_request_path(&clean).is_some() {
            urls.push(clean);
        }
    }
    if path == "index.html" {
        urls.push("/".to_owned());
    } else if let Some(directory) = path.strip_suffix("/index.html") {
        urls.push(format!("/{directory}"));
        urls.push(format!("/{directory}/"));
    }
    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::{
        Route, is_reserved_url, resolve_link, served_urls, validate_links, validate_served_urls,
    };
    use anyhow::{Context, Result};
    use std::{collections::BTreeSet, path::Path};

    #[test]
    fn content_and_request_paths_share_the_same_route() -> Result<()> {
        for (source, url, output) in [
            ("root.md", "/root", "root.html"),
            ("about.md", "/about", "about.html"),
            ("entries/index.md", "/entries/index", "entries/index.html"),
            ("entries/hello.md", "/entries/hello", "entries/hello.html"),
        ] {
            let route = Route::from_content_path(Path::new(source))?;
            assert_eq!(route.url(), url);
            assert_eq!(route.output(), Path::new(output));
            let request = Route::from_request_path(url)
                .ok_or_else(|| anyhow::anyhow!("invalid test URL {url}"))?;
            assert_eq!(request.url(), route.url());
            assert_eq!(request.output(), route.output());
        }
        Ok(())
    }

    #[test]
    fn request_paths_are_percent_decoded_like_validated_links() -> Result<()> {
        let request = Route::from_request_path("/entries/%68ello-world")
            .context("rejected percent-encoded request path")?;
        assert_eq!(request.url(), "/entries/hello-world");
        assert_eq!(request.output(), Path::new("entries/hello-world.html"));
        assert_eq!(
            resolve_link("/", "/entries/%68ello-world")?.as_deref(),
            Some(request.url())
        );
        Ok(())
    }

    #[test]
    fn rejects_unsupported_content_and_request_paths() {
        for path in [
            "../escape.md",
            "/absolute.md",
            "a?b.md",
            "a\\b.md",
            ".md",
            "index.md",
        ] {
            assert!(
                Route::from_content_path(Path::new(path)).is_err(),
                "accepted {path}"
            );
        }
        for path in [
            "",
            "/",
            "foo",
            "//foo",
            "/foo/",
            "/a//b",
            "/a.html",
            "/a?x=1",
            "/あ",
            "/a%2Fb",
            "/a%2e",
            "/a%zz",
            "/%E3%81%82",
        ] {
            assert!(Route::from_request_path(path).is_none(), "accepted {path}");
        }
    }

    #[test]
    fn records_direct_clean_and_directory_index_urls() -> Result<()> {
        assert_eq!(
            served_urls(Path::new("index.html"))?,
            ["/index.html", "/index", "/"]
        );
        assert_eq!(
            served_urls(Path::new("posts/index.html"))?,
            ["/posts/index.html", "/posts/index", "/posts", "/posts/"]
        );
        assert_eq!(
            served_urls(Path::new("posts/file.txt"))?,
            ["/posts/file.txt"]
        );
        Ok(())
    }

    #[test]
    fn detects_served_url_collisions_and_reserved_paths() -> Result<()> {
        for (left, right, url) in [
            ("foo.html", "foo", "/foo"),
            ("foo.html", "foo/index.html", "/foo"),
            ("foo/index.html", "foo", "/foo"),
            ("index.html", "index", "/index"),
        ] {
            let error =
                validate_served_urls([(Path::new(left), "first"), (Path::new(right), "second")])
                    .err()
                    .context("accepted URL collision")?;
            assert!(error.to_string().contains(url), "{error:#}");
        }
        for path in [
            "__genbit",
            "__genbit/reload.html",
            "__genbit.html",
            "__genbit/asset.txt",
        ] {
            let error = validate_served_urls([(Path::new(path), "source")])
                .err()
                .context("accepted reserved URL")?;
            assert!(error.to_string().contains("reserved URL"), "{error:#}");
        }
        assert!(is_reserved_url("/__genbit"));
        assert!(is_reserved_url("/__GENBIT/reload"));
        assert!(!is_reserved_url("/__genbit-other"));
        validate_served_urls([
            (Path::new("foo.html"), "article"),
            (Path::new("foo/bar.html"), "nested article"),
        ])?;
        Ok(())
    }

    #[test]
    fn resolves_links_like_a_browser() -> Result<()> {
        for (link, expected) in [
            ("/entries/foo", "/entries/foo"),
            ("/entries/foo#section", "/entries/foo"),
            ("/entries/foo?view=full#section", "/entries/foo"),
            ("foo", "/entries/foo"),
            ("./foo", "/entries/foo"),
            ("../about", "/about"),
            ("sub/../foo", "/entries/foo"),
            ("../../../about", "/about"),
            ("/../about", "/about"),
            ("..", "/"),
            ("sub/.", "/entries/sub/"),
            ("/tags/rust/", "/tags/rust/"),
            ("%E7%94%BB%E5%83%8F.png", "/entries/画像.png"),
            ("画像.png", "/entries/画像.png"),
            ("a%20b.png", "/entries/a b.png"),
        ] {
            assert_eq!(
                resolve_link("/entries/a", link)?.as_deref(),
                Some(expected),
                "{link}"
            );
        }
        assert_eq!(resolve_link("/about", "foo")?.as_deref(), Some("/foo"));
        Ok(())
    }

    #[test]
    fn skips_external_and_same_page_links() -> Result<()> {
        for link in [
            "https://example.com/missing",
            "http://example.com/missing",
            "//example.com/missing",
            "mailto:someone@example.com",
            "tel:+81-3-0000-0000",
            "data:image/png;base64,AAAA",
            "#section",
            "?view=full",
            "",
        ] {
            assert_eq!(resolve_link("/entries/a", link)?, None, "{link}");
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_percent_encoding() {
        for link in ["%", "a%2", "a%zz", "a%+1", "%FF"] {
            assert!(resolve_link("/entries/a", link).is_err(), "accepted {link}");
        }
    }

    #[test]
    fn validates_links_against_served_urls_exactly() -> Result<()> {
        let served = validate_served_urls([
            (Path::new("entries/foo.html"), "article"),
            (Path::new("tags/rust/index.html"), "tag"),
            (Path::new("assets/Photo.png"), "static/assets/Photo.png"),
        ])?;
        let source = Path::new("entries/a.md");
        let valid = [
            "foo",
            "/entries/foo.html#x",
            "/tags/rust",
            "/tags/rust/",
            "../assets/Photo.png",
            "https://example.com/missing",
        ]
        .map(str::to_owned);
        validate_links("/entries/a", &valid, source, &served)?;
        for (link, target) in [
            ("missing", "/entries/missing"),
            ("foo/", "/entries/foo/"),
            ("/Entries/foo", "/Entries/foo"),
            ("/assets/photo.png", "/assets/photo.png"),
        ] {
            let error = validate_links("/entries/a", &[link.to_owned()], source, &served)
                .err()
                .context("accepted broken link")?;
            assert_eq!(
                error.to_string(),
                format!(
                    "broken internal link in content/entries/a.md: {link} resolves to {target}, which is not generated"
                )
            );
        }
        let error = validate_links("/entries/a", &["a%zz".to_owned()], source, &BTreeSet::new())
            .err()
            .context("accepted invalid link")?;
        assert!(
            format!("{error:#}").contains("invalid internal link in content/entries/a.md: a%zz"),
            "{error:#}"
        );
        Ok(())
    }
}
