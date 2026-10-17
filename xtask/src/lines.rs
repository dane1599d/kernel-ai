//! Builds source-line tables (see the `linetable` crate) from ELF DWARF data, for
//! the kernel and for each user program.

use linetable::Builder;
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use std::fs;
use std::path::Path;

/// Which source lines a table keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every file: the kernel, whose faults may be anywhere.
    All,
    /// Only workspace files: user programs, whose dependencies (Rhai) would make
    /// the table several megabytes.
    Workspace,
}

/// Reads an ELF (the kernel or a user program) and returns its serialized line table.
pub fn build(elf: &Path, workspace: &Path, scope: Scope) -> Result<Vec<u8>, String> {
    let data = fs::read(elf).map_err(|e| format!("cannot read {}: {e}", elf.display()))?;
    let file =
        object::File::parse(&*data).map_err(|e| format!("cannot parse {}: {e}", elf.display()))?;
    let text = file
        .section_by_name(".text")
        .ok_or_else(|| format!("{} has no .text section", elf.display()))?;
    let (low, high) = (text.address(), text.address() + text.size());

    let mut builder = Builder::new();
    let workspace = normalize(&workspace.to_string_lossy());
    let loader = addr2line::Loader::new(elf).map_err(|e| format!("cannot load DWARF: {e}"))?;
    let rows = loader
        .find_location_range(low, high)
        .map_err(|e| format!("cannot read line rows: {e}"))?;
    let mut line_rows = 0usize;
    for (addr, len, location) in rows {
        if let (Some(path), Some(line)) = (location.file, location.line) {
            if scope == Scope::Workspace && !normalize(path).starts_with(&workspace) {
                continue;
            }
            let len = u32::try_from(len).unwrap_or(u32::MAX);
            builder.add_line(addr, len, &shorten(path, &workspace), line);
            line_rows += 1;
        }
    }
    let mut symbols = 0usize;
    for symbol in file.symbols() {
        if symbol.kind() != SymbolKind::Text || symbol.size() == 0 {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        let readable = format!("{:#}", rustc_demangle::demangle(name));
        builder.add_symbol(symbol.address(), symbol.size(), &readable);
        symbols += 1;
    }
    let table = builder.finish();
    println!(
        "xtask: {} line table {} KiB ({line_rows} rows, {symbols} symbols)",
        elf.file_name().unwrap_or_default().to_string_lossy(),
        table.len() / 1024
    );
    Ok(table)
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

/// Shortens a DWARF path for display: workspace files become `kernel/src/...`,
/// crates.io files `crate-version/src/...`, and the standard library `rust/library/...`.
fn shorten(path: &str, workspace: &str) -> String {
    let path = normalize(path);
    if let Some(rest) = path
        .strip_prefix(workspace)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        return rest.to_string();
    }
    if let Some(index) = path.find("/registry/src/") {
        let after_registry = &path[index + "/registry/src/".len()..];
        if let Some(slash) = after_registry.find('/') {
            return after_registry[slash + 1..].to_string();
        }
    }
    if let Some(index) = path.find("/library/") {
        return format!("rust{}", &path[index..]);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn shortens_workspace_registry_and_std_paths() {
        let workspace = "C:/work/kai";
        assert_eq!(
            shorten("C:\\work\\kai\\kernel\\src\\faults.rs", workspace),
            "kernel/src/faults.rs"
        );
        assert_eq!(
            shorten(
                "/home/u/.cargo/registry/src/index.crates.io-123/rhai-1.26.1/src/engine.rs",
                workspace
            ),
            "rhai-1.26.1/src/engine.rs"
        );
        assert_eq!(
            shorten("/rustc/abc/library/core/src/ptr/mod.rs", workspace),
            "rust/library/core/src/ptr/mod.rs"
        );
        assert_eq!(shorten("relative/other.rs", workspace), "relative/other.rs");
    }
}
