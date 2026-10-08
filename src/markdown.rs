use crate::images::{Image, Size};
use anyhow::{Context as _, bail, ensure};
use bitview::Html;
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};
use std::{collections::HashSet, slice};

pub(crate) struct Rendered {
    pub(crate) content: Html,
    /// The targets that `site_links` selects, for checking against the site.
    pub(crate) links: Vec<String>,
}

/// Raw HTML written in the Markdown text, which articles may not contain.
#[derive(Debug)]
pub(crate) struct RawHtml {
    /// Byte offset of the first raw HTML in the text passed to `parse`.
    pub(crate) offset: usize,
}

pub(crate) struct Parsed<'a> {
    events: Vec<Event<'a>>,
}

pub(crate) fn parse(text: &str) -> Result<Parsed<'_>, RawHtml> {
    let parsed = Parser::new_ext(text, Options::ENABLE_TABLES)
        .into_offset_iter()
        .map(|(event, range)| match event {
            Event::Html(_) | Event::InlineHtml(_) => Err(RawHtml {
                offset: range.start,
            }),
            other => Ok(other),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Parsed { events: parsed })
}

impl Parsed<'_> {
    pub(crate) fn render(
        self,
        mut local_image: impl FnMut(&str) -> anyhow::Result<Option<Image>>,
    ) -> anyhow::Result<Rendered> {
        Ok(Rendered {
            links: site_links(&self.events),
            content: to_html(&self.events, &mut local_image)?,
        })
    }
}

/// Returns the link and image targets that the HTML will contain, in document order, leaving out
/// external web links and email autolinks. Images keep only the text of their alt content.
fn site_links(events: &[Event<'_>]) -> Vec<String> {
    let mut links = Vec::new();
    let mut image_depth = 0_usize;
    let mut linked_heading = false;
    for event in events {
        match event {
            Event::Start(Tag::Heading { level, .. }) if is_linked_heading(*level) => {
                linked_heading = true;
            }
            Event::End(TagEnd::Heading(_)) => linked_heading = false,
            Event::Start(Tag::Image { dest_url, .. }) => {
                if image_depth == 0 {
                    links.push(dest_url.to_string());
                }
                image_depth += 1;
            }
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            // Email autolinks have no mailto: yet.
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) if image_depth == 0
                && !linked_heading
                && *link_type != LinkType::Email
                && !crate::route::is_external_web_link(dest_url) =>
            {
                links.push(dest_url.to_string());
            }
            _ => {}
        }
    }
    links
}

fn to_html(
    events: &[Event<'_>],
    local_image: &mut impl FnMut(&str) -> anyhow::Result<Option<Image>>,
) -> anyhow::Result<Html> {
    let mut tree = Tree::default();
    let mut state = State::default();
    let mut events = events.iter();
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                let image = image(&mut events, dest_url, title, local_image)?;
                tree.push(image);
            }
            Event::Start(tag) => {
                let opened = state.start(&mut tree, tag, &events)?;
                tree.started.push(opened);
            }
            Event::End(tag) => {
                match tag {
                    TagEnd::Heading(_) => state.linked_heading = false,
                    TagEnd::TableHead => state.table_head = false,
                    TagEnd::Table => state.table_body = false,
                    _ => {}
                }
                tree.end()?;
            }
            Event::Text(text) => tree.push(Html::text(text.as_ref())),
            Event::Code(code) => tree.push(element("code", Vec::new(), Html::text(code.as_ref()))?),
            Event::SoftBreak => tree.push(Html::text("\n")),
            Event::HardBreak => tree.push(element("br", Vec::new(), Html::default())?),
            Event::Rule => tree.push(element("hr", Vec::new(), Html::default())?),
            // `parse` rejects raw HTML, and the other events need options that are not enabled.
            other => bail!("unsupported Markdown event {other:?}"),
        }
    }
    tree.finish()
}

#[derive(Default)]
struct Tree {
    root: Html,
    open: Vec<Open>,
    /// A Markdown tag can open several elements (a linked heading opens `h2` and `a`), so its end
    /// must close as many as it opened.
    started: Vec<usize>,
}

