//! The fuzz targets' bodies for `deskcore`, the parsers and queues `desktop`
//! runs on other processes' bytes (plan p3-desktop §9, P8): a client's
//! `kai-surface 1` calls through `Client::handle` and the desk, the text wires
//! of the shell and the apps' `comphost`s (settings, models, effects, network,
//! panel, files) and the agent's `ui ...` requests. Each `fuzz_targets/*.rs`
//! calls one with libFuzzer's input, and `deskcore/tests/hostile.rs` includes
//! this file and runs each over hostile mutations of the samples here, which
//! also seed the targets' corpora. Each must return for any input; only a bug
//! in the code under it may panic.

#![allow(dead_code)]

use deskcore::agent::{self, Agent, Clients};
use deskcore::desk::{Desk, WindowId};
use deskcore::files::{self, Files};
use deskcore::geom::Size;
use deskcore::server::Client;
use deskcore::{effects, models_wire, net_wire, panel_wire, settings_wire, view};
use surfabi::proto::{REGISTRY, Request};
use surfabi::tree::{Node, Role, Tree, Update, action, flag};
use surfabi::{Format, Rect};

/// The most a call carries (`kabi::MAX_MESSAGE`).
const CALL: usize = 8192;

/// A client's first window and a few of its calls, as one byte stream.
pub fn calls() -> Vec<u8> {
    let requests = [
        (
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 1,
                id: 2,
            },
        ),
        (
            REGISTRY,
            Request::Bind {
                name: 2,
                interface: "surface",
                version: 1,
                id: 3,
            },
        ),
        (
            REGISTRY,
            Request::Bind {
                name: 3,
                interface: "seat",
                version: 1,
                id: 7,
            },
        ),
        (
            2,
            Request::PoolCreate {
                size: 4096 + 200 * 100 * 4,
            },
        ),
        (
            2,
            Request::PoolBuffer {
                id: 4,
                offset: 4096,
                width: 200,
                height: 100,
                stride: 800,
                format: Format::Xrgb8888,
            },
        ),
        (3, Request::GetToplevel { id: 5 }),
        (5, Request::SetTitle("Notes")),
        (5, Request::SetAppId("app:notes")),
        (
            5,
            Request::SetMinSize {
                width: 50,
                height: 40,
            },
        ),
        (5, Request::SetUnsaved(true)),
        (3, Request::GetA11y { id: 6 }),
        (3, Request::SetBufferScale(150)),
        (3, Request::Attach { buffer: 4 }),
        (
            3,
            Request::DamageBuffer(Rect {
                x: 0,
                y: 0,
                w: 200,
                h: 100,
            }),
        ),
        (3, Request::Commit),
        (
            3,
            Request::DamageBuffer(Rect {
                x: 5,
                y: 5,
                w: 10,
                h: 10,
            }),
        ),
        (3, Request::Commit),
        (5, Request::AckConfigure { serial: 1 }),
        (4, Request::BufferDestroy),
        (5, Request::ToplevelDestroy),
    ];
    requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap())
        .collect()
}

/// The `desk_server` target: a client's calls, one `Client::handle` each as
/// `desktop` serves them, on a desk, until the first error closes it, with the
/// events and input it would have sent taken as `desktop` takes them.
pub fn desk_server(bytes: &[u8]) {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(7, "fuzz");
    for (n, call) in bytes.chunks(CALL).enumerate() {
        let handled = client.handle(&mut desk, call, n == 0);
        // Every pool it may have made is mapped, as `desktop` does when it can.
        for pool in 1..=8 {
            client.pool_mapped(pool);
        }
        let _ = client.take_events(CALL);
        let _ = client.take_posts();
        if handled.is_err() {
            break;
        }
    }
    let _ = desk.take_damage();
    let _ = client.close_all(&mut desk);
}

/// Splits the input into the fields a wire is fed: NUL-separated, as text.
fn fields(bytes: &[u8]) -> Vec<(u8, String)> {
    bytes
        .split(|&b| b == 0)
        .map(|f| {
            (
                f.first().copied().unwrap_or(0),
                String::from_utf8_lossy(f.get(1..).unwrap_or(&[])).into_owned(),
            )
        })
        .collect()
}

