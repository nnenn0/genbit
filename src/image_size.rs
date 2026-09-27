use crate::{input::SiteInput, route};
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

/// A build-local cache, including files that are not images. Only rendered images are read.
pub(crate) struct ImageSizes<'a> {
    input: SiteInput<'a>,
    files: BTreeMap<String, PathBuf>,
    sizes: BTreeMap<PathBuf, Option<Size>>,
}

impl<'a> ImageSizes<'a> {
    pub(crate) fn new(root: &'a Path, files: &[PathBuf]) -> Result<Self> {
        let static_root = root.join("static");
        let files = files
            .iter()
            .filter(|path| path.file_name().is_none_or(|name| name != ".gitkeep"))
            .map(|path| {
                let relative = path.strip_prefix(&static_root)?;
                let url = format!(
                    "/{}",
                    relative.to_str().context("static path must be UTF-8")?
                );
                Ok((url, path.strip_prefix(root)?.to_path_buf()))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            input: SiteInput::new(root),
            files,
            sizes: BTreeMap::new(),
        })
    }

    pub(crate) fn get(&mut self, page: &str, url: &str) -> Result<Option<Size>> {
        // Invalid and missing targets are diagnosed by the existing link validator.
        let Ok(Some(target)) = route::resolve_link(page, url) else {
            return Ok(None);
        };
        let Some(path) = self.files.get(&target) else {
            return Ok(None);
        };
        if let Some(size) = self.sizes.get(path) {
            return Ok(*size);
        }
        let file = self.input.open_file(path)?;
        let size =
            probe(&mut BufReader::new(file), has_image_extension(path)).with_context(|| {
                format!(
                    "cannot get the size of image {}",
                    self.input.root().join(path).display()
                )
            })?;
        self.sizes.insert(path.clone(), size);
        Ok(size)
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
    fn only_referenced_images_are_read_and_results_are_cached_by_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let static_root = root.path().join("static");
        fs::create_dir(&static_root)?;
        let image = static_root.join("image.png");
        let data = static_root.join("notes.data");
        let broken = static_root.join("broken.png");
        let unused = static_root.join("unused.png");
        fs::write(&image, include_bytes!("../tests/fixtures/images/basic.png"))?;
        fs::write(&data, b"notes")?;
        fs::write(&broken, b"broken")?;
        fs::write(&unused, b"unused")?;
        let mut images = ImageSizes::new(
            root.path(),
            &[image.clone(), data.clone(), broken.clone(), unused.clone()],
        )?;
        fs::remove_file(unused)?;
        assert_eq!(
            images.get("/entries/post", "../image.png?x=1#y")?,
            Some(Size {
                width: 300,
                height: 200
            })
        );
        assert_eq!(images.get("/entries/post", "/notes.data")?, None);
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
        assert_eq!(
            images.get("/elsewhere", "/%69mage.png")?,
            Some(Size {
                width: 300,
                height: 200
            })
        );
        assert_eq!(images.get("/elsewhere", "/notes.data?x=2")?, None);
        for url in [
            "https://example.com/image.png",
            "//example.com/image.png",
            "/missing.png",
            "#same",
            "%zz",
        ] {
            assert_eq!(images.get("/post", url)?, None);
        }
        assert_eq!(images.sizes.len(), 2);
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
        let mut images = ImageSizes::new(root.path(), std::slice::from_ref(&image))?;
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
