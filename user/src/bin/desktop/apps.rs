//! The Apps sheet's host half (P4 task 3): `apps` sent in the panel opens it,
//! with what `pkg` lists in `kai-catalog 1`, asked for page by page through the
//! `uses=pkg` route and read with `kaipkg::catalog::parse`, which is strict:
//! an answer it does not take shows no row of it. The sheet takes the panel's
//! keys while it is up. Enter sends the shell the line a user would type,
//! `install <name>` or `open <name>`, by the row's own name, so it runs with
//! the same consent as a typed one; Esc, or the focus leaving the panel,
//! closes it. Rows are data: nothing a summary says is sent anywhere.
//!
//! `desktop` calls `pkg`, never the other way, and only when the sheet opens.

use super::State;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use deskcore::catalog::{App, Pick, Sheet, Source};
use deskcore::geom::Rect;
use deskcore::paint_catalog;
use kaipkg::catalog::{self, Place, Row};
use user::{println, sys};

/// How long `pkg` has to answer one page: `desktop` waits on it, so not long.
const CATALOG_MS: u64 = 2_000;

/// The route to `pkg`, the first in `uses=pkg`; none without one.
fn pkg() -> Option<kabi::handle::Handle> {
    user::route(0)
}

/// The sheet's rectangle now, for its damage.
fn area(state: &State, sheet: &Sheet) -> Rect {
    paint_catalog::sheet(state.width, state.height, sheet.apps().len())
}

/// Opens the sheet with `pkg`'s rows, or none when it does not answer.
pub(super) fn open(state: &mut State) {
    if state.sheet.is_some() {
        return;
    }
    let apps = match rows() {
        Ok(rows) => rows.into_iter().map(app).collect::<Vec<App>>(),
        Err(why) => {
            println!("desktop: the Apps sheet has no rows: {why}");
            Vec::new()
        }
    };
    let installed = apps
        .iter()
        .filter(|a| a.source == Source::Installed)
        .count();
    println!(
        "desktop: the Apps sheet lists {} apps, {installed} installed",
        apps.len()
    );
    for app in apps.iter().filter(|a| a.source != Source::Installed) {
        // Shown as text, which this line is too.
        println!(
            "desktop: the Apps sheet offers {}: {:?}",
            app.name, app.summary
        );
    }
    let sheet = Sheet::new(apps);
    state.extra.push(area(state, &sheet));
    state.sheet = Some(sheet);
}

/// Closes the sheet, if it is up.
pub(super) fn close(state: &mut State) {
    if let Some(sheet) = state.sheet.take() {
        state.extra.push(area(state, &sheet));
    }
}

/// A key for the panel while the sheet is up: the sheet's. False when the
/// sheet is not up, and the panel has it.
pub(super) fn key(state: &mut State, usage: u16, down: bool, now_ms: u64) -> bool {
    let Some(sheet) = state.sheet.as_mut() else {
        return false;
    };
    if !down {
        return true;
    }
    let (changed, pick) = sheet.key(usage);
    if changed {
        let rect = paint_catalog::sheet(state.width, state.height, sheet.apps().len());
        state.extra.push(rect);
    }
    let line = match pick {
        None => return true,
        Some(Pick::Close) => None,
        Some(Pick::Open(name)) => Some(format!("open {name}")),
        Some(Pick::Install(name)) => Some(format!("install {name}")),
    };
    close(state);
    if let Some(line) = line {
        println!("desktop: the Apps sheet sent {line:?}");
        state.panel.send(line, now_ms);
        state.extra.push(state.desk.panel());
        super::wake_shell();
    }
    true
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
