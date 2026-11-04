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

/// The host refuses a read-only row that changes the checkpoint, and `files_open`
/// and `files_reveal` are read-only rows that move the folder shown, so the folder
/// is not in the checkpoint: "open my Documents folder" would be refused otherwise.
#[test]
fn the_folder_shown_is_not_state_so_opening_one_changes_nothing_and_a_restart_starts_at_home() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    call(&mut g, |g| g.deliver_event(&AppEvent::Started))
        .0
        .unwrap();
    done(
        &mut g,
        1,
        Ok("docs/
notes.txt
"),
    );
    let before = Instance::export(&mut g, 0).unwrap().unwrap();

    // The agent's files_open moves the window to another folder.
    let (r, txn) = call(&mut g, |g| {
        Instance::call(g, &action("open"), &[Val::String("docs".into())])
    });
    assert!(matches!(r.unwrap().unwrap(), Val::Result(Ok(_))));
    assert_eq!(effects(&txn), [r#"fs_list {path: "docs"}"#]);
    assert_eq!(Instance::export(&mut g, 0).unwrap().unwrap(), before);
    done(
        &mut g,
        2,
        Ok("a.txt
b.txt"),
    );
    let (_, f) = frame(&mut g);
    assert!(
        names(&f).contains(&String::from("Home / docs")),
        "{:?}",
        names(&f)
    );
    assert_eq!(Instance::export(&mut g, 0).unwrap().unwrap(), before);

    // files_reveal goes back to the home folder with a name selected.
    let (r, txn) = call(&mut g, |g| {
        Instance::call(g, &action("reveal"), &[Val::String("notes.txt".into())])
    });
    assert!(matches!(r.unwrap().unwrap(), Val::Result(Ok(_))));
    assert_eq!(effects(&txn), [r#"fs_list {path: ""}"#]);
    assert_eq!(Instance::export(&mut g, 0).unwrap().unwrap(), before);
    done(
        &mut g,
        3,
        Ok("docs/
notes.txt
"),
    );
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from("Home")), "{:?}", names(&f));
    assert_eq!(Instance::export(&mut g, 0).unwrap().unwrap(), before);

    // A person's click on the folder and Open moves it too, and changes nothing.
    // (The reveal's own preview read is ticket 4, unanswered.)
    call(&mut g, |g| {
        g.ui_act(w, LIST, Action::SetValue, Some(Value::Index(0)))
    })
    .0
    .unwrap()
    .unwrap();
    done(&mut g, 5, Ok("a.txt"));
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(comp_files_ui::OPEN)])
            .unwrap()
    });
    done(
        &mut g,
        6,
        Ok("a.txt
b.txt"),
    );
    let (_, f) = frame(&mut g);
    assert!(
        names(&f).contains(&String::from("Home / docs")),
        "{:?}",
        names(&f)
    );
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    assert_eq!(saved, before);

    // A restart starts at home, and asks for the home folder when it starts.
    let Some(mut h) = load() else { return };
    assert_eq!(h.import(&saved, 1), Ok(0));
    let (_, f) = frame(&mut h);
    assert!(names(&f).contains(&String::from("Home")), "{:?}", names(&f));
    let (_, txn) = call(&mut h, |h| h.deliver_event(&AppEvent::Started));
    assert_eq!(effects(&txn), [r#"fs_list {path: ""}"#]);
    // A state it cannot read and another schema are refused, not guessed at.
    assert_eq!(h.import(b"junk", 1), Ok(2));
    assert_eq!(h.import(&saved, 9), Ok(1));
    // The state the last version saved, with a folder in it, still imports (a
    // live update from it): the folder is dropped and the window starts at home.
    #[derive(minicbor::Encode)]
    struct Old {
        #[n(0)]
        dir: String,
    }
    let old = compsdk::state::encode(
        1,
        &Old {
            dir: String::from("docs"),
        },
    );
    let Some(mut j) = load() else { return };
    assert_eq!(j.import(&old, 1), Ok(0));
    let (_, f) = frame(&mut j);
    assert!(names(&f).contains(&String::from("Home")), "{:?}", names(&f));
    let (_, txn) = call(&mut j, |j| j.deliver_event(&AppEvent::Started));
    assert_eq!(effects(&txn), [r#"fs_list {path: ""}"#]);
}

/// The OS tells the app its files changed (the AI, the Terminal or an undo made,
/// moved or trashed one) with an `effect-done` under the notice ticket, and the
/// window lists the folder it shows again: the new file appears with no one
/// asking for it. A notice is no answer to an effect, so any other text, or a
/// refusal under its ticket, does nothing.
#[test]
fn a_notice_that_the_files_changed_lists_the_folder_shown_again() {
    use compsdk::app::{NOTICE_FILES, NOTICE_TICKET};
    let Some(mut g) = load() else { return };
    frame(&mut g);
    call(&mut g, |g| g.deliver_event(&AppEvent::Started))
        .0
        .unwrap();
    done(&mut g, 1, Ok("docs/\nnotes.txt\n"));
    let (_, f) = frame(&mut g);
    assert!(!names(&f).iter().any(|t| t == "plan.txt"));

    // The home folder is listed again, and the new file shows.
    assert_eq!(
        done(&mut g, NOTICE_TICKET, Ok(NOTICE_FILES)),
        [r#"fs_list {path: ""}"#]
    );
    done(&mut g, 2, Ok("docs/\nnotes.txt\nplan.txt\n"));
    let (_, f) = frame(&mut g);
    assert!(names(&f).iter().any(|t| t == "plan.txt"), "{:?}", names(&f));

    // Not the notice it knows, and a refusal under its ticket: nothing is asked.
    assert!(done(&mut g, NOTICE_TICKET, Ok("something else")).is_empty());
    assert!(done(&mut g, NOTICE_TICKET, Err("the effect is too long")).is_empty());

    // In another folder, that folder is listed again, not home.
    let (r, txn) = call(&mut g, |g| {
        Instance::call(g, &action("open"), &[Val::String("docs".into())])
    });
    assert!(matches!(r.unwrap().unwrap(), Val::Result(Ok(_))));
    assert_eq!(effects(&txn), [r#"fs_list {path: "docs"}"#]);
    done(&mut g, 3, Ok("a.txt\n"));
    assert_eq!(
        done(&mut g, NOTICE_TICKET, Ok(NOTICE_FILES)),
        [r#"fs_list {path: "docs"}"#]
    );
}
