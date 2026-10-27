//! The terminal as the desktop's first client (P3 task 8): termcore's cell grid
//! painted into a window's canvas. Rows take their look and cells their ink from
//! `termcore::style`, the same as the kernel console, so the window and the
//! console read alike; glyphs come from [`Text`]'s cell path and box drawing from
//! [`crate::boxdraw`]. Only damaged rows repaint, and a scroll moves the picture
//! up before them, as the console does.

use crate::boxdraw;
use crate::canvas::{Canvas, Rect};
use crate::damage::Damage;
use crate::text::Text;
use alloc::string::String;
use look::console::{EMBER, INK};
use termcore::grid::{self, Grid};
use termcore::style::{Kind, ink, look};

/// Where the cells are: the text area inside `area`, with a margin for the bar.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub area: Rect,
    pub cols: usize,
    pub rows: usize,
    /// A cell's advance, fractional: the font's own.
    pub cell_w: f32,
    pub cell_h: i32,
    /// The baseline's offset from a cell's top.
    pub baseline: i32,
    /// The left margin, where a row's bar sits.
    pub margin: i32,
    pub scale: f32,
}

impl Layout {
    pub fn new(text: &Text, area: Rect, scale: f32) -> Layout {
        let (cell_w, cell_h, baseline) = text.cell(scale);
        let margin = crate::px(12.0, scale);
        let cols = ((area.w - 2 * margin).max(0) as f32 / cell_w) as usize;
        let rows = (area.h / cell_h.max(1)).max(0) as usize;
        Layout {
            area,
            cols: cols.min(grid::INPUT_MAX),
            rows: rows.min(grid::MAX_ROWS),
            cell_w,
            cell_h,
            baseline,
            margin,
            scale,
        }
    }

    pub fn row_rect(&self, row: usize) -> Rect {
        Rect::new(
            self.area.x,
            self.area.y + row as i32 * self.cell_h,
            self.area.w,
            self.cell_h,
        )
    }

    fn col_x(&self, col: usize) -> f32 {
        (self.area.x + self.margin) as f32 + col as f32 * self.cell_w
    }

    /// Cell (`row`, `col`)'s pixels, on whole px.
    pub fn cell_rect(&self, row: usize, col: usize) -> Rect {
        let (x0, x1) = (
            libm::roundf(self.col_x(col)) as i32,
            libm::roundf(self.col_x(col + 1)) as i32,
        );
        let r = self.row_rect(row);
        Rect::new(x0, r.y, x1 - x0, r.h)
    }
}

/// Repaints row `row` of `cells`, of `kind`, with the cursor at `cursor` if it is
/// on this row; returns the rectangle painted.
pub fn paint_row(
    c: &mut Canvas<'_>,
    text: &mut Text,
    l: &Layout,
    row: usize,
    cells: &[char],
    kind: Kind,
    cursor: Option<usize>,
) -> Rect {
    let rect = l.row_rect(row);
    let row_look = look(kind);
    c.fill(rect, row_look.background | 0xFF00_0000);
    if let Some(bar) = row_look.bar {
        let w = crate::px(3.0, l.scale).max(1);
        c.fill(
            Rect::new(rect.x + l.margin / 2 - w / 2, rect.y, w, rect.h),
            bar | 0xFF00_0000,
        );
    }
    let baseline = rect.y + l.baseline;
    // Runs of printable ASCII in one ink go to the cell path in one call.
    let mut run = String::new();
    let mut run_start = 0;
    let mut run_ink = None;
    let flush = |text: &mut Text,
                 c: &mut Canvas<'_>,
                 run: &mut String,
                 start: usize,
                 ink: Option<(u32, bool)>| {
        if let (false, Some((color, bold))) = (run.is_empty(), ink) {
            text.draw_mono(c, rect, l.col_x(start), baseline, l.scale, bold, color, run);
        }
        run.clear();
    };
    for (col, &ch) in cells.iter().enumerate().take(l.cols) {
        let cell_ink = ink(kind.tone(col));
        let ascii = (' '..='~').contains(&ch);
        if !ascii || cell_ink != run_ink || cursor == Some(col) {
            flush(text, c, &mut run, run_start, run_ink);
            run_start = col;
            run_ink = cell_ink;
        }
        let Some((color, bold)) = cell_ink else {
            continue;
        };
        let (color, bold) = if cursor == Some(col) {
            c.fill(l.cell_rect(row, col), EMBER | 0xFF00_0000);
            (INK, bold)
        } else {
            (color, bold)
        };
        if cursor == Some(col) || !ascii {
            if boxdraw::handles(ch) {
                boxdraw::draw(c, rect, l.cell_rect(row, col), ch, color);
            } else if ch != '\0' && ch != ' ' {
                let mut one = [0u8; 4];
                text.draw_mono(
                    c,
                    rect,
                    l.col_x(col),
                    baseline,
                    l.scale,
                    bold,
                    color,
                    ch.encode_utf8(&mut one),
                );
            }
            run_start = col + 1;
            continue;
        }
        run.push(ch);
    }
    flush(text, c, &mut run, run_start, run_ink);
    rect
}

