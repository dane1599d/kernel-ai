//! The Kai menu (Start in the code; ADR 0005 §8): one box that says "Ask
//! Kai or search apps", over three views.
//!
//! - Home, the box empty: the pinned apps, your own recent lines, a footer.
//! - Search, any text in the box: "Ask Kai: <text>" first, then the
//!   catalog's apps that match. The selection starts on the first app whose
//!   name starts with the text, else on Ask Kai: "notes" then Enter opens
//!   Notes, a question then Enter asks Kai.
//! - All apps, by the `apps` line, "All apps ›" or the taskbar's "more":
//!   the Apps sheet's rows, in its order, with its keys ([`Sheet`]).
//!
//! Rows are the catalog's data. Only typed text is ever sent to the model,
//! as Ask Kai; an app row sends `open <name>` or `install <name>`, which the
//! shell carries out without the model.

use crate::catalog::{App, Sheet, Source};
use crate::desk::usage;
use crate::keymap;
use alloc::string::String;
use alloc::vec::Vec;
use termcore::keys::Key;
use termcore::line::{Line, Outcome};

const ENTER: u16 = 0x28;
const KEYPAD_ENTER: u16 = 0x58;
const ESCAPE: u16 = 0x29;
const DOWN: u16 = 0x51;
const UP: u16 = 0x52;
const RIGHT: u16 = 0x4F;
const LEFT: u16 = 0x50;
/// The pinned grid's columns and its most tiles.
const COLS: usize = deskgeom::start::GRID_COLS as usize;
const PINNED: usize = COLS * deskgeom::start::GRID_ROWS as usize;
/// The longest line the box takes, as the pane's input.
pub const MAX_LINE: usize = crate::panel::MAX_LINE;
/// Search rows below Ask Kai, at most.
pub const MAX_MATCHES: usize = deskgeom::start::SEARCH_ROWS as usize - 1;

/// Which view Start shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Home,
    Search,
    AllApps,
}

/// One search row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// "Ask Kai: <text>".
    Ask,
    /// The catalog's app at this index.
    App(usize),
}

/// What Start asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// `open <name>`: a built-in or installed app.
    Open(String),
    /// `install <name>`: an offered one.
    Install(String),
    /// The typed text, to Kai: the panel queues it and the pane opens.
    Ask(String),
    /// Esc: Start closes.
    Close,
}

/// Start's state, while it is open.
pub struct Start {
    line: Line,
    sheet: Sheet,
    all_apps: bool,
    /// The selected search row.
    selected: usize,
    /// The selected pinned tile on Home, once an arrow moved there.
    home: Option<usize>,
    shift: u8,
    ctrl: u8,
    caps: bool,
}

impl Start {
    /// Start over the catalog's `apps`, on All apps when `all_apps`.
    pub fn new(apps: Vec<App>, all_apps: bool) -> Self {
        Start {
            line: Line::new(MAX_LINE),
            sheet: Sheet::new(apps),
            all_apps,
            selected: 0,
            home: None,
            shift: 0,
            ctrl: 0,
            caps: false,
        }
    }

    pub fn apps(&self) -> &[App] {
        self.sheet.apps()
    }

    /// All apps' rows and selection.
    pub fn sheet(&self) -> &Sheet {
        &self.sheet
    }

    pub fn text(&self) -> &str {
        self.line.text()
    }

    pub fn cursor(&self) -> usize {
        self.line.cursor()
    }

    pub fn view(&self) -> View {
        match (self.line.text().is_empty(), self.all_apps) {
            (false, _) => View::Search,
            (true, true) => View::AllApps,
            (true, false) => View::Home,
        }
    }

    /// "All apps ›", the `apps` line or "more".
    pub fn show_all(&mut self) {
        self.line = Line::new(MAX_LINE);
        self.all_apps = true;
    }

    /// Caps Lock, as the desk says.
    pub fn set_caps(&mut self, caps: bool) {
        self.caps = caps;
    }

