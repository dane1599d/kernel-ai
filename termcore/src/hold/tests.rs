use super::*;
use crate::grid::Grid;
use core::fmt::Write;
use std::format;
use std::string::String;
use std::vec::Vec;

const TERMINAL: usize = 7;
const PRINTER: usize = 9;
const ROWS: usize = 10;

/// The console as the kernel's is: a grid and the hold in front of it.
struct Console {
    grid: Grid<{ 40 * ROWS }>,
    hold: Hold<256>,
}

impl Console {
    fn new() -> Self {
        let mut grid = Grid::new();
        grid.resize(40, ROWS);
        Console {
            grid,
            hold: Hold::new(),
        }
    }

    /// What `console::print` does with a line logged by `writer`.
    fn print(&mut self, writer: usize, text: &str) {
        if self.hold.holds(writer) {
            let _ = self.hold.write_str(text);
        } else {
            self.grid.log(text);
        }
    }

    fn release(&mut self) -> usize {
        let Released { text, lost } = self.hold.close();
        let text = String::from(text);
        self.grid.log(&text);
        lost
    }

    fn rows(&self) -> Vec<String> {
        (0..ROWS)
            .map(|r| {
                self.grid
                    .row(r)
                    .iter()
                    .collect::<String>()
                    .trim_end_matches([' ', '\0'])
                    .into()
            })
            .collect()
    }
}

/// A question as tall as `:apply`'s can be: the rows above the prompt.
fn question() -> Vec<String> {
    let mut rows = Vec::from([String::from("? apply notes v7 (from b) over v6")]);
    rows.extend((1..ROWS - 1).map(|n| format!("? +line {n}")));
    rows
}

/// Prints the question the way the terminal does, with the printer logging a
/// line between every two of its rows; then the prompt.
fn ask(console: &mut Console, question: &[String]) {
    for row in question {
        console.print(TERMINAL, &format!("{row}\n"));
        console.print(PRINTER, "[printer] tick\n");
    }
    console.grid.set_input("apply v7 to notes? yes/no> ", "", 0);
}

#[test]
fn a_hold_opened_first_keeps_the_whole_question_on_screen() {
    let mut console = Console::new();
    let question = question();
    console.hold.open(TERMINAL);
    ask(&mut console, &question);
    let rows = console.rows();
    assert_eq!(
        rows[..ROWS - 1],
        question[..],
        "every row, top first, nothing between"
    );
    assert!(rows[ROWS - 1].starts_with("apply v7 to notes?"));
    // The answer is in: what the printer logged comes after, in order.
    assert_eq!(console.release(), 0);
    let rows = console.rows();
    assert_eq!(rows[ROWS - 2], "[printer] tick");
    assert!(
        rows.iter().all(|row| !row.starts_with("? apply")),
        "the question scrolled on"
    );
}

/// The order before this fix: the question printed, and only then the guard.
/// The printer's lines land between its rows and push its top off the screen.
#[test]
fn a_question_printed_before_the_hold_loses_its_top() {
    let mut console = Console::new();
    let question = question();
    ask(&mut console, &question);
    console.hold.open(TERMINAL);
    let rows = console.rows();
    assert!(
        !rows.contains(&question[0]),
        "the header scrolled away: {rows:?}"
    );
    assert!(rows.iter().any(|row| row == "[printer] tick"));
}

#[test]
fn the_askers_own_lines_pass_and_others_wait_until_it_closes() {
    let mut hold = Hold::<64>::new();
    assert!(!hold.is_open() && !hold.holds(PRINTER));
    hold.open(TERMINAL);
    assert!(hold.is_open());
    assert!(!hold.holds(TERMINAL));
    assert!(hold.holds(PRINTER));
    hold.keep("one\n");
    hold.keep("two");
    hold.keep(" halves\n");
    hold.keep("open");
    assert_eq!(
        hold.close(),
        Released {
            text: "one\ntwo halves\nopen\n",
            lost: 0
        },
        "a line left open is ended"
    );
    assert!(!hold.is_open() && !hold.holds(PRINTER));
    hold.open(TERMINAL);
    assert_eq!(
        hold.close(),
        Released { text: "", lost: 0 },
        "a new hold starts empty"
    );
}

#[test]
fn a_full_hold_keeps_whole_lines_in_order_and_counts_the_rest() {
    let mut hold = Hold::<16>::new();
    hold.open(TERMINAL);
    hold.keep("0123456789\n");
    hold.keep("abc");
    assert!(!hold.is_full());
    hold.keep("defgh\n");
    assert!(hold.is_full());
    // Back in room, but after a lost line: kept text stays in order.
    hold.keep("x\n");
    hold.keep("y\nz\n");
    assert_eq!(
        hold.close(),
        Released {
            text: "0123456789\n",
            lost: 4
        }
    );
    // Exactly full, with a line open at the close: that line is lost.
    hold.open(TERMINAL);
    hold.keep("0123456789abcdef");
    assert_eq!(hold.close(), Released { text: "", lost: 1 });
}

#[test]
fn text_written_through_fmt_is_kept_as_printed() {
    let mut hold = Hold::<64>::new();
    hold.open(TERMINAL);
    let (name, text) = ("app", "é ünï");
    let _ = writeln!(hold, "[{name}] {text}");
    assert_eq!(hold.close().text, "[app] é ünï\n");
}
