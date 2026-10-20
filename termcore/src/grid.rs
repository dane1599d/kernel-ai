//! The console's text as a grid of cells: log lines written into it, and below the
//! last one the input region (prompt, typed text, cursor), redrawn under each new
//! log line so a line logged while someone types never splits what they typed.
//!
//! A grid allocates nothing and holds its cells inline: `Grid<32768>` is 128 KiB,
//! twice a kernel stack, so the kernel keeps one in a static and never builds or
//! moves one at run time. `new` is const and leaves every cell `'\0'`, which counts
//! as blank, so that static needs no image space.
//!
//! Each change records damage: how far the picture scrolled and which rows to
//! repaint, so a painter shifts the old picture and repaints only those rows.

use core::fmt;

/// Rows a grid may have: damage keeps one bit per row.
pub const MAX_ROWS: usize = 128;
/// Characters of prompt and typed text the input region keeps.
pub const INPUT_MAX: usize = 320;

/// What changed since the last `take_damage`: shift the picture up `scrolled` rows,
/// then repaint `rows` (bit n is row n).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    pub scrolled: usize,
    pub rows: u128,
}

impl Damage {
    /// Whether `row` must be repainted.
    pub fn has(&self, row: usize) -> bool {
        row < MAX_ROWS && self.rows & (1 << row) != 0
    }
}

/// The prompt and the typed text after it, as one run of characters.
struct Input {
    chars: [char; INPUT_MAX],
    prompt: usize,
    len: usize,
    /// The cursor's place in the typed text, in characters.
    cursor: usize,
    shown: bool,
}

pub struct Grid<const CELLS: usize> {
    cells: [char; CELLS],
    cols: usize,
    rows: usize,
    /// Where the next log character goes, as (row, column). Column `cols` means the
    /// row is full and the next character starts a new one.
    log: (usize, usize),
    input: Input,
    input_top: usize,
    damage: Damage,
    /// The cursor at the last `take_damage`, moved along with scrolls since.
    shown_cursor: Option<(usize, usize)>,
}

fn is_blank(ch: char) -> bool {
    ch == ' ' || ch == '\0'
}

/// `bits >> n`, with every bit gone once `n` reaches 128.
fn shift_down(bits: u128, n: usize) -> u128 {
    u32::try_from(n)
        .ok()
        .and_then(|n| bits.checked_shr(n))
        .unwrap_or(0)
}

impl<const CELLS: usize> Default for Grid<CELLS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CELLS: usize> Grid<CELLS> {
    /// A 0 by 0 grid, which ignores every write until `resize`.
    pub const fn new() -> Self {
        Self {
            cells: ['\0'; CELLS],
            cols: 0,
            rows: 0,
            log: (0, 0),
            input: Input {
                chars: ['\0'; INPUT_MAX],
                prompt: 0,
                len: 0,
                cursor: 0,
                shown: false,
            },
            input_top: 0,
            damage: Damage {
                scrolled: 0,
                rows: 0,
            },
            shown_cursor: None,
        }
    }

    /// Blanks the grid at a new size, clamped to `MAX_ROWS` rows and `CELLS` cells,
    /// with the input hidden and every row damaged. Returns the size it took.
    pub fn resize(&mut self, cols: usize, rows: usize) -> (usize, usize) {
        let cols = cols.min(CELLS);
        let rows = match cols {
            0 => 0,
            _ => rows.min(MAX_ROWS).min(CELLS / cols),
        };
        self.cols = if rows == 0 { 0 } else { cols };
        self.rows = rows;
        self.cells[..self.cols * rows].fill(' ');
        self.log = (0, 0);
        self.input.shown = false;
        self.input_top = 0;
        self.shown_cursor = None;
        self.damage = Damage {
            scrolled: 0,
            rows: self.all_rows(),
        };
        (self.cols, rows)
    }

