//! Command-line options for `cargo xtask build|run|test|soak`.

use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const SOAK_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Build,
    Run,
    Test,
    /// Long-running cases (the 10-minute storm), run on demand rather than on every PR.
    Soak,
    /// Only the host scenarios (live components and the edit loop), without QEMU.
    Smoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accel {
    Auto,
    Kvm,
    Whpx,
    Tcg,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub task: Task,
    pub release: bool,
    pub spikes: bool,
    pub accel: Accel,
    pub timeout: Duration,
    /// LLM server as HOST:PORT for `run`, for example the Mac mini on the LAN.
    pub llm: Option<String>,
    pub model: Option<String>,
    /// `run --edit`: start the build service on the dev clone, so `:edit` works.
    pub edit: bool,
    /// `run --model-disk PATH`: a raw file attached read-only as the `kai-model` disk.
    pub model_disk: Option<PathBuf>,
}

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let task = match args.first().map(String::as_str) {
        Some("build") => Task::Build,
        Some("run") => Task::Run,
        Some("test") => Task::Test,
        Some("soak") => Task::Soak,
        Some("smoke") => Task::Smoke,
        other => {
            return Err(format!(
                "expected build, run, test, soak, smoke or fluency, got {other:?}"
            ));
        }
    };
    let mut options = Options {
        task,
        release: false,
        spikes: task == Task::Test,
        accel: Accel::Auto,
        timeout: Duration::from_secs(if task == Task::Soak {
            SOAK_TIMEOUT_SECS
        } else {
            DEFAULT_TIMEOUT_SECS
        }),
        llm: None,
        model: None,
        edit: false,
        model_disk: None,
    };
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--release" => options.release = true,
            "--spikes" => options.spikes = true,
            "--accel" => {
                options.accel = match rest.next().map(String::as_str) {
                    Some("auto") => Accel::Auto,
                    Some("kvm") => Accel::Kvm,
                    Some("whpx") => Accel::Whpx,
                    Some("tcg") => Accel::Tcg,
                    other => {
                        return Err(format!("--accel expects auto|kvm|whpx|tcg, got {other:?}"));
                    }
                }
            }
            "--timeout" => {
                let secs = rest
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or("--timeout expects a number of seconds")?;
                options.timeout = Duration::from_secs(secs);
            }
            "--llm" => {
                let server = rest.next().ok_or("--llm expects HOST:PORT")?;
                if !valid_host_port(server) {
                    return Err(format!(
                        "--llm expects HOST:PORT with an IPv4 host, got {server:?}"
                    ));
                }
                options.llm = Some(server.clone());
            }
            "--edit" if task == Task::Run => options.edit = true,
            "--model-disk" if task == Task::Run => {
                let path = rest
                    .next()
                    .ok_or("--model-disk expects the path of a file")?;
                options.model_disk = Some(PathBuf::from(path));
            }
            "--model" => {
                let model = rest.next().ok_or("--model expects a model name")?;
                if model.is_empty() || model.chars().any(char::is_whitespace) {
                    return Err("--model expects a name without spaces".into());
                }
                options.model = Some(model.clone());
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

/// IPv4 address and port, for example 192.168.1.50:8080. The kernel has no DNS.
fn valid_host_port(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    host.parse::<std::net::Ipv4Addr>().is_ok() && port.parse::<u16>().is_ok_and(|p| p > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_task_enables_spikes_by_default() {
        let options = parse_args(&args(&["test"])).unwrap();
        assert_eq!(options.task, Task::Test);
        assert!(options.spikes);
        assert_eq!(options.accel, Accel::Auto);
    }

    #[test]
    fn run_task_parses_flags() {
        let options = parse_args(&args(&[
            "run",
            "--release",
            "--accel",
            "tcg",
            "--timeout",
            "5",
        ]))
        .unwrap();
        assert!(options.release);
        assert!(!options.spikes);
        assert_eq!(options.accel, Accel::Tcg);
        assert_eq!(options.timeout, Duration::from_secs(5));
    }

    #[test]
    fn edit_is_for_run_only() {
        assert!(parse_args(&args(&["run", "--edit"])).unwrap().edit);
        assert!(parse_args(&args(&["test", "--edit"])).is_err());
    }

    #[test]
    fn model_disk_is_for_run_only() {
        let options = parse_args(&args(&["run", "--model-disk", r"D:\kai-models\m.gguf"])).unwrap();
        assert_eq!(
            options.model_disk,
            Some(PathBuf::from(r"D:\kai-models\m.gguf"))
        );
        assert!(parse_args(&args(&["run", "--model-disk"])).is_err());
        assert!(parse_args(&args(&["test", "--model-disk", "m.gguf"])).is_err());
    }

    #[test]
    fn rejects_unknown_task_and_options() {
        assert!(parse_args(&args(&["deploy"])).is_err());
        assert!(parse_args(&args(&["run", "--fast"])).is_err());
        assert!(parse_args(&args(&["run", "--accel", "hyperv"])).is_err());
    }
}
