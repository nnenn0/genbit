use crate::route::{Route, UNTAGGED_TAG};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Deserializer, de::Error as _};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use toml::value::{Date, Datetime};

const CREATED_AT_FORMAT: &str =
    "created_at must be a TOML local date-time with minute precision (YYYY-MM-DD HH:MM)";
const UPDATED_AT_FORMAT: &str =
    "updated_at must be a TOML local date-time with minute precision (YYYY-MM-DD HH:MM)";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    title: Option<String>,
    description: Option<String>,
    template: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_created_at")]
    created_at: Option<Datetime>,
    #[serde(default, deserialize_with = "deserialize_updated_at")]
    updated_at: Option<Datetime>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LocalDateTime {
    date: Date,
    minutes_of_day: u16,
}

impl LocalDateTime {
    fn from_datetime(value: Datetime, format: &'static str) -> Result<Self> {
        ensure!(value.offset.is_none(), "{format}");
        let date = value.date.context(format)?;
        let time = value.time.context(format)?;
        ensure!(
            time.second.is_none() && time.nanosecond.is_none(),
            "{format}"
        );
        let minutes_of_day = u16::from(time.hour) * 60 + u16::from(time.minute);
        Ok(Self {
            date,
            minutes_of_day,
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

fn deserialize_updated_at<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Datetime>, D::Error>
where
    D: Deserializer<'de>,
{
    Datetime::deserialize(deserializer)
        .map(Some)
        .map_err(|_| D::Error::custom(UPDATED_AT_FORMAT))
}

pub(crate) struct Article {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) route: Route,
    pub(crate) created_at: LocalDateTime,
    pub(crate) updated_at: LocalDateTime,
    pub(crate) template: String,
    pub(crate) tags: Vec<String>,
    pub(crate) html: String,
    pub(crate) source: PathBuf,
}

pub(crate) fn parse(source: &str, relative: &Path) -> Result<Article> {
    let (metadata, body) = split_front_matter(source)?;
    let route = Route::from_content_path(relative)?;
    let created_at = metadata
        .created_at
        .context("created_at is required for articles")
        .and_then(|value| LocalDateTime::from_datetime(value, CREATED_AT_FORMAT))?;
    let updated_at = metadata
        .updated_at
        .context("updated_at is required for articles")
        .and_then(|value| LocalDateTime::from_datetime(value, UPDATED_AT_FORMAT))?;
    ensure!(
        updated_at >= created_at,
        "updated_at must not precede created_at"
    );
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
    let mut seen_tags = BTreeSet::new();
    for tag in &metadata.tags {
        ensure!(
            valid_tag(tag),
            "invalid tag {tag:?}: use lowercase kebab-case"
        );
        ensure!(
            tag != UNTAGGED_TAG,
            "tag \"untagged\" is reserved for articles without tags"
        );
        ensure!(seen_tags.insert(tag), "duplicate tag {tag:?}");
    }
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
        route,
        created_at,
        updated_at,
        template,
        tags: metadata.tags,
        html: rendered,
        source: relative.to_path_buf(),
    })
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
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
    fn validates_creation_and_update_times() -> Result<()> {
        for (source, reason) in [
            (
                "+++\ndescription = 'Post description'\n+++\n# Missing date",
                "created_at is required",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Missing update",
                "updated_at is required",
            ),
            (
                "+++\ncreated_at = \"2026-09-17\"\ndescription = 'Post description'\n+++\n# Quoted date",
                "created_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17\ndescription = 'Post description'\n+++\n# Date only",
                "created_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:00:00\ndescription = 'Post description'\n+++\n# Seconds",
                "created_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17T10:00:00.5\ndescription = 'Post description'\n+++\n# Fractional seconds",
                "created_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17T10:00:00Z\ndescription = 'Post description'\n+++\n# Offset",
                "created_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-16 23:59\ndescription = 'Post description'\n+++\n# Reversed",
                "updated_at must not precede created_at",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-17 10:29\ndescription = 'Post description'\n+++\n# Earlier minute",
                "updated_at must not precede created_at",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-18\ndescription = 'Post description'\n+++\n# Update date only",
                "updated_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-18 10:00:00\ndescription = 'Post description'\n+++\n# Update seconds",
                "updated_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = \"2026-09-18 10:00\"\ndescription = 'Post description'\n+++\n# Quoted update",
                "updated_at must be a TOML local date-time",
            ),
            (
                "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = '  '\n+++\n# Empty description",
                "description must not be empty",
            ),
            (
                "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\n+++\n# Missing description",
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
            "+++\ncreated_at = 2026-09-17 00:00\ndescription = 'Post description'\nupdated_at = 2026-09-22 00:00\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at.date_string(), "2026-09-17");
        assert_eq!(article.created_at.minutes_of_day, 0);
        assert_eq!(article.updated_at.date_string(), "2026-09-22");
        assert_eq!(article.updated_at.minutes_of_day, 0);
        let article = parse(
            "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at.date_string(), "2026-09-17");
        assert_eq!(article.created_at.minutes_of_day, 10 * 60 + 30);
        let article = parse(
            "+++\ncreated_at = 2026-09-17T10:30\ndescription = 'Post description'\nupdated_at = 2026-09-17 10:31\n+++\n# Post",
            Path::new("post.md"),
        )?;
        assert_eq!(article.created_at.date_string(), "2026-09-17");
        assert_eq!(article.created_at.minutes_of_day, 10 * 60 + 30);
        assert_eq!(article.updated_at.minutes_of_day, 10 * 60 + 31);
        let midnight = parse(
            "+++\ncreated_at = 2026-09-17T00:00\nupdated_at = 2026-09-17T00:00\ndescription = 'Post description'\n+++\n# Post",
            Path::new("midnight.md"),
        )?;
        assert_eq!(midnight.created_at.minutes_of_day, 0);
        Ok(())
    }

    #[test]
    fn validates_tags() -> Result<()> {
        let source = "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\ntags = ['react', 'react-19', 'web-security']\n+++\nBody";
        let article = parse(source, Path::new("post.md"))?;
        assert_eq!(article.tags, ["react", "react-19", "web-security"]);
        let untagged = parse(
            "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\n+++\nBody",
            Path::new("post.md"),
        )?;
        assert!(untagged.tags.is_empty());
        for tags in [
            "['React']",
            "['Web Security']",
            "['web_security']",
            "['web--security']",
            "['-web']",
            "['web-']",
            "['セキュリティ']",
            "['']",
            "['react', 'react']",
            "['untagged']",
        ] {
            let source = format!(
                "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\ntags = {tags}\n+++\nBody"
            );
            assert!(
                parse(&source, Path::new("post.md")).is_err(),
                "accepted {tags}"
            );
        }
        Ok(())
    }
}
