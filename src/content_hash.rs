use sha2::{Digest as _, Sha256};
use std::{
    fmt::Write as _,
    io::{self, Read, Write},
};

/// Hex digits of the hash written into URLs. A URL only has to differ from the previous
/// version of the same file, so 64 bits are enough.
const URL_DIGITS: usize = 16;

/// SHA-256 of a file's bytes. URLs carry a prefix; copies are compared in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContentHash([u8; 32]);

impl ContentHash {
    pub(crate) fn of_reader(reader: &mut impl Read) -> io::Result<Self> {
        let mut writer = HashingWriter::new(io::sink());
        io::copy(reader, &mut writer)?;
        Ok(writer.finish())
    }

    pub(crate) fn url_version(&self) -> String {
        let mut version = String::with_capacity(URL_DIGITS);
        for byte in self.0.iter().take(URL_DIGITS / 2) {
            // Writing to a String is infallible.
            let _ = write!(version, "{byte:02x}");
        }
        version
    }
}

/// Hashes exactly the bytes that the inner writer accepts.
pub(crate) struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    pub(crate) fn finish(self) -> ContentHash {
        ContentHash(self.hasher.finalize().into())
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(buf.get(..written).unwrap_or_default());
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ShortWriter(Vec<u8>);

    impl Write for ShortWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let accepted = buf.get(..buf.len().min(3)).unwrap_or_default();
            self.0.extend_from_slice(accepted);
            Ok(accepted.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn url_version_is_the_sha256_prefix() -> io::Result<()> {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223...
        let hash = ContentHash::of_reader(&mut b"abc".as_slice())?;
        assert_eq!(hash.url_version(), "ba7816bf8f01cfea");
        Ok(())
    }

    #[test]
    fn hashes_only_the_bytes_the_writer_accepted() -> io::Result<()> {
        let mut writer = HashingWriter::new(ShortWriter(Vec::new()));
        assert_eq!(writer.write(b"abcdef")?, 3);
        assert_eq!(
            writer.finish(),
            ContentHash::of_reader(&mut b"abc".as_slice())?
        );
        Ok(())
    }
}
