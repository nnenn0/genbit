use crate::image_size::Size;
use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd, html,
};
use std::collections::HashSet;

pub(crate) struct Rendered {
    pub(crate) html: String,
    /// Link and image targets written to the HTML, except external links, for checking against the site.
    pub(crate) links: Vec<String>,
}

/// Raw HTML written in the Markdown source, which articles may not contain.
#[derive(Debug)]
pub(crate) struct RawHtml {
    /// Byte offset of the first raw HTML in the source passed to `render`.
    pub(crate) offset: usize,
}

/// Renders `source`, giving each image the intrinsic size `image_size` returns for its URL.
pub(crate) fn render(
    source: &str,
    image_size: impl Fn(&str) -> Option<Size>,
) -> Result<Rendered, RawHtml> {
    // Checked before any rewriting, because the later steps emit HTML events of their own.
    let parsed = Parser::new_ext(source, Options::ENABLE_TABLES)
        .into_offset_iter()
        .map(|(event, range)| match event {
            Event::Html(_) | Event::InlineHtml(_) => Err(RawHtml {
                offset: range.start,
            }),
            other => Ok(rewrite_article_link(other)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut links = Vec::new();
    let events = to_html_events(anchor_headings(parsed.into_iter()), &mut links, &image_size);
    let mut output = String::new();
    html::push_html(&mut output, events.into_iter());
    Ok(Rendered {
        html: output,
        links,
    })
}

fn rewrite_article_link(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: article_url(&dest_url).map_or(dest_url, Into::into),
            title,
            id,
        }),
        other => other,
    }
}

/// Makes each second- and third-level heading a link to itself, keeping the typed events of its content.
fn anchor_headings<'a>(mut events: impl Iterator<Item = Event<'a>>) -> Vec<Event<'a>> {
    let mut anchored = Vec::new();
    let mut used_ids = HashSet::new();
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::Heading {
                level,
                classes,
                attrs,
                ..
            }) if is_linked_heading(level) => {
                let heading = events
                    .by_ref()
                    .take_while(|inner| !matches!(inner, Event::End(TagEnd::Heading(_))))
                    .collect::<Vec<_>>();
                let id = unique_heading_id(&heading_text(&heading), &mut used_ids);
                anchored.push(Event::Start(Tag::Heading {
                    level,
                    id: Some(id.clone().into()),
                    classes,
                    attrs,
                }));
                anchored.push(Event::Html(
                    format!("<a class=\"heading-anchor\" href=\"#{id}\">").into(),
                ));
                // Anchors cannot nest, so links inside the heading keep only their text.
                anchored.extend(heading.into_iter().filter(|inner| {
                    !matches!(
                        inner,
                        Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link)
                    )
                }));
                anchored.push(Event::Html("</a>".into()));
                anchored.push(Event::End(TagEnd::Heading(level)));
            }
            other => anchored.push(other),
        }
    }
    anchored
}

/// Replaces events that need attributes pulldown-cmark cannot write, collecting link and image targets in document order.
fn to_html_events<'a>(
    events: Vec<Event<'a>>,
    links: &mut Vec<String>,
    image_size: &impl Fn(&str) -> Option<Size>,
) -> Vec<Event<'a>> {
    let mut converted = Vec::with_capacity(events.len());
    let mut events = events.into_iter();
    let mut external_link = false;
    let mut has_label = false;
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                links.push(dest_url.to_string());
                converted.push(Event::Html(
                    image_html(&mut events, &dest_url, &title, image_size(&dest_url)).into(),
                ));
            }
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) if is_external(&dest_url) => {
                external_link = true;
                converted.push(Event::Html(external_anchor(&dest_url, &title).into()));
            }
            Event::End(TagEnd::Link) if external_link => {
                external_link = false;
                converted.push(Event::Html("</a>".into()));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                // Email autolinks have no mailto: yet.
                if link_type != LinkType::Email {
                    links.push(dest_url.to_string());
                }
                converted.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if let Some(label) = code_block_label(&kind) {
                    converted.push(Event::Html(label.into()));
                    has_label = true;
                }
                converted.push(Event::Start(Tag::CodeBlock(kind)));
            }
            Event::End(TagEnd::CodeBlock) => {
                converted.push(Event::End(TagEnd::CodeBlock));
                if has_label {
                    converted.push(Event::Html("</div>".into()));
                    has_label = false;
                }
            }
            other => converted.push(other),
        }
    }
    converted
}

fn is_external(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("//")
}

fn external_anchor(url: &str, title: &str) -> String {
    let mut anchor = format!(
        "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\"",
        escape_attribute(url)
    );
    if !title.is_empty() {
        anchor.push_str(" title=\"");
        anchor.push_str(&escape_attribute(title));
        anchor.push('"');
    }
    anchor.push('>');
    anchor
}

fn is_linked_heading(level: HeadingLevel) -> bool {
    matches!(level, HeadingLevel::H2 | HeadingLevel::H3)
}

