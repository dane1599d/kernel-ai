//! Which key does what: the draft line's editing keys, the Ctrl commands, the
//! pager, the prompt and clicks.

use super::edit::{INDENT, typable};
use super::input::{Input, Key};
use super::pager::{self, Act, Pager};
use super::{Kind, Mode, PromptKind, Request, Screen, link};
use alloc::format;
use alloc::string::{String, ToString};

impl Screen {
    pub fn input(&mut self, input: Input) {
        match input {
            Input::Key(key) => match &self.mode {
                Mode::Edit => self.edit_key(key),
                Mode::Pager(_) => self.pager_key(key),
                Mode::Prompt(..) => self.prompt_key(key),
            },
            Input::Click { row, col, ctrl } => self.click(row, col, ctrl),
        }
        self.fill();
    }

    fn edit_key(&mut self, key: Key) {
        let page = self.rows.saturating_sub(1).max(1);
        match key {
            Key::Char(c) if typable(c) => self.insert_text(&c.to_string()),
            Key::Char(_) => self.say("only printable ASCII can be typed here", true),
            Key::Tab => self.insert_text(&" ".repeat(INDENT)),
            Key::Enter => self.enter(),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete(),
            Key::Left => self.left(),
            Key::Right => self.right(),
            Key::Up => self.go_line(self.line.saturating_sub(1), self.goal),
            Key::Down => self.go_line(self.line + 1, self.goal),
            Key::Home => {
                self.col = 0;
                self.goal = 0;
            }
            Key::End => {
                self.col = self.line_len(self.line).unwrap_or(0);
                self.goal = self.col;
            }
            Key::PageUp => {
                self.top = self.top.saturating_sub(page).max(1);
                self.go_line(self.line.saturating_sub(page), self.goal);
            }
            Key::PageDown => {
                self.top = (self.top + page).min(self.doc.len.max(1));
                self.go_line(self.line + page, self.goal);
            }
            Key::Esc => {}
            Key::Ctrl(c) => self.ctrl(c),
        }
        // Up and Down keep the column they started from, where the line is long enough.
        if matches!(key, Key::Up | Key::Down | Key::PageUp | Key::PageDown) {
            self.col = self.goal.min(self.line_len(self.line).unwrap_or(self.goal));
        }
    }

    fn ctrl(&mut self, c: char) {
        self.commit();
        match c {
            'z' => self.reload("undo"),
            'b' => self.push(Kind::Build, Request::Cmd("build".into())),
            'd' => self.show("diff", "diff"),
            't' => self.show("try", "try"),
            'l' => self.show("status", "status"),
            'h' => self.show("help", "help"),
            'f' => self.mode = Mode::Prompt(PromptKind::Find, String::new()),
            'g' => self.mode = Mode::Prompt(PromptKind::Goto, String::new()),
            'e' => self.mode = Mode::Prompt(PromptKind::Command, String::new()),
            'o' if self.file_mode || self.place.is_none() => {
                self.mode = Mode::Prompt(PromptKind::OpenPath, String::new());
            }
            'o' => self.files_pager(),
            's' => self.save = true,
            'q' => self.quit = true,
            _ => {}
        }
    }

    fn reload(&mut self, command: &str) {
        self.push(Kind::Reload(command.into()), Request::Cmd(command.into()));
    }

    fn show(&mut self, title: &str, command: &str) {
        self.push(Kind::Show(title.into()), Request::Cmd(command.into()));
    }

    fn files_pager(&mut self) {
        let mut text = format!("{} files; Enter opens one", self.files.len());
        for file in &self.files {
            let mark = match (file.changed, file.current) {
                (true, true) => "*>",
                (true, false) => "* ",
                (false, true) => " >",
                (false, false) => "  ",
            };
            text.push_str(&format!("\n{mark} {} ({} lines)", file.path, file.len));
        }
        self.mode = Mode::Pager(Pager::new("files", pager::Kind::Files, &text));
        self.push(Kind::Files, Request::Cmd("files".into()));
    }

    // The pager and the prompt.

    fn pager_key(&mut self, key: Key) {
        let Mode::Pager(mut pager) = core::mem::replace(&mut self.mode, Mode::Edit) else {
            return;
        };
        let act = pager.key(key, self.rows);
        self.pager_act(pager, act);
    }

    fn pager_act(&mut self, pager: Pager, act: Act) {
        match act {
            Act::None => self.mode = Mode::Pager(pager),
            Act::Close => {}
            Act::Follow(found) => self.go_to(&found.path, found.line as usize),
            Act::Open(path) => {
                self.push(Kind::OpenFile(1), Request::Cmd(format!("open {path}")));
            }
        }
    }

    fn prompt_key(&mut self, key: Key) {
        let Mode::Prompt(kind, mut text) = core::mem::replace(&mut self.mode, Mode::Edit) else {
            return;
        };
        match key {
            Key::Esc => return,
            Key::Enter => return self.submit(kind, &text),
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) if typable(c) => text.push(c),
            _ => {}
        }
        self.mode = Mode::Prompt(kind, text);
    }

    fn submit(&mut self, kind: PromptKind, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match kind {
            PromptKind::Find => self.show("find", &format!("find {text}")),
            PromptKind::Goto => match text.parse::<usize>() {
                Ok(n) if n >= 1 => self.go_line(n, 0),
                _ => self.say(format!("{text:?} is not a line number"), true),
            },
            PromptKind::Command => self.command(text),
            PromptKind::OpenPath => self.open = Some(String::from(text)),
        }
    }

    /// A command typed at the prompt, as the kernel's edit mode takes it.
    fn command(&mut self, text: &str) {
        let word = text.split(' ').next().unwrap_or("");
        let request = Request::Cmd(text.into());
        match word {
            "build" => self.push(Kind::Build, request),
            "open" => self.push(Kind::OpenFile(1), request),
            "undo" | "reset" | "s" | "c" | "i" | "a" | "d" | "split" | "join" => {
                self.push(Kind::Reload(text.into()), request);
            }
            _ => self.push(Kind::Show(word.into()), request),
        }
    }

    // Clicks.

    fn click(&mut self, row: usize, col: usize, ctrl: bool) {
        if let Mode::Pager(_) = &self.mode {
            let Mode::Pager(mut pager) = core::mem::replace(&mut self.mode, Mode::Edit) else {
                return;
            };
            let act = pager.click(row, col, ctrl, self.rows);
            return self.pager_act(pager, act);
        }
        if !matches!(self.mode, Mode::Edit) {
            return;
        }
        let line = self.top + row;
        let Some(text) = self.text_of(line).map(String::from) else {
            return;
        };
        let cell = col.saturating_sub(self.gutter());
        let col = super::render::col_of_cell(&text, cell);
        if ctrl && let Some(found) = link::at_column(&text, col) {
            return self.go_to(&found.path, found.line as usize);
        }
        self.go_line(line, col);
        self.goal = self.col;
    }
}
