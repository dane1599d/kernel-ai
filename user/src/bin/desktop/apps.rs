//! Start's host half (Kai Desk PR 8; the Apps sheet's before it, P4 task
//! 3): Start opens with what `pkg` lists in `kai-catalog 1`, asked for page
//! by page through the `uses=pkg` route and read with
//! `kaipkg::catalog::parse`, which is strict: an answer it does not take
//! shows no row of it. With no rows, the image's own apps. Start takes the
//! panel's keys while it is open. An app sends the shell the line a person
//! would type, `install <name>` or `open <name>`, by the row's own name, so
//! it runs with the same consent as a typed one; the shell opens `open
//! <name>` itself, with no model. Ask Kai sends the typed text, and only
//! that: rows are data, never queued to the model. Esc, a press outside it,
//! a card or the focus leaving it closes Start.
//!
//! `desktop` calls `pkg`, never the other way, and only when Start opens.

use super::{State, deliver};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use deskcore::catalog::{App, Source};
use deskcore::geom::Point;
use deskcore::start::{Pick, Start};
use kaipkg::catalog::{self, Place, Row};
use user::{println, sys};

/// How long `pkg` has to answer one page: `desktop` waits on it, so not long.
const CATALOG_MS: u64 = 2_000;

/// The route to `pkg`, the first in `uses=pkg`; none without one.
fn pkg() -> Option<kabi::handle::Handle> {
    user::route(0)
}

/// The image's own apps, every image's: what `ui_open` names each by, and
/// what the sheet says of it. `pkg` lists them as installed when it runs;
/// when it gives no rows (no `pkg`, or it failed) the sheet lists these, so it
/// is never empty. The Editor and Models come only with a data disk, so they
/// are not listed.
const BUILT_IN: [(&str, &str); 5] = [
    ("notes", "Notes: short notes, kept"),
    ("files", "Files: your files and folders"),
    ("terminal", "Terminal: type commands, or ask Kai"),
    ("settings", "Settings: look, rules, grants, the network"),
    ("monitor", "Monitor: what runs, and the memory it uses"),
];

/// A built-in app's row.
fn built_in((name, summary): (&str, &str)) -> App {
    App {
        source: Source::BuiltIn,
        name: String::from(name),
        version: String::new(),
        kind: String::from("app"),
        bytes: 0,
        licence: String::new(),
        grants: Vec::new(),
        installed: None,
        summary: String::from(summary),
    }
}

/// The image's own apps, by name: the taskbar's pins before `pkg` answers.
pub(super) fn built_in_names() -> Vec<String> {
    deskcore::taskbar::IMAGE_APPS
        .iter()
        .map(|name| String::from(*name))
        .collect()
}

/// Start opened: its rows from `pkg`, or with none (it does not answer, or
/// lists nothing) the image's own apps. The taskbar's pins follow what is
/// installed.
pub(super) fn open(state: &mut State) {
    if state.start.is_some() {
        return;
    }
    let mut apps: Vec<App> = match rows() {
        Ok(rows) => rows.into_iter().map(app).collect(),
        Err(why) => {
            println!("desktop: the Apps sheet has no package rows: {why}");
            Vec::new()
        }
    };
    if apps.is_empty() {
        apps = BUILT_IN.into_iter().map(built_in).collect();
    }
    let installed = apps
        .iter()
        .filter(|a| a.source == Source::Installed)
        .count();
    println!(
        "desktop: the Apps sheet lists {} apps, {installed} installed",
        apps.len()
    );
    for app in apps
        .iter()
        .filter(|a| !matches!(a.source, Source::Installed | Source::BuiltIn))
    {
        // Shown as text, which this line is too.
        println!(
            "desktop: the Apps sheet offers {}: {:?}",
            app.name, app.summary
        );
    }
    let on_machine: Vec<String> = apps
        .iter()
        .filter(|a| matches!(a.source, Source::Installed | Source::BuiltIn))
        .map(|a| a.name.clone())
        .collect();
    state.desk.set_installed(on_machine);
    let mut start = Start::new(apps, false);
    start.set_caps(state.desk.caps());
    state.start = Some(start);
    damage(state);
}

/// Start closed.
pub(super) fn close(state: &mut State) {
    if state.start.take().is_some() {
        damage(state);
    }
}

