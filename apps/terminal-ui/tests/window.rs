//! The Terminal window on the host, through a real `uikit::Window`: every phase's
//! tree is one `uikit` takes, new rows send only themselves, a key typed in the
//! input line reaches the model and the shell's reply reaches the window, and
//! the AI and the user press the same Clear.

use comp_terminal::os::Recorder;
use comp_terminal::span::{Kind, MAX_SPANS};
use comp_terminal::{Phase, Source, Submitted, Terminal};
use comp_terminal_ui::{
    CANCEL, CLEAR, CLEAR_ACTION, Change, Entry, INPUT, SHOWN_SPANS, SIZE, SPANS, STATUS, View,
    WAITING, build, diff, next, on_ui, wants_focus,
};
use uikit::a11y::Action;
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance as M;
use uikit::widget::{ROOT, UiEvent, Value, Widget};
use uikit::window::Window;

fn window() -> Window {
    Window::new("Terminal", SIZE.0, SIZE.1)
}

fn apply(w: &mut Window, changes: &[Change]) {
    for c in changes {
        match c {
            Change::Put(e) => w
                .put(e.id, e.parent, e.layout, e.widget.clone())
                .unwrap_or_else(|err| panic!("{err:?} putting {e:?}")),
            Change::Remove(id) => w
                .remove(*id)
                .unwrap_or_else(|err| panic!("{err:?} removing {id}")),
        }
    }
}

/// The window, its last tree and the view, as the component keeps them.
struct App {
    term: Terminal,
    os: Recorder,
    view: View,
    shown: Vec<Entry>,
    w: Window,
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            term: Terminal::new(),
            os: Recorder::default(),
            view: View::default(),
            shown: Vec::new(),
            w: window(),
        };
        app.present();
        app
    }

    /// Sends what changed; the changes that went.
    fn present(&mut self) -> Vec<Change> {
        let n = next(&mut self.shown, &self.term, &self.view);
        apply(&mut self.w, &n.changes);
        if let Some(focus) = n.focus {
            self.w.focus(focus).unwrap();
        }
        self.shown = n.tree;
        self.view.sent();
        self.w.commit(&M);
        n.changes
    }

    /// Delivers `e` as the component would; the line it sent to the shell, if any.
    fn event(&mut self, e: &UiEvent) -> Option<String> {
        let before = self.os.lines().len();
        if on_ui(&mut self.term, &mut self.os, &mut self.view, e) {
            self.present();
        }
        self.os.lines().get(before).map(|args| plain(args))
    }

    fn key(&mut self, input: Input) -> Option<String> {
        let mut sent = None;
        for e in self.w.input(input, &M) {
            sent = sent.or(self.event(&e));
        }
        sent
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.key(Input::Key {
                key: Key::Other(0x04),
                down: true,
                mods: Mods::default(),
                text: ch.to_string(),
            });
        }
    }

    fn enter(&mut self) -> Option<String> {
        self.key(Input::Key {
            key: Key::Enter,
            down: true,
            mods: Mods::default(),
            text: String::new(),
        })
    }

    /// The shell's answer to the latest effect, as `effect-done` brings it.
    fn reply(&mut self, wire: &str) {
        let ticket = self.os.last_ticket().unwrap();
        self.term.done(&mut self.os, ticket, Ok(wire.to_string()));
        self.present();
    }
}

/// A line from the effect's record `{line: "..."}`, for lines without escapes.
fn plain(args: &str) -> String {
    args.strip_prefix("{line: \"")
        .and_then(|rest| rest.strip_suffix("\"}"))
        .unwrap_or(args)
        .to_string()
}

fn a11y_names(w: &mut Window) -> Vec<String> {
    w.commit(&M).a11y.into_iter().map(|n| n.name).collect()
}

fn has_node(changes: &[Change], id: u32) -> bool {
    changes.iter().any(|c| match c {
        Change::Put(e) => e.id == id,
        Change::Remove(r) => *r == id,
    })
}