/// A sample for the queues: a settings put, a take, an answer, an effect, an
/// ask, each a field.
pub fn queues() -> Vec<u8> {
    let mut out = Vec::new();
    for (op, text) in [
        (0u8, "7\t1\tappearance_set theme light"),
        (1, ""),
        (2, "7\tok\t2\nok"),
        (3, "9\t1\tui_act w1 b1"),
        (4, ""),
        (5, "example.com\thosts:any"),
        (6, "notes-window example.com\nallowed"),
        (7, ""),
    ] {
        out.push(op);
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
    out
}

/// The `desk_queues` target: the settings, models, effects and network queues
/// fed each field as the verb its first byte picks, in an order the input
/// chooses: put, take, put back, answer and read the results.
pub fn desk_queues(bytes: &[u8]) {
    let mut settings = settings_wire::Queue::default();
    let mut models = settings_wire::Queue::default();
    let mut effects = effects::Queue::default();
    let mut sent = effects::Sent::default();
    let mut net = net_wire::Queue::default();
    let mut taken_settings = Vec::new();
    let mut taken_effects = None;
    for (n, (op, text)) in fields(bytes).into_iter().enumerate() {
        let now = n as u64 * 1_000;
        match op % 12 {
            0 => {
                let _ = settings.put(settings_wire::SETTINGS_APP, &text);
            }
            1 => {
                let _ = models.put_as(&models_wire::MODELS, models_wire::MODELS_APP, &text);
            }
            2 => {
                if let Some(taken) = settings.take(op as usize * 64) {
                    taken_settings = taken;
                }
            }
            3 => {
                settings.put_back(core::mem::take(&mut taken_settings));
            }
            4 => {
                if let Some(answers) = settings_wire::parse_answers(&text) {
                    for answer in answers {
                        settings.finish(answer);
                    }
                }
                let _ = settings.results(settings_wire::MAX_ANSWER);
            }
            5 => {
                let _ = effects.put_as("notes-window", &text, op & 1 == 1);
            }
            6 => {
                taken_effects = effects.take(op as usize * 64);
                if let Some(taken) = &taken_effects {
                    sent.record(&taken.lines.join("\n"));
                }
            }
            7 => {
                if let Some(taken) = taken_effects.take() {
                    effects.put_back(taken);
                }
            }
            8 => {
                for done in effects.results("notes-window", effects::MESSAGE_BUDGET) {
                    effects.finish("notes-window", done);
                }
                let (accepted, _) = sent.accept(Vec::new());
                let _ = effects::split_notice(accepted);
            }
            9 => {
                let _ = net.ask("notes-window", &text, now);
            }
            10 => {
                let _ = net.take();
                let _ = net.done(&text, now);
            }
            _ => {
                let _ = effects.withdraw_users("notes-window");
            }
        }
    }
}

/// A sample for the parsers: the wires' own encodings.
pub fn texts() -> Vec<u8> {
    let mut out = Vec::new();
    for text in [
        settings_wire::encode_take(None),
        panel_wire::encode_appearance("kai-appearance 1\ntheme = light\n"),
        panel_wire::encode_said("hello"),
        panel_wire::encode_model("qwen3"),
        net_wire::encode_ask("example.com", "hosts:any"),
        net_wire::encode_done("notes-window", "example.com", "allowed"),
        effects::encode_take_but(&["notes-window"]),
        String::from("files take"),
        String::from("files put 1 editor read /home/user/todo.txt"),
        String::from("ui view w1"),
        String::from("ui act w1 b1 hi"),
        String::from("ui windows"),
    ] {
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
    out
}

/// The `desk_parsers` target: every wire's reader on each NUL-separated piece
/// of the input, as text and as bytes, and the files door's request handler as
/// the shell and as the editor.
pub fn desk_parsers(bytes: &[u8]) {
    let mut files = Files::default();
    for (n, piece) in bytes.split(|&b| b == 0).enumerate() {
        let text = String::from_utf8_lossy(piece);
        let text = text.as_ref();
        let _ = settings_wire::parse_request(text);
        let _ = settings_wire::parse_take(text);
        let _ = settings_wire::parse_back_for(&settings_wire::SETTINGS, text);
        let _ = settings_wire::parse_answers(text);
        let _ = panel_wire::parse_appearance(text);
        let _ = panel_wire::parse_ready(text);
        let _ = panel_wire::parse_model(text);
        let _ = panel_wire::parse_card(text);
        let _ = panel_wire::parse_card_held(text);
        let _ = panel_wire::parse_said(text);
        let _ = panel_wire::parse_note(text);
        let _ = panel_wire::parse_take(text);
        let _ = panel_wire::parse_done(text);
        let _ = panel_wire::parse_state(text);
        let _ = panel_wire::clean_model(text);
        let _ = net_wire::parse_take(text);
        let _ = effects::parse_back(text);
        let _ = effects::parse_take_but(text);
        let _ = effects::parse_take_during_but(text);
        let _ = effects::ticket_of(text);
        let _ = effects::split_stops(text);
        let _ = files::parse_job(text);
        let _ = files::check_path(text);
        let _ = files::check_name(text);
        let _ = agent::Request::parse(text);
        let _ = agent::parse_windows(text);
        let _ = view::parse_window(text);
        let now = n as u64 * 10_000;
        let _ = files.request(files::SHELL, piece, now);
        let _ = files.request(files::DOOR_APP, piece, now);
    }
}

/// An app's tree for the agent's requests to read and act on.
struct Windows(Tree);

impl Clients for Windows {
    fn tree(&self, _: WindowId) -> Option<&Tree> {
        Some(&self.0)
    }

    fn private(&self, window: WindowId) -> bool {
        window.0.is_multiple_of(5)
    }

    fn act(&mut self, _: WindowId, node: u32, action: u32, _: &str) -> bool {
        self.0
            .node(node)
            .is_some_and(|n| action < 16 && n.actions & (1 << action) != 0)
    }
}

fn tree() -> Tree {
    let node = |id, role, name: &str, children: Vec<u32>| Node {
        id,
        role,
        flags: flag::FOCUSABLE,
        actions: action::CLICK | action::FOCUS,
        x: 0,
        y: 0,
        w: 10,
        h: 10,
        name: name.into(),
        value: "v".into(),
        children,
    };
    let mut tree = Tree::default();
    let _ = tree.apply(Update {
        root: 1,
        focus: 2,
        nodes: vec![
            node(1, Role::Window, "Notes", vec![2, 3]),
            node(2, Role::Button, "New note", vec![]),
            node(3, Role::List, "Notes", vec![4]),
            node(4, Role::ListItem, "Groceries", vec![]),
        ],
    });
    tree
}

/// A sample for the agent: its requests, a line each.
pub fn requests() -> Vec<u8> {
    let mut out = Vec::new();
    for text in [
        "ui windows",
        "ui view w1",
        "ui act w1 b1",
        "ui act w1 t2 hello",
        "ui focus w2",
        "ui check w1 b1",
        "ui agent w1",
        "ui agent none",
        "ui close w2",
        "ui refocus w1",
    ] {
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
    out
}

/// The `desk_agent` target: the agent's `ui ...` requests, a line each, on a
/// desk of three windows whose app has a small tree, one window of which is
/// private, until the input ends.
pub fn desk_agent(bytes: &[u8]) {
    let mut desk = Desk::new(1280, 800);
    for title in ["One", "Two", "Three"] {
        desk.open(1, "fuzz", title, Size { w: 300, h: 200 }, true);
    }
    let mut agent = Agent::default();
    let mut windows = Windows(tree());
    for piece in bytes.split(|&b| b == 0) {
        let text = String::from_utf8_lossy(piece);
        let _ = agent.answer(&mut desk, &mut windows, &text);
    }
}

/// The seeds of each target's corpus: the samples above.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("desk_server", calls()),
        ("desk_queues", queues()),
        ("desk_parsers", texts()),
        ("desk_agent", requests()),
    ]
}
