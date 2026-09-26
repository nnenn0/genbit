use crate::{
    config::Config,
    content::Article,
    input::SiteInput,
    output::Artifact,
    route::{TAGS_INDEX_URL, UNTAGGED_TAG, tag_url},
};
use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tera::{Context, Tera};

pub(crate) struct Renderer<'a> {
    tera: Tera,
    styles: BTreeMap<String, String>,
    reload_script: Option<&'a str>,
}

#[derive(Serialize)]
struct PublicArticle<'a> {
    title: &'a str,
    description: &'a str,
    url: &'a str,
    created_at: String,
    updated_at: String,
    tags: Vec<&'a str>,
}

impl<'a> From<&'a Article> for PublicArticle<'a> {
    fn from(article: &'a Article) -> Self {
        Self {
            title: &article.title,
            description: &article.description,
            url: article.route.url(),
            created_at: article.created_at.date_string(),
            updated_at: article.updated_at.date_string(),
            tags: if article.tags.is_empty() {
                vec![UNTAGGED_TAG]
            } else {
                article.tags.iter().map(String::as_str).collect()
            },
        }
    }
}

#[derive(Serialize)]
struct HomeView<'a, 'b> {
    site: &'a Config,
    description: &'a str,
    canonical_url: &'a str,
    json_ld: &'a str,
    entries: &'a [PublicArticle<'b>],
    css: &'a str,
}

#[derive(Serialize)]
struct ArticleView<'a, 'b> {
    site: &'a Config,
    description: &'a str,
    canonical_url: &'a str,
    json_ld: &'a str,
    article: &'a PublicArticle<'b>,
    content: &'a str,
    css: &'a str,
}

#[derive(Serialize)]
struct NotFoundView<'a> {
    site: &'a Config,
    css: &'a str,
}

#[derive(Serialize)]
struct PublicTag<'a> {
    name: &'a str,
    url: String,
    count: usize,
}

#[derive(Serialize)]
struct TagsView<'a> {
    site: &'a Config,
    description: &'a str,
    canonical_url: String,
    json_ld: &'a str,
    tags: Vec<PublicTag<'a>>,
    css: &'a str,
}

#[derive(Serialize)]
struct TagView<'a, 'b> {
    site: &'a Config,
    description: String,
    canonical_url: String,
    json_ld: &'a str,
    tag: &'a str,
    entries: Vec<PublicArticle<'b>>,
    css: &'a str,
}

impl<'a> Renderer<'a> {
    pub(crate) fn load(
        input: &SiteInput<'_>,
        articles: &[Article],
        reload_script: Option<&'a str>,
    ) -> Result<Self> {
        let tera = load_templates(input)?;
        ensure!(
            tera.get_template_names().any(|name| name == "tags.html")
                && tera.get_template_names().any(|name| name == "tag.html"),
            "tag pages require templates/tags.html and templates/tag.html"
        );
        let mut templates = BTreeSet::from(["root.html", "404.html", "tags.html", "tag.html"]);
        templates.extend(articles.iter().map(|article| article.template.as_str()));
        let styles = load_styles(input, &templates)?;
        Ok(Self {
            tera,
            styles,
            reload_script,
        })
    }

    pub(crate) fn tags_index(
        &self,
        config: &Config,
        tags: &BTreeMap<String, Vec<&Article>>,
        untagged_count: usize,
        json_ld: &str,
    ) -> Result<Artifact> {
        let css = self
            .styles
            .get("tags.html")
            .context("missing styles for tags.html")?;
        let mut public_tags: Vec<PublicTag<'_>> = tags
            .iter()
            .map(|(name, articles)| PublicTag {
                name,
                url: tag_url(name),
                count: articles.len(),
            })
            .collect();
        if untagged_count > 0 {
            public_tags.push(PublicTag {
                name: UNTAGGED_TAG,
                url: tag_url(UNTAGGED_TAG),
                count: untagged_count,
            });
        }
        self.render(
            "tags.html",
            &TagsView {
                site: config,
                description: "記事のタグ一覧",
                canonical_url: config.site_url.join_root_path(TAGS_INDEX_URL),
                json_ld,
                tags: public_tags,
                css,
            },
            PathBuf::from("tags/index.html"),
            "<generated tag index>",
        )
    }

    pub(crate) fn tag(
        &self,
        config: &Config,
        tag: &str,
        articles: &[&Article],
        json_ld: &str,
    ) -> Result<Artifact> {
        self.tag_page(config, tag, articles, json_ld)
    }

    pub(crate) fn untagged(
        &self,
        config: &Config,
        articles: &[&Article],
        json_ld: &str,
    ) -> Result<Artifact> {
        self.tag_page(config, UNTAGGED_TAG, articles, json_ld)
    }

