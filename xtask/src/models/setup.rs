//! `cargo xtask setup`: the guided host setup for the VM era. It checks the PC,
//! recommends a model for its RAM, downloads the pinned file to the store with
//! a progress bar and resume, checks its SHA-256, and prints what to run next.

use super::catalog::{Catalog, Entry};
use super::{ModelArgs, fetch, machine, store};
use std::io::{BufRead, Write};

/// Room to keep free beyond the model itself.
const SPARE_BYTES: u64 = 1 << 30;

pub fn run(args: &ModelArgs) -> Result<(), String> {
    let store = store::resolve(args.store.as_deref())?;
    println!("xtask: checking this PC for Kernel AI's local model");
    let pc = machine::check(&store);
    for finding in &pc.findings {
        println!("  {:<6} {}", finding.topic, finding.found);
        if let Some(fix) = &finding.fix {
            println!("         fix: {fix}");
        }
    }
    let catalog = Catalog::load()?;
    let entry = choose(&catalog, args, pc.ram_gb)?;
    let size_mb = entry.size / 1_000_000;
    println!(
        "xtask: model: {} ({size_mb} MB, {}). {}",
        entry.name, entry.license, entry.note
    );
    let target = store.join(&entry.file);
    if !target.exists() {
        if let Some(free) = pc.free_bytes
            && free < entry.size + SPARE_BYTES
        {
            return Err(format!(
                "{} has {} MB free; the model needs {size_mb} MB and 1 GB spare. \
                 Free space there or pass --store DIR on another drive.",
                store.display(),
                free / 1_000_000
            ));
        }
        if !args.yes && !confirm(&format!("download {size_mb} MB to {}?", target.display()))? {
            let store = args
                .store
                .as_ref()
                .map_or_else(String::new, |s| format!(" --store \"{s}\""));
            println!(
                "xtask: nothing downloaded; cargo xtask setup {}{store} --yes skips the question",
                entry.id
            );
            return Ok(());
        }
    }
    let path = fetch::fetch(entry, &store)?;
    println!(
        "xtask: {} is ready at {} (SHA-256 checked)",
        entry.name,
        path.display()
    );
    if let Some(why) = entry.not_runnable() {
        println!("xtask: {why}; until then it waits in the store.");
        return Ok(());
    }
    let store_flag = if args.store.is_some() {
        format!(" --store \"{}\"", store.display())
    } else {
        String::new()
    };
    let local = if entry.id == catalog.default {
        String::new()
    } else {
        format!(" --local {}", entry.id)
    };
    println!("next:");
    println!(
        "  cargo xtask model chat{local}{store_flag} \"What does an operating system kernel do?\""
    );
    println!("  cargo xtask model check {}{store_flag}", entry.id);
    println!(
        "  cargo xtask run --local {}{store_flag}   (Kernel AI with the model inside; then: ask ...)",
        entry.id
    );
    Ok(())
}

/// The model named on the command line, else the largest that suits the RAM,
/// else the default.
fn choose<'c>(
    catalog: &'c Catalog,
    args: &ModelArgs,
    ram_gb: Option<u64>,
) -> Result<&'c Entry, String> {
    if args.local.is_some() || !args.words.is_empty() {
        return args.entry(catalog, true);
    }
    let Some(ram) = ram_gb else {
        return catalog.get(&catalog.default);
    };
    match catalog.recommend(ram) {
        Some(entry) => {
            println!("xtask: recommended for {ram} GB of RAM:");
            Ok(entry)
        }
        None => Err(format!(
            "{ram} GB of RAM is below every catalog model's minimum; the smallest needs {} GB",
            catalog.runnable().map(|e| e.min_ram_gb).min().unwrap_or(0)
        )),
    }
}

fn confirm(question: &str) -> Result<bool, String> {
    print!("xtask: {question} [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("cannot read the answer: {e}"))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_follows_the_command_line_then_the_ram() {
        let catalog = Catalog::load().unwrap();
        let none = ModelArgs::default();
        assert_eq!(
            choose(&catalog, &none, Some(32)).unwrap().id,
            "qwen3-4b-2507"
        );
        assert_eq!(choose(&catalog, &none, Some(6)).unwrap().id, "qwen3-1.7b");
        assert_eq!(choose(&catalog, &none, None).unwrap().id, catalog.default);
        assert!(choose(&catalog, &none, Some(1)).is_err());
        let named = ModelArgs::parse(&["qwen3-0.6b"]).unwrap();
        assert_eq!(choose(&catalog, &named, Some(1)).unwrap().id, "qwen3-0.6b");
        // Every model the catalog recommends runs, and so does the default.
        for ram in [4, 8, 16, 32, 1024] {
            let picked = choose(&catalog, &none, Some(ram)).unwrap();
            assert!(picked.not_runnable().is_none(), "{} at {ram} GB", picked.id);
        }
        let k = ModelArgs::parse(&["qwen3-4b-2507"]).unwrap();
        assert!(
            choose(&catalog, &k, Some(1))
                .unwrap()
                .not_runnable()
                .is_none()
        );
        let unknown = ModelArgs::parse(&["--local", "nope"]).unwrap();
        assert!(choose(&catalog, &unknown, Some(32)).is_err());
    }
}
