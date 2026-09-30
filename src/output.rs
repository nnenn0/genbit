use crate::{content_hash::ContentHash, input::SiteInput};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

const MARKER: &str = ".genbit-output";
const MARKER_CONTENT: &str = "genbit output v1\n";

pub(crate) struct Artifact {
    path: PathBuf,
    content: ArtifactContent,
    source: String,
}

enum ArtifactContent {
    Generated(Vec<u8>),
    CopyFrom {
        source: PathBuf,
        /// The hash already written into article URLs, which the copy must match.
        hash: Option<ContentHash>,
    },
}

pub(crate) struct OutputPlan {
    artifacts: Vec<Artifact>,
    urls: BTreeSet<String>,
}

struct StagedOutput {
    directory: tempfile::TempDir,
}

impl Artifact {
    pub(crate) fn generated(path: PathBuf, bytes: Vec<u8>, source: &str) -> Self {
        Self {
            path,
            content: ArtifactContent::Generated(bytes),
            source: source.to_owned(),
        }
    }

    pub(crate) fn copy_from(path: PathBuf, source: PathBuf, hash: Option<ContentHash>) -> Self {
        let description = source.display().to_string();
        Self {
            path,
            content: ArtifactContent::CopyFrom { source, hash },
            source: description,
        }
    }
}

impl OutputPlan {
    pub(crate) fn new(artifacts: Vec<Artifact>) -> Result<Self> {
        let urls = validate(&artifacts)?;
        Ok(Self { artifacts, urls })
    }

    pub(crate) fn urls(&self) -> &BTreeSet<String> {
        &self.urls
    }

    pub(crate) fn publish(self, input: &SiteInput<'_>) -> Result<()> {
        let exists = check_destination(input)?;
        let staged = self.stage(input)?;
        staged.publish(input.root(), exists)
    }

    fn stage(self, input: &SiteInput<'_>) -> Result<StagedOutput> {
        let directory = tempfile::Builder::new()
            .prefix(".genbit-build-")
            .tempdir_in(input.root())
            .with_context(|| format!("cannot stage build in {}", input.root().display()))?;
        for artifact in self.artifacts {
            let target = directory.path().join(&artifact.path);
            let parent = target.parent().context("output file has no parent")?;
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
            match artifact.content {
                ArtifactContent::Generated(bytes) => {
                    fs::write(&target, bytes).with_context(|| {
                        format!("cannot write {} from {}", target.display(), artifact.source)
                    })?;
                }
                ArtifactContent::CopyFrom { source, hash: None } => {
                    input.copy_file(&source, &target)?;
                }
                ArtifactContent::CopyFrom {
                    source,
                    hash: Some(expected),
                } => {
                    ensure!(
                        input.copy_file_hashed(&source, &target)? == expected,
                        "{} changed during the build, so the hash in its image URLs no longer matches; build again",
                        input.root().join(&source).display()
                    );
                }
            }
        }
        fs::write(directory.path().join(MARKER), MARKER_CONTENT)
            .context("cannot mark generated output")?;
        Ok(StagedOutput { directory })
    }
}

