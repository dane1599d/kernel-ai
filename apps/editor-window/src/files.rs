//! The editor on a file of the person's own: Ctrl-O asks for a path, the OS reads
//! the file (`kai:files/files.read`), and Ctrl-S sends it back whole
//! (`kai:files/files.write`). Ctrl-B on a saved file in a C component's folder
//! asks the OS to build that component (`kai:files/files.build`) and shows its
//! report. The file's lines are edited here, in an
//! `editcore::file::FileEditor` that answers the screen's requests in the call
//! that makes them; the edit service is for components and has no part in it.
//!
//! The import names a whole file and nothing of how it travels, so this module
//! does not know. The OS reads and writes through its own `fs_*` rows, so what it
//! refuses (a path the person has not allowed, a file that changed) comes back as
//! the reason and shows in the status row. One request runs at a time.

use crate::Host;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use editcore::file::{COMPONENT, FileEditor, check_path, component_of};
use editcore::screen::Screen;

/// Most requests the screen is answered in one go: it asks for the lines on
/// screen, the list and the cursor, a few at a time.
const MAX_ROUNDS: usize = 500;
/// What a read's reason starts with when the path is not there, which makes the
/// editor start a new file there (`kai:files`).
pub const MISSING: &str = "missing";
/// Most bytes of unsaved text a checkpoint keeps. A checkpoint is at most
/// `compsdk::MAX_STATE` (64 KiB, the host's limit) with its path and framing, and a
/// file the editor holds is at most `editcore::MAX_BUNDLE` (60 KiB), so this leaves
/// room; past it the path is kept and the text is not, which cannot happen to text
/// the session took, and keeps a checkpoint from ever being too big to save.
pub const KEEP_MAX: usize = compsdk::MAX_STATE - 2048;
// Every text the session takes is kept, and a checkpoint with it still fits.
const _: () = assert!(KEEP_MAX >= editcore::MAX_BUNDLE && KEEP_MAX + 1024 < compsdk::MAX_STATE);

/// What a checkpoint kept of the file open, to put back once the file is read
/// again: the text as it stood (not what the disk has) and the cursor's line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// The text not saved, if there was any.
    pub text: Option<String>,
    /// The line the cursor was on, from 1.
    pub line: u32,
}

/// A request out.
enum Job {
    Read {
        path: String,
        ticket: u32,
    },
    Write {
        path: String,
        /// What was sent: the file as it was when Ctrl-S was pressed.
        bytes: Vec<u8>,
        ticket: u32,
    },
    Build {
        /// The C component being built.
        name: String,
        ticket: u32,
    },
}

impl Job {
    fn ticket(&self) -> u32 {
        match self {
            Self::Read { ticket, .. } | Self::Write { ticket, .. } | Self::Build { ticket, .. } => {
                *ticket
            }
        }
    }
}

/// The file open, if any, and the request out, if any.
#[derive(Default)]
pub(crate) struct Files {
    editor: Option<FileEditor>,
    job: Option<Job>,
    /// The path was not there when it was opened: the first save makes it.
    new: bool,
    /// What a checkpoint kept, to put back when the read comes.
    restore: Option<Kept>,
    /// What to say once the screen has taken the file's answers, which say
    /// "editing" over it.
    say_after: Option<String>,
}

impl Files {
    /// Whether `ticket` is a file request's.
    pub(crate) fn owns(&self, ticket: u32) -> bool {
        self.job.as_ref().is_some_and(|job| job.ticket() == ticket)
    }

    /// Whether the file open has edits the disk does not, or the component's
    /// source has edits not built, or a line is being typed.
    pub(crate) fn unsaved(&self, screen: &Screen) -> bool {
        let edits = match &self.editor {
            Some(editor) => editor.changed(),
            None => screen.edits() > 0,
        };
        edits || screen.typing()
    }

    /// The text of the file open that the disk does not have, with the line being
    /// typed in it, for a checkpoint; None when nothing is unsaved or it is over
    /// [`KEEP_MAX`].
    pub(crate) fn unsaved_text(&self, screen: &Screen) -> Option<String> {
        let editor = self.editor.as_ref()?;
        let typed = screen.draft().and_then(|draft| draft.dirty);
        if !editor.changed() && typed.is_none() {
            return None;
        }
        let bytes = match &typed {
            Some((line, text)) => editor.bytes_with_line(*line, text),
            None => editor.bytes(),
        };
        let text = String::from_utf8(bytes).ok()?;
        (text.len() <= KEEP_MAX).then_some(text)
    }