#[test]
fn every_phase_is_a_tree_uikit_takes() {
    let mut t = Terminal::new();
    let view = View::default();
    let phases = [
        |_: &mut Terminal| {},
        |t: &mut Terminal| {
            t.submit("fn f() {", Source::Typed);
            t.reply("more", None);
        },
        |t: &mut Terminal| {
            t.submit("go", Source::Ai);
        },
        |t: &mut Terminal| {
            t.reply("confirm 3 script 3: trash", None);
        },
        |t: &mut Terminal| {
            t.cancel();
            t.reply("err denied", Some("c> "));
        },
    ];
    for step in phases {
        step(&mut t);
        let mut w = window();
        for e in build(&t, &view) {
            w.put(e.id, e.parent, e.layout, e.widget.clone())
                .unwrap_or_else(|err| panic!("{err:?} putting {e:?}"));
        }
        assert!(!w.commit(&M).a11y.is_empty());
    }
}

#[test]
fn the_window_names_the_prompt_the_clear_action_and_the_status() {
    let mut app = App::new();
    let frame = app.w.commit(&M);
    let by = |id| frame.a11y.iter().find(|n| n.id == id).unwrap();
    assert_eq!(by(INPUT).name, "kai>");
    assert_eq!(by(CLEAR).bound.as_deref(), Some(CLEAR_ACTION));
    assert!(by(STATUS).name.starts_with("Ready · 1 rows"));
    assert!(frame.a11y.iter().all(|n| n.id != CANCEL));
    assert_eq!(app.w.focus_node(), Some(INPUT));
}

#[test]
fn a_new_row_sends_only_itself_and_the_input_stays_put() {
    let mut app = App::new();
    app.term.submit("mem", Source::Typed);
    app.present();
    app.term.reply("ok 12 MiB\nheap 3", None);
    let changes = app.present();
    // The rows and the status are new or changed; the input line comes back
    // (it was replaced by "Running"), and the scrollback's column is untouched.
    assert!(has_node(&changes, INPUT));
    assert!(!has_node(&changes, SPANS));
    assert!(!has_node(&changes, ROOT));
    // With the input already there, a notice touches neither it nor its box.
    app.term.submit("   ", Source::Typed);
    app.term.submit("a\nb", Source::Ai);
    let changes = app.present();
    assert!(!has_node(&changes, INPUT));
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Put(e) if e.parent == SPANS))
    );
    assert_eq!(app.w.focus_node(), Some(INPUT));
}

#[test]
fn a_typed_line_goes_to_the_model_and_the_field_clears() {
    let mut app = App::new();
    app.type_text("mem");
    assert_eq!(app.view.draft, "mem");
    let sent = app.enter();
    assert_eq!(sent.as_deref(), Some("mem"));
    assert_eq!(app.term.phase(), Phase::Waiting);
    // The input line gives way to "Running" while the shell works.
    let names = a11y_names(&mut app.w);
    assert!(names.iter().any(|n| n == "Running"));
    assert!(!names.iter().any(|n| n == "kai>"));
    app.reply("ok free 12 MiB");
    assert_eq!(app.term.phase(), Phase::Ready);
    let f = app.w.commit(&M);
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert_eq!(input.value.as_deref().unwrap_or(""), "");
    assert!(input.focused);
}

#[test]
fn the_field_clears_even_when_the_input_never_left() {
    // A refused line keeps its draft; a handled one (:clear) empties the field.
    let mut app = App::new();
    app.type_text(":clear");
    assert_eq!(app.enter(), None);
    assert_eq!(app.view.draft, "");
    let f = app.w.commit(&M);
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert_eq!(input.value.as_deref().unwrap_or(""), "");
    assert!(app.term.spans().is_empty());
}

#[test]
fn a_refused_line_stays_in_the_field() {
    let mut app = App::new();
    app.type_text("answer 3 yes");
    assert_eq!(app.enter(), None);
    assert_eq!(app.view.draft, "answer 3 yes");
    let f = app.w.commit(&M);
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert_eq!(input.value.as_deref(), Some("answer 3 yes"));
    assert_eq!(app.term.spans().last().unwrap().kind, Kind::Notice);
}

