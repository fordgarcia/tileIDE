// SPDX-License-Identifier: GPL-3.0-or-later
//! Command line: `tileide [OPTIONS] [PATH]`, like `code .`. Pure parsing (no GTK), so
//! it is unit-tested; `main` acts on the result.

use std::path::{Path, PathBuf};

use crate::background;

pub const HELP: &str = "\
TileIDE: a tiling, riceable code editor

Usage: tileide [OPTIONS] [PATH]

  PATH                 folder to open, or a file to open in an editor tile
                       (its folder becomes the workspace). Default: current directory

Options:
  -w, --windowed       open as a normal window (default: full-screen)
  -f, --fullscreen     open full-screen
      --wait           stay attached to the terminal until TileIDE closes
                       (default: return to the shell right away, like `code`)
      --solid          opaque colours, for compositors without transparency
      --check-config [FILE]
                       load the config (default ~/.config/tileide/tileide.lua),
                       print errors and warnings, exit 0 if it is valid
  -h, --help           print this help
  -V, --version        print the version

Full-screen background (ignored when windowed; overrides tileide.background):
      --bg MODE        see-through (default) | image:FILE | solid:RRGGBB |
                       gradient:RRGGBB,RRGGBB
      --dim N          darken 0..100 (raised automatically if text gets hard to read)
      --tint RRGGBB@N  colour overlay of strength 0..100
      --blur PX        blur an image background
      --fit MODE       cover (default) | contain | tile
      --tile-opacity N tile background opacity 88..100
      --no-auto-dim    never dim automatically

Keys (Alt is TileIDE's modifier; all can be rebound with tileide.bind):
  Alt+Return terminal   Alt+E new file      Alt+W close tile    Ctrl+S save
  Alt+B files drawer    Alt+C config        Alt+Z word wrap     Alt+1..9, 0 workspaces
  Alt+H/J/K/L focus     Alt+Shift+H/J/K/L resize   Alt+Ctrl+Shift+H/J/K/L move
  Alt+V dwindle/columns Alt+S next tile below      Alt+O overview  Alt+T themes
  Alt+Escape quit

Config lives in ~/.config/tileide/ (created from a sample on first run); saved
changes apply at once.";

/// Flags that take a value; anything else starting with `-` is a switch.
const WITH_VALUE: &[&str] = &["--bg", "--dim", "--tint", "--blur", "--fit", "--tile-opacity", "--seconds", "--bench-drawer"];
const SWITCHES: &[&str] = &["-w", "--windowed", "-f", "--fullscreen", "--wait", "--foreground", "--solid", "--overlay", "--no-auto-dim"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Windowed,
    Fullscreen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub mode: Mode,
    /// Root of the Files drawer and where terminals start.
    pub folder: PathBuf,
    /// A file given on the command line, opened in the first tile.
    pub open: Option<PathBuf>,
    /// Don't detach from the terminal (`--wait`; `--foreground` is the detached child's flag).
    pub foreground: bool,
    /// The drawer pushes the window aside (`--overlay` covers it instead).
    pub push: bool,
    pub solid: bool,
    pub seconds: Option<u32>,
    pub bench_toggles: Option<u32>,
    /// Set only when background flags were given; they then win over `tileide.background`.
    pub background: Option<background::Settings>,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    /// Offscreen PNG of the canvas (development: `tileide render OUT.png DRAWER [flags]`).
    Render(Vec<String>),
    /// `--check-config [FILE]`.
    CheckConfig(Option<PathBuf>),
    Run(Box<Options>),
}

/// Flags that set the full-screen background.
const BACKGROUND_FLAGS: &[&str] = &["--bg", "--dim", "--tint", "--blur", "--fit", "--tile-opacity", "--no-auto-dim"];

/// `cwd` resolves a relative PATH; it is a parameter so tests need no process state.
pub fn parse(args: &[String], cwd: &Path) -> Result<Command, String> {
    if args.first().map(String::as_str) == Some("render") {
        return Ok(Command::Render(args[1..].to_vec()));
    }
    if args.first().map(String::as_str) == Some("--check-config") {
        return match args.get(1..) {
            Some([]) | None => Ok(Command::CheckConfig(None)),
            Some([file]) => Ok(Command::CheckConfig(Some(cwd.join(file)))),
            Some(_) => Err("--check-config takes at most one FILE".into()),
        };
    }
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            _ if WITH_VALUE.contains(&arg) => {
                if args.get(i + 1).is_none() {
                    return Err(format!("{arg} needs a value"));
                }
                i += 1;
            }
            _ if SWITCHES.contains(&arg) => {}
            _ if arg.starts_with('-') && arg != "-" => return Err(format!("unknown option '{arg}' (see tileide --help)")),
            _ if path.is_some() => return Err(format!("only one PATH can be opened (got '{arg}' too)")),
            _ => path = Some(PathBuf::from(arg)),
        }
        i += 1;
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let number = |flag: &str| value(flag).map(|n| n.parse::<u32>().map_err(|_| format!("{flag} needs a number"))).transpose();

    let target = cwd.join(path.unwrap_or_else(|| PathBuf::from(".")));
    let target = target.canonicalize().map_err(|e| format!("{}: {e}", target.display()))?;
    let (folder, open) = if target.is_dir() {
        (target, None)
    } else {
        let parent = target.parent().map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        (parent, Some(target))
    };
    // The later of --windowed / --fullscreen wins, like most CLIs.
    let last = |flags: &[&str]| args.iter().rposition(|a| flags.contains(&a.as_str()));
    let mode = match (last(&["-w", "--windowed"]), last(&["-f", "--fullscreen"])) {
        (Some(w), Some(f)) if w > f => Mode::Windowed,
        (Some(_), None) => Mode::Windowed,
        _ => Mode::Fullscreen,
    };
    Ok(Command::Run(Box::new(Options {
        mode,
        folder,
        open,
        foreground: has("--wait") || has("--foreground"),
        push: !has("--overlay"),
        solid: has("--solid"),
        seconds: number("--seconds")?,
        bench_toggles: number("--bench-drawer")?,
        background: match args.iter().any(|a| BACKGROUND_FLAGS.contains(&a.as_str())) {
            true => Some(background::Settings::from_args(args)?),
            false => None,
        },
    })))
}

