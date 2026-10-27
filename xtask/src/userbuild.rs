//! Builds the ring-3 programs in the `user` crate, each for the target it ships on:
//! most soft-float like the kernel, and the ones that use x87, SSE or AVX for the
//! hard-float `user/x86_64-kai-user.json`.

use crate::KERNEL_TARGET;
use crate::options::Options;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ring-3 programs built soft-float, like the kernel: they never touch vector state.
const USER_PROGRAMS: &[&str] = &[
    "supervisor",
    "shell",
    "crasher",
    "spinner",
    "hog",
    "edit",
    "work",
    "netcheck",
    "handles",
    "scriptrun",
];
/// Ring-3 programs built for the hard-float target: `llm`, whose engine needs SSE and
/// AVX; `comphost`, whose wasmi runs guests' floats natively (ADR 0003 section 6);
/// `appbench`, which measures comphost's crates there; and the tests that check the
/// kernel keeps vector state apart.
const USER_FLOAT_PROGRAMS: &[&str] = &[
    "llm",
    "probe",
    "vecstress",
    "workers",
    "comphost",
    "appbench",
    // Spike S05-1 only (never merged).
    "compose",
];
/// The view guest `appbench` embeds, built first.
const APPBENCH_GUEST: &str = "comp-viewbench";
/// The hard-float ring-3 target and the directory cargo builds it into. A JSON target
/// has no prebuilt core or alloc, so they are built from rust-src.
const USER_FLOAT_TARGET: &str = "user/x86_64-kai-user.json";
const USER_FLOAT_DIR: &str = "x86_64-kai-user";
const BUILD_STD: &[&str] = &[
    "-Zjson-target-spec",
    "-Zbuild-std=core,alloc",
    "-Zbuild-std-features=compiler-builtins-mem",
];

/// Builds every user program, each for its target; returns (name, ELF path) pairs.
pub fn build_user(root: &Path, options: &Options) -> Result<Vec<(&'static str, PathBuf)>, String> {
    build_guest(root)?;
    let mut programs = build_programs(
        root,
        options,
        &[],
        KERNEL_TARGET,
        KERNEL_TARGET,
        USER_PROGRAMS,
    )?;
    programs.extend(build_programs(
        root,
        options,
        BUILD_STD,
        USER_FLOAT_TARGET,
        USER_FLOAT_DIR,
        USER_FLOAT_PROGRAMS,
    )?);
    Ok(programs)
}

/// Builds the wasm32 guest that `appbench` embeds with `include_bytes!`.
fn build_guest(root: &Path) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(root)
        .args(["build", "--quiet", "-p", APPBENCH_GUEST])
        .args([
            "--target",
            crate::components::TARGET,
            "--profile",
            "component",
        ])
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building {APPBENCH_GUEST} failed"));
    }
    Ok(())
}

/// Builds `names` from the `user` crate for `target`, into `target/<dir>/<profile>`.
fn build_programs(
    root: &Path,
    options: &Options,
    unstable: &[&str],
    target: &str,
    dir: &str,
    names: &[&'static str],
) -> Result<Vec<(&'static str, PathBuf)>, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .arg("build")
        .args(unstable)
        .args(["-p", "user", "--target", target]);
    for name in names {
        command.args(["--bin", name]);
    }
    // Spike S05-1 only: the compose stack builds for the hard-float target alone.
    if names.contains(&"compose") {
        command.args(["--features", "compose"]);
    }
    if options.release {
        command.arg("--release");
    }
    let status = command
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("user program build for {target} failed"));
    }
    let profile = if options.release { "release" } else { "debug" };
    let dir = root.join("target").join(dir).join(profile);
    Ok(names.iter().map(|&name| (name, dir.join(name))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    #[test]
    fn every_program_is_built_for_exactly_one_target() {
        let soft: BTreeSet<&str> = USER_PROGRAMS.iter().copied().collect();
        let float: BTreeSet<&str> = USER_FLOAT_PROGRAMS.iter().copied().collect();
        assert!(soft.is_disjoint(&float));
        let sources: BTreeSet<String> = fs::read_dir(root().join("user/src/bin"))
            .expect("user/src/bin")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.strip_suffix(".rs").map(String::from)
            })
            .collect();
        let built: BTreeSet<String> = soft.union(&float).map(|s| s.to_string()).collect();
        assert_eq!(built, sources);
    }

    #[test]
    fn ci_lints_each_program_for_the_target_it_ships_on() {
        let ci = fs::read_to_string(root().join(".github/workflows/ci.yml")).expect("ci.yml");
        let bins = |target: &str| -> BTreeSet<String> {
            let line = ci
                .lines()
                .find(|line| {
                    line.contains("clippy -") && line.contains(target) && line.contains("-p user")
                })
                .unwrap_or_else(|| panic!("no user clippy line for {target}"));
            line.split("--bin ")
                .skip(1)
                .filter_map(|rest| rest.split_whitespace().next())
                .map(String::from)
                .collect()
        };
        let set = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(
            bins(&format!("--target {KERNEL_TARGET}")),
            set(USER_PROGRAMS)
        );
        assert_eq!(
            bins(&format!("--target {USER_FLOAT_TARGET}")),
            set(USER_FLOAT_PROGRAMS)
        );
    }
}