#[test]
fn an_open_entry_changes_the_label_and_offers_cancel() {
    let mut app = App::new();
    app.type_text("if x {");
    app.enter();
    app.reply("more");
    let f = app.w.commit(&M);
    assert_eq!(f.a11y.iter().find(|n| n.id == INPUT).unwrap().name, "..>");
    assert!(f.a11y.iter().any(|n| n.id == CANCEL));
    // Cancel drops the entry: the shell is sent the cancel line.
    let events = app.w.act(CANCEL, Action::Click, None, &M).unwrap();
    assert_eq!(events, [UiEvent::Pressed(CANCEL)]);
    assert_eq!(app.event(&events[0]).as_deref(), Some(r"\u{3}"));
    // The record escapes the control character the shell takes as "drop it".
    assert_eq!(comp_terminal::CANCEL_ENTRY, "\u{3}");
}

#[test]
fn a_question_takes_no_input_and_only_declines() {
    let mut app = App::new();
    app.type_text("stop it");
    app.enter();
    app.reply("confirm 4 script 4: stop it");
    assert!(matches!(app.term.phase(), Phase::Asking(_)));
    let names = a11y_names(&mut app.w);
    assert!(
        names
            .iter()
            .any(|n| n == "A question is waiting on the AI panel")
    );
    assert!(names.iter().any(|n| n == "? script 4: stop it"));
    let events = app.w.act(CANCEL, Action::Click, None, &M).unwrap();
    assert_eq!(app.event(&events[0]).as_deref(), Some("answer 4 no"));
}

#[test]
fn the_ai_and_the_button_clear_the_same_way() {
    let mut app = App::new();
    app.term.submit("mem", Source::Ai);
    app.term.reply("ok 1", None);
    app.present();
    let events = app.w.act(CLEAR, Action::Click, None, &M).unwrap();
    assert_eq!(app.event(&events[0]), None);
    assert!(app.term.spans().is_empty());
    // "terminal_clear" is the action the button names, and the model's clear is
    // what that action runs.
    assert_eq!(CLEAR_ACTION, "terminal_clear");
}

#[test]
fn the_ais_typed_line_waits_in_the_field_for_a_person() {
    let mut app = App::new();
    let said = comp_terminal_ui::type_line(&mut app.term, &mut app.view, "mem").unwrap();
    assert_eq!(said, comp_terminal::TYPED);
    app.present();
    let f = app.w.commit(&M);
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert_eq!(input.value.as_deref(), Some("mem"));
    assert!(input.focused);
    assert!(app.os.asked.is_empty(), "typing sends nothing to the shell");
    // A person's Enter runs it, and the row says the AI typed it.
    assert_eq!(app.enter().as_deref(), Some("mem"));
    let echo = app
        .term
        .spans()
        .iter()
        .rfind(|s| s.kind == Kind::Input)
        .unwrap();
    assert!(echo.by_ai);
    // What a person has started to type is theirs: the AI cannot type over it.
    app.reply("ok");
    app.type_text("par");
    let refused = comp_terminal_ui::type_line(&mut app.term, &mut app.view, "mem");
    assert!(refused.is_err());
    assert_eq!(app.view.draft, "par");
    // A line the terminal would refuse is not typed either.
    app.view.draft.clear();
    assert!(comp_terminal_ui::type_line(&mut app.term, &mut app.view, "a\nb").is_err());
    assert_eq!(app.view.draft, "");
}

#[test]
fn rows_show_their_kind_in_text_a_reply_cannot_fake() {
    let mut app = App::new();
    app.term.submit("x", Source::Typed);
    app.term.reply("ok ! not an error", None);
    app.term.submit("y", Source::Ai);
    app.term.reply("err bad\nline two", None);
    app.present();
    let names = a11y_names(&mut app.w);
    assert!(names.contains(&"= ! not an error".to_string()));
    assert!(names.contains(&"! bad".to_string()));
    assert!(names.contains(&"! line two".to_string()));
    assert!(names.contains(&"by the AI".to_string()));
}

