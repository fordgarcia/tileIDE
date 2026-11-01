// SPDX-License-Identifier: GPL-3.0-or-later
//! Installing and removing extensions: copy a folder, or clone a git URL
//! (`git clone --depth 1`), into `<extensions dir>/<id>/`. Nothing is run while
//! installing. Blocking: the app calls these off the main thread.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Installed, MANIFEST, read_installed};

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
        // Symlinks are skipped: an extension must not point outside its folder.
    }
    Ok(())
}

/// Moves a checked folder in place as `<root>/<id>`, replacing an older copy if asked.
fn settle(staged: &Path, root: &Path, replace: bool) -> Result<Installed, String> {
    let manifest = read_installed(staged)?.manifest;
    let target = root.join(&manifest.id);
    if target.exists() {
        if !replace {
            let _ = std::fs::remove_dir_all(staged);
            return Err(format!("\"{}\" is already installed", manifest.id));
        }
        std::fs::remove_dir_all(&target).map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
    }
    std::fs::rename(staged, &target).map_err(|e| format!("cannot install into {}: {e}", target.display()))?;
    Ok(Installed { dir: target, manifest })
}

fn staging(root: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let dir = root.join(format!(".install-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    Ok(dir)
}

/// Copies the extension folder `from` (it must hold an `extension.json`).
pub fn from_folder(from: &Path, root: &Path, replace: bool) -> Result<Installed, String> {
    if !from.join(MANIFEST).is_file() {
        return Err(format!("{} has no {MANIFEST}", from.display()));
    }
    // Checked before copying, so a bad manifest leaves nothing behind.
    read_installed(from)?;
    let staged = staging(root)?;
    if let Err(e) = copy_dir(from, &staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("copy failed: {e}"));
    }
    settle(&staged, root, replace)
}

/// Clones `url` (shallow) and installs it.
pub fn from_git(url: &str, root: &Path, replace: bool) -> Result<Installed, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("no URL".into());
    }
    let staged = staging(root)?;
    let output = Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", "--"])
        .arg(url)
        .arg(&staged)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&staged);
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git clone failed: {}", reason.trim()));
    }
    let _ = std::fs::remove_dir_all(staged.join(".git"));
    settle(&staged, root, replace).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staged);
    })
}

pub fn remove(installed: &Installed) -> Result<(), String> {
    std::fs::remove_dir_all(&installed.dir).map_err(|e| format!("cannot remove {}: {e}", installed.dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tileide-ext-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn installs_replaces_and_removes_a_folder() {
        let base = scratch("install");
        let src = base.join("src");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::write(src.join(MANIFEST), r#"{ "id": "demo", "run": ["./run.sh"] }"#).unwrap();
        std::fs::write(src.join("lib/x.txt"), "x").unwrap();
        let root = base.join("extensions");
        let installed = from_folder(&src, &root, false).unwrap();
        assert_eq!(installed.dir, root.join("demo"));
        assert!(root.join("demo/lib/x.txt").is_file());
        assert!(from_folder(&src, &root, false).unwrap_err().contains("already installed"));
        from_folder(&src, &root, true).unwrap();
        // No staging folders left behind.
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        remove(&installed).unwrap();
        assert!(!root.join("demo").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn refuses_a_folder_without_a_valid_manifest() {
        let base = scratch("bad");
        std::fs::create_dir_all(&base).unwrap();
        assert!(from_folder(&base, &base.join("x"), false).unwrap_err().contains("has no"));
        std::fs::write(base.join(MANIFEST), "{").unwrap();
        assert!(from_folder(&base, &base.join("x"), false).is_err());
        assert!(!base.join("x").exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
