//! The editor component on the host: keys, clicks, the wheel and the edit
//! service's answers drive an `EditorWindow` through a fake `Host`, and what
//! reaches the window and the service is checked. The service here answers from
//! one small file; `editcore`'s own tests run the real service under the screen.

use comp_editor_ui::{CODE, STATUS, TITLE};
use comp_editor_window::{EditorWindow, Host, keys};
use comp_ui_bind::tree::Entry;
use comp_ui_bind::{OnUi, Sink};
use compsdk::{Component, ImportError};
use editcore::ipc::{At, Ipc, decode_ipc};
use uikit::widget::{NodeId, UiError, UiEvent, Widget};

const FILE: &str = "src/lib.rs";
const LINES: [&str; 6] = [
    "fn main() {",
    "    run(); // see src/ui.rs:3",
    "}",
    "",
    "fn run() {}",
    "// end",
];

#[derive(Default)]
struct Os {
    opened: Vec<String>,
    puts: Vec<Entry>,
    commits: usize,
    closed: Vec<u32>,
    /// Requests sent and not answered: (ticket, message).
    sent: Vec<(u32, Vec<u8>)>,
    /// Every message ever sent, as text.
    log: Vec<String>,
    next_ticket: u32,
    now: u64,
}

impl Sink for Os {
    fn open(&mut self, title: &str, _width: u16, _height: u16) -> Result<u32, UiError> {
        self.opened.push(title.into());
        Ok(7)
    }

    fn put(&mut self, _window: u32, e: &Entry) -> Result<(), UiError> {
        self.puts.retain(|p| p.id != e.id);
        self.puts.push(e.clone());
        Ok(())
    }

    fn remove(&mut self, _window: u32, node: NodeId) -> Result<(), UiError> {
        self.puts.retain(|p| p.id != node);
        Ok(())
    }

    fn commit(&mut self, _window: u32) -> Result<(), UiError> {
        self.commits += 1;
        Ok(())
    }

    fn close(&mut self, window: u32) -> Result<(), UiError> {
        self.closed.push(window);
        Ok(())
    }
}

impl Host for Os {
    fn send(&mut self, message: &[u8]) -> u32 {
        self.next_ticket += 1;
        self.sent.push((self.next_ticket, message.to_vec()));
        self.log.push(String::from_utf8_lossy(message).into_owned());
        self.next_ticket
    }

    fn now_ms(&self) -> u64 {
        self.now
    }
}

impl Os {
    fn node(&self, id: NodeId) -> &Entry {
        self.puts
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("no node {id}"))
    }

    /// The rows the code view holds, the empty ones past the file's end cut off.
    fn code_rows(&self) -> Vec<String> {
        let Widget::Code(v) = &self.node(CODE).widget else {
            panic!("the code node is not a code view");
        };
        let mut rows: Vec<String> = v.rows.iter().map(|r| r.text.clone()).collect();
        while rows.len() > LINES.len() && rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }

    fn status(&self) -> String {
        match &self.node(STATUS).widget {
            Widget::Label { text, .. } => text.clone(),
            other => panic!("the status node is {other:?}"),
        }
    }

    fn title(&self) -> String {
        match &self.node(TITLE).widget {
            Widget::Label { text, .. } => text.clone(),
            other => panic!("the title node is {other:?}"),
        }
    }

    fn sent_text(&self) -> Vec<&str> {
        self.log.iter().map(String::as_str).collect()
    }
}

/// What the edit service answers for `request`, from the six-line file.
fn service(request: &str) -> String {
    let message = decode_ipc(request.as_bytes()).unwrap_or_else(|why| panic!("{request}: {why}"));
    match message {
        Ipc::Open { component, at, .. } => {
            let line = at.map_or(1, |At { line, .. }| line);
            format!("ok opened {component} 2, cursor at line {line}")
        }
        Ipc::Cmd(command) if command.starts_with("view ") => view(&command["view ".len()..]),
        Ipc::Cmd("files") => {
            format!(
                "ok 2 files; * changed, > current\n > {FILE} (6 lines)\n   src/ui.rs (10 lines)"
            )
        }
        Ipc::Cmd("build") => "ok building e1".into(),
        Ipc::Cmd(_) => "ok".into(),
        other => panic!("{other:?} is not for an app"),
    }
}

fn view(range: &str) -> String {
    let (from, to) = range.split_once(',').expect("view n,m");
    let (from, to): (usize, usize) = (from.parse().unwrap(), to.parse().unwrap());
    let mut out = format!("ok view notes 2 {FILE} {} 2 -", LINES.len());
    for n in from..=to.min(LINES.len()) {
        out.push_str(&format!("\n{n}:{}", LINES[n - 1]));
    }
    out
}