    /// A recent line clicked: back in the box, to send again or edit.
    pub fn set_text(&mut self, text: &str) {
        let mut line = Line::new(MAX_LINE);
        for ch in text.chars() {
            line.apply(Key::Char(ch));
        }
        self.line = line;
        self.selected = self.default_row();
    }

    /// The search rows now: Ask Kai, then the apps whose name or summary
    /// holds the text, case-insensitive, in the catalog's order.
    pub fn rows(&self) -> Vec<Row> {
        let text = self.line.text().trim().to_ascii_lowercase();
        let mut rows = alloc::vec![Row::Ask];
        if text.is_empty() {
            return rows;
        }
        let matches = self.apps().iter().enumerate().filter(|(_, app)| {
            app.name.to_ascii_lowercase().contains(&text)
                || app.summary.to_ascii_lowercase().contains(&text)
        });
        rows.extend(matches.take(MAX_MATCHES).map(|(i, _)| Row::App(i)));
        rows
    }

    /// Home's pinned apps: the catalog's apps on the machine, in its order,
    /// at most 18 (6 by 3), as indices into [`Start::apps`].
    pub fn pinned(&self) -> Vec<usize> {
        self.apps()
            .iter()
            .enumerate()
            .filter(|(_, a)| matches!(a.source, Source::Installed | Source::BuiltIn))
            .map(|(i, _)| i)
            .take(PINNED)
            .collect()
    }

    /// Home's selected tile, an index into [`Start::pinned`].
    pub fn home_selected(&self) -> Option<usize> {
        self.home
    }

    /// The selected search row's index in [`Start::rows`].
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The default selection: the first app whose name starts with the
    /// text, else Ask Kai.
    fn default_row(&self) -> usize {
        let text = self.line.text().trim().to_ascii_lowercase();
        let rows = self.rows();
        rows.iter()
            .position(|row| match row {
                Row::App(i) => self.apps()[*i].name.to_ascii_lowercase().starts_with(&text),
                Row::Ask => false,
            })
            .filter(|_| !text.is_empty())
            .unwrap_or(0)
    }

    /// The wheel turned by `detents` over Start (Kai Desk PR 9): a row a
    /// detent in Search and All apps, down for a positive turn, within the
    /// rows; Home does not scroll. True when the selection moved.
    pub fn wheel(&mut self, detents: i32) -> bool {
        let code = if detents > 0 { DOWN } else { UP };
        let mut moved = false;
        for _ in 0..detents.unsigned_abs() {
            moved |= match self.view() {
                View::Home => false,
                View::AllApps => self.sheet.key(code).0,
                View::Search => {
                    let rows = self.rows().len();
                    let next = match code {
                        DOWN => (self.selected + 1).min(rows.saturating_sub(1)),
                        _ => self.selected.saturating_sub(1),
                    };
                    let changed = next != self.selected;
                    self.selected = next;
                    changed
                }
            };
        }
        moved
    }

    /// What a click on search row `row` asks for.
    pub fn pick_row(&self, row: usize) -> Option<Pick> {
        match self.rows().get(row)? {
            Row::Ask => {
                let text = self.line.text().trim();
                (!text.is_empty()).then(|| Pick::Ask(String::from(text)))
            }
            Row::App(i) => Some(open(&self.apps()[*i])),
        }
    }

