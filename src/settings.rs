// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the config, theme and state files live, and how their contents become what
//! the GTK side uses: themes, background settings and key binds.

use std::path::{Path, PathBuf};

use gtk::gdk;
use gtk4 as gtk;
use tileide_config::model::{self, Action, Bind};
use tileide_config::{Limits, Loaded};
use tileide_core::theme::Theme;

use crate::background;

/// The packaged sample config, then the source tree's (for `cargo run`).
pub fn sample_dirs() -> [PathBuf; 2] {
    [PathBuf::from("/usr/share/tileide/config"), Path::new(env!("CARGO_MANIFEST_DIR")).join("config")]
}

pub struct Paths {
    /// `tileide.lua`.
    pub file: PathBuf,
    pub dir: PathBuf,
    /// `state.lua`: GUI choices (Q15).
    pub state: PathBuf,
    /// User themes first, so they shadow packaged ones of the same name.
    pub theme_dirs: Vec<PathBuf>,
}

impl Paths {
    pub fn new(file: PathBuf) -> Paths {
        let dir = file.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let mut theme_dirs = vec![dir.join("themes")];
        theme_dirs.extend(sample_dirs().iter().map(|d| d.join("themes")));
        Paths { state: dir.join("state.lua"), file, dir, theme_dirs }
    }

    pub fn load(&self) -> Result<Loaded, String> {
        tileide_config::load(&self.file, Limits::default())
    }

    /// Theme `name` (a file stem), or the built-in one with the reason it was not loaded.
    pub fn theme(&self, name: &str) -> (Theme, Option<String>) {
        match tileide_config::find_themes(&self.theme_dirs).get(name) {
            Some(path) => match tileide_config::load_theme(path) {
                Ok((theme, warnings)) => {
                    warnings.iter().for_each(|w| eprintln!("{w}"));
                    (theme, None)
                }
                Err(e) => (Theme::builtin(), Some(format!("theme \"{name}\": {e}; using the built-in theme"))),
            },
            None if name == DEFAULT_THEME => (Theme::builtin(), None),
            None => (Theme::builtin(), Some(format!("theme \"{name}\" not found in {}; using the built-in theme", self.dir.join("themes").display()))),
        }
    }

    /// Every theme that loads, by file stem. Broken files are reported and left out.
    pub fn all_themes(&self) -> Vec<(String, Theme)> {
        let mut out: Vec<(String, Theme)> = tileide_config::find_themes(&self.theme_dirs)
            .into_iter()
            .filter_map(|(key, path)| match tileide_config::load_theme(&path) {
                Ok((theme, _)) => Some((key, theme)),
                Err(e) => {
                    eprintln!("themes: {e}");
                    None
                }
            })
            .collect();
        if !out.iter().any(|(k, _)| k == DEFAULT_THEME) {
            out.insert(0, (DEFAULT_THEME.into(), Theme::builtin()));
        }
        out
    }
}

/// Used when neither the config, the workspace nor `state.lua` names a theme.
pub const DEFAULT_THEME: &str = "dusk";

/// `tileide.background` (plus a workspace's image override) as background settings,
/// through the same parser as the command line.
pub fn background(cfg: &model::Background, image: Option<&str>) -> Result<background::Settings, String> {
    let source = match (image, cfg.mode.as_str(), cfg.source.as_deref()) {
        (Some(path), _, _) => format!("image:{path}"),
        (None, "see-through", _) => "see-through".into(),
        (None, mode, Some(source)) => format!("{mode}:{source}"),
        (None, mode, None) => return Err(format!("background: mode \"{mode}\" needs a source")),
    };
    let args: Vec<String> = [
        ("--bg", source),
        ("--dim", cfg.dim.to_string()),
        ("--blur", cfg.blur.to_string()),
        ("--tile-opacity", (cfg.tile_opacity * 100.0).round().to_string()),
        ("--fit", cfg.fit.clone()),
    ]
    .into_iter()
    .flat_map(|(flag, value)| [flag.to_string(), value])
    .collect();
    background::Settings::from_args(&args)
}

