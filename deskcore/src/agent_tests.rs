use crate::agent::{Agent, Clients, Request, find_window, parse_windows};
use crate::desk::{Desk, Effect, Focus, Input, WindowId};
use crate::geom::Size;
use crate::view::mock::Mock;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use surfabi::tree::{Role, Tree};

/// Two client windows' trees, and the action requests sent.
struct Fake {
    trees: Vec<(WindowId, Tree)>,
    sent: Vec<(WindowId, u32, u32, String)>,
    /// The windows whose apps keep them from the agent.
    private: Vec<WindowId>,
}

impl Clients for Fake {
    fn tree(&self, window: WindowId) -> Option<&Tree> {
        self.trees
            .iter()
            .find(|(w, _)| *w == window)
            .map(|(_, t)| t)
    }

    fn private(&self, window: WindowId) -> bool {
        self.private.contains(&window)
    }

    fn act(&mut self, window: WindowId, node: u32, action: u32, value: &str) -> bool {
        let offered = self
            .tree(window)
            .and_then(|t| t.node(node))
            .is_some_and(|n| n.actions & (1 << action) != 0);
        if offered {
            self.sent.push((window, node, action, value.into()));
        }
        offered
    }
}

/// Notes (a search field and a button) and a terminal, the terminal on top.
fn desktop() -> (Desk, Fake, WindowId, WindowId) {
    let mut desk = Desk::new(1280, 800);
    let size = Size { w: 400, h: 300 };
    let (notes, _) = desk.open(1, "comphost", "Notes", size, true);
    desk.set_app_id(notes, "notes");
    let (term, _) = desk.open(2, "term", "kai>", size, true);
    let mut m = Mock::new("Notes");
    m.add(1, Role::TextInput, "Search");
    m.add(1, Role::Button, "New note");
    let fake = Fake {
        trees: vec![(notes, m.tree()), (term, Tree::default())],
        sent: Vec::new(),
        private: Vec::new(),
    };
    (desk, fake, notes, term)
}

#[test]
fn every_request_reads_back_and_nothing_else_does() {
    let requests = [
        Request::Windows,
        Request::View("notes".into()),
        Request::Focus("w2".into()),
        Request::Refocus("panel".into()),
        Request::Close("files".into()),
        Request::Act {
            window: "w1".into(),
            node: "t1".into(),
            action: 2,
            value: "milk and eggs".into(),
        },
        Request::Agent(Some("w1".into())),
        Request::Agent(None),
        Request::Check {
            window: "w1".into(),
            node: "b2".into(),
            action: 0,
        },
    ];
    for request in requests {
        assert_eq!(Request::parse(&request.encode()), Some(request.clone()));
    }
    for bad in [
        "",
        "ui",
        "ui windows now",
        "ui view",
        "ui view two words",
        "ui act w1 b2 0",
        "ui act w1 b2 16\t",
        "ui act w1 b2 0 extra\t",
        "ui act w1 b2 0\tline\nbreak",
        "panel take",
        "ui focus aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert_eq!(Request::parse(bad), None, "{bad:?}");
    }
    let long = alloc::format!("ui act w1 t1 2\t{}", "x".repeat(513));
    assert_eq!(Request::parse(&long), None);
}

#[test]
fn windows_list_front_to_back_and_read_back() {
    let (mut desk, mut fake, notes, term) = desktop();
    let mut agent = Agent::default();
    let (text, _) = agent.answer(&mut desk, &mut fake, "ui windows");
    let list = parse_windows(&text).unwrap();
    assert_eq!(
        list,
        [
            (
                crate::view::window_id(term),
                "term".into(),
                "kai>".into(),
                true
            ),
            (
                crate::view::window_id(notes),
                "notes".into(),
                "Notes".into(),
                false
            ),
        ]
    );
    assert_eq!(find_window(&desk, "NOTES"), Some(notes));
    assert_eq!(find_window(&desk, "kai>"), Some(term), "by title");
    assert_eq!(find_window(&desk, "w99"), None);
    assert_eq!(parse_windows("w1\tnotes\tNotes\tyes"), None);
}

