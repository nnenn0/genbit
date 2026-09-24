use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

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

    pub(crate) fn from_request_path(path: &str) -> Option<Self> {
        let relative = path.strip_prefix('/')?;
        if relative.is_empty() {
            return None;
        }
        let segments = relative.split('/').collect::<Vec<_>>();
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

pub(crate) fn validate_served_urls<'a>(
    files: impl IntoIterator<Item = (&'a Path, &'a str)>,
) -> Result<()> {
    let mut claimed = BTreeMap::new();
    for (path, source) in files {
        for url in served_urls(path)? {
            let key = url.to_ascii_lowercase();
            ensure!(!is_reserved_url(&url), "reserved URL {url} from {source}");
            if let Some(previous) = claimed.insert(key, source) {
                bail!("URL collision at {url}: {previous} and {source}");
            }
        }
    }
    Ok(())
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
    use super::{Route, is_reserved_url, served_urls, validate_served_urls};
    use anyhow::{Context, Result};
    use std::path::Path;

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
            "", "/", "foo", "//foo", "/foo/", "/a//b", "/a.html", "/a?x=1", "/あ",
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
}
