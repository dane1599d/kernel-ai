//! The Files component as built for wasm32, run in `wasmhost` with no OS: it
//! opens its window when it starts, asks for its first listing when told it
//! started, turns the OS's answers into the window, and queues the file rows a
//! click or the agent asks for as effects inside a transaction. It is skipped
//! when `cargo build -p comp-files-window --target wasm32-unknown-unknown
//! --profile component` has not run (CI runs it first).

use appabi::{Func, Ty, Val};
use comp_files_ui::{LIST, MOVE, NAME, RESTORE, TRASH, UP};
use livecore::Instance;
use livecore::txn::{Effect, Inputs, Storage, Txn};
use uikit::a11y::Action;
use uikit::widget::{UiEvent, Value};
use wasmhost::{AppEvent, CoreInstance, Limits, Update, load_core};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

fn load() -> Option<CoreInstance> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-unknown-unknown/component/comp_files_window.wasm"
    );
    let Ok(wasm) = std::fs::read(path) else {
        eprintln!("skipped: build comp-files-window for wasm32 first");
        return None;
    };
    NEXT.set(1);
    let mut g = load_core(&wasm, LIMITS, None).unwrap();
    // As the host loads it: storage readable, empty.
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    Some(g)
}

thread_local! {
    /// The ticket the next effect gets: the host numbers them across calls.
    static NEXT: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

/// Runs `f` as the host runs a call: in a transaction, its effects numbered on
/// from the last call's.
fn call<R>(g: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> (R, Txn) {
    let first_ticket = NEXT.get();
    let inputs = Inputs {
        first_ticket,
        ..Inputs::default()
    };
    g.begin(&Storage::new(), inputs);
    let out = f(g);
    let txn = g.finish();
    NEXT.set(first_ticket + txn.effects.len() as u32);
    (out, txn)
}

fn effects(txn: &Txn) -> Vec<String> {
    txn.effects
        .iter()
        .map(|e| match e {
            Effect::Call { name, args } => format!("{name} {args}"),
            Effect::Notify(text) => format!("notify {text}"),
        })
        .collect()
}

/// The newest frame of the window the app has open.
fn frame(g: &mut CoreInstance) -> (u32, uikit::Frame) {
    match g.take_ui().pop() {
        Some(Update::Frame { window, frame }) => (window, frame),
        other => panic!("{other:?}"),
    }
}

fn names(f: &uikit::Frame) -> Vec<String> {
    f.a11y.iter().map(|n| n.name.clone()).collect()
}

fn done(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>) -> Vec<String> {
    let outcome = outcome.map(String::from).map_err(String::from);
    let (r, txn) = call(g, |g| {
        g.deliver_event(&AppEvent::EffectDone { ticket, outcome })
    });
    r.unwrap();
    effects(&txn)
}

fn action(name: &str) -> Func {
    Func {
        export: format!("app:files/actions@1.0.0#{name}"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    }
}

#[test]
fn the_files_guest_lists_previews_and_asks_the_os_for_its_changes() {
    let Some(mut g) = load() else { return };

    // Its window opens at once, with nothing in it, and the first listing is
    // asked for when the OS says it started.
    let (w, f) = frame(&mut g);
    assert_eq!(f.title, "Files");
    assert!(names(&f).contains(&String::from("Home")));
    let (r, txn) = call(&mut g, |g| g.deliver_event(&AppEvent::Started));
    r.unwrap();
    assert_eq!(effects(&txn), [r#"fs_list {path: ""}"#]);

    // The listing arrives: folders first, and the count says so.
    assert_eq!(
        done(&mut g, 1, Ok("notes.txt\ndocs/\nphoto.png\n")),
        [""; 0]
    );
    let (_, f) = frame(&mut g);
    let n = names(&f);
    assert!(n.iter().any(|t| t == "docs/") && n.iter().any(|t| t == "notes.txt"));
    assert!(n.iter().any(|t| t == "1 folder, 2 files"), "{n:?}");

    // A click on the second name asks for that file, and its text previews.
    let (r, txn) = call(&mut g, |g| {
        g.ui_act(w, LIST, Action::SetValue, Some(Value::Index(1)))
    });
    assert_eq!(r, Ok(Ok(())));
    assert_eq!(effects(&txn), [r#"fs_read {path: "notes.txt"}"#]);
    done(&mut g, 2, Ok("milk\neggs"));
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from("milk")));

    // Typing a new name and pressing Move asks for fs_move; a refusal shows.
    let (_, txn) = call(&mut g, |g| {
        g.ui_events(
            w,
            &[
                UiEvent::Changed(NAME, Value::Text(String::from("list.txt"))),
                UiEvent::Pressed(MOVE),
            ],
        )
        .unwrap()
    });
    assert_eq!(
        effects(&txn),
        [r#"fs_move {from: "notes.txt", to: "list.txt"}"#]
    );
    done(&mut g, 3, Err("the disk is full"));
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from("the disk is full")));

    // Trash and Restore are the OS's rows too.
    let (_, txn) = call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(TRASH)]).unwrap()
    });
    assert_eq!(effects(&txn), [r#"fs_trash {path: "notes.txt"}"#]);
    assert_eq!(done(&mut g, 4, Ok("")), [r#"fs_list {path: ""}"#]);
    done(&mut g, 5, Ok("docs/\nphoto.png"));
    let (_, txn) = call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(RESTORE)]).unwrap()
    });
    assert_eq!(effects(&txn), [r#"fs_restore {path: "notes.txt"}"#]);

    // The agent's files_open reaches the same model: it opens a folder, and
    // answers at once with what it started.
    let (r, txn) = call(&mut g, |g| {
        Instance::call(g, &action("open"), &[Val::String("docs".into())])
    });
    let said = r.unwrap().unwrap();
    assert_eq!(
        said,
        Val::Result(Ok(Some(Box::new(Val::String("Opening docs.".into())))))
    );
    assert_eq!(effects(&txn), [r#"fs_list {path: "docs"}"#]);
    assert!(
        call(&mut g, |g| Instance::call(
            g,
            &action("select"),
            &[Val::String("nope".into())]
        ))
        .0
        .unwrap()
        .is_some_and(|v| matches!(v, Val::Result(Err(_))))
    );

    // Up, then close: the window goes.
    let (_, txn) = call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(UP)]).unwrap());
    assert!(effects(&txn).len() <= 1);
    g.take_ui();
    let (r, _) = call(&mut g, |g| g.ui_events(w, &[UiEvent::CloseRequested]));
    r.unwrap();
    assert_eq!(g.take_ui(), vec![Update::Closed(w)]);
}

/// The desktop tour's Files window (`03-files`) listed `ideas.txt` alone for
/// a folder of three files: the shell's `fs_list` answer, one name a line, lost
/// all but one line on the way. The built app shows every name it is given, and
/// says "3 files".
#[test]
fn a_folder_of_three_files_shows_all_three_and_says_three_files() {
    let Some(mut g) = load() else { return };
    frame(&mut g);
    call(&mut g, |g| g.deliver_event(&AppEvent::Started))
        .0
        .unwrap();
    done(&mut g, 1, Ok("ideas.txt\nnotes.txt\nplan.txt\n"));
    let (_, f) = frame(&mut g);
    let n = names(&f);
    for name in ["ideas.txt", "notes.txt", "plan.txt"] {
        assert!(n.iter().any(|t| t == name), "{name} is missing: {n:?}");
    }
    assert!(n.iter().any(|t| t == "0 folders, 3 files"), "{n:?}");
}

#[test]
fn it_comes_back_to_the_folder_it_was_showing() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    call(&mut g, |g| g.deliver_event(&AppEvent::Started))
        .0
        .unwrap();
    done(&mut g, 1, Ok("docs/\n"));
    call(&mut g, |g| {
        g.ui_act(w, LIST, Action::SetValue, Some(Value::Index(0)))
    })
    .0
    .unwrap()
    .unwrap();
    done(&mut g, 2, Ok("a.txt"));
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(comp_files_ui::OPEN)])
            .unwrap()
    });
    done(&mut g, 3, Ok("a.txt\nb.txt"));
    // Its checkpoint names the folder; a new instance imports it.
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    let Some(mut h) = load() else { return };
    assert_eq!(h.import(&saved, 1), Ok(0));
    let (_, f) = frame(&mut h);
    assert!(
        names(&f).contains(&String::from("Home / docs")),
        "{:?}",
        names(&f)
    );
    // And it asks for that folder when it starts.
    let (_, txn) = call(&mut h, |h| h.deliver_event(&AppEvent::Started));
    assert_eq!(effects(&txn), [r#"fs_list {path: "docs"}"#]);
    // A state it cannot read is refused, not guessed at.
    assert_eq!(h.import(b"junk", 1), Ok(2));
    assert_eq!(h.import(&saved, 9), Ok(1));
}