pub struct KeyBind {
    pub mods: gdk::ModifierType,
    /// Lower-case keyval, as `key_pressed` sees it once modifiers are taken out.
    pub key: gdk::Key,
    pub action: Action,
}

/// The modifiers binds can use; others (Num Lock, Caps Lock) are ignored when matching.
pub const BIND_MODS: gdk::ModifierType =
    gdk::ModifierType::ALT_MASK.union(gdk::ModifierType::CONTROL_MASK).union(gdk::ModifierType::SHIFT_MASK).union(gdk::ModifierType::SUPER_MASK);

/// Resolves key names; unknown ones are reported and skipped.
pub fn key_binds(binds: &[Bind]) -> (Vec<KeyBind>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for bind in binds {
        let key = gdk::Key::from_name(&bind.key).or_else(|| gdk::Key::from_name(bind.key.to_lowercase()));
        let Some(key) = key.filter(|k| *k != gdk::Key::VoidSymbol) else {
            problems.push(format!("bind: unknown key \"{}\" (GDK key names, e.g. B, 1, Return, F5)", bind.key));
            continue;
        };
        let mods = bind.mods.iter().fold(gdk::ModifierType::empty(), |m, name| {
            m | match name.as_str() {
                "ALT" => gdk::ModifierType::ALT_MASK,
                "CTRL" => gdk::ModifierType::CONTROL_MASK,
                "SHIFT" => gdk::ModifierType::SHIFT_MASK,
                _ => gdk::ModifierType::SUPER_MASK,
            }
        });
        out.push(KeyBind { mods, key: key.to_lower(), action: bind.action.clone() });
    }
    (out, problems)
}

/// Where the theme comes from, highest priority first.
#[derive(Clone, Copy)]
pub struct ThemeSources<'a> {
    /// A Themes section row being previewed (hover or selection).
    pub preview: Option<&'a str>,
    /// `tileide.workspace(n, { theme = … })`: the config wins over GUI picks (Q15).
    pub config_workspace: Option<&'a str>,
    /// A Themes section pick for this workspace only (`state.lua`).
    pub picked_workspace: Option<&'a str>,
    /// A tray pick while the config pins the theme: lasts the session.
    pub session: Option<&'a str>,
    pub config: Option<&'a str>,
    pub state: Option<&'a str>,
}

pub fn theme_name(s: &ThemeSources) -> String {
    s.preview.or(s.config_workspace).or(s.picked_workspace).or(s.session).or(s.config).or(s.state).unwrap_or(DEFAULT_THEME).to_string()
}

/// Which background applies (full-screen only).
#[derive(Debug, PartialEq)]
pub enum BackgroundFrom<'a> {
    /// `tileide.background`, or the config's defaults; with the workspace's image if it has one.
    Config(Option<&'a str>),
    /// The picker's choice: for this workspace, this session, or saved for all.
    Picked(&'a model::Background),
    /// `--bg` and friends on the command line.
    CommandLine,
}

/// The background saved from Settings, unless `tileide.background` was edited after it
/// was picked (then the config's is the newer choice).
pub fn saved_background<'a>(state: &'a tileide_config::State, config: &tileide_config::Config) -> Option<&'a model::Background> {
    state.background.as_ref().filter(|_| !config.background_set || state.background_over.as_ref() == Some(&config.background))
}

