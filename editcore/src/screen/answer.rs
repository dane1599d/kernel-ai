//! What each of the service's answers does to the screen.

use super::pager::{self, Pager};
use super::reply::{self, View};
use super::{Cell, Kind, Mode, Request, Screen};
use crate::ipc;
use alloc::format;
use alloc::string::String;

/// The place a reply says a build moved the cursor to: `path:line`.
fn place(text: &str) -> Option<(String, usize)> {
    let (path, line) = text.rsplit_once(':')?;
    Some((String::from(path), line.parse().ok()?))
}

impl Screen {
    /// The answer to request `id`. One for a request that is not the one
    /// waiting is ignored.
    pub fn reply(&mut self, id: u32, text: &str) {
        let Some(pending) = self.take(id) else { return };
        let (moved, body) = ipc::split_moved(text.as_bytes());
        let body = core::str::from_utf8(body).unwrap_or("err not text");
        let result = reply::split(body);
        match pending.kind {
            // Asked for before the screen changed those lines.
            Kind::Window(..) if pending.epoch != self.epoch => {}
            Kind::Open => self.opened(result),
            Kind::Meta => self.meta(result),
            Kind::Window(from, to) => self.window(result, from, to),
            Kind::Files => match result.map(reply::parse_files) {
                Ok(Ok(files)) => self.files = files,
                Ok(Err(why)) => self.say(format!("files: {why}"), true),
                Err(why) => self.say(format!("files: {why}"), true),
            },
            Kind::Edit(command) => match result {
                Ok(_) => {
                    self.keep(&command);
                    self.doc.flags.changed = true;
                    self.doc.flags.undo = true;
                    self.doc.flags.built = false;
                }
                Err(why) => {
                    self.say(String::from(why), true);
                    self.reload_all();
                }
            },
            Kind::Reload(command) => {
                match result {
                    Ok(_) => self.keep(&command),
                    Err(why) => self.say(String::from(why), true),
                }
                self.reload_all();
            }
            Kind::Replay(command) => match result {
                Ok(_) => self.keep(&command),
                Err(why) => {
                    self.queue.retain(|p| !matches!(p.kind, Kind::Replay(_)));
                    let kept = self.log.len();
                    self.say(
                        format!("restored {kept} edits; the next did not replay: {why}"),
                        true,
                    );
                }
            },
            Kind::Build => match result {
                Ok(started) => {
                    self.building = true;
                    self.polled = None;
                    self.say(String::from(started), false);
                }
                Err(why) => self.say(String::from(why), true),
            },
            Kind::Status => match result {
                Ok(status) => self.status_reply(status),
                Err(why) => self.say(String::from(why), true),
            },
            Kind::Show(title) => match result {
                Ok(body) => {
                    self.mode = Mode::Pager(Pager::new(&title, pager::Kind::Text, body));
                }
                Err(why) => self.say(String::from(why), true),
            },
            Kind::OpenFile(line) => match result {
                Ok(_) => {
                    self.goto = Some(line);
                    self.reload_all();
                }
                Err(why) => self.say(String::from(why), true),
            },
        }
        if let Some((path, line)) = moved.and_then(place) {
            self.go_to(&path, line);
        }
        self.fill();
    }

    fn opened(&mut self, result: Result<&str, &str>) {
        match result {
            Ok(body) => {
                // A session that still holds the edits has them already.
                if self.resume.is_some() && body.contains(", with your edits") {
                    self.queue.retain(|p| !matches!(p.kind, Kind::Replay(_)));
                }
                self.say(String::from(body), false);
            }
            Err(why) => {
                self.resume = None;
                self.queue
                    .retain(|p| !matches!(p.kind, Kind::Replay(_) | Kind::Meta | Kind::Files));
                self.say(String::from(why), true);
            }
        }
    }

    /// `view 1,1`: the file, how long it is and where the service's cursor is.
    fn meta(&mut self, result: Result<&str, &str>) {
        let view = match result.map(reply::parse_view) {
            Ok(Ok(view)) => view,
            Ok(Err(why)) => return self.say(why, true),
            Err(why) => return self.say(String::from(why), true),
        };
        let same = self.place.as_ref() == Some(&(view.component.clone(), view.label.clone()))
            && self.doc.path == view.path;
        if !same {
            self.doc.lines.clear();
        }
        self.place = Some((view.component.clone(), view.label.clone()));
        self.take_view(&view);
        let last = view.len.max(1);
        let line = self.goto.take().unwrap_or(view.cursor).clamp(1, last);
        self.line = line;
        self.col = 0;
        self.goal = 0;
        self.top = 1;
        if let Some(draft) = self.resume.take() {
            self.restore(&draft);
        }
        self.follow();
    }

