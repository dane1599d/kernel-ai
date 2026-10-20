//! Where model files live. The store stays off C:, which is nearly full on the
//! dev PC, and out of OneDrive, which would sync 640 MB files to the cloud. It is
//! `--store DIR`, else `KAI_MODELS`, else `D:\kai-models` on Windows and
//! `~/kai-models` elsewhere.

use std::env;
use std::path::{Path, PathBuf};

pub const ENV: &str = "KAI_MODELS";
const WINDOWS_DEFAULT: &str = "D:\\kai-models";

/// The store directory, made absolute and checked, but not created.
pub fn resolve(flag: Option<&str>) -> Result<PathBuf, String> {
    let chosen = match (flag, env::var(ENV)) {
        (Some(dir), _) => PathBuf::from(dir),
        (None, Ok(dir)) if !dir.is_empty() => PathBuf::from(dir),
        _ if cfg!(windows) => PathBuf::from(WINDOWS_DEFAULT),
        _ => home()
            .ok_or("no home directory; pass --store DIR")?
            .join("kai-models"),
    };
    let path = std::path::absolute(&chosen)
        .map_err(|e| format!("cannot resolve {}: {e}", chosen.display()))?;
    check(&path, &onedrive_roots())?;
    // A junction or link on D: may still lead to C: or OneDrive.
    if let Some(real) = deepest_real(&path) {
        check(&real, &onedrive_roots())?;
    }
    Ok(path)
}

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Where OneDrive keeps its folders, from the variables its client sets.
fn onedrive_roots() -> Vec<PathBuf> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(env::var_os)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The canonical form of the longest part of `path` that exists.
fn deepest_real(path: &Path) -> Option<PathBuf> {
    path.ancestors().find_map(|p| p.canonicalize().ok())
}

/// Refuses a store on drive C: or inside OneDrive. Works on the path's text, so
/// Windows paths are judged the same on every host.
pub fn check(path: &Path, onedrive: &[PathBuf]) -> Result<(), String> {
    let text = path.to_string_lossy();
    let fix = "pass --store D:\\kai-models (or set KAI_MODELS) for a folder on another drive";
    let lower = plain_path(&text);
    // A share or device path may still be C: (\\localhost\C$), so only drives pass.
    if lower.starts_with("//") || lower.starts_with("unc/") || lower.starts_with("globalroot") {
        return Err(format!(
            "{} is a network or device path, which may be C:; {fix}",
            path.display()
        ));
    }
    // WSL mounts C: at /mnt/c.
    let wsl_c = lower == "/mnt/c" || lower.starts_with("/mnt/c/");
    if drive(&text) == Some('C') || wsl_c {
        return Err(format!(
            "{} is on C:, which is nearly full; {fix}",
            path.display()
        ));
    }
    let in_onedrive = lower
        .split('/')
        .any(|part| part == "onedrive" || part.starts_with("onedrive - "))
        || onedrive.iter().any(|root| {
            let root = plain_path(&root.to_string_lossy());
            let root = root.trim_end_matches('/');
            !root.is_empty() && (lower == root || lower.starts_with(&format!("{root}/")))
        });
    if in_onedrive {
        return Err(format!(
            "{} is inside OneDrive, which would sync the model to the cloud; {fix}",
            path.display()
        ));
    }
    Ok(())
}

/// A path's text in lowercase with `/` separators and no `\\?\` prefix, so a
/// canonical path compares with the one typed.
fn plain_path(text: &str) -> String {
    let text = text.to_lowercase().replace('\\', "/");
    match text.strip_prefix("//?/") {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// The drive letter of a Windows path, in capitals: `C:\x`, `\\?\C:\x` and
/// `//?/c:/x` all give `C`.
fn drive(text: &str) -> Option<char> {
    let rest = text
        .strip_prefix("\\\\?\\")
        .or_else(|| text.strip_prefix("//?/"))
        .unwrap_or(text);
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(path: &str) -> bool {
        check(
            Path::new(path),
            &[PathBuf::from("E:\\Sync\\OneDrive Personal")],
        )
        .is_err()
    }

    #[test]
    fn stores_on_c_or_in_onedrive_are_refused() {
        assert!(refused("C:\\kai-models"));
        assert!(refused("c:/kai-models"));
        assert!(refused("\\\\?\\C:\\Users\\me\\models"));
        assert!(refused("D:\\Users\\me\\OneDrive\\models"));
        assert!(refused("D:\\OneDrive - Contoso\\models"));
        assert!(refused("/home/me/OneDrive/models"));
        assert!(refused("e:\\sync\\onedrive personal\\models"));
        assert!(refused("E:\\Sync\\OneDrive Personal"));
        assert!(!refused("D:\\kai-models"));
        assert!(!refused("\\\\?\\D:\\kai-models"));
        assert!(!refused("E:\\Sync\\OneDrive Personal Backup\\models"));
        assert!(!refused("D:\\models\\onedrive-notes"));
        assert!(!refused("/home/me/kai-models"));
        // A canonical path, as resolving a junction gives, is still refused.
        assert!(refused("\\\\?\\E:\\Sync\\OneDrive Personal\\models"));
        // Shares, device paths and WSL's view of C: are refused too.
        assert!(refused("\\\\localhost\\C$\\kai-models"));
        assert!(refused("\\\\?\\UNC\\localhost\\C$\\kai-models"));
        assert!(refused("\\\\?\\GLOBALROOT\\Device\\HarddiskVolume3\\m"));
        assert!(refused("/mnt/c/kai-models"));
        assert!(!refused("/mnt/d/kai-models"));
        let message = check(Path::new("C:\\m"), &[]).unwrap_err();
        assert!(message.contains("--store D:\\kai-models"), "{message}");
    }

    #[test]
    fn drive_letters_are_read_from_text() {
        assert_eq!(drive("C:\\x"), Some('C'));
        assert_eq!(drive("d:/x"), Some('D'));
        assert_eq!(drive("\\\\?\\c:\\x"), Some('C'));
        assert_eq!(drive("/c/x"), None);
        assert_eq!(drive("relative"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_into_onedrive_is_refused() {
        let base = std::env::temp_dir().join(format!("kai-store-link-{}", std::process::id()));
        let real = base.join("OneDrive").join("models");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(base.join("OneDrive"), &link).unwrap();
        let flag = link.join("models").to_string_lossy().into_owned();
        assert!(resolve(Some(&flag)).unwrap_err().contains("OneDrive"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_store_flag_is_made_absolute_and_checked() {
        let dir = std::env::temp_dir().join("kai-store-test");
        let flag = dir.to_string_lossy().into_owned();
        // The temp directory may itself be on C: on Windows; then the refusal
        // is the expected answer.
        match resolve(Some(&flag)) {
            Ok(path) => assert!(path.is_absolute() && path.ends_with("kai-store-test")),
            Err(message) => assert!(message.contains("C:") || message.contains("OneDrive")),
        }
        assert!(resolve(Some("C:\\models")).is_err() || !cfg!(windows));
    }
}
