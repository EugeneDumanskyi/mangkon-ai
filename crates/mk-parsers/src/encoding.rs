//! Text encodings (US-02.3 AC3, NFR-COMP-06): UTF-8, UTF-16 with a byte-order mark, and
//! Windows-1252. Detection is one pass over the file; a streaming decoder then turns the bytes
//! into UTF-8 for the parser, so no format holds the whole file.

use std::io::{self, Read, SeekFrom};

use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

use crate::content::{ParseError, Source};

const CHUNK: usize = 64 << 10;

/// The encoding a text file is read in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl TextEncoding {
    fn encoding(self) -> &'static Encoding {
        match self {
            TextEncoding::Utf8 => UTF_8,
            TextEncoding::Utf16Le => UTF_16LE,
            TextEncoding::Utf16Be => UTF_16BE,
            TextEncoding::Windows1252 => WINDOWS_1252,
        }
    }
}

/// A byte that a Windows-1252 text never holds: NUL and the C0 controls other than tab, line
/// feed, form feed and carriage return, and the five bytes Windows-1252 leaves undefined.
/// ASSUMPTION: a file with one of them is binary, not text in an unknown encoding.
fn is_binary(b: u8) -> bool {
    matches!(b, 0x00..=0x08 | 0x0b | 0x0e..=0x1f | 0x81 | 0x8d | 0x8f | 0x90 | 0x9d)
}

/// Detects the encoding of `input`, reading it once from the start:
///
/// 1. A byte-order mark picks UTF-8, UTF-16 LE or UTF-16 BE (UTF-8 with a mark must then be
///    valid UTF-8).
/// 2. Valid UTF-8 is UTF-8 (NUL characters are kept as text).
/// 3. UTF-8 cut inside its last character fails: no repair of a cut tail.
/// 4. Bytes that look binary ([`is_binary`]) fail.
/// 5. Anything else is Windows-1252.
///
/// Failures are "Unknown text encoding". The input is left at its start.
pub fn detect(input: &mut dyn Source) -> Result<TextEncoding, ParseError> {
    input.seek(SeekFrom::Start(0)).map_err(ParseError::io)?;
    let mut buf = vec![0u8; CHUNK];
    let mut first = true;
    let mut utf8_bom = false;
    let mut utf8 = true;
    let mut binary = false;
    // The start of a UTF-8 character cut by the chunk edge.
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = read_full(input, &mut buf).map_err(ParseError::io)?;
        let mut chunk = &buf[..n];
        if first {
            first = false;
            if chunk.starts_with(&[0xff, 0xfe]) {
                rewind(input)?;
                return Ok(TextEncoding::Utf16Le);
            }
            if chunk.starts_with(&[0xfe, 0xff]) {
                rewind(input)?;
                return Ok(TextEncoding::Utf16Be);
            }
            if let Some(rest) = chunk.strip_prefix(&[0xef, 0xbb, 0xbf]) {
                utf8_bom = true;
                chunk = rest;
            }
        }
        if n == 0 {
            break;
        }
        binary = binary || chunk.iter().copied().any(is_binary);
        if utf8 {
            carry.extend_from_slice(chunk);
            match std::str::from_utf8(&carry) {
                Ok(_) => carry.clear(),
                Err(e) if e.error_len().is_none() => {
                    let tail = carry.split_off(e.valid_up_to());
                    carry = tail;
                }
                Err(_) => {
                    utf8 = false;
                    carry.clear();
                }
            }
        }
        if !utf8 && (binary || utf8_bom) {
            break;
        }
    }
    rewind(input)?;
    if utf8 && carry.is_empty() {
        return Ok(TextEncoding::Utf8);
    }
    if utf8 || binary || utf8_bom {
        return Err(ParseError::unknown_encoding());
    }
    Ok(TextEncoding::Windows1252)
}

fn rewind(input: &mut dyn Source) -> Result<(), ParseError> {
    input
        .seek(SeekFrom::Start(0))
        .map(|_| ())
        .map_err(ParseError::io)
}

/// Reads until `buf` is full or the input ends.
fn read_full(input: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match input.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// `input` from its start as UTF-8 bytes, decoded as it is read. A byte-order mark is
/// dropped.
pub struct Decoded<'a> {
    inner: &'a mut dyn Source,
    decoder: Decoder,
    input: Vec<u8>,
    in_pos: usize,
    in_len: usize,
    eof: bool,
    out: Vec<u8>,
    out_pos: usize,
    out_len: usize,
    done: bool,
}

