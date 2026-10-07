use crate::{
    config::Config,
    content::{self, Article},
    input::SiteInput,
    output::Artifact,
    route::{self, TAGS_INDEX_URL},
    tags::{TagGroup, TagIndex, article_tags},
};
use anyhow::{Context as _, Result, ensure};
use jiff::Zoned;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tera::{Context, Tera};

pub(crate) struct Renderer {
    tera: Tera,
    styles: BTreeMap<String, String>,
    /// Whether `genbit dev` renders the pages, which then reload on rebuilds and may show drafts.
    preview: bool,
}

#[derive(Serialize)]
struct PublicArticle<'a> {
    title: &'a str,
    description: &'a str,
    url: &'a str,
    created_at: PublicTimestamp,
    updated_at: PublicTimestamp,
    tags: Vec<&'a str>,
    draft: bool,
}

impl<'a> From<&'a Article> for PublicArticle<'a> {
    fn from(article: &'a Article) -> Self {
        Self {
            title: article.title.as_str(),
            description: article.description.as_str(),
            url: article.route.url(),
            created_at: PublicTimestamp::from(&article.created_at),
            updated_at: PublicTimestamp::from(&article.updated_at),
            tags: article_tags(article).collect(),
            draft: article.draft,
        }
    }
}

#[derive(Serialize)]
struct PublicTimestamp {
    datetime: String,
    date: String,
    time: String,
}

impl From<&Zoned> for PublicTimestamp {
    fn from(value: &Zoned) -> Self {
        Self {
            datetime: content::rfc3339(value),
            date: value.strftime("%Y-%m-%d").to_string(),
            time: value.strftime("%H:%M").to_string(),
        }
    }
}

/// The variables every template receives, around those of its own view.
#[derive(Serialize)]
struct Page<'a, V> {
    site: &'a Config,
    css: &'a str,
    preview: bool,
    #[serde(flatten)]
    view: V,
}

#[derive(Serialize)]
struct HomeView<'a> {
    description: &'a str,
    canonical_url: &'a str,
    json_ld: &'a str,
    entries: Vec<PublicArticle<'a>>,
}

#[derive(Serialize)]
struct ArticleView<'a> {
    description: &'a str,
    canonical_url: &'a str,
    json_ld: &'a str,
    article: PublicArticle<'a>,
    content: String,
}

#[derive(Serialize)]
struct PublicTag<'a> {
    name: &'a str,
    url: String,
    count: usize,
}

#[derive(Serialize)]
struct TagsView<'a> {
    description: &'a str,
    canonical_url: String,
    json_ld: &'a str,
    tags: Vec<PublicTag<'a>>,
}

#[derive(Serialize)]
struct TagView<'a, 'b> {
    description: String,
    canonical_url: String,
    json_ld: &'a str,
    tag: &'a str,
    entries: Vec<PublicArticle<'b>>,
}

impl Renderer {
    pub(crate) fn load(input: &SiteInput<'_>, preview: bool) -> Result<Self> {
        let tera = load_templates(input)?;
        ensure!(
            tera.get_template_names().any(|name| name == "tags.html")
                && tera.get_template_names().any(|name| name == "tag.html"),
            "tag pages require templates/tags.html and templates/tag.html"
        );
        let templates = BTreeSet::from([
            "root.html",
            "page.html",
            "404.html",
            "tags.html",
            "tag.html",
        ]);
        let styles = load_styles(input, &templates)?;
        Ok(Self {
            tera,
            styles,
            preview,
        })
    }

    pub(crate) fn tags_index(
        &self,
        config: &Config,
        tags: &TagIndex<'_>,
        json_ld: &str,
    ) -> Result<Artifact> {
        let public_tags = tags
            .groups()
            .iter()
            .map(|group| PublicTag {
                name: group.name,
                url: group.url(),
                count: group.articles.len(),
            })
            .collect();
        self.render(
            config,
            "tags.html",
            TagsView {
                description: "記事のタグ一覧",
                canonical_url: config.site_url.join_root_path(TAGS_INDEX_URL),
                json_ld,
                tags: public_tags,
            },
            route::output_path(TAGS_INDEX_URL),
            "<generated tag index>",
        )
    }

    pub(crate) fn tag(
        &self,
        config: &Config,
        group: &TagGroup<'_>,
        json_ld: &str,
    ) -> Result<Artifact> {
        let tag = group.name;
        let url = group.url();
        let entries = group
            .articles
            .iter()
            .map(|article| PublicArticle::from(*article))
            .collect();
        self.render(
            config,
            "tag.html",
            TagView {
                description: format!("{tag} の記事一覧"),
                canonical_url: config.site_url.join_root_path(&url),
                json_ld,
                tag,
                entries,
            },
            route::output_path(&url),
            &format!("<generated tag {tag}>"),
        )
    }

