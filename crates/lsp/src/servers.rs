// SPDX-License-Identifier: GPL-3.0-or-later
//! The built-in language servers (v0.3: Python, JS/TS, C/C++, Java, Rust). TileIDE
//! doesn't install them; it starts the first candidate found on `PATH`, or the
//! command `tileide.lsp(name, { cmd = … })` gives.

use std::path::Path;

pub struct Server {
    /// The name `tileide.lsp` uses.
    pub name: &'static str,
    /// What the user sees ("Python").
    pub label: &'static str,
    /// File extension → LSP language id.
    pub languages: &'static [(&'static str, &'static str)],
    /// Tried in order; the first whose program is on `PATH` wins.
    pub candidates: &'static [&'static [&'static str]],
    /// How to get one, for the "no server found" notice.
    pub install: &'static str,
}

pub const SERVERS: &[Server] = &[
    Server {
        name: "rust",
        label: "Rust",
        languages: &[("rs", "rust")],
        candidates: &[&["rust-analyzer"]],
        install: "rustup component add rust-analyzer",
    },
    Server {
        name: "c",
        label: "C/C++",
        languages: &[("c", "c"), ("h", "c"), ("cc", "cpp"), ("cpp", "cpp"), ("cxx", "cpp"), ("hpp", "cpp"), ("hh", "cpp"), ("hxx", "cpp")],
        candidates: &[&["clangd"]],
        install: "install clangd (Arch: pacman -S clang)",
    },
    Server {
        name: "python",
        label: "Python",
        languages: &[("py", "python"), ("pyi", "python")],
        candidates: &[&["basedpyright-langserver", "--stdio"], &["pyright-langserver", "--stdio"], &["pylsp"]],
        install: "install pyright (npm i -g pyright) or python-lsp-server",
    },
    Server {
        name: "typescript",
        label: "JavaScript/TypeScript",
        languages: &[("js", "javascript"), ("mjs", "javascript"), ("cjs", "javascript"), ("jsx", "javascriptreact"), ("ts", "typescript"), ("mts", "typescript"), ("cts", "typescript"), ("tsx", "typescriptreact")],
        candidates: &[&["typescript-language-server", "--stdio"]],
        install: "npm i -g typescript typescript-language-server",
    },
    Server {
        name: "java",
        label: "Java",
        languages: &[("java", "java")],
        candidates: &[&["jdtls"]],
        install: "install jdtls (Arch: AUR jdtls)",
    },
];

/// Servers that run beside a language's main one and only add diagnostics (and
/// formatting): linters. Optional: a missing one is mentioned once, nothing breaks.
pub const LINTERS: &[Server] = &[Server {
    name: "ruff",
    label: "Ruff (Python linter)",
    languages: &[("py", "python"), ("pyi", "python")],
    candidates: &[&["ruff", "server"]],
    install: "pacman -S ruff, or pip install ruff in the project's virtualenv",
}];

/// Other names `tileide.lsp` accepts.
const ALIASES: &[(&str, &str)] = &[("cpp", "c"), ("c++", "c"), ("clangd", "c"), ("javascript", "typescript"), ("js", "typescript"), ("ts", "typescript"), ("py", "python")];

pub fn by_name(name: &str) -> Option<&'static Server> {
    let name = ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(name)).map_or(name, |(_, n)| n);
    SERVERS.iter().chain(LINTERS).find(|s| s.name.eq_ignore_ascii_case(name))
}

/// The linters for a file, by extension.
pub fn linters_for(path: &Path) -> Vec<&'static Server> {
    let Some(ext) = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) else { return Vec::new() };
    LINTERS.iter().filter(|s| s.languages.iter().any(|(e, _)| *e == ext)).collect()
}

/// The server and language id for a file, by extension.
pub fn for_file(path: &Path) -> Option<(&'static Server, &'static str)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    SERVERS.iter().find_map(|s| s.languages.iter().find(|(e, _)| *e == ext).map(|(_, id)| (s, *id)))
}

/// The command to start `server`: the override if given, else the first candidate
/// whose program `find` locates (normally `locate`), with the program as found.
pub fn command(server: &Server, override_cmd: Option<&[String]>, find: impl Fn(&str) -> Option<String>) -> Option<Vec<String>> {
    if let Some(cmd) = override_cmd {
        return Some(cmd.to_vec());
    }
    server.candidates.iter().find_map(|c| {
        let program = find(c[0])?;
        Some(std::iter::once(program).chain(c[1..].iter().map(|s| s.to_string())).collect())
    })
}

