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

const JS_TS: &[(&str, &str)] = &[("js", "javascript"), ("mjs", "javascript"), ("cjs", "javascript"), ("jsx", "javascriptreact"), ("ts", "typescript"), ("mts", "typescript"), ("cts", "typescript"), ("tsx", "typescriptreact")];

/// Servers that run beside a language's main one and only add diagnostics (and
/// formatting): linters. Optional: a missing one is mentioned once, nothing breaks.
/// JS/TS gets Biome, or ESLint in a project configured for it (`linters_for`).
pub const LINTERS: &[Server] = &[
    Server {
        name: "ruff",
        label: "Ruff (Python linter)",
        languages: &[("py", "python"), ("pyi", "python")],
        candidates: &[&["ruff", "server"]],
        install: "pacman -S ruff, or pip install ruff in the project's virtualenv",
    },
    Server { name: "biome", label: "Biome (JS/TS linter)", languages: JS_TS, candidates: &[&["biome", "lsp-proxy"]], install: "npm i -g @biomejs/biome" },
    Server {
        name: "eslint",
        label: "ESLint",
        languages: JS_TS,
        candidates: &[&["vscode-eslint-language-server", "--stdio"]],
        install: "npm i -g vscode-langservers-extracted (and eslint in the project)",
    },
];

/// Formatters run as a command (the text on stdin, formatted on stdout). When one is
/// installed it formats instead of the language server.
pub struct Formatter {
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    pub command: &'static [&'static str],
}

pub const FORMATTERS: &[Formatter] = &[Formatter { label: "google-java-format", extensions: &["java"], command: &["google-java-format", "-"] }];

/// The command-line formatter for a file, by extension (installed or not).
pub fn formatter_for(path: &Path) -> Option<&'static Formatter> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    FORMATTERS.iter().find(|f| f.extensions.contains(&ext.as_str()))
}

/// Whether `root` (or a folder above it) configures ESLint.
pub fn uses_eslint(root: &Path) -> bool {
    const FILES: &[&str] = &[
        "eslint.config.js", "eslint.config.mjs", "eslint.config.cjs", "eslint.config.ts", "eslint.config.mts", "eslint.config.cts",
        ".eslintrc", ".eslintrc.js", ".eslintrc.cjs", ".eslintrc.json", ".eslintrc.yml", ".eslintrc.yaml",
    ];
    root.ancestors().any(|d| {
        FILES.iter().any(|f| d.join(f).is_file()) || std::fs::read_to_string(d.join("package.json")).is_ok_and(|p| p.contains("\"eslintConfig\""))
    })
}

/// Other names `tileide.lsp` accepts.
const ALIASES: &[(&str, &str)] = &[("cpp", "c"), ("c++", "c"), ("clangd", "c"), ("javascript", "typescript"), ("js", "typescript"), ("ts", "typescript"), ("py", "python")];

pub fn by_name(name: &str) -> Option<&'static Server> {
    let name = ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(name)).map_or(name, |(_, n)| n);
    SERVERS.iter().chain(LINTERS).find(|s| s.name.eq_ignore_ascii_case(name))
}

/// The linters for a file in the project at `root`, by extension.
pub fn linters_for(path: &Path, root: &Path) -> Vec<&'static Server> {
    let Some(ext) = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) else { return Vec::new() };
    let mut eslint = None;
    LINTERS
        .iter()
        .filter(|s| s.languages.iter().any(|(e, _)| *e == ext))
        .filter(|s| match s.name {
            "eslint" => *eslint.get_or_insert_with(|| uses_eslint(root)),
            "biome" => !*eslint.get_or_insert_with(|| uses_eslint(root)),
            _ => true,
        })
        .collect()
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

/// The TypeScript a project uses and how to serve it.
#[derive(Debug, PartialEq)]
pub enum TypeScript {
    /// TypeScript 6 and older: typescript-language-server over `tsserver.js`, given
    /// when it's a global one (the server finds a project's own, never a global one).
    Classic(Option<std::path::PathBuf>),
    /// TypeScript 7 (native): no `tsserver.js`; its own `tsc --lsp --stdio` serves.
    Native(std::path::PathBuf),
}

