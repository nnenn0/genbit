use anyhow::{Context, Result, bail, ensure};
use gray_matter::{Matter, engine::TOML};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    title: Option<String>,
    description: Option<String>,
    template: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Article {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) url: String,
    pub(crate) created_at: String,
    #[serde(skip)]
    pub(crate) created_at_order: String,
    pub(crate) updated_at: Option<String>,
    #[serde(skip)]
    pub(crate) template: String,
    #[serde(skip)]
    pub(crate) html: String,
    #[serde(skip)]
    pub(crate) output: PathBuf,
    #[serde(skip)]
    pub(crate) source: PathBuf,
}

pub(crate) fn parse(source: &str, relative: &Path) -> Result<Article> {
    let (metadata, body) = split_front_matter(source)?;
    let (url, output) = route(relative)?;
    let (created_at, created_at_order) = metadata
        .created_at
        .as_deref()
        .context("created_at is required for articles")
        .and_then(local_created_at)?;
    let updated_at = metadata
        .updated_at
        .as_deref()
        .map(|value| local_date(value, "updated_at"))
        .transpose()?;
    if let Some(updated) = &updated_at {
        ensure!(
            updated >= &created_at,
            "updated_at must not precede created_at"
        );
    }
    let title = metadata.title.unwrap_or_else(|| {
        relative
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Untitled")
            .to_owned()
    });
    ensure!(!title.trim().is_empty(), "title must not be empty");
    let description = metadata
        .description
        .as_deref()
        .context("description is required for articles")?
        .trim()
        .to_owned();
    ensure!(!description.is_empty(), "description must not be empty");
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
    Ok(Article {
        title,
        description,
        url,
        created_at,
        created_at_order,
        updated_at,
        template,
        html: rendered,
        output,
        source: relative.to_path_buf(),
    })
}

fn local_created_at(value: &str) -> Result<(String, String)> {
    const FORMAT: &str = "created_at must be a TOML local date or date-time without fractional seconds (YYYY-MM-DD, YYYY-MM-DD HH:MM, or YYYY-MM-DD HH:MM:SS)";
    let parsed = value.parse::<toml::value::Datetime>().context(FORMAT)?;
    ensure!(parsed.offset.is_none(), "{FORMAT}");
    let date = parsed.date.context(FORMAT)?.to_string();
    let order = if let Some(time) = parsed.time {
        ensure!(time.nanosecond.is_none(), "{FORMAT}");
        format!(
            "{date}T{:02}:{:02}:{:02}",
            time.hour,
            time.minute,
            time.second.unwrap_or(0)
        )
    } else {
        format!("{date}T00:00:00")
    };
    Ok((date, order))
}

fn local_date(value: &str, field: &str) -> Result<String> {
    let parsed = value
        .parse::<toml::value::Datetime>()
        .with_context(|| format!("{field} must be a TOML local date (YYYY-MM-DD)"))?;
    ensure!(
        parsed.date.is_some() && parsed.time.is_none() && parsed.offset.is_none(),
        "{field} must be a TOML local date (YYYY-MM-DD)"
    );
    Ok(parsed.to_string())
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
            let fields = source
                .get(first.len()..end - line.len())
                .context("invalid front matter fields boundary")?;
            let values: toml::Table =
                toml::from_str(fields).context("invalid TOML front matter")?;
            for field in ["created_at", "updated_at"] {
                if let Some(value) = values.get(field) {
                    let format = if field == "created_at" {
                        "created_at must be a TOML local date or date-time without fractional seconds (YYYY-MM-DD, YYYY-MM-DD HH:MM, or YYYY-MM-DD HH:MM:SS)"
                    } else {
                        "updated_at must be a TOML local date (YYYY-MM-DD)"
                    };
                    ensure!(matches!(value, toml::Value::Datetime(_)), "{format}");
                }
            }
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
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "content paths must use ASCII letters, numbers, hyphens or underscores"
            );
            Ok(segment.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    let output = segments.iter().collect::<PathBuf>().with_extension("html");
    let url = format!("/{}", segments.join("/"));
    Ok((url, output))
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
            ("root.md", "/root", "root.html"),
            ("about.md", "/about", "about.html"),
            ("entries/index.md", "/entries/index", "entries/index.html"),
            ("entries/hello.md", "/entries/hello", "entries/hello.html"),
        ] {
            let actual = route(Path::new(source))?;
            assert_eq!(actual, (url.to_owned(), PathBuf::from(output)));
        }
        for path in [
            "../escape.md",
            "/absolute.md",
            "a?b.md",
            "a\\b.md",
            ".md",
            "index.md",
        ] {
            assert!(route(Path::new(path)).is_err(), "accepted {path}");
        }
        Ok(())
    }

    #[test]
    fn validates_creation_time_and_update_date() -> Result<()> {
        for source in [
            "# Missing date",
            "+++\ncreated_at = \"2026-09-17\"\n+++\n# Quoted date",
            "+++\ncreated_at = 2026-09-17T10:00:00.5\n+++\n# Fractional seconds",
            "+++\ncreated_at = 2026-09-17T10:00:00Z\n+++\n# Offset",
            "+++\ncreated_at = 2026-09-17\nupdated_at = 2026-09-16\n+++\n# Reversed",
            "+++\ncreated_at = 2026-09-17\nupdated_at = 2026-09-17T10:00:00\n+++\n# Updated time",
            "+++\ncreated_at = 2026-09-17\ndescription = '  '\n+++\n# Empty description",
            "+++\ncreated_at = 2026-09-17\n+++\n# Missing description",
        ] {
            assert!(
                parse(source, Path::new("post.md")).is_err(),
                "accepted {source:?}"
            );
        }
        let article = parse(
            "+++\ncreated_at = 2026-09-17\ndescription = 'Post description'\nupdated_at = 2026-09-22\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at, "2026-09-17");
        assert_eq!(article.created_at_order, "2026-09-17T00:00:00");
        assert_eq!(article.updated_at.as_deref(), Some("2026-09-22"));
        let article = parse(
            "+++\ncreated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at, "2026-09-17");
        assert_eq!(article.created_at_order, "2026-09-17T10:30:00");
        let article = parse(
            "+++\ncreated_at = 2026-09-17T10:30:42\ndescription = 'Post description'\nupdated_at = 2026-09-17\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at, "2026-09-17");
        assert_eq!(article.created_at_order, "2026-09-17T10:30:42");
        Ok(())
    }
}
