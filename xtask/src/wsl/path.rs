//! Turning a Windows QEMU command line into one QEMU inside WSL can run: Windows
//! paths become the distro's (`D:\x` is `/mnt/d/x`), and guest memory is checked
//! against what WSL's VM has. Pure functions, tested on any host.

/// Memory WSL keeps for itself and QEMU's own overhead, beyond the guest's RAM.
pub const HEADROOM_MIB: u64 = 1024;

/// A Windows path as the distro sees it: a drive path under the mount root
/// (`/mnt/` unless /etc/wsl.conf moves it), or a path inside the distro itself
/// (`\\wsl.localhost\Ubuntu\home\me`). Network shares cannot be reached.
pub fn to_wsl(path: &str, mount_root: &str, distro: &str) -> Result<String, String> {
    let text = path.replace('/', "\\");
    let text = match text.strip_prefix("\\\\?\\UNC\\") {
        Some(rest) => format!("\\\\{rest}"),
        None => text.strip_prefix("\\\\?\\").unwrap_or(&text).to_string(),
    };
    let lower = text.to_ascii_lowercase();
    for share in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
        if lower.starts_with(share) {
            let rest = &text[share.len()..];
            let (name, inner) = rest.split_once('\\').unwrap_or((rest, ""));
            if !name.eq_ignore_ascii_case(distro) {
                return Err(format!(
                    "{path} is inside the WSL distro {name}, not {distro}"
                ));
            }
            return Ok(format!("/{}", inner.replace('\\', "/")));
        }
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let letter = char::from(bytes[0].to_ascii_lowercase());
        let rest = text[2..].trim_start_matches('\\').replace('\\', "/");
        let root = format!("{}/{letter}", mount_root.trim_end_matches('/'));
        return Ok(if rest.is_empty() {
            root
        } else {
            format!("{root}/{rest}")
        });
    }
    Err(format!(
        "{path} is not on a drive WSL mounts; use a path on a local drive such as D:"
    ))
}

/// The QEMU arguments with each model disk's file (the JSON `-blockdev` that
/// `disk.rs` writes) renamed by `rename`. Everything else is relative to
/// `target/` or already a Linux path, and passes unchanged.
pub fn translate_args(
    args: Vec<String>,
    mut rename: impl FnMut(&str) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(args.len());
    let mut blockdev = false;
    for arg in args {
        let next = if blockdev && arg.starts_with('{') {
            let mut spec: serde_json::Value =
                serde_json::from_str(&arg).map_err(|e| format!("bad -blockdev {arg}: {e}"))?;
            if let Some(name) = spec["file"]["filename"].as_str() {
                spec["file"]["filename"] = rename(name)?.into();
            }
            spec.to_string()
        } else {
            arg
        };
        blockdev = next == "-blockdev";
        out.push(next);
    }
    Ok(out)
}

/// Refuses a guest bigger than WSL's VM can hold, with the `.wslconfig` line
/// that makes room. `mem_kib` is WSL's MemTotal.
pub fn check_memory(args: &[String], mem_kib: Option<u64>, wslconfig: &str) -> Result<(), String> {
    let (Some(total_kib), Some(asked)) = (mem_kib, guest_mib(args)) else {
        return Ok(());
    };
    let total = total_kib / 1024;
    if asked + HEADROOM_MIB <= total {
        return Ok(());
    }
    let gb = (asked + HEADROOM_MIB).div_ceil(1024) + 1;
    Err(format!(
        "the VM needs {asked} MiB, but WSL has {total} MiB, and keeps {HEADROOM_MIB} MiB for \
         itself. Give WSL more: add memory={gb}GB under [wsl2] in {wslconfig}, then run \
         wsl --shutdown"
    ))
}

/// Guest memory in MiB from QEMU's `-m 1408M` (or `-m 2G`).
pub fn guest_mib(args: &[String]) -> Option<u64> {
    let value = args.windows(2).find(|w| w[0] == "-m")?[1].to_ascii_uppercase();
    match value.strip_suffix('G') {
        Some(gb) => gb.parse::<u64>().ok().map(|g| g * 1024),
        None => value.strip_suffix('M').unwrap_or(&value).parse().ok(),
    }
}

/// Model disks at least this big are copied into the distro's own disk.
pub const CACHE_MIN_BYTES: u64 = 64 << 20;

/// Whether a model disk is worth copying into the distro: big, and on a Windows
/// drive (a file already inside the distro is read where it is).
pub fn should_cache(windows: &str, len: u64) -> bool {
    let plain = windows.strip_prefix(r"\\?\").unwrap_or(windows).as_bytes();
    let on_drive = plain.len() >= 2 && plain[0].is_ascii_alphabetic() && plain[1] == b':';
    on_drive && len >= CACHE_MIN_BYTES
}

/// The folder holding `distro`'s virtual disk, from
/// `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss /s`: blocks of
/// `DistributionName` and `BasePath` values under one key each.
pub fn base_path(reg: &str, distro: &str) -> Option<String> {
    let mut name = None;
    let mut base = None;
    for line in reg.lines().map(str::trim) {
        if line.starts_with("HKEY_") {
            if name
                .as_deref()
                .is_some_and(|n: &str| n.eq_ignore_ascii_case(distro))
                && base.is_some()
            {
                return base;
            }
            (name, base) = (None, None);
            continue;
        }
        let words: Vec<&str> = line.splitn(3, "    ").map(str::trim).collect();
        if let [key, "REG_SZ", value] = words.as_slice() {
            match *key {
                "DistributionName" => name = Some(value.to_string()),
                "BasePath" => base = Some(value.to_string()),
                _ => {}
            }
        }
    }
    name.filter(|n| n.eq_ignore_ascii_case(distro)).and(base)
}

/// `wsl.exe`'s arguments: QEMU run in the distro, in `target`. A boot whose
/// console xtask reads (`limit` seconds at most) goes through `sh`, which sends
/// QEMU's errors to the same pipe and ends it with `timeout` should xtask die.
pub fn argv(
    distro: &str,
    target: &str,
    qemu: &str,
    limit: Option<u64>,
    args: Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = ["-d", distro, "--cd", target, "-e"]
        .map(String::from)
        .to_vec();
    if let Some(secs) = limit {
        out.extend(
            ["sh", "-c", "exec timeout -k 5 \"$@\" 2>&1", "sh"]
                .map(String::from)
                .into_iter()
                .chain([secs.to_string()]),
        );
    }
    out.push(qemu.to_string());
    out.extend(args);
    out
}
