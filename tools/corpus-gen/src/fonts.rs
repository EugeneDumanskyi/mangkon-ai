//! Fonts and metrics. PDFs use the standard-14 fonts (exact AFM widths, nothing embedded);
//! scans are drawn with the committed Noto fonts (OFL-1.1, `fonts/`).

use ab_glyph::{Font as _, FontRef};

use crate::afm;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Sans,
    SansBold,
    Mono,
    Serif,
}

impl Font {
    /// PDF resource name and BaseFont.
    pub fn pdf(self) -> (&'static str, &'static str) {
        match self {
            Font::Sans => ("F1", "Helvetica"),
            Font::SansBold => ("F2", "Helvetica-Bold"),
            Font::Mono => ("F3", "Courier"),
            Font::Serif => ("F4", "Times-Roman"),
        }
    }

    pub const ALL: [Font; 4] = [Font::Sans, Font::SansBold, Font::Mono, Font::Serif];
}

/// Widths and vertical extents in em units (1.0 = font size).
pub trait Metrics {
    fn advance(&self, font: Font, c: char) -> f64;
    /// Above the baseline, positive.
    fn ascent(&self, font: Font) -> f64;
    /// Below the baseline, negative.
    fn descent(&self, font: Font) -> f64;

    fn width(&self, font: Font, s: &str) -> f64 {
        s.chars().map(|c| self.advance(font, c)).sum()
    }
}

/// Standard-14 AFM metrics.
pub struct Afm;

impl Metrics for Afm {
    fn advance(&self, font: Font, c: char) -> f64 {
        if font == Font::Mono {
            return 0.6;
        }
        let (ascii, extra): (&[u16; 95], &[(u8, char, u16)]) = match font {
            Font::Sans => (&afm::HELVETICA_ASCII, &afm::HELVETICA_EXTRA),
            Font::SansBold => (&afm::HELVETICA_BOLD_ASCII, &afm::HELVETICA_BOLD_EXTRA),
            Font::Serif => (&afm::TIMES_ASCII, &afm::TIMES_EXTRA),
            Font::Mono => unreachable!(),
        };
        let w = match c {
            ' '..='~' => ascii[c as usize - 32],
            _ => {
                extra
                    .iter()
                    .find(|e| e.1 == c)
                    .unwrap_or_else(|| panic!("no WinAnsi glyph for {c:?}"))
                    .2
            }
        };
        w as f64 / 1000.0
    }

    fn ascent(&self, font: Font) -> f64 {
        match font {
            Font::Sans | Font::SansBold => 0.718,
            Font::Mono => 0.629,
            Font::Serif => 0.683,
        }
    }

    fn descent(&self, font: Font) -> f64 {
        match font {
            Font::Sans | Font::SansBold => -0.207,
            Font::Mono => -0.157,
            Font::Serif => -0.217,
        }
    }
}

/// WinAnsi byte for a char the corpus prints in PDFs.
pub fn winansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        _ => {
            afm::HELVETICA_EXTRA
                .iter()
                .find(|e| e.1 == c)
                .unwrap_or_else(|| panic!("no WinAnsi code for {c:?}"))
                .0
        }
    }
}

static NOTO_SANS: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
static NOTO_SANS_BOLD: &[u8] = include_bytes!("../fonts/NotoSans-Bold.ttf");
static NOTO_MONO: &[u8] = include_bytes!("../fonts/NotoSansMono-Regular.ttf");
static NOTO_SERIF: &[u8] = include_bytes!("../fonts/NotoSerif-Regular.ttf");

/// The Noto fonts for scans.
pub struct Noto {
    fonts: [FontRef<'static>; 4],
}

impl Noto {
    pub fn load() -> Self {
        let f = |b: &'static [u8]| FontRef::try_from_slice(b).expect("valid font");
        Noto {
            fonts: [f(NOTO_SANS), f(NOTO_SANS_BOLD), f(NOTO_MONO), f(NOTO_SERIF)],
        }
    }

    pub fn font(&self, font: Font) -> &FontRef<'static> {
        &self.fonts[match font {
            Font::Sans => 0,
            Font::SansBold => 1,
            Font::Mono => 2,
            Font::Serif => 3,
        }]
    }

    fn upem(&self, font: Font) -> f64 {
        self.font(font).units_per_em().expect("units per em") as f64
    }
}

impl Metrics for Noto {
    fn advance(&self, font: Font, c: char) -> f64 {
        let f = self.font(font);
        f.h_advance_unscaled(f.glyph_id(c)) as f64 / self.upem(font)
    }

    fn ascent(&self, font: Font) -> f64 {
        self.font(font).ascent_unscaled() as f64 / self.upem(font)
    }

    fn descent(&self, font: Font) -> f64 {
        self.font(font).descent_unscaled() as f64 / self.upem(font)
    }
}