#[test]
fn long_lines_wrap_with_a_hanging_indent() {
    let mut t = Terminal::new();
    t.submit("go", Source::Typed);
    let words = vec!["word"; 40].join(" ");
    t.reply(&format!("ok {words}"), None);
    let tree = build(&t, &View::default());
    let reply = tree
        .iter()
        .find_map(|e| match &e.widget {
            Widget::Label { text, .. } if text.starts_with("= word") => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    let lines: Vec<&str> = reply.lines().collect();
    assert!(lines.len() >= 2);
    assert!(
        lines
            .iter()
            .all(|l| l.chars().count() <= comp_terminal_ui::COLS)
    );
    assert!(lines[1..].iter().all(|l| l.starts_with("  word")));
}

#[test]
fn the_tree_holds_only_the_newest_rows() {
    let mut t = Terminal::new();
    for i in 0..MAX_SPANS {
        t.submit(&format!("echo {i}"), Source::Typed);
        t.reply("ok", None);
    }
    let tree = build(&t, &View::default());
    let rows = tree.iter().filter(|e| e.parent == SPANS).count();
    assert_eq!(rows, SHOWN_SPANS);
    // A frame of the full tree still lays out and paints.
    let mut w = window();
    for e in &tree {
        w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap();
    }
    assert!(!w.commit(&M).a11y.is_empty());
}

/// Applies `changes` to a window holding `old` and checks it matches `new`.
fn diffs_to(old: &[Entry], new: &[Entry]) {
    let mut w = window();
    for e in old {
        w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap();
    }
    apply(&mut w, &diff(old, new));
    let got: Vec<(u32, u32, Widget)> = w
        .tree()
        .preorder()
        .into_iter()
        .filter(|&id| id != ROOT)
        .map(|id| {
            let n = w.tree().get(id).unwrap();
            (id, n.parent, n.widget.clone())
        })
        .collect();
    let want: Vec<(u32, u32, Widget)> = new
        .iter()
        .map(|e| (e.id, e.parent, e.widget.clone()))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn a_diff_turns_any_phase_into_any_other() {
    let steps: [fn(&mut Terminal); 6] = [
        |t| {
            t.submit("a", Source::Typed);
        },
        |t| {
            t.reply("ok one\ntwo", None);
        },
        |t| {
            t.submit("f {", Source::Ai);
            t.reply("more", Some("c> "));
        },
        |t| {
            t.submit("}", Source::Typed);
            t.reply("confirm 2 script", None);
        },
        |t| {
            t.cancel();
            t.reply("err denied", None);
        },
        |t| t.clear(),
    ];
    let mut trees = vec![build(&Terminal::new(), &View::default())];
    let mut t = Terminal::new();
    for step in steps {
        step(&mut t);
        trees.push(build(&t, &View::default()));
    }
    for old in &trees {
        for new in &trees {
            diffs_to(old, new);
        }
    }
}

#[test]
fn focus_goes_to_the_input_only_when_it_appears() {
    let t = Terminal::new();
    let ready = build(&t, &View::default());
    assert_eq!(wants_focus(&[], &ready), Some(INPUT));
    assert_eq!(wants_focus(&ready, &ready), None);
    let mut busy = Terminal::new();
    busy.submit("x", Source::Typed);
    let waiting = build(&busy, &View::default());
    assert!(waiting.iter().any(|e| e.id == WAITING));
    assert_eq!(wants_focus(&ready, &waiting), None);
    assert_eq!(wants_focus(&waiting, &ready), Some(INPUT));
}

#[test]
fn a_changed_event_for_another_node_does_nothing() {
    let mut t = Terminal::new();
    let mut os = Recorder::default();
    let mut view = View::default();
    for e in [
        UiEvent::Changed(99, Value::Text("x".into())),
        UiEvent::Pressed(99),
        UiEvent::Submitted(99),
    ] {
        assert!(!on_ui(&mut t, &mut os, &mut view, &e));
    }
    assert_eq!(view, View::default());
    assert!(os.asked.is_empty());
    assert!(matches!(t.submit("", Source::Typed), Submitted::Nothing));
}
