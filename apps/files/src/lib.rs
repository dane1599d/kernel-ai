//! The Files app's model (P4 task 5): the folder it shows, what is selected, the
//! preview, the name field, and what it asks the OS for. It never touches a
//! file: it queues `fs_list`, `fs_read`, `fs_move`, `fs_trash` and `fs_restore`
//! as effects ([`calls`]) and learns what happened from their results ([`Files::done`]),
//! so the tiers, the log and `undo` are the OS's. Everything a result holds is
//! the user's content, so names are checked ([`names`]) and text is tamed before
//! the window shows it.
//!
//! The agent's rows are `files_open`, `files_reveal` and `files_select`
//! ([`Files::act_open`] and the next two). They move the view; changing files
//! stays with the OS's own `fs_*` rows, which the window's buttons are bound to.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod calls;
pub mod fake;
pub mod names;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use calls::Call;
use names::{Item, Kind, file_name, join, parent, parse_listing, path_ok};

/// The most characters a preview shows.
pub const PREVIEW_CHARS: usize = 1_500;
/// The most lines a preview shows.
pub const PREVIEW_LINES: usize = 24;
/// The most names a folder's preview lists.
pub const PEEK_NAMES: usize = 12;
/// The longest message the status line shows, in characters.
pub const MESSAGE_CHARS: usize = 120;
/// Most requests waiting for an answer; the oldest is forgotten past this.
const MAX_PENDING: usize = 16;
/// Most trashed paths remembered for Restore.
const MAX_TRASHED: usize = 32;

/// How the app queues an effect (`compsdk::app::request` in the component).
pub trait Os {
    /// Queues `call`; its ticket, which [`Files::done`] gets with the result.
    fn request(&mut self, call: &Call) -> u32;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    /// Nothing is selected.
    None,
    Loading,
    Text {
        text: String,
        /// Whether the file goes on past what is shown.
        cut: bool,
    },
    /// A folder's first names, folders marked with a `/`.
    Folder {
        names: Vec<String>,
        count: usize,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Done(String),
    Failed(String),
}

/// Where a listing leads once it arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Then {
    Nothing,
    /// Select this name in the folder.
    Select(String),
    /// The path may name a file: if it is not a folder, show its folder with it
    /// selected.
    OrFile,
}

/// What a queued effect was for. A change carries `nav`, the listing count when
/// it was asked: if a listing was asked since, that listing was queued after the
/// change and the OS runs effects in order, so it already shows the change.
enum Pending {
    List { dir: String, then: Then, nav: u32 },
    Peek { pick: u32 },
    Read { pick: u32 },
    Move { to: String, nav: u32 },
    Trash { path: String, nav: u32 },
    Restore { path: String, nav: u32 },
}

pub struct Files {
    dir: String,
    items: Vec<Item>,
    skipped: usize,
    more: bool,
    selected: Option<String>,
    preview: Preview,
    rename: String,
    /// Paths trashed from this window, newest last: what Restore brings back.
    trashed: Vec<String>,
    status: Status,
    pending: Vec<(u32, Pending)>,
    /// Which navigation and which selection the app is waiting on; an older
    /// answer is dropped.
    nav: u32,
    pick: u32,
}

impl Default for Files {
    fn default() -> Self {
        Self::new()
    }
}