/// `program` in one of `first` (the project's virtualenvs: their tools match the project),
/// else on `PATH`, else in one of `extra` (a full path for both folder lists). Apps
/// started from a desktop launcher often get a shorter `PATH` than a shell: no
/// version-manager tool folders, no npm globals.
pub fn locate(program: &str, first: &[std::path::PathBuf], extra: &[std::path::PathBuf]) -> Option<String> {
    let in_dirs = |dirs: &[std::path::PathBuf]| dirs.iter().map(|dir| dir.join(program)).find(|p| executable(p)).map(|p| p.to_string_lossy().into_owned());
    in_dirs(first).or_else(|| on_path(program).then(|| program.to_string())).or_else(|| in_dirs(extra))
}

/// Folders where user-installed servers usually live, best first: `~/.local/bin`,
/// npm/cargo globals, and every tool version mise installed (newest name first).
pub fn user_dirs(home: Option<&Path>) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    let Some(home) = home else { return dirs };
    for d in [".local/bin", ".npm-global/bin", ".cargo/bin", ".local/share/pnpm", ".bun/bin"] {
        dirs.push(home.join(d));
    }
    // ~/.local/share/mise/installs/<tool>/<version>/bin
    let mise = home.join(".local/share/mise/installs");
    let mut versions: Vec<std::path::PathBuf> = std::fs::read_dir(&mise)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|tool| std::fs::read_dir(tool.path()).into_iter().flatten().flatten().map(|v| v.path()))
        .filter(|p| !p.is_symlink())
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    dirs.extend(versions.into_iter().map(|v| v.join("bin")).filter(|b| b.is_dir()));
    dirs
}

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// An executable file named `program` in a `PATH` directory (or the path itself).
pub fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return executable(Path::new(program));
    }
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| executable(&dir.join(program))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_map_to_servers_and_language_ids() {
        assert_eq!(for_file(Path::new("/x/main.rs")).map(|(s, id)| (s.name, id)), Some(("rust", "rust")));
        assert_eq!(for_file(Path::new("a.HPP")).map(|(s, id)| (s.name, id)), Some(("c", "cpp")));
        assert_eq!(for_file(Path::new("App.tsx")).map(|(s, id)| (s.name, id)), Some(("typescript", "typescriptreact")));
        assert!(for_file(Path::new("notes.md")).is_none());
        assert!(for_file(Path::new("Makefile")).is_none());
    }

    #[test]
    fn names_and_aliases_resolve() {
        assert_eq!(by_name("cpp").map(|s| s.name), Some("c"));
        assert_eq!(by_name("Python").map(|s| s.name), Some("python"));
        assert!(by_name("cobol").is_none());
    }

    #[test]
    fn first_installed_candidate_or_the_override_wins() {
        let python = by_name("python").unwrap();
        let only = |name: &'static str| move |p: &str| (p == name).then(|| format!("/opt/{p}"));
        assert_eq!(command(python, None, only("pylsp")), Some(vec!["/opt/pylsp".to_string()]));
        assert_eq!(command(python, None, only("pyright-langserver")), Some(vec!["/opt/pyright-langserver".into(), "--stdio".into()]));
        assert_eq!(command(python, None, |_| None), None);
        let mine = vec!["my-ls".to_string()];
        assert_eq!(command(python, Some(&mine), |_| None), Some(mine));
        assert!(on_path("sh") && !on_path("tileide-no-such-program"));
        assert_eq!(locate("sh", &[], &[]).as_deref(), Some("sh"));
        let bin = std::env::temp_dir().join(format!("tileide-locate-{}", std::process::id()));
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("tileide-test-ls");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(locate("tileide-test-ls", &[], std::slice::from_ref(&bin)), Some(tool.to_string_lossy().into_owned()));
        let venv_sh = bin.join("sh");
        std::fs::write(&venv_sh, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&venv_sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(locate("sh", std::slice::from_ref(&bin), &[]), Some(venv_sh.to_string_lossy().into_owned()), "a virtualenv's tool beats PATH's");
        assert_eq!(locate("sh", &[], std::slice::from_ref(&bin)).as_deref(), Some("sh"), "other user folders don't");
        std::fs::remove_dir_all(&bin).unwrap();
    }

    #[test]
    fn python_files_get_the_linter() {
        assert_eq!(linters_for(Path::new("a.py")).iter().map(|s| s.name).collect::<Vec<_>>(), vec!["ruff"]);
        assert!(linters_for(Path::new("a.rs")).is_empty());
        assert_eq!(by_name("ruff").map(|s| s.name), Some("ruff"));
        assert_eq!(for_file(Path::new("a.py")).map(|(s, _)| s.name), Some("python"), "the linter never replaces the main server");
    }
}
