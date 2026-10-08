use crate::{
    images::Image,
    input::Tree,
    route::Route,
    tags::{Tag, Tags},
    text::PublishableText,
};
use anyhow::{Context, Result, bail, ensure};
use jiff::{Zoned, civil::DateTime, tz::TimeZone};
use serde::{Deserialize, Deserializer, de::Error as _};
use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};
use toml::value::{Datetime, Value};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Tags,
    #[serde(default, deserialize_with = "created_at")]
    created_at: Option<DateTime>,
    #[serde(default, deserialize_with = "updated_at")]
    updated_at: Option<DateTime>,
}

/// Front matter completed from the file name and placed in the site's time zone.
struct ArticleMeta {
    title: PublishableText,
    description: PublishableText,
    tags: Vec<Tag>,
    created_at: Zoned,
    updated_at: Zoned,
}

impl FrontMatter {
    fn resolve(self, relative: &Path, timezone: &TimeZone) -> Result<ArticleMeta> {
        let created_at = zoned(self.created_at, "created_at", timezone)?;
        let updated_at = zoned(self.updated_at, "updated_at", timezone)?;
        ensure!(
            updated_at >= created_at,
            "updated_at must not precede created_at"
        );
        let title = self.title.unwrap_or_else(|| {
            relative
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled")
                .to_owned()
        });
        let title = PublishableText::new(title, "title")?;
        let description = self
            .description
            .as_deref()
            .context("description is required for articles")?
            .trim()
            .to_owned();
        let description = PublishableText::new(description, "description")?;
        Ok(ArticleMeta {
            title,
            description,
            tags: self.tags.into_vec(),
            created_at,
            updated_at,
        })
    }
}

fn created_at<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DateTime>, D::Error> {
    local_minute(deserializer, "created_at").map(Some)
}

fn updated_at<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DateTime>, D::Error> {
    local_minute(deserializer, "updated_at").map(Some)
}

/// Reads a TOML local date-time with minute precision, such as `2026-09-17 10:30`.
fn local_minute<'de, D: Deserializer<'de>>(
    deserializer: D,
    field: &str,
) -> Result<DateTime, D::Error> {
    let invalid = || {
        D::Error::custom(format!(
            "{field} must be a TOML local date-time with minute precision (YYYY-MM-DD HH:MM)"
        ))
    };
    let Value::Datetime(Datetime {
        date: Some(date),
        time: Some(time),
        offset: None,
    }) = Value::deserialize(deserializer)?
    else {
        return Err(invalid());
    };
    if time.second.is_some() || time.nanosecond.is_some() {
        return Err(invalid());
    }
    let civil = || -> Result<DateTime> {
        Ok(DateTime::new(
            i16::try_from(date.year)?,
            i8::try_from(date.month)?,
            i8::try_from(date.day)?,
            i8::try_from(time.hour)?,
            i8::try_from(time.minute)?,
            0,
            0,
        )?)
    };
    civil().map_err(D::Error::custom)
}

fn zoned(value: Option<DateTime>, field: &str, timezone: &TimeZone) -> Result<Zoned> {
    value
        .with_context(|| format!("{field} is required for articles"))?
        .to_zoned(timezone.clone())
        .map_err(Into::into)
}

/// Formats a date-time for JSON-LD, sitemaps, and views, e.g. `2026-09-23T09:30:00+09:00`.
pub(crate) fn rfc3339(value: &Zoned) -> String {
    value.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

pub(crate) struct Article {
    pub(crate) title: PublishableText,
    pub(crate) description: PublishableText,
    pub(crate) route: Route,
    pub(crate) created_at: Zoned,
    pub(crate) updated_at: Zoned,
    pub(crate) tags: Vec<Tag>,
    pub(crate) content: bitview::Html,
    pub(crate) links: Vec<String>,
    /// The `index.md`, relative to the site root.
    pub(crate) source: PathBuf,
    /// Whether the page comes from `drafts/`, which only `genbit dev` builds.
    pub(crate) draft: bool,
}

/// The directory that holds pages. Only `genbit dev` reads `drafts/`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentDir {
    Content,
    Drafts,
}