impl<'a> Decoded<'a> {
    pub fn new(inner: &'a mut dyn Source, encoding: TextEncoding) -> Result<Self, ParseError> {
        rewind(inner)?;
        let decoder = match encoding {
            TextEncoding::Windows1252 => encoding.encoding().new_decoder_without_bom_handling(),
            _ => encoding.encoding().new_decoder_with_bom_removal(),
        };
        Ok(Decoded {
            inner,
            decoder,
            input: vec![0; CHUNK],
            in_pos: 0,
            in_len: 0,
            eof: false,
            out: vec![0; CHUNK * 3 + 16],
            out_pos: 0,
            out_len: 0,
            done: false,
        })
    }
}

impl Read for Decoded<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.out_pos < self.out_len {
                let n = buf.len().min(self.out_len - self.out_pos);
                buf[..n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
                self.out_pos += n;
                return Ok(n);
            }
            if self.done {
                return Ok(0);
            }
            if self.in_pos == self.in_len && !self.eof {
                self.in_len = self.inner.read(&mut self.input)?;
                self.in_pos = 0;
                self.eof = self.in_len == 0;
            }
            // Malformed UTF-16 becomes U+FFFD, as browsers do; detection checked the others.
            let (result, read, written, _) = self.decoder.decode_to_utf8(
                &self.input[self.in_pos..self.in_len],
                &mut self.out,
                self.eof,
            );
            self.in_pos += read;
            self.out_pos = 0;
            self.out_len = written;
            if result == CoderResult::InputEmpty && self.eof {
                self.done = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn detect_bytes(b: &[u8]) -> Result<TextEncoding, ParseError> {
        detect(&mut Cursor::new(b))
    }

    fn decode(b: &[u8]) -> String {
        let mut c = Cursor::new(b);
        let enc = detect(&mut c).unwrap();
        let mut s = String::new();
        Decoded::new(&mut c, enc)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        s
    }

    /// NFR-COMP-06, US-02.3 AC3: the three encodings, with and without marks.
    #[test]
    fn nfr_comp_06_detects_and_decodes() {
        assert_eq!(detect_bytes(b"plain"), Ok(TextEncoding::Utf8));
        assert_eq!(detect_bytes(b""), Ok(TextEncoding::Utf8));
        assert_eq!(decode("\u{feff}Grüße".as_bytes()), "Grüße");
        assert_eq!(decode(b"\xff\xfeh\x00\xe9\x00"), "hé");
        assert_eq!(decode(b"\xfe\xff\x00h\x00\xe9"), "hé");
        assert_eq!(decode(b"M\xfcller \x80 Stra\xdfe"), "Müller € Straße");
        assert_eq!(decode(b"nul\0inside utf-8"), "nul\0inside utf-8");
    }

    /// A cut UTF-8 tail, binary bytes and broken UTF-8 after a mark fail; a cut across a chunk
    /// edge does not.
    #[test]
    fn unknown_encodings_fail() {
        let cut = &"Zweite Zeile ü".as_bytes()[..14];
        for bad in [
            cut,
            b"\x89PNG\r\n\x1a\n\xfc",
            b"caf\xe9\0",
            b"\xef\xbb\xbfcaf\xe9",
        ] {
            assert_eq!(
                detect_bytes(bad).unwrap_err().code,
                "unknown_text_encoding",
                "{bad:?}"
            );
        }
        let mut long = vec![b'a'; CHUNK - 1];
        long.extend_from_slice("ü end".as_bytes());
        assert_eq!(detect_bytes(&long), Ok(TextEncoding::Utf8));
        assert!(decode(&long).ends_with("aü end"));
    }

    /// Decoding is the same whatever size the reader asks for.
    #[test]
    fn decoded_reader_any_read_size() {
        let mut text = Vec::new();
        for i in 0..20_000 {
            text.extend_from_slice(format!("line {i} \u{e9}\n").as_bytes());
        }
        let mut utf16 = vec![0xff, 0xfe];
        for u in String::from_utf8(text.clone()).unwrap().encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let mut c = Cursor::new(&utf16[..]);
        let mut d = Decoded::new(&mut c, TextEncoding::Utf16Le).unwrap();
        let mut out = Vec::new();
        let mut small = [0u8; 7];
        loop {
            let n = d.read(&mut small).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&small[..n]);
        }
        assert_eq!(out, text);
    }
}