/// The user's config file; on first run the sample config is copied there, so the
/// Config workspace edits the user's own copy. Falls back to the sample read-only.
pub fn config_path(config_home: &Path, samples: &[PathBuf]) -> PathBuf {
    let dir = config_home.join("tileide");
    let file = dir.join("tileide.lua");
    if file.exists() {
        return file;
    }
    let Some(sample) = samples.iter().find(|s| s.join("tileide.lua").is_file()) else { return file };
    match copy_missing(sample, &dir) {
        Ok(()) => {
            eprintln!("tileide: created {} from the sample config", dir.display());
            file
        }
        Err(e) => {
            eprintln!("tileide: cannot create {}: {e}; opening the sample read-only", dir.display());
            sample.join("tileide.lua")
        }
    }
}

/// Copies `from` into `to` recursively; never overwrites a file that already exists.
fn copy_missing(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_missing(&entry.path(), &target)?;
        } else if !target.exists() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tileide-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn run(a: &str, cwd: &Path) -> Options {
        match parse(&args(a), cwd).unwrap() {
            Command::Run(o) => *o,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn defaults_to_cwd_fullscreen_detached() {
        let dir = temp("defaults");
        let o = run("", &dir);
        assert_eq!((o.mode, o.folder.as_path(), o.open.as_ref(), o.foreground), (Mode::Fullscreen, dir.as_path(), None, false));
        assert_eq!(run(".", &dir).folder, dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_opens_in_its_folder() {
        let dir = temp("file");
        std::fs::write(dir.join("main.rs"), "").unwrap();
        let o = run("main.rs --windowed --wait", &dir);
        assert_eq!(o.folder, dir);
        assert_eq!(o.open, Some(dir.join("main.rs")));
        assert_eq!((o.mode, o.foreground), (Mode::Windowed, true));
        assert_eq!(run("-w -f", &dir).mode, Mode::Fullscreen, "last one wins");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flag_values_are_not_paths() {
        let dir = temp("values");
        let o = run("--bg solid:112233 --dim 20", &dir);
        assert_eq!(o.folder, dir, "'solid:112233' and '20' are values, not PATHs");
        assert!((o.background.as_ref().unwrap().dim - 0.2).abs() < 1e-9);
        assert_eq!(run("", &dir).background, None, "no flags: the config decides");
        assert_eq!(parse(&args("--check-config"), &dir).unwrap(), Command::CheckConfig(None));
        assert_eq!(parse(&args("--check-config x.lua"), &dir).unwrap(), Command::CheckConfig(Some(dir.join("x.lua"))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_bad_input() {
        let dir = temp("bad");
        assert!(parse(&args("--nope"), &dir).is_err());
        assert!(parse(&args("a b"), &dir).is_err());
        assert!(parse(&args("missing-folder"), &dir).unwrap_err().contains("missing-folder"));
        assert!(parse(&args("--dim"), &dir).is_err());
        assert_eq!(parse(&args("--help"), &dir).unwrap(), Command::Help);
        assert_eq!(parse(&args("-V"), &dir).unwrap(), Command::Version);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn first_run_copies_the_sample_without_overwriting() {
        let root = temp("config");
        let sample = root.join("sample");
        std::fs::create_dir_all(sample.join("rices")).unwrap();
        std::fs::write(sample.join("tileide.lua"), "-- sample").unwrap();
        std::fs::write(sample.join("rices/nord.lua"), "-- nord").unwrap();
        let home = root.join("home");

        let path = config_path(&home, &[root.join("absent"), sample.clone()]);
        assert_eq!(path, home.join("tileide/tileide.lua"));
        assert_eq!(std::fs::read_to_string(home.join("tileide/rices/nord.lua")).unwrap(), "-- nord");

        std::fs::write(&path, "-- mine").unwrap();
        std::fs::remove_file(home.join("tileide/rices/nord.lua")).unwrap();
        config_path(&home, &[sample]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "-- mine", "an existing config is never touched");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
