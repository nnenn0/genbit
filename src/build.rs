use crate::{
    config::Config,
    content::{self, Article},
    input::SiteInput,
    metadata,
    output::{Artifact, OutputPlan},
    render::Renderer,
};
use anyhow::{Context as _, Result};
use std::{collections::BTreeMap, path::Path};

pub(crate) fn run(root: &Path) -> Result<usize> {
    run_with_reload(root, None)
}

pub(crate) fn run_dev(root: &Path, reload_script: &str) -> Result<usize> {
    run_with_reload(root, Some(reload_script))
}

fn run_with_reload(root: &Path, reload_script: Option<&str>) -> Result<usize> {
    let input = SiteInput::new(root);
    let config_path = root.join("config.toml");
    let config = Config::parse(&input.read_text(Path::new("config.toml"))?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    let mut articles = load_articles(&input)?;
    articles.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.route.url().cmp(right.route.url()))
    });
    let mut tags: BTreeMap<String, Vec<&Article>> = BTreeMap::new();
    let mut untagged = Vec::new();
    for article in &articles {
        if article.tags.is_empty() {
            untagged.push(article);
        }
        for tag in &article.tags {
            tags.entry(tag.clone()).or_default().push(article);
        }
    }
    let renderer = Renderer::load(&input, &articles, reload_script)?;
    let mut artifacts = Vec::with_capacity(articles.len() + tags.len() + 6);
    let home_json_ld = metadata::website_json_ld(&config)?;
    artifacts.push(renderer.home(&config, &articles, &home_json_ld)?);
    artifacts.push(renderer.tags_index(&config, &tags, untagged.len(), &home_json_ld)?);
    for (tag, entries) in &tags {
        artifacts.push(renderer.tag(&config, tag, entries, &home_json_ld)?);
    }
    if !untagged.is_empty() {
        artifacts.push(renderer.untagged(&config, &untagged, &home_json_ld)?);
    }
    artifacts.push(renderer.not_found(&config)?);
    for article in &articles {
        let canonical_url = config.site_url.join_root_path(article.route.url());
        let json_ld = metadata::article_json_ld(&config, article, &canonical_url)?;
        artifacts.push(renderer.article(&config, article, &canonical_url, &json_ld)?);
    }
    artifacts.push(metadata::sitemap(
        &config.site_url,
        &articles,
        &tags,
        !untagged.is_empty(),
    )?);
    artifacts.push(metadata::feed(&config, &articles));
    artifacts.push(metadata::robots(&config.site_url));
    let static_root = root.join("static");
    let assets = input
        .files(Path::new("static"))?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
        .map(|path| {
            let relative = path.strip_prefix(&static_root)?.to_path_buf();
            let site_relative = path.strip_prefix(root)?;
            Ok(Artifact::copy_from(relative, site_relative.to_path_buf()))
        })
        .collect::<Result<Vec<_>>>()?;
    artifacts.extend(assets);
    OutputPlan::new(artifacts)?.publish(&input)?;
    Ok(articles.len() + tags.len() + usize::from(!untagged.is_empty()) + 2)
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