#[test]
fn a_view_then_an_act_reaches_the_app_and_moves_the_outline() {
    let (mut desk, mut fake, notes, _) = desktop();
    let mut agent = Agent::default();
    // Acting before a view has no ids to read: the answer says to view.
    let (early, _) = agent.answer(&mut desk, &mut fake, "ui act notes b2 0\t");
    assert_eq!(early, "err unknown node; view w1 first");
    let (view, _) = agent.answer(&mut desk, &mut fake, "ui view notes");
    assert!(view.starts_with("ok windows: "), "{view}");
    assert!(view.contains("[t1] textbox \"Search\"  [b2] button \"New note\""));
    let (typed, _) = agent.answer(&mut desk, &mut fake, "ui act notes t1 2\tplumber");
    assert_eq!(typed, "ok");
    let (clicked, _) = agent.answer(&mut desk, &mut fake, "ui act w1 b2 0\t");
    assert_eq!(clicked, "ok");
    assert_eq!(
        fake.sent,
        [
            (notes, 2, 2, "plumber".into()),
            (notes, 3, 0, String::new())
        ]
    );
    assert_eq!(desk.agent(), Some(notes));
    let (refused, _) = agent.answer(&mut desk, &mut fake, "ui act notes b2 2\tx");
    assert_eq!(refused, "err b2 does not take that action");
    agent.closed(notes);
    let (gone, _) = agent.answer(&mut desk, &mut fake, "ui act notes b2 0\t");
    assert_eq!(gone, "err unknown node; view w1 first");
}

#[test]
fn a_private_window_shows_its_title_alone_and_takes_no_act_or_focus() {
    let (mut desk, mut fake, notes, _) = desktop();
    fake.private.push(notes);
    let mut agent = Agent::default();
    let (view, _) = agent.answer(&mut desk, &mut fake, "ui view notes");
    // The window line (with its title, as every view has), then no node.
    assert!(view.starts_with("ok windows: "), "{view}");
    assert!(view.ends_with("\nw1: private"), "{view}");
    assert!(
        !view.contains("Search") && !view.contains("New note"),
        "{view}"
    );
    let refused = "err notes is private: its app keeps it from the agent";
    let refused_w1 = "err w1 is private: its app keeps it from the agent";
    for (request, want) in [
        ("ui act notes b2 0\t", refused_w1),
        ("ui check notes b2 0", refused_w1),
        ("ui focus notes", refused),
        ("ui refocus notes", refused),
    ] {
        let (answer, effects) = agent.answer(&mut desk, &mut fake, request);
        assert_eq!(answer, want, "{request}");
        assert!(effects.is_empty(), "{request}");
    }
    assert!(fake.sent.is_empty());
    assert_eq!(desk.agent(), None);
}

#[test]
fn a_window_that_turns_private_after_a_view_takes_no_act() {
    let (mut desk, mut fake, notes, _) = desktop();
    let mut agent = Agent::default();
    agent.answer(&mut desk, &mut fake, "ui view notes");
    assert_eq!(
        agent.answer(&mut desk, &mut fake, "ui check notes b2 0").0,
        "ok"
    );
    fake.private.push(notes);
    let (act, _) = agent.answer(&mut desk, &mut fake, "ui act notes b2 0\t");
    assert_eq!(act, "err w1 is private: its app keeps it from the agent");
    // Its view went with the next one, so turning shared again needs a new view.
    agent.answer(&mut desk, &mut fake, "ui view notes");
    fake.private.clear();
    let (stale, _) = agent.answer(&mut desk, &mut fake, "ui act notes b2 0\t");
    assert_eq!(stale, "err unknown node; view w1 first");
    assert!(fake.sent.is_empty());
}

