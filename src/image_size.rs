use crate::{content_hash::ContentHash, input::SiteInput, output::CopiedFile, route};
use anyhow::{Context, Result, bail, ensure};
use imagesize::{ImageError, ImageType};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, ErrorKind, Seek},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) width: usize,
    pub(crate) height: usize,
}

/// A local file referenced as an image, which need not be in a format with known dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) hash: ContentHash,
    pub(crate) size: Option<Size>,
}

/// A build-local cache, including files that are not images. Only rendered images are read.
pub(crate) struct Images<'a> {
    input: SiteInput<'a>,
    files: BTreeMap<String, PathBuf>,
    probed: BTreeMap<PathBuf, Image>,
}

impl<'a> Images<'a> {
    /// Takes the files under `static/`, as paths relative to it.
    /// Takes every file copied into the output, so that a URL finds the file it serves.
    pub(crate) fn new(root: &'a Path, files: &[CopiedFile]) -> Result<Self> {
        let files = files
            .iter()
            .map(|file| {
                let url = route::slash_path(&file.output)
                    .with_context(|| format!("invalid file {}", file.source.display()))?;
                Ok((format!("/{url}"), file.source.clone()))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            input: SiteInput::new(root),
            files,
            probed: BTreeMap::new(),
        })
    }

    /// Returns `None` for targets outside `static/`.
    pub(crate) fn get(&mut self, page: &str, url: &str) -> Result<Option<Image>> {
        // Invalid and missing targets are diagnosed by the existing link validator.
        let Ok(Some(target)) = route::resolve_link(page, url) else {
            return Ok(None);
        };
        let Some(path) = self.files.get(&target) else {
            return Ok(None);
        };
        if let Some(image) = self.probed.get(path) {
            return Ok(Some(*image));
        }
        let full_path = self.input.root().join(path);
        let mut reader = BufReader::new(self.input.open_file(path)?);
        let size = probe(&mut reader, has_image_extension(path))
            .with_context(|| format!("cannot get the size of image {}", full_path.display()))?;
        reader.rewind()?;
        let hash = ContentHash::of_reader(&mut reader)
            .with_context(|| format!("cannot read {}", full_path.display()))?;
        let image = Image { hash, size };
        self.probed.insert(path.clone(), image);
        Ok(Some(image))
    }

    /// The hashes of the files that articles referenced as images, by path under the site root.
    pub(crate) fn into_hashes(self) -> BTreeMap<PathBuf, ContentHash> {
        self.probed
            .into_iter()
            .map(|(path, image)| (path, image.hash))
            .collect()
    }
}

fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "gif", "webp"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Reads dimensions without decoding pixels. Every PNG, JPEG, GIF, or WebP image must yield a size,
/// so that no local image in these formats is left without one; other files return `None`.
fn probe(reader: &mut (impl BufRead + Seek), image_extension: bool) -> Result<Option<Size>> {
    let format = match imagesize::reader_type(&mut *reader) {
        Ok(format @ (ImageType::Png | ImageType::Jpeg | ImageType::Gif | ImageType::Webp)) => {
            Some(format)
        }
        Ok(_) | Err(ImageError::NotSupported | ImageError::CorruptedImage) => None,
        Err(ImageError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => None,
        Err(ImageError::IoError(error)) => return Err(error.into()),
    };
    let Some(format) = format else {
        ensure!(
            !image_extension,
            "the file name has an image extension, but the content is not a PNG, JPEG, GIF, or WebP image; export the image again"
        );
        return Ok(None);
    };
    reader.rewind()?;
    let dimensions = match imagesize::reader_size(&mut *reader) {
        Ok(size) => size,
        Err(ImageError::IoError(error)) if !is_invalid_data(&error) => return Err(error.into()),
        Err(_) => bail!("the image header is truncated or invalid; export the image again"),
    };
    ensure!(
        dimensions.width > 0 && dimensions.height > 0,
        "the image has no width or height; export the image again"
    );
    let mut size = Size {
        width: dimensions.width,
        height: dimensions.height,
    };
    if format == ImageType::Gif {
        return Ok(Some(size));
    }
    reader.rewind()?;
    let metadata = match exif::Reader::new().read_from_container(reader) {
        Ok(metadata) => metadata,
        Err(exif::Error::NotFound(_)) => return Ok(Some(size)),
        Err(exif::Error::Io(error)) if !is_invalid_data(&error) => return Err(error.into()),
        Err(_) => bail!(
            "the EXIF metadata is truncated or invalid; export the image again or remove its EXIF metadata"
        ),
    };
    let Some(field) = metadata.get_field(exif::Tag::Orientation, exif::In::PRIMARY) else {
        return Ok(Some(size));
    };
    match field.value.get_uint(0) {
        Some(1..=4) => {}
        Some(5..=8) if format == ImageType::Jpeg => {
            std::mem::swap(&mut size.width, &mut size.height);
        }
        // Browsers ignore PNG and WebP EXIF that follows the pixel data, and they disagree on
        // WebP, so no single width and height matches every browser.
        Some(orientation @ 5..=8) => bail!(
            "{} images with EXIF Orientation {orientation} are displayed differently by each browser; remove the EXIF metadata or convert the image to JPEG",
            if format == ImageType::Png {
                "PNG"
            } else {
                "WebP"
            }
        ),
        Some(orientation) => bail!(
            "EXIF Orientation {orientation} is not between 1 and 8; export the image again or remove its EXIF metadata"
        ),
        None => bail!(
            "EXIF Orientation is not an integer; export the image again or remove its EXIF metadata"
        ),
    }
    Ok(Some(size))
}

fn is_invalid_data(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::InvalidData | ErrorKind::InvalidInput
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Cursor};

    #[test]
    fn reads_supported_formats_and_applies_exif_orientation() -> Result<()> {
        for (bytes, width, height) in [
            (
                include_bytes!("../tests/fixtures/images/basic.png").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/basic.jpg").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/basic.gif").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/lossy.webp").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/lossless.webp").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/alpha.webp").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/animated.webp").as_slice(),
                300,
                200,
            ),
            (
                include_bytes!("../tests/fixtures/images/exif.jpg").as_slice(),
                200,
                300,
            ),
        ] {
            assert_eq!(
                probe(&mut Cursor::new(bytes), true)?,
                Some(Size { width, height })
            );
        }
        Ok(())
    }

    fn static_files(names: &[&str]) -> Vec<CopiedFile> {
        names
            .iter()
            .map(|name| CopiedFile {
                output: PathBuf::from(name),
                source: Path::new("static").join(name),
            })
            .collect()
    }

    fn probe_error(bytes: &[u8], image_extension: bool) -> Result<String> {
        let error = probe(&mut Cursor::new(bytes), image_extension)
            .err()
            .context("accepted a broken image")?;
        Ok(format!("{error:#}"))
    }

    #[test]
    fn files_that_are_not_images_have_no_size() -> Result<()> {
        for bytes in [
            b"".as_slice(),
            b"not an image",
            b"<svg width='300' height='200'></svg>",
        ] {
            assert_eq!(probe(&mut Cursor::new(bytes), false)?, None);
        }
        Ok(())
    }

    #[test]
    fn broken_images_and_unsupported_orientations_are_errors() -> Result<()> {
        let png = include_bytes!("../tests/fixtures/images/basic.png");
        for (bytes, image_extension, expected) in [
            (
                b"".as_slice(),
                true,
                "the content is not a PNG, JPEG, GIF, or WebP image",
            ),
            (
                b"not an image",
                true,
                "the content is not a PNG, JPEG, GIF, or WebP image",
            ),
            (
                png.get(..20).context("short fixture")?,
                false,
                "the image header is truncated or invalid",
            ),
            // The size is in the header, but the EXIF scan reaches the end of the file.
            (
                png.get(..40).context("short fixture")?,
                false,
                "the EXIF metadata is truncated or invalid",
            ),
            (
                include_bytes!("../tests/fixtures/images/exif.png").as_slice(),
                false,
                "PNG images with EXIF Orientation 6 are displayed differently by each browser",
            ),
            (
                include_bytes!("../tests/fixtures/images/exif.webp").as_slice(),
                false,
                "WebP images with EXIF Orientation 6 are displayed differently by each browser",
            ),
        ] {
            let error = probe_error(bytes, image_extension)?;
            assert!(error.contains(expected), "{error}");
        }
        Ok(())
    }

    #[test]
    fn only_referenced_files_are_read_and_results_are_cached_by_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let static_root = root.path().join("static");
        fs::create_dir(&static_root)?;
        let image = static_root.join("image.png");
        let copy = static_root.join("copy.png");
        let data = static_root.join("notes.data");
        let broken = static_root.join("broken.png");
        let unused = static_root.join("unused.png");
        let png = include_bytes!("../tests/fixtures/images/basic.png");
        fs::write(&image, png)?;
        fs::write(&copy, png)?;
        fs::write(&data, b"notes")?;
        fs::write(&broken, b"broken")?;
        fs::write(&unused, b"unused")?;
        let mut images = Images::new(
            root.path(),
            &static_files(&[
                "image.png",
                "copy.png",
                "notes.data",
                "broken.png",
                "unused.png",
            ]),
        )?;
        fs::remove_file(unused)?;
        let png_image = Image {
            hash: ContentHash::of_reader(&mut png.as_slice())?,
            size: Some(Size {
                width: 300,
                height: 200,
            }),
        };
        let data_image = Image {
            hash: ContentHash::of_reader(&mut b"notes".as_slice())?,
            size: None,
        };
        assert_eq!(
            images.get("/entries/post", "../image.png?x=1#y")?,
            Some(png_image)
        );
        assert_eq!(images.get("/entries/post", "/copy.png")?, Some(png_image));
        assert_eq!(
            images.get("/entries/post", "/notes.data")?,
            Some(data_image)
        );
        let error = images
            .get("/entries/post", "/broken.png")
            .err()
            .context("accepted a broken PNG")?;
        assert!(
            format!("{error:#}").contains(&format!(
                "cannot get the size of image {}",
                broken.display()
            )),
            "{error:#}"
        );
        fs::remove_file(image)?;
        fs::remove_file(data)?;
        assert_eq!(images.get("/elsewhere", "/%69mage.png")?, Some(png_image));
        assert_eq!(
            images.get("/elsewhere", "/notes.data?x=2")?,
            Some(data_image)
        );
        for url in [
            "https://example.com/image.png",
            "//example.com/image.png",
            "/missing.png",
            "#same",
            "%zz",
        ] {
            assert_eq!(images.get("/post", url)?, None);
        }
        assert_eq!(
            images.into_hashes(),
            BTreeMap::from([
                (PathBuf::from("static/copy.png"), png_image.hash),
                (PathBuf::from("static/image.png"), png_image.hash),
                (PathBuf::from("static/notes.data"), data_image.hash),
            ])
        );
        Ok(())
    }

    #[test]
    fn recognizes_image_extensions_case_insensitively() {
        for (name, expected) in [
            ("a.png", true),
            ("a.JPG", true),
            ("a.jpeg", true),
            ("a.Gif", true),
            ("a.webp", true),
            ("a.svg", false),
            ("a.avif", false),
            ("png", false),
        ] {
            assert_eq!(has_image_extension(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn handles_all_orientation_values_and_invalid_metadata() -> Result<()> {
        let fixture = include_bytes!("../tests/fixtures/images/exif.jpg");
        let marker = [0x12, 0x01, 0x03, 0, 1, 0, 0, 0, 6, 0, 0, 0];
        let offset = fixture
            .windows(marker.len())
            .position(|bytes| bytes == marker)
            .context("missing orientation field")?;
        for orientation in 0..=9 {
            let mut bytes = fixture.to_vec();
            *bytes
                .get_mut(offset + 8)
                .context("short orientation field")? = orientation;
            let (width, height) = match orientation {
                1..=4 => (300, 200),
                5..=8 => (200, 300),
                _ => {
                    let error = probe_error(&bytes, true)?;
                    assert!(
                        error.contains(&format!(
                            "EXIF Orientation {orientation} is not between 1 and 8"
                        )),
                        "{error}"
                    );
                    continue;
                }
            };
            assert_eq!(
                probe(&mut Cursor::new(bytes), true)?,
                Some(Size { width, height })
            );
        }
        let mut invalid = fixture.to_vec();
        *invalid
            .get_mut(offset + 2)
            .context("short orientation field")? = 2; // ASCII, not an integer
        let error = probe_error(&invalid, true)?;
        assert!(error.contains("EXIF"), "{error}");
        assert!(!is_invalid_data(&io::Error::from(
            ErrorKind::PermissionDenied
        )));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rechecks_input_paths_before_probing() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("static"))?;
        let image = root.path().join("static/image.png");
        fs::write(&image, b"image")?;
        let mut images = Images::new(root.path(), &static_files(&["image.png"]))?;
        fs::remove_file(&image)?;
        std::os::unix::fs::symlink(root.path().join("outside.png"), &image)?;
        let error = images
            .get("/post", "/image.png")
            .err()
            .context("accepted symlink")?;
        assert!(format!("{error:#}").contains("symlinks are not supported"));
        Ok(())
    }
}
