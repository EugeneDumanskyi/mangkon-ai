//! Hand-written bindings for the part of Tesseract's C API (`include/tesseract/capi.h`) that
//! Mangkon uses, and the safe [`Tesseract`] handle over them. The only module in the workspace
//! that may use `unsafe` (ADR-0002).
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uchar, c_void};
use std::fmt;
use std::path::Path;
use std::ptr::{self, NonNull};

#[repr(C)]
struct TessBaseAPI {
    _private: [u8; 0],
}

#[repr(C)]
struct TessResultIterator {
    _private: [u8; 0],
}

#[repr(C)]
struct TessPageIterator {
    _private: [u8; 0],
}

/// `TessPageIteratorLevel::RIL_TEXTLINE` and `RIL_WORD`.
const RIL_TEXTLINE: c_int = 2;
const RIL_WORD: c_int = 3;

unsafe extern "C" {
    fn TessVersion() -> *const c_char;
    fn TessDeleteText(text: *const c_char);
    fn TessBaseAPICreate() -> *mut TessBaseAPI;
    fn TessBaseAPIDelete(handle: *mut TessBaseAPI);
    fn TessBaseAPIInit3(
        handle: *mut TessBaseAPI,
        datapath: *const c_char,
        language: *const c_char,
    ) -> c_int;
    fn TessBaseAPISetImage(
        handle: *mut TessBaseAPI,
        imagedata: *const c_uchar,
        width: c_int,
        height: c_int,
        bytes_per_pixel: c_int,
        bytes_per_line: c_int,
    );
    fn TessBaseAPISetSourceResolution(handle: *mut TessBaseAPI, ppi: c_int);
    fn TessBaseAPIRecognize(handle: *mut TessBaseAPI, monitor: *mut c_void) -> c_int;
    fn TessBaseAPIClear(handle: *mut TessBaseAPI);
    fn TessBaseAPIGetUTF8Text(handle: *mut TessBaseAPI) -> *mut c_char;
    fn TessBaseAPIGetIterator(handle: *mut TessBaseAPI) -> *mut TessResultIterator;
    fn TessResultIteratorDelete(handle: *mut TessResultIterator);
    fn TessResultIteratorNext(handle: *mut TessResultIterator, level: c_int) -> c_int;
    fn TessResultIteratorGetUTF8Text(
        handle: *const TessResultIterator,
        level: c_int,
    ) -> *mut c_char;
    fn TessResultIteratorConfidence(handle: *const TessResultIterator, level: c_int) -> c_float;
    fn TessResultIteratorGetPageIteratorConst(
        handle: *const TessResultIterator,
    ) -> *const TessPageIterator;
    fn TessPageIteratorIsAtBeginningOf(handle: *const TessPageIterator, level: c_int) -> c_int;
    fn TessPageIteratorBoundingBox(
        handle: *const TessPageIterator,
        level: c_int,
        left: *mut c_int,
        top: *mut c_int,
        right: *mut c_int,
        bottom: *mut c_int,
    ) -> c_int;
}

/// The linked Tesseract version, for example `5.5.2`.
pub fn version() -> String {
    // SAFETY: TessVersion returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(TessVersion()) }
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The data path is not valid UTF-8 or contains a NUL byte.
    BadPath,
    /// The language code contains a NUL byte.
    BadLanguage,
    /// `Init` failed: the language file is missing or unreadable.
    Init {
        language: String,
    },
    /// The pixel buffer does not match the given size, or the size does not fit a C `int`.
    BadImage,
    Recognize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadPath => f.write_str("OCR data folder path cannot be used"),
            Error::BadLanguage => f.write_str("OCR language code cannot be used"),
            Error::Init { language } => write!(f, "OCR language data for {language:?} not found"),
            Error::BadImage => f.write_str("image size does not match its pixels"),
            Error::Recognize => f.write_str("OCR failed on this image"),
        }
    }
}

impl std::error::Error for Error {}

/// One recognised word, as Tesseract reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    /// Box in pixels, top-left origin, y down; `right` and `bottom` are exclusive.
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// Tesseract's word confidence, 0 to 100.
    pub confidence: f32,
    /// 0-based index of the text line the word is in, in Tesseract's order. Words of one line
    /// are consecutive.
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Tesseract's own text for the page, lines separated by `\n`.
    pub text: String,
    /// Words in Tesseract's reading order.
    pub words: Vec<Word>,
}

/// One `TessBaseAPI`, initialised for one language. Not thread-safe; one per worker (ADR-0014).
pub struct Tesseract {
    handle: NonNull<TessBaseAPI>,
}