#[test]
fn check_names_an_unknown_or_guessed_node_and_sends_nothing() {
    let (mut desk, mut fake, _, _) = desktop();
    let mut agent = Agent::default();
    let (before, _) = agent.answer(&mut desk, &mut fake, "ui check notes b2 0");
    assert_eq!(before, "err unknown node; view w1 first");
    agent.answer(&mut desk, &mut fake, "ui view notes");
    for (request, want) in [
        ("ui check notes b2 0", "ok"),
        ("ui check w1 t1 2", "ok"),
        // An id the model made up, and one no view of that window showed.
        (
            "ui check notes new-note 0",
            "err unknown node; view w1 first",
        ),
        ("ui check notes b9 0", "err unknown node; view w1 first"),
        ("ui check kai> b2 0", "err unknown node; view w2 first"),
        ("ui check notes b2 2", "err b2 does not take that action"),
    ] {
        assert_eq!(
            agent.answer(&mut desk, &mut fake, request).0,
            want,
            "{request}"
        );
    }
    assert!(fake.sent.is_empty(), "a check acts on nothing");
    assert_eq!(desk.agent(), None);
}

#[test]
fn focus_says_where_it_was_and_refocus_puts_it_back() {
    let (mut desk, mut fake, notes, term) = desktop();
    let mut agent = Agent::default();
    let (moved, effects) = agent.answer(&mut desk, &mut fake, "ui focus notes");
    assert_eq!(moved, alloc::format!("ok {}", crate::view::window_id(term)));
    assert_eq!(desk.focus(), Focus::Window(notes));
    assert!(effects.contains(&Effect::Focus(Focus::Window(notes))));
    assert_eq!(desk.windows().last().unwrap().id, notes, "it rose");
    assert_eq!(
        desk.agent(),
        Some(notes),
        "the agent drives it: the outline"
    );
    let before = moved.strip_prefix("ok ").unwrap();
    let (back, _) = agent.answer(&mut desk, &mut fake, &alloc::format!("ui refocus {before}"));
    assert_eq!(back, "ok");
    assert_eq!(desk.focus(), Focus::Window(term));
    agent.answer(&mut desk, &mut fake, "ui refocus panel");
    assert_eq!(desk.focus(), Focus::Panel);
    let (unknown, _) = agent.answer(&mut desk, &mut fake, "ui focus files");
    assert_eq!(unknown, "err no window is files");
}

#[test]
fn close_asks_the_app_as_its_close_control_would() {
    let (mut desk, mut fake, notes, _) = desktop();
    let mut agent = Agent::default();
    let (closed, effects) = agent.answer(&mut desk, &mut fake, "ui close notes");
    assert_eq!(closed, "ok notes");
    assert_eq!(effects, [Effect::Close { window: notes }]);
    // The app decides: the window is still there until it closes it.
    assert!(desk.window(notes).is_some());
}

#[test]
fn a_key_in_the_window_the_agent_drives_stops_the_run() {
    let (mut desk, mut fake, notes, _) = desktop();
    let mut agent = Agent::default();
    agent.answer(&mut desk, &mut fake, "ui focus notes");
    let (ok, _) = agent.answer(&mut desk, &mut fake, "ui agent notes");
    assert_eq!(ok, "ok");
    assert_eq!(desk.agent(), Some(notes));
    let effects = desk.input(Input::Key {
        usage: 0x04,
        down: true,
    });
    assert!(
        effects.contains(&Effect::TookOver { window: notes }),
        "{effects:?}"
    );
    assert_eq!(desk.agent(), None);
    agent.answer(&mut desk, &mut fake, "ui agent notes");
    agent.answer(&mut desk, &mut fake, "ui agent none");
    assert_eq!(desk.agent(), None);
    // The rest of the run acts on nothing; looking is still allowed.
    agent.took_over();
    for request in [
        "ui focus notes",
        "ui act notes b2 0\t",
        "ui close notes",
        "ui agent notes",
    ] {
        let (refused, effects) = agent.answer(&mut desk, &mut fake, request);
        assert_eq!(refused, "err stopped: you took over", "{request}");
        assert!(effects.is_empty(), "{request}");
    }
    assert!(
        agent
            .answer(&mut desk, &mut fake, "ui view notes")
            .0
            .starts_with("ok ")
    );
    assert!(fake.sent.is_empty());
    // The shell's next request starts a new run.
    agent.answer(&mut desk, &mut fake, "ui agent none");
    assert!(
        agent
            .answer(&mut desk, &mut fake, "ui focus notes")
            .0
            .starts_with("ok ")
    );
    let (garbage, _) = agent.answer(&mut desk, &mut fake, "rm -rf /");
    assert_eq!(garbage, "err desktop does not know that request");
}