impl ContentDir {
    pub(crate) fn path(self) -> &'static Path {
        Path::new(match self {
            Self::Content => "content",
            Self::Drafts => "drafts",
        })
    }
}

impl fmt::Display for ContentDir {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path().display().fmt(formatter)
    }
}

/// The files of `content/` or `drafts/`, as paths relative to that directory.
#[derive(Default)]
pub(crate) struct ContentFiles {
    /// The `index.md` of each page.
    pub(crate) pages: Vec<PathBuf>,
    /// The other files, each directly in a page directory, copied unchanged.
    pub(crate) assets: Vec<PathBuf>,
}

/// Classifies the files of `directory` into pages and their assets. A page is a directory that
/// holds `index.md`; it holds files only, so pages never nest.
pub(crate) fn classify_files(tree: Tree, directory: ContentDir) -> Result<ContentFiles> {
    let Tree { files, directories } = tree;
    ensure!(
        !files.iter().any(|file| file == Path::new("index.md")),
        "{directory}/index.md is not supported; the home page is generated from config.toml and views/pages/root.bv"
    );
    let page_directories = files
        .iter()
        .filter(|file| is_page_source(file))
        .filter_map(|file| file.parent())
        .map(Path::to_path_buf)
        .collect::<BTreeSet<_>>();
    for file in &files {
        let parent = file.parent().unwrap_or(Path::new(""));
        if let Some(page) = parent
            .ancestors()
            .skip(1)
            .find(|ancestor| page_directories.contains(*ancestor))
        {
            bail!(
                "{directory}/{} is in a subdirectory of the page {directory}/{}; a page directory holds files only",
                file.display(),
                page.display()
            );
        }
    }
    // Directories with no files, or only `.gitkeep`, would pass the check above until a file is added.
    for subdirectory in &directories {
        if let Some(page) = subdirectory
            .ancestors()
            .skip(1)
            .find(|ancestor| page_directories.contains(*ancestor))
        {
            bail!(
                "{directory}/{}/ is a subdirectory of the page {directory}/{}; a page directory holds files only",
                subdirectory.display(),
                page.display()
            );
        }
    }
    let mut pages = Vec::new();
    let mut assets = Vec::new();
    for file in files {
        let parent = file.parent().unwrap_or(Path::new(""));
        if is_page_source(&file) {
            pages.push(file);
        } else if file.extension().is_some_and(|ext| ext == "md") {
            bail!(
                "{directory}/{} is not supported; write each page as index.md in its own directory",
                file.display()
            );
        } else if page_directories.contains(parent) {
            assets.push(file);
        } else {
            bail!(
                "{directory}/{} is not in a page directory; put it next to a page's index.md, or in static/",
                file.display()
            );
        }
    }
    Ok(ContentFiles { pages, assets })
}

/// Checks that drafts, laid over `content/`, keep one page per directory and no nesting.
pub(crate) fn check_drafts(content: &ContentFiles, drafts: &ContentFiles) -> Result<()> {
    let published = content
        .pages
        .iter()
        .filter_map(|page| page.parent())
        .collect::<BTreeSet<_>>();
    for draft in drafts.pages.iter().filter_map(|page| page.parent()) {
        ensure!(
            !published.contains(draft),
            "drafts/{0}/ is already published as content/{0}/; delete the draft",
            draft.display()
        );
        if let Some(page) = published
            .iter()
            .find(|page| draft.starts_with(page) || page.starts_with(draft))
        {
            bail!(
                "the draft drafts/{}/ and the page content/{}/ would nest; a page directory holds files only",
                draft.display(),
                page.display()
            );
        }
    }
    Ok(())
}

fn is_page_source(file: &Path) -> bool {
    file.file_name().is_some_and(|name| name == "index.md")
}