struct Open {
    name: &'static str,
    attrs: Vec<(&'static str, String)>,
    children: Html,
}

impl Tree {
    fn push(&mut self, html: Html) {
        match self.open.last_mut() {
            Some(parent) => parent.children.push(html),
            None => self.root.push(html),
        }
    }

    fn open(&mut self, name: &'static str, attrs: Vec<(&'static str, String)>) {
        self.open.push(Open {
            name,
            attrs,
            children: Html::default(),
        });
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let open = self.open.pop().context("unbalanced Markdown events")?;
        let name = open.name;
        let link = open
            .attrs
            .iter()
            .find(|(attr, _)| *attr == "href")
            .map(|(_, href)| href.clone());
        let closed = element(name, open.attrs, open.children).with_context(|| match &link {
            Some(href) => format!("invalid link {href}"),
            None => format!("cannot build <{name}>"),
        })?;
        self.push(closed);
        Ok(())
    }

    fn end(&mut self) -> anyhow::Result<()> {
        let opened = self.started.pop().context("unbalanced Markdown events")?;
        for _ in 0..opened {
            self.close()?;
        }
        Ok(())
    }

    fn finish(self) -> anyhow::Result<Html> {
        ensure!(
            self.open.is_empty() && self.started.is_empty(),
            "unbalanced Markdown events"
        );
        Ok(self.root)
    }
}

#[derive(Default)]
struct State {
    used_ids: HashSet<String>,
    linked_heading: bool,
    table_head: bool,
    table_body: bool,
    alignments: Vec<Alignment>,
    column: usize,
}

impl State {
    fn start(
        &mut self,
        tree: &mut Tree,
        tag: &Tag<'_>,
        rest: &slice::Iter<'_, Event<'_>>,
    ) -> anyhow::Result<usize> {
        match tag {
            Tag::Paragraph => tree.open("p", Vec::new()),
            Tag::Heading { level, .. } if is_linked_heading(*level) => {
                let text = heading_text(
                    rest.clone()
                        .take_while(|event| !matches!(event, Event::End(TagEnd::Heading(_)))),
                );
                let id = unique_heading_id(&text, &mut self.used_ids);
                tree.open(heading_name(*level), vec![("id", id.clone())]);
                tree.open(
                    "a",
                    vec![
                        ("class", "heading-anchor".to_owned()),
                        ("href", format!("#{id}")),
                    ],
                );
                self.linked_heading = true;
                return Ok(2);
            }
            Tag::Heading { level, .. } => tree.open(heading_name(*level), Vec::new()),
            Tag::BlockQuote(_) => tree.open("blockquote", Vec::new()),
            Tag::CodeBlock(kind) => return code_block(tree, kind),
            Tag::List(Some(start)) => {
                let attrs = if *start == 1 {
                    Vec::new()
                } else {
                    vec![("start", start.to_string())]
                };
                tree.open("ol", attrs);
            }
            Tag::List(None) => tree.open("ul", Vec::new()),
            Tag::Item => tree.open("li", Vec::new()),
            Tag::Table(alignments) => {
                self.alignments.clone_from(alignments);
                tree.open("table", Vec::new());
            }
            Tag::TableHead => {
                self.table_head = true;
                self.column = 0;
                tree.open("thead", Vec::new());
                tree.open("tr", Vec::new());
                return Ok(2);
            }
            Tag::TableRow => {
                // `tbody` has no Markdown tag of its own: it opens with the first row and closes
                // with the table, so the table's end closes it.
                if !self.table_body {
                    self.table_body = true;
                    tree.open("tbody", Vec::new());
                    if let Some(table) = tree.started.last_mut() {
                        *table += 1;
                    }
                }
                self.column = 0;
                tree.open("tr", Vec::new());
            }
            Tag::TableCell => {
                // A class rather than a `style` attribute, so that the CSS decides how cells align.
                let attrs = match self.alignments.get(self.column) {
                    Some(Alignment::Left) => vec![("class", "align-left".to_owned())],
                    Some(Alignment::Center) => vec![("class", "align-center".to_owned())],
                    Some(Alignment::Right) => vec![("class", "align-right".to_owned())],
                    Some(Alignment::None) | None => Vec::new(),
                };
                self.column += 1;
                tree.open(if self.table_head { "th" } else { "td" }, attrs);
            }
            Tag::Emphasis => tree.open("em", Vec::new()),
            Tag::Strong => tree.open("strong", Vec::new()),
            // Anchors cannot nest, so links inside a linked heading keep only their text.
            Tag::Link { .. } if self.linked_heading => return Ok(0),
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => tree.open("a", link_attributes(*link_type, dest_url, title)),
            other => bail!("unsupported Markdown element {other:?}"),
        }
        Ok(1)
    }
}

