//! Page layout in points: word wrap, columns, tables, with every mark's position kept.
//! PDFs and scans are drawn from the same layout, so their reference text and boxes agree.

use crate::fonts::{Font, Metrics};
use crate::rich::{MarkSpec, Rich};

pub const A4: (f64, f64) = (595.0, 842.0);

#[derive(Clone, Debug)]
pub struct LineMark {
    /// Index into `Laid::marks`.
    pub mark: usize,
    /// Char range in `Line::text`.
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct Line {
    /// Pen start and baseline, top-left origin, y down, display space.
    pub x: f64,
    pub y: f64,
    pub font: Font,
    pub size: f64,
    pub text: String,
    pub marks: Vec<LineMark>,
}

/// A raster image placed on a page (scanned page inside a PDF).
#[derive(Clone, Debug)]
pub struct PageImage {
    pub width_px: u32,
    pub height_px: u32,
    pub data: ImageData,
    /// Placement in points, display space.
    pub rect: [f64; 4],
}

#[derive(Clone, Debug)]
pub enum ImageData {
    Gray(Vec<u8>),
    Jpeg(Vec<u8>),
}

/// An invisible text word (OCR text layer), drawn in render mode 3 and scaled to `rect`.
#[derive(Clone, Debug)]
pub struct HiddenWord {
    pub text: String,
    pub rect: [f64; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    /// Display size. For `rotate == 90` the unrotated MediaBox is `height × width`.
    pub width: f64,
    pub height: f64,
    pub rotate: u32,
    /// Reading order.
    pub lines: Vec<Line>,
    pub rules: Vec<[f64; 4]>,
    pub images: Vec<PageImage>,
    pub hidden: Vec<HiddenWord>,
    /// Write the content stream in a different order than reading order.
    pub shuffle: bool,
}

#[derive(Default)]
pub struct Laid {
    pub pages: Vec<Page>,
    pub marks: Vec<MarkSpec>,
}

/// One unbreakable piece of a line, with its marks.
struct Atom {
    text: String,
    marks: Vec<LineMark>,
    break_after: bool,
}

pub struct Flow<'m> {
    m: &'m dyn Metrics,
    pub laid: Laid,
    pub width: f64,
    pub height: f64,
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    /// Top of the next line.
    pub y: f64,
    pub leading: f64,
}

impl<'m> Flow<'m> {
    pub fn new(m: &'m dyn Metrics, size: (f64, f64), margin: f64) -> Self {
        let mut f = Flow {
            m,
            laid: Laid::default(),
            width: size.0,
            height: size.1,
            left: margin,
            right: margin,
            top: margin,
            bottom: margin,
            y: margin,
            leading: 1.3,
        };
        f.new_page();
        f
    }

    pub fn new_page(&mut self) {
        self.laid.pages.push(Page {
            width: self.width,
            height: self.height,
            ..Page::default()
        });
        self.y = self.top;
    }

    pub fn page(&mut self) -> &mut Page {
        self.laid.pages.last_mut().unwrap()
    }

    pub fn gap(&mut self, h: f64) {
        self.y += h;
    }

    /// Starts a new page unless `h` points fit.
    pub fn ensure(&mut self, h: f64) {
        if self.y + h > self.height - self.bottom {
            self.new_page();
        }
    }

    pub fn text_width(&self, font: Font, size: f64, s: &str) -> f64 {
        self.m.width(font, s) * size
    }

    /// A wrapped paragraph across the full text width.
    pub fn para(&mut self, font: Font, size: f64, rich: &Rich) {
        let (x, w) = (self.left, self.width - self.left - self.right);
        self.para_in(x, w, font, size, rich);
    }

    /// A wrapped paragraph in the column `x .. x + width`.
    pub fn para_in(&mut self, x: f64, width: f64, font: Font, size: f64, rich: &Rich) {
        let atoms = self.atoms(rich);
        let space = self.m.advance(font, ' ') * size;
        let mut line: Vec<Atom> = Vec::new();
        let mut line_w = 0.0;
        let flush = |this: &mut Self, line: &mut Vec<Atom>| {
            this.ensure(size * this.leading);
            let base = this.y + size;
            let l = join(x, base, font, size, std::mem::take(line));
            this.page().lines.push(l);
            this.y += size * this.leading;
        };
        for a in atoms {
            let w = self.m.width(font, &a.text) * size;
            if !line.is_empty() && line_w + space + w > width {
                flush(self, &mut line);
                line_w = 0.0;
            }
            line_w += if line.is_empty() { w } else { space + w };
            let brk = a.break_after;
            line.push(a);
            if brk {
                flush(self, &mut line);
                line_w = 0.0;
            }
        }
        if !line.is_empty() {
            flush(self, &mut line);
        }
    }

    /// One line at a fixed position, no wrap. Does not move `y`.
    pub fn line_at(&mut self, x: f64, baseline: f64, font: Font, size: f64, rich: &Rich) {
        let atoms = self.atoms(rich);
        assert!(
            atoms.iter().all(|a| !a.break_after),
            "split marks need a paragraph"
        );
        let l = join(x, baseline, font, size, atoms);
        self.page().lines.push(l);
    }

