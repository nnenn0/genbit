use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Parser, Tag, TagEnd, html};
use std::collections::HashSet;

pub(crate) struct Rendered {
    pub(crate) html: String,
    /// Link and image targets written to the HTML, except external links, for checking against the site.
    pub(crate) links: Vec<String>,
}

pub(crate) fn render(source: &str) -> Rendered {
    let mut events = Parser::new(source);
    let mut external_link = false;
    let mut in_linked_heading = false;
    let mut links = Vec::new();
    let collected = &mut links;
    let mut transformed = std::iter::from_fn(move || {
        let event = events.next()?;
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                in_linked_heading = is_linked_heading(level);
                Some(event)
            }
            Event::End(TagEnd::Heading(_)) => {
                in_linked_heading = false;
                Some(event)
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                collected.push(dest_url.to_string());
                Some(Event::Html(
                    image_html(&mut events, &dest_url, &title).into(),
                ))
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                if dest_url.starts_with("https://")
                    || dest_url.starts_with("http://")
                    || dest_url.starts_with("//")
                {
                    external_link = true;
                    Some(Event::Html(external_anchor(&dest_url, &title).into()))
                } else {
                    let dest_url = article_url(&dest_url).map_or(dest_url, Into::into);
                    // Links inside linked headings are unwrapped, and email autolinks have no mailto: yet.
                    if !in_linked_heading && link_type != LinkType::Email {
                        collected.push(dest_url.to_string());
                    }
                    Some(Event::Start(Tag::Link {
                        link_type,
                        dest_url,
                        title,
                        id,
                    }))
                }
            }
            Event::End(TagEnd::Link) if external_link => {
                external_link = false;
                Some(Event::Html("</a>".into()))
            }
            other => Some(other),
        }
    });
    let mut anchored = Vec::new();
    let mut used_ids = HashSet::new();
    while let Some(event) = transformed.next() {
        match event {
            Event::Start(Tag::Heading {
                level,
                classes,
                attrs,
                ..
            }) if is_linked_heading(level) => {
                let mut heading = Vec::new();
                for inner in transformed.by_ref() {
                    if matches!(inner, Event::End(TagEnd::Heading(_))) {
                        break;
                    }
                    heading.push(inner);
                }
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
                anchored.extend(heading.into_iter().filter(|event| !is_link_boundary(event)));
                anchored.push(Event::Html("</a>".into()));
                anchored.push(Event::End(TagEnd::Heading(level)));
            }
            other => anchored.push(other),
        }
    }
    drop(transformed);
    let mut output = String::new();
    html::push_html(&mut output, add_code_labels(anchored).into_iter());
    Rendered {
        html: output,
        links,
    }
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

/// 見出し全体をリンクにするため、見出し内のリンクはテキストだけ残す。
fn is_link_boundary(event: &Event<'_>) -> bool {
    match event {
        Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => true,
        Event::Html(html) => html.as_ref() == "</a>" || html.starts_with("<a href="),
        _ => false,
    }
}

fn add_code_labels(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut labeled = Vec::with_capacity(events.len());
    let mut has_label = false;
    for event in events {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                if let Some(label) = code_block_label(&kind) {
                    labeled.push(Event::Html(label.into()));
                    has_label = true;
                }
                labeled.push(Event::Start(Tag::CodeBlock(kind)));
            }
            Event::End(TagEnd::CodeBlock) => {
                labeled.push(Event::End(TagEnd::CodeBlock));
                if has_label {
                    labeled.push(Event::Html("</div>".into()));
                    has_label = false;
                }
            }
            other => labeled.push(other),
        }
    }
    labeled
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

fn heading_text(events: &[Event<'_>]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(value),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
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
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                alt.push_str(&text);
            }
            Event::SoftBreak | Event::HardBreak => alt.push(' '),
            _ => {}
        }
    }
    let mut image = format!(
        "<img src=\"{}\" alt=\"{}\" loading=\"lazy\" decoding=\"async\"",
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
    use super::render;

    fn render_html(source: &str) -> String {
        render(source).html
    }

    #[test]
    fn adds_image_attributes_and_escapes_alt_text() {
        let html = render_html("![a **bold** & <bad>](image.png \"A title\")");
        assert!(html.contains("src=\"image.png\""), "{html}");
        assert!(html.contains("alt=\"a bold &amp; &lt;bad&gt;\""), "{html}");
        assert!(html.contains("loading=\"lazy\""), "{html}");
        assert!(html.contains("decoding=\"async\""), "{html}");
        assert!(html.contains("title=\"A title\""), "{html}");
    }

    #[test]
    fn preserves_non_image_markdown() {
        let html = render_html("**bold** and `code`\n\n```rust\nlet x = 1;\n```\n");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("let x = 1;"), "{html}");
    }

    #[test]
    fn shows_fenced_code_language_without_changing_code_markup() {
        let html =
            render_html("```tsx\nconst value = 1;\n```\n\n```js title=example\nalert(1);\n```\n");
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
    }

    #[test]
    fn leaves_code_blocks_without_language_unlabeled() {
        let html = render_html("```\nplain\n```\n\n    indented\n");
        assert!(!html.contains("code-language"), "{html}");
        assert!(!html.contains("code-block"), "{html}");
        assert_eq!(html.matches("<pre>").count(), 2);
    }

    #[test]
    fn escapes_code_language_as_html_text() {
        let html = render_html("```a<b&c\nvalue\n```\n");
        assert!(html.contains("a&lt;b&amp;c</span>"), "{html}");
        assert!(
            !html.contains("<span class=\"code-language\">a<b"),
            "{html}"
        );
    }

    #[test]
    fn adds_links_to_second_and_third_level_headings() {
        let html = render_html("# Title\n\n## Fiberとは\n\n### `useState` と Fiber\n");
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
    }

    #[test]
    fn unwraps_links_inside_headings() {
        let html = render_html("## [内部](other.md) と [外部](https://example.com)\n");
        assert!(
            html.contains(
                "<a class=\"heading-anchor\" href=\"#内部-と-外部\">内部 と 外部</a></h2>"
            ),
            "{html}"
        );
    }

    #[test]
    fn keeps_heading_ids_unique_after_normalization() {
        let html = render_html("## A B\n\n## A B\n\n### A-B\n\n## !!!\n\n## !!!\n");
        for id in ["a-b", "a-b-2", "a-b-3", "section", "section-2"] {
            assert!(html.contains(&format!("id=\"{id}\"")), "{html}");
            assert!(html.contains(&format!("href=\"#{id}\"")), "{html}");
        }
    }

    #[test]
    fn rewrites_relative_article_links_without_changing_other_urls() {
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
            let html = render_html(&format!("[article]({url})"));
            assert!(html.contains(&format!("href=\"{expected}\"")), "{html}");
        }
        let image = render_html("![image](other.md)");
        assert!(image.contains("src=\"other.md\""), "{image}");
    }

    #[test]
    fn opens_external_links_in_new_tabs() {
        let html = render_html(
            "[web](https://example.com/?a=1&b=2 \"A & B\") [cdn](//cdn.example.com) [local](next.md) [section](#top)",
        );
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
    }

    #[test]
    fn collects_rendered_internal_link_and_image_targets() {
        let rendered = render(concat!(
            "[next](next.md#x) ![photo](../img/a.png) [abs](/about) [top](#top)\n\n",
            "[web](https://example.com) <https://example.com/auto> <someone@example.com> ",
            "[mail](mailto:a@example.com)\n\n",
            "## [heading](gone.md) ![icon](icon.png)\n\n",
            "# [title](kept.md)\n",
        ));
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
    }
}