    /// Columns and rows.
    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// The cells of `row`, empty past the last row. `'\0'` and `' '` are both blank.
    pub fn row(&self, row: usize) -> &[char] {
        if row >= self.rows {
            return &[];
        }
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// How many cells of `row` there are up to its last non-blank one.
    pub fn row_width(&self, row: usize) -> usize {
        let cells = self.row(row);
        cells
            .iter()
            .rposition(|&ch| !is_blank(ch))
            .map_or(0, |at| at + 1)
    }

    /// Whether `row` shows exactly `text`, trailing blanks aside.
    pub fn row_reads(&self, row: usize, text: &str) -> bool {
        let mut cells = self.row(row).iter();
        row < self.rows
            && text.chars().all(|ch| cells.next() == Some(&ch))
            && cells.all(|&ch| is_blank(ch))
    }

    /// Writes log text. `\n` starts a line, `\r` goes back to its start, `\x08`
    /// moves back a cell (to the end of the row above from the first column) and
    /// `\x0c` clears the screen. Other control characters are dropped, so no cell
    /// holds one. The input region, if it shows, is redrawn below the text.
    pub fn log(&mut self, text: &str) {
        if self.cols == 0 {
            return;
        }
        let shown = self.input.shown;
        if shown {
            self.erase_input();
        }
        for ch in text.chars() {
            match ch {
                '\n' => self.newline(),
                '\r' => self.log.1 = 0,
                '\x08' => self.back_one(),
                '\x0c' => self.clear(),
                ch if ch.is_control() => {}
                ch => self.put(ch),
            }
        }
        if shown {
            self.draw_input();
        }
    }

    /// Shows `prompt` and `text` below the log, the cursor `cursor` characters into
    /// `text` (at most at its end). Prompt and text together keep `INPUT_MAX`
    /// characters; a wider input wraps onto more rows.
    pub fn set_input(&mut self, prompt: &str, text: &str, cursor: usize) {
        if self.cols == 0 {
            return;
        }
        if self.input.shown {
            self.erase_input();
        }
        let printable = |ch: char| if ch.is_control() { '?' } else { ch };
        let input = &mut self.input;
        input.len = 0;
        for ch in prompt.chars().take(INPUT_MAX) {
            input.chars[input.len] = printable(ch);
            input.len += 1;
        }
        input.prompt = input.len;
        for ch in text.chars().take(INPUT_MAX - input.len) {
            input.chars[input.len] = printable(ch);
            input.len += 1;
        }
        input.cursor = cursor.min(input.len - input.prompt);
        input.shown = true;
        self.draw_input();
    }

    /// Takes the input region off the screen.
    pub fn hide_input(&mut self) {
        if self.cols == 0 || !self.input.shown {
            return;
        }
        self.erase_input();
        self.input.shown = false;
    }

    /// The cursor as (row, column), while the input shows and the cursor is on screen.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        if !self.input.shown || self.cols == 0 {
            return None;
        }
        let at = self.input.prompt + self.input.cursor;
        let row = self.input_top + at / self.cols;
        (row < self.rows).then_some((row, at % self.cols))
    }

    /// The damage since the last call. A cursor that moved, by an edit or a scroll,
    /// damages the row it left and the row it is on.
    pub fn take_damage(&mut self) -> Damage {
        let cursor = self.cursor();
        if cursor != self.shown_cursor {
            for (row, _) in [self.shown_cursor, cursor].into_iter().flatten() {
                self.mark(row);
            }
            self.shown_cursor = cursor;
        }
        core::mem::take(&mut self.damage)
    }

    fn all_rows(&self) -> u128 {
        shift_down(u128::MAX, MAX_ROWS - self.rows)
    }

    fn mark(&mut self, row: usize) {
        if row < self.rows {
            self.damage.rows |= 1 << row;
        }
    }

    fn blank_row(&mut self, row: usize) {
        let cols = self.cols;
        self.cells[row * cols..(row + 1) * cols].fill(' ');
        self.mark(row);
    }

    fn put(&mut self, ch: char) {
        if self.log.1 >= self.cols {
            self.newline();
        }
        let (row, col) = self.log;
        self.cells[row * self.cols + col] = ch;
        self.mark(row);
        self.log.1 += 1;
    }

    fn newline(&mut self) {
        self.log.1 = 0;
        if self.log.0 + 1 < self.rows {
            self.log.0 += 1;
            return;
        }
        self.scroll(1);
        self.log.0 = self.rows - 1;
    }

    fn back_one(&mut self) {
        if self.log.1 > 0 {
            self.log.1 -= 1;
        } else if self.log.0 > 0 {
            // Wrapped text continues on the next row, so it ends in the last column.
            self.log.0 -= 1;
            self.log.1 = self.cols - 1;
        }
    }

    fn clear(&mut self) {
        self.cells[..self.cols * self.rows].fill(' ');
        self.log = (0, 0);
        self.damage = Damage {
            scrolled: 0,
            rows: self.all_rows(),
        };
    }

    /// Moves every row up `n`, blanking the rows that come in at the bottom.
    fn scroll(&mut self, n: usize) {
        let (cols, rows) = (self.cols, self.rows);
        let n = n.min(rows);
        if n == 0 {
            return;
        }
        self.cells.copy_within(n * cols..rows * cols, 0);
        self.cells[(rows - n) * cols..rows * cols].fill(' ');
        // A log row scrolled away leaves the log at the top of a blank screen.
        self.log = match self.log.0.checked_sub(n) {
            Some(row) => (row, self.log.1),
            None => (0, 0),
        };
        self.input_top = self.input_top.saturating_sub(n);
        self.shown_cursor = self
            .shown_cursor
            .and_then(|(row, col)| Some((row.checked_sub(n)?, col)));
        let all = self.all_rows();
        let fresh = all & !shift_down(all, n);
        self.damage.rows = shift_down(self.damage.rows, n) | fresh;
        self.damage.scrolled += n;
        // Repainting every row needs no shift first.
        if self.damage.scrolled >= rows || self.damage.rows == all {
            self.damage = Damage {
                scrolled: 0,
                rows: all,
            };
        }
    }

    /// Rows the input takes: its text, and the cell after it when the cursor is there.
    fn input_rows(&self) -> usize {
        let span = self
            .input
            .len
            .max(self.input.prompt + self.input.cursor + 1);
        span.div_ceil(self.cols)
    }

    fn erase_input(&mut self) {
        let end = (self.input_top + self.input_rows()).min(self.rows);
        for row in self.input_top..end {
            self.blank_row(row);
        }
    }

    /// Draws the input on the log row, or below it if log text is on that row (or on
    /// the rows after it, which `\x08` can move back over), scrolling the log up when
    /// the input would pass the last row.
    fn draw_input(&mut self) {
        let (row, col) = self.log;
        let mut top = if col == 0 { row } else { row + 1 };
        while top < self.rows && self.row_width(top) > 0 {
            top += 1;
        }
        let count = self.input_rows();
        let lift = (top + count).saturating_sub(self.rows).min(top);
        self.scroll(lift);
        self.input_top = top - lift;
        let end = (self.input_top + count).min(self.rows);
        for row in self.input_top..end {
            self.blank_row(row);
        }
        let start = self.input_top * self.cols;
        let cells = &mut self.cells[start..end * self.cols];
        for (cell, &ch) in cells.iter_mut().zip(&self.input.chars[..self.input.len]) {
            *cell = ch;
        }
    }
}

/// Log text written with `write!`.
impl<const CELLS: usize> fmt::Write for Grid<CELLS> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.log(text);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
