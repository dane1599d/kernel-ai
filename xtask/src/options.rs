//! Command-line options for `cargo xtask build|run|test|soak`.

use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const SOAK_TIMEOUT_SECS: u64 = 900;
/// Per icount boot: OVMF alone takes minutes under single-threaded TCG with icount.
const ICOUNT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Build,
    Run,
    Test,
    /// Long-running cases (the 10-minute storm), run on demand rather than on every PR.
    Soak,
    /// Only the host scenarios (live components and the edit loop), without QEMU.
    Smoke,
    /// The icount measurement spike: the `icount-probe` case booted several times.
    Icount,
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
    /// `run --local ID`: the catalog model `ID`, from the model store, attached the
    /// same way. `--model` keeps meaning the model name sent to an LLM server.
    pub local: Option<String>,
    /// `run --store DIR`: the model store `--local` reads, as `setup --store`.
    pub store: Option<String>,
    /// `--cpus N` and `--mem SIZE`: the VM's vCPUs and memory, for `run`, `test`
    /// and `soak`.
    pub sizing: Sizing,
    /// `icount --shift N`: each guest instruction takes 2^N virtual nanoseconds.
    pub shift: u32,
    /// `icount --boots N`: how many boots must print the same digest.
    pub boots: usize,
    /// `--accel wsl`: QEMU runs inside WSL2, started from Windows (`wsl/launch.rs`),
    /// and `accel` is KVM there.
    pub wsl: bool,
    /// `run --workload A|B [--seed N]`: M10's mix, run as ring-3 `work` processes.
    pub workload: Option<(String, u64)>,
    /// `test --shard K/N`: boot only shard K (from 1) of N; CI splits the cases
    /// across parallel jobs this way.
    pub shard: Option<(usize, usize)>,
}

impl Options {
    /// The kernel command-line words for `--workload`, with a leading space.
    pub fn workload_args(&self) -> String {
        self.workload
            .as_ref()
            .map_or_else(String::new, |(mix, seed)| {
                format!(" workload={mix} seed={seed}")
            })
    }
}

/// The VM's size when given on the command line; xtask's defaults otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sizing {
    /// vCPUs, 1 to the kernel's MAX_CPUS (8).
    pub cpus: Option<usize>,
    /// Guest memory in MiB; a model disk's need is a floor under it.
    pub mem_mib: Option<u64>,
}

/// The kernel brings up at most this many CPUs (`kernel/src/percpu.rs` MAX_CPUS).
pub const MAX_VM_CPUS: usize = 8;
/// Guest memory bounds for `--mem`, in MiB.
const MIN_MEM_MIB: u64 = 256;
const MAX_MEM_MIB: u64 = 1 << 20;

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let task = match args.first().map(String::as_str) {
        Some("build") => Task::Build,
        Some("run") => Task::Run,
        Some("test") => Task::Test,
        Some("soak") => Task::Soak,
        Some("smoke") => Task::Smoke,
        Some("icount") => Task::Icount,
        other => {
            return Err(format!(
                "expected build, run, test, soak, smoke, icount, fluency, setup, model, bench or \
                 doctor, got {other:?}"
            ));
        }
    };
    let mut options = Options {
        task,
        release: false,
        spikes: task == Task::Test,
        accel: Accel::Auto,
        timeout: Duration::from_secs(match task {
            Task::Soak => SOAK_TIMEOUT_SECS,
            Task::Icount => ICOUNT_TIMEOUT_SECS,
            _ => DEFAULT_TIMEOUT_SECS,
        }),
        llm: None,
        model: None,
        edit: false,
        model_disk: None,
        local: None,
        store: None,
        sizing: Sizing::default(),
        shift: crate::icount::DEFAULT_SHIFT,
        boots: crate::icount::DEFAULT_BOOTS,
        wsl: false,
        workload: None,
        shard: None,
    };
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--release" => options.release = true,
            "--spikes" => options.spikes = true,
            "--accel" => {
                let name = rest.next().map(String::as_str);
                options.wsl = name == Some("wsl");
                options.accel = match name {
                    Some("auto") => Accel::Auto,
                    Some("kvm" | "wsl") => Accel::Kvm,
                    Some("whpx") => Accel::Whpx,
                    Some("tcg") => Accel::Tcg,
                    other => {
                        return Err(format!(
                            "--accel expects auto|kvm|whpx|tcg|wsl, got {other:?}"
                        ));
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
            "--local" if task == Task::Run => {
                let id = rest
                    .next()
                    .ok_or("--local expects a catalog id, such as qwen3-0.6b")?;
                options.local = Some(id.clone());
            }
            "--workload" if task == Task::Run => {
                let mix = rest.next().filter(|mix| matches!(mix.as_str(), "A" | "B"));
                let mix = mix.ok_or("--workload expects the mix A or B")?;
                options.workload = Some((mix.clone(), 1));
            }
            "--seed" if task == Task::Run && options.workload.is_some() => {
                let seed = rest.next().and_then(|s| s.parse::<u64>().ok());
                let seed = seed.ok_or("--seed expects a number")?;
                options.workload.as_mut().expect("checked above").1 = seed;
            }
            "--store" if task == Task::Run => {
                let dir = rest.next().ok_or("--store expects a directory")?;
                options.store = Some(dir.clone());
            }
            "--cpus" if task != Task::Build && task != Task::Smoke => {
                let cpus = rest
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| (1..=MAX_VM_CPUS).contains(n))
                    .ok_or(format!("--cpus expects 1 to {MAX_VM_CPUS}"))?;
                options.sizing.cpus = Some(cpus);
            }
            "--mem" if task != Task::Build && task != Task::Smoke => {
                let size = rest
                    .next()
                    .ok_or("--mem expects a size such as 6G or 2048M")?;
                options.sizing.mem_mib = Some(parse_mem(size)?);
            }
            "--shift" if task == Task::Icount => {
                options.shift = rest
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|&shift| shift <= crate::icount::MAX_SHIFT)
                    .ok_or(format!(
                        "--shift expects 0 to {}: above that the 1 kHz tick cannot keep up",
                        crate::icount::MAX_SHIFT
                    ))?;
            }
            "--boots" if task == Task::Icount => {
                options.boots = rest
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&boots| boots >= 2)
                    .ok_or("--boots expects a number of at least 2")?;
            }
            "--shard" if task == Task::Test => {
                let text = rest.next().ok_or("--shard expects K/N, such as 1/3")?;
                options.shard = Some(parse_shard(text)?);
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
    if options.local.is_some() && options.model_disk.is_some() {
        return Err("--local and --model-disk both name the model disk; give one".into());
    }
    if options.store.is_some() && options.local.is_none() {
        return Err("--store names where --local finds its model; give --local too".into());
    }
    if task == Task::Icount && matches!(options.accel, Accel::Kvm | Accel::Whpx) {
        return Err(
            "icount measures under single-threaded TCG only; --accel kvm or whpx is refused".into(),
        );
    }
    if task == Task::Icount && (options.llm.is_some() || options.model.is_some()) {
        return Err("icount boots with no network card, so --llm and --model are refused".into());
    }
    Ok(options)
}

