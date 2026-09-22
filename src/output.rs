use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

const MARKER: &str = ".genbit-output";
const MARKER_CONTENT: &str = "genbit output v1\n";

pub(crate) struct Artifact {
    pub(crate) path: PathBuf,
    pub(crate) bytes: Vec<u8>,
    pub(crate) source: String,
}

pub(crate) fn validate(artifacts: &[Artifact]) -> Result<()> {
    let mut paths = BTreeMap::new();
    for artifact in artifacts {
        ensure!(
            !artifact.path.as_os_str().is_empty()
                && artifact
                    .path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "invalid output path {}",
            artifact.path.display()
        );
        // Case-insensitive comparison keeps generated sites portable to common macOS/Windows filesystems.
        let key = artifact
            .path
            .to_str()
            .context("output path must be UTF-8")?
            .replace('\\', "/")
            .to_lowercase();
        ensure!(
            key != MARKER && !key.starts_with(&format!("{MARKER}/")),
            "reserved output path {}",
            artifact.path.display()
        );
        if let Some(previous) = paths.insert(key, &artifact.source) {
            bail!(
                "output collision at {}: {previous} and {}",
                artifact.path.display(),
                artifact.source
            );
        }
    }
    for (path, source) in &paths {
        for (offset, _) in path.match_indices('/') {
            let parent = path.get(..offset).context("invalid output path boundary")?;
            if let Some(previous) = paths.get(parent) {
                bail!(
                    "output collision: {previous} creates file {parent}, required as directory by {source}"
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn publish(root: &Path, artifacts: &[Artifact]) -> Result<()> {
    validate(artifacts)?;
    let dist = root.join("dist");
    let exists = check_destination(&dist)?;
    let staging = tempfile::Builder::new()
        .prefix(".genbit-build-")
        .tempdir_in(root)
        .with_context(|| format!("cannot stage build in {}", root.display()))?;
    for artifact in artifacts {
        let target = staging.path().join(&artifact.path);
        let parent = target.parent().context("output file has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        fs::write(&target, &artifact.bytes)
            .with_context(|| format!("cannot write {}", target.display()))?;
    }
    fs::write(staging.path().join(MARKER), MARKER_CONTENT)
        .context("cannot mark generated output")?;
    // Keep old output until all new files have been written. The two renames are not a single atomic swap.
    let backup = tempfile::Builder::new()
        .prefix(".genbit-backup-")
        .tempdir_in(root)?;
    let old = backup.path().join("dist");
    if exists {
        fs::rename(&dist, &old).context("cannot preserve previous dist")?;
    }
    if let Err(error) = fs::rename(staging.path(), &dist) {
        if exists && let Err(restore) = fs::rename(&old, &dist) {
            let retained = backup.keep();
            bail!(
                "cannot publish dist: {error}; restore failed: {restore}; previous output retained at {}",
                retained.display()
            );
        }
        return Err(error).context("cannot publish dist");
    }
    Ok(())
}

fn check_destination(dist: &Path) -> Result<bool> {
    match fs::symlink_metadata(dist) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "dist must be a real directory"
            );
            let marker = dist.join(MARKER);
            let owned = fs::symlink_metadata(&marker)
                .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
                && fs::read_to_string(&marker).is_ok_and(|value| value == MARKER_CONTENT);
            ensure!(
                owned,
                "refusing to replace unrecognized dist directory; move it aside before building"
            );
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", dist.display())),
    }
}
