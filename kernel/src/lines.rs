//! Source lines for kernel addresses, from the `kernel.lines` Limine module.

use core::fmt;
use linetable::{LineTable, Location};
use spin::Once;

static TABLE: Once<LineTable<'static>> = Once::new();

/// Installs the table. The module memory stays mapped for the kernel's lifetime.
pub fn init(data: &'static [u8]) -> Result<(), linetable::Error> {
    let table = LineTable::parse(data)?;
    TABLE.call_once(|| table);
    Ok(())
}

pub fn table() -> Option<&'static LineTable<'static>> {
    TABLE.get()
}

pub fn resolve(addr: u64) -> Option<Location<'static>> {
    TABLE.get().and_then(|table| table.resolve(addr))
}

/// Formats an address as `0x... in symbol at file:line`, with whatever is known.
pub struct Describe(pub u64);

impl fmt::Display for Describe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#018x}", self.0)?;
        let Some(location) = resolve(self.0) else {
            return f.write_str(" (no source)");
        };
        if let Some(symbol) = location.symbol {
            write!(f, " in {symbol}")?;
        }
        if let (Some(file), Some(line)) = (location.file, location.line) {
            write!(f, "\n      at {file}:{line}")?;
        }
        Ok(())
    }
}
