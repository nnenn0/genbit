use crate::route;
use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Intrinsic image dimensions in pixels, as written to `<img width height>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Formats whose headers state the displayed size without decoding the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Png,
    Gif,
    WebP,
}

impl Format {
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some(Self::Png),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }
}

const WEBP_EXIF_FLAG: u8 = 0x08;

/// Sizes of local images keyed by the URL path that serves them.
#[derive(Default)]
pub(crate) struct ImageSizes(BTreeMap<String, Size>);

impl ImageSizes {
    pub(crate) fn insert(&mut self, url: String, size: Size) {
        self.0.insert(url, size);
    }

    /// Returns the size of the local image that `link` points to from `page_url`.
    pub(crate) fn find(&self, page_url: &str, link: &str) -> Option<Size> {
        // Links that do not resolve are reported by link validation instead.
        let target = route::resolve_link(page_url, link).ok().flatten()?;
        self.0.get(&target).copied()
    }
}

/// Reads the size a browser shows for the image, or `None` when EXIF metadata may rotate it.
pub(crate) fn read(format: Format, reader: &mut (impl Read + Seek)) -> Result<Option<Size>> {
    match format {
        Format::Png => png(reader),
        Format::Gif => gif(reader).map(Some),
        Format::WebP => webp(reader),
    }
}

fn png(reader: &mut (impl Read + Seek)) -> Result<Option<Size>> {
    ensure!(
        read_array(reader)? == *b"\x89PNG\r\n\x1a\n",
        "not a PNG file"
    );
    let (length, kind) = png_chunk_header(reader)?;
    ensure!(
        kind == *b"IHDR" && length == 13,
        "PNG does not start with IHDR"
    );
    let header: [u8; 13] = read_array(reader)?;
    let size = checked_size(be_u32(&header, 0), be_u32(&header, 4))?;
    skip(reader, 4)?;
    // Browsers may rotate by the orientation in eXIf, which can appear anywhere before IEND.
    loop {
        let (length, kind) = png_chunk_header(reader)?;
        match &kind {
            b"eXIf" => return Ok(None),
            b"IEND" => return Ok(Some(size)),
            _ => skip(reader, u64::from(length) + 4)?,
        }
    }
}

fn png_chunk_header(reader: &mut impl Read) -> Result<(u32, [u8; 4])> {
    let header: [u8; 8] = read_array(reader)?;
    let length = be_u32(&header, 0);
    ensure!(length <= 0x7fff_ffff, "invalid PNG chunk length");
    Ok((length, bytes(&header, 4)))
}

fn gif(reader: &mut impl Read) -> Result<Size> {
    let header: [u8; 10] = read_array(reader)?;
    ensure!(
        header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a"),
        "not a GIF file"
    );
    checked_size(u32::from(le_u16(&header, 6)), u32::from(le_u16(&header, 8)))
}

fn webp(reader: &mut (impl Read + Seek)) -> Result<Option<Size>> {
    let header: [u8; 12] = read_array(reader)?;
    ensure!(
        header.starts_with(b"RIFF") && header.get(8..) == Some(b"WEBP".as_slice()),
        "not a WebP file"
    );
    let (kind, length) = riff_chunk_header(reader)?;
    match &kind {
        b"VP8 " => {
            let frame: [u8; 10] = read_array(reader)?;
            ensure!(
                length >= 10 && frame.first().is_some_and(|tag| tag & 1 == 0),
                "WebP VP8 data is not a key frame"
            );
            ensure!(
                frame.get(3..6) == Some([0x9d, 0x01, 0x2a].as_slice()),
                "invalid WebP VP8 start code"
            );
            // The top two bits are an upscaling hint that decoders do not apply.
            checked_size(
                u32::from(le_u16(&frame, 6) & 0x3fff),
                u32::from(le_u16(&frame, 8) & 0x3fff),
            )
            .map(Some)
        }
        b"VP8L" => {
            let data: [u8; 5] = read_array(reader)?;
            ensure!(
                length >= 5 && data.first() == Some(&0x2f),
                "invalid WebP VP8L signature"
            );
            let bits = le_u32(&data, 1);
            ensure!(bits >> 29 == 0, "unsupported WebP VP8L version");
            Ok(Some(Size {
                width: (bits & 0x3fff) + 1,
                height: ((bits >> 14) & 0x3fff) + 1,
            }))
        }
        b"VP8X" => {
            let data: [u8; 10] = read_array(reader)?;
            ensure!(length >= 10, "invalid WebP VP8X chunk");
            let size = Size {
                width: le_u24(&data, 4) + 1,
                height: le_u24(&data, 7) + 1,
            };
            if data
                .first()
                .is_some_and(|flags| flags & WEBP_EXIF_FLAG != 0)
            {
                return Ok(None);
            }
            skip(reader, u64::from(length) - 10 + u64::from(length & 1))?;
            // Also catch an EXIF chunk whose flag was not set.
            while !at_end(reader)? {
                let (kind, length) = riff_chunk_header(reader)?;
                if kind == *b"EXIF" {
                    return Ok(None);
                }
                skip(reader, u64::from(length) + u64::from(length & 1))?;
            }
            Ok(Some(size))
        }
        _ => bail!("unknown WebP chunk {}", String::from_utf8_lossy(&kind)),
    }
}

fn riff_chunk_header(reader: &mut impl Read) -> Result<([u8; 4], u32)> {
    let header: [u8; 8] = read_array(reader)?;
    Ok((bytes(&header, 0), le_u32(&header, 4)))
}

fn checked_size(width: u32, height: u32) -> Result<Size> {
    ensure!(width > 0 && height > 0, "image has no pixels");
    Ok(Size { width, height })
}

fn read_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    reader
        .read_exact(&mut bytes)
        .context("image header ends early")?;
    Ok(bytes)
}

/// Skips `length` bytes, failing when that passes the end of the file.
fn skip(reader: &mut (impl Read + Seek), length: u64) -> Result<()> {
    let target = reader
        .stream_position()?
        .checked_add(length)
        .context("image chunk is too long")?;
    let end = reader.seek(SeekFrom::End(0))?;
    ensure!(target <= end, "image header ends early");
    reader.seek(SeekFrom::Start(target))?;
    Ok(())
}

fn at_end(reader: &mut impl Seek) -> Result<bool> {
    let position = reader.stream_position()?;
    let end = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(position))?;
    Ok(position == end)
}

fn bytes<const N: usize>(data: &[u8], start: usize) -> [u8; N] {
    let mut bytes = [0; N];
    if let Some(slice) = data.get(start..start + N) {
        bytes.copy_from_slice(slice);
    }
    bytes
}

fn be_u32(data: &[u8], start: usize) -> u32 {
    u32::from_be_bytes(bytes(data, start))
}

fn le_u32(data: &[u8], start: usize) -> u32 {
    u32::from_le_bytes(bytes(data, start))
}

fn le_u16(data: &[u8], start: usize) -> u16 {
    u16::from_le_bytes(bytes(data, start))
}

fn le_u24(data: &[u8], start: usize) -> u32 {
    let [first, second, third] = bytes(data, start);
    u32::from_le_bytes([first, second, third, 0])
}

#[cfg(test)]
mod tests {
    use super::{Format, ImageSizes, Size, read};
    use anyhow::{Context, Result};
    use std::io::Cursor;
    use std::path::Path;

    const FIXTURE_SIZE: Size = Size {
        width: 300,
        height: 200,
    };

    fn read_bytes(format: Format, bytes: &[u8]) -> Result<Option<Size>> {
        read(format, &mut Cursor::new(bytes))
    }

    fn prefix(bytes: &[u8], length: usize) -> &[u8] {
        bytes.get(..length).unwrap_or_default()
    }

    #[test]
    fn reads_sizes_written_by_real_encoders() -> Result<()> {
        for (format, bytes) in [
            (
                Format::Png,
                include_bytes!("../tests/fixtures/images/basic.png").as_slice(),
            ),
            (
                Format::Gif,
                include_bytes!("../tests/fixtures/images/basic.gif").as_slice(),
            ),
            (
                Format::WebP,
                include_bytes!("../tests/fixtures/images/lossy.webp").as_slice(),
            ),
            (
                Format::WebP,
                include_bytes!("../tests/fixtures/images/lossless.webp").as_slice(),
            ),
            (
                Format::WebP,
                include_bytes!("../tests/fixtures/images/alpha.webp").as_slice(),
            ),
            (
                Format::WebP,
                include_bytes!("../tests/fixtures/images/animated.webp").as_slice(),
            ),
        ] {
            assert_eq!(read_bytes(format, bytes)?, Some(FIXTURE_SIZE), "{format:?}");
        }
        Ok(())
    }

    #[test]
    fn leaves_images_with_exif_unsized() -> Result<()> {
        assert_eq!(
            read_bytes(
                Format::Png,
                include_bytes!("../tests/fixtures/images/exif.png")
            )?,
            None
        );
        assert_eq!(
            read_bytes(
                Format::WebP,
                include_bytes!("../tests/fixtures/images/exif.webp")
            )?,
            None
        );
        Ok(())
    }

    #[test]
    fn finds_exif_after_image_data_in_png() -> Result<()> {
        let png = include_bytes!("../tests/fixtures/images/basic.png");
        let end = png
            .windows(4)
            .rposition(|window| window == b"IEND")
            .context("fixture has no IEND")?
            - 4;
        let (image, trailer) = png.split_at_checked(end).context("invalid IEND offset")?;
        // An eXIf chunk with 2 data bytes and a CRC, which is not checked.
        let bytes = [image, b"\0\0\0\x02eXIfMM\0\0\0\0", trailer].concat();
        assert_eq!(read_bytes(Format::Png, &bytes)?, None);
        Ok(())
    }

    #[test]
    fn rejects_truncated_and_mismatched_files() {
        let png = include_bytes!("../tests/fixtures/images/basic.png");
        let webp = include_bytes!("../tests/fixtures/images/lossy.webp");
        for (format, bytes) in [
            (Format::Png, prefix(png, 20)),
            (Format::Png, prefix(png, png.len() - 12)),
            (Format::Png, webp.as_slice()),
            (Format::Gif, png.as_slice()),
            (Format::Gif, b"GIF89a\x00\x00\x01\x00".as_slice()),
            (Format::WebP, prefix(webp, 24)),
            (Format::WebP, png.as_slice()),
            (
                Format::WebP,
                b"RIFF\0\0\0\0WEBPVP8 \x0a\0\0\0\0\0\0\0\0\0\0\0\0\0",
            ),
        ] {
            assert!(
                read_bytes(format, bytes).is_err(),
                "{format:?} accepted {bytes:?}"
            );
        }
    }

    #[test]
    fn chooses_formats_by_extension() {
        for (path, format) in [
            ("a.png", Some(Format::Png)),
            ("a.PNG", Some(Format::Png)),
            ("a.gif", Some(Format::Gif)),
            ("a.webp", Some(Format::WebP)),
            ("a.jpg", None),
            ("a.avif", None),
            ("a.svg", None),
            ("png", None),
        ] {
            assert_eq!(Format::from_path(Path::new(path)), format, "{path}");
        }
    }

    #[test]
    fn finds_sizes_from_links_resolved_like_a_browser() {
        let mut sizes = ImageSizes::default();
        sizes.insert("/img/a.png".to_owned(), FIXTURE_SIZE);
        sizes.insert("/entries/画像.png".to_owned(), FIXTURE_SIZE);
        for (page, link, expected) in [
            ("/entries/post", "/img/a.png", Some(FIXTURE_SIZE)),
            ("/entries/post", "../img/a.png?v=2#x", Some(FIXTURE_SIZE)),
            (
                "/entries/post",
                "%E7%94%BB%E5%83%8F.png",
                Some(FIXTURE_SIZE),
            ),
            ("/entries/post", "img/a.png", None),
            ("/entries/post", "/img/A.png", None),
            ("/entries/post", "https://example.com/img/a.png", None),
            ("/entries/post", "a%zz.png", None),
        ] {
            assert_eq!(sizes.find(page, link), expected, "{page} {link}");
        }
    }
}