fn link_attributes(link_type: LinkType, url: &str, title: &str) -> Vec<(&'static str, String)> {
    let href = if link_type == LinkType::Email {
        format!("mailto:{url}")
    } else {
        url.to_owned()
    };
    let mut attrs = vec![("href", href)];
    if crate::route::is_external_web_link(url) {
        attrs.push(("target", "_blank".to_owned()));
        attrs.push(("rel", "noopener noreferrer".to_owned()));
    }
    if !title.is_empty() {
        attrs.push(("title", title.to_owned()));
    }
    attrs
}

fn code_block(tree: &mut Tree, kind: &CodeBlockKind<'_>) -> anyhow::Result<usize> {
    let language = match kind {
        CodeBlockKind::Fenced(info) => info.split_whitespace().next(),
        CodeBlockKind::Indented => None,
    };
    let Some(language) = language else {
        tree.open("pre", Vec::new());
        tree.open("code", Vec::new());
        return Ok(2);
    };
    tree.open("div", vec![("class", "code-block".to_owned())]);
    tree.push(element(
        "span",
        vec![("class", "code-language".to_owned())],
        Html::text(language),
    )?);
    tree.open("pre", Vec::new());
    tree.open("code", vec![("class", format!("language-{language}"))]);
    Ok(3)
}

fn image(
    events: &mut slice::Iter<'_, Event<'_>>,
    url: &str,
    title: &str,
    local_image: &mut impl FnMut(&str) -> anyhow::Result<Option<Image>>,
) -> anyhow::Result<Html> {
    let mut alt = String::new();
    let mut depth = 1_usize;
    for event in events.by_ref() {
        match event {
            Event::Start(Tag::Image { .. }) => depth += 1,
            Event::End(TagEnd::Image) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Text(text) | Event::Code(text) => alt.push_str(text),
            Event::SoftBreak | Event::HardBreak => alt.push(' '),
            _ => {}
        }
    }
    let image = local_image(url)?;
    let src = match &image {
        Some(image) => versioned_url(url, &image.hash.url_version())?,
        None => url.to_owned(),
    };
    let mut attrs = vec![
        ("src", src),
        ("alt", alt),
        ("loading", "lazy".to_owned()),
        ("decoding", "async".to_owned()),
    ];
    if let Some(Size { width, height }) = image.and_then(|image| image.size) {
        attrs.push(("width", width.to_string()));
        attrs.push(("height", height.to_string()));
    }
    if !title.is_empty() {
        attrs.push(("title", title.to_owned()));
    }
    element("img", attrs, Html::default()).with_context(|| format!("invalid image {url}"))
}

fn element(name: &str, attrs: Vec<(&'static str, String)>, children: Html) -> anyhow::Result<Html> {
    let attrs = attrs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    Ok(Html::element(name, attrs, children)?)
}

fn is_linked_heading(level: HeadingLevel) -> bool {
    matches!(level, HeadingLevel::H2 | HeadingLevel::H3)
}

fn heading_name(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
}

/// Reads the visible heading text for its ID, leaving out image alt text.
fn heading_text<'a>(events: impl Iterator<Item = &'a Event<'a>>) -> String {
    let mut text = String::new();
    let mut image_depth = 0_usize;
    for event in events {
        match event {
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            Event::Text(value) | Event::Code(value) if image_depth == 0 => text.push_str(value),
            Event::SoftBreak | Event::HardBreak if image_depth == 0 => text.push(' '),
            _ => {}
        }
    }
    text
}

