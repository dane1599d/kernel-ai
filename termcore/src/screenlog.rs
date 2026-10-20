//! `screenlog`: a serial copy of what the console draws. After each repaint, one
//! line per repainted row, top to bottom, then the cursor while the input shows:
//!
//! `screen: <row>|<text>`, the row's text without trailing blanks, and
//! `screen: cursor <row> <column>`, both counting from 0. Rows that only moved with
//! a scroll are not printed again, so the lines record what was drawn, not a
//! screen that can be replayed.

use crate::grid::{Damage, Grid};
use core::fmt::{self, Write};

/// Writes the lines for one repaint of `grid` with `damage`.
pub fn write<const CELLS: usize>(
    out: &mut impl Write,
    grid: &Grid<CELLS>,
    damage: &Damage,
) -> fmt::Result {
    let (_, rows) = grid.size();
    let mut any = false;
    for row in (0..rows).filter(|&row| damage.has(row)) {
        write!(out, "screen: {row}|")?;
        for &ch in &grid.row(row)[..grid.row_width(row)] {
            out.write_char(if ch == '\0' { ' ' } else { ch })?;
        }
        out.write_char('\n')?;
        any = true;
    }
    if any && let Some((row, col)) = grid.cursor() {
        writeln!(out, "screen: cursor {row} {col}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write;
    use crate::grid::Grid;
    use std::string::String;

    type Small = Grid<{ 24 * 6 }>;

    fn grid() -> Small {
        let mut grid = Small::new();
        grid.resize(24, 6);
        grid.take_damage();
        grid
    }

    fn lines(grid: &mut Small) -> String {
        let damage = grid.take_damage();
        let mut out = String::new();
        write(&mut out, grid, &damage).unwrap();
        out
    }

    #[test]
    fn a_repainted_row_loses_its_trailing_blanks() {
        let mut grid = grid();
        grid.log("hi  \n");
        assert_eq!(lines(&mut grid), "screen: 0|hi\n");
        assert_eq!(lines(&mut grid), "", "nothing changed since");
    }

    #[test]
    fn a_blank_row_prints_with_nothing_after_the_bar() {
        let mut grid = grid();
        grid.log("a\nb\nc\n");
        grid.set_input("kai> ", "x", 1);
        grid.take_damage();
        grid.hide_input();
        assert_eq!(lines(&mut grid), "screen: 3|\n");
    }

    #[test]
    fn the_cursor_follows_the_rows() {
        let mut grid = grid();
        grid.set_input("kai> ", "pol", 3);
        assert_eq!(lines(&mut grid), "screen: 0|kai> pol\nscreen: cursor 0 8\n");
    }

    #[test]
    fn a_scroll_at_the_bottom_prints_the_new_log_row_and_the_input_row() {
        let mut grid = grid();
        grid.log("l0\nl1\nl2\nl3\nl4\n");
        grid.set_input("kai> ", "pol", 3);
        grid.take_damage();
        grid.log("hello\n");
        assert_eq!(
            lines(&mut grid),
            "screen: 4|hello\nscreen: 5|kai> pol\nscreen: cursor 5 8\n"
        );
    }
}