/// Parses the `index.md` at `relative` under `directory`.
pub(crate) fn parse(
    text: &str,
    directory: ContentDir,
    relative: &Path,
    timezone: &TimeZone,
    mut image: impl FnMut(&str, &str) -> Result<Option<Image>>,
) -> Result<Article> {
    let (front_matter, body) = split_front_matter(text)?;
    let route = Route::from_content_path(relative)?;
    let meta = front_matter.resolve(relative, timezone)?;
    let parsed = crate::markdown::parse(body).map_err(|error| {
        let line = line_number(text, text.len() - body.len() + error.offset);
        anyhow::anyhow!(
            "raw HTML is not allowed in Markdown at line {line}; write it with Markdown syntax, or use a code span or `\\<` to show it as text"
        )
    })?;
    let rendered = parsed.render(|url| image(route.url(), url))?;
    Ok(Article {
        title: meta.title,
        description: meta.description,
        route,
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        tags: meta.tags,
        content: rendered.content,
        links: rendered.links,
        source: directory.path().join(relative),
        draft: directory == ContentDir::Drafts,
    })
}

/// Returns the 1-based line of a byte offset, counting CRLF, LF, and lone CR as line breaks like Markdown parsers do.
fn line_number(text: &str, offset: usize) -> usize {
    let before = text.as_bytes().get(..offset).unwrap_or_default();
    let breaks = before
        .iter()
        .enumerate()
        .filter(|&(index, &byte)| {
            byte == b'\n' || (byte == b'\r' && text.as_bytes().get(index + 1) != Some(&b'\n'))
        })
        .count();
    breaks + 1
}

