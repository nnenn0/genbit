use pulldown_cmark::{Event, Parser, Tag, TagEnd, html};

pub(crate) fn description(source: &str) -> Option<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut in_paragraph = false;
    let mut list_depth = 0;
    let mut image_depth = 0;
    for event in Parser::new(source) {
        match event {
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::End(TagEnd::List(_)) => list_depth -= 1,
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth -= 1,
            Event::Start(Tag::Paragraph) if list_depth == 0 => {
                in_paragraph = true;
                current.clear();
            }
            Event::End(TagEnd::Paragraph) if in_paragraph => {
                in_paragraph = false;
                let paragraph = current.split_whitespace().collect::<Vec<_>>().join(" ");
                if !paragraph.is_empty() {
                    paragraphs.push(paragraph);
                    if paragraphs.len() == 2
                        || paragraphs
                            .iter()
                            .map(|part| part.chars().count())
                            .sum::<usize>()
                            >= 80
                    {
                        break;
                    }
                }
            }
            Event::Text(text) | Event::Code(text) if in_paragraph && image_depth == 0 => {
                current.push_str(&text);
            }
            Event::SoftBreak | Event::HardBreak if in_paragraph => current.push(' '),
            _ => {}
        }
    }
    (!paragraphs.is_empty()).then(|| paragraphs.join(" "))
}

pub(crate) fn render(source: &str) -> String {
    let mut events = Parser::new(source);
    let transformed = std::iter::from_fn(move || {
        let event = events.next()?;
        match event {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => Some(Event::Html(
                image_html(&mut events, &dest_url, &title).into(),
            )),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => Some(Event::Start(Tag::Link {
                link_type,
                dest_url: article_url(&dest_url).map_or(dest_url, Into::into),
                title,
                id,
            })),
            other => Some(other),
        }
    });
    let mut output = String::new();
    html::push_html(&mut output, transformed);
    output
}

fn article_url(url: &str) -> Option<String> {
    let boundary = url.find(['?', '#']).unwrap_or(url.len());
    let (path, suffix) = url.split_at(boundary);
    if path.starts_with('/') || path.contains(':') {
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
    use super::{description, render};

    #[test]
    fn description_uses_intro_prose_without_lists_images_or_code_blocks() {
        let markdown = "# Title\n\n## Intro\n\nShort [linked](next.md) intro.\n\n- Table of contents\n\n![Alt](image.png)\n\nMore detail with `inline code`.\n\n```rust\nignored code\n```\n\n## Next\n\nLater text.\n";
        assert_eq!(
            description(markdown).as_deref(),
            Some("Short linked intro. More detail with inline code.")
        );
        assert_eq!(description("# Title\n\n- Item\n\n```\ncode\n```"), None);
    }

    #[test]
    fn adds_image_attributes_and_escapes_alt_text() {
        let html = render("![a **bold** & <bad>](image.png \"A title\")");
        assert!(html.contains("src=\"image.png\""), "{html}");
        assert!(html.contains("alt=\"a bold &amp; &lt;bad&gt;\""), "{html}");
        assert!(html.contains("loading=\"lazy\""), "{html}");
        assert!(html.contains("decoding=\"async\""), "{html}");
        assert!(html.contains("title=\"A title\""), "{html}");
    }

    #[test]
    fn preserves_non_image_markdown() {
        let html = render("**bold** and `code`\n\n```rust\nlet x = 1;\n```\n");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("let x = 1;"), "{html}");
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
            let html = render(&format!("[article]({url})"));
            assert!(html.contains(&format!("href=\"{expected}\"")), "{html}");
        }
        let image = render("![image](other.md)");
        assert!(image.contains("src=\"other.md\""), "{image}");
    }
}
