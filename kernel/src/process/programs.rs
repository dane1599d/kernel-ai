//! The ring-3 programs Limine loaded as modules (`boot/user/NAME.elf`), each with its
//! source-line table (`boot/user/NAME.lines`) so a fault in ring 3 is reported with
//! the program's file and line.

use alloc::string::String;
use alloc::vec::Vec;
use linetable::LineTable;
use spin::Once;

pub struct Program {
    pub name: String,
    pub elf: &'static [u8],
    lines: Option<LineTable<'static>>,
}

static PROGRAMS: Once<Vec<Program>> = Once::new();

/// Records the programs among the boot modules, given as (path, contents).
pub fn init<'a>(modules: impl Iterator<Item = (&'a str, &'static [u8])> + Clone) {
    let name_of = |path: &'a str, suffix: &str| -> Option<&'a str> {
        let file = path.rsplit('/').next()?;
        path.contains("/user/")
            .then(|| file.strip_suffix(suffix))
            .flatten()
    };
    let programs = modules
        .clone()
        .filter_map(|(path, elf)| {
            let name = name_of(path, ".elf")?;
            let lines = modules
                .clone()
                .find(|(other, _)| name_of(other, ".lines") == Some(name))
                .and_then(|(_, data)| LineTable::parse(data).ok());
            Some(Program {
                name: String::from(name),
                elf,
                lines,
            })
        })
        .collect();
    PROGRAMS.call_once(|| programs);
}

pub fn all() -> &'static [Program] {
    PROGRAMS.get().map_or(&[], Vec::as_slice)
}

pub fn find(name: &str) -> Option<&'static Program> {
    all().iter().find(|program| program.name == name)
}

impl Program {
    /// `file:line (symbol)` for an address in this program, if its table knows it.
    pub fn describe(&self, addr: u64) -> Option<String> {
        let location = self.lines.as_ref()?.resolve(addr)?;
        let mut text = String::new();
        if let (Some(file), Some(line)) = (location.file, location.line) {
            text = alloc::format!("{file}:{line}");
        }
        if let Some(symbol) = location.symbol {
            text = alloc::format!("{text} ({symbol})");
        }
        (!text.is_empty()).then_some(text)
    }
}
