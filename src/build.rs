use crate::{
    content::{self, Article},
    output::{self, Artifact},
};
use anyhow::{Context as _, Result, ensure};
use axum::http::Uri;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tera::{Context, Tera};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    title: String,
    description: String,
    site_url: String,
}

#[derive(Serialize)]
struct HomeView<'a> {
    site: &'a Config,
    description: &'a str,
    canonical_url: &'a str,
    entries: &'a [Article],
    css: &'a str,
}

#[derive(Serialize)]
struct ArticleView<'a> {
    site: &'a Config,
    description: &'a str,
    canonical_url: &'a str,
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
    let config_path = root.join("config.toml");
    let mut config: Config = toml::from_str(&read_text(&config_path)?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    ensure!(
        !config.title.trim().is_empty(),
        "config.toml: title must not be empty"
    );
    ensure!(
        !config.description.trim().is_empty(),
        "config.toml: description must not be empty"
    );
    config.site_url = validate_site_url(&config.site_url)?;
    let tera = load_templates(&root.join("templates"))?;
    let mut articles = load_articles(&root.join("content"))?;
    articles.sort_by(|left, right| {
        right
            .created_at_order
            .cmp(&left.created_at_order)
            .then_with(|| left.url.cmp(&right.url))
    });
    let mut templates = BTreeSet::from(["root.html"]);
    templates.extend(articles.iter().map(|article| article.template.as_str()));
    let styles = load_styles(root, &templates)?;
    let root_css = styles
        .get("root.html")
        .context("missing styles for root.html")?;
    let mut artifacts = Vec::with_capacity(articles.len() + 2);
    let home_url = config.site_url.as_str();
    artifacts.push(render(
        &tera,
        "root.html",
        &HomeView {
            site: &config,
            description: &config.description,
            canonical_url: home_url,
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
        let source = article.source.display().to_string();
        let canonical_url = format!("{}{url}", home_url.trim_end_matches('/'), url = article.url);
        artifacts.push(render(
            &tera,
            &article.template,
            &ArticleView {
                site: &config,
                description: &article.description,
                canonical_url: &canonical_url,
                article,
                content: &article.html,
                css,
            },
            article.output.clone(),
            &source,
            dev,
        )?);
    }
    artifacts.push(sitemap(home_url, &articles)?);
    let static_root = root.join("static");
    let assets = files(&static_root)?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
        .map(|path| {
            let relative = path.strip_prefix(&static_root)?.to_path_buf();
            let bytes =
                fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
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

fn validate_site_url(value: &str) -> Result<String> {
    ensure!(
        !value.contains('#'),
        "config.toml: site_url must not contain a fragment"
    );
    let uri: Uri = value
        .parse()
        .context("config.toml: site_url must be an absolute HTTP(S) URL")?;
    let scheme = uri
        .scheme_str()
        .context("config.toml: site_url must have a scheme")?;
    ensure!(
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"),
        "config.toml: site_url must use http or https"
    );
    let authority = uri
        .authority()
        .context("config.toml: site_url must have a host")?;
    ensure!(
        !authority.host().is_empty() && !authority.as_str().contains('@'),
        "config.toml: site_url must have a host without credentials"
    );
    ensure!(
        uri.path_and_query().is_none_or(|part| part.as_str() == "/"),
        "config.toml: site_url must point to the site root without a path or query"
    );
    Ok(format!("{}://{authority}/", scheme.to_ascii_lowercase()))
}

fn sitemap(base: &str, articles: &[Article]) -> Result<Artifact> {
    ensure!(
        articles.len() < 50_000,
        "sitemap.xml supports at most 50,000 URLs including the home page"
    );
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    let escaped_base = base.replace('&', "&amp;").replace('<', "&lt;");
    xml.push_str("  <url><loc>");
    xml.push_str(&escaped_base);
    xml.push_str("</loc></url>\n");
    for article in articles {
        xml.push_str("  <url><loc>");
        xml.push_str(escaped_base.trim_end_matches('/'));
        xml.push_str(&article.url);
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

fn load_styles(root: &Path, templates: &BTreeSet<&str>) -> Result<BTreeMap<String, String>> {
    let styles = root.join("styles");
    let common = read_text(&styles.join("common.css"))?;
    templates
        .iter()
        .map(|template| {
            let specific = styles.join(Path::new(template).with_extension("css"));
            let mut css = common.clone();
            if let Some(specific) = read_optional_text(&specific)? {
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

fn load_articles(root: &Path) -> Result<Vec<Article>> {
    files(root)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let relative = path.strip_prefix(root)?;
            content::parse(&read_text(&path)?, relative)
                .with_context(|| format!("cannot parse {}", path.display()))
        })
        .collect()
}

fn load_templates(root: &Path) -> Result<Tera> {
    let templates = files(root)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .map(|path| {
            let name = path
                .strip_prefix(root)?
                .to_str()
                .context("template path must be UTF-8")?
                .replace('\\', "/");
            Ok((name, read_text(&path)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut tera = Tera::default();
    tera.add_raw_templates(templates)
        .with_context(|| format!("cannot load templates in {}", root.display()))?;
    Ok(tera)
}

fn read_text(path: &Path) -> Result<String> {
    ensure!(
        fs::symlink_metadata(path)
            .with_context(|| format!("cannot inspect {}", path.display()))?
            .is_file(),
        "expected a regular file: {}",
        path.display()
    );
    fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

fn read_optional_text(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "expected a regular file: {}",
                path.display()
            );
            fs::read_to_string(path)
                .map(Some)
                .with_context(|| format!("cannot read {}", path.display()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", path.display())),
    }
}

fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let metadata =
        fs::symlink_metadata(root).with_context(|| format!("cannot inspect {}", root.display()))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "expected a real directory: {}",
        root.display()
    );
    let mut result = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("cannot read {}", root.display()))? {
        let entry = entry.with_context(|| format!("cannot read entry in {}", root.display()))?;
        let path = entry.path();
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "symlinks are not supported: {}",
            path.display()
        );
        if kind.is_dir() {
            result.extend(files(&path)?);
        } else {
            ensure!(
                kind.is_file(),
                "expected a regular file: {}",
                path.display()
            );
            result.push(path);
        }
    }
    result.sort();
    Ok(result)
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
        let site = Config {
            title: "Blog".to_owned(),
            description: "Blog articles".to_owned(),
            site_url: "http://127.0.0.1:3000/".to_owned(),
        };
        let articles = vec![content::parse(
            "+++\ncreated_at = 2026-09-17\ndescription = 'Post description'\n+++\n# Post",
            Path::new("post.md"),
        )?];
        let home = Context::from_serialize(&HomeView {
            site: &site,
            description: &site.description,
            canonical_url: &site.site_url,
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
            article,
            content: &article.html,
            css: "",
        })?;
        assert!(single.get("article").is_some());
        assert!(single.get("entries").is_none());
        Ok(())
    }
}
