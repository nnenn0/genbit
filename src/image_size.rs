use crate::{input::SiteInput, route};
use anyhow::{Context, Result};
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

/// A build-local cache, including unsuccessful probes. Only rendered images are read.
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
        let size = probe(&mut BufReader::new(file)).with_context(|| {
            format!(
                "cannot read image metadata {}",
                self.input.root().join(path).display()
            )
        })?;
        self.sizes.insert(path.clone(), size);
        Ok(size)
    }
}

/// Reads dimensions without decoding pixels. Metadata errors are optional; filesystem errors are not.
fn probe(reader: &mut (impl BufRead + Seek)) -> io::Result<Option<Size>> {
    let format = match imagesize::reader_type(&mut *reader) {
        Ok(format) => format,
        Err(imagesize::ImageError::IoError(error)) => return invalid_metadata(error),
        Err(_) => return Ok(None),
    };
    if !matches!(
        format,
        imagesize::ImageType::Png
            | imagesize::ImageType::Jpeg
            | imagesize::ImageType::Gif
            | imagesize::ImageType::Webp
    ) {
        return Ok(None);
    }
    reader.rewind()?;
    let dimensions = match imagesize::reader_size(&mut *reader) {
        Ok(size) => size,
        Err(imagesize::ImageError::IoError(error)) => return invalid_metadata(error),
        Err(_) => return Ok(None),
    };
    if dimensions.width == 0 || dimensions.height == 0 {
        return Ok(None);
    }
    let mut size = Size {
        width: dimensions.width,
        height: dimensions.height,
    };
    if format != imagesize::ImageType::Gif {
        reader.rewind()?;
        match exif::Reader::new().read_from_container(reader) {
            Ok(metadata) => {
                if let Some(field) = metadata.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
                    match field.value.get_uint(0) {
                        Some(1..=4) => {}
                        Some(5..=8) if format == imagesize::ImageType::Jpeg => {
                            std::mem::swap(&mut size.width, &mut size.height);
                        }
                        // PNG/WebP metadata may follow pixel data and be ignored by browsers.
                        // Avoid promising a ratio without implementing container-order parsing.
                        _ => return Ok(None),
                    }
                }
            }
            Err(exif::Error::NotFound(_)) => {}
            Err(exif::Error::Io(error)) => return invalid_metadata(error),
            Err(_) => return Ok(None),
        }
    }
    Ok(Some(size))
}

fn invalid_metadata(error: io::Error) -> io::Result<Option<Size>> {
    match error.kind() {
        ErrorKind::UnexpectedEof | ErrorKind::InvalidData | ErrorKind::InvalidInput => Ok(None),
        _ => Err(error),
    }
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
                probe(&mut Cursor::new(bytes))?,
                Some(Size { width, height })
            );
        }
        Ok(())
    }

    #[test]
    fn corrupt_and_unsupported_images_are_optional() -> Result<()> {
        for bytes in [
            b"".as_slice(),
            b"not an image",
            b"<svg width='300' height='200'></svg>",
            include_bytes!("../tests/fixtures/images/exif.png").as_slice(),
            include_bytes!("../tests/fixtures/images/exif.webp").as_slice(),
        ] {
            assert_eq!(probe(&mut Cursor::new(bytes))?, None);
        }
        // A truncated PNG chunk must not hide a failed EXIF scan.
        let png = include_bytes!("../tests/fixtures/images/basic.png");
        assert_eq!(
            probe(&mut Cursor::new(png.get(..40).context("short fixture")?))?,
            None
        );
        Ok(())
    }

    #[test]
    fn only_referenced_images_are_read_and_results_are_cached_by_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let static_root = root.path().join("static");
        fs::create_dir(&static_root)?;
        let image = static_root.join("image.png");
        let broken = static_root.join("broken.png");
        let unused = static_root.join("unused.png");
        fs::write(&image, include_bytes!("../tests/fixtures/images/basic.png"))?;
        fs::write(&broken, b"broken")?;
        fs::write(&unused, b"unused")?;
        let mut images = ImageSizes::new(
            root.path(),
            &[image.clone(), broken.clone(), unused.clone()],
        )?;
        fs::remove_file(unused)?;
        assert_eq!(
            images.get("/entries/post", "../image.png?x=1#y")?,
            Some(Size {
                width: 300,
                height: 200
            })
        );
        assert_eq!(images.get("/entries/post", "/broken.png")?, None);
        fs::remove_file(image)?;
        fs::remove_file(broken)?;
        assert_eq!(
            images.get("/elsewhere", "/%69mage.png")?,
            Some(Size {
                width: 300,
                height: 200
            })
        );
        assert_eq!(images.get("/elsewhere", "/broken.png?x=2")?, None);
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
            let expected = match orientation {
                1..=4 => Some(Size {
                    width: 300,
                    height: 200,
                }),
                5..=8 => Some(Size {
                    width: 200,
                    height: 300,
                }),
                _ => None,
            };
            assert_eq!(probe(&mut Cursor::new(bytes))?, expected);
        }
        let mut invalid = fixture.to_vec();
        *invalid
            .get_mut(offset + 2)
            .context("short orientation field")? = 2; // ASCII, not an integer
        assert_eq!(probe(&mut Cursor::new(invalid))?, None);
        assert!(invalid_metadata(io::Error::from(ErrorKind::PermissionDenied)).is_err());
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
