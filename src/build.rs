use crate::{
    config::Config,
    content::{self, Article},
    image_size::{self, Format, ImageSizes},
    input::SiteInput,
    metadata,
    output::{self, Artifact, OutputPlan},
    render::Renderer,
    route,
    tags::TagIndex,
};
use anyhow::{Context as _, Result};
use jiff::tz::TimeZone;
use std::path::{Path, PathBuf};

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
    let static_files = static_files(&input)?;
    let image_sizes = read_image_sizes(&input, &static_files)?;
    let mut articles = load_articles(&input, &config.timezone, &image_sizes)?;
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
    artifacts.extend(static_files.into_iter().map(|relative| {
        let source = Path::new("static").join(&relative);
        Artifact::copy_from(relative, source)
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

/// Returns the files under `static/` relative to it, leaving out `.gitkeep` placeholders.
fn static_files(input: &SiteInput<'_>) -> Result<Vec<PathBuf>> {
    let static_root = input.root().join("static");
    input
        .files(Path::new("static"))?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
        .map(|path| Ok(path.strip_prefix(&static_root)?.to_path_buf()))
        .collect()
}

/// Reads the size of every static image in a format whose header states it, keyed by its URL.
fn read_image_sizes(input: &SiteInput<'_>, static_files: &[PathBuf]) -> Result<ImageSizes> {
    let mut sizes = ImageSizes::default();
    for relative in static_files {
        let Some(format) = Format::from_path(relative) else {
            continue;
        };
        let site_relative = Path::new("static").join(relative);
        let size =
            image_size::read(format, &mut input.open_file(&site_relative)?).with_context(|| {
                format!(
                    "cannot read the image size of {}",
                    input.root().join(&site_relative).display()
                )
            })?;
        if let Some(size) = size {
            let url = relative
                .to_str()
                .with_context(|| format!("static path must be UTF-8: {}", relative.display()))?
                .replace('\\', "/");
            sizes.insert(format!("/{url}"), size);
        }
    }
    Ok(sizes)
}

fn load_articles(
    input: &SiteInput<'_>,
    timezone: &TimeZone,
    image_sizes: &ImageSizes,
) -> Result<Vec<Article>> {
    let content_root = input.root().join("content");
    input
        .files(Path::new("content"))?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let relative = path.strip_prefix(&content_root)?;
            let site_relative = path.strip_prefix(input.root())?;
            content::parse(
                &input.read_text(site_relative)?,
                relative,
                timezone,
                image_sizes,
            )
            .with_context(|| format!("cannot parse {}", path.display()))
        })
        .collect()
}
