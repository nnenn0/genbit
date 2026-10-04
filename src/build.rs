use crate::{
    config::Config,
    content::{self, Article},
    content_hash::ContentHash,
    image_size::Images,
    input::SiteInput,
    metadata,
    output::{self, Artifact, CopiedFile, OutputPlan},
    render::Renderer,
    route,
    tags::TagIndex,
};
use anyhow::{Context as _, Result};
use jiff::tz::TimeZone;
use std::{
    collections::BTreeMap,
    iter,
    path::{Path, PathBuf},
};

/// What a build does with the output it plans.
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// Replace `dist/`.
    Publish,
    /// Run every step and check, but leave `dist/` unchanged.
    DryRun,
    /// Replace `dist/` with pages that reload when `genbit dev` rebuilds.
    Dev,
}

/// Returns the number of HTML pages.
pub(crate) fn run(root: &Path, mode: Mode) -> Result<usize> {
    let input = SiteInput::new(root);
    let config_path = root.join("config.toml");
    let config = Config::parse(&input.read_text(Path::new("config.toml"))?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    let content = content::sort_files(files(&input, "content")?)?;
    let copied = files(&input, "static")?
        .into_iter()
        .map(|path| (path, "static"))
        .chain(content.assets.into_iter().map(|path| (path, "content")))
        .map(|(path, directory)| CopiedFile {
            source: Path::new(directory).join(&path),
            output: path,
        })
        .collect::<Vec<_>>();
    let images = Images::new(root, &copied)?;
    let (mut articles, image_hashes) =
        load_articles(&input, content.pages, &config.timezone, images)?;
    articles.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.route.url().cmp(right.route.url()))
    });
    let tags = TagIndex::new(&articles);
    let reload_script = matches!(mode, Mode::Dev).then_some(crate::dev::RELOAD_SCRIPT);
    let renderer = Renderer::load(&input, &articles, reload_script)?;
    let pages = render_pages(&renderer, &config, &articles, &tags)?;
    let page_count = pages.len();
    let artifacts = pages
        .into_iter()
        .chain([
            metadata::sitemap(&config.site_url, &articles, &tags)?,
            metadata::feed(&config, &articles),
            metadata::robots(&config.site_url),
        ])
        .chain(copied.into_iter().map(|file| {
            let hash = image_hashes.get(&file.source).copied();
            Artifact::copy_from(file.output, file.source, hash)
        }))
        .collect();
    let plan = OutputPlan::new(artifacts)?;
    for article in &articles {
        route::validate_links(
            article.route.url(),
            &article.links,
            &article.source,
            plan.urls(),
        )?;
    }
    match mode {
        Mode::DryRun => {
            output::check_destination(&input)?;
        }
        Mode::Publish | Mode::Dev => plan.publish(&input)?,
    }
    Ok(page_count)
}

fn render_pages(
    renderer: &Renderer,
    config: &Config,
    articles: &[Article],
    tags: &TagIndex<'_>,
) -> Result<Vec<Artifact>> {
    let home_json_ld = metadata::website_json_ld(config)?;
    [
        renderer.home(config, articles, &home_json_ld),
        renderer.tags_index(config, tags, &home_json_ld),
    ]
    .into_iter()
    .chain(
        tags.groups()
            .iter()
            .map(|group| renderer.tag(config, group, &home_json_ld)),
    )
    .chain(iter::once_with(|| renderer.not_found(config)))
    .chain(articles.iter().map(|article| {
        let canonical_url = config.site_url.join_root_path(article.route.url());
        let json_ld = metadata::article_json_ld(config, article, &canonical_url)?;
        renderer.article(config, article, &canonical_url, &json_ld)
    }))
    .collect()
}

/// Lists the files under `directory`, leaving out the `.gitkeep` files that keep empty
/// directories in Git.
fn files(input: &SiteInput<'_>, directory: &str) -> Result<Vec<PathBuf>> {
    Ok(input
        .files(Path::new(directory))?
        .into_iter()
        .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
        .collect())
}

fn load_articles(
    input: &SiteInput<'_>,
    pages: Vec<PathBuf>,
    timezone: &TimeZone,
    mut images: Images<'_>,
) -> Result<(Vec<Article>, BTreeMap<PathBuf, ContentHash>)> {
    let articles = pages
        .into_iter()
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
        .collect::<Result<_>>()?;
    // Hashes are complete only once every article has been rendered.
    Ok((articles, images.into_hashes()))
}
