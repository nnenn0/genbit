use crate::content_hash::{ContentHash, HashingWriter};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::{self, ErrorKind},
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

    pub(crate) fn open_file(&self, relative: &Path) -> Result<fs::File> {
        let path = self.required_file(relative)?;
        fs::File::open(&path).with_context(|| format!("cannot read {}", path.display()))
    }

    pub(crate) fn read_optional_text(&self, relative: &Path) -> Result<Option<String>> {
        self.checked_file(relative)?
            .map(|path| {
                fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))
            })
            .transpose()
    }

    pub(crate) fn copy_file(&self, relative: &Path, target: &Path) -> Result<()> {
        let (path, mut source, mut output) = self.open_copy(relative, target)?;
        io::copy(&mut source, &mut output).with_context(|| copy_error(&path, target))?;
        Ok(())
    }

    /// Copies like `copy_file`, hashing the bytes as they are written to `target`.
    pub(crate) fn copy_file_hashed(&self, relative: &Path, target: &Path) -> Result<ContentHash> {
        let (path, mut source, output) = self.open_copy(relative, target)?;
        let mut output = HashingWriter::new(output);
        io::copy(&mut source, &mut output).with_context(|| copy_error(&path, target))?;
        Ok(output.finish())
    }

    fn open_copy(&self, relative: &Path, target: &Path) -> Result<(PathBuf, fs::File, fs::File)> {
        let path = self.required_file(relative)?;
        // `fs::copy` clones files on macOS, and FSEvents reports that as a change to the
        // source, so `dev` would rebuild forever. Stream the bytes instead.
        let source =
            fs::File::open(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let output = fs::File::create(target)
            .with_context(|| format!("cannot write {}", target.display()))?;
        Ok((path, source, output))
    }

    /// Lists the regular files under `relative`, as paths relative to it.
    pub(crate) fn files(&self, relative: &Path) -> Result<Vec<PathBuf>> {
        let path = self.root.join(relative);
        self.optional_files(relative)?
            .with_context(|| format!("cannot inspect {}", path.display()))
    }

    /// Lists the regular files under `relative` like `files`, or returns `None` when it does not exist.
    pub(crate) fn optional_files(&self, relative: &Path) -> Result<Option<Vec<PathBuf>>> {
        let path = self.root.join(relative);
        let Some(metadata) = self.inspect(&path)? else {
            return Ok(None);
        };
        ensure!(
            metadata.is_dir(),
            "expected a real directory: {}",
            path.display()
        );
        let mut result = Vec::new();
        collect_files(&path, Path::new(""), &mut result)?;
        result.sort();
        Ok(Some(result))
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

fn copy_error(source: &Path, target: &Path) -> String {
    format!("cannot copy {} to {}", source.display(), target.display())
}

fn collect_files(directory: &Path, prefix: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("cannot read {}", directory.display()))?
    {
        let entry =
            entry.with_context(|| format!("cannot read entry in {}", directory.display()))?;
        let path = entry.path();
        let relative = prefix.join(entry.file_name());
        let kind = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        ensure!(
            !kind.is_symlink(),
            "symlinks are not supported: {}",
            path.display()
        );
        if kind.is_dir() {
            collect_files(&path, &relative, result)?;
        } else {
            ensure!(
                kind.is_file(),
                "expected a regular file: {}",
                path.display()
            );
            result.push(relative);
        }
    }
    Ok(())
}
