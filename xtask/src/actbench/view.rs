//! Arm `view`: view-then-act (ADR 0005 §6, P3 task 10), which ADR 0005 calls
//! unmeasured. Each request is answered as llm answers `act` on the OS pack, in
//! a desktop of S05-2's seven mock windows with the State line's `app`,
//! `window` and `windows`, and its reads come back as the shell gives them
//! (`shellcore::ui`): a `ui_view` reads that window's view once, and a `ui_act`
//! on a node no view showed reads `desktop`'s refusal ("unknown node; view w3
//! first"), at most two reads in all. A request about a control with no
//! declared action must end in `ui_act` on the right control, action and value
//! of the view it read; one about a whole window (focus, close, open) must end
//! in its row. It reports at [`BAR`]; the gate comes once the rows' wording
//! settles.

use super::{Loaded, Next};
use deskcore::desk::WindowId;
use deskcore::view::{self, Listed, mock};
use infer::chat::Step;
use llmproto::act::{ActRequest, Call, History, Read};
use llmproto::local::act::{self as local_act, Slots};
use serde_json::{Value, json};
use std::cell::Cell;
use std::fs;
use std::path::Path;

/// What a request must come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// `ui_view` on the focused app's window, then `ui_act` on the control the
    /// view shows as `control` (`button "Save"`), with this action and value.
    Act {
        control: &'static str,
        action: &'static str,
        value: Option<&'static str>,
    },
    /// This row at once, on this app's window.
    Row(&'static str, &'static str),
}

use Want::{Act, Row};

const fn act(control: &'static str, action: &'static str) -> Want {
    Act {
        control,
        action,
        value: None,
    }
}

const fn set(control: &'static str, value: &'static str) -> Want {
    Act {
        control,
        action: "set_value",
        value: Some(value),
    }
}

/// (request, the focused app, what it must come to).
pub const TASKS: &[(&str, &str, Want)] = &[
    (
        "click New note",
        "notes",
        act("button \"New note\"", "click"),
    ),
    (
        "search my notes for plumber",
        "notes",
        set("textbox \"Search\"", "plumber"),
    ),
    (
        "delete this note",
        "notes",
        act("button \"Delete note\"", "click"),
    ),
    (
        "turn on reduced motion",
        "settings",
        act("switch \"Reduced motion\"", "click"),
    ),
    (
        "go to the Keyboard page",
        "settings",
        act("\"Keyboard\"", "click"),
    ),
    (
        "open the Downloads folder",
        "files",
        act("\"Downloads\"", "click"),
    ),
    ("go up a folder", "files", act("button \"Up\"", "click")),
    ("save the file", "editor", act("button \"Save\"", "click")),
    (
        "find println in this file",
        "editor",
        set("textbox \"Find\"", "println"),
    ),
    (
        "unload the model",
        "models",
        act("button \"Unload\"", "click"),
    ),
    (
        "switch to the terminal",
        "notes",
        Row("ui_focus", "terminal"),
    ),
    (
        "close the files window",
        "files",
        Row("window_close", "files"),
    ),
    ("open notes", "terminal", Row("ui_open", "notes")),
];
/// Share of [`TASKS`] that should come right; reported, not gated, until the
/// rows' wording settles (P3 plan, risk "Qwen3-4B does view-then-act poorly").
pub const BAR: f64 = 0.8;
/// The time llm gives one call.
const TIMEOUT_MS: u64 = 60_000;

/// The desktop: S05-2's mock windows, `focused` first and focused.
fn windows(focused: &str) -> Vec<Listed> {
    let mut out: Vec<Listed> = mock::apps()
        .iter()
        .enumerate()
        .map(|(n, (app, title, _))| Listed {
            id: WindowId(n as u32 + 1),
            app: (*app).into(),
            title: (*title).into(),
            focused: *app == focused,
        })
        .collect();
    out.sort_by_key(|w| !w.focused);
    out
}

/// The State line for that desktop (ADR 0010 §5).
fn state(windows: &[Listed]) -> String {
    let focused = windows.iter().find(|w| w.focused);
    shellcore::state::State {
        app: focused.map(|w| w.app.clone()),
        window: focused.map(|w| w.title.clone()),
        windows: windows.iter().map(|w| w.app.clone()).collect(),
        ..Default::default()
    }
    .snapshot()
    .unwrap_or_default()
}

/// The view `ui_view` answers for `app`'s window.
fn view_of(windows: &[Listed], app: &str) -> Option<String> {
    let (_, _, tree) = mock::apps().into_iter().find(|(a, _, _)| *a == app)?;
    let id = windows.iter().find(|w| w.app == app)?.id;
    Some(view::project(windows, id, &tree).text)
}

