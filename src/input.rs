use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

pub(crate) struct SiteInput<'a> {
    root: &'a Path,
}

impl<'a> SiteInput<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        self.root
    }

    pub(crate) fn read_text(&self, relative: &Path) -> Result<String> {
        let path = self.required_file(relative)?;
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))
    }

    pub(crate) fn read_optional_text(&self, relative: &Path) -> Result<Option<String>> {
        self.checked_file(relative)?
            .map(|path| {
                fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))
            })
            .transpose()
    }

    pub(crate) fn copy_file(&self, relative: &Path, target: &Path) -> Result<()> {
        let path = self.required_file(relative)?;
        fs::copy(&path, target)
            .with_context(|| format!("cannot copy {} to {}", path.display(), target.display()))?;
        Ok(())
    }

    pub(crate) fn files(&self, relative: &Path) -> Result<Vec<PathBuf>> {
        let path = self.root.join(relative);
        let metadata = self
            .inspect(&path)?
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        ensure!(
            metadata.is_dir(),
            "expected a real directory: {}",
            path.display()
        );
        let mut result = Vec::new();
        collect_files(&path, &mut result)?;
        result.sort();
        Ok(result)
    }

    fn required_file(&self, relative: &Path) -> Result<PathBuf> {
        self.checked_file(relative)?
            .with_context(|| format!("cannot inspect {}", self.root.join(relative).display()))
    }

    fn checked_file(&self, relative: &Path) -> Result<Option<PathBuf>> {
        let path = self.root.join(relative);
        self.inspect(&path)?
            .map(|metadata| {
                ensure!(
                    metadata.is_file(),
                    "expected a regular file: {}",
                    path.display()
                );
                Ok(path)
            })
            .transpose()
    }

    fn inspect(&self, path: &Path) -> Result<Option<fs::Metadata>> {
        let relative = path.strip_prefix(self.root).with_context(|| {
            format!(
                "input path is outside {}: {}",
                self.root.display(),
                path.display()
            )
        })?;
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "invalid input path {}",
            path.display()
        );
        let mut current = self.root.to_path_buf();
        let mut parts = relative.components().peekable();
        while let Some(part) = parts.next() {
            current.push(part.as_os_str());
            let metadata = match fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("cannot inspect {}", current.display()));
                }
            };
            ensure!(
                !metadata.file_type().is_symlink(),
                "symlinks are not supported: {}",
                current.display()
            );
            if parts.peek().is_some() {
                ensure!(
                    metadata.is_dir(),
                    "expected a real directory: {}",
                    current.display()
                );
            } else {
                return Ok(Some(metadata));
            }
        }
        bail!("invalid input path {}", path.display())
    }
}

fn collect_files(directory: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("cannot read {}", directory.display()))?
    {
        let entry =
            entry.with_context(|| format!("cannot read entry in {}", directory.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        ensure!(
            !kind.is_symlink(),
            "symlinks are not supported: {}",
            path.display()
        );
        if kind.is_dir() {
            collect_files(&path, result)?;
        } else {
            ensure!(
                kind.is_file(),
                "expected a regular file: {}",
                path.display()
            );
            result.push(path);
        }
    }
    Ok(())
}