/// `text` for one line of the window: control characters (an escape sequence in
/// a file name, say) become spaces, and it is cut to `max` characters.
pub fn tame(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// A file's text for the preview: tabs and newlines stay, other control
/// characters show as U+FFFD, and it is cut to the preview's size.
fn tame_text(text: &str) -> (String, bool) {
    let mut out = String::new();
    let (mut chars, mut lines) = (0, 1);
    for c in text.chars() {
        if chars == PREVIEW_CHARS || (c == '\n' && lines == PREVIEW_LINES) {
            return (out, true);
        }
        lines += usize::from(c == '\n');
        chars += 1;
        out.push(match c {
            '\n' | '\t' => c,
            c if c.is_control() => '\u{fffd}',
            c => c,
        });
    }
    (out, false)
}

impl Files {
    pub fn new() -> Self {
        Self {
            dir: String::new(),
            items: Vec::new(),
            skipped: 0,
            more: false,
            selected: None,
            preview: Preview::None,
            rename: String::new(),
            trashed: Vec::new(),
            status: Status::Ready,
            pending: Vec::new(),
            nav: 0,
            pick: 0,
        }
    }

    /// The folder shown, relative to the home folder; `""` is home.
    pub fn dir(&self) -> &str {
        &self.dir
    }

    /// The names in the folder: folders first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Names the listing could not use, and whether the folder may hold more.
    pub fn left_out(&self) -> (usize, bool) {
        (self.skipped, self.more)
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected item's place in [`Files::items`].
    pub fn selected_index(&self) -> Option<usize> {
        let name = self.selected.as_deref()?;
        self.items.iter().position(|i| i.name == name)
    }

    pub fn selected_kind(&self) -> Option<Kind> {
        self.selected_index().map(|i| self.items[i].kind)
    }

    pub fn preview(&self) -> &Preview {
        &self.preview
    }

    /// The name field's text.
    pub fn rename_text(&self) -> &str {
        &self.rename
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Whether answers are still to come.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether Restore has something to bring back.
    pub fn can_restore(&self) -> bool {
        !self.trashed.is_empty()
    }

    fn fail(&mut self, why: &str) {
        self.status = Status::Failed(tame(why, MESSAGE_CHARS));
    }

    fn ask(&mut self, os: &mut dyn Os, call: Option<Call>, what: Pending) -> bool {
        let Some(call) = call else {
            self.fail("That path is too long to ask the system about.");
            return false;
        };
        let ticket = os.request(&call);
        if self.pending.len() == MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push((ticket, what));
        true
    }

    fn list(&mut self, os: &mut dyn Os, dir: &str, then: Then) {
        self.nav = self.nav.wrapping_add(1);
        let what = Pending::List {
            dir: dir.to_string(),
            then,
            nav: self.nav,
        };
        self.ask(os, calls::list(dir), what);
    }

    /// Lists the home folder.
    pub fn start(&mut self, os: &mut dyn Os) {
        self.list(os, "", Then::Nothing);
    }

    /// Lists the folder again, keeping what is selected.
    pub fn refresh(&mut self, os: &mut dyn Os) {
        let keep = self.selected.clone();
        self.relist(os, keep);
    }

    /// Lists this folder again with `select` selected, if it is still there.
    fn relist(&mut self, os: &mut dyn Os, select: Option<String>) {
        let dir = self.dir.clone();
        self.list(os, &dir, select.map_or(Then::Nothing, Then::Select));
    }

    /// After a change that was asked when the listing count was `nav`: lists this
    /// folder again, unless a listing was asked since. That one was queued after
    /// the change, and the OS runs effects in order, so it shows the change;
    /// listing again would only drop the navigation it carries.
    fn relist_unless_listed_since(&mut self, os: &mut dyn Os, nav: u32, select: Option<String>) {
        if nav == self.nav {
            self.relist(os, select);
        }
    }

    /// Nothing is selected. A read or peek still on its way for the old
    /// selection is dropped when it arrives.
    fn clear_selection(&mut self) {
        self.pick = self.pick.wrapping_add(1);
        self.selected = None;
        self.rename.clear();
        self.preview = Preview::None;
    }

    /// One folder up, with the folder it came from selected.
    pub fn up(&mut self, os: &mut dyn Os) {
        if self.dir.is_empty() {
            return;
        }
        let came_from = Then::Select(file_name(&self.dir).to_string());
        let above = parent(&self.dir).to_string();
        self.list(os, &above, came_from);
    }

    /// Opens the selected folder; a file is already shown.
    pub fn open_selected(&mut self, os: &mut dyn Os) {
        let (Some(name), Some(Kind::Dir)) = (self.selected.clone(), self.selected_kind()) else {
            return;
        };
        match join(&self.dir, &name) {
            Some(path) => self.list(os, &path, Then::Nothing),
            None => self.fail("That path is too long to open."),
        }
    }

    /// Selects `name` in the folder and asks for what the preview shows.
    pub fn select(&mut self, os: &mut dyn Os, name: &str) -> Result<(), String> {
        let kind = self
            .items
            .iter()
            .find(|i| i.name == name)
            .map(|i| i.kind)
            .ok_or_else(|| format!("There is no {} here.", tame(name, 40)))?;
        self.pick = self.pick.wrapping_add(1);
        self.selected = Some(name.to_string());
        self.rename = name.to_string();
        self.preview = Preview::Loading;
        let pick = self.pick;
        let path = join(&self.dir, name);
        let (call, what) = match kind {
            Kind::File => (
                path.as_deref().and_then(calls::read),
                Pending::Read { pick },
            ),
            Kind::Dir => (
                path.as_deref().and_then(calls::list),
                Pending::Peek { pick },
            ),
        };
        if !self.ask(os, call, what) {
            self.preview = Preview::Failed(String::from("Too long a path to show."));
        }
        Ok(())
    }

    /// The name field changed.
    pub fn set_rename(&mut self, text: &str) {
        self.rename = text.to_string();
    }

    /// Where the name field sends the selected item: a new name in this folder.
    /// `fs_move` renames within one folder only (the shell's `FsChange::Move` and
    /// fsserve's `Rename` refuse anything else), so a name with a `/` is refused
    /// here rather than sent to fail.
    fn target(&self, from_name: &str) -> Result<String, &'static str> {
        let text = self.rename.trim();
        if text.is_empty() {
            return Err("Type a new name first.");
        }
        if text.contains('/') {
            return Err("Moving to another folder comes later; rename within this one.");
        }
        let to = join(&self.dir, text).ok_or("That is not a name I can use.")?;
        if text == from_name {
            return Err("It is already called that.");
        }
        if self.items.iter().any(|i| i.name == text) {
            return Err("There is already one with that name.");
        }
        Ok(to)
    }

    /// Renames the selected item to what the name field says.
    pub fn move_selected(&mut self, os: &mut dyn Os) {
        let Some(name) = self.selected.clone() else {
            return self.fail("Pick something to rename first.");
        };
        match self.target(&name) {
            Err(why) => self.fail(why),
            Ok(to) => {
                let from = join(&self.dir, &name);
                let call = from.as_deref().and_then(|from| calls::mv(from, &to));
                let nav = self.nav;
                self.ask(os, call, Pending::Move { to, nav });
            }
        }
    }

    /// Puts the selected item in the trash; `undo` or Restore brings it back.
    pub fn trash_selected(&mut self, os: &mut dyn Os) {
        let Some(name) = self.selected.clone() else {
            return self.fail("Pick something to trash first.");
        };
        match join(&self.dir, &name) {
            Some(path) => {
                let call = calls::trash(&path);
                let nav = self.nav;
                self.ask(os, call, Pending::Trash { path, nav });
            }
            None => self.fail("That path is too long to trash."),
        }
    }

    /// Brings back the newest item this window trashed.
    pub fn restore_last(&mut self, os: &mut dyn Os) {
        let Some(path) = self.trashed.last().cloned() else {
            return self.fail(
                "Nothing was trashed from this window. `undo` brings back your last change.",
            );
        };
        let call = calls::restore(&path);
        let nav = self.nav;
        self.ask(os, call, Pending::Restore { path, nav });
    }

    /// `files_open`: a folder, or a file in its folder with the file selected.
    pub fn act_open(&mut self, os: &mut dyn Os, path: &str) -> Result<String, String> {
        if !path_ok(path) {
            return Err(String::from("That is not a path under your home folder."));
        }
        let then = if path.is_empty() {
            Then::Nothing
        } else {
            Then::OrFile
        };
        self.list(os, path, then);
        Ok(format!("Opening {}.", shown(path)))
    }

    /// `files_reveal`: the folder `path` is in, with it selected.
    pub fn act_reveal(&mut self, os: &mut dyn Os, path: &str) -> Result<String, String> {
        if path.is_empty() || !path_ok(path) {
            return Err(String::from("That is not a path under your home folder."));
        }
        self.list(os, parent(path), Then::Select(file_name(path).to_string()));
        Ok(format!("Showing {} in its folder.", shown(path)))
    }

    /// `files_select`: a name in the folder shown.
    pub fn act_select(&mut self, os: &mut dyn Os, name: &str) -> Result<String, String> {
        self.select(os, name)?;
        Ok(format!("Selected {}.", shown(name)))
    }

    /// The result of the effect with `ticket`; whether the window changed.
    pub fn done(&mut self, os: &mut dyn Os, ticket: u32, outcome: Result<String, String>) -> bool {
        let Some(at) = self.pending.iter().position(|(t, _)| *t == ticket) else {
            return false;
        };
        let (_, what) = self.pending.remove(at);
        match (what, outcome) {
            (Pending::List { nav, .. }, _) if nav != self.nav => false,
            (Pending::List { dir, then, .. }, Ok(text)) => {
                let listing = parse_listing(&text);
                self.dir = dir;
                self.items = listing.items;
                self.skipped = listing.skipped;
                self.more = listing.more;
                self.clear_selection();
                if let Then::Select(name) = then {
                    // A name that is gone leaves nothing selected.
                    let _ = self.select(os, &name);
                }
                true
            }
            (
                Pending::List {
                    dir,
                    then: Then::OrFile,
                    ..
                },
                Err(_),
            ) => {
                let name = file_name(&dir).to_string();
                self.list(os, parent(&dir), Then::Select(name));
                true
            }
            (Pending::List { .. }, Err(why)) => {
                self.fail(&why);
                true
            }
            (Pending::Peek { pick } | Pending::Read { pick }, _) if pick != self.pick => false,
            (Pending::Peek { .. }, Ok(text)) => {
                let listing = parse_listing(&text);
                let names = listing
                    .items
                    .iter()
                    .take(PEEK_NAMES)
                    .map(|i| match i.kind {
                        Kind::Dir => format!("{}/", i.name),
                        Kind::File => i.name.clone(),
                    })
                    .collect();
                self.preview = Preview::Folder {
                    names,
                    count: listing.items.len(),
                };
                true
            }
            (Pending::Read { .. }, Ok(text)) => {
                let (text, cut) = tame_text(&text);
                self.preview = Preview::Text { text, cut };
                true
            }
            (Pending::Peek { .. } | Pending::Read { .. }, Err(why)) => {
                self.preview = Preview::Failed(tame(&why, MESSAGE_CHARS));
                true
            }
            (Pending::Move { to, nav }, Ok(_)) => {
                self.status = Status::Done(format!("Renamed to {}.", tame(file_name(&to), 60)));
                // Keep the renamed item selected if it is still in this folder.
                let select = (parent(&to) == self.dir).then(|| file_name(&to).to_string());
                self.relist_unless_listed_since(os, nav, select);
                true
            }
            (Pending::Trash { path, nav }, Ok(_)) => {
                if self.trashed.len() == MAX_TRASHED {
                    self.trashed.remove(0);
                }
                self.trashed.push(path.clone());
                self.status = Status::Done(format!(
                    "{} is in the trash. Restore or `undo` brings it back.",
                    tame(file_name(&path), 40)
                ));
                self.clear_selection();
                self.relist_unless_listed_since(os, nav, None);
                true
            }
            (Pending::Restore { path, nav }, Ok(_)) => {
                self.trashed.retain(|p| *p != path);
                self.status = Status::Done(format!("Restored {}.", tame(&shown(&path), 60)));
                let select = (parent(&path) == self.dir).then(|| file_name(&path).to_string());
                self.relist_unless_listed_since(os, nav, select);
                true
            }
            (Pending::Move { .. } | Pending::Trash { .. } | Pending::Restore { .. }, Err(why)) => {
                self.fail(&why);
                true
            }
        }
    }
}

/// A path as the window says it: `Home` for the folder itself.
pub fn shown(path: &str) -> String {
    match path {
        "" => String::from("Home"),
        p => tame(p, 80),
    }
}
