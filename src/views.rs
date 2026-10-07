//! `views/pages/` holds a file for each page genbit renders and `views/components/` the functions
//! they share, so the files show which functions genbit calls. Each function's CSS sits next to the
//! file that defines it, so a component's HTML and CSS are read, changed, and removed together.

use crate::{
    input::{SiteInput, Tree},
    route,
};
use anyhow::{Context as _, Result, bail, ensure};
use bitview::{Html, Program, Source};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Page {
    Root,
    Article,
    Tags,
    Tag,
    NotFound,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Root,
        Self::Article,
        Self::Tags,
        Self::Tag,
        Self::NotFound,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Article => "page",
            Self::Tags => "tags",
            Self::Tag => "tag",
            Self::NotFound => "not-found",
        }
    }

    fn renders(self) -> &'static str {
        match self {
            Self::Root => "the home page",
            Self::Article => "articles",
            Self::Tags => "the tag index",
            Self::Tag => "tag pages",
            Self::NotFound => "404.html",
        }
    }

    pub(crate) fn file(self) -> String {
        format!("views/pages/{}.bitview", self.name())
    }
}

const DIRECTORIES: [&str; 2] = ["pages", "components"];

pub(crate) struct Views {
    pub(crate) program: Program,
    /// Which functions a page may call depends only on the views, so each page's CSS is joined once.
    styles: BTreeMap<Page, Html>,
}

impl Views {
    pub(crate) fn load(input: &SiteInput<'_>) -> Result<Self> {
        let root = Path::new("views");
        let tree = without_hidden(input.tree(root)?);
        if let Some(directory) = tree.directories.iter().find(|directory| {
            directory
                .to_str()
                .is_none_or(|name| !DIRECTORIES.contains(&name))
        }) {
            bail!(
                "{} is not a views directory; views/ holds pages/ and components/",
                root.join(directory).display()
            );
        }
        let mut sources = Vec::new();
        let mut styles = Vec::new();
        for file in &tree.files {
            let path = root.join(file);
            let name = route::slash_path(&path)
                .with_context(|| format!("invalid view path {}", path.display()))?;
            ensure!(
                file.parent()
                    .and_then(Path::to_str)
                    .is_some_and(|directory| DIRECTORIES.contains(&directory)),
                "{name} is outside views/pages/ and views/components/"
            );
            match file.extension().and_then(|extension| extension.to_str()) {
                Some("bitview") => sources.push((name, input.read_text(&path)?)),
                Some("css") => styles.push((name, input.read_text(&path)?)),
                _ => bail!("{name} is neither a .bitview nor a .css file"),
            }
        }
        let program = Program::parse(
            &sources
                .iter()
                .map(|(name, text)| Source { name, text })
                .collect::<Vec<_>>(),
        )
        .with_context(|| format!("cannot load views in {}", input.root().join(root).display()))?;
        check_pages(&program, &sources)?;
        let css = css_by_function(&program, styles)?;
        let styles = Page::ALL
            .into_iter()
            .map(|page| Ok((page, page_style(&program, page, &css)?)))
            .collect::<Result<_>>()?;
        Ok(Self { program, styles })
    }

    pub(crate) fn style(&self, page: Page) -> Option<&Html> {
        self.styles.get(&page)
    }
}

/// Tools such as the macOS Finder (`.DS_Store`) write files whose names start with `.`; views never
/// use them, so they must not fail the build.
fn without_hidden(mut tree: Tree) -> Tree {
    let hidden = |path: &PathBuf| {
        path.components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
    };
    tree.files.retain(|path| !hidden(path));
    tree.directories.retain(|path| !hidden(path));
    tree
}

fn check_pages(program: &Program, sources: &[(String, String)]) -> Result<()> {
    for (name, _) in sources {
        let Some(file) = name.strip_prefix("views/pages/") else {
            continue;
        };
        ensure!(
            Page::ALL
                .into_iter()
                .any(|page| file.strip_suffix(".bitview") == Some(page.name())),
            "{name} is not a page; views/pages/ holds {}",
            Page::ALL.map(Page::file).join(", ")
        );
    }
    for page in Page::ALL {
        let file = page.file();
        ensure!(
            program
                .defined_at(page.name())
                .is_some_and(|span| span.source() == file)
                && program.has_entry(page.name()),
            "{file} must define fn {}(ctx), which renders {}",
            page.name(),
            page.renders()
        );
    }
    Ok(())
}

/// Requiring the CSS next to the function's file means that moving or renaming a function cannot
/// leave its CSS behind unnoticed.
fn css_by_function(
    program: &Program,
    styles: Vec<(String, String)>,
) -> Result<BTreeMap<String, String>> {
    styles
        .into_iter()
        .map(|(name, text)| {
            let (directory, file) = name.rsplit_once('/').context("invalid view path")?;
            let function = file.strip_suffix(".css").context("invalid view path")?;
            ensure!(
                program
                    .defined_at(function)
                    .and_then(|span| span.source().rsplit_once('/'))
                    .is_some_and(|(defined_in, _)| defined_in == directory),
                "{name} does not belong to a function defined in {directory}/; name a CSS file after a function defined next to it"
            );
            Html::style(text.as_str()).with_context(|| format!("invalid CSS in {name}"))?;
            Ok((function.to_owned(), text))
        })
        .collect()
}

/// The functions a page calls come first, so that a function's CSS can override theirs.
fn page_style(program: &Program, page: Page, css: &BTreeMap<String, String>) -> Result<Html> {
    let joined = program
        .functions_used_by(page.name())
        .with_context(|| format!("missing view function {}", page.name()))?
        .into_iter()
        .filter_map(|function| css.get(function))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Html::style(joined)?)
}