    /// The file being read again for a checkpoint, and what the checkpoint kept of
    /// it, while the read is out: nothing else holds that text until it is put back.
    pub(crate) fn restoring(&self) -> Option<(&str, &Kept)> {
        match (&self.job, &self.restore) {
            (Some(Job::Read { path, .. }), Some(kept)) => Some((path.as_str(), kept)),
            _ => None,
        }
    }

    /// Whether there is a file to save, as opposed to a component's source.
    pub(crate) fn can_save(&self) -> bool {
        self.editor.is_some()
    }

    /// Whether a request is out.
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The path of the file open, if any.
    pub(crate) fn path(&self) -> Option<&str> {
        self.editor.as_ref().map(FileEditor::path)
    }

    /// Leaves file mode for a component's source: a fresh screen of `grid`.
    pub(crate) fn close(&mut self, screen: &mut Screen, grid: (usize, usize)) {
        if self.editor.take().is_some() {
            *screen = Screen::new(grid.0, grid.1);
        }
        self.new = false;
        self.restore = None;
    }

    /// Answers what the screen asks of "the service" from the file, at once.
    /// False when no file is open and the edit service is the one to ask.
    pub(crate) fn answer(&mut self, screen: &mut Screen) -> bool {
        let Some(editor) = self.editor.as_mut() else {
            return false;
        };
        for _ in 0..MAX_ROUNDS {
            let Some(out) = screen.next_request() else {
                if let Some(note) = self.say_after.take() {
                    screen.note(note, true);
                }
                return true;
            };
            let reply = editor.handle(&out.request.encode());
            screen.reply(out.id, &reply);
        }
        true
    }

    /// Ctrl-O's path was typed, or the agent's `editor_open_file` named one: asks
    /// the OS for the file, or says why not (for the caller to show or answer).
    pub(crate) fn ask_open(
        &mut self,
        host: &mut dyn Host,
        screen: &mut Screen,
        path: &str,
    ) -> Result<(), String> {
        if self.job.is_some() {
            return Err(String::from("a file is still on its way; wait for it"));
        }
        if self.unsaved(screen) {
            return Err(String::from(
                "there are edits not saved or built: Ctrl-S saves a file, Ctrl-B builds a source",
            ));
        }
        check_path(path)?;
        let ticket = host.read_file(path);
        self.job = Some(Job::Read {
            path: String::from(path),
            ticket,
        });
        screen.note(format!("opening {path}..."), false);
        Ok(())
    }

    /// A checkpoint had `path` open, with `kept`: reads it again, and the text
    /// not saved is put back over what the disk has once the read comes.
    ///
    /// When the file cannot be asked for, the text kept is shown as a new file at
    /// the path, and says so, rather than lost with the file.
    pub(crate) fn ask_restore(
        &mut self,
        host: &mut dyn Host,
        screen: &mut Screen,
        grid: (usize, usize),
        path: &str,
        kept: Kept,
    ) -> Result<(), String> {
        match self.ask_open(host, screen, path) {
            Ok(()) => {
                self.restore = Some(kept);
                Ok(())
            }
            Err(why) if kept.text.is_some() => {
                self.restore = Some(kept);
                self.start(screen, grid, path, b"", true);
                let shown = format!("{path} was not opened again ({why}); the text kept is shown");
                self.say_after.get_or_insert(shown);
                Ok(())
            }
            Err(why) => Err(why),
        }
    }

    /// Ctrl-S: sends the file whole.
    pub(crate) fn ask_save(&mut self, host: &mut dyn Host, screen: &mut Screen) {
        let Some(editor) = &self.editor else {
            return screen.note(
                "Ctrl-S saves a file; Ctrl-B builds a component's source",
                true,
            );
        };
        if self.job.is_some() {
            return screen.note("a file is still on its way; wait for it", true);
        }
        if !editor.changed() && !self.new {
            return screen.note("nothing to save", false);
        }
        let path = String::from(editor.path());
        let bytes = editor.bytes();
        let ticket = host.write_file(&path, &bytes, self.new);
        screen.note(format!("saving {path}..."), false);
        self.job = Some(Job::Write {
            path,
            bytes,
            ticket,
        });
    }