fn split_front_matter(text: &str) -> Result<(FrontMatter, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok((FrontMatter::default(), text));
    };
    if first.trim_end() != "+++" {
        return Ok((FrontMatter::default(), text));
    }
    let mut end = first.len();
    for line in lines {
        end += line.len();
        if line.trim_end() == "+++" {
            let fields = text
                .get(first.len()..end - line.len())
                .context("invalid front matter fields boundary")?;
            let body = text.get(end..).context("invalid body boundary")?;
            // The leading newline stands for the opening `+++`, so TOML errors report file lines.
            let metadata: FrontMatter =
                toml::from_str(&format!("\n{fields}")).context("invalid TOML front matter")?;
            return Ok((metadata, body));
        }
    }
    bail!("front matter is missing closing +++ delimiter")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, relative: &Path, timezone: &TimeZone) -> Result<Article> {
        super::parse(source, ContentDir::Content, relative, timezone, |_, _| {
            Ok(None)
        })
    }

    /// Lists `files` with every directory that holds them, as `SiteInput::tree` does.
    fn tree(files: &[&str]) -> Tree {
        let files = files.iter().map(PathBuf::from).collect::<Vec<_>>();
        let directories = files
            .iter()
            .flat_map(|file| file.ancestors().skip(1))
            .filter(|directory| !directory.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Tree { files, directories }
    }

    #[test]
    fn sorts_content_files_into_pages_and_their_assets() -> Result<()> {
        let sorted = classify_files(
            tree(&[
                "about/index.md",
                "entries/a/index.md",
                "entries/a/figure.png",
                "entries/b/index.md",
            ]),
            ContentDir::Content,
        )?;
        assert_eq!(
            sorted.pages,
            ["about/index.md", "entries/a/index.md", "entries/b/index.md"].map(PathBuf::from)
        );
        assert_eq!(sorted.assets, [PathBuf::from("entries/a/figure.png")]);
        Ok(())
    }

    #[test]
    fn rejects_files_outside_the_page_rule() -> Result<()> {
        for (files, expected) in [
            (vec!["index.md"], "content/index.md is not supported"),
            (
                vec!["entries/a/index.md", "entries/a/b/index.md"],
                "content/entries/a/b/index.md is in a subdirectory of the page content/entries/a",
            ),
            (
                vec!["entries/a/index.md", "entries/a/images/x.png"],
                "content/entries/a/images/x.png is in a subdirectory of the page content/entries/a",
            ),
            (
                vec!["entries/a/index.md", "entries/a/notes.md"],
                "content/entries/a/notes.md is not supported; write each page as index.md",
            ),
            (
                vec!["entries/post.md"],
                "content/entries/post.md is not supported",
            ),
            (
                vec!["entries/a/index.md", "entries/x.png"],
                "content/entries/x.png is not in a page directory",
            ),
        ] {
            let error = classify_files(tree(&files), ContentDir::Content)
                .err()
                .with_context(|| format!("accepted files for {expected}"))?;
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        Ok(())
    }

    #[test]
    fn rejects_empty_subdirectories_of_a_page() -> Result<()> {
        let mut listed = tree(&["entries/a/index.md"]);
        listed.directories.push(PathBuf::from("entries/a/images"));
        let error = classify_files(listed, ContentDir::Content)
            .err()
            .context("accepted an empty subdirectory of a page")?;
        assert!(
            error.to_string().contains(
                "content/entries/a/images/ is a subdirectory of the page content/entries/a"
            ),
            "{error:#}"
        );
        let mut listed = tree(&["entries/a/index.md"]);
        listed.directories.push(PathBuf::from("entries/b"));
        classify_files(listed, ContentDir::Content)?;
        Ok(())
    }

    #[test]
    fn drafts_cannot_share_or_nest_with_published_pages() -> Result<()> {
        let sorted = |files: &[&str], directory| classify_files(tree(files), directory);
        let content = sorted(&["entries/a/index.md"], ContentDir::Content)?;
        check_drafts(
            &content,
            &sorted(&["entries/b/index.md"], ContentDir::Drafts)?,
        )?;
        check_drafts(
            &content,
            &sorted(&["entries/ab/index.md"], ContentDir::Drafts)?,
        )?;
        for (draft, expected) in [
            (
                "entries/a/index.md",
                "drafts/entries/a/ is already published as content/entries/a/",
            ),
            (
                "entries/a/b/index.md",
                "the draft drafts/entries/a/b/ and the page content/entries/a/ would nest",
            ),
            (
                "entries/index.md",
                "the draft drafts/entries/ and the page content/entries/a/ would nest",
            ),
        ] {
            let error = check_drafts(&content, &sorted(&[draft], ContentDir::Drafts)?)
                .err()
                .with_context(|| format!("accepted {draft}"))?;
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        let error = sorted(&["entries/x.png"], ContentDir::Drafts)
            .err()
            .context("accepted a draft file outside a page")?;
        assert!(
            error
                .to_string()
                .contains("drafts/entries/x.png is not in a page directory"),
            "{error:#}"
        );
        Ok(())
    }

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
            let error = parse(source, Path::new("post/index.md"), &TimeZone::UTC)
                .err()
                .context(format!("accepted {source:?}"))?;
            assert!(
                format!("{error:#}").contains(reason),
                "{source:?}: expected {reason:?}, got {error:#}"
            );
        }
        for (created_at, updated_at, expected_created, expected_updated) in [
            (
                "2026-09-17 00:00",
                "2026-09-22 00:00",
                "2026-09-17T00:00:00+00:00",
                "2026-09-22T00:00:00+00:00",
            ),
            (
                "2026-09-17T10:30",
                "2026-09-17 10:31",
                "2026-09-17T10:30:00+00:00",
                "2026-09-17T10:31:00+00:00",
            ),
        ] {
            let article = parse(
                &format!(
                    "+++\ncreated_at = {created_at}\nupdated_at = {updated_at}\ndescription = 'Post'\n+++\n"
                ),
                Path::new("post/index.md"),
                &TimeZone::UTC,
            )?;
            assert_eq!(rfc3339(&article.created_at), expected_created);
            assert_eq!(rfc3339(&article.updated_at), expected_updated);
        }
        Ok(())
    }

    #[test]
    fn front_matter_errors_report_file_lines() -> Result<()> {
        for (source, location) in [
            (
                "+++\ndescription = 'Post'\ncreated_at = 2026-09-17 10:00:00\n+++\n",
                "line 3, column 14",
            ),
            (
                "\u{feff}+++\r\n\r\ntags = ['React']\r\n+++\r\n",
                "line 3, column 8",
            ),
            (
                "+++\ndescription = 'Post'\ntemplate = 'page.html'\n+++\n",
                "line 3, column 1",
            ),
        ] {
            let error = parse(source, Path::new("post/index.md"), &TimeZone::UTC)
                .err()
                .context(format!("accepted {source:?}"))?;
            assert!(
                format!("{error:#}").contains(location),
                "{source:?}: expected {location:?}, got {error:#}"
            );
        }
        Ok(())
    }

    #[test]
    fn validates_tags() -> Result<()> {
        let source = "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\ntags = ['react', 'react-19', 'web-security']\n+++\nBody";
        let article = parse(source, Path::new("post/index.md"), &TimeZone::UTC)?;
        assert_eq!(
            article.tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
            ["react", "react-19", "web-security"]
        );
        let untagged = parse(
            "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\n+++\nBody",
            Path::new("post/index.md"),
            &TimeZone::UTC,
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
                parse(&source, Path::new("post/index.md"), &TimeZone::UTC).is_err(),
                "accepted {tags}"
            );
        }
        Ok(())
    }

    #[test]
    fn applies_the_time_zone_offset_in_effect_on_each_date() -> Result<()> {
        let timezone = TimeZone::get("America/New_York")?;
        for (source, expected) in [
            ("2026-01-15 09:00", "2026-01-15T09:00:00-05:00"),
            ("2026-07-15 09:00", "2026-07-15T09:00:00-04:00"),
            ("2026-03-08 02:30", "2026-03-08T03:30:00-04:00"),
        ] {
            let article = parse(
                &format!(
                    "+++\ncreated_at = {source}\nupdated_at = {source}\ndescription = 'Post'\n+++\n"
                ),
                Path::new("post/index.md"),
                &timezone,
            )?;
            assert_eq!(rfc3339(&article.created_at), expected, "{source}");
        }
        Ok(())
    }

    #[test]
    fn reports_the_file_line_of_raw_html() -> Result<()> {
        let front = "+++\ncreated_at = 2026-09-17 00:00\nupdated_at = 2026-09-17 00:00\ndescription = 'Post'\n+++\n";
        let crlf_front = front.replace('\n', "\r\n");
        for (source, line) in [
            (format!("{front}<div>x</div>\n"), 6),
            (
                format!("{front}# Title\n\nText <b>bold</b>\n\n<div>second</div>\n"),
                8,
            ),
            (
                format!("\u{feff}{front}\n本文です。\n\n日本語<!-- メモ -->\n"),
                9,
            ),
            (
                format!("{crlf_front}# 見出し\r\n\r\n## 日本語 <span>x</span>\r\n"),
                8,
            ),
            (
                format!("\u{feff}{crlf_front}段落\r\n\r\n![画像 <i>x</i>](a.png)\r\n"),
                8,
            ),
        ] {
            let error = parse(&source, Path::new("post/index.md"), &TimeZone::UTC)
                .err()
                .context(format!("accepted {source:?}"))?;
            let expected = format!("raw HTML is not allowed in Markdown at line {line};");
            assert!(
                format!("{error:#}").contains(&expected),
                "{source:?}: expected line {line}, got {error:#}"
            );
        }
        Ok(())
    }

    #[test]
    fn counts_every_commonmark_line_ending() {
        let source = "a\nb\r\nc\rd\r\n\re";
        for (offset, line) in [(0, 1), (2, 2), (5, 3), (7, 4), (10, 5), (11, 6)] {
            assert_eq!(line_number(source, offset), line, "offset {offset}");
        }
    }
}
