use anyhow::{Context, Result, bail, ensure};
use gray_matter::{Matter, engine::TOML};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    title: Option<String>,
    template: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Page {
    pub(crate) title: String,
    pub(crate) url: String,
    #[serde(skip)]
    pub(crate) template: String,
    #[serde(skip)]
    pub(crate) html: String,
    #[serde(skip)]
    pub(crate) output: PathBuf,
    #[serde(skip)]
    pub(crate) source: PathBuf,
}

pub(crate) fn parse(source: &str, relative: &Path) -> Result<Page> {
    let (metadata, body) = split_front_matter(source)?;
    let (url, output) = route(relative)?;
    let title = metadata.title.unwrap_or_else(|| {
        relative
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Untitled")
            .to_owned()
    });
    ensure!(!title.trim().is_empty(), "title must not be empty");
    let template = metadata.template.unwrap_or_else(|| "page.html".to_owned());
    ensure!(
        !template.is_empty()
            && Path::new(&template)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            && !template.contains('\\'),
        "template must be a relative path inside templates/"
    );
    let rendered = crate::markdown::render(body);
    Ok(Page {
        title,
        url,
        template,
        html: rendered,
        output,
        source: relative.to_path_buf(),
    })
}

fn split_front_matter(source: &str) -> Result<(FrontMatter, &str)> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok((FrontMatter::default(), source));
    };
    if first.trim_end() != "+++" {
        return Ok((FrontMatter::default(), source));
    }
    let mut end = first.len();
    for line in lines {
        end += line.len();
        if line.trim_end() == "+++" {
            let header = source.get(..end).context("invalid front matter boundary")?;
            let body = source.get(end..).context("invalid body boundary")?;
            let mut matter = Matter::<TOML>::new();
            "+++".clone_into(&mut matter.delimiter);
            // Only parse the header: gray_matter normalizes body whitespace and short inputs.
            let parsed = matter
                .parse::<FrontMatter>(header)
                .context("invalid TOML front matter")?;
            return Ok((parsed.data.unwrap_or_default(), body));
        }
    }
    bail!("front matter is missing closing +++ delimiter")
}

fn route(relative: &Path) -> Result<(String, PathBuf)> {
    ensure!(
        relative.extension().is_some_and(|ext| ext == "md"),
        "expected a .md file"
    );
    let stem = relative.with_extension("");
    let mut segments = stem
        .components()
        .map(|part| {
            let Component::Normal(segment) = part else {
                bail!("invalid content path");
            };
            let segment = segment.to_str().context("content path must be UTF-8")?;
            ensure!(
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "content paths must use ASCII letters, numbers, hyphens or underscores"
            );
            Ok(segment.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    if segments.last().is_some_and(|segment| segment == "index") {
        segments.pop();
    }
    let url = if segments.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}/", segments.join("/"))
    };
    let output = segments.iter().collect::<PathBuf>().join("index.html");
    Ok((url, output))
}

pub(crate) fn home(title: &str) -> Page {
    Page {
        title: title.to_owned(),
        url: "/".to_owned(),
        template: "page.html".to_owned(),
        html: String::new(),
        output: PathBuf::from("index.html"),
        source: PathBuf::from("<generated home>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_plain_and_front_matter_bodies() -> Result<()> {
        for body in [
            "",
            "a",
            "あ",
            "    indented\n",
            "line  \nnext\n",
            "\n\n---\n",
            "```rust\nlet x = 1;\n```\n",
        ] {
            assert_eq!(split_front_matter(body)?.1, body);
            let source = format!("+++\ntitle = \"Test\"\n+++\n{body}");
            assert_eq!(split_front_matter(&source)?.1, body);
        }
        let (_, body) = split_front_matter("\u{feff}+++\r\ntitle = \"Test\"\r\n+++\r\nbody\r\n")?;
        assert_eq!(body, "body\r\n");
        Ok(())
    }

    #[test]
    fn rejects_malformed_metadata() {
        for source in [
            "+++",
            "+++\ntitle = 'test'",
            "+++\ntitle = [\n+++\n",
            "+++\ntitle = 1\n+++\n",
            "+++\ntitel = 'typo'\n+++\n",
        ] {
            assert!(split_front_matter(source).is_err(), "accepted {source:?}");
        }
    }

    #[test]
    fn maps_clean_urls_and_rejects_unsafe_paths() -> Result<()> {
        for (source, url, output) in [
            ("index.md", "/", "index.html"),
            ("about.md", "/about/", "about/index.html"),
            ("posts/index.md", "/posts/", "posts/index.html"),
            ("posts/hello.md", "/posts/hello/", "posts/hello/index.html"),
        ] {
            let actual = route(Path::new(source))?;
            assert_eq!(actual, (url.to_owned(), PathBuf::from(output)));
        }
        for path in ["../escape.md", "/absolute.md", "a?b.md", "a\\b.md", ".md"] {
            assert!(route(Path::new(path)).is_err(), "accepted {path}");
        }
        Ok(())
    }
}