/// Moves the text area's picture up `rows` rows, for a scroll.
fn scroll(c: &mut Canvas<'_>, l: &Layout, rows: usize) {
    let area = l.area.intersect(&c.bounds());
    let by = rows as i32 * l.cell_h;
    let stride = c.stride_px as usize;
    for y in area.y..area.bottom() - by {
        let (to, from) = (
            y as usize * stride + area.x as usize,
            (y + by) as usize * stride + area.x as usize,
        );
        c.pixels.copy_within(from..from + area.w as usize, to);
    }
}

/// Paints what `damage` says changed in `g`: a scroll, then the damaged rows.
/// Returns what to flush.
pub fn paint<const CELLS: usize>(
    c: &mut Canvas<'_>,
    text: &mut Text,
    l: &Layout,
    g: &Grid<CELLS>,
    damage: grid::Damage,
) -> Damage<32> {
    let mut out = Damage::default();
    let rows = g.size().1.min(l.rows);
    if damage.scrolled > 0 {
        scroll(c, l, damage.scrolled.min(rows));
        out.add(l.area);
    }
    let input = g.input_span();
    let cursor = g.cursor();
    for row in (0..rows).filter(|&r| damage.has(r)) {
        let cells = g.row(row);
        let kind = match input.filter(|&(top, end, _)| (top..end).contains(&row)) {
            Some((top, _, prompt)) if row == top => Kind::Input { prompt },
            Some(_) => Kind::Input { prompt: 0 },
            None => Kind::of(cells),
        };
        let at = cursor.filter(|&(r, _)| r == row).map(|(_, col)| col);
        out.add(paint_row(c, text, l, row, cells, kind, at));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Image;
    use crate::testfonts::{INTER, MONO};
    use crate::text::Fonts;
    use look::console::{CORAL, REPLY_TINT};

    fn text() -> Text {
        Text::new(
            Fonts {
                sans: INTER,
                mono: MONO,
            },
            true,
        )
        .unwrap()
    }

    const CELLS: usize = 80 * 24;

    fn grid(l: &Layout) -> Grid<CELLS> {
        let mut g = Grid::<CELLS>::new();
        g.resize(l.cols.min(80), l.rows.min(24));
        g
    }

    #[test]
    fn the_layout_fits_whole_cells_in_the_area() {
        let t = text();
        let l = Layout::new(&t, Rect::new(10, 40, 800, 480), 1.0);
        assert_eq!((l.cell_h, l.rows), (20, 24));
        assert!(l.cell_w > 8.0 && l.cell_w < 9.0, "{}", l.cell_w);
        assert!(l.col_x(l.cols) <= (l.area.right() - l.margin) as f32);
        assert!(l.baseline > 10 && l.baseline < 20);
        let (a, b) = (l.cell_rect(0, 3), l.cell_rect(0, 4));
        assert_eq!(a.right(), b.x, "cells tile with no gap");
    }

    #[test]
    fn rows_take_the_consoles_looks() {
        let mut t = text();
        let l = Layout::new(&t, Rect::new(0, 0, 800, 480), 1.0);
        let mut g = grid(&l);
        g.log("kernel: started\n= the answer\n! an error\n");
        let mut img = Image::new(800, 480);
        let d = g.take_damage();
        paint(&mut img.canvas(), &mut t, &l, &g, d);
        let at = |x: i32, y: i32| img.px[(y * 800 + x) as usize] & 0x00FF_FFFF;
        // The reply row's tint, past its text; the error row's coral bar.
        assert_eq!(at(790, 20 + 10), REPLY_TINT);
        assert_eq!(at(l.margin / 2, 40 + 10), CORAL);
        assert_eq!(at(790, 10), INK);
    }

    #[test]
    fn the_cursor_is_an_ember_cell() {
        let mut t = text();
        let l = Layout::new(&t, Rect::new(0, 0, 800, 480), 1.0);
        let mut g = grid(&l);
        g.set_input("kai> ", "ls", 2);
        let mut img = Image::new(800, 480);
        let d = g.take_damage();
        paint(&mut img.canvas(), &mut t, &l, &g, d);
        let (row, col) = g.cursor().unwrap();
        let cell = l.cell_rect(row, col);
        assert_eq!(
            img.px[((cell.y + 2) * 800 + cell.x + 1) as usize] & 0x00FF_FFFF,
            EMBER
        );
    }

    #[test]
    fn a_scroll_and_its_rows_look_the_same_as_a_full_repaint() {
        let mut t = text();
        let l = Layout::new(&t, Rect::new(0, 0, 640, 200), 1.0);
        let mut g = grid(&l);
        let mut img = Image::new(640, 200);
        for i in 0..(l.rows + 4) {
            g.log(&alloc::format!("line {i}: kai> ls ─┼─ done\n"));
            let d = g.take_damage();
            paint(&mut img.canvas(), &mut t, &l, &g, d);
        }
        // The same grid, every row painted once on a fresh canvas.
        let mut full = Image::new(640, 200);
        let all = grid::Damage {
            scrolled: 0,
            rows: u128::MAX,
        };
        paint(&mut full.canvas(), &mut t, &l, &g, all);
        let text_area = l.rows as i32 * l.cell_h;
        assert!(
            img.px[..(text_area * 640) as usize] == full.px[..(text_area * 640) as usize],
            "incremental painting differs from a full repaint"
        );
    }
}

#[cfg(test)]
mod picture {
    use super::*;
    use crate::canvas::Image;
    use crate::testfonts::{INTER, MONO};
    use crate::text::Fonts;

    /// `cargo test -p gfx2d term::picture -- --ignored` writes a terminal to
    /// target/gfx2d-term.ppm.
    #[test]
    #[ignore]
    fn write_a_terminal() {
        let mut t = Text::new(
            Fonts {
                sans: INTER,
                mono: MONO,
            },
            true,
        )
        .unwrap();
        let l = Layout::new(&t, Rect::new(0, 0, 720, 260), 1.0);
        let mut g = Grid::<{ 80 * 24 }>::new();
        g.resize(l.cols.min(80), l.rows.min(24));
        g.log("Kernel AI 0.10.0\nboot-ok\nshell: ready\n= Done: created plan.txt (8 bytes). undo\n! no such file: todo.txt\n? Allow fs_move(plan.txt, todo.txt)? y/n\n┌──────┬──────┐\n│ name │ size │\n└──────┴──────┘\n");
        g.set_input("kai> ", "what's in plan.txt", 18);
        let mut img = Image::new(720, 260);
        let d = g.take_damage();
        paint(&mut img.canvas(), &mut t, &l, &g, d);
        let mut out = std::format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
        for p in &img.px {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gfx2d-term.ppm"),
            out,
        )
        .unwrap();
    }
}