    /// A key, `down` or up: whether Start changed, and what was picked.
    pub fn key(&mut self, code: u16, down: bool) -> (bool, Option<Pick>) {
        let held = |count: &mut u8| {
            *count = match down {
                true => count.saturating_add(1),
                false => count.saturating_sub(1),
            }
        };
        match code {
            usage::LEFT_SHIFT | usage::RIGHT_SHIFT => {
                held(&mut self.shift);
                return (false, None);
            }
            usage::LEFT_CTRL | usage::RIGHT_CTRL => {
                held(&mut self.ctrl);
                return (false, None);
            }
            _ if !down => return (false, None),
            ESCAPE => return (false, Some(Pick::Close)),
            _ => {}
        }
        match self.view() {
            View::Home if matches!(code, LEFT | RIGHT | UP | DOWN) => {
                let n = self.pinned().len();
                if n == 0 {
                    return (false, None);
                }
                let next = match (self.home, code) {
                    (None, _) => 0,
                    (Some(i), RIGHT) => (i + 1).min(n - 1),
                    (Some(i), LEFT) => i.saturating_sub(1),
                    (Some(i), DOWN) if i + COLS < n => i + COLS,
                    (Some(i), UP) if i >= COLS => i - COLS,
                    (Some(i), _) => i,
                };
                let changed = self.home != Some(next);
                self.home = Some(next);
                return (changed, None);
            }
            View::Home if code == ENTER || code == KEYPAD_ENTER => {
                let pick = self
                    .home
                    .and_then(|n| self.pinned().get(n).copied())
                    .map(|i| open(&self.apps()[i]));
                return (false, pick);
            }
            View::AllApps if matches!(code, UP | DOWN | ENTER) => {
                let (changed, pick) = self.sheet.key(code);
                let pick = pick.map(|p| match p {
                    crate::catalog::Pick::Open(name) => Pick::Open(name),
                    crate::catalog::Pick::Install(name) => Pick::Install(name),
                    crate::catalog::Pick::Close => Pick::Close,
                });
                return (changed, pick);
            }
            View::Search if code == UP && self.selected > 0 => {
                self.selected -= 1;
                return (true, None);
            }
            View::Search if code == DOWN && self.selected + 1 < self.rows().len() => {
                self.selected += 1;
                return (true, None);
            }
            View::Search if code == ENTER || code == KEYPAD_ENTER => {
                return (false, self.pick_row(self.selected));
            }
            _ if matches!(code, UP | DOWN | ENTER | KEYPAD_ENTER) => return (false, None),
            _ => {}
        }
        let Some(key) = keymap::key_of(code, self.shift > 0, self.ctrl > 0, self.caps) else {
            return (false, None);
        };
        match self.line.apply(key) {
            Outcome::Unchanged => (false, None),
            _ => {
                self.selected = self.default_row();
                (true, None)
            }
        }
    }
}

