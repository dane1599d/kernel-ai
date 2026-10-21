//! `cargo xtask doctor`: can this PC run Kernel AI in an accelerated VM, and if
//! not, exactly what to do. It prints one ✓ or ✗ line per item, each ✗ followed by
//! the commands or Windows steps that fix it, and ends with one recommended path
//! and the next command to run.
//!
//! It changes nothing: no install, no setting, no elevation. It only runs the
//! read-only probes `facts.rs` lists, and the founder runs the fixes. The checks
//! (`checks.rs`, with the Linux side's in `wsl/check.rs`) and the recommendation
//! (`advice.rs`) are pure functions of the gathered facts, tested on any host over
//! canned outputs.

pub mod advice;
pub mod checks;
mod facts;
#[cfg(test)]
mod tests;

use crate::wsl::check::{print_items, print_step};

pub fn main(args: &[String]) -> Result<(), String> {
    if let Some(extra) = args.first() {
        return Err(format!("doctor takes no options, got {extra}"));
    }
    println!("xtask: checking this PC for running Kernel AI in an accelerated VM (read-only)");
    let facts = facts::gather();
    println!();
    let mut items = checks::items(&facts);
    if facts.windows {
        let version = crate::vbox::manage::version();
        items.push(crate::vbox::doctor::item(
            version.as_deref(),
            facts.hypervisor,
        ));
    }
    print_items(&items);
    let advice = advice::advise(&facts);
    println!();
    println!("Recommended: {}", advice.path.title());
    println!("  {}", advice.why);
    for (i, step) in advice.steps.iter().enumerate() {
        let lead = format!("  {}. ", i + 1);
        print_step(&lead, "       ", step);
    }
    println!();
    if let Some(first) = advice.steps.first() {
        let next = first.run.as_deref().unwrap_or(&first.say);
        println!("Next (step 1): {next}");
    }
    println!("To see Kernel AI boot, once every step is done:");
    println!("  {}", advice.then);
    Ok(())
}
