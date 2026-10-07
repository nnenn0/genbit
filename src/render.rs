use crate::{
    config::Config,
    content::{self, Article},
    input::SiteInput,
    output::Artifact,
    route::{self, TAGS_INDEX_URL},
    tags::{TagGroup, TagIndex, article_tags},
    views::{Page, Views},
};
use anyhow::{Context as _, Result};
use bitview::{Html, Value};
use jiff::Zoned;
use std::path::PathBuf;

pub(crate) struct Renderer {
    views: Views,
}

impl Renderer {
    pub(crate) fn load(input: &SiteInput<'_>) -> Result<Self> {
        Ok(Self {
            views: Views::load(input)?,
        })
    }

    pub(crate) fn tags_index(
        &self,
        config: &Config,
        tags: &TagIndex<'_>,
        json_ld: &str,
    ) -> Result<Artifact> {
        let tags = tags
            .groups()
            .iter()
            .map(|group| {
                Value::record([
                    ("name", Value::from(group.name)),
                    ("url", Value::from(group.url())),
                    ("count", Value::from(group.articles.len().to_string())),
                ])
            })
            .collect::<Vec<_>>();
        let mut variables = indexed(&config.site_url.join_root_path(TAGS_INDEX_URL), json_ld)?;
        variables.push(("tags", Value::from(tags)));
        self.render(
            config,
            Page::Tags,
            variables,
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
        let mut variables = indexed(&config.site_url.join_root_path(&url), json_ld)?;
        variables.push(("tag", Value::from(tag)));
        variables.push(("entries", entries(group.articles.iter().copied())));
        self.render(
            config,
            Page::Tag,
            variables,
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
        let mut variables = indexed(config.site_url.as_str(), json_ld)?;
        variables.push(("entries", entries(articles)));
        self.render(
            config,
            Page::Root,
            variables,
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
        let mut variables = indexed(canonical_url, json_ld)?;
        variables.push(("article", entry(article)));
        variables.push(("content", Value::from(article.content.clone())));
        self.render(
            config,
            Page::Article,
            variables,
            article.route.output().to_path_buf(),
            &article.source.display().to_string(),
        )
    }

    pub(crate) fn not_found(&self, config: &Config) -> Result<Artifact> {
        self.render(
            config,
            Page::NotFound,
            Vec::new(),
            PathBuf::from("404.html"),
            "<generated 404>",
        )
    }

    fn render(
        &self,
        config: &Config,
        page: Page,
        variables: Vec<(&'static str, Value)>,
        output: PathBuf,
        origin: &str,
    ) -> Result<Artifact> {
        let style = self
            .views
            .style(page)
            .with_context(|| format!("missing styles for {}", page.name()))?;
        let mut all = vec![
            ("site", site(config)),
            ("style", Value::from(style.clone())),
        ];
        all.extend(variables);
        let html = self
            .views
            .program
            .render(page.name(), Value::record(all))
            .and_then(|html| html.to_document())
            .with_context(|| format!("cannot render {origin} with {}", page.file()))?;
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

fn site(config: &Config) -> Value {
    Value::record([
        ("title", Value::from(config.title.as_str())),
        ("description", Value::from(config.description.as_str())),
        ("url", Value::from(config.site_url.as_str())),
        ("og-image", Value::from(config.og_image.as_str())),
    ])
}

fn indexed(canonical_url: &str, json_ld: &str) -> Result<Vec<(&'static str, Value)>> {
    Ok(vec![
        ("canonical-url", Value::from(canonical_url)),
        (
            "json-ld",
            Value::from(Html::json("application/ld+json", json_ld)?),
        ),
    ])
}

fn entries<'a>(articles: impl IntoIterator<Item = &'a Article>) -> Value {
    Value::from(articles.into_iter().map(entry).collect::<Vec<_>>())
}

fn entry(article: &Article) -> Value {
    let tags = article_tags(article)
        .map(|tag| {
            Value::record([
                ("name", Value::from(tag)),
                ("url", Value::from(route::tag_url(tag))),
            ])
        })
        .collect::<Vec<_>>();
    Value::record([
        ("title", Value::from(article.title.as_str())),
        ("description", Value::from(article.description.as_str())),
        ("url", Value::from(article.route.url())),
        ("created-at", timestamp(&article.created_at)),
        ("updated-at", timestamp(&article.updated_at)),
        ("tags", Value::from(tags)),
        ("draft", Value::from(article.draft)),
    ])
}

fn timestamp(value: &Zoned) -> Value {
    Value::record([
        ("datetime", Value::from(content::rfc3339(value))),
        ("date", Value::from(value.strftime("%Y-%m-%d").to_string())),
    ])
}

#[cfg(test)]
mod tests {
    use super::{entry, site};
    use crate::{config::Config, content};
    use anyhow::{Context as _, Result};
    use bitview::Value;
    use std::path::Path;

    fn field_names(value: &Value) -> Vec<&str> {
        match value {
            Value::Record(fields) => fields.iter().map(|(name, _)| name.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
        match value {
            Value::Record(fields) => fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    #[test]
    fn views_see_only_the_documented_fields() -> Result<()> {
        let config = Config::parse(
            "title = 'Blog'\ndescription = 'Blog articles'\nsite_url = 'http://127.0.0.1:3000/'\nog_image = '/assets/site/ogp.png'\ntimezone = 'Asia/Tokyo'\n",
        )?;
        assert_eq!(
            field_names(&site(&config)),
            ["title", "description", "url", "og-image"]
        );
        let article = content::parse(
            "+++\ncreated_at = 2026-09-17 10:30\nupdated_at = 2026-09-17 10:30\ndescription = 'Post description'\n+++\n# Post",
            content::ContentDir::Content,
            Path::new("post/index.md"),
            &config.timezone,
            |_, _| Ok(None),
        )?;
        let entry = entry(&article);
        assert_eq!(
            field_names(&entry),
            [
                "title",
                "description",
                "url",
                "created-at",
                "updated-at",
                "tags",
                "draft"
            ]
        );
        assert_eq!(field(&entry, "url"), Some(&Value::from("/post/")));
        assert_eq!(field(&entry, "draft"), Some(&Value::from(false)));
        let timestamp = Value::record([
            ("datetime", Value::from("2026-09-17T10:30:00+09:00")),
            ("date", Value::from("2026-09-17")),
        ]);
        assert_eq!(field(&entry, "created-at"), Some(&timestamp));
        assert_eq!(field(&entry, "updated-at"), Some(&timestamp));
        let tags = field(&entry, "tags").context("missing tags")?;
        assert_eq!(
            tags,
            &Value::from(vec![Value::record([
                ("name", Value::from("untagged")),
                ("url", Value::from("/tags/untagged/")),
            ])])
        );
        Ok(())
    }
}