/// The project's own TypeScript (`node_modules` here or above), else the one behind a
/// `tsc` in one of `dirs` (npm links `bin/tsc` to `<typescript>/bin/tsc`).
pub fn typescript(root: &Path, dirs: &[std::path::PathBuf]) -> Option<TypeScript> {
    let own = root.ancestors().map(|d| d.join("node_modules/typescript")).find(|p| p.is_dir());
    let global = || dirs.iter().filter_map(|d| std::fs::canonicalize(d.join("tsc")).ok()).find_map(|tsc| Some(tsc.parent()?.parent()?.to_path_buf()));
    let (package, is_own) = match own {
        Some(p) => (p, true),
        None => (global()?, false),
    };
    let tsserver = package.join("lib/tsserver.js");
    if tsserver.is_file() {
        return Some(TypeScript::Classic((!is_own).then_some(tsserver)));
    }
    let tsc = package.join("bin/tsc");
    tsc.is_file().then_some(TypeScript::Native(tsc))
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
    fn typescript_versions_pick_their_server() {
        let dir = std::env::temp_dir().join(format!("tileide-ts-{}", std::process::id()));
        let (project, global) = (dir.join("proj"), dir.join("global/lib/node_modules/typescript"));
        let bin = dir.join("global/bin");
        for d in [&project, &global.join("bin"), &global.join("lib"), &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(global.join("bin/tsc"), "").unwrap();
        std::os::unix::fs::symlink(global.join("bin/tsc"), bin.join("tsc")).unwrap();
        let tsc = std::fs::canonicalize(global.join("bin/tsc")).unwrap();
        // A global TypeScript 7: only bin/tsc.
        assert_eq!(typescript(&project, std::slice::from_ref(&bin)), Some(TypeScript::Native(tsc)));
        // A global TypeScript 6: its tsserver.js is passed on.
        std::fs::write(global.join("lib/tsserver.js"), "").unwrap();
        let tsserver = std::fs::canonicalize(global.join("lib/tsserver.js")).unwrap();
        assert_eq!(typescript(&project, std::slice::from_ref(&bin)), Some(TypeScript::Classic(Some(tsserver))));
        // The project's own wins, and the server finds it by itself.
        std::fs::create_dir_all(project.join("node_modules/typescript/lib")).unwrap();
        std::fs::write(project.join("node_modules/typescript/lib/tsserver.js"), "").unwrap();
        assert_eq!(typescript(&project, std::slice::from_ref(&bin)), Some(TypeScript::Classic(None)));
        assert_eq!(typescript(&dir.join("elsewhere"), &[]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn js_projects_get_biome_or_their_eslint() {
        let dir = std::env::temp_dir().join(format!("tileide-lint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let names = |root: &Path| linters_for(Path::new("a.ts"), root).iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(&dir), ["biome"]);
        std::fs::write(dir.join("eslint.config.js"), "").unwrap();
        assert_eq!(names(&dir), ["eslint"]);
        assert_eq!(linters_for(Path::new("a.py"), &dir).iter().map(|s| s.name).collect::<Vec<_>>(), ["ruff"]);
        assert_eq!(formatter_for(Path::new("A.java")).map(|f| f.label), Some("google-java-format"));
        assert!(formatter_for(Path::new("a.ts")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(linters_for(Path::new("a.py"), Path::new("/")).iter().map(|s| s.name).collect::<Vec<_>>(), vec!["ruff"]);
        assert!(linters_for(Path::new("a.rs"), Path::new("/")).is_empty());
        assert_eq!(by_name("ruff").map(|s| s.name), Some("ruff"));
        assert_eq!(for_file(Path::new("a.py")).map(|(s, _)| s.name), Some("python"), "the linter never replaces the main server");
    }
}
