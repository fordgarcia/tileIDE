// SPDX-License-Identifier: GPL-3.0-or-later
//! Extensions (v0.3 phase 8, spec §6.5): separate processes the app talks to over
//! JSON-RPC (`host`), found as folders with an `extension.json` (`manifest`), started
//! only by their activation events (`should_start`), measured by the weight meter
//! (`meter`). Nothing here touches GTK.

pub mod host;
pub mod install;
pub mod manifest;
pub mod meter;

use std::path::{Path, PathBuf};

pub use manifest::{Activation, Manifest};
pub use tileide_core::extension::Mode;

pub const MANIFEST: &str = "extension.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

pub fn read_installed(dir: &Path) -> Result<Installed, String> {
    let path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest = manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Installed { dir: dir.to_path_buf(), manifest })
}

/// Every extension under `root` (one folder each), by name; broken ones are reported.
/// Two folders with the same id: the first by folder name wins.
pub fn discover(root: &Path) -> (Vec<Installed>, Vec<String>) {
    let (mut found, mut problems) = (Vec::<Installed>::new(), Vec::new());
    let Ok(entries) = std::fs::read_dir(root) else { return (found, problems) };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
    dirs.sort();
    for dir in dirs {
        match read_installed(&dir) {
            Ok(i) if found.iter().any(|f| f.manifest.id == i.manifest.id) => problems.push(format!("{}: id \"{}\" is used twice; ignored", dir.display(), i.manifest.id)),
            Ok(i) => found.push(i),
            Err(e) => problems.push(format!("extension {e}")),
        }
    }
    found.sort_by_key(|i| i.manifest.name.to_lowercase());
    (found, problems)
}

/// Something that may start an extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// TileIDE started (in the active workspace).
    Startup,
    /// A workspace became active.
    WorkspaceEntered,
    /// A file of this GtkSourceView language opened.
    Language(&'a str),
    /// One of its commands was run.
    Command(&'a str),
}

/// Lazy activation: an extension starts only on an event it asked for, or on entering
/// a workspace where it is "Always here". Disabled or "Off here" wins over everything.
pub fn should_start(m: &Manifest, enabled: bool, mode: Mode, event: Event) -> bool {
    if !enabled || mode == Mode::Off {
        return false;
    }
    match event {
        Event::Startup => mode == Mode::On || m.starts_always(),
        Event::WorkspaceEntered => mode == Mode::On,
        Event::Language(lang) => m.activation.iter().any(|a| matches!(a, Activation::Language(l) if l == lang)),
        Event::Command(id) => m.commands.iter().any(|c| c.id == id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(activation: &str) -> Manifest {
        manifest::parse(&format!(r#"{{ "id": "x", "run": ["x"], "activation": [{activation}], "commands": [{{ "id": "x.go" }}] }}"#)).unwrap()
    }

    #[test]
    fn starts_only_on_its_events() {
        let m = manifest(r#""onLanguage:rust""#);
        assert!(should_start(&m, true, Mode::OnDemand, Event::Language("rust")));
        assert!(!should_start(&m, true, Mode::OnDemand, Event::Language("python3")));
        assert!(!should_start(&m, true, Mode::OnDemand, Event::Startup));
        assert!(!should_start(&m, true, Mode::OnDemand, Event::WorkspaceEntered));
        assert!(should_start(&m, true, Mode::OnDemand, Event::Command("x.go")));
        assert!(should_start(&m, true, Mode::On, Event::WorkspaceEntered));
        assert!(should_start(&manifest(r#""*""#), true, Mode::OnDemand, Event::Startup));
    }

    #[test]
    fn disabled_and_off_win() {
        let m = manifest(r#""*""#);
        for event in [Event::Startup, Event::WorkspaceEntered, Event::Command("x.go")] {
            assert!(!should_start(&m, false, Mode::On, event));
            assert!(!should_start(&m, true, Mode::Off, event));
        }
    }

    #[test]
    fn discovers_and_reports() {
        let root = std::env::temp_dir().join(format!("tileide-ext-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, text) in [("b", r#"{ "id": "b", "name": "Beta", "run": ["x"] }"#), ("a", r#"{ "id": "a", "name": "alpha", "run": ["x"] }"#), ("bad", "{"), ("dup", r#"{ "id": "a", "run": ["x"] }"#)] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(MANIFEST), text).unwrap();
        }
        std::fs::create_dir_all(root.join(".install-1")).unwrap();
        let (found, problems) = discover(&root);
        assert_eq!(found.iter().map(|i| i.manifest.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}
