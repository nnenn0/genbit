use crate::{
    content::{self, Page},
    output::{self, Artifact},
};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
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
    let config_path = root.join("config.toml");
    let config: Config = toml::from_str(&read_text(&config_path)?)
        .with_context(|| format!("invalid configuration {}", config_path.display()))?;
    ensure!(
        !config.title.trim().is_empty(),
        "config.toml: title must not be empty"
    );
    let css = read_text(&root.join("styles/main.css"))?;
    let tera = load_templates(&root.join("templates"))?;
    let mut pages = load_pages(&root.join("content"))?;
    if !pages.iter().any(|page| page.url == "/") {
        pages.push(content::home(&config.title));
    }
    let listing = pages
        .iter()
        .filter(|page| page.url != "/")
        .collect::<Vec<_>>();
    let mut artifacts = pages
        .iter()
        .map(|page| render(&tera, &config, &css, page, &listing))
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

fn render(tera: &Tera, site: &Config, css: &str, page: &Page, pages: &[&Page]) -> Result<Artifact> {
    let view = View {
        site,
        page,
        pages,
        content: &page.html,
        css,
    };
    let context = Context::from_serialize(&view).context("cannot serialize template context")?;
    let html = tera.render(&page.template, &context).with_context(|| {
        format!(
            "cannot render {} with template {}",
            page.source.display(),
            page.template
        )
    })?;
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