impl StagedOutput {
    fn publish(self, root: &Path, exists: bool) -> Result<()> {
        let dist = root.join("dist");
        // Keep old output until all new files have been written. The two renames are not a single atomic swap.
        let backup = tempfile::Builder::new()
            .prefix(".genbit-backup-")
            .tempdir_in(root)?;
        let old = backup.path().join("dist");
        if exists {
            fs::rename(&dist, &old).context("cannot preserve previous dist")?;
        }
        if let Err(error) = fs::rename(self.directory.path(), &dist) {
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
}

/// An output path joined with `/`, and the source that produces it.
struct PlannedFile<'a> {
    path: String,
    source: &'a str,
}

fn validate(artifacts: &[Artifact]) -> Result<BTreeSet<String>> {
    let files = artifacts
        .iter()
        .map(|artifact| {
            let path = crate::route::slash_path(&artifact.path).with_context(|| {
                format!(
                    "invalid output path {} from {}",
                    artifact.path.display(),
                    artifact.source
                )
            })?;
            ensure!(
                path.split('/')
                    .next()
                    .is_none_or(|first| first.to_lowercase() != MARKER),
                "reserved output path {path}"
            );
            Ok(PlannedFile {
                path,
                source: &artifact.source,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ensure_one_spelling_per_directory(&files)?;
    let claimed = claim_paths(&files)?;
    ensure_no_file_is_a_directory(&claimed)?;
    crate::route::validate_served_urls(files.iter().map(|file| (file.path.as_str(), file.source)))
}

/// Directories are shared by name on case-insensitive filesystems, so each needs one spelling.
fn ensure_one_spelling_per_directory(files: &[PlannedFile<'_>]) -> Result<()> {
    let mut spellings = BTreeMap::new();
    for file in files {
        for directory in parent_directories(&file.path) {
            let (previous, previous_source) = spellings
                .entry(directory.to_lowercase())
                .or_insert((directory, file.source));
            ensure!(
                *previous == directory,
                "output directory case collision: {previous} from {previous_source} and {directory} from {}",
                file.source
            );
        }
    }
    Ok(())
}

/// Returns the source of each path by its lowercase spelling. Case-insensitive comparison keeps
/// generated sites portable to common macOS/Windows filesystems.
fn claim_paths<'a>(files: &[PlannedFile<'a>]) -> Result<BTreeMap<String, &'a str>> {
    let mut claimed = BTreeMap::new();
    for file in files {
        if let Some(previous) = claimed.insert(file.path.to_lowercase(), file.source) {
            bail!(
                "output collision at {}: {previous} and {}",
                file.path,
                file.source
            );
        }
    }
    Ok(claimed)
}

fn ensure_no_file_is_a_directory(claimed: &BTreeMap<String, &str>) -> Result<()> {
    for (path, source) in claimed {
        if let Some((parent, previous)) = parent_directories(path)
            .find_map(|parent| claimed.get(parent).map(|previous| (parent, previous)))
        {
            bail!(
                "output collision: {previous} creates file {parent}, required as directory by {source}"
            );
        }
    }
    Ok(())
}

/// The directories that contain `path`, outermost first.
fn parent_directories(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .filter_map(|(offset, _)| path.get(..offset))
}

/// Returns whether `dist` exists, failing when it is not output that genbit may replace.
pub(crate) fn check_destination(input: &SiteInput<'_>) -> Result<bool> {
    let dist = input.root().join("dist");
    match fs::symlink_metadata(&dist) {
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

#[cfg(test)]
mod tests {
    use super::{Artifact, OutputPlan};
    use crate::{content_hash::ContentHash, input::SiteInput};
    use anyhow::{Context, Result};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[test]
    fn rejects_directories_spelled_with_different_case() -> Result<()> {
        let error = OutputPlan::new(vec![
            Artifact::generated(PathBuf::from("Docs/a/post.html"), Vec::new(), "first"),
            Artifact::generated(PathBuf::from("docs/a/image.svg"), Vec::new(), "second"),
        ])
        .err()
        .context("accepted directory case collision")?;
        assert_eq!(
            error.to_string(),
            "output directory case collision: Docs from first and docs from second"
        );
        OutputPlan::new(vec![
            Artifact::generated(PathBuf::from("docs/a/post.html"), Vec::new(), "first"),
            Artifact::generated(PathBuf::from("docs/b/image.svg"), Vec::new(), "second"),
        ])?;
        Ok(())
    }

    #[test]
    fn rejects_the_output_marker_in_any_case() -> Result<()> {
        for path in [".genbit-output", ".Genbit-Output/a.html"] {
            let error = OutputPlan::new(vec![Artifact::generated(
                PathBuf::from(path),
                Vec::new(),
                "source",
            )])
            .err()
            .context("accepted the output marker")?;
            assert_eq!(error.to_string(), format!("reserved output path {path}"));
        }
        OutputPlan::new(vec![Artifact::generated(
            PathBuf::from("a/.genbit-output"),
            Vec::new(),
            "source",
        )])?;
        Ok(())
    }

    #[test]
    fn rejects_files_needed_as_directories_in_any_case() -> Result<()> {
        let error = OutputPlan::new(vec![
            Artifact::generated(PathBuf::from("Docs/a.html"), Vec::new(), "first"),
            Artifact::generated(PathBuf::from("docs"), Vec::new(), "second"),
        ])
        .err()
        .context("accepted a file where a directory is needed")?;
        assert_eq!(
            error.to_string(),
            "output collision: second creates file docs, required as directory by first"
        );
        Ok(())
    }

    #[test]
    fn missing_static_file_after_listing_preserves_previous_dist() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let input = SiteInput::new(root);
        OutputPlan::new(vec![Artifact::generated(
            PathBuf::from("index.html"),
            b"old output".to_vec(),
            "home",
        )])?
        .publish(&input)?;

        fs::create_dir(root.join("static"))?;
        fs::write(root.join("static/asset.bin"), [0, 1, 2, 255])?;
        let listed = input.files(Path::new("static"))?;
        assert_eq!(listed.len(), 1);
        let plan = OutputPlan::new(vec![
            Artifact::generated(PathBuf::from("index.html"), b"new output".to_vec(), "home"),
            Artifact::copy_from(
                PathBuf::from("asset.bin"),
                PathBuf::from("static/asset.bin"),
                None,
            ),
        ])?;
        fs::remove_file(root.join("static/asset.bin"))?;

        let error = plan
            .publish(&input)
            .err()
            .context("accepted vanished input")?;
        assert!(
            format!("{error:#}").contains("static/asset.bin"),
            "{error:#}"
        );
        assert_eq!(fs::read(root.join("dist/index.html"))?, b"old output");
        assert!(!root.join("dist/asset.bin").exists());
        for entry in fs::read_dir(root)? {
            let name = entry?.file_name();
            assert!(
                !name.to_string_lossy().starts_with(".genbit-build-"),
                "staging directory remains: {}",
                name.to_string_lossy()
            );
        }
        Ok(())
    }

    #[test]
    fn copies_that_no_longer_match_their_url_hash_preserve_previous_dist() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let input = SiteInput::new(root);
        OutputPlan::new(vec![Artifact::generated(
            PathBuf::from("index.html"),
            b"old output".to_vec(),
            "home",
        )])?
        .publish(&input)?;

        fs::create_dir(root.join("static"))?;
        let rendered = ContentHash::of_reader(&mut b"rendered".as_slice())?;
        let plan = || {
            OutputPlan::new(vec![
                Artifact::generated(PathBuf::from("index.html"), b"new output".to_vec(), "home"),
                Artifact::copy_from(
                    PathBuf::from("photo.png"),
                    PathBuf::from("static/photo.png"),
                    Some(rendered),
                ),
            ])
        };
        fs::write(root.join("static/photo.png"), b"edited")?;
        let error = plan()?
            .publish(&input)
            .err()
            .context("accepted a copy that differs from the hashed file")?;
        assert!(
            format!("{error:#}").contains("static/photo.png changed during the build"),
            "{error:#}"
        );
        assert_eq!(fs::read(root.join("dist/index.html"))?, b"old output");
        assert!(!root.join("dist/photo.png").exists());

        fs::write(root.join("static/photo.png"), b"rendered")?;
        plan()?.publish(&input)?;
        assert_eq!(fs::read(root.join("dist/photo.png"))?, b"rendered");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_replacing_static_file_before_copy_preserves_previous_dist() -> Result<()> {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let input = SiteInput::new(root);
        OutputPlan::new(vec![Artifact::generated(
            PathBuf::from("index.html"),
            b"old output".to_vec(),
            "home",
        )])?
        .publish(&input)?;

        fs::create_dir(root.join("static"))?;
        fs::write(root.join("static/asset.bin"), [0, 1, 2, 255])?;
        assert_eq!(input.files(Path::new("static"))?.len(), 1);
        let plan = OutputPlan::new(vec![Artifact::copy_from(
            PathBuf::from("asset.bin"),
            PathBuf::from("static/asset.bin"),
            None,
        )])?;
        fs::remove_file(root.join("static/asset.bin"))?;
        let outside = root.join("outside.bin");
        fs::write(&outside, b"outside")?;
        symlink(&outside, root.join("static/asset.bin"))?;

        let error = plan
            .publish(&input)
            .err()
            .context("accepted symlink input")?;
        assert!(
            format!("{error:#}").contains("symlinks are not supported"),
            "{error:#}"
        );
        assert_eq!(fs::read(root.join("dist/index.html"))?, b"old output");
        assert_eq!(fs::read(outside)?, b"outside");
        Ok(())
    }
}
