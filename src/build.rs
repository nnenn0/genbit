use crate::{
    config::Config,
    content::{self, Article},
    image_size::Images,
    input::SiteInput,
    metadata,
    output::{self, Artifact, OutputPlan},
    render::Renderer,
    route,
    tags::TagIndex,
};
use anyhow::{Context as _, Result};
use jiff::tz::TimeZone;
use std::path::Path;

pub(crate) fn run(root: &Path, dry_run: bool) -> Result<usize> {
    run_with_reload(root, None, dry_run)
}

pub(crate) fn run_dev(root: &Path, reload_script: &str) -> Result<usize> {
    run_with_reload(root, Some(reload_script), false)
}

fn run_with_reload(root: &Path, reload_script: Option<&str>, dry_run: bool) -> Result<usize> {
    let input = SiteInput::new(root);
    let config_path = root.join("config.toml");
    let config = Config::parse(&input.read_text(Path::new("config.toml"))?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    let static_files = input
        .files(Path::new("static"))?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
        .collect::<Vec<_>>();
    let mut images = Images::new(root, &static_files)?;
    let mut articles = load_articles(&input, &config.timezone, &mut images)?;
    articles.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.route.url().cmp(right.route.url()))
    });
    let tags = TagIndex::new(&articles);
    let renderer = Renderer::load(&input, &articles, reload_script)?;
    let mut artifacts = Vec::with_capacity(articles.len() + tags.groups().len() + 7);
    let home_json_ld = metadata::website_json_ld(&config)?;
    artifacts.push(renderer.home(&config, &articles, &home_json_ld)?);
    artifacts.push(renderer.tags_index(&config, &tags, &home_json_ld)?);
    for group in tags.groups() {
        artifacts.push(renderer.tag(&config, group, &home_json_ld)?);
    }
    artifacts.push(renderer.not_found(&config)?);
    for article in &articles {
        let canonical_url = config.site_url.join_root_path(article.route.url());
        let json_ld = metadata::article_json_ld(&config, article, &canonical_url)?;
        artifacts.push(renderer.article(&config, article, &canonical_url, &json_ld)?);
    }
    let page_count = artifacts.len();
    artifacts.push(metadata::sitemap(&config.site_url, &articles, &tags)?);
    artifacts.push(metadata::feed(&config, &articles));
    artifacts.push(metadata::robots(&config.site_url));
    artifacts.extend(static_files.into_iter().map(|path| {
        let site_relative = Path::new("static").join(&path);
        let hash = images.hash(&site_relative);
        Artifact::copy_from(path, site_relative, hash)
    }));
    let plan = OutputPlan::new(artifacts)?;
    for article in &articles {
        route::validate_links(
            article.route.url(),
            &article.links,
            &article.source,
            plan.urls(),
        )?;
    }
    if dry_run {
        output::check_destination(&input)?;
    } else {
        plan.publish(&input)?;
    }
    Ok(page_count)
}

fn load_articles(
    input: &SiteInput<'_>,
    timezone: &TimeZone,
    images: &mut Images<'_>,
) -> Result<Vec<Article>> {
    input
        .files(Path::new("content"))?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let site_relative = Path::new("content").join(&path);
            content::parse(
                &input.read_text(&site_relative)?,
                &path,
                timezone,
                |page, url| images.get(page, url),
            )
            .with_context(|| {
                format!(
                    "cannot parse {}",
                    input.root().join(site_relative).display()
                )
            })
        })
        .collect()
}