    /// Puts a draft's cursor and typed line back, if they still fit the file.
    fn restore(&mut self, draft: &super::Draft) {
        if draft.file == self.doc.path {
            self.line = draft.line.clamp(1, self.doc.len.max(1));
            self.col = draft.col;
            self.top = draft.top.max(1);
        }
        if let Some((line, text)) = &draft.dirty
            && draft.file == self.doc.path
            && *line <= self.doc.len
        {
            self.dirty = Some(super::Dirty {
                line: *line,
                text: text.clone(),
            });
            self.line = *line;
            self.col = self.col.min(text.chars().count());
        }
        self.goal = self.col;
    }

    /// Takes a view's header and lines into the file on screen.
    fn take_view(&mut self, view: &View) {
        self.doc.path = view.path.clone();
        self.doc.len = view.len;
        self.doc.flags = view.flags;
        self.doc.lines.retain(|&n, _| n <= view.len);
        for shown in &view.lines {
            self.doc.lines.insert(
                shown.n,
                Cell {
                    text: shown.text.clone(),
                    exact: shown.exact,
                },
            );
        }
    }

    fn window(&mut self, result: Result<&str, &str>, from: usize, to: usize) {
        let view = match result.map(reply::parse_view) {
            Ok(Ok(view)) => view,
            Ok(Err(why)) => return self.fail_window(from, to, &why),
            Err(why) => return self.fail_window(from, to, why),
        };
        let place = (view.component.clone(), view.label.clone());
        if self.place.as_ref() != Some(&place) || self.doc.path != view.path {
            // The service moved to another file or version under the screen.
            return self.reload_all();
        }
        if view.lines.is_empty() {
            self.fail_window(from, to, "the service sent no lines");
        }
        self.take_view(&view);
    }

    /// Lines that cannot be had show as empty and read-only, so the screen does
    /// not ask for them again.
    fn fail_window(&mut self, from: usize, to: usize, why: &str) {
        for n in from..=to.min(self.doc.len) {
            self.doc.lines.entry(n).or_insert(Cell {
                text: String::new(),
                exact: false,
            });
        }
        self.say(format!("lines {from} to {to}: {why}"), true);
    }

    fn status_reply(&mut self, status: &str) {
        let (first, last) = match status.split_once("\nlast build: ") {
            Some((first, last)) => (first, Some(last)),
            None => (status, None),
        };
        self.building = first.contains("building");
        if self.building {
            return self.say(String::from(first), false);
        }
        let Some(last) = last else {
            return self.say(String::from(first), false);
        };
        let failed = last.starts_with("err ");
        let headline = last.lines().next().unwrap_or("");
        self.say(String::from(headline), failed);
        if failed && last.contains('\n') {
            self.mode = Mode::Pager(Pager::new("build", pager::Kind::Text, last));
        }
    }

    /// Goes to `path` line `line`: this file at once, another by opening it.
    pub(crate) fn go_to(&mut self, path: &str, line: usize) {
        let Some(path) = self.resolve(path) else {
            return self.say(format!("{path} is not in this session"), true);
        };
        self.commit();
        if path == self.doc.path {
            self.line = line.clamp(1, self.doc.len.max(1));
            self.col = 0;
            self.goal = 0;
            self.follow();
        } else {
            self.push(Kind::OpenFile(line), Request::Cmd(format!("open {path}")));
        }
    }

    /// The session's file `path` names: itself, or the one file that ends with
    /// `/path`.
    fn resolve(&self, path: &str) -> Option<String> {
        if path == self.doc.path || self.files.iter().any(|f| f.path == path) {
            return Some(String::from(path));
        }
        let suffix = format!("/{path}");
        let mut matches = self
            .files
            .iter()
            .map(|f| f.path.as_str())
            .chain(core::iter::once(self.doc.path.as_str()))
            .filter(|p| p.ends_with(&suffix));
        let first = matches.next()?;
        matches.all(|p| p == first).then(|| String::from(first))
    }
}
