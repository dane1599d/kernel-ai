//! Compiling an edited component: cargo with the boot versions' target and
//! profile, the lock file frozen, a scrubbed environment and a time limit.

use crate::components::TARGET;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// A build that runs longer than this is killed.
pub const BUILD_LIMIT: Duration = Duration::from_secs(120);
/// Environment variables a build keeps; everything else is dropped. On Windows the
/// system paths stay too: without SYSTEMDRIVE, rustc cannot find the MSVC linker
/// and takes whatever `link.exe` comes first on PATH.
const KEPT: &[&str] = &[
    "PATH",
    "SYSTEMDRIVE",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramData",
    "LOCALAPPDATA",
    "APPDATA",
    "COMSPEC",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    "HOME",
    "USERPROFILE",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "SYSTEMROOT",
    "SystemRoot",
    "windir",
    "TEMP",
    "TMP",
    "TMPDIR",
];

pub trait Compiler: Send {
    /// Builds crate `krate` in `repo` with KAI_VERSION `label`: the module, or the
    /// compiler's messages.
    fn compile(&mut self, repo: &Path, krate: &str, label: &str) -> Result<Vec<u8>, String>;
}

pub struct CargoCompiler {
    /// Kept apart from the workspace's own target directory, so the boot
    /// versions' output is never overwritten.
    pub target_dir: PathBuf,
}

impl Compiler for CargoCompiler {
    fn compile(&mut self, repo: &Path, krate: &str, label: &str) -> Result<Vec<u8>, String> {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(repo)
            .args([
                "build",
                "--frozen",
                "-p",
                krate,
                "--target",
                TARGET,
                "--profile",
                "component",
                "--message-format",
                "short",
                "--color",
                "never",
            ])
            .env_clear()
            .env("KAI_VERSION", label)
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in KEPT {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let errors = thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let output = thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(status);
            }
            if started.elapsed() > BUILD_LIMIT {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            thread::sleep(Duration::from_millis(100));
        };
        let log = format!(
            "{}{}",
            errors.join().unwrap_or_default(),
            output.join().unwrap_or_default()
        );
        match status {
            None => Err(format!(
                "the build took longer than {} s and was stopped\n{log}",
                BUILD_LIMIT.as_secs()
            )),
            Some(status) if !status.success() => Err(log),
            Some(_) => {
                let wasm = self
                    .target_dir
                    .join(TARGET)
                    .join("component")
                    .join(format!("{}.wasm", krate.replace('-', "_")));
                std::fs::read(&wasm).map_err(|e| format!("cannot read {}: {e}", wasm.display()))
            }
        }
    }
}

/// Compiler output fit to send back: host paths taken out, `/` in file paths,
/// cut to `max` bytes.
pub fn sanitize(log: &str, repo: &Path, max: usize) -> String {
    let mut hidden: Vec<String> = Vec::new();
    for path in [
        Some(repo.to_path_buf()),
        dirs_home(),
        std::env::var_os("CARGO_HOME").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    {
        let text = path.to_string_lossy().into_owned();
        hidden.push(text.replace('\\', "/"));
        hidden.push(text);
    }
    hidden.sort_by_key(|text| std::cmp::Reverse(text.len()));
    let mut out = String::new();
    for line in log.lines() {
        let mut line = line.to_string();
        for text in &hidden {
            if !text.is_empty() {
                line = line
                    .replace(&format!("{text}/"), "")
                    .replace(&format!("{text}\\"), "");
                line = line.replace(text.as_str(), "~");
            }
        }
        // `components\notes\src\lib.rs:40:24: error...` on Windows.
        if let Some((path, rest)) = line.split_once(':')
            && !path.contains(' ')
            && rest.starts_with(|c: char| c.is_ascii_digit())
        {
            line = format!("{}:{rest}", path.replace('\\', "/"));
        }
        let line: String = line
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .collect();
        if out.len() + line.len() + 1 > max {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
