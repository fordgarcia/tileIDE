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

/// Other names `tileide.lsp` accepts.
const ALIASES: &[(&str, &str)] = &[("cpp", "c"), ("c++", "c"), ("clangd", "c"), ("javascript", "typescript"), ("js", "typescript"), ("ts", "typescript"), ("py", "python")];

pub fn by_name(name: &str) -> Option<&'static Server> {
    let name = ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(name)).map_or(name, |(_, n)| n);
    SERVERS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// The server and language id for a file, by extension.
pub fn for_file(path: &Path) -> Option<(&'static Server, &'static str)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    SERVERS.iter().find_map(|s| s.languages.iter().find(|(e, _)| *e == ext).map(|(_, id)| (s, *id)))
}

/// The command to start `server`: the override if given, else the first candidate
/// `exists` accepts (normally `on_path`).
pub fn command(server: &Server, override_cmd: Option<&[String]>, exists: impl Fn(&str) -> bool) -> Option<Vec<String>> {
    if let Some(cmd) = override_cmd {
        return Some(cmd.to_vec());
    }
    server.candidates.iter().find(|c| exists(c[0])).map(|c| c.iter().map(|s| s.to_string()).collect())
}

/// An executable file named `program` in a `PATH` directory (or the path itself).
pub fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
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
        assert_eq!(command(python, None, |p| p == "pylsp"), Some(vec!["pylsp".to_string()]));
        assert_eq!(command(python, None, |p| p != "basedpyright-langserver"), Some(vec!["pyright-langserver".into(), "--stdio".into()]));
        assert_eq!(command(python, None, |_| false), None);
        let mine = vec!["my-ls".to_string()];
        assert_eq!(command(python, Some(&mine), |_| false), Some(mine));
        assert!(on_path("sh") && !on_path("tileide-no-such-program"));
    }
}