/// What an app row opens: `open` for one on the machine, `install` for an
/// offer.
pub fn open(app: &App) -> Pick {
    match app.source {
        Source::Installed | Source::BuiltIn => Pick::Open(app.name.clone()),
        _ => Pick::Install(app.name.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests::app;

    fn apps() -> Vec<App> {
        alloc::vec![
            app(
                Source::BuiltIn,
                "files",
                None,
                "Files: your files and folders"
            ),
            app(Source::BuiltIn, "notes", None, "Notes: short notes, kept"),
            app(Source::Mirror, "notepad", None, "A plain text editor"),
        ]
    }

    /// Types `text`, one key at a time (letters and spaces only).
    fn typed(start: &mut Start, text: &str) {
        for ch in text.chars() {
            let code = match ch {
                ' ' => 0x2C,
                c => 0x04 + (c as u8 - b'a') as u16,
            };
            start.key(code, true);
            start.key(code, false);
        }
    }

    #[test]
    fn notes_then_enter_opens_notes() {
        let mut start = Start::new(apps(), false);
        assert_eq!(start.view(), View::Home);
        typed(&mut start, "notes");
        assert_eq!(start.view(), View::Search);
        assert_eq!(start.rows(), [Row::Ask, Row::App(1)]);
        assert_eq!(start.selected(), 1, "the app whose name starts with it");
        assert_eq!(
            start.key(ENTER, true),
            (false, Some(Pick::Open("notes".into())))
        );
    }

    /// "not" matches notes and notepad: the first whose name starts with it.
    #[test]
    fn the_first_app_that_starts_with_the_text_is_the_default() {
        let mut start = Start::new(apps(), false);
        typed(&mut start, "not");
        assert_eq!(start.rows(), [Row::Ask, Row::App(1), Row::App(2)]);
        assert_eq!(start.selected(), 1);
        start.key(DOWN, true);
        assert_eq!(
            start.key(ENTER, true).1,
            Some(Pick::Install("notepad".into()))
        );
    }

    /// Text no app's name starts with makes Ask Kai the default, even where
    /// a summary matches; Enter asks Kai the text, and nothing else.
    #[test]
    fn a_question_defaults_to_ask_kai() {
        let mut start = Start::new(apps(), false);
        typed(&mut start, "what are my files");
        assert_eq!(start.selected(), 0);
        assert_eq!(
            start.key(ENTER, true).1,
            Some(Pick::Ask("what are my files".into()))
        );
        let mut start = Start::new(apps(), false);
        typed(&mut start, "text");
        assert_eq!(start.rows(), [Row::Ask, Row::App(2)]);
        assert_eq!(start.selected(), 0, "a summary match is not a name match");
    }

    /// All apps keeps the sheet's order and keys; typing searches instead;
    /// Esc closes from any view.
    #[test]
    fn all_apps_keeps_the_sheets_keys_and_esc_closes() {
        let mut start = Start::new(apps(), true);
        assert_eq!(start.view(), View::AllApps);
        assert_eq!(start.key(DOWN, true), (true, None));
        assert_eq!(start.key(ENTER, true).1, Some(Pick::Open("notes".into())));
        typed(&mut start, "f");
        assert_eq!(start.view(), View::Search);
        assert_eq!(start.key(ESCAPE, true).1, Some(Pick::Close));
        let mut home = Start::new(apps(), false);
        assert_eq!(
            home.key(ENTER, true),
            (false, None),
            "an empty box sends nothing"
        );
        assert_eq!(home.key(ESCAPE, true).1, Some(Pick::Close));
    }

    /// Home: an arrow selects the first pinned tile, the arrows move over
    /// the grid within it, and Enter opens the selected one.
    #[test]
    fn home_tiles_are_chosen_with_the_arrows() {
        let mut start = Start::new(apps(), false);
        assert_eq!(start.pinned(), [0, 1], "an offer is never pinned");
        assert_eq!(start.home_selected(), None);
        assert_eq!(start.key(RIGHT, true), (true, None));
        assert_eq!(start.home_selected(), Some(0));
        assert_eq!(start.key(RIGHT, true), (true, None));
        assert_eq!(start.key(RIGHT, true), (false, None), "the last tile");
        assert_eq!(start.key(DOWN, true), (false, None), "no row below");
        assert_eq!(start.key(ENTER, true).1, Some(Pick::Open("notes".into())));
    }

    /// The wheel moves a row a detent in All apps and Search, never past
    /// the ends, and leaves Home be.
    #[test]
    fn the_wheel_moves_a_row_a_detent() {
        let mut all = Start::new(apps(), true);
        assert!(all.wheel(2));
        assert_eq!(all.sheet().selected_index(), 2);
        assert!(!all.wheel(5), "past the last row");
        assert!(all.wheel(-1));
        assert_eq!(all.sheet().selected_index(), 1);
        let mut search = Start::new(apps(), false);
        typed(&mut search, "e");
        assert_eq!(search.selected(), 0);
        assert!(search.wheel(1));
        assert_eq!(search.selected(), 1);
        let mut home = Start::new(apps(), false);
        assert!(!home.wheel(3));
    }

    /// A recent line clicked comes back into the box, with its default.
    #[test]
    fn a_recent_line_comes_back_into_the_box() {
        let mut start = Start::new(apps(), false);
        start.set_text("notes");
        assert_eq!(start.text(), "notes");
        assert_eq!(start.selected(), 1);
    }
}
