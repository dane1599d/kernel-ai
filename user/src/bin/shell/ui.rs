//! The agent's window rows on the OS (P3 task 10): `Host::ui` asks `desktop`
//! over the shell's route to it (`deskcore::agent`'s requests), and opens an
//! app through the supervisor's `app-call`, which starts an app that is not
//! running; the app opens its window when it starts. The State line's `app`,
//! `window` and `windows` come from `desktop`'s window list.

use super::{ask_desktop, desktop, supervisor};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use deskcore::agent::Request;
use shellcore::state::State;
use shellcore::ui::{self as rows, UiCall, Window};
use user::sys;

/// The supervisor starts an app that is down and calls it again once it serves,
/// within 10 s; this leaves it a margin.
const APP_CALL_MS: u64 = 15_000;
/// How long an app started for `ui_open` has to show its window, and how often
/// the list is read meanwhile. Under TCG a component's first frame takes
/// seconds.
const WINDOW_WAIT_MS: u64 = 20_000;
const WINDOW_POLL_MS: u64 = 250;

/// One window row: `desktop`'s answer after its `ok`, or why not.
pub fn call(call: &UiCall) -> Result<String, String> {
    let route = desktop().ok_or_else(|| String::from("there is no desktop here"))?;
    let request = match call {
        UiCall::Open { app, arg } => return open(route, app, arg.as_deref()),
        UiCall::Windows => Request::Windows,
        UiCall::View(w) => Request::View(w.clone()),
        UiCall::Focus(w) => Request::Focus(w.clone()),
        UiCall::Refocus(w) => Request::Refocus(w.clone()),
        UiCall::Close(w) => Request::Close(w.clone()),
        UiCall::Check {
            window,
            node,
            action,
        } => Request::Check {
            window: window.clone(),
            node: node.clone(),
            action: *action,
        },
        UiCall::Act {
            window,
            node,
            action,
            value,
        } => Request::Act {
            window: window.clone(),
            node: node.clone(),
            action: *action,
            value: value.clone(),
        },
    };
    let answer = ask_desktop(route, &request.encode())?;
    match (answer.strip_prefix("err "), request) {
        (Some(why), _) => Err(why.to_string()),
        // The list has no `ok`: one line a window.
        (None, Request::Windows) => Ok(answer),
        (None, _) => Ok(answer
            .strip_prefix("ok")
            .map_or(answer.as_str(), str::trim_start)
            .to_string()),
    }
}

/// `ui_open`: `focused w3 w1` when the app has a window already (it takes
/// focus, from `w1`), or `opened w5` once the app, started if it was not
/// running, shows its window.
fn open(route: kabi::handle::Handle, app: &str, arg: Option<&str>) -> Result<String, String> {
    if arg.is_some_and(|arg| !arg.is_empty()) {
        return Err(format!(
            "{app} cannot be opened with an argument yet; open it, then act in it"
        ));
    }
    let mine = |list: &[Window]| {
        list.iter()
            .find(|w| w.app.eq_ignore_ascii_case(app))
            .map(|w| w.id.clone())
    };
    if let Some(id) = mine(&windows(route)?) {
        let before = call(&UiCall::Focus(id.clone()))?;
        return Ok(format!("focused {id} {before}"));
    }
    app_call(app, "kai-actions 1")?;
    let mut waited = 0;
    while waited < WINDOW_WAIT_MS {
        if let Some(id) = mine(&windows(route)?) {
            return Ok(format!("opened {id}"));
        }
        sys::sleep_ms(WINDOW_POLL_MS);
        waited += WINDOW_POLL_MS;
    }
    Err(format!("{app} started but showed no window"))
}

fn windows(route: kabi::handle::Handle) -> Result<alloc::vec::Vec<Window>, String> {
    let list = ask_desktop(route, &Request::Windows.encode())?;
    rows::parse_windows(&list)
}

/// The supervisor's `app-call`: `kai-actions 1` is answered by the app's host
/// from what it loaded, never by its code, and starts the app if it is down.
fn app_call(app: &str, request: &str) -> Result<String, String> {
    let text = format!("app-call {app}\n{request}");
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(supervisor(), text.as_bytes(), &mut reply, APP_CALL_MS)
        .map_err(|error| format!("the supervisor did not answer ({error:?})"))?;
    let answer = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
    match answer.strip_prefix("err ") {
        Some(why) => Err(format!("{app} could not be opened: {why}")),
        None => Ok(answer),
    }
}

/// The State line with the open windows, or as it was when there is no
/// desktop.
pub fn state(base: State) -> State {
    match desktop().map(windows) {
        Some(Ok(list)) => rows::state_of(&list, base),
        _ => base,
    }
}

/// A new request: the outline on the window the last one acted in goes, and
/// with it a takeover's stop, which held for the rest of that run.
pub fn begin() {
    if let Some(route) = desktop() {
        let _ = ask_desktop(route, &Request::Agent(None).encode());
    }
}