    /// Ctrl-B: builds the C component whose folder the file is in, from what is
    /// saved on disk. The report comes back with [`Files::reply`].
    pub(crate) fn ask_build(&mut self, host: &mut dyn Host, screen: &mut Screen) {
        let Some(editor) = &self.editor else {
            return screen.note(
                "Ctrl-B builds a C component: open a file in one first",
                true,
            );
        };
        if self.job.is_some() {
            return screen.note("a file is still on its way; wait for it", true);
        }
        let name = match component_of(editor.path()) {
            Ok(name) => String::from(name),
            Err(why) => return screen.note(why, true),
        };
        // The OS builds the files as they are on disk, not what is typed here.
        if editor.changed() || self.new {
            return screen.note("Ctrl-S saves first: a build reads the files as saved", true);
        }
        let ticket = host.build_component(&name);
        screen.note(format!("building {name}..."), false);
        self.job = Some(Job::Build { name, ticket });
    }

    /// The OS's answer to the request out, or why there is none.
    pub(crate) fn reply(
        &mut self,
        screen: &mut Screen,
        grid: (usize, usize),
        outcome: Result<Vec<u8>, String>,
    ) {
        let Some(job) = self.job.take() else {
            return;
        };
        match (job, outcome) {
            (Job::Read { path, .. }, Ok(bytes)) => self.start(screen, grid, &path, &bytes, false),
            (Job::Read { path, .. }, Err(why)) if why.starts_with(MISSING) => {
                self.start(screen, grid, &path, b"", true);
            }
            // The text kept is shown as a new file, which a save refuses to
            // write over a file that is there, rather than lost for a read.
            (Job::Read { path, .. }, Err(why))
                if self
                    .restore
                    .as_ref()
                    .is_some_and(|kept| kept.text.is_some()) =>
            {
                self.start(screen, grid, &path, b"", true);
                self.say_after = Some(format!(
                    "{path} was not read again ({why}); the text kept is shown"
                ));
            }
            (Job::Read { .. }, Err(why)) => {
                self.restore = None;
                screen.note(why, true);
            }
            (Job::Write { path, bytes, .. }, Ok(_)) => self.wrote(screen, &path, &bytes),
            (Job::Write { .. }, Err(why)) => screen.note(format!("not saved: {why}"), true),
            (Job::Build { .. }, Ok(report)) => {
                screen.build_report(&String::from_utf8_lossy(&report))
            }
            (Job::Build { name, .. }, Err(why)) => {
                screen.note(format!("{name} was not built: {why}"), true);
            }
        }
    }

    /// The OS wrote `bytes`: they are the disk's, unless more was typed meanwhile.
    fn wrote(&mut self, screen: &mut Screen, path: &str, bytes: &[u8]) {
        self.new = false;
        if let Some(editor) = self.editor.as_mut()
            && editor.bytes() == bytes
        {
            editor.saved();
            screen.saved();
        }
        screen.note(format!("saved {path} ({} bytes)", bytes.len()), false);
    }

    /// Shows `bytes` as the file at `path`, on a fresh screen.
    fn start(
        &mut self,
        screen: &mut Screen,
        grid: (usize, usize),
        path: &str,
        bytes: &[u8],
        new: bool,
    ) {
        let kept = self.restore.take().unwrap_or_default();
        let mut new = new;
        let mut lost = None;
        let opened = match FileEditor::open(path, bytes) {
            Ok(editor) => Ok(editor),
            // A file that cannot be opened (not text, too big) must not take the
            // text kept with it: it is shown as a new file at the path.
            Err(why) if kept.text.is_some() => {
                new = true;
                lost = Some(format!("{why}; the text kept is shown"));
                FileEditor::open(path, b"")
            }
            Err(why) => Err(why),
        };
        match opened {
            Err(why) => screen.note(why, true),
            Ok(mut editor) => {
                let name = FileEditor::name_of(path);
                if let Some(text) = &kept.text
                    && let Err(why) = editor.replace_text(text.as_bytes())
                {
                    lost = Some(format!("the text kept was not put back: {why}"));
                }
                self.editor = Some(editor);
                self.new = new;
                *screen = Screen::new(grid.0, grid.1);
                screen.set_file_mode(true);
                screen.open(COMPONENT, None, Some((&name, kept.line.max(1))));
                if lost.is_some() {
                    self.say_after = lost;
                }
            }
        }
    }
}