/// `K/N` with 1 <= K <= N.
fn parse_shard(text: &str) -> Result<(usize, usize), String> {
    text.split_once('/')
        .and_then(|(k, n)| Some((k.parse::<usize>().ok()?, n.parse::<usize>().ok()?)))
        .filter(|&(k, n)| k >= 1 && k <= n)
        .ok_or_else(|| format!("--shard expects K/N with 1 <= K <= N, got {text:?}"))
}

/// Whether test case `index` (from 0) boots in `shard`: the cases are dealt out
/// in turn, so each is in exactly one of the N shards, and all are without one.
pub fn in_shard(shard: Option<(usize, usize)>, index: usize) -> bool {
    shard.is_none_or(|(k, n)| index % n == k - 1)
}

/// A memory size in MiB: `2048`, `2048M` or `6G` (any case).
fn parse_mem(text: &str) -> Result<u64, String> {
    let lower = text.to_ascii_lowercase();
    let (digits, scale) = match lower.strip_suffix('g') {
        Some(digits) => (digits, 1024),
        None => (lower.strip_suffix('m').unwrap_or(&lower), 1),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .filter(|mib| (MIN_MEM_MIB..=MAX_MEM_MIB).contains(mib))
        .ok_or(format!(
            "--mem expects {MIN_MEM_MIB}M to {}G, such as 6G or 2048M, got {text:?}",
            MAX_MEM_MIB / 1024
        ))
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
    fn workload_names_a_mix_for_run_only() {
        let options = parse_args(&args(&["run", "--workload", "A", "--seed", "4"])).unwrap();
        assert_eq!(options.workload_args(), " workload=A seed=4");
        assert_eq!(parse_args(&args(&["run"])).unwrap().workload_args(), "");
        assert!(parse_args(&args(&["run", "--workload", "C"])).is_err());
        assert!(parse_args(&args(&["run", "--seed", "4"])).is_err());
        assert!(parse_args(&args(&["test", "--workload", "A"])).is_err());
    }

    #[test]
    fn shards_split_the_cases_exactly_once() {
        let options = parse_args(&args(&["test", "--shard", "2/3"])).unwrap();
        assert_eq!(options.shard, Some((2, 3)));
        for bad in ["0/3", "4/3", "3", "a/b", "1/0"] {
            assert!(
                parse_args(&args(&["test", "--shard", bad])).is_err(),
                "{bad}"
            );
        }
        assert!(parse_args(&args(&["run", "--shard", "1/2"])).is_err());
        for index in 0..40 {
            let hits = (1..=3).filter(|&k| in_shard(Some((k, 3)), index)).count();
            assert_eq!(hits, 1);
            assert!(in_shard(None, index));
        }
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
    fn local_names_a_catalog_model_for_run_only() {
        let options = parse_args(&args(&["run", "--local", "qwen3-0.6b"])).unwrap();
        assert_eq!(options.local.as_deref(), Some("qwen3-0.6b"));
        assert_eq!(options.model, None);
        assert!(parse_args(&args(&["run", "--local"])).is_err());
        assert!(parse_args(&args(&["test", "--local", "qwen3-0.6b"])).is_err());
        let both = ["run", "--local", "qwen3-0.6b", "--model-disk", "m.gguf"];
        assert!(parse_args(&args(&both)).is_err());
        let stored = ["run", "--local", "qwen3-0.6b", "--store", r"E:\models"];
        assert_eq!(
            parse_args(&args(&stored)).unwrap().store.as_deref(),
            Some(r"E:\models")
        );
        assert!(parse_args(&args(&["run", "--store", "E:/m"])).is_err());
        // --model still names the server's model, beside a local one.
        let server = [
            "run",
            "--local",
            "qwen3-0.6b",
            "--llm",
            "10.0.0.2:8080",
            "--model",
            "q",
        ];
        let options = parse_args(&args(&server)).unwrap();
        assert_eq!(options.model.as_deref(), Some("q"));
    }

    #[test]
    fn cpus_and_mem_size_the_vm_for_run_and_test() {
        let options = parse_args(&args(&["test", "--cpus", "8", "--mem", "6G"])).unwrap();
        assert_eq!(
            options.sizing,
            Sizing {
                cpus: Some(8),
                mem_mib: Some(6144)
            }
        );
        let run = parse_args(&args(&["run", "--mem", "2048"])).unwrap();
        assert_eq!(run.sizing.mem_mib, Some(2048));
        assert_eq!(parse_mem("512m"), Ok(512));
        assert_eq!(
            parse_args(&args(&["run"])).unwrap().sizing,
            Sizing::default()
        );
        for bad in [
            ["--cpus", "0"],
            ["--cpus", "9"],
            ["--mem", "100M"],
            ["--mem", "x"],
        ] {
            assert!(
                parse_args(&args(&["run", bad[0], bad[1]])).is_err(),
                "{bad:?}"
            );
        }
        assert!(parse_args(&args(&["build", "--cpus", "2"])).is_err());
        assert!(parse_args(&args(&["run", "--mem"])).is_err());
    }

    #[test]
    fn icount_takes_shift_and_boots_and_refuses_kvm() {
        let options = parse_args(&args(&["icount", "--shift", "6", "--boots", "3"])).unwrap();
        assert_eq!(options.task, Task::Icount);
        assert_eq!((options.shift, options.boots), (6, 3));
        assert!(!options.spikes);
        let defaults = parse_args(&args(&["icount"])).unwrap();
        assert_eq!((defaults.shift, defaults.boots), (3, 2));
        assert_eq!(defaults.timeout, Duration::from_secs(ICOUNT_TIMEOUT_SECS));
        assert!(parse_args(&args(&["icount", "--accel", "tcg"])).is_ok());
        assert!(parse_args(&args(&["icount", "--accel", "kvm"])).is_err());
        assert!(parse_args(&args(&["icount", "--accel", "whpx"])).is_err());
        assert!(parse_args(&args(&["icount", "--shift", "7"])).is_err());
        assert!(parse_args(&args(&["icount", "--model", "q"])).is_err());
        assert!(parse_args(&args(&["icount", "--boots", "1"])).is_err());
        assert!(parse_args(&args(&["icount", "--llm", "10.0.2.2:80"])).is_err());
        assert!(parse_args(&args(&["test", "--shift", "3"])).is_err());
    }

    #[test]
    fn accel_wsl_is_kvm_inside_wsl() {
        let options = parse_args(&args(&["test", "--accel", "wsl"])).unwrap();
        assert!(options.wsl);
        assert_eq!(options.accel, Accel::Kvm);
        assert!(parse_args(&args(&["run", "--accel", "wsl"])).unwrap().wsl);
        // The last --accel wins, and plain kvm is the host's own.
        let later = parse_args(&args(&["run", "--accel", "wsl", "--accel", "tcg"])).unwrap();
        assert!(!later.wsl);
        assert_eq!(later.accel, Accel::Tcg);
        assert!(!parse_args(&args(&["run", "--accel", "kvm"])).unwrap().wsl);
    }

    #[test]
    fn rejects_unknown_task_and_options() {
        let unknown = parse_args(&args(&["deploy"])).unwrap_err();
        assert!(
            unknown.contains("icount") && !unknown.contains("  "),
            "{unknown}"
        );
        assert!(parse_args(&args(&["run", "--fast"])).is_err());
        assert!(parse_args(&args(&["run", "--accel", "hyperv"])).is_err());
    }
}