    fn tag_page(
        &self,
        config: &Config,
        tag: &str,
        articles: &[&Article],
        json_ld: &str,
    ) -> Result<Artifact> {
        let css = self
            .styles
            .get("tag.html")
            .context("missing styles for tag.html")?;
        let entries = articles
            .iter()
            .map(|article| PublicArticle::from(*article))
            .collect();
        self.render(
            "tag.html",
            &TagView {
                site: config,
                description: format!("{tag} の記事一覧"),
                canonical_url: config.site_url.join_root_path(&tag_url(tag)),
                json_ld,
                tag,
                entries,
                css,
            },
            PathBuf::from("tags").join(tag).join("index.html"),
            &format!("<generated tag {tag}>"),
        )
    }

    pub(crate) fn home(
        &self,
        config: &Config,
        articles: &[Article],
        json_ld: &str,
    ) -> Result<Artifact> {
        let entries = articles.iter().map(PublicArticle::from).collect::<Vec<_>>();
        let css = self
            .styles
            .get("root.html")
            .context("missing styles for root.html")?;
        self.render(
            "root.html",
            &HomeView {
                site: config,
                description: &config.description,
                canonical_url: config.site_url.as_str(),
                json_ld,
                entries: &entries,
                css,
            },
            PathBuf::from("index.html"),
            "<generated home>",
        )
    }

    pub(crate) fn article(
        &self,
        config: &Config,
        article: &Article,
        canonical_url: &str,
        json_ld: &str,
    ) -> Result<Artifact> {
        let css = self
            .styles
            .get(&article.template)
            .with_context(|| format!("missing styles for template {}", article.template))?;
        let public = PublicArticle::from(article);
        self.render(
            &article.template,
            &ArticleView {
                site: config,
                description: &article.description,
                canonical_url,
                json_ld,
                article: &public,
                content: &article.html,
                css,
            },
            article.route.output().to_path_buf(),
            &format!("content/{}", article.source.display()),
        )
    }

    pub(crate) fn not_found(&self, config: &Config) -> Result<Artifact> {
        let css = self
            .styles
            .get("404.html")
            .context("missing styles for 404.html")?;
        self.render(
            "404.html",
            &NotFoundView { site: config, css },
            PathBuf::from("404.html"),
            "templates/404.html",
        )
    }

    fn render(
        &self,
        template: &str,
        view: &impl Serialize,
        output: PathBuf,
        source: &str,
    ) -> Result<Artifact> {
        let context = Context::from_serialize(view).context("cannot serialize template context")?;
        let mut html = self
            .tera
            .render(template, &context)
            .with_context(|| format!("cannot render {source} with template {template}"))?;
        if let Some(script) = self.reload_script {
            html.push_str(script);
        }
        let minified = minify_html::minify(
            html.as_bytes(),
            &minify_html::Cfg {
                minify_css: true,
                keep_html_and_head_opening_tags: true,
                ..minify_html::Cfg::default()
            },
        );
        Ok(Artifact::generated(output, minified, source))
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
    use super::{ArticleView, HomeView, PublicArticle};
    use crate::{config::Config, content};
    use anyhow::{Context as _, Result};
    use std::path::Path;
    use tera::Context;

    #[test]
    fn template_views_expose_only_the_documented_article_fields() -> Result<()> {
        let site = Config::parse(
            "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'http://127.0.0.1:3000/'\nog_image = '/assets/img/ogp.png'\n",
        )?;
        let articles = [content::parse(
            "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Post",
            Path::new("post.md"),
        )?];
        let entries = articles.iter().map(PublicArticle::from).collect::<Vec<_>>();
        let home = Context::from_serialize(&HomeView {
            site: &site,
            description: &site.description,
            canonical_url: site.site_url.as_str(),
            json_ld: "{}",
            entries: &entries,
            css: "",
        })?;
        assert!(home.get("entries").is_some());
        assert!(home.get("article").is_none());

        let article = articles.first().context("test article missing")?;
        let public = PublicArticle::from(article);
        let single = Context::from_serialize(&ArticleView {
            site: &site,
            description: &article.description,
            canonical_url: "http://127.0.0.1:3000/post",
            json_ld: "{}",
            article: &public,
            content: &article.html,
            css: "",
        })?;
        assert!(single.get("article").is_some());
        assert!(single.get("entries").is_none());
        let value = serde_json::to_value(&public)?;
        let fields = value.as_object().context("article view is not an object")?;
        assert_eq!(fields.len(), 6);
        assert_eq!(fields.get("tags"), Some(&serde_json::json!(["untagged"])));
        assert_eq!(
            fields.get("url").and_then(serde_json::Value::as_str),
            Some("/post")
        );
        assert_eq!(
            fields.get("created_at").and_then(serde_json::Value::as_str),
            Some("2026-09-17")
        );
        assert_eq!(
            fields.get("updated_at").and_then(serde_json::Value::as_str),
            Some("2026-09-17")
        );
        Ok(())
    }
}