/// All apps: the `apps` line, "All apps ›" or the taskbar's "more". Start
/// opens for it if it is not open.
pub(super) fn show_all(state: &mut State) {
    if state.desk.start().is_none() {
        let mut effects = Vec::new();
        state.desk.open_start(&mut effects);
        deliver(state, &effects);
    }
    if let Some(start) = state.start.as_mut() {
        start.show_all();
        println!("desktop: Start shows all apps");
        damage(state);
    }
}

/// Start's rectangle, damaged: it changed.
fn damage(state: &mut State) {
    let area = state.desk.start_area();
    state.extra.push(area);
}

/// A key for the panel while Start is open: Start's. False when it is not
/// open, and the panel has it.
pub(super) fn key(state: &mut State, usage: u16, down: bool, now_ms: u64) -> bool {
    let Some(start) = state.start.as_mut() else {
        return false;
    };
    let (changed, pick) = start.key(usage, down);
    if changed {
        damage(state);
    }
    if let Some(pick) = pick {
        carry_out(state, pick, now_ms, true);
    }
    true
}

/// A press inside Start at `at`.
pub(super) fn click(state: &mut State, at: Point, now_ms: u64) {
    let Some(start) = state.start.as_mut() else {
        return;
    };
    let area = state.desk.start_area();
    let (changed, pick) = deskcore::paint_start::click(start, area, at, &state.chat);
    if changed {
        damage(state);
    }
    if let Some(pick) = pick {
        carry_out(state, pick, now_ms, false);
    }
}

/// What Start picked: an app's line to the shell, the typed text to Kai
/// (and the pane opens with it), or Start closes. `by_key`: an Enter picked
/// it, which the pane's input then lets go of before it takes another.
fn carry_out(state: &mut State, pick: Pick, now_ms: u64, by_key: bool) {
    let mut effects = Vec::new();
    let line = match pick {
        Pick::Close => None,
        Pick::Open(name) => Some((format!("open {name}"), false)),
        Pick::Install(name) => Some((format!("install {name}"), false)),
        Pick::Ask(text) => Some((text, true)),
    };
    let asked = matches!(line, Some((_, true)));
    match line {
        Some((text, true)) => {
            let len = text.chars().count();
            state.panel.send(text, now_ms);
            println!("desktop: the panel queued a request ({len} characters)");
            state.desk.focus_pane(&mut effects);
            super::wake_shell();
        }
        Some((line, false)) => {
            println!("desktop: the Apps sheet sent {line:?}");
            state.panel.send(line, now_ms);
            state.desk.close_start(&mut effects);
            super::wake_shell();
        }
        None => state.desk.close_start(&mut effects),
    }
    state.extra.push(state.desk.pane_area());
    deliver(state, &effects);
    // After the focus effects (Start's close passes the focus through the
    // window before the pane): the pane's input lets the held Enter go.
    if by_key && asked {
        state.panel.hold_enter();
    }
}

/// Every page of `pkg`'s catalogue, from offset 0 until a page holds no row.
fn rows() -> Result<Vec<Row>, String> {
    let route = pkg().ok_or("no route to pkg")?;
    let mut rows = Vec::new();
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    while rows.len() < catalog::MAX_ROWS {
        let request = format!("catalog {}", rows.len());
        let len = sys::call_route(route, request.as_bytes(), &mut reply, CATALOG_MS)
            .map_err(|error| format!("pkg did not answer ({error:?})"))?;
        let text = core::str::from_utf8(&reply[..len.min(reply.len())])
            .map_err(|_| String::from("the catalogue is not text"))?;
        let page = catalog::parse(text)?;
        if page.is_empty() {
            break;
        }
        rows.extend(page);
    }
    rows.truncate(catalog::MAX_ROWS);
    Ok(rows)
}

/// A catalogue row as the sheet draws it.
fn app(row: Row) -> App {
    App {
        source: match row.place {
            Place::Installed => Source::Installed,
            Place::Image => Source::Image,
            Place::Mirror => Source::Mirror,
        },
        name: row.name,
        version: format!("{}", row.version),
        kind: row.kind,
        bytes: row.size,
        licence: row.licence,
        grants: row.grants,
        installed: row.installed.map(|v| format!("{v}")),
        summary: row.summary,
    }
}