impl Tesseract {
    /// `datapath` is the folder that holds `<language>.traineddata`. `language` is one code or
    /// several joined by `+` (`eng+deu`).
    pub fn new(datapath: &Path, language: &str) -> Result<Self, Error> {
        let path = datapath.to_str().ok_or(Error::BadPath)?;
        let path = CString::new(path).map_err(|_| Error::BadPath)?;
        let lang = CString::new(language).map_err(|_| Error::BadLanguage)?;
        // SAFETY: TessBaseAPICreate has no preconditions; a null result is handled.
        let handle = NonNull::new(unsafe { TessBaseAPICreate() }).ok_or(Error::Recognize)?;
        let api = Self { handle };
        // SAFETY: the handle is live, and both strings outlive the call (Tesseract copies them).
        let rc = unsafe { TessBaseAPIInit3(api.handle.as_ptr(), path.as_ptr(), lang.as_ptr()) };
        if rc != 0 {
            return Err(Error::Init {
                language: language.to_string(),
            });
        }
        Ok(api)
    }

    /// OCRs an 8-bit grey image, `width × height` bytes, row by row. `ppi` is the image's
    /// resolution.
    pub fn recognize_grey(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        ppi: u32,
    ) -> Result<Page, Error> {
        let w = c_int::try_from(width).map_err(|_| Error::BadImage)?;
        let h = c_int::try_from(height).map_err(|_| Error::BadImage)?;
        let ppi = c_int::try_from(ppi).map_err(|_| Error::BadImage)?;
        if w == 0 || h == 0 || pixels.len() as u64 != u64::from(width) * u64::from(height) {
            return Err(Error::BadImage);
        }
        let api = self.handle.as_ptr();
        // SAFETY: the handle is live; `pixels` holds exactly `h` rows of `w` bytes and stays
        // borrowed until `Recognize` returns, after which Tesseract no longer reads it (it keeps
        // its own copy as a Leptonica Pix).
        unsafe {
            TessBaseAPISetImage(api, pixels.as_ptr(), w, h, 1, w);
            TessBaseAPISetSourceResolution(api, ppi);
            if TessBaseAPIRecognize(api, ptr::null_mut()) != 0 {
                return Err(Error::Recognize);
            }
        }
        // SAFETY: the handle is live and recognition has run.
        let text = unsafe { take_text(TessBaseAPIGetUTF8Text(api)) };
        let words = self.words();
        // SAFETY: the handle is live; no iterator or result of this page is used after this.
        // Frees the page image and results, which Tesseract otherwise keeps until the next
        // page, so an idle worker holds only the loaded language data (NFR-RES-02).
        unsafe { TessBaseAPIClear(api) };
        Ok(Page { text, words })
    }

    fn words(&mut self) -> Vec<Word> {
        let mut words = Vec::new();
        let mut line = None::<u32>;
        // SAFETY: the handle is live and recognition has run. The iterator is deleted below and
        // is not used after that.
        unsafe {
            let it = TessBaseAPIGetIterator(self.handle.as_ptr());
            if it.is_null() {
                return words;
            }
            loop {
                let text = take_text(TessResultIteratorGetUTF8Text(it, RIL_WORD));
                let confidence = TessResultIteratorConfidence(it, RIL_WORD);
                let (mut l, mut t, mut r, mut b) = (0, 0, 0, 0);
                let page = TessResultIteratorGetPageIteratorConst(it);
                if line.is_none() || TessPageIteratorIsAtBeginningOf(page, RIL_TEXTLINE) != 0 {
                    line = Some(line.map_or(0, |n| n + 1));
                }
                let has_box =
                    TessPageIteratorBoundingBox(page, RIL_WORD, &mut l, &mut t, &mut r, &mut b);
                if has_box != 0 && !text.is_empty() {
                    words.push(Word {
                        text,
                        left: l,
                        top: t,
                        right: r,
                        bottom: b,
                        confidence,
                        line: line.unwrap_or(0),
                    });
                }
                if TessResultIteratorNext(it, RIL_WORD) == 0 {
                    break;
                }
            }
            TessResultIteratorDelete(it);
        }
        words
    }
}

impl Drop for Tesseract {
    fn drop(&mut self) {
        // SAFETY: the handle came from TessBaseAPICreate and is deleted exactly once.
        unsafe { TessBaseAPIDelete(self.handle.as_ptr()) }
    }
}

/// Copies a Tesseract-owned string and frees it. Null gives an empty string.
///
/// # Safety
/// `p` is null or a string Tesseract allocated for the caller, not used after this call.
unsafe fn take_text(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: per the contract, `p` is a live NUL-terminated string that we own.
    unsafe {
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        TessDeleteText(p);
        s
    }
}
