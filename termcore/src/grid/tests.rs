use super::*;
use std::string::String;

/// 24 columns by 6 rows.
type Small = Grid<{ 24 * 6 }>;

fn grid() -> Small {
    let mut grid = Small::new();
    assert_eq!(grid.resize(24, 6), (24, 6));
    grid.take_damage();
    grid
}

fn text(grid: &Small, row: usize) -> String {
    grid.row(row)[..grid.row_width(row)].iter().collect()
}

fn rows(list: &[usize]) -> u128 {
    list.iter().fold(0, |bits, &row| bits | 1 << row)
}

/// A grid whose log has filled every row but the last.
fn full_grid() -> Small {
    let mut grid = grid();
    grid.log("l0\nl1\nl2\nl3\nl4\n");
    grid.take_damage();
    grid
}

#[test]
fn a_log_line_lands_on_the_input_row_and_the_input_moves_down() {
    let mut grid = grid();
    grid.log("one\n");
    grid.set_input("kai> ", "pol", 3);
    assert_eq!(grid.cursor(), Some((1, 8)));
    grid.log("terminal: two\n");
    assert_eq!(text(&grid, 1), "terminal: two");
    assert!(grid.row_reads(2, "kai> pol"));
    assert_eq!(grid.cursor(), Some((2, 8)));
}

#[test]
fn typing_damages_only_the_input_rows() {
    let mut grid = grid();
    grid.log("one\ntwo\n");
    grid.set_input("kai> ", "", 0);
    grid.take_damage();
    grid.set_input("kai> ", "p", 1);
    let damage = grid.take_damage();
    assert_eq!(
        damage,
        Damage {
            scrolled: 0,
            rows: rows(&[2])
        }
    );
    assert_eq!(text(&grid, 2), "kai> p");
}

#[test]
fn a_log_line_at_the_bottom_scrolls_once() {
    let mut grid = full_grid();
    grid.set_input("kai> ", "pol", 3);
    assert_eq!(grid.cursor(), Some((5, 8)));
    grid.take_damage();
    grid.log("hello\n");
    let damage = grid.take_damage();
    assert_eq!(
        damage,
        Damage {
            scrolled: 1,
            rows: rows(&[4, 5])
        }
    );
    assert_eq!(text(&grid, 0), "l1");
    assert_eq!(text(&grid, 4), "hello");
    assert_eq!(text(&grid, 5), "kai> pol");
    assert_eq!(grid.cursor(), Some((5, 8)));
}

#[test]
fn scrolling_a_whole_screen_or_more_repaints_everything() {
    let mut grid = full_grid();
    grid.log("a\nb\nc\nd\ne\nf\n");
    let damage = grid.take_damage();
    assert_eq!(
        damage,
        Damage {
            scrolled: 0,
            rows: rows(&[0, 1, 2, 3, 4, 5])
        }
    );
    assert_eq!(text(&grid, 0), "b");
    assert_eq!(text(&grid, 4), "f");
}

#[test]
fn a_wide_input_wraps_and_scrolls_at_the_bottom() {
    let mut grid = full_grid();
    let typed = "abcdefghijklmnopqrstuvwxy";
    grid.set_input("kai> ", typed, typed.len());
    let damage = grid.take_damage();
    assert_eq!(
        damage,
        Damage {
            scrolled: 1,
            rows: rows(&[4, 5])
        }
    );
    assert_eq!(text(&grid, 3), "l4");
    assert_eq!(text(&grid, 4), "kai> abcdefghijklmnopqrs");
    assert_eq!(text(&grid, 5), "tuvwxy");
    assert_eq!(grid.cursor(), Some((5, 6)));
}

#[test]
fn the_cursor_can_sit_anywhere_in_a_wrapped_input() {
    let mut grid = grid();
    let typed = "abcdefghijklmnopqrstuvwxy";
    grid.set_input("kai> ", typed, 0);
    assert_eq!(grid.cursor(), Some((0, 5)));
    grid.set_input("kai> ", typed, 20);
    assert_eq!(grid.cursor(), Some((1, 1)));
    grid.set_input("kai> ", typed, 18);
    assert_eq!(grid.cursor(), Some((0, 23)));
    grid.take_damage();
    grid.set_input("kai> ", typed, 19);
    assert_eq!(grid.cursor(), Some((1, 0)));
    assert_eq!(grid.take_damage().rows, rows(&[0, 1]));
    grid.set_input("kai> ", "ab", 99);
    assert_eq!(
        grid.cursor(),
        Some((0, 7)),
        "clamped to the end of the text"
    );
}

#[test]
fn an_input_that_fills_its_row_puts_the_cursor_on_the_next() {
    let mut grid = grid();
    grid.set_input("kai> ", "abcdefghijklmnopqrs", 19);
    assert_eq!(grid.cursor(), Some((1, 0)));
    grid.log("x\n");
    assert_eq!(grid.cursor(), Some((2, 0)));
}

#[test]
fn partial_log_text_puts_the_input_on_the_next_row() {
    let mut grid = grid();
    grid.log("abc");
    grid.set_input("kai> ", "", 0);
    assert_eq!(grid.cursor(), Some((1, 5)));
    grid.log("def\n");
    assert_eq!(text(&grid, 0), "abcdef");
    assert!(grid.row_reads(1, "kai> "));
    assert_eq!(grid.cursor(), Some((1, 5)));
}