fn code_block_label(kind: &CodeBlockKind<'_>) -> Option<String> {
    let CodeBlockKind::Fenced(info) = kind else {
        return None;
    };
    let language = info.split_whitespace().next()?;
    Some(format!(
        "<div class=\"code-block\"><span class=\"code-language\">{}</span>",
        escape_attribute(language)
    ))
}

/// Reads the visible heading text for its ID, leaving out image alt text.
fn heading_text(events: &[Event<'_>]) -> String {
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

fn article_url(url: &str) -> Option<String> {
    let (path, suffix) = crate::route::split_site_link(url)?;
    if path.starts_with('/') {
        return None;
    }
    path.strip_suffix(".md")
        .map(|stem| format!("{stem}{suffix}"))
}

fn image_html<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    source: &str,
    title: &str,
    size: Option<Size>,
) -> String {
    let mut alt = String::new();
    let mut image_depth = 1;
    for event in events {
        match event {
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => {
                image_depth -= 1;
                if image_depth == 0 {
                    break;
                }
            }
            Event::Text(text) | Event::Code(text) => alt.push_str(&text),
            Event::SoftBreak | Event::HardBreak => alt.push(' '),
            _ => {}
        }
    }
    let size = size.map_or_else(String::new, |Size { width, height }| {
        format!(" width=\"{width}\" height=\"{height}\"")
    });
    let mut image = format!(
        "<img src=\"{}\" alt=\"{}\"{size} loading=\"lazy\" decoding=\"async\"",
        escape_attribute(source),
        escape_attribute(&alt)
    );
    if !title.is_empty() {
        image.push_str(" title=\"");
        image.push_str(&escape_attribute(title));
        image.push('"');
    }
    image.push('>');
    image
}

fn escape_attribute(source: &str) -> String {
    let mut escaped = String::with_capacity(source.len());
    for character in source.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{RawHtml, Rendered, render};
    use crate::image_size::Size;

    fn render_without_sizes(source: &str) -> Result<Rendered, RawHtml> {
        render(source, |_| None)
    }

    fn render_html(source: &str) -> Result<String, RawHtml> {
        Ok(render_without_sizes(source)?.html)
    }

    #[test]
    fn adds_image_attributes_and_escapes_alt_text() -> Result<(), RawHtml> {
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
    fn renders_tables_with_alignment() -> Result<(), RawHtml> {
        let html = render_html("| Page | Size |\n| --- | ---: |\n| Top | 1,098 |\n")?;
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<th>Page</th>"), "{html}");
        assert!(
            html.contains("<td style=\"text-align: right\">1,098</td>"),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn collects_internal_links_inside_tables() -> Result<(), RawHtml> {
        let rendered =
            render_without_sizes("| Page |\n| --- |\n| [Top](/) |\n| [Next](next.md) |\n")?;
        assert_eq!(rendered.links, ["/", "next"]);
        Ok(())
    }

    #[test]
    fn preserves_non_image_markdown() -> Result<(), RawHtml> {
        let html = render_html("**bold** and `code`\n\n```rust\nlet x = 1;\n```\n")?;
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("let x = 1;"), "{html}");
        Ok(())
    }

    #[test]
    fn shows_fenced_code_language_without_changing_code_markup() -> Result<(), RawHtml> {
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
    fn leaves_code_blocks_without_language_unlabeled() -> Result<(), RawHtml> {
        let html = render_html("```\nplain\n```\n\n    indented\n")?;
        assert!(!html.contains("code-language"), "{html}");
        assert!(!html.contains("code-block"), "{html}");
        assert_eq!(html.matches("<pre>").count(), 2);
        Ok(())
    }

    #[test]
    fn escapes_code_language_as_html_text() -> Result<(), RawHtml> {
        let html = render_html("```a<b&c\nvalue\n```\n")?;
        assert!(html.contains("a&lt;b&amp;c</span>"), "{html}");
        assert!(
            !html.contains("<span class=\"code-language\">a<b"),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn adds_links_to_second_and_third_level_headings() -> Result<(), RawHtml> {
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
    fn unwraps_links_inside_headings() -> Result<(), RawHtml> {
        let html = render_html("## [内部](other.md) と [外部](https://example.com)\n")?;
        assert!(
            html.contains(
                "<a class=\"heading-anchor\" href=\"#内部-と-外部\">内部 と 外部</a></h2>"
            ),
            "{html}"
        );
        Ok(())
    }

    #[test]
    fn leaves_image_alt_text_out_of_heading_ids() -> Result<(), RawHtml> {
        let html = render_html("## ![icon](icon.png) Title\n")?;
        assert!(html.contains("<h2 id=\"title\">"), "{html}");
        assert!(html.contains("alt=\"icon\""), "{html}");
        Ok(())
    }

    #[test]
    fn keeps_heading_ids_unique_after_normalization() -> Result<(), RawHtml> {
        let html = render_html("## A B\n\n## A B\n\n### A-B\n\n## !!!\n\n## !!!\n")?;
        for id in ["a-b", "a-b-2", "a-b-3", "section", "section-2"] {
            assert!(html.contains(&format!("id=\"{id}\"")), "{html}");
            assert!(html.contains(&format!("href=\"#{id}\"")), "{html}");
        }
        Ok(())
    }

    #[test]
    fn rewrites_relative_article_links_without_changing_other_urls() -> Result<(), RawHtml> {
        for (url, expected) in [
            ("other.md", "other"),
            ("../other.md#section", "../other#section"),
            (
                "posts/other.md?view=full#section",
                "posts/other?view=full#section",
            ),
            (
                "https://example.com/other.md",
                "https://example.com/other.md",
            ),
            ("mailto:other.md", "mailto:other.md"),
            ("/other.md", "/other.md"),
            ("//example.com/other.md", "//example.com/other.md"),
            ("#section", "#section"),
            ("other.html", "other.html"),
        ] {
            let html = render_html(&format!("[article]({url})"))?;
            assert!(html.contains(&format!("href=\"{expected}\"")), "{html}");
        }
        let image = render_html("![image](other.md)")?;
        assert!(image.contains("src=\"other.md\""), "{image}");
        Ok(())
    }

    #[test]
    fn opens_external_links_in_new_tabs() -> Result<(), RawHtml> {
        let html = render_html(
            "[web](https://example.com/?a=1&b=2 \"A & B\") [cdn](//cdn.example.com) [local](next.md) [section](#top)",
        )?;
        assert!(html.contains("href=\"https://example.com/?a=1&amp;b=2\" target=\"_blank\" rel=\"noopener noreferrer\" title=\"A &amp; B\""), "{html}");
        assert!(
            html.contains(
                "href=\"//cdn.example.com\" target=\"_blank\" rel=\"noopener noreferrer\""
            ),
            "{html}"
        );
        assert!(html.contains("href=\"next\""), "{html}");
        assert!(html.contains("href=\"#top\""), "{html}");
        assert_eq!(html.matches("target=\"_blank\"").count(), 2, "{html}");
        Ok(())
    }

    #[test]
    fn collects_rendered_internal_link_and_image_targets() -> Result<(), RawHtml> {
        let rendered = render_without_sizes(concat!(
            "[next](next.md#x) ![photo](../img/a.png) [abs](/about) [top](#top)\n\n",
            "[web](https://example.com) <https://example.com/auto> <someone@example.com> ",
            "[mail](mailto:a@example.com)\n\n",
            "## [heading](gone.md) ![icon](icon.png)\n\n",
            "# [title](kept.md)\n",
        ))?;
        assert_eq!(
            rendered.links,
            [
                "next#x",
                "../img/a.png",
                "/about",
                "#top",
                "mailto:a@example.com",
                "icon.png",
                "kept"
            ]
        );
        Ok(())
    }

    fn rejected_offset(source: &str) -> Option<usize> {
        render_without_sizes(source).err().map(|error| error.offset)
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
    fn shows_html_written_as_code_or_escaped_text() -> Result<(), RawHtml> {
        let html = render_html(concat!(
            "Use `<div>` and `<!-- -->`.\n\n",
            "```html\n<picture><img src=\"a.png\"></picture>\n<!-- note -->\n```\n\n",
            "    <details>indented</details>\n\n",
            "\\<kbd>Ctrl\\</kbd> &lt;br&gt; &#60;span&#62;\n",
        ))?;
        for expected in [
            "<code>&lt;div&gt;</code>",
            "<code>&lt;!-- --&gt;</code>",
            "&lt;picture&gt;&lt;img src=\"a.png\"&gt;&lt;/picture&gt;\n&lt;!-- note --&gt;",
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
    fn keeps_url_and_email_autolinks() -> Result<(), RawHtml> {
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
    fn writes_the_intrinsic_size_of_known_images() -> Result<(), RawHtml> {
        let size = |url: &str| {
            (url == "a.png").then_some(Size {
                width: 300,
                height: 200,
            })
        };
        let rendered = render(
            "![A & B](a.png \"T\") [![linked](a.png)](next.md) ![other](b.png)\n\n## ![icon](a.png) Title\n",
            size,
        )?;
        let html = rendered.html;
        assert!(
            html.contains("<img src=\"a.png\" alt=\"A &amp; B\" width=\"300\" height=\"200\" loading=\"lazy\" decoding=\"async\" title=\"T\">"),
            "{html}"
        );
        assert!(
            html.contains("<a href=\"next\"><img src=\"a.png\" alt=\"linked\" width=\"300\" height=\"200\" loading=\"lazy\""),
            "{html}"
        );
        assert!(
            html.contains("<img src=\"b.png\" alt=\"other\" loading=\"lazy\""),
            "{html}"
        );
        assert!(html.contains("<h2 id=\"title\">"), "{html}");
        assert_eq!(html.matches("width=\"300\"").count(), 3, "{html}");
        assert_eq!(rendered.links, ["a.png", "next", "a.png", "b.png", "a.png"]);
        Ok(())
    }
}