/// The config's workspace image wins (Q15), then the picker's live or per-workspace
/// choice, then the command line, then the picker's saved choice, else
/// `tileide.background`. Editing `tileide.background` clears the saved choice
/// (`App::apply_config`), so the latest of the two wins.
pub fn background_from<'a>(workspace_image: Option<&'a str>, picked_workspace: Option<&'a model::Background>, session: Option<&'a model::Background>, command_line: bool, saved: Option<&'a model::Background>) -> BackgroundFrom<'a> {
    if workspace_image.is_some() {
        return BackgroundFrom::Config(workspace_image);
    }
    if let Some(b) = picked_workspace.or(session) {
        return BackgroundFrom::Picked(b);
    }
    if command_line {
        return BackgroundFrom::CommandLine;
    }
    match saved {
        Some(b) => BackgroundFrom::Picked(b),
        None => BackgroundFrom::Config(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_priority() {
        let none = ThemeSources { preview: None, config_workspace: None, picked_workspace: None, session: None, config: None, state: None };
        assert_eq!(theme_name(&none), "dusk");
        assert_eq!(theme_name(&ThemeSources { state: Some("s"), ..none }), "s");
        assert_eq!(theme_name(&ThemeSources { config: Some("c"), state: Some("s"), ..none }), "c", "the config wins over state.lua");
        assert_eq!(theme_name(&ThemeSources { session: Some("t"), config: Some("c"), ..none }), "t", "a tray pick lasts the session");
        assert_eq!(theme_name(&ThemeSources { picked_workspace: Some("p"), session: Some("t"), config: Some("c"), ..none }), "p", "a workspace pick beats global ones");
        assert_eq!(theme_name(&ThemeSources { config_workspace: Some("w"), picked_workspace: Some("p"), ..none }), "w", "the config's workspace theme wins over a pick");
        assert_eq!(theme_name(&ThemeSources { preview: Some("v"), config_workspace: Some("w"), ..none }), "v", "a preview shows over everything");
    }

    #[test]
    fn background_priority() {
        let a = tileide_config::Config::default().background;
        let b = model::Background { mode: "solid".into(), source: Some("112233".into()), ..a.clone() };
        assert_eq!(background_from(Some("/w.png"), Some(&a), Some(&a), true, Some(&a)), BackgroundFrom::Config(Some("/w.png")));
        assert_eq!(background_from(None, Some(&b), Some(&a), true, None), BackgroundFrom::Picked(&b), "a workspace pick beats the session's");
        assert_eq!(background_from(None, None, Some(&b), true, None), BackgroundFrom::Picked(&b), "a live pick beats --bg");
        assert_eq!(background_from(None, None, None, true, Some(&b)), BackgroundFrom::CommandLine, "--bg beats a saved pick");
        assert_eq!(background_from(None, None, None, false, Some(&b)), BackgroundFrom::Picked(&b), "a saved pick beats tileide.background");
        assert_eq!(background_from(None, None, None, false, None), BackgroundFrom::Config(None));

        let mut config = tileide_config::Config::default();
        let state = tileide_config::State { background: Some(b.clone()), background_over: Some(a.clone()), ..Default::default() };
        assert_eq!(saved_background(&state, &config), Some(&b), "no tileide.background: the pick");
        config.background_set = true;
        assert_eq!(saved_background(&state, &config), Some(&b), "picked over this very config");
        config.background.dim = 30;
        assert_eq!(saved_background(&state, &config), None, "tileide.background edited since");
    }

    #[test]
    fn background_from_config() {
        let mut cfg = tileide_config::Config::default().background;
        assert_eq!(background(&cfg, None).unwrap().source, background::Source::SeeThrough);
        cfg.mode = "solid".into();
        assert!(background(&cfg, None).is_err(), "solid needs a source");
        cfg.source = Some("112233".into());
        cfg.tile_opacity = 0.95;
        let s = background(&cfg, None).unwrap();
        assert_eq!(s.source, background::Source::Solid(tileide_core::theme::rgb(0x112233)));
        assert!((s.tile_opacity - 0.95).abs() < 1e-9);
        let s = background(&cfg, Some("/w.png")).unwrap();
        assert_eq!(s.source, background::Source::Image(PathBuf::from("/w.png")), "a workspace image wins");
    }

    #[test]
    fn sample_themes_are_found() {
        let paths = Paths::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("config/tileide.lua"));
        let names: Vec<String> = paths.all_themes().into_iter().map(|(k, _)| k).collect();
        for expected in ["dune-teal", "dusk", "miasma"] {
            assert!(names.iter().any(|n| n == expected), "{names:?}");
        }
        assert_eq!(paths.theme("miasma").0.name, "Miasma");
        assert!(paths.theme("nope").1.is_some());
    }
}
