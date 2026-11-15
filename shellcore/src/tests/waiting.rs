//! What the apps ask while the model is asked (the founder's test 2, item 7):
//! a read is answered at once, through the same door as between lines, and a
//! change waits for the line.

use super::*;
use crate::door::DOOR_APP;
use crate::effects::EffectDone;
use crate::waiting::{Served, Waiting};
use alloc::boxed::Box;
use alloc::vec;

const LIST: &str = r#"effect 1 call({name: "fs_list", args: "{path: \"\"}"})"#;
const MOVE: &str =
    r#"effect 2 call({name: "fs_move", args: "{from: \"notes.txt\", to: \"plan.txt\"}"})"#;
const SHELL_LINE: &str = r#"effect 3 call({name: "shell_line", args: "{line: \"cpus\"}"})"#;

/// Asks `requests` of `serve` while the line's `act` is in flight, keeping
/// what each came to.
fn serving(host: &Rc<FakeHost>, requests: Vec<Asked>) -> Rc<RefCell<Vec<Option<Served>>>> {
    let served = Rc::new(RefCell::new(Vec::new()));
    let kept = Rc::clone(&served);
    *host.serving.borrow_mut() = Some(Box::new(move |serve| {
        for asked in &requests {
            let settings: Vec<(&str, bool)> = match asked {
                Asked::Settings(texts) => texts.iter().map(|t| (t.as_str(), true)).collect(),
                _ => Vec::new(),
            };
            let waiting = match asked {
                Asked::Effects(app, lines) => Waiting::Effects {
                    app,
                    lines: lines.as_str(),
                    user: true,
                },
                Asked::Read(path) => Waiting::FileRead {
                    app: DOOR_APP,
                    path,
                },
                Asked::Settings(_) => Waiting::Settings {
                    app: crate::SETTINGS_APP,
                    requests: &settings,
                },
            };
            kept.borrow_mut().push(serve(waiting));
        }
    }));
    served
}