fn unique_heading_id(text: &str, used: &mut HashSet<String>) -> String {
    let mut base = String::new();
    let mut separator = false;
    for character in text.chars() {
        if character.is_alphanumeric() {
            if separator && !base.is_empty() {
                base.push('-');
            }
            base.extend(character.to_lowercase());
            separator = false;
        } else if character.is_whitespace() || matches!(character, '-' | '_') {
            separator = true;
        }
    }
    if base.is_empty() {
        base.push_str("section");
    }
    let mut id = base.clone();
    let mut suffix = 2;
    while !used.insert(id.clone()) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

/// Adds `v=<version>` to the query, before any fragment. The key `v` is reserved for it.
fn versioned_url(url: &str, version: &str) -> anyhow::Result<String> {
    let (before_fragment, fragment) = url.split_at(url.find('#').unwrap_or(url.len()));
    let mut versioned = match before_fragment.split_once('?') {
        None => format!("{before_fragment}?"),
        Some((_, query)) => {
            // Query parsers decode keys, so `%76` is also `v`. A key that cannot be decoded is not `v`.
            anyhow::ensure!(
                !query.split('&').any(|pair| {
                    pair.split('=').next().is_some_and(|key| {
                        crate::route::percent_decode(key).is_ok_and(|key| key == "v")
                    })
                }),
                "image {url} has the query parameter v, which genbit reserves for the hash of the image; remove v from the query"
            );
            let mut versioned = before_fragment.to_owned();
            if !query.is_empty() && !query.ends_with('&') {
                versioned.push('&');
            }
            versioned
        }
    };
    versioned.push_str("v=");
    versioned.push_str(version);
    versioned.push_str(fragment);
    Ok(versioned)
}

#[cfg(test)]
mod tests {
    use super::Rendered;
    use crate::{
        content_hash::ContentHash,
        images::{Image, Size},
    };
    use anyhow::{Result, anyhow};

    fn render(source: &str) -> Result<Rendered> {
        super::parse(source)
            .map_err(|error| anyhow!("raw HTML at {}", error.offset))?
            .render(|_| Ok(None))
    }

    fn render_html(source: &str) -> Result<String> {
        Ok(render(source)?.content.to_fragment())
    }

    #[test]
    fn only_emitted_images_request_dimensions_and_errors_propagate() -> Result<()> {
        let source = "![outer ![inner](/inner.png)](/outer.png)";
        let mut requested = Vec::new();
        let parsed = super::parse(source).map_err(|_| anyhow::anyhow!("invalid test Markdown"))?;
        let hash = ContentHash::of_reader(&mut b"image".as_slice())?;
        let rendered = parsed.render(|url| {
            requested.push(url.to_owned());
            Ok(Some(Image {
                hash,
                size: Some(Size {
                    width: 300,
                    height: 200,
                }),
            }))
        })?;
        assert_eq!(requested, ["/outer.png"]);
        assert!(
            rendered
                .content
                .to_fragment()
                .contains("width=\"300\" height=\"200\"")
        );
        assert_eq!(rendered.links, ["/outer.png"]);
        let parsed = super::parse(source).map_err(|_| anyhow::anyhow!("invalid test Markdown"))?;
        assert!(
            parsed
                .render(|_| anyhow::bail!("cannot read image"))
                .is_err()
        );
        assert!(super::parse("![photo](/photo.png) <kbd>bad</kbd>").is_err());
        Ok(())
    }

    #[test]
    fn local_images_carry_the_content_hash_in_the_query() -> Result<()> {
        let hash = ContentHash::of_reader(&mut b"image".as_slice())?;
        let version = hash.url_version();
        let source = concat!(
            "![a](/a.svg) ![b](b.png?x=1#icon) ![c](c.png?#top) ![d](d.png?x=1&) ",
            "![e](https://example.com/e.png)"
        );
        let parsed = super::parse(source).map_err(|_| anyhow!("invalid test Markdown"))?;
        let rendered = parsed
            .render(|url| Ok((!url.starts_with("https:")).then_some(Image { hash, size: None })))?;
        for expected in [
            format!("src=\"/a.svg?v={version}\""),
            format!("src=\"b.png?x=1&amp;v={version}#icon\""),
            format!("src=\"c.png?v={version}#top\""),
            format!("src=\"d.png?x=1&amp;v={version}\""),
            "src=\"https://example.com/e.png\"".to_owned(),
        ] {
            assert!(
                rendered.content.to_fragment().contains(&expected),
                "{expected}: {}",
                rendered.content.to_fragment()
            );
        }
        assert!(
            !rendered.content.to_fragment().contains("width="),
            "{}",
            rendered.content.to_fragment()
        );
        assert_eq!(
            rendered.links,
            [
                "/a.svg",
                "b.png?x=1#icon",
                "c.png?#top",
                "d.png?x=1&",
                "https://example.com/e.png"
            ]
        );
        Ok(())
    }

    #[test]
    fn the_v_query_key_is_reserved_for_hashed_images() -> Result<()> {
        let hash = ContentHash::of_reader(&mut b"image".as_slice())?;
        for source in [
            "![a](/a.png?v=1)",
            "![a](/a.png?x=1&v#top)",
            "![a](/a.png?v=)",
            "![a](/a.png?%76=old)",
            "![a](/a.png?x=1&%76)",
        ] {
            let parsed = super::parse(source).map_err(|_| anyhow!("invalid test Markdown"))?;
            let error = parsed
                .render(|_| Ok(Some(Image { hash, size: None })))
                .err()
                .ok_or_else(|| anyhow!("accepted {source}"))?;
            assert!(
                format!("{error:#}").contains("has the query parameter v"),
                "{error:#}"
            );
        }
        for source in [
            "![a](/a.png?va=1&xv=2&%56=3&%zz=4)",
            "![a](https://example.com/a.png?v=1)",
        ] {
            let parsed = super::parse(source).map_err(|_| anyhow!("invalid test Markdown"))?;
            parsed.render(|url| {
                Ok((!url.starts_with("https:")).then_some(Image { hash, size: None }))
            })?;
        }
        Ok(())
    }

    #[test]
    fn adds_image_attributes_and_escapes_alt_text() -> Result<()> {
        let html = render_html("![a **bold** & &lt;bad&gt; \\<too>](image.png \"A title\")")?;
        assert!(html.contains("src=\"image.png\""), "{html}");
        assert!(
            html.contains("alt=\"a bold &amp; &lt;bad&gt; &lt;too&gt;\""),
            "{html}"
        );
        assert!(html.contains("loading=\"lazy\""), "{html}");
        assert!(html.contains("decoding=\"async\""), "{html}");
        assert!(html.contains("title=\"A title\""), "{html}");
        Ok(())
    }

    #[test]
    fn renders_tables_with_alignment_classes() -> Result<()> {
        let html = render_html("| Page | Size |\n| --- | ---: |\n| Top | 1,098 |\n| End | 0 |\n")?;
        assert_eq!(
            html,
            concat!(
                "<table><thead><tr><th>Page</th><th class=\"align-right\">Size</th></tr></thead>",
                "<tbody><tr><td>Top</td><td class=\"align-right\">1,098</td></tr>",
                "<tr><td>End</td><td class=\"align-right\">0</td></tr></tbody></table>",
            )
        );
        assert_eq!(
            render_html("| a | b | c |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |\n")?,
            concat!(
                "<table><thead><tr><th class=\"align-left\">a</th><th class=\"align-center\">b</th>",
                "<th class=\"align-right\">c</th></tr></thead><tbody><tr><td class=\"align-left\">1</td>",
                "<td class=\"align-center\">2</td><td class=\"align-right\">3</td></tr></tbody></table>",
            )
        );
        assert_eq!(
            render_html("| Page |\n| --- |\n")?,
            "<table><thead><tr><th>Page</th></tr></thead></table>"
        );
        Ok(())
    }

    #[test]
    fn numbers_lists_from_their_first_number() -> Result<()> {
        assert_eq!(
            render_html("1. a\n2. b\n\n- c\n")?,
            "<ol><li>a</li><li>b</li></ol><ul><li>c</li></ul>"
        );
        assert_eq!(render_html("3. a\n")?, "<ol start=\"3\"><li>a</li></ol>");
        Ok(())
    }

    #[test]
    fn rejects_links_and_images_with_unsafe_schemes() {
        for (source, expected) in [
            (
                "[x](javascript:alert(1))",
                "invalid link javascript:alert(1)",
            ),
            (
                "[x](JavaScript:alert(1))",
                "invalid link JavaScript:alert(1)",
            ),
            ("[x](data:text/html,x)", "invalid link data:text/html,x"),
            (
                "[x](tel:+81-3-0000-0000)",
                "invalid link tel:+81-3-0000-0000",
            ),
            (
                "![x](data:image/png;base64,AAAA)",
                "invalid image data:image/png;base64,AAAA",
            ),
        ] {
            let error = render(source).err().map(|error| format!("{error:#}"));
            assert!(
                error
                    .as_deref()
                    .is_some_and(|error| error.starts_with(expected)),
                "{source}: {error:?}"
            );
        }
    }

    #[test]
    fn deeply_nested_quotes_fail_instead_of_exhausting_the_stack() {
        let error = render(&format!("{} deep\n", ">".repeat(100_000)))
            .err()
            .map(|error| format!("{error:#}"));
        assert!(
            error
                .as_deref()
                .is_some_and(|error| error.contains("nested more than")),
            "{error:?}"
        );
    }

    #[test]
    fn collects_internal_links_inside_tables() -> Result<()> {
        let rendered = render("| Page |\n| --- |\n| [Top](/) |\n| [Next](../next/) |\n")?;
        assert_eq!(rendered.links, ["/", "../next/"]);
        Ok(())
    }

    #[test]
    fn preserves_non_image_markdown() -> Result<()> {
        let html = render_html("**bold** and `code`\n\n```rust\nlet x = 1;\n```\n")?;
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("let x = 1;"), "{html}");
        Ok(())
    }

    #[test]
    fn shows_fenced_code_language_without_changing_code_markup() -> Result<()> {
        let html =
            render_html("```tsx\nconst value = 1;\n```\n\n```js title=example\nalert(1);\n```\n")?;
        assert!(
            html.contains("<span class=\"code-language\">tsx</span>"),
            "{html}"
        );
        assert!(html.contains("<code class=\"language-tsx\">"), "{html}");
        assert!(
            html.contains("<span class=\"code-language\">js</span>"),
            "{html}"
        );
        assert!(html.contains("<code class=\"language-js\">"), "{html}");
        assert_eq!(html.matches("<div class=\"code-block\">").count(), 2);
        assert_eq!(html.matches("</div>").count(), 2);
        Ok(())
    }

    #[test]
    fn leaves_code_blocks_without_language_unlabeled() -> Result<()> {
        let html = render_html("```\nplain\n```\n\n    indented\n")?;
        assert!(!html.contains("code-language"), "{html}");
        assert!(!html.contains("code-block"), "{html}");
        assert_eq!(html.matches("<pre>").count(), 2);
        Ok(())
    }

    #[test]
    fn escapes_code_language_as_html_text() -> Result<()> {
        let html = render_html("```a<b&c\nvalue\n```\n")?;
        assert!(html.contains("a&lt;b&amp;c</span>"), "{html}");
        assert!(
            !html.contains("<span class=\"code-language\">a<b"),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn adds_links_to_second_and_third_level_headings() -> Result<()> {
        let html = render_html("# Title\n\n## Fiberとは\n\n### `useState` と Fiber\n")?;
        assert!(html.contains("<h1>Title</h1>"), "{html}");
        assert!(html.contains("<h2 id=\"fiberとは\">"), "{html}");
        assert!(html.contains("href=\"#fiberとは\""), "{html}");
        assert!(
            html.contains("<a class=\"heading-anchor\" href=\"#fiberとは\">Fiberとは</a></h2>"),
            "{html}"
        );
        assert!(html.contains("<h3 id=\"usestate-と-fiber\">"), "{html}");
        assert!(html.contains("href=\"#usestate-と-fiber\""), "{html}");
        assert!(
            html.contains("href=\"#usestate-と-fiber\"><code>useState</code> と Fiber</a></h3>"),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn collects_no_targets_from_image_alt_text() -> Result<()> {
        let rendered = render("![see [a](/a) ![b](/b.png)](/c.png) [d](/d)")?;
        assert_eq!(rendered.links, ["/c.png", "/d"]);
        Ok(())
    }

    #[test]
    fn unwraps_links_inside_headings() -> Result<()> {
        let parsed = super::parse("## [内部](other.md) と [外部](https://example.com)\n")
            .map_err(|_| anyhow!("invalid test Markdown"))?;
        let rendered = parsed.render(|_| Ok(None))?;
        assert!(rendered.links.is_empty(), "{:?}", rendered.links);
        let html = rendered.content.to_fragment();
        assert!(
            html.contains(
                "<a class=\"heading-anchor\" href=\"#内部-と-外部\">内部 と 外部</a></h2>"
            ),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn leaves_image_alt_text_out_of_heading_ids() -> Result<()> {
        let html = render_html("## ![icon](icon.png) Title\n")?;
        assert!(html.contains("<h2 id=\"title\">"), "{html}");
        assert!(html.contains("alt=\"icon\""), "{html}");
        Ok(())
    }

    #[test]
    fn keeps_heading_ids_unique_after_normalization() -> Result<()> {
        let html = render_html("## A B\n\n## A B\n\n### A-B\n\n## !!!\n\n## !!!\n")?;
        for id in ["a-b", "a-b-2", "a-b-3", "section", "section-2"] {
            assert!(html.contains(&format!("id=\"{id}\"")), "{html}");
            assert!(html.contains(&format!("href=\"#{id}\"")), "{html}");
        }
        Ok(())
    }

    #[test]
    fn leaves_markdown_file_links_unchanged() -> Result<()> {
        let html = render_html("[article](../other/index.md)")?;
        assert!(html.contains("href=\"../other/index.md\""), "{html}");
        Ok(())
    }

    #[test]
    fn opens_external_links_in_new_tabs() -> Result<()> {
        let html = render_html(
            "[web](https://example.com/?a=1&b=2 \"A & B\") [cdn](//cdn.example.com) [local](../next/) [section](#top)",
        )?;
        assert!(html.contains("href=\"https://example.com/?a=1&amp;b=2\" target=\"_blank\" rel=\"noopener noreferrer\" title=\"A &amp; B\""), "{html}");
        assert!(
            html.contains(
                "href=\"//cdn.example.com\" target=\"_blank\" rel=\"noopener noreferrer\""
            ),
            "{html}"
        );
        assert!(html.contains("href=\"../next/\""), "{html}");
        assert!(html.contains("href=\"#top\""), "{html}");
        assert_eq!(html.matches("target=\"_blank\"").count(), 2, "{html}");
        Ok(())
    }

    #[test]
    fn opens_http_links_with_case_insensitive_schemes_in_new_tabs() -> Result<()> {
        for url in [
            "HTTPS://example.com/a.md",
            "HtTp://example.com/a.md",
            "//example.com/a.md",
        ] {
            let html = render_html(&format!("[web]({url})"))?;
            assert!(
                html.contains(&format!("href=\"{url}\" target=\"_blank\"")),
                "{html}"
            );
            assert!(html.contains("rel=\"noopener noreferrer\""), "{html}");
        }
        Ok(())
    }

    #[test]
    fn collects_rendered_internal_link_and_image_targets() -> Result<()> {
        let rendered = render(concat!(
            "[next](../next/#x) ![photo](../img/a.png) [abs](/about) [top](#top)\n\n",
            "[web](https://example.com) <https://example.com/auto> <someone@example.com> ",
            "[mail](mailto:a@example.com)\n\n",
            "## [heading](gone.md) ![icon](icon.png)\n\n",
            "# [title](../kept/)\n",
        ))?;
        assert_eq!(
            rendered.links,
            [
                "../next/#x",
                "../img/a.png",
                "/about",
                "#top",
                "mailto:a@example.com",
                "icon.png",
                "../kept/"
            ]
        );
        Ok(())
    }

    fn rejected_offset(source: &str) -> Option<usize> {
        super::parse(source).err().map(|error| error.offset)
    }

    #[test]
    fn rejects_raw_html_at_its_source_offset() {
        for (source, offset) in [
            ("<div>block</div>\n", 0),
            (
                "Text\n\n<details>\n<summary>More</summary>\n</details>\n",
                6,
            ),
            ("Press <kbd>Ctrl</kbd> now\n", 6),
            ("Line<br>next\n", 4),
            ("<!-- note -->\n", 0),
            ("Text <!-- note --> more\n", 5),
            ("## Title <span>x</span>\n", 9),
            ("### <a href=\"/feed.xml\">RSS</a>\n", 4),
            ("![alt <b>bold</b>](a.png)\n", 6),
            ("| A |\n| --- |\n| <i>x</i> |\n", 16),
            ("> quoted <em>x</em>\n", 9),
            ("- item\n\n  <div>x</div>\n", 10),
            ("日本語<b>太字</b>\n", 9),
        ] {
            assert_eq!(rejected_offset(source), Some(offset), "{source:?}");
        }
    }

    #[test]
    fn reports_the_first_raw_html() {
        assert_eq!(
            rejected_offset("Plain\n\nfirst <i>x</i>\n\n<div>second</div>\n"),
            Some(13)
        );
    }

    #[test]
    fn shows_html_written_as_code_or_escaped_text() -> Result<()> {
        let html = render_html(concat!(
            "Use `<div>` and `<!-- -->`.\n\n",
            "```html\n<picture><img src=\"a.png\"></picture>\n<!-- note -->\n```\n\n",
            "    <details>indented</details>\n\n",
            "\\<kbd>Ctrl\\</kbd> &lt;br&gt; &#60;span&#62;\n",
        ))?;
        for expected in [
            "<code>&lt;div&gt;</code>",
            "<code>&lt;!-- --&gt;</code>",
            "&lt;picture&gt;&lt;img src=&quot;a.png&quot;&gt;&lt;/picture&gt;\n&lt;!-- note --&gt;",
            "&lt;details&gt;indented&lt;/details&gt;",
            "&lt;kbd&gt;Ctrl&lt;/kbd&gt; &lt;br&gt; &lt;span&gt;",
        ] {
            assert!(html.contains(expected), "{expected}: {html}");
        }
        for tag in ["<div>", "<picture>", "<details>", "<kbd>", "<br>", "<span>"] {
            assert!(!html.contains(tag), "{tag}: {html}");
        }
        Ok(())
    }

    #[test]
    fn keeps_url_and_email_autolinks() -> Result<()> {
        let html = render_html("<https://example.com/a> <someone@example.com>\n")?;
        assert!(
            html.contains(
                "<a href=\"https://example.com/a\" target=\"_blank\" rel=\"noopener noreferrer\">https://example.com/a</a>"
            ),
            "{html}"
        );
        assert!(
            html.contains("<a href=\"mailto:someone@example.com\">someone@example.com</a>"),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn preserves_md_email_addresses() -> Result<()> {
        let rendered = render(
            "<person@example.md> [mail](mailto:person@example.md) [next](../next/?x=1#section)",
        )?;
        let html = rendered.content.to_fragment();
        assert_eq!(html.matches("href=\"mailto:person@example.md\"").count(), 2);
        assert!(html.contains("<a href=\"mailto:person@example.md\">person@example.md</a>"));
        assert!(html.contains("href=\"../next/?x=1#section\""));
        assert_eq!(
            rendered.links,
            ["mailto:person@example.md", "../next/?x=1#section"]
        );
        Ok(())
    }
}
