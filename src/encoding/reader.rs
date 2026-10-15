//! [`DecodeReader`]: decoding a byte stream incrementally.

use std::io::{self, BufRead, Read};

use super::detect::{Mode, SNIFF_WINDOW, declared_char, sniff};
use super::{Decoder, GedcomEncoding};

/// A reader that decodes GEDCOM bytes of any encoding into UTF-8 text.
///
/// It reads up to 64 KiB (or the whole input if shorter) to choose the
/// decoding with the rules of [`decode`](super::decode), then decodes the rest
/// chunk by chunk, so memory stays bounded whatever the input size. The
/// output never contains a byte order mark; line terminators are kept as
/// they were.
///
/// # Example
///
/// ```rust
/// use std::io::Read;
/// use oxidged::encoding::{DecodeReader, GedcomEncoding};
///
/// let bytes: &[u8] = b"0 HEAD\r1 CHAR ANSEL\r1 NOTE Jos\xE2e\r0 TRLR\r";
/// let mut reader = DecodeReader::new(bytes)?;
/// assert_eq!(reader.encoding(), GedcomEncoding::Ansel);
/// let mut text = String::new();
/// reader.read_to_string(&mut text)?;
/// assert!(text.contains("José"));
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct DecodeReader<R> {
    inner: R,
    decoder: Decoder,
    /// Decoded text not yet consumed starts at `pos`.
    out: String,
    pos: usize,
    eof: bool,
    encoding: GedcomEncoding,
    declared: Option<String>,
}

impl<R: BufRead> DecodeReader<R> {
    /// Reads the start of `inner` and chooses the decoding.
    ///
    /// # Errors
    ///
    /// Only the I/O errors of `inner`.
    pub fn new(mut inner: R) -> io::Result<Self> {
        let mut window = Vec::with_capacity(8 * 1024);
        let mut eof = false;
        while window.len() < SNIFF_WINDOW {
            let chunk = match inner.fill_buf() {
                Ok(chunk) => chunk,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if chunk.is_empty() {
                eof = true;
                break;
            }
            let take = chunk.len().min(SNIFF_WINDOW - window.len());
            window.extend_from_slice(chunk.get(..take).unwrap_or_default());
            inner.consume(take);
        }
        let sniffed = sniff(&window, eof);
        let mut decoder = Decoder::new(sniffed.mode);
        let mut out = String::with_capacity(window.len() + window.len() / 8);
        decoder.decode(window.get(sniffed.bom..).unwrap_or_default(), &mut out);
        if eof {
            decoder.finish(&mut out);
        }
        let declared = sniffed.declared.or_else(|| {
            matches!(sniffed.mode, Mode::Utf16 { .. })
                .then(|| {
                    declared_char(out.as_bytes()).map(|v| String::from_utf8_lossy(v).into_owned())
                })
                .flatten()
        });
        Ok(Self {
            inner,
            decoder,
            out,
            pos: 0,
            eof,
            encoding: sniffed.encoding,
            declared,
        })
    }

    /// The encoding chosen for the input.
    #[must_use]
    pub fn encoding(&self) -> GedcomEncoding {
        self.encoding
    }

    /// The `HEAD.CHAR` payload as written, if the sniffed window holds one.
    #[must_use]
    pub fn declared(&self) -> Option<&str> {
        self.declared.as_deref()
    }

    /// Returns the inner reader. Bytes it buffered but this reader did not
    /// decode yet are lost.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Decodes more input until some output is ready or the input ends.
    fn refill(&mut self) -> io::Result<()> {
        // Drop what was consumed; keep what is held back.
        self.out.drain(..self.pos);
        self.decoder.drained(self.pos);
        self.pos = 0;
        while self.decoder.stable_len(&self.out) == 0 && !self.eof {
            let chunk = match self.inner.fill_buf() {
                Ok(chunk) => chunk,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if chunk.is_empty() {
                self.decoder.finish(&mut self.out);
                self.eof = true;
                break;
            }
            let len = chunk.len();
            self.decoder.decode(chunk, &mut self.out);
            self.inner.consume(len);
        }
        Ok(())
    }
}

impl<R: BufRead> Read for DecodeReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..n), available.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.consume(n);
        Ok(n)
    }
}

impl<R: BufRead> BufRead for DecodeReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.decoder.stable_len(&self.out) {
            self.refill()?;
        }
        let end = self.decoder.stable_len(&self.out);
        Ok(self.out.as_bytes().get(self.pos..end).unwrap_or_default())
    }

    fn consume(&mut self, amt: usize) {
        self.pos = (self.pos + amt).min(self.decoder.stable_len(&self.out));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn read_all(bytes: &[u8], capacity: usize) -> (String, GedcomEncoding) {
        let mut reader = DecodeReader::new(BufReader::with_capacity(capacity, bytes)).unwrap();
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        (text, reader.encoding())
    }

    #[test]
    fn stream_decodes_like_memory() {
        let mut ansel = b"0 HEAD\r\n1 CHAR ANSEL\r\n".to_vec();
        for _ in 0..5000 {
            ansel.extend_from_slice(b"1 NOTE Jos\xE2e Ma\xE2\r\n2 CONC ria \xA1\xE2od\xE2z\r\n");
        }
        let mut late_utf8 = b"0 HEAD\n1 CHAR ANSI\n".to_vec();
        late_utf8.extend(std::iter::repeat_n(b'a', SNIFF_WINDOW));
        late_utf8.extend_from_slice("\n1 NOTE Zoë\n".as_bytes());
        // UTF-8 in the window, then more stray bytes than sequences past
        // it: the window decides, in memory as in the stream.
        let mut late_strays = "0 HEAD\n1 CHAR ASCII\n1 NOTE Zoë\n".as_bytes().to_vec();
        late_strays.extend(std::iter::repeat_n(b'a', SNIFF_WINDOW));
        late_strays.extend_from_slice(b"\n1 NOTE Ren\xE9e \xE9\xE9\xE9\n");
        for input in [
            ansel.as_slice(),
            late_utf8.as_slice(),
            late_strays.as_slice(),
            "0 HEAD\n1 NOTE é\n".as_bytes(),
        ] {
            let memory = super::super::decode(input);
            for capacity in [1, 2, 3, 7, 4096] {
                let (text, encoding) = read_all(input, capacity);
                assert_eq!(text, memory.text, "capacity {capacity}");
                // Only an ASCII window lets the rest decide in memory.
                if input.get(..SNIFF_WINDOW).is_none_or(|w| !w.is_ascii()) {
                    assert_eq!(encoding, memory.encoding);
                }
            }
        }
        let memory = super::super::decode(&late_strays);
        assert_eq!(memory.encoding, GedcomEncoding::Utf8);
        assert!(memory.text.contains("Zoë") && memory.text.contains("Renée ééé"));
    }

    #[test]
    fn empty_input() {
        assert_eq!(read_all(b"", 8), (String::new(), GedcomEncoding::Ascii));
    }
}