/// Answers every request the editor has out, and those its answers lead to.
fn serve(app: &mut EditorWindow, os: &mut Os) {
    for _ in 0..64 {
        let Some((ticket, message)) = os.sent.first().cloned() else {
            return;
        };
        os.sent.remove(0);
        let reply = service(&String::from_utf8(message).unwrap());
        app.on_reply(os, ticket, Ok(reply.into_bytes())).unwrap();
    }
    panic!("the editor keeps asking");
}

/// An editor opened on `FILE` at line 2 with the service's answers in.
fn opened() -> (EditorWindow, Os) {
    let mut app = EditorWindow::init();
    let mut os = Os::default();
    app.act_open(&mut os, "notes", FILE, 2).unwrap();
    serve(&mut app, &mut os);
    (app, os)
}

#[test]
fn opening_a_file_asks_the_service_and_draws_what_it_answers() {
    let mut app = EditorWindow::init();
    let mut os = Os::default();
    let said = app.act_open(&mut os, "notes", FILE, 2).unwrap();
    assert!(
        said.contains("notes") && said.contains("src/lib.rs:2"),
        "{said}"
    );
    // The window is up before the answer, and the first message is the open.
    assert_eq!(os.opened, ["Editor"]);
    assert_eq!(app.window(), Some(7));
    assert_eq!(
        decode_ipc(&os.sent[0].1),
        Ok(Ipc::Open {
            component: "notes",
            label: None,
            at: Some(At {
                path: FILE,
                line: 2
            }),
        })
    );
    serve(&mut app, &mut os);
    assert_eq!(os.code_rows(), LINES);
    assert!(os.title().contains("src/lib.rs"), "{}", os.title());
    assert_eq!(app.screen().position().0, 2);
}

#[test]
fn an_action_never_starts_a_build_and_never_sends_more_than_an_open_and_reads() {
    let (_, os) = opened();
    for text in os.sent_text() {
        let message = decode_ipc(text.as_bytes()).unwrap();
        assert!(
            matches!(&message, Ipc::Open { .. })
                || matches!(&message, Ipc::Cmd(c) if c.starts_with("view ") || *c == "files"),
            "{text}"
        );
        assert!(editcore::ipc::for_app(text.as_bytes()).is_ok(), "{text}");
    }
}

#[test]
fn typing_is_sent_as_one_edit_when_the_cursor_moves_on() {
    let (mut app, mut os) = opened();
    let before = os.log.len();
    app.on_key(&mut os, "x", "x", 0).unwrap();
    app.on_key(&mut os, "y", "y", 0).unwrap();
    // Held in the draft line: nothing has gone to the service.
    assert_eq!(os.log.len(), before);
    app.on_key(&mut os, "down", "", 0).unwrap();
    let edit = os.log[before..]
        .iter()
        .find(|m| m.starts_with("cmd c "))
        .unwrap_or_else(|| panic!("no edit in {:?}", &os.log[before..]));
    assert!(edit.contains("xy"), "{edit}");
    serve(&mut app, &mut os);
    assert!(app.screen().edits() >= 1);
}

#[test]
fn control_b_asks_for_a_build_and_alt_and_super_keys_do_nothing() {
    let (mut app, mut os) = opened();
    let before = os.log.len();
    app.on_key(&mut os, "b", "b", keys::ALT).unwrap();
    app.on_key(&mut os, "b", "b", keys::SUPER).unwrap();
    app.on_key(&mut os, "f12", "", 0).unwrap();
    assert_eq!(os.log.len(), before);
    app.on_key(&mut os, "b", "", keys::CTRL).unwrap();
    assert!(
        os.log[before..].iter().any(|m| m == "cmd build"),
        "{:?}",
        &os.log[before..]
    );
}

#[test]
fn a_failed_exchange_is_said_in_the_status_and_the_file_stays() {
    let (mut app, mut os) = opened();
    app.on_key(&mut os, "b", "", keys::CTRL).unwrap();
    let (ticket, _) = os.sent.remove(0);
    app.on_reply(&mut os, ticket, Err("the edit service is busy".into()))
        .unwrap();
    assert!(os.status().contains("busy"), "{}", os.status());
    assert_eq!(os.code_rows(), LINES);
}

#[test]
fn an_answer_for_no_request_is_ignored() {
    let (mut app, mut os) = opened();
    let (puts, commits) = (os.puts.clone(), os.commits);
    app.on_reply(&mut os, 9_999, Ok(b"ok view x 1 y 1 1 -\n1:z".to_vec()))
        .unwrap();
    assert_eq!(os.puts.len(), puts.len());
    assert_eq!(os.commits, commits);
    assert_eq!(os.code_rows(), LINES);
}