enum Asked {
    Effects(&'static str, String),
    Read(&'static str),
    Settings(Vec<String>),
}

fn cpus_call() -> Result<Call, ActError> {
    Ok(llmproto::act::parse(r#"{"name":"cpus","arguments":{}}"#).unwrap())
}

#[test]
fn an_apps_reads_are_answered_while_the_model_is_asked() {
    let host = super::effects::host();
    host.files
        .borrow_mut()
        .insert("notes.txt".into(), "milk".into());
    host.calls.borrow_mut().push_back(cpus_call());
    let served = serving(
        &host,
        vec![
            Asked::Effects("files", String::from(LIST)),
            Asked::Read("/home/user/notes.txt"),
            Asked::Settings(vec![String::from("about")]),
        ],
    );
    let mut shell = shell(&host);
    // The line goes on as if nothing had been asked meanwhile.
    assert_eq!(shell.eval("how many cpus are there"), ok("4"));
    let served = served.borrow();
    assert_eq!(
        served[0],
        Some(Served::Effects(vec![EffectDone {
            ticket: 1,
            outcome: Ok(String::from("notes.txt\n")),
        }]))
    );
    assert_eq!(served[1], Some(Served::FileRead(Ok(b"milk".to_vec()))));
    assert!(
        matches!(&served[2], Some(Served::Settings(answers)) if answers.len() == 1 && answers[0].is_ok()),
        "{:?}",
        served[2]
    );
}

#[test]
fn a_change_waits_for_the_line_and_nothing_of_it_runs() {
    let host = super::effects::host();
    host.files
        .borrow_mut()
        .insert("notes.txt".into(), "milk".into());
    host.calls.borrow_mut().push_back(cpus_call());
    let served = serving(
        &host,
        vec![
            // A batch with one change in it waits whole, its read too.
            Asked::Effects("files", [LIST, MOVE].join("\n")),
            Asked::Effects("terminal-window", String::from(SHELL_LINE)),
            Asked::Settings(vec![
                String::from("about"),
                String::from("keep-running on files"),
            ]),
        ],
    );
    let mut shell = shell(&host);
    assert_eq!(shell.eval("how many cpus are there"), ok("4"));
    assert_eq!(*served.borrow(), [None, None, None]);
    assert_eq!(host.files.borrow()["notes.txt"], "milk");
    assert!(!host.files.borrow().contains_key("plan.txt"));
    assert!(host.kept.borrow().is_empty());
}

#[test]
fn a_host_that_cannot_wait_off_its_thread_just_asks() {
    struct Plain;
    impl crate::Host for Plain {
        fn now_ns(&self) -> u64 {
            0
        }
        fn interrupted(&self) -> bool {
            false
        }
        fn heap_bytes(&self) -> u64 {
            0
        }
        fn metrics(&self) -> Result<kabi::Metrics, kabi::Error> {
            Err(kabi::Error::Unavailable)
        }
        fn set_policy(&self, _id: u64) -> Result<(), kabi::Error> {
            Ok(())
        }
        fn ask(&self, _prompt: &str) -> Result<String, String> {
            Err(String::new())
        }
        fn script(&self, _request: &ScriptRequest) -> Result<Answer, AnswerError> {
            Err(AnswerError::Unavailable(String::new()))
        }
        fn act(&self, _request: &ActRequest) -> Result<Call, ActError> {
            cpus_call()
        }
        fn audit(&self, _text: &str) {}
    }
    let request = ActRequest {
        line: String::from("cpus"),
        state: String::new(),
        timeout_ms: 1,
        read: None,
        apps: String::new(),
    };
    let mut asked = 0;
    let call = Plain.act_serving(&request, &mut |_| {
        asked += 1;
        None
    });
    assert_eq!(call, cpus_call());
    assert_eq!(asked, 0);
}

/// An app's own change row waits like an OS change, and so do a name no row
/// has and a line that does not parse; an app's read-only row is a read (the
/// agent core then refuses it as an effect, which changes nothing).
#[test]
fn an_app_rows_change_and_an_unknown_name_wait() {
    let host = super::effects::host();
    host.apps
        .borrow_mut()
        .rows
        .load("notes", super::apps::NOTES)
        .unwrap();
    let mut shell = shell(&host);
    let effects = |shell: &mut Shell, lines: &str| {
        shell.serve_waiting(Waiting::Effects {
            app: "notes",
            lines,
            user: true,
        })
    };
    let add = r#"effect 1 call({name: "notes_add", args: "{text: \"milk\"}"})"#;
    let clear = r#"effect 2 call({name: "notes_clear", args: "{}"})"#;
    let unknown = r#"effect 3 call({name: "no_such_row", args: "{}"})"#;
    let malformed = "effect 4 call(not wave)";
    for lines in [add, clear, unknown, malformed] {
        assert_eq!(effects(&mut shell, lines), None, "{lines}");
    }
    let open = r#"effect 5 call({name: "files_open", args: "{path: \"\"}"})"#;
    assert!(matches!(
        effects(&mut shell, open),
        Some(Served::Effects(done)) if done.len() == 1 && done[0].outcome.is_err()
    ));
    assert!(host.app_requests.borrow().is_empty());
}

/// A read never sees nor leaves the line's own state: the question it is
/// clarifying, its pending call, an undo waiting for a yes, the menu's ONCE
/// and the panel's marks are the line's before and after it.
#[test]
fn a_read_leaves_the_lines_own_state_as_it_was() {
    let host = super::effects::host();
    let mut shell = shell(&host);
    shell.clarifying = Some(String::from("which file?"));
    shell.undo_asking = Some(7);
    shell.owner_once = Some(9);
    shell.asked_model = true;
    shell.worded = true;
    let served = shell.serve_waiting(Waiting::Effects {
        app: "files",
        lines: LIST,
        user: true,
    });
    assert!(matches!(served, Some(Served::Effects(_))));
    assert_eq!(shell.clarifying.as_deref(), Some("which file?"));
    assert_eq!(shell.undo_asking, Some(7));
    assert_eq!(shell.owner_once, Some(9));
    assert!(shell.asked_model && shell.worded);
    assert!(shell.pending.is_none());
    // And a line with none of it has none after a read.
    let mut shell = super::shell(&host);
    let _ = shell.serve_waiting(Waiting::Settings {
        app: crate::SETTINGS_APP,
        requests: &[("about", false)],
    });
    assert!(shell.clarifying.is_none() && shell.undo_asking.is_none());
    assert!(!shell.asked_model && !shell.worded);
}

mod held {
    use crate::waiting::{Held, Kind};

    /// Held requests run in the order they came, and the one running still
    /// holds back its app's later requests until it is done.
    #[test]
    fn they_run_in_order_and_the_running_one_still_holds_its_app_back() {
        let mut held: Held<&str> = Held::default();
        assert!(held.is_empty() && !held.blocks("files"));
        held.hold(Kind::Effects, "files", "move");
        held.hold(Kind::File, "editor", "save");
        // Per app, whatever the kind: Files' read waits, the Terminal's not.
        assert!(held.blocks("files") && held.blocks("editor"));
        assert!(!held.blocks("terminal-window"));
        assert_eq!(held.start(), Some("move"));
        assert!(held.blocks("files"), "running, so not done");
        held.done();
        assert!(!held.blocks("files") && held.blocks("editor"));
        assert_eq!(held.start(), Some("save"));
        held.done();
        assert_eq!(held.start(), None);
        assert!(held.is_empty());
    }

    /// A request the host runs from elsewhere holds its app back as a held
    /// one does, and an answer still to be given holds back only its own app.
    #[test]
    fn a_batch_run_from_elsewhere_and_an_answer_to_give_hold_back_their_app() {
        let mut held: Held<&str> = Held::default();
        held.running(Kind::Effects, "terminal-window");
        assert!(held.blocks("terminal-window") && !held.blocks("files"));
        held.done();
        assert!(held.is_empty());
        held.hold(Kind::Answer, "files", "listing");
        assert!(held.blocks("files") && !held.blocks("editor"));
    }

    /// A held Settings change holds every app's requests back, until it has
    /// run.
    #[test]
    fn a_held_settings_change_holds_every_app_back() {
        let mut held: Held<&str> = Held::default();
        held.hold(Kind::Settings, "settings-window", "revoke");
        assert!(held.blocks_all());
        assert!(held.blocks("files") && held.blocks("editor"));
        assert_eq!(held.start(), Some("revoke"));
        assert!(held.blocks_all() && held.blocks("files"), "running");
        held.done();
        assert!(!held.blocks_all() && !held.blocks("files"));
    }

    /// #890: the apps a take passes by are those held back, running or held,
    /// each once; another app's are not.
    #[test]
    fn the_apps_held_back_are_named_once_for_a_take_to_pass_by() {
        let mut held: Held<&str> = Held::default();
        assert!(held.apps().is_empty());
        held.running(Kind::Effects, "terminal-window");
        held.hold(Kind::Effects, "files", "move");
        held.hold(Kind::Answer, "files", "listing");
        held.hold(Kind::File, "editor", "save");
        assert_eq!(held.apps(), ["terminal-window", "files", "editor"]);
        assert!(!held.blocks_all() && !held.blocks("notes"));
        held.done();
        assert_eq!(held.start(), Some("move"));
        assert_eq!(held.apps(), ["files", "editor"]);
    }
}

/// #938's review: a give-back `desktop` may have applied is never run from
/// the host's own copy. Sent again after the line and taken again, each kind
/// runs once, whether `desktop` applied the first and answered late or never
/// had it.
mod given_back {
    use crate::waiting::{Came, GaveBack, Keep, Step, Took, restarted};
    use alloc::string::String;
    use deskcore::{effects, files, settings_wire as wire};

    const BUDGET: usize = 4096;

    #[test]
    fn only_a_give_back_desktop_surely_did_not_take_keeps_a_copy() {
        assert_eq!(GaveBack::Kept.keep(1), Keep::Back { resend: false });
        assert_eq!(GaveBack::Unknown.keep(1), Keep::Back { resend: true });
        assert_eq!(GaveBack::Refused.keep(1), Keep::Own(1));
        assert_eq!(GaveBack::Withdrawn.keep(1), Keep::Own(1));
        assert_eq!(GaveBack::NotSent.keep(1), Keep::Own(1));
    }

    /// #938's re-review: a reply, or an error raised before the call was
    /// queued, says what `desktop` has; any other error may come after it
    /// took the call (a late reply is dropped), so it is not the shell's.
    #[test]
    fn a_give_back_calls_reply_or_error_says_where_the_request_is() {
        use kabi::Error;
        let table: [(Result<&[u8], Error>, GaveBack); 11] = [
            (Ok(b"ok"), GaveBack::Kept),
            (
                Ok(b"err that transfer is not one the shell may give back"),
                GaveBack::Refused,
            ),
            (Ok(b""), GaveBack::Refused),
            (Err(Error::Interrupted), GaveBack::Withdrawn),
            (Err(Error::Busy), GaveBack::NotSent),
            (Err(Error::NoMemory), GaveBack::NotSent),
            (Err(Error::TooBig), GaveBack::NotSent),
            (Err(Error::Unavailable), GaveBack::NotSent),
            (Err(Error::TimedOut), GaveBack::Unknown),
            (Err(Error::Dead), GaveBack::Unknown),
            (Err(Error::Invalid), GaveBack::Unknown),
        ];
        for (call, gave) in table {
            assert_eq!(GaveBack::of(call), gave, "{call:?}");
        }
    }

    /// #938's re-review: after the line, a request given back is told it did
    /// not run whenever it may be lost, and is never taken behind a resend
    /// that got no answer.
    #[test]
    fn after_the_line_a_given_back_request_runs_or_its_app_is_told() {
        let ended = |restarted, resend| Came::LineEnded { restarted, resend };
        let table = [
            (ended(true, false), Step::TellLost),
            (ended(true, true), Step::TellLost),
            (ended(false, true), Step::Resend),
            (ended(false, false), Step::Take),
            (Came::Resent(GaveBack::Kept), Step::Take),
            // A transfer waiting again refuses the second give-back.
            (Came::Resent(GaveBack::Refused), Step::Take),
            (Came::Resent(GaveBack::Unknown), Step::TellLost),
            (Came::Resent(GaveBack::NotSent), Step::TellLost),
            (Came::Resent(GaveBack::Withdrawn), Step::TellLost),
            (Came::Taken(Took::Ran), Step::Done),
            (Came::Taken(Took::Empty), Step::TellLost),
            // #938's third look: a failed take call says nothing of what
            // waits, so the app is not told it did not run.
            (Came::Taken(Took::CallFailed), Step::Leave),
        ];
        for (came, step) in table {
            assert_eq!(came.next(), step, "{came:?}");
        }
        // An unknown pid is no restart: the take that finds nothing tells.
        assert!(restarted(Some(4), Some(9)));
        assert!(!restarted(Some(4), Some(4)));
        assert!(!restarted(None, Some(9)) && !restarted(Some(4), None));
    }

    #[test]
    fn effects_given_back_twice_run_once() {
        for applied in [true, false] {
            let mut desk = effects::Queue::default();
            desk.put_as("files", "effect 1 notify(\"rm\")", true);
            let taken = desk.take_during(BUDGET).unwrap();
            let back = effects::encode_back(&taken);
            if applied {
                assert_eq!(desk.put_back(effects::parse_back(&back).unwrap()), 1);
            }
            // The call timed out: the host keeps no copy, and resends.
            assert_eq!(
                GaveBack::Unknown.keep(taken.clone()),
                Keep::Back { resend: true }
            );
            let again = desk.put_back(effects::parse_back(&back).unwrap());
            assert_eq!(again, usize::from(!applied), "applied {applied}");
            assert_eq!(desk.take_app(BUDGET, "files"), Some(taken));
            assert_eq!(desk.take_app(BUDGET, "files"), None, "once");
        }
    }

    #[test]
    fn settings_given_back_twice_run_once() {
        for applied in [true, false] {
            let mut desk = wire::Queue::default();
            desk.put(wire::SETTINGS_APP, "2\t1\trules-remove 2")
                .unwrap();
            let taken = desk.take(BUDGET).unwrap();
            let back = wire::encode_back_for(&wire::SETTINGS, &taken);
            let parsed = || wire::parse_back_for(&wire::SETTINGS, &back).unwrap();
            if applied {
                assert_eq!(desk.put_back(parsed()), 1);
            }
            assert_eq!(desk.put_back(parsed()), usize::from(!applied));
            assert_eq!(desk.take(BUDGET), Some(taken));
            assert_eq!(desk.take(BUDGET), None, "once");
        }
    }

    #[test]
    fn a_file_given_back_twice_is_done_once() {
        let ask = |desk: &mut files::Files, caller: &str, text: &str| {
            let reply = desk.request(caller, text.as_bytes(), 0).unwrap();
            String::from_utf8(reply.bytes).unwrap()
        };
        for applied in [true, false] {
            let mut desk = files::Files::default();
            let start = "files start 1 read /home/user/a.txt";
            assert_eq!(ask(&mut desk, files::DOOR_APP, start), "ok");
            let job = ask(&mut desk, files::SHELL, "files take");
            assert!(job.starts_with("job 0 editor read"), "{job}");
            if applied {
                assert_eq!(ask(&mut desk, files::SHELL, "files back 0"), "ok");
            }
            // Sent again: refused once it waits again, which is no harm.
            let again = ask(&mut desk, files::SHELL, "files back 0");
            assert_eq!(again.starts_with("err"), applied, "{again}");
            assert_eq!(ask(&mut desk, files::SHELL, "files take"), job);
            assert_eq!(ask(&mut desk, files::SHELL, "files take"), "none");
        }
    }
}
