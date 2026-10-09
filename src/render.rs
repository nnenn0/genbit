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
use bitview::{Html, HtmlType, Type, Value};
use jiff::Zoned;
use std::path::PathBuf;

pub(crate) struct Renderer {
    views: Views,
}

impl Renderer {
    pub(crate) fn load(input: &SiteInput<'_>) -> Result<Self> {
        let views = Views::load(input)?;
        let pages = Page::ALL.map(|page| (page.name(), page_type(page)));
        views
            .program
            .check(&pages)
            .context("the views do not fit the variables genbit passes")?;
        Ok(Self { views })
    }

    pub(crate) fn tags_index(
        &self,
        config: &Config,
        tags: &TagIndex<'_>,
        json_ld: &str,
    ) -> Result<Artifact> {
        let tags = tags.groups().iter().map(tag_count).collect::<Vec<_>>();
        let mut variables = searchable(&config.site_url.join_root_path(TAGS_INDEX_URL), json_ld)?;
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
        let mut variables = searchable(&config.site_url.join_root_path(&url), json_ld)?;
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
        let mut variables = searchable(config.site_url.as_str(), json_ld)?;
        variables.push(("entries", entries(articles)));
        self.render(
            config,
            Page::Home,
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
        let mut variables = searchable(canonical_url, json_ld)?;
        variables.push(("entry", entry(article)));
        variables.push(("content", Value::from(article.content.clone())));
        self.render(
            config,
            Page::Entry,
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
        let ctx = Value::record(all);
        page_type(page).validate(&ctx).with_context(|| {
            format!(
                "internal error: the variables of {} do not match their type",
                page.file()
            )
        })?;
        let html = self
            .views
            .program
            .render(page.name(), ctx)
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

/// The types genbit names for the views, which they write as the types of parameters, as in
/// `(defn home [ctx HomePage] ...)`. `genbit types` prints them.
pub(crate) fn types() -> Vec<(&'static str, Type)> {
    let mut types = Page::ALL
        .map(|page| (page.type_name(), page_type(page)))
        .to_vec();
    types.extend([
        ("Site", site_type()),
        ("Search", search_type()),
        ("Entry", entry_type()),
        ("Timestamp", timestamp_type()),
        ("TagLink", tag_link_type()),
        ("TagCount", tag_count_type()),
    ]);
    types
}

/// The value each page's function takes. The views are checked against these types when they
/// load, and the values are checked against them when a page renders, so the check at load time
/// holds for every page.
fn page_type(page: Page) -> Type {
    let mut fields = vec![
        ("site", site_type()),
        ("style", Type::Html(HtmlType::Metadata)),
    ];
    if page != Page::NotFound {
        fields.push(("search", search_type()));
    }
    fields.extend(match page {
        Page::Home => vec![("entries", Type::list(entry_type()))],
        Page::Entry => vec![
            ("entry", entry_type()),
            ("content", Type::Html(HtmlType::Flow)),
        ],
        Page::Tags => vec![("tags", Type::list(tag_count_type()))],
        Page::Tag => vec![("tag", Type::String), ("entries", Type::list(entry_type()))],
        Page::NotFound => Vec::new(),
    });
    Type::record(fields)
}

fn site_type() -> Type {
    Type::record([
        ("title", Type::String),
        ("description", Type::String),
        ("url", Type::String),
        ("og-image", Type::String),
    ])
}

fn site(config: &Config) -> Value {
    Value::record([
        ("title", Value::from(config.title.as_str())),
        ("description", Value::from(config.description.as_str())),
        ("url", Value::from(config.site_url.as_str())),
        ("og-image", Value::from(config.og_image.as_str())),
    ])
}

/// What search engines read on a page that they index: its canonical URL and structured data.
fn search_type() -> Type {
    Type::record([
        ("url", Type::String),
        ("json-ld", Type::Html(HtmlType::Metadata)),
    ])
}

fn searchable(canonical_url: &str, json_ld: &str) -> Result<Vec<(&'static str, Value)>> {
    Ok(vec![(
        "search",
        Value::record([
            ("url", Value::from(canonical_url)),
            (
                "json-ld",
                Value::from(Html::json("application/ld+json", json_ld)?),
            ),
        ]),
    )])
}

fn entries<'a>(articles: impl IntoIterator<Item = &'a Article>) -> Value {
    Value::from(articles.into_iter().map(entry).collect::<Vec<_>>())
}

fn entry_type() -> Type {
    Type::record([
        ("title", Type::String),
        ("description", Type::String),
        ("url", Type::String),
        ("created-at", timestamp_type()),
        ("updated-at", timestamp_type()),
        ("tags", Type::list(tag_link_type())),
        ("draft", Type::Bool),
    ])
}

fn entry(article: &Article) -> Value {
    let tags = article_tags(article).map(tag_link).collect::<Vec<_>>();
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

fn tag_link_type() -> Type {
    Type::record([("name", Type::String), ("url", Type::String)])
}

fn tag_link(tag: &str) -> Value {
    Value::record([
        ("name", Value::from(tag)),
        ("url", Value::from(route::tag_url(tag))),
    ])
}

fn tag_count_type() -> Type {
    Type::record([
        ("name", Type::String),
        ("url", Type::String),
        ("count", Type::String),
    ])
}

fn tag_count(group: &TagGroup<'_>) -> Value {
    Value::record([
        ("name", Value::from(group.name)),
        ("url", Value::from(group.url())),
        ("count", Value::from(group.articles.len().to_string())),
    ])
}

fn timestamp_type() -> Type {
    Type::record([("datetime", Type::String), ("date", Type::String)])
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