/// The window `text` names, by its app or its id, as `desktop` finds it.
fn find<'a>(windows: &'a [Listed], text: &str) -> Option<&'a Listed> {
    windows
        .iter()
        .find(|w| w.app.eq_ignore_ascii_case(text) || view::parse_window(text) == Some(w.id))
}

/// What one request's calls come to, as the shell takes them.
struct Rounds {
    last: Result<Call, String>,
    /// The view the model last read, if any.
    view: Option<String>,
    got: Vec<String>,
    first_view: bool,
    rounds: usize,
    tokens: usize,
}

/// The shell's reads (module note): at most two, one of them a view.
const MAX_READS: usize = 2;

fn rounds(
    loaded: &Loaded,
    grammar: &local_act::Grammar,
    request: &ActRequest,
    windows: &[Listed],
) -> Rounds {
    let (first, mut rounds, mut tokens) = ask(loaded, grammar, request);
    let mut got = vec![first.as_ref().map_or_else(Clone::clone, Call::text)];
    let first_view = first.as_ref().is_ok_and(|c| c.name == "ui_view");
    let (mut last, mut view, mut reads) = (first, None::<(String, String)>, 0);
    while let Ok(call) = &last {
        let data = match call.name.as_str() {
            "ui_view" if view.is_none() => match find(windows, text_arg(call, "window")) {
                Some(w) => {
                    let text = view_of(windows, &w.app).unwrap_or_default();
                    view = Some((w.app.clone(), text.clone()));
                    text
                }
                None => break,
            },
            "ui_act" => {
                let Some(w) = find(windows, text_arg(call, "window")) else {
                    break;
                };
                let node = format!("[{}]", text_arg(call, "node"));
                let known = view
                    .as_ref()
                    .is_some_and(|(app, text)| *app == w.app && text.contains(&node));
                if known {
                    break;
                }
                format!("unknown node; view {} first", view::window_id(w.id))
            }
            _ => break,
        };
        if reads == MAX_READS {
            break;
        }
        reads += 1;
        let again = ActRequest {
            read: Some(Read {
                call: call.text(),
                data,
            }),
            ..request.clone()
        };
        let (next, r, t) = ask(loaded, grammar, &again);
        (rounds, tokens) = (rounds + r, tokens + t);
        got.push(next.as_ref().map_or_else(Clone::clone, Call::text));
        last = next;
    }
    Rounds {
        last,
        view: view.map(|(_, text)| text),
        got,
        first_view,
        rounds,
        tokens,
    }
}

/// Whether `text` names `app`'s window: the app's name or its id.
fn names_window(windows: &[Listed], app: &str, text: &str) -> bool {
    text.eq_ignore_ascii_case(app)
        || windows
            .iter()
            .any(|w| w.app == app && view::parse_window(text) == Some(w.id))
}

fn text_arg<'a>(call: &'a Call, name: &str) -> &'a str {
    call.arguments
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The short id a view gives `control`: the `xx` of `[xx] control`.
fn short_id(view: &str, control: &str) -> Option<String> {
    let at = view.find(&format!("] {control}"))?;
    let open = view[..at].rfind('[')?;
    Some(view[open + 1..at].to_string())
}

/// Whether round two's `call` acts as `want` asks on `view`.
fn acts(call: &Call, view: &str, control: &str, action: &str, value: Option<&str>) -> bool {
    let value_ok = match value {
        Some(want) => text_arg(call, "value").eq_ignore_ascii_case(want),
        None => true,
    };
    call.name == "ui_act"
        && short_id(view, control).as_deref() == Some(text_arg(call, "node"))
        && text_arg(call, "action") == action
        && value_ok
}

/// One decode round's call, with its decode rounds and computed tokens.
fn ask(
    loaded: &Loaded,
    grammar: &local_act::Grammar,
    request: &ActRequest,
) -> (Result<Call, String>, usize, usize) {
    let (rounds, tokens) = (Cell::new(0), Cell::new(0));
    let call = local_act::routed(
        &mut loaded.engine.borrow_mut(),
        grammar,
        request,
        &History::new(),
        Slots::ONE,
        |_| true,
        |step| {
            match step {
                Step::Cached(_) => rounds.set(rounds.get() + 1),
                Step::Prompt | Step::Token(_) => tokens.set(tokens.get() + 1),
            }
            Next::Continue
        },
    );
    (call.map_err(|e| e.describe()), rounds.get(), tokens.get())
}

