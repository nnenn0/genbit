use anyhow::{Context, Result, bail, ensure};
use serde::{Serialize, Serializer};
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

impl Serialize for Route {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.url())
    }
}

#[cfg(test)]
mod tests {
    use super::Route;
    use anyhow::Result;
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
}
