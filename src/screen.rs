//! A small cell buffer with diff output, like curses: draw into the buffer,
//! then `flush` writes only the cells that changed. Colors use the same SGR
//! codes that curses writes from the tmux-256color terminfo entry.

use std::io::Write;

use unicode_width::UnicodeWidthChar;

/// A terminal color: the default color or a palette index (0-255).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Default,
    Idx(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
}

impl Style {
    pub const DEFAULT: Style = Style { fg: Color::Default, bg: Color::Default, bold: false };

    pub fn bold(self) -> Style {
        Style { bold: true, ..self }
    }

    fn sgr(&self) -> String {
        let mut s = String::from("\x1b[0");
        if self.bold {
            s.push_str(";1");
        }
        match self.fg {
            Color::Default => {}
            Color::Idx(n) if n < 8 => s.push_str(&format!(";{}", 30 + n)),
            Color::Idx(n) if n < 16 => s.push_str(&format!(";{}", 90 + n - 8)),
            Color::Idx(n) => s.push_str(&format!(";38;5;{n}")),
        }
        match self.bg {
            Color::Default => {}
            Color::Idx(n) if n < 8 => s.push_str(&format!(";{}", 40 + n)),
            Color::Idx(n) if n < 16 => s.push_str(&format!(";{}", 100 + n - 8)),
            Color::Idx(n) => s.push_str(&format!(";48;5;{n}")),
        }
        s.push('m');
        s
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Cell {
    ch: char,
    /// The right half of a wide character: nothing is printed here.
    cont: bool,
    style: Style,
}

const BLANK: Cell = Cell { ch: ' ', cont: false, style: Style::DEFAULT };

pub struct Screen {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
    shown: Option<Vec<Cell>>,
}

impl Screen {
    pub fn new() -> Self {
        Screen { w: 0, h: 0, cells: Vec::new(), shown: None }
    }

    /// Clear the buffer for a new frame of `w` x `h` cells.
    pub fn erase(&mut self, w: usize, h: usize) {
        if (w, h) != (self.w, self.h) {
            self.w = w;
            self.h = h;
            self.shown = None;
        }
        self.cells = vec![BLANK; w * h];
    }

    pub fn size(&self) -> (i64, i64) {
        (self.h as i64, self.w as i64)
    }

    /// Force a full repaint on the next flush (after a resize).
    pub fn invalidate(&mut self) {
        self.shown = None;
    }

    /// Like curses `addnstr(y, x, text, w - x - 1)`: at most `w - x - 1`
    /// characters, nothing outside the screen.
    pub fn put(&mut self, y: i64, x: i64, text: &str, style: Style) {
        let (h, w) = self.size();
        if !(0 <= y && y < h && 0 <= x && x < w) {
            return;
        }
        let n = (w - x - 1).max(0) as usize;
        let row = y as usize * self.w;
        let mut col = x as usize;
        for ch in text.chars().take(n) {
            let cw = ch.width().unwrap_or(0);
            if cw == 0 {
                continue;
            }
            if col + cw > self.w - 1 {
                break;
            }
            // overwriting half of a wide character clears the other half
            if self.cells[row + col].cont && col > 0 {
                self.cells[row + col - 1] = Cell { ch: ' ', ..self.cells[row + col - 1].clone() };
            }
            let end = col + cw;
            if end < self.w && self.cells[row + end].cont {
                self.cells[row + end] = Cell { ch: ' ', cont: false, ..self.cells[row + end].clone() };
            }
            self.cells[row + col] = Cell { ch, cont: false, style };
            if cw == 2 {
                self.cells[row + col + 1] = Cell { ch: ' ', cont: true, style };
            }
            col = end;
        }
    }

    /// Write the changed cells to the terminal.
    pub fn flush(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        let mut buf = String::new();
        let full = self.shown.is_none();
        if full {
            buf.push_str("\x1b[0m\x1b[2J");
        }
        let mut style: Option<Style> = None;
        let mut cursor: Option<(usize, usize)> = None;
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let cell = &self.cells[i];
                if cell.cont {
                    continue;
                }
                let unchanged = match &self.shown {
                    Some(shown) => shown[i] == *cell,
                    None => *cell == BLANK, // the screen was just cleared
                };
                if unchanged {
                    continue;
                }
                if cursor != Some((y, x)) {
                    buf.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
                }
                if style != Some(cell.style) {
                    buf.push_str(&cell.style.sgr());
                    style = Some(cell.style);
                }
                buf.push(cell.ch);
                let cw = cell.ch.width().unwrap_or(1).max(1);
                cursor = Some((y, x + cw));
            }
        }
        buf.push_str("\x1b[0m");
        out.write_all(buf.as_bytes())?;
        out.flush()?;
        self.shown = Some(self.cells.clone());
        Ok(())
    }
}