/// The arm at `seed`: whether it meets [`BAR`], and its Markdown rows.
pub fn run(loaded: &Loaded, seed: u64, target_dir: &Path) -> Result<(bool, String), String> {
    let grammar = local_act::grammar().map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    let (mut passed, mut first_ok, mut views) = (0, 0, 0);
    for &(line, app, want) in TASKS {
        let windows = windows(app);
        let request = ActRequest {
            line: line.into(),
            state: state(&windows),
            timeout_ms: TIMEOUT_MS,
            read: None,
        };
        let started = std::time::Instant::now();
        let Rounds {
            last,
            view,
            got,
            first_view,
            rounds,
            tokens,
        } = rounds(loaded, &grammar, &request, &windows);
        views += usize::from(view.is_some());
        let first_right = match want {
            Row(row, _) => got[0].contains(&format!("\"name\":\"{row}\"")),
            Act { .. } => first_view,
        };
        first_ok += usize::from(first_right);
        let ok = match (want, &last, &view) {
            (Row(row, target), Ok(call), _) => {
                let named = match row {
                    "ui_open" => text_arg(call, "app"),
                    _ => text_arg(call, "window"),
                };
                call.name == row && names_window(&windows, target, named)
            }
            (
                Act {
                    control,
                    action,
                    value,
                },
                Ok(call),
                Some(view),
            ) => {
                let viewed = find(&windows, text_arg(call, "window")).is_some_and(|w| w.app == app);
                viewed && acts(call, view, control, action, value)
            }
            _ => false,
        };
        passed += usize::from(ok);
        println!(
            "  {} [{app}] {line} -> {} ({} ms, {rounds} rounds, {tokens} tokens)",
            if ok { "ok  " } else { "FAIL" },
            got.join(" then "),
            started.elapsed().as_millis()
        );
        records.push(json!({
            "line": line, "app": app, "want": format!("{want:?}"), "got": got,
            "passed": ok, "rounds": rounds, "tokens": tokens,
        }));
    }
    let need = (TASKS.len() as f64 * BAR).ceil() as usize;
    let passes = passed >= need;
    println!(
        "xtask: actbench view seed {seed}: {} {passed}/{} requests (bar {need}); \
         first round right {first_ok}/{}; {views} views read",
        if passes { "meets" } else { "BELOW" },
        TASKS.len(),
        TASKS.len()
    );
    let dir = target_dir.join("actbench");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("view-s{seed}.jsonl"));
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    let mark = if passes { "yes" } else { "**no**" };
    let rows = format!(
        "\n### view seed {seed}\n\n| Bar | Value | Meets |\n|---|---|---|\n\
         | View-then-act and window requests (>= {need}/{}, reported) | {passed}/{} | {mark} |\n\
         | First round right | {first_ok}/{} | |\n",
        TASKS.len(),
        TASKS.len(),
        TASKS.len()
    );
    // Reported, not gated (see [`BAR`]).
    Ok((true, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(json: &str) -> Call {
        llmproto::act::parse(json).unwrap()
    }

    #[test]
    fn every_control_a_task_names_is_in_its_apps_view() {
        for (line, app, want) in TASKS {
            let windows = windows(app);
            match want {
                Act { control, .. } => {
                    let view = view_of(&windows, app).unwrap();
                    assert!(short_id(&view, control).is_some(), "{line}: {view}");
                }
                Row(row, target) => {
                    assert!(llmproto::pack::shows(
                        &llmproto::pack::render(llmproto::pack::Format::Dsl1),
                        row
                    ));
                    assert!(windows.iter().any(|w| w.app == *target), "{line}");
                }
            }
        }
    }

    #[test]
    fn the_state_line_names_the_focused_app_and_every_window() {
        let state = state(&windows("notes"));
        assert!(state.starts_with("#{app: \"notes\", window: \"Notes\", windows: [\"notes\""));
        assert!(state.len() <= llmproto::act::STATE_MAX_BYTES);
    }

    #[test]
    fn round_two_is_judged_by_the_views_ids() {
        let windows = windows("notes");
        let view = view_of(&windows, "notes").unwrap();
        let id = short_id(&view, "button \"New note\"").unwrap();
        let good = call(&format!(
            r#"{{"name":"ui_act","arguments":{{"window":"notes","node":"{id}","action":"click","value":null}}}}"#
        ));
        assert!(acts(&good, &view, "button \"New note\"", "click", None));
        let wrong = call(
            r#"{"name":"ui_act","arguments":{"window":"notes","node":"b9","action":"click","value":null}}"#,
        );
        assert!(!acts(&wrong, &view, "button \"New note\"", "click", None));
        assert!(names_window(&windows, "notes", "notes"));
        assert!(names_window(&windows, "notes", "w3"));
        assert!(!names_window(&windows, "notes", "w1"));
    }
}
