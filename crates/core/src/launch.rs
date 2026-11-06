// SPDX-License-Identifier: GPL-3.0-or-later
//! Launch configs (v0.6 phase 3): a named program with its arguments, environment and
//! working folder, shared by Run (`Alt+R`) and Debug (`F5`). They come from
//! `tileide.launch { … }` in `tileide.lua` and from a project's `.vscode/launch.json`.
//! Values may use VS Code's variables (`${workspaceFolder}`, `${file}`, `${env:HOME}` …);
//! a relative `program` or `cwd` is taken from the workspace folder, as VS Code does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Launch {
    pub name: String,
    /// A source file (run or debugged as `Alt+R`/`F5` would) or an executable.
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Default: where the file's own run would happen (its folder, or its Cargo project).
    pub cwd: Option<String>,
}

/// A launch config with its variables filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

/// What `${…}` variables refer to: the workspace folder and the focused file.
pub struct Context<'a> {
    pub folder: Option<&'a Path>,
    pub file: Option<&'a Path>,
}

impl Launch {
    pub fn resolve(&self, at: &Context) -> Result<Resolved, String> {
        let fill = |text: &str| substitute(text, at).map_err(|e| format!("{}: {e}", self.name));
        let absolute = |text: &str| -> Result<PathBuf, String> {
            let path = PathBuf::from(fill(text)?);
            Ok(match at.folder {
                Some(folder) if path.is_relative() => folder.join(path),
                _ => path,
            })
        };
        if self.program.is_empty() {
            return Err(format!("{}: no program", self.name));
        }
        Ok(Resolved {
            name: self.name.clone(),
            program: absolute(&self.program)?,
            args: self.args.iter().map(|a| fill(a)).collect::<Result<_, _>>()?,
            env: self.env.iter().map(|(k, v)| Ok((k.clone(), fill(v)?))).collect::<Result<_, String>>()?,
            cwd: self.cwd.as_deref().map(absolute).transpose()?,
        })
    }
}

/// Fills VS Code's variables in `text`. An unknown variable, or one that needs a file
/// or folder that isn't there, is an error (running with a literal `${file}` would only
/// confuse).
pub fn substitute(text: &str, at: &Context) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or_else(|| format!("unclosed ${{ in \"{text}\""))?;
        out.push_str(&variable(&after[..end], at)?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn variable(name: &str, at: &Context) -> Result<String, String> {
    if let Some(var) = name.strip_prefix("env:") {
        return Ok(std::env::var(var).unwrap_or_default());
    }
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let name_of = |p: &Path| p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let folder = || at.folder.ok_or_else(|| format!("${{{name}}} needs a workspace folder (Alt+D)"));
    let file = || at.file.ok_or_else(|| format!("${{{name}}} needs a focused file"));
    Ok(match name {
        "workspaceFolder" | "workspaceRoot" => text(folder()?),
        "workspaceFolderBasename" => name_of(folder()?),
        "file" => text(file()?),
        "fileBasename" => name_of(file()?),
        "fileBasenameNoExtension" => file()?.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        "fileExtname" => file()?.extension().map_or_else(String::new, |e| format!(".{}", e.to_string_lossy())),
        "fileDirname" => file()?.parent().map_or_else(String::new, text),
        "relativeFile" => {
            let file = file()?;
            text(at.folder.and_then(|f| file.strip_prefix(f).ok()).unwrap_or(file))
        }
        "userHome" => std::env::var("HOME").unwrap_or_default(),
        "cwd" => std::env::current_dir().map(|d| text(&d)).unwrap_or_default(),
        "pathSeparator" | "/" => "/".into(),
        other => return Err(format!("${{{other}}} is not a variable TileIDE knows")),
    })
}

/// Whether `path` is an executable file (not a source file to run through a runner).
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_variables_and_relative_paths() {
        let launch = Launch {
            name: "app".into(),
            program: "src/${fileBasename}".into(),
            args: vec!["--in".into(), "${workspaceFolder}/data".into(), "${fileBasenameNoExtension}${fileExtname}".into()],
            env: BTreeMap::from([("MODE".into(), "${workspaceFolderBasename}".into())]),
            cwd: Some("out".into()),
        };
        let at = Context { folder: Some(Path::new("/w/proj")), file: Some(Path::new("/w/proj/src/main.py")) };
        let r = launch.resolve(&at).unwrap();
        assert_eq!(r.program, PathBuf::from("/w/proj/src/main.py"));
        assert_eq!(r.args, ["--in", "/w/proj/data", "main.py"]);
        assert_eq!(r.env, [("MODE".to_string(), "proj".to_string())]);
        assert_eq!(r.cwd, Some(PathBuf::from("/w/proj/out")));
    }

    #[test]
    fn missing_context_and_unknown_variables_fail() {
        let none = Context { folder: None, file: None };
        assert!(substitute("${file}", &none).unwrap_err().contains("focused file"));
        assert!(substitute("${workspaceFolder}", &none).unwrap_err().contains("workspace folder"));
        assert!(substitute("${command:pickProcess}", &none).unwrap_err().contains("not a variable"));
        assert!(substitute("${oops", &none).is_err());
        assert_eq!(substitute("plain $HOME", &none).unwrap(), "plain $HOME");
    }
}
