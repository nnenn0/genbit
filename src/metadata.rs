use crate::{
    config::{Config, SiteUrl},
    content::{self, Article},
    output::Artifact,
    route::{FEED_URL, TAGS_INDEX_URL},
    tags::TagIndex,
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{iter, path::PathBuf};

const FEED_ITEM_LIMIT: usize = 20;

#[derive(Serialize)]
struct WebsiteStructuredData<'a> {
    #[serde(rename = "@context")]
    context: &'a str,
    #[serde(rename = "@type")]
    kind: &'a str,
    name: &'a str,
    url: &'a str,
    description: &'a str,
    image: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArticleStructuredData<'a> {
    #[serde(rename = "@context")]
    context: &'a str,
    #[serde(rename = "@type")]
    kind: &'a str,
    headline: &'a str,
    description: &'a str,
    url: &'a str,
    image: &'a str,
    date_published: &'a str,
    date_modified: &'a str,
}

pub(crate) fn website_json_ld(config: &Config) -> Result<String> {
    json_ld(&WebsiteStructuredData {
        context: "https://schema.org",
        kind: "WebSite",
        name: config.title.as_str(),
        url: config.site_url.as_str(),
        description: config.description.as_str(),
        image: &config.og_image,
    })
}

pub(crate) fn article_json_ld(config: &Config, article: &Article, url: &str) -> Result<String> {
    let published = content::rfc3339(&article.created_at);
    let modified = content::rfc3339(&article.updated_at);
    json_ld(&ArticleStructuredData {
        context: "https://schema.org",
        kind: "BlogPosting",
        headline: article.title.as_str(),
        description: article.description.as_str(),
        url,
        image: &config.og_image,
        date_published: &published,
        date_modified: &modified,
    })
}

