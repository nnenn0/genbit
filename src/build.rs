use crate::{
    config::Config,
    content::{self, Article, ContentDir},
    content_hash::ContentHash,
    images::Images,
    input::{SiteInput, Tree},
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

/// What a build reads and what it does with the output it plans.
#[derive(Clone, Copy)]
pub(crate) enum Mode<'a> {
    /// Replace `dist/`.
    Publish,
    /// Run every step and check, but leave `dist/` unchanged.
    DryRun,
    /// Lay `drafts/` over `content/`, and replace `output` with pages that reload when
    /// `genbit dev` rebuilds. `dist/` is left unchanged, so drafts never reach it.
    Dev { output: &'a Path },
}

/// Returns the number of HTML pages.
pub(crate) fn run(root: &Path, mode: Mode<'_>) -> Result<usize> {
    use ContentDir::{Content, Drafts};
    let input = SiteInput::new(root);
    let config_path = root.join("config.toml");
    let config = Config::parse(&input.read_text(Path::new("config.toml"))?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    let content =
        content::classify_files(without_tool_files(input.tree(Content.path())?), Content)?;
    let drafts = match mode {
        Mode::Dev { .. } => input
            .optional_tree(Drafts.path())?
            .map(|tree| content::classify_files(without_tool_files(tree), Drafts))
            .transpose()?,
        Mode::Publish | Mode::DryRun => None,
    }
    .unwrap_or_default();
    content::check_drafts(&content, &drafts)?;
    let copied = without_tool_files(input.tree(Path::new("static"))?)
        .files
        .into_iter()
        .map(|path| (path, Path::new("static")))
        .chain(
            content
                .assets
                .into_iter()
                .map(|path| (path, Content.path())),
        )
        .chain(drafts.assets.into_iter().map(|path| (path, Drafts.path())))
        .map(|(path, directory)| CopiedFile {
            source: directory.join(&path),
            output: path,
        })
        .collect::<Vec<_>>();
    let pages = content
        .pages
        .into_iter()
        .map(|path| (path, Content))
        .chain(drafts.pages.into_iter().map(|path| (path, Drafts)))
        .collect();
    let images = Images::new(input, &copied)?;
    let (mut articles, image_hashes) = load_articles(&input, pages, &config.timezone, images)?;
    articles.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.route.url().cmp(right.route.url()))
    });
    let tags = TagIndex::new(&articles);
    let renderer = Renderer::load(&input, &articles, matches!(mode, Mode::Dev { .. }))?;
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
    let dist = root.join("dist");
    match mode {
        Mode::DryRun => {
            output::check_destination(&dist)?;
        }
        Mode::Publish => plan.publish(&input, &dist)?,
        Mode::Dev { output } => plan.publish(&input, output)?,
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

/// Leaves out the files that tools write next to the site's files: `.gitkeep`, which keeps empty
/// directories in Git, and `.DS_Store`, which the macOS Finder writes. They are neither pages nor
/// files to publish.
fn without_tool_files(mut tree: Tree) -> Tree {
    tree.files.retain(|path| {
        path.file_name()
            .is_none_or(|name| name != ".gitkeep" && name != ".DS_Store")
    });
    tree
}

/// Takes each page's `index.md` with the directory it is in.
fn load_articles(
    input: &SiteInput<'_>,
    pages: Vec<(PathBuf, ContentDir)>,
    timezone: &TimeZone,
    mut images: Images<'_>,
) -> Result<(Vec<Article>, BTreeMap<PathBuf, ContentHash>)> {
    let articles = pages
        .into_iter()
        .map(|(path, directory)| {
            let site_relative = directory.path().join(&path);
            content::parse(
                &input.read_text(&site_relative)?,
                directory,
                &path,
                timezone,
                |page, url| images.get(page, url),
            )
            .with_context(|| {
                format!(
                    "cannot parse {}",
                    input.root().join(&site_relative).display()
                )
            })
        })
        .collect::<Result<_>>()?;
    // Hashes are complete only once every article has been rendered.
    Ok((articles, images.into_hashes()))
}
