//! Path translation and the QEMU command line for `--accel wsl`, on any host.

use super::path::{argv, base_path, check_memory, guest_mib, should_cache, to_wsl, translate_args};

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn windows_paths_become_the_distros() {
    let wsl = |path: &str| to_wsl(path, "/mnt/", "Ubuntu");
    assert_eq!(
        wsl("D:\\kai-targets\\local-vm-doctor").unwrap(),
        "/mnt/d/kai-targets/local-vm-doctor"
    );
    // canonicalize's prefix, forward slashes, spaces and commas.
    assert_eq!(
        wsl("\\\\?\\C:\\Users\\user\\Projects\\Kernal AI\\target").unwrap(),
        "/mnt/c/Users/user/Projects/Kernal AI/target"
    );
    assert_eq!(
        wsl("d:/kai-models/a b,c.gguf").unwrap(),
        "/mnt/d/kai-models/a b,c.gguf"
    );
    assert_eq!(wsl("E:\\").unwrap(), "/mnt/e");
    // A file inside the distro itself.
    assert_eq!(
        wsl("\\\\wsl.localhost\\Ubuntu\\home\\dane1\\m.gguf").unwrap(),
        "/home/dane1/m.gguf"
    );
    assert_eq!(wsl("\\\\?\\UNC\\wsl$\\ubuntu\\tmp\\x").unwrap(), "/tmp/x");
    assert!(wsl("\\\\wsl.localhost\\Debian\\home\\x").is_err());
    // Shares and relative paths are not on a drive WSL mounts.
    assert!(wsl("\\\\server\\share\\m.gguf").is_err());
    assert!(wsl("target\\esp").is_err());
    // /etc/wsl.conf may mount drives elsewhere.
    assert_eq!(to_wsl("D:\\x", "/", "Ubuntu").unwrap(), "/d/x");
    assert_eq!(to_wsl("D:\\x", "/win/", "Ubuntu").unwrap(), "/win/d/x");
}

#[test]
fn only_the_model_disks_file_is_renamed() {
    let spec = r#"{"node-name":"kai-model","driver":"raw","read-only":true,"file":{"driver":"file","filename":"D:\\kai-models\\Qwen3-0.6B-Q8_0.gguf","read-only":true}}"#;
    let args = strings(&[
        "-m",
        "1408M",
        "-drive",
        "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd",
        "-drive",
        "format=raw,file=fat:rw:esp",
        "-blockdev",
        spec,
        "-device",
        "virtio-blk-pci,drive=kai-model,serial=kai-model",
    ]);
    let out = translate_args(args.clone(), |p| to_wsl(p, "/mnt/", "Ubuntu")).unwrap();
    assert_eq!(out.len(), args.len());
    let value: serde_json::Value = serde_json::from_str(&out[7]).unwrap();
    assert_eq!(
        value["file"]["filename"],
        "/mnt/d/kai-models/Qwen3-0.6B-Q8_0.gguf"
    );
    assert_eq!(value["read-only"], true);
    assert_eq!(value["file"]["read-only"], true);
    assert_eq!(out[..7], args[..7]);
    assert_eq!(out[8..], args[8..]);
    // A path WSL cannot reach stops the boot.
    let share = spec.replace("D:\\\\kai-models", "\\\\\\\\server\\\\share");
    let bad = strings(&["-blockdev", &share]);
    assert!(translate_args(bad, |p| to_wsl(p, "/mnt/", "Ubuntu")).is_err());
}

#[test]
fn guest_memory_must_fit_in_wsl() {
    let wsl_kib = Some(14_132_508); // 13,801 MiB, the founder's WSL.
    let config = "C:\\Users\\user\\.wslconfig";
    assert_eq!(guest_mib(&strings(&["-m", "1408M"])), Some(1408));
    assert_eq!(guest_mib(&strings(&["-m", "6G"])), Some(6144));
    assert_eq!(guest_mib(&strings(&["-smp", "4"])), None);
    assert!(check_memory(&strings(&["-m", "1408M"]), wsl_kib, config).is_ok());
    assert!(check_memory(&strings(&["-m", "12777M"]), wsl_kib, config).is_ok());
    let error = check_memory(&strings(&["-m", "12778M"]), wsl_kib, config).unwrap_err();
    assert!(
        error.contains("add memory=15GB under [wsl2] in C:\\Users\\user\\.wslconfig"),
        "{error}"
    );
    // Unknown memory on either side is not a reason to refuse.
    assert!(check_memory(&strings(&["-m", "99999M"]), None, config).is_ok());
}

#[test]
fn big_models_on_a_drive_are_cached_and_the_vhdx_is_found() {
    assert!(should_cache(r"D:\kai-models\m.gguf", 639_446_688));
    assert!(should_cache(r"\\?\D:\m.gguf", 64 << 20));
    assert!(!should_cache(r"D:\kai\fixtures\model.gguf", 3_000_000));
    assert!(!should_cache(
        r"\\wsl.localhost\Ubuntu\home\m.gguf",
        1 << 30
    ));
    // What `reg query ...\Lxss /s` printed on the founder's PC.
    let reg = r"
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Lxss\{2b34}
    BasePath    REG_SZ    D:\WSL\Ubuntu
    DistributionName    REG_SZ    Ubuntu

HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Lxss\{6a16}
    BasePath    REG_SZ    \\?\C:\Users\user\AppData\Local\Docker\wsl\main
    DistributionName    REG_SZ    docker-desktop
";
    assert_eq!(base_path(reg, "ubuntu").as_deref(), Some(r"D:\WSL\Ubuntu"));
    assert!(
        base_path(reg, "docker-desktop")
            .unwrap()
            .ends_with(r"wsl\main")
    );
    assert_eq!(base_path(reg, "Debian"), None);
}

#[test]
fn a_test_boot_runs_under_timeout_with_errors_on_the_console_pipe() {
    let args = strings(&["-m", "512M"]);
    let run = argv(
        "Ubuntu",
        "/mnt/d/t",
        "/usr/bin/qemu-system-x86_64",
        None,
        args.clone(),
    );
    assert_eq!(
        run,
        strings(&[
            "-d",
            "Ubuntu",
            "--cd",
            "/mnt/d/t",
            "-e",
            "/usr/bin/qemu-system-x86_64",
            "-m",
            "512M"
        ])
    );
    let test = argv(
        "Ubuntu",
        "/mnt/d/t",
        "/usr/bin/qemu-system-x86_64",
        Some(960),
        args,
    );
    assert_eq!(
        test[5..],
        strings(&[
            "sh",
            "-c",
            "exec timeout -k 5 \"$@\" 2>&1",
            "sh",
            "960",
            "/usr/bin/qemu-system-x86_64",
            "-m",
            "512M"
        ])
    );
}