    /// One line on an earlier page (headers and footers added after the body is laid out).
    pub fn line_on(
        &mut self,
        page: usize,
        x: f64,
        baseline: f64,
        font: Font,
        size: f64,
        rich: &Rich,
    ) {
        let atoms = self.atoms(rich);
        let l = join(x, baseline, font, size, atoms);
        self.laid.pages[page].lines.push(l);
    }

    /// One line ending at `x_right`.
    pub fn line_right(&mut self, x_right: f64, baseline: f64, font: Font, size: f64, rich: &Rich) {
        let w = self.m.width(font, &plain_line(rich)) * size;
        self.line_at(x_right - w, baseline, font, size, rich);
    }

    /// A table row: one cell per column start; cells do not wrap. Moves `y` down one row.
    pub fn row(&mut self, cols: &[f64], font: Font, size: f64, cells: &[Rich]) {
        self.ensure(size * self.leading);
        let base = self.y + size;
        for (x, c) in cols.iter().zip(cells) {
            if !c.is_empty() {
                self.line_at(*x, base, font, size, c);
            }
        }
        self.y += size * self.leading;
    }

    pub fn rule(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.page().rules.push([x0, y0, x1, y1]);
    }

    pub fn hrule(&mut self) {
        let (l, r, y) = (self.left, self.width - self.right, self.y);
        self.rule(l, y, r, y);
        self.y += 4.0;
    }

    /// Splits rich text into atoms (words, with marks kept whole unless they carry `split_at`).
    fn atoms(&mut self, rich: &Rich) -> Vec<Atom> {
        let mut out = Vec::new();
        let mut cur = Atom {
            text: String::new(),
            marks: Vec::new(),
            break_after: false,
        };
        let push_cur = |out: &mut Vec<Atom>, cur: &mut Atom, brk: bool| {
            if !cur.text.is_empty() {
                cur.break_after = brk;
                out.push(std::mem::replace(
                    cur,
                    Atom {
                        text: String::new(),
                        marks: Vec::new(),
                        break_after: false,
                    },
                ));
            } else if brk && let Some(last) = out.last_mut() {
                last.break_after = true;
            }
        };
        for run in &rich.runs {
            match &run.mark {
                None => {
                    for c in run.text.chars() {
                        match c {
                            '\n' => push_cur(&mut out, &mut cur, true),
                            c if c.is_whitespace() => push_cur(&mut out, &mut cur, false),
                            c => cur.text.push(c),
                        }
                    }
                }
                Some(spec) => {
                    let idx = self.laid.marks.len();
                    self.laid.marks.push(spec.clone());
                    let chars: Vec<char> = run.text.chars().collect();
                    let parts: Vec<&[char]> = match spec.split_at {
                        Some(n) => vec![&chars[..n], &chars[n..]],
                        None => vec![&chars[..]],
                    };
                    for (pi, part) in parts.iter().enumerate() {
                        // A split part may start with the space it was split at.
                        let trimmed: Vec<char> = if pi > 0 {
                            part.iter().copied().skip_while(|c| *c == ' ').collect()
                        } else {
                            part.to_vec()
                        };
                        let trimmed: Vec<char> = if pi + 1 < parts.len() {
                            let mut t = trimmed;
                            while t.last() == Some(&' ') {
                                t.pop();
                            }
                            t
                        } else {
                            trimmed
                        };
                        let start = cur.text.chars().count();
                        cur.text.extend(trimmed.iter());
                        let end = cur.text.chars().count();
                        cur.marks.push(LineMark {
                            mark: idx,
                            start,
                            end,
                        });
                        if pi + 1 < parts.len() {
                            push_cur(&mut out, &mut cur, true);
                        }
                    }
                }
            }
        }
        push_cur(&mut out, &mut cur, false);
        out
    }
}

fn plain_line(rich: &Rich) -> String {
    rich.plain()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn join(x: f64, y: f64, font: Font, size: f64, atoms: Vec<Atom>) -> Line {
    let mut text = String::new();
    let mut marks = Vec::new();
    for (i, a) in atoms.into_iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        let off = text.chars().count();
        text.push_str(&a.text);
        marks.extend(a.marks.into_iter().map(|m| LineMark {
            mark: m.mark,
            start: m.start + off,
            end: m.end + off,
        }));
    }
    Line {
        x,
        y,
        font,
        size,
        text,
        marks,
    }
}

/// Char x positions of a line: `xs[i]` is where char `i` starts; `xs[len]` is the line end.
pub fn char_xs(m: &dyn Metrics, line: &Line) -> Vec<f64> {
    let mut xs = Vec::with_capacity(line.text.len() + 1);
    let mut x = line.x;
    for c in line.text.chars() {
        xs.push(x);
        x += m.advance(line.font, c) * line.size;
    }
    xs.push(x);
    xs
}

/// Word char ranges of a line (split at single spaces).
pub fn word_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut n = 0;
    for (i, c) in text.chars().enumerate() {
        if c == ' ' {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
        n = i + 1;
    }
    if let Some(s) = start {
        out.push((s, n));
    }
    out
}

/// Page reference text (lines joined with `\n`) and each line's char offset in it.
pub fn page_text(page: &Page) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut offs = Vec::new();
    for (i, l) in page.lines.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        offs.push(text.chars().count());
        text.push_str(&l.text);
    }
    (text, offs)
}