fn json_ld(value: &impl Serialize) -> Result<String> {
    let serialized = serde_json::to_string(value).context("cannot serialize structured data")?;
    // Prevent user-provided text from closing the surrounding script element.
    Ok(serialized
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

/// A `<url>` entry of the sitemap. Only articles have a last modification time.
struct SitemapUrl {
    loc: String,
    lastmod: Option<String>,
}

pub(crate) fn sitemap(
    base: &SiteUrl,
    articles: &[Article],
    tags: &TagIndex<'_>,
) -> Result<Artifact> {
    let listing = |url: String| SitemapUrl {
        loc: url,
        lastmod: None,
    };
    let urls = iter::once(listing(base.as_str().to_owned()))
        .chain(articles.iter().map(|article| SitemapUrl {
            loc: base.join_root_path(article.route.url()),
            lastmod: Some(content::rfc3339(&article.updated_at)),
        }))
        .chain(iter::once(listing(base.join_root_path(TAGS_INDEX_URL))))
        .chain(
            tags.groups()
                .iter()
                .map(|group| listing(base.join_root_path(&group.url()))),
        )
        .collect::<Vec<_>>();
    ensure!(
        urls.len() <= 50_000,
        "sitemap.xml supports at most 50,000 URLs including generated pages"
    );
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for SitemapUrl { loc, lastmod } in urls {
        xml.push_str("  <url><loc>");
        xml.push_str(&xml_escape(&loc));
        xml.push_str("</loc>");
        if let Some(lastmod) = lastmod {
            xml.push_str("<lastmod>");
            xml.push_str(&lastmod);
            xml.push_str("</lastmod>");
        }
        xml.push_str("</url>\n");
    }
    xml.push_str("</urlset>\n");
    ensure!(
        xml.len() <= 50 * 1024 * 1024,
        "sitemap.xml exceeds the 50 MB uncompressed limit"
    );
    Ok(Artifact::generated(
        PathBuf::from("sitemap.xml"),
        xml.into_bytes(),
        "<generated sitemap>",
    ))
}

/// Builds an RSS 2.0 feed of the newest articles. `articles` must already be
/// sorted newest first.
pub(crate) fn feed(config: &Config, articles: &[Article]) -> Artifact {
    Artifact::generated(
        PathBuf::from(FEED_URL.trim_start_matches('/')),
        feed_xml(config, articles).into_bytes(),
        "<generated feed>",
    )
}

fn feed_xml(config: &Config, articles: &[Article]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\">\n  <channel>\n",
    );
    push_element(&mut xml, "    ", "title", config.title.as_str());
    push_element(&mut xml, "    ", "link", config.site_url.as_str());
    push_element(&mut xml, "    ", "description", config.description.as_str());
    xml.push_str("    <atom:link href=\"");
    xml.push_str(&xml_escape(&config.site_url.join_root_path(FEED_URL)));
    xml.push_str("\" rel=\"self\" type=\"application/rss+xml\"/>\n");
    for article in articles.iter().take(FEED_ITEM_LIMIT) {
        let url = config.site_url.join_root_path(article.route.url());
        xml.push_str("    <item>\n");
        push_element(&mut xml, "      ", "title", article.title.as_str());
        push_element(&mut xml, "      ", "link", &url);
        push_element(&mut xml, "      ", "guid", &url);
        push_element(
            &mut xml,
            "      ",
            "pubDate",
            &article
                .created_at
                .strftime("%a, %d %b %Y %H:%M:%S %z")
                .to_string(),
        );
        // RSS item descriptions are HTML inside XML. These XML entities are also
        // valid HTML: escape the text once for HTML, then again when writing XML.
        let description = xml_escape(article.description.as_str());
        push_element(&mut xml, "      ", "description", &description);
        xml.push_str("    </item>\n");
    }
    xml.push_str("  </channel>\n</rss>\n");
    xml
}

fn push_element(xml: &mut String, indent: &str, name: &str, text: &str) {
    xml.push_str(indent);
    xml.push('<');
    xml.push_str(name);
    xml.push('>');
    xml.push_str(&xml_escape(text));
    xml.push_str("</");
    xml.push_str(name);
    xml.push_str(">\n");
}

fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

pub(crate) fn robots(base: &SiteUrl) -> Artifact {
    Artifact::generated(
        PathBuf::from("robots.txt"),
        format!(
            "User-agent: *\nAllow: /\n\nSitemap: {}\n",
            base.join_root_path("/sitemap.xml")
        )
        .into_bytes(),
        "<generated robots.txt>",
    )
}

#[cfg(test)]
mod tests {
    use super::{feed_xml, json_ld};
    use crate::{config::Config, content};
    use anyhow::Result;
    use std::path::Path;

    #[test]
    fn json_ld_cannot_close_script_element() -> Result<()> {
        let encoded = json_ld(&"</script><script>alert('&')</script>")?;
        assert!(!encoded.contains("</script>"));
        assert!(encoded.contains("\\u003c/script\\u003e"));
        assert!(encoded.contains("\\u0026"));
        Ok(())
    }

    #[test]
    fn feed_lists_newest_twenty_articles_with_escaped_text() -> Result<()> {
        let config = Config::parse(
            "title = 'A & B <Blog>'\ndescription = 'Posts \"quoted\"'\nsite_url = 'https://example.com/'\nog_image = '/card.png'\ntimezone = 'Asia/Tokyo'\n",
        )?;
        let articles = (0..21)
            .rev()
            .map(|day| {
                content::parse(
                    &format!(
                        "+++\ntitle = 'Post {day} & <b>'\ncreated_at = 2026-01-{:02} 08:00\nupdated_at = 2026-02-01 00:00\ndescription = \"It's > 1\"\n+++\n",
                        day + 1
                    ),
                    Path::new(&format!("entries/post-{day}/index.md")),
                    &config.timezone,
                    |_, _| Ok(None),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let xml = feed_xml(&config, &articles);
        assert!(
            xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\"")
        );
        assert!(
            xml.contains("<title>A &amp; B &lt;Blog&gt;</title>"),
            "{xml}"
        );
        assert!(xml.contains("<link>https://example.com/</link>"), "{xml}");
        assert!(
            xml.contains("<description>Posts &quot;quoted&quot;</description>"),
            "{xml}"
        );
        assert!(
            xml.contains("<atom:link href=\"https://example.com/feed.xml\" rel=\"self\""),
            "{xml}"
        );
        assert_eq!(xml.matches("<item>").count(), 20);
        assert!(xml.contains(
            "<title>Post 20 &amp; &lt;b&gt;</title>\n      <link>https://example.com/entries/post-20/</link>\n      <guid>https://example.com/entries/post-20/</guid>\n      <pubDate>Wed, 21 Jan 2026 08:00:00 +0900</pubDate>\n      <description>It&amp;apos;s &amp;gt; 1</description>"
        ), "{xml}");
        assert!(xml.contains("<title>Post 1 &amp;"), "{xml}");
        assert!(!xml.contains("<title>Post 0 &amp;"), "{xml}");
        assert!(!xml.contains("Feb 2026"), "{xml}");
        let newest = xml.find("Post 20").unwrap_or(usize::MAX);
        let older = xml.find("Post 19").unwrap_or(0);
        assert!(newest < older, "{xml}");
        Ok(())
    }
}