    pub(crate) fn home(
        &self,
        config: &Config,
        articles: &[Article],
        json_ld: &str,
    ) -> Result<Artifact> {
        self.render(
            config,
            "root.html",
            HomeView {
                description: config.description.as_str(),
                canonical_url: config.site_url.as_str(),
                json_ld,
                entries: articles.iter().map(PublicArticle::from).collect(),
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
        self.render(
            config,
            "page.html",
            ArticleView {
                description: article.description.as_str(),
                canonical_url,
                json_ld,
                article: PublicArticle::from(article),
                content: article.content.to_fragment(),
            },
            article.route.output().to_path_buf(),
            &article.source.display().to_string(),
        )
    }

    pub(crate) fn not_found(&self, config: &Config) -> Result<Artifact> {
        self.render(
            config,
            "404.html",
            (),
            PathBuf::from("404.html"),
            "templates/404.html",
        )
    }

    fn render(
        &self,
        config: &Config,
        template: &str,
        view: impl Serialize,
        output: PathBuf,
        origin: &str,
    ) -> Result<Artifact> {
        let css = self
            .styles
            .get(template)
            .with_context(|| format!("missing styles for template {template}"))?;
        let page = Page {
            site: config,
            css,
            preview: self.preview,
            view,
        };
        let context =
            Context::from_serialize(&page).context("cannot serialize template context")?;
        let mut html = self
            .tera
            .render(template, &context)
            .with_context(|| format!("cannot render {origin} with template {template}"))?;
        if self.preview {
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
        Ok(Artifact::generated(output, minified, origin))
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
    let template_root = Path::new("templates");
    let templates = input
        .files(template_root)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "html"))
        .map(|path| {
            let site_relative = template_root.join(&path);
            let name = crate::route::slash_path(&path)
                .with_context(|| format!("invalid template path {}", site_relative.display()))?;
            Ok((name, input.read_text(&site_relative)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut tera = Tera::default();
    tera.add_raw_templates(templates).with_context(|| {
        format!(
            "cannot load templates in {}",
            input.root().join(template_root).display()
        )
    })?;
    Ok(tera)
}

#[cfg(test)]
mod tests {
    use super::{ArticleView, HomeView, Page, PublicArticle};
    use crate::{config::Config, content};
    use anyhow::{Context as _, Result};
    use std::path::Path;
    use tera::Context;

    #[test]
    fn template_views_expose_only_the_documented_article_fields() -> Result<()> {
        let site = Config::parse(
            "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'http://127.0.0.1:3000/'\nog_image = '/assets/site/ogp.png'\ntimezone = 'Asia/Tokyo'\n",
        )?;
        let articles = [content::parse(
            "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Post",
            content::ContentDir::Content,
            Path::new("post/index.md"),
            &site.timezone,
            |_, _| Ok(None),
        )?];
        let home = Context::from_serialize(&Page {
            site: &site,
            css: "",
            preview: false,
            view: HomeView {
                description: site.description.as_str(),
                canonical_url: site.site_url.as_str(),
                json_ld: "{}",
                entries: articles.iter().map(PublicArticle::from).collect(),
            },
        })?;
        assert!(home.get("site").is_some() && home.get("css").is_some());
        assert_eq!(
            home.get("preview").and_then(tera::Value::as_bool),
            Some(false)
        );
        assert!(home.get("entries").is_some());
        assert!(home.get("article").is_none());

        let article = articles.first().context("test article missing")?;
        let public = PublicArticle::from(article);
        let single = Context::from_serialize(&Page {
            site: &site,
            css: "",
            preview: false,
            view: ArticleView {
                description: article.description.as_str(),
                canonical_url: "http://127.0.0.1:3000/post",
                json_ld: "{}",
                article: PublicArticle::from(article),
                content: article.content.to_fragment(),
            },
        })?;
        assert!(single.get("article").is_some());
        assert!(single.get("entries").is_none());
        let value = serde_json::to_value(&public)?;
        let fields = value.as_object().context("article view is not an object")?;
        assert_eq!(fields.len(), 7);
        assert_eq!(fields.get("draft"), Some(&serde_json::json!(false)));
        assert_eq!(fields.get("tags"), Some(&serde_json::json!(["untagged"])));
        assert_eq!(
            fields.get("url").and_then(serde_json::Value::as_str),
            Some("/post/")
        );
        let timestamp = serde_json::json!({
            "datetime": "2026-09-17T10:30:00+09:00",
            "date": "2026-09-17",
            "time": "10:30",
        });
        assert_eq!(fields.get("created_at"), Some(&timestamp));
        assert_eq!(fields.get("updated_at"), Some(&timestamp));
        Ok(())
    }
}