#[test]
fn backspace_moves_back_like_the_old_console() {
    let mut grid = grid();
    grid.log("ab\x08c\n");
    assert_eq!(text(&grid, 0), "ac");
    // From the first column, back to the last column of the row above.
    grid.log("x\x08\x08y");
    assert_eq!(text(&grid, 0), std::format!("ac{}y", " ".repeat(21)));
    assert_eq!(text(&grid, 1), "x");
}

#[test]
fn form_feed_clears_everything_and_redraws_the_input_at_the_top() {
    let mut grid = full_grid();
    grid.set_input("kai> ", "x", 1);
    grid.take_damage();
    grid.log("\x0c");
    let damage = grid.take_damage();
    assert_eq!(
        damage,
        Damage {
            scrolled: 0,
            rows: rows(&[0, 1, 2, 3, 4, 5])
        }
    );
    assert!(grid.row_reads(0, "kai> x"));
    assert!((1..6).all(|row| grid.row_reads(row, "")));
    assert_eq!(grid.cursor(), Some((0, 6)));
}

#[test]
fn control_characters_never_reach_a_cell() {
    let mut grid = grid();
    grid.log("a\x1bb\tc\n");
    assert_eq!(text(&grid, 0), "abc");
    grid.set_input("q\x07> ", "a\tb", 3);
    assert_eq!(text(&grid, 1), "q?> a?b");
}

#[test]
fn a_log_line_with_the_input_hidden_draws_no_input() {
    let mut grid = grid();
    grid.log("one\n");
    assert_eq!(grid.cursor(), None);
    assert_eq!(text(&grid, 1), "");
    assert_eq!(
        grid.take_damage(),
        Damage {
            scrolled: 0,
            rows: rows(&[0])
        }
    );
}

#[test]
fn hiding_the_input_clears_its_rows_and_the_cursor() {
    let mut grid = grid();
    grid.log("one\n");
    grid.set_input("kai> ", "abcdefghijklmnopqrstuvwxy", 25);
    grid.take_damage();
    grid.hide_input();
    assert_eq!(grid.cursor(), None);
    assert_eq!(grid.take_damage().rows, rows(&[1, 2]));
    assert!((1..6).all(|row| grid.row_reads(row, "")));
    assert_eq!(text(&grid, 0), "one");
}

#[test]
fn resize_clamps_to_the_cells_and_128_rows() {
    let mut small = Small::new();
    assert_eq!(small.resize(100, 100), (100, 1));
    assert_eq!(small.resize(24, 200), (24, 6));
    assert_eq!(small.resize(500, 3), (144, 1));
    assert_eq!(small.resize(0, 5), (0, 0));
    let mut tall = Grid::<1000>::new();
    assert_eq!(tall.resize(5, 500), (5, 128));
    assert_eq!(tall.take_damage().rows, u128::MAX);
}

#[test]
fn nul_cells_count_as_blank() {
    let mut grid = grid();
    grid.log("ab\n");
    grid.cells[1] = '\0';
    grid.cells[5] = '\0';
    assert_eq!(grid.row_width(0), 1);
    assert!(grid.row_reads(0, "a"));
    assert!(grid.row_reads(1, ""));
}

#[test]
fn a_grid_before_resize_ignores_writes() {
    let mut grid = Small::new();
    grid.log("x\n\x0c\x08");
    grid.set_input("kai> ", "pol", 3);
    assert_eq!(grid.cursor(), None);
    grid.hide_input();
    assert_eq!(grid.size(), (0, 0));
    assert_eq!(grid.row(0), &[] as &[char]);
    assert_eq!(grid.take_damage(), Damage::default());
}

#[test]
fn a_long_input_on_a_short_grid_is_cut_off() {
    let mut grid = grid();
    let typed: String = core::iter::repeat_n('x', 300).collect();
    grid.set_input("kai> ", &typed, typed.len());
    assert_eq!(grid.cursor(), None, "the cursor is past the last row");
    assert_eq!(grid.row_width(5), 24);
    grid.set_input("kai> ", &typed, 0);
    assert_eq!(grid.cursor(), Some((0, 5)));
}

#[test]
fn backspacing_up_a_row_never_puts_the_input_over_log_text() {
    let mut grid = grid();
    grid.set_input("kai> ", "", 0);
    grid.log("abc\nxy");
    grid.log("\x08\x08\x08");
    assert_eq!(text(&grid, 1), "xy");
    assert!(grid.row_reads(2, "kai> "));
    assert_eq!(grid.cursor(), Some((2, 5)));
}

#[test]
fn a_log_row_scrolled_away_by_a_tall_input_starts_again_at_the_top() {
    let mut grid = Grid::<12>::new();
    assert_eq!(grid.resize(4, 3), (4, 3));
    grid.log("ab");
    grid.set_input("> ", "0123456789", 10);
    grid.hide_input();
    grid.log("cd\n");
    assert!(grid.row_reads(0, "cd"));
}