#[test]
fn the_wheel_moves_the_cursor_and_is_bounded() {
    let (mut app, mut os) = opened();
    app.on_scroll(&mut os, 3).unwrap();
    serve(&mut app, &mut os);
    assert_eq!(app.screen().position().0, 5);
    app.on_scroll(&mut os, -i32::MAX).unwrap();
    serve(&mut app, &mut os);
    assert_eq!(app.screen().position().0, 1);
    app.on_scroll(&mut os, i32::MIN).unwrap();
    serve(&mut app, &mut os);
    assert_eq!(app.screen().position().0, 1);
}

#[test]
fn a_ctrl_click_on_a_source_link_opens_its_line() {
    let (mut app, mut os) = opened();
    let before = os.log.len();
    // `src/ui.rs:3` is on the second row from text cell 18.
    app.on_click(&mut os, 1, 22, keys::CTRL).unwrap();
    assert!(
        os.log[before..].iter().any(|m| m == "cmd open src/ui.rs"),
        "{:?}",
        &os.log[before..]
    );
}

#[test]
fn a_click_without_ctrl_on_a_source_link_only_moves_the_cursor() {
    let (mut app, mut os) = opened();
    let before = os.log.len();
    app.on_click(&mut os, 1, 22, 0).unwrap();
    assert!(
        !os.log[before..].iter().any(|m| m.contains("open ")),
        "{:?}",
        &os.log[before..]
    );
    assert_eq!(app.screen().position().0, 2);
}

#[test]
fn a_plain_click_moves_the_cursor_without_asking_for_a_file() {
    let (mut app, mut os) = opened();
    let before = os.log.len();
    app.on_click(&mut os, 4, 3, 0).unwrap();
    serve(&mut app, &mut os);
    assert!(!os.log[before..].iter().any(|m| m.contains("open ")));
    assert_eq!(app.screen().position().0, 5);
}

#[test]
fn the_code_view_size_sets_how_many_rows_are_asked_for() {
    let (mut app, mut os) = opened();
    app.on_size(&mut os, 3, 60).unwrap();
    serve(&mut app, &mut os);
    let rows = os.code_rows();
    assert!(rows.len() <= 3, "{rows:?}");
    // A zero-row view still leaves one row.
    app.on_size(&mut os, 0, 0).unwrap();
    serve(&mut app, &mut os);
}

#[test]
fn a_bad_place_is_refused_before_it_is_sent() {
    let mut app = EditorWindow::init();
    let mut os = Os::default();
    for (component, path, line) in [
        ("", FILE, 1),
        ("no te", FILE, 1),
        ("notes", "", 1),
        ("notes", "/etc/passwd", 1),
        ("notes", "a b.rs", 1),
        ("notes", "src/\u{1b}[2J.rs", 1),
        ("notes", FILE, 0),
        ("notes", FILE, 100_000),
        ("notes\n", FILE, 1),
        ("a-component-with-a-name-over-24-bytes", FILE, 1),
        ("notes", &"d/".repeat(40), 1),
    ] {
        assert!(
            app.act_open(&mut os, component, path, line).is_err(),
            "{component:?} {path:?} {line}"
        );
    }
    assert!(os.log.is_empty() && os.opened.is_empty());
}

#[test]
fn the_status_says_where_the_editor_is() {
    let (mut app, mut os) = opened();
    let said = app.status(&mut os);
    assert!(
        said.contains("src/lib.rs") && said.contains("line 2"),
        "{said}"
    );
    assert!(said.contains("0 edit(s)"), "{said}");
    app.on_key(&mut os, "b", "", keys::CTRL).unwrap();
    serve(&mut app, &mut os);
    let said = app.status(&mut os);
    assert!(said.contains("line 2"), "{said}");
}

#[test]
fn closing_the_window_forgets_it() {
    let (mut app, mut os) = opened();
    let id = app.window().unwrap();
    app.on_ui(&mut os, id, &UiEvent::CloseRequested).unwrap();
    assert_eq!(os.closed, [id]);
    assert_eq!(app.window(), None);
    // Another window's close is not this one's.
    let (mut app, mut os) = opened();
    app.on_ui(&mut os, 99, &UiEvent::CloseRequested).unwrap();
    assert!(os.closed.is_empty());
}

#[test]
fn the_checkpoint_round_trips_and_an_unknown_schema_is_refused() {
    let app = EditorWindow::init();
    let bytes = app.export(comp_editor_window::SCHEMA).unwrap();
    assert!(EditorWindow::import(&bytes, comp_editor_window::SCHEMA).is_ok());
    assert!(app.export(comp_editor_window::SCHEMA + 1).is_none());
    assert!(matches!(
        EditorWindow::import(&bytes, comp_editor_window::SCHEMA + 1),
        Err(ImportError::Unsupported)
    ));
    assert!(matches!(
        EditorWindow::import(&[0xff, 0x00], comp_editor_window::SCHEMA),
        Err(ImportError::Bad)
    ));
}
