//! `cargo xtask cc-bind [--check]` (ADR 0012 D7; P4 task 13): writes the C
//! bindings `cc build` compiles components and apps against into
//! `ccserve/kai/bind/`, from wit-bindgen 0.63.0's C generator over `wit/`.
//! `--check` writes nothing and fails if the files differ.
//!
//! - `component`: the world `kai:app/component`.
//! - `app`: [`CC_APP`], `kai:app/app` with an empty `actions` export, as
//!   ADR 0003 section 5 requires of every app world.
//!
//! wcc reads `import_module` and `import_name` but not wit-bindgen's
//! `__import_module__` spelling, and no `export_name` at all (S12-1 (h),
//! measured on the host). So the imports are rewritten to the spelling wcc
//! reads, and each world gets `<world>.exports`: one `<symbol> <export name>`
//! line per export, which `cc build`'s link applies. wit-bindgen's
//! `*_component_type.o` is for `wasm-tools component new` and is not written:
//! the OS loads core modules.

use crate::wit;
use std::fs;
use std::path::Path;
use wit_bindgen_core::Files;

/// Where the bindings live.
pub const OUTPUT: &str = "ccserve/kai/bind";

/// The world of an app `cc build` makes: no actions of its own.
pub const CC_APP: &str = "package kai:cc@0.1.0;

/// What an app built at the `c>` prompt exports: no actions of its own.
interface actions {}

world app {
    include kai:app/app@0.1.0;
    export actions;
}
";

/// What the generated text starts with.
const HEADER: &str = "// cargo xtask cc-bind: wit-bindgen-c 0.63.0's output, with its imports in\n// the spelling wcc reads. Do not edit.\n";

pub fn main(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let files = generate(&root.join("wit"))?;
    let dir = root.join(OUTPUT);
    match args {
        [] => {
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for (name, text) in &files {
                let path = dir.join(name);
                fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            println!("xtask: cc-bind: wrote {} files to {OUTPUT}", files.len());
            Ok(())
        }
        [flag] if flag == "--check" => check(&dir, &files),
        _ => Err("usage: cargo xtask cc-bind [--check]".into()),
    }
}

/// Fails unless `dir` holds exactly `files`.
pub fn check(dir: &Path, files: &[(String, String)]) -> Result<(), String> {
    for (name, text) in files {
        let current = fs::read_to_string(dir.join(name))
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if current != *text {
            return Err(format!(
                "{OUTPUT}/{name} is out of date with wit/: run `cargo xtask cc-bind`"
            ));
        }
    }
    Ok(())
}

/// The bindings, as (file name, text): each world's `.h`, `.c` and `.exports`.
pub fn generate(wit_root: &Path) -> Result<Vec<(String, String)>, String> {
    let (mut resolve, packages) = wit::load_tree(wit_root)?;
    let app = packages
        .iter()
        .find(|(name, _)| name.starts_with("kai:app@"))
        .map(|(_, id)| *id)
        .ok_or("no package kai:app under wit/")?;
    let cc = resolve
        .push_str("kai-cc.wit", CC_APP)
        .map_err(|e| format!("kai:cc: {e:#}"))?;
    let mut out = Vec::new();
    for (package, world) in [(app, "component"), (cc, "app")] {
        let id = resolve
            .select_world(&[package], Some(world))
            .map_err(|e| format!("{world}: {e:#}"))?;
        let mut files = Files::default();
        wit_bindgen_c::Opts::default()
            .build()
            .generate(&mut resolve, id, &mut files)
            .map_err(|e| format!("wit-bindgen-c, {world}: {e:#}"))?;
        let mut exports = String::new();
        for (name, bytes) in files.iter() {
            if !(name.ends_with(".h") || name.ends_with(".c")) {
                continue;
            }
            let text = std::str::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8"))?;
            let text = for_wcc(text);
            if name.ends_with(".c") {
                exports.push_str(&export_map(&text)?);
            }
            out.push((name.to_string(), format!("{HEADER}{text}")));
        }
        if exports.is_empty() {
            return Err(format!("{world} exports nothing"));
        }
        out.push((format!("{world}.exports"), exports));
    }
    out.sort();
    Ok(out)
}

/// `text` with its import attributes in the spelling wcc reads.
fn for_wcc(text: &str) -> String {
    text.replace("__import_module__(", "import_module(")
        .replace("__import_name__(", "import_name(")
}

/// One `<symbol> <export name>` line for each `__export_name__` in `text`: the
/// name of the function defined right after the attribute.
fn export_map(text: &str) -> Result<String, String> {
    const MARK: &str = "__export_name__(\"";
    let mut out = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(at) = line.find(MARK) else {
            continue;
        };
        let rest = &line[at + MARK.len()..];
        let name = rest
            .split('"')
            .next()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("an export name without its end: {line}"))?;
        let def = lines
            .next()
            .ok_or_else(|| format!("{name}: no function after its attribute"))?;
        let symbol = def
            .split('(')
            .next()
            .and_then(|head| head.split([' ', '*']).rfind(|w| !w.is_empty()))
            .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .ok_or_else(|| format!("{name}: no function name in {def:?}"))?;
        out.push_str(&format!("{symbol} {name}\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bindings_match_wit() {
        let root = crate::workspace_root();
        let files = generate(&root.join("wit")).unwrap();
        check(&root.join(OUTPUT), &files).unwrap();
    }

    #[test]
    fn imports_take_the_spelling_wcc_reads() {
        let files = generate(&crate::workspace_root().join("wit")).unwrap();
        for (name, text) in &files {
            assert!(!text.contains("__import_module__"), "{name}");
            assert!(!text.contains("__import_name__"), "{name}");
        }
        let app = &files.iter().find(|(n, _)| n == "app.c").unwrap().1;
        assert!(app.contains("import_module(\"kai:app/effects@0.1.0\")"));
    }

    #[test]
    fn each_export_maps_its_symbol_to_its_name() {
        let files = generate(&crate::workspace_root().join("wit")).unwrap();
        let map = &files
            .iter()
            .find(|(n, _)| n == "component.exports")
            .unwrap()
            .1;
        assert!(
            map.lines().any(|l| l
                == "__wasm_export_exports_kai_live_lifecycle_init kai:live/lifecycle@0.1.0#init")
        );
        assert!(map.lines().any(|l| l == "cabi_realloc cabi_realloc"));
        let app = &files.iter().find(|(n, _)| n == "app.exports").unwrap().1;
        assert!(
            app.lines()
                .any(|l| l.ends_with(" kai:app/events@0.1.0#on-event"))
        );
    }

    #[test]
    fn an_export_map_refuses_what_it_cannot_read() {
        assert!(export_map("__attribute__((__export_name__(\"x\")))").is_err());
        assert!(export_map("__attribute__((__export_name__(\"\")))\nint f(void) {").is_err());
        assert_eq!(
            export_map("__attribute__((__export_name__(\"a#b\")))\nint32_t *fn_1(int x) {")
                .unwrap(),
            "fn_1 a#b\n"
        );
    }
}
