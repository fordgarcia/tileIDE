// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → Share → Export setup (v0.5 phase 4b, the export half of v0.7's rice
//! sharing): `tileide.lua`, `state.lua`, the user's themes and rices, the background
//! images they use and a README with the extension list, as one `.tar.gz` whose top
//! folder is `tileide/`, so unpacking it in `~/.config` restores the setup.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What went into the archive, for the message shown afterwards.
#[derive(Debug, PartialEq)]
pub struct Exported {
    pub files: usize,
    /// Background images that were set but not found.
    pub missing: Vec<PathBuf>,
}

/// Packs the setup in `config_dir` into `out`. `images` are the background images in
/// use (copied to `tileide/backgrounds/`); `extensions` are the installed ones' names.
pub fn export(config_dir: &Path, images: &[PathBuf], extensions: &[String], out: &Path) -> Result<Exported, String> {
    let staging = std::env::temp_dir().join(format!("tileide-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = pack(config_dir, images, extensions, out, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn pack(config_dir: &Path, images: &[PathBuf], extensions: &[String], out: &Path, staging: &Path) -> Result<Exported, String> {
    let top = staging.join("tileide");
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    std::fs::create_dir_all(&top).map_err(|e| io(&top, e))?;
    let mut files = 0;
    for name in ["tileide.lua", "state.lua"] {
        let from = config_dir.join(name);
        if from.is_file() {
            std::fs::copy(&from, top.join(name)).map_err(|e| io(&from, e))?;
            files += 1;
        }
    }
    // Only `.lua` files: these folders are data, and nothing else is read from them.
    for folder in ["themes", "rices"] {
        let Ok(entries) = std::fs::read_dir(config_dir.join(folder)) else { continue };
        let target = top.join(folder);
        std::fs::create_dir_all(&target).map_err(|e| io(&target, e))?;
        for path in entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "lua")) {
            std::fs::copy(&path, target.join(path.file_name().unwrap_or_default())).map_err(|e| io(&path, e))?;
            files += 1;
        }
    }
    let mut missing = Vec::new();
    let mut copied = Vec::new();
    for image in images {
        let Some(name) = image.file_name() else { continue };
        if !image.is_file() {
            missing.push(image.clone());
            continue;
        }
        let target = top.join("backgrounds");
        std::fs::create_dir_all(&target).map_err(|e| io(&target, e))?;
        if !target.join(name).exists() {
            std::fs::copy(image, target.join(name)).map_err(|e| io(image, e))?;
            files += 1;
            copied.push(image.clone());
        }
    }
    std::fs::write(top.join("README.txt"), readme(&copied, extensions)).map_err(|e| io(&top, e))?;
    let output = Command::new("bsdtar").arg("-czf").arg(out).arg("-C").arg(staging).arg("tileide").output().map_err(|e| format!("cannot run bsdtar: {e}"))?;
    if !output.status.success() {
        return Err(format!("bsdtar: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(Exported { files, missing })
}

fn readme(images: &[PathBuf], extensions: &[String]) -> String {
    let mut text = format!(
        "TileIDE setup, exported by TileIDE {}.\n\n\
         To use it, back up ~/.config/tileide first, then unpack this archive in ~/.config:\n\
         \x20   tar -xzf <this file> -C ~/.config\n\n\
         tileide.lua is your config, state.lua the choices made in Settings, themes/ and\n\
         rices/ your own themes and layouts.\n",
        env!("CARGO_PKG_VERSION")
    );
    if !images.is_empty() {
        text += "\nBackground images are in backgrounds/. The setup still points at their old\nplaces; put them back there or pick them again in Settings → Themes & background:\n";
        for i in images {
            text += &format!("    {}\n", i.display());
        }
    }
    if !extensions.is_empty() {
        text += "\nExtensions installed (not included; install them again):\n";
        for e in extensions {
            text += &format!("    {e}\n");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_config_themes_and_images() {
        let base = std::env::temp_dir().join(format!("tileide-export-test-{}", std::process::id()));
        let config = base.join("config");
        std::fs::create_dir_all(config.join("themes")).unwrap();
        std::fs::write(config.join("tileide.lua"), "-- config").unwrap();
        std::fs::write(config.join("themes/mine.lua"), "return {}").unwrap();
        std::fs::write(config.join("themes/notes.txt"), "skipped").unwrap();
        std::fs::write(base.join("wall.png"), "png").unwrap();
        let out = base.join("setup.tar.gz");
        let images = [base.join("wall.png"), base.join("gone.png")];
        let exported = export(&config, &images, &["git-blame".into()], &out).unwrap();
        assert_eq!(exported, Exported { files: 3, missing: vec![base.join("gone.png")] });
        let list = Command::new("bsdtar").arg("-tf").arg(&out).output().unwrap();
        let mut names: Vec<String> = String::from_utf8_lossy(&list.stdout).lines().map(|l| l.trim_end_matches('/').to_string()).collect();
        names.sort();
        assert_eq!(names, ["tileide", "tileide/README.txt", "tileide/backgrounds", "tileide/backgrounds/wall.png", "tileide/themes", "tileide/themes/mine.lua", "tileide/tileide.lua"]);
        let readme = Command::new("bsdtar").arg("-xOf").arg(&out).arg("tileide/README.txt").output().unwrap();
        assert!(String::from_utf8_lossy(&readme.stdout).contains("git-blame"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
