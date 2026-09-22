use crate::{
    content::{self, Page},
    output::{self, Artifact},
};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tera::{Context, Tera};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    title: String,
}

#[derive(Serialize)]
struct View<'a> {
    site: &'a Config,
    page: &'a Page,
    pages: &'a [&'a Page],
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
    let config: Config = toml::from_str(&read_text(&config_path)?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    ensure!(
        !config.title.trim().is_empty(),
        "config.toml: title must not be empty"
    );
    let tera = load_templates(&root.join("templates"))?;
    let mut pages = load_pages(&root.join("content"))?;
    pages.push(content::home(&config.title));
    let styles = load_styles(root, &pages)?;
    let mut listing = pages
        .iter()
        .filter(|page| page.url != "/")
        .collect::<Vec<_>>();
    listing.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.url.cmp(&right.url))
    });
    let mut artifacts = pages
        .iter()
        .map(|page| {
            let css = styles
                .get(&page.template)
                .with_context(|| format!("missing styles for template {}", page.template))?;
            render(&tera, &config, css, page, &listing, dev)
        })
        .collect::<Result<Vec<_>>>()?;
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
    Ok(pages.len())
}

fn load_styles(root: &Path, pages: &[Page]) -> Result<BTreeMap<String, String>> {
    let styles = root.join("styles");
    let common = read_text(&styles.join("common.css"))?;
    pages
        .iter()
        .map(|page| {
            let template = Path::new(&page.template).with_extension("css");
            let specific = styles.join(template);
            let mut css = common.clone();
            if let Some(specific) = read_optional_text(&specific)? {
                css.push('\n');
                css.push_str(&specific);
            }
            Ok((page.template.clone(), css))
        })
        .collect()
}

fn render(
    tera: &Tera,
    site: &Config,
    css: &str,
    page: &Page,
    pages: &[&Page],
    dev: bool,
) -> Result<Artifact> {
    let view = View {
        site,
        page,
        pages,
        content: &page.html,
        css,
    };
    let context = Context::from_serialize(&view).context("cannot serialize template context")?;
    let mut html = tera.render(&page.template, &context).with_context(|| {
        format!(
            "cannot render {} with template {}",
            page.source.display(),
            page.template
        )
    })?;
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
        path: page.output.clone(),
        bytes: minified,
        source: page.source.display().to_string(),
    })
}

fn load_pages(root: &Path) -> Result<Vec<Page>> {
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
