use crate::{
    config::{Config, SiteUrl},
    content::Article,
    output::Artifact,
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::path::PathBuf;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    date_published: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date_modified: Option<&'a str>,
}

pub(crate) fn website_json_ld(config: &Config) -> Result<String> {
    json_ld(&WebsiteStructuredData {
        context: "https://schema.org",
        kind: "WebSite",
        name: &config.title,
        url: config.site_url.as_str(),
        description: &config.description,
        image: &config.og_image,
    })
}

pub(crate) fn article_json_ld(config: &Config, article: &Article, url: &str) -> Result<String> {
    let published = article.created_at.date_string();
    let modified = article.updated_at.map(|date| date.to_string());
    json_ld(&ArticleStructuredData {
        context: "https://schema.org",
        kind: "BlogPosting",
        headline: &article.title,
        description: &article.description,
        url,
        image: &config.og_image,
        date_published: Some(&published),
        date_modified: modified.as_deref(),
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

pub(crate) fn sitemap(base: &SiteUrl, articles: &[Article]) -> Result<Artifact> {
    ensure!(
        articles.len() < 50_000,
        "sitemap.xml supports at most 50,000 URLs including the home page"
    );
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    let escaped_base = base.as_str().replace('&', "&amp;").replace('<', "&lt;");
    xml.push_str("  <url><loc>");
    xml.push_str(&escaped_base);
    xml.push_str("</loc></url>\n");
    for article in articles {
        xml.push_str("  <url><loc>");
        xml.push_str(
            &base
                .join_root_path(article.route.url())
                .replace('&', "&amp;")
                .replace('<', "&lt;"),
        );
        xml.push_str("</loc></url>\n");
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
    use super::json_ld;
    use anyhow::Result;

    #[test]
    fn json_ld_cannot_close_script_element() -> Result<()> {
        let encoded = json_ld(&"</script><script>alert('&')</script>")?;
        assert!(!encoded.contains("</script>"));
        assert!(encoded.contains("\\u003c/script\\u003e"));
        assert!(encoded.contains("\\u0026"));
        Ok(())
    }
}
