//! Who may ask `updated` what (P4 task 10). The shell carries out the agent's
//! `model_*` rows, so it may send every request `updated` answers. The model
//! manager's window reads the list and nothing else: a switch or an import is
//! Kai's row, logged and undoable, never the window's. Any other process is
//! refused. A caller is known by the name the kernel gave its process (its
//! plan line's), never by anything in the request.

/// The shell's process name.
pub const SHELL: &str = "shell";
/// The model manager's window: its plan line's name, which comphost also
/// grants `kai:models` by.
pub const WINDOW: &str = "models-window";
/// The one request the window may send.
pub const LIST: &str = "models";

/// A caller of `updated`, by its process name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asker {
    Shell,
    Window,
    Other,
}

impl Asker {
    pub fn of(name: &str) -> Self {
        match name {
            SHELL => Asker::Shell,
            WINDOW => Asker::Window,
            _ => Asker::Other,
        }
    }

    /// Whether this caller may send `request`, or why not.
    pub fn may_ask(self, request: &str) -> Result<(), &'static str> {
        match self {
            Asker::Shell => Ok(()),
            Asker::Window if request == LIST => Ok(()),
            Asker::Window => Err("the model manager's window may only read the list"),
            Asker::Other => Err("only the shell and the model manager's window may ask"),
        }
    }
}
