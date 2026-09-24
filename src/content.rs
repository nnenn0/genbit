use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::path::{Component, Path, PathBuf};
use toml::value::{Date, Datetime};

const CREATED_AT_FORMAT: &str = "created_at must be a TOML local date or date-time without fractional seconds (YYYY-MM-DD, YYYY-MM-DD HH:MM, or YYYY-MM-DD HH:MM:SS)";
const UPDATED_AT_FORMAT: &str = "updated_at must be a TOML local date (YYYY-MM-DD)";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    title: Option<String>,
    description: Option<String>,
    template: Option<String>,
    #[serde(default, deserialize_with = "deserialize_created_at")]
    created_at: Option<Datetime>,
    #[serde(default, deserialize_with = "deserialize_updated_at")]
    updated_at: Option<Date>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CreatedAt {
    date: Date,
    seconds_of_day: u32,
}

impl CreatedAt {
    fn from_datetime(value: Datetime) -> Result<Self> {
        ensure!(value.offset.is_none(), "{CREATED_AT_FORMAT}");
        let date = value.date.context(CREATED_AT_FORMAT)?;
        let seconds_of_day = if let Some(time) = value.time {
            ensure!(time.nanosecond.is_none(), "{CREATED_AT_FORMAT}");
            u32::from(time.hour) * 3600
                + u32::from(time.minute) * 60
                + u32::from(time.second.unwrap_or(0))
        } else {
            0
        };
        Ok(Self {
            date,
            seconds_of_day,
        })
    }

    pub(crate) fn date_string(self) -> String {
        self.date.to_string()
    }
}

fn deserialize_created_at<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Datetime>, D::Error>
where
    D: Deserializer<'de>,
{
    Datetime::deserialize(deserializer)
        .map(Some)
        .map_err(|_| D::Error::custom(CREATED_AT_FORMAT))
}

fn deserialize_updated_at<'de, D>(deserializer: D) -> std::result::Result<Option<Date>, D::Error>
where
    D: Deserializer<'de>,
{
    Date::deserialize(deserializer)
        .map(Some)
        .map_err(|_| D::Error::custom(UPDATED_AT_FORMAT))
}

// Serde's serialize_with callback requires a reference to the field.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn serialize_created_date<S>(
    created_at: &CreatedAt,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&created_at.date_string())
}

// Serde's serialize_with callback requires a reference to the field.
#[allow(clippy::ref_option, clippy::trivially_copy_pass_by_ref)]
fn serialize_updated_date<S>(
    updated_at: &Option<Date>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    updated_at
        .map(|date| date.to_string())
        .serialize(serializer)
}

#[derive(Serialize)]
pub(crate) struct Article {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) url: String,
    #[serde(serialize_with = "serialize_created_date")]
    pub(crate) created_at: CreatedAt,
    #[serde(serialize_with = "serialize_updated_date")]
    pub(crate) updated_at: Option<Date>,
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
    let created_at = metadata
        .created_at
        .context("created_at is required for articles")
        .and_then(CreatedAt::from_datetime)?;
    let updated_at = metadata.updated_at;
    if let Some(updated) = updated_at {
        ensure!(
            updated >= created_at.date,
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
        updated_at,
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
            let fields = source
                .get(first.len()..end - line.len())
                .context("invalid front matter fields boundary")?;
            let body = source.get(end..).context("invalid body boundary")?;
            let metadata: FrontMatter =
                toml::from_str(fields).context("invalid TOML front matter")?;
            return Ok((metadata, body));
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
        for (source, reason) in [
            (
                "+++\ndescription = 'Post description'\n+++\n# Missing date",
                "created_at is required",
            ),
            (
                "+++\ncreated_at = \"2026-09-17\"\ndescription = 'Post description'\n+++\n# Quoted date",
                "created_at must be a TOML local date",
            ),
            (
                "+++\ncreated_at = 2026-09-17T10:00:00.5\ndescription = 'Post description'\n+++\n# Fractional seconds",
                "created_at must be a TOML local date",
            ),
            (
                "+++\ncreated_at = 2026-09-17T10:00:00Z\ndescription = 'Post description'\n+++\n# Offset",
                "created_at must be a TOML local date",
            ),
            (
                "+++\ncreated_at = 2026-09-17\nupdated_at = 2026-09-16\ndescription = 'Post description'\n+++\n# Reversed",
                "updated_at must not precede created_at",
            ),
            (
                "+++\ncreated_at = 2026-09-17\nupdated_at = 2026-09-17T10:00:00\ndescription = 'Post description'\n+++\n# Updated time",
                "updated_at must be a TOML local date",
            ),
            (
                "+++\ncreated_at = 2026-09-17\nupdated_at = \"2026-09-18\"\ndescription = 'Post description'\n+++\n# Quoted update",
                "updated_at must be a TOML local date",
            ),
            (
                "+++\ncreated_at = 2026-09-17\ndescription = '  '\n+++\n# Empty description",
                "description must not be empty",
            ),
            (
                "+++\ncreated_at = 2026-09-17\n+++\n# Missing description",
                "description is required",
            ),
        ] {
            let error = parse(source, Path::new("post.md"))
                .err()
                .context(format!("accepted {source:?}"))?;
            assert!(
                format!("{error:#}").contains(reason),
                "{source:?}: expected {reason:?}, got {error:#}"
            );
        }
        let article = parse(
            "+++\ncreated_at = 2026-09-17\ndescription = 'Post description'\nupdated_at = 2026-09-22\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at.date_string(), "2026-09-17");
        assert_eq!(article.created_at.seconds_of_day, 0);
        assert_eq!(
            article.updated_at.map(|date| date.to_string()).as_deref(),
            Some("2026-09-22")
        );
        let article = parse(
            "+++\ncreated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at.date_string(), "2026-09-17");
        assert_eq!(article.created_at.seconds_of_day, 10 * 3600 + 30 * 60);
        let article = parse(
            "+++\ncreated_at = 2026-09-17T10:30:42\ndescription = 'Post description'\nupdated_at = 2026-09-17\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at.date_string(), "2026-09-17");
        assert_eq!(article.created_at.seconds_of_day, 10 * 3600 + 30 * 60 + 42);
        let midnight = parse(
            "+++\ncreated_at = 2026-09-17T00:00:00\ndescription = 'Post description'\n+++\n# Post",
            Path::new("midnight.md"),
        )?;
        let date_only = parse(
            "+++\ncreated_at = 2026-09-17\ndescription = 'Post description'\n+++\n# Post",
            Path::new("date-only.md"),
        )?;
        assert_eq!(midnight.created_at, date_only.created_at);
        Ok(())
    }
}
