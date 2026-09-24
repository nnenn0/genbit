use crate::{
    config::{Config, SiteUrl},
    content::{self, Article},
    input::SiteInput,
    output::{self, Artifact},
};
use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tera::{Context, Tera};

#[derive(Serialize)]
struct HomeView<'a> {
    site: &'a Config,
    description: &'a str,
    canonical_url: &'a str,
    json_ld: &'a str,
    entries: &'a [Article],
    css: &'a str,
}

#[derive(Serialize)]
struct ArticleView<'a> {
    site: &'a Config,
    description: &'a str,
    canonical_url: &'a str,
    json_ld: &'a str,
    article: &'a Article,
    content: &'a str,
    css: &'a str,
}

pub(crate) fn run(root: &Path) -> Result<usize> {
    run_with_mode(root, false)
}

pub(crate) fn run_dev(root: &Path) -> Result<usize> {
    run_with_mode(root, true)
}

fn run_with_mode(root: &Path, dev: bool) -> Result<usize> {
    let input = SiteInput::new(root);
    let config_path = root.join("config.toml");
    let config = Config::parse(&input.read_text(Path::new("config.toml"))?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    let tera = load_templates(&input)?;
    let mut articles = load_articles(&input)?;
    articles.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.route.url().cmp(right.route.url()))
    });
    let mut templates = BTreeSet::from(["root.html"]);
    templates.extend(articles.iter().map(|article| article.template.as_str()));
    let styles = load_styles(&input, &templates)?;
    let root_css = styles
        .get("root.html")
        .context("missing styles for root.html")?;
    let mut artifacts = Vec::with_capacity(articles.len() + 2);
    let home_url = config.site_url.as_str();
    let home_json_ld = website_json_ld(&config)?;
    artifacts.push(render(
        &tera,
        "root.html",
        &HomeView {
            site: &config,
            description: &config.description,
            canonical_url: home_url,
            json_ld: &home_json_ld,
            entries: &articles,
            css: root_css,
        },
        PathBuf::from("index.html"),
        "<generated home>",
        dev,
    )?);
    for article in &articles {
        let css = styles
            .get(&article.template)
            .with_context(|| format!("missing styles for template {}", article.template))?;
        let source = format!("content/{}", article.source.display());
        let canonical_url = config.site_url.join_root_path(article.route.url());
        let article_json_ld = article_json_ld(&config, article, &canonical_url)?;
        artifacts.push(render(
            &tera,
            &article.template,
            &ArticleView {
                site: &config,
                description: &article.description,
                canonical_url: &canonical_url,
                json_ld: &article_json_ld,
                article,
                content: &article.html,
                css,
            },
            article.route.output().to_path_buf(),
            &source,
            dev,
        )?);
    }
    artifacts.push(sitemap(&config.site_url, &articles)?);
    artifacts.push(robots(&config.site_url));
    let static_root = root.join("static");
    let assets = input
        .files(Path::new("static"))?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
        .map(|path| {
            let relative = path.strip_prefix(&static_root)?.to_path_buf();
            let site_relative = path.strip_prefix(root)?;
            let bytes = input.read_bytes(site_relative)?;
            Ok(Artifact {
                path: relative,
                bytes,
                source: path.display().to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    artifacts.extend(assets);
    output::publish(root, &artifacts)?;
    Ok(articles.len() + 1)
}

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

fn website_json_ld(config: &Config) -> Result<String> {
    json_ld(&WebsiteStructuredData {
        context: "https://schema.org",
        kind: "WebSite",
        name: &config.title,
        url: config.site_url.as_str(),
        description: &config.description,
        image: &config.og_image,
    })
}

fn article_json_ld(config: &Config, article: &Article, url: &str) -> Result<String> {
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

fn sitemap(base: &SiteUrl, articles: &[Article]) -> Result<Artifact> {
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
    Ok(Artifact {
        path: PathBuf::from("sitemap.xml"),
        bytes: xml.into_bytes(),
        source: "<generated sitemap>".to_owned(),
    })
}

fn robots(base: &SiteUrl) -> Artifact {
    Artifact {
        path: PathBuf::from("robots.txt"),
        bytes: format!(
            "User-agent: *\nAllow: /\n\nSitemap: {}\n",
            base.join_root_path("/sitemap.xml")
        )
        .into_bytes(),
        source: "<generated robots.txt>".to_owned(),
    }
}

fn load_styles(
    input: &SiteInput<'_>,
    templates: &BTreeSet<&str>,
) -> Result<BTreeMap<String, String>> {
    let common = input.read_text(Path::new("styles/common.css"))?;
    templates
        .iter()
        .map(|template| {
            let specific = Path::new("styles").join(Path::new(template).with_extension("css"));
            let mut css = common.clone();
            if let Some(specific) = input.read_optional_text(&specific)? {
                css.push('\n');
                css.push_str(&specific);
            }
            Ok(((*template).to_owned(), css))
        })
        .collect()
}

fn render(
    tera: &Tera,
    template: &str,
    view: &impl Serialize,
    output: PathBuf,
    source: &str,
    dev: bool,
) -> Result<Artifact> {
    let context = Context::from_serialize(view).context("cannot serialize template context")?;
    let mut html = tera
        .render(template, &context)
        .with_context(|| format!("cannot render {source} with template {template}"))?;
    if dev {
        html.push_str(crate::dev::RELOAD_SCRIPT);
    }
    let minified = minify_html::minify(
        html.as_bytes(),
        &minify_html::Cfg {
            minify_css: true,
            keep_html_and_head_opening_tags: true,
            ..minify_html::Cfg::default()
        },
    );
    Ok(Artifact {
        path: output,
        bytes: minified,
        source: source.to_owned(),
    })
}

fn load_articles(input: &SiteInput<'_>) -> Result<Vec<Article>> {
    let content_root = input.root().join("content");
    input
        .files(Path::new("content"))?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let relative = path.strip_prefix(&content_root)?;
            let site_relative = path.strip_prefix(input.root())?;
            content::parse(&input.read_text(site_relative)?, relative)
                .with_context(|| format!("cannot parse {}", path.display()))
        })
        .collect()
}

fn load_templates(input: &SiteInput<'_>) -> Result<Tera> {
    let template_root = input.root().join("templates");
    let templates = input
        .files(Path::new("templates"))?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .map(|path| {
            let name = path
                .strip_prefix(&template_root)?
                .to_str()
                .context("template path must be UTF-8")?
                .replace('\\', "/");
            let site_relative = path.strip_prefix(input.root())?;
            Ok((name, input.read_text(site_relative)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut tera = Tera::default();
    tera.add_raw_templates(templates)
        .with_context(|| format!("cannot load templates in {}", template_root.display()))?;
    Ok(tera)
}

#[cfg(test)]
mod tests {
    use super::{ArticleView, Config, HomeView};
    use crate::content;
    use anyhow::{Context as _, Result};
    use std::path::Path;
    use tera::Context;

    #[test]
    fn home_and_article_have_distinct_template_data() -> Result<()> {
        let site = Config::parse(
            "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'http://127.0.0.1:3000/'\nog_image = '/assets/img/ogp.png'\n",
        )?;
        let articles = vec![content::parse(
            "+++\ncreated_at = 2026-09-17\ndescription = 'Post description'\n+++\n# Post",
            Path::new("post.md"),
        )?];
        let home = Context::from_serialize(&HomeView {
            site: &site,
            description: &site.description,
            canonical_url: site.site_url.as_str(),
            json_ld: "{}",
            entries: &articles,
            css: "",
        })?;
        assert!(home.get("entries").is_some());
        assert!(home.get("article").is_none());

        let article = articles.first().context("test article missing")?;
        let single = Context::from_serialize(&ArticleView {
            site: &site,
            description: &article.description,
            canonical_url: "http://127.0.0.1:3000/post",
            json_ld: "{}",
            article,
            content: &article.html,
            css: "",
        })?;
        assert!(single.get("article").is_some());
        assert!(single.get("entries").is_none());
        Ok(())
    }
}
