// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings changed from the GUI (v0.4 phase 1). They live in `state.lua` as
//! `settings = { ["tiling.gaps_in"] = 12, … }` and apply over the defaults, but never
//! over a key `tileide.lua` sets (Q15: the config wins; the GUI shows those locked).

use std::collections::BTreeMap;

use tileide_core::motion::{Curve, MAX_SPEED, MIN_SPEED};
use tileide_core::theme::Edge;
use tileide_core::token::{BORDER_MAX, Rice};

use crate::model::Config;

/// (min, max) of the interface text scale and the code font size (points).
pub const UI_SCALE: (f64, f64) = (0.8, 1.5);
pub const CODE_SIZE: (f64, f64) = (7.0, 32.0);
/// (min, max) of the rail's size and of its icons within it.
pub const RAIL_SCALE: (f64, f64) = (0.8, 1.5);
pub const ICON_SCALE: (f64, f64) = (0.8, 1.25);
/// Sidebar and tile see-through (percent) and backdrop blur (px) upper bounds.
pub const TRANSPARENCY_MAX: f64 = 90.0;
pub const BLUR_MAX: f64 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Number(f64, f64),
    Text,
    Choice(&'static [&'static str]),
}

/// Every key the GUI may set, with what it holds.
pub const KEYS: &[(&str, Kind)] = &[
    ("appearance.ui_font", Kind::Text),
    ("appearance.ui_scale", Kind::Number(UI_SCALE.0, UI_SCALE.1)),
    ("appearance.code_font", Kind::Text),
    ("appearance.code_size", Kind::Number(CODE_SIZE.0, CODE_SIZE.1)),
    ("appearance.window_radius", Kind::Number(0.0, 40.0)),
    ("appearance.animations", Kind::Bool),
    ("appearance.motion", Kind::Choice(&["fluid", "snappy"])),
    ("appearance.motion_speed", Kind::Number(MIN_SPEED, MAX_SPEED)),
    ("appearance.rice", Kind::Choice(&Rice::KEYS)),
    ("appearance.tile_border", Kind::Number(0.0, BORDER_MAX)),
    ("appearance.tile_border_color", Kind::Choice(&Edge::KEYS)),
    ("appearance.frame_border", Kind::Number(0.0, BORDER_MAX)),
    ("appearance.frame_border_color", Kind::Choice(&Edge::KEYS)),
    ("tiling.gaps_in", Kind::Number(0.0, 60.0)),
    ("tiling.gaps_out", Kind::Number(0.0, 80.0)),
    ("tiling.max_tiles", Kind::Number(1.0, 64.0)),
    ("tiling.peek", Kind::Number(0.0, tileide_core::token::PEEK_MAX)),
    ("sidebar.start_expanded", Kind::Bool),
    ("sidebar.show_launcher", Kind::Bool),
    ("sidebar.show_workspaces", Kind::Bool),
    ("sidebar.show_label", Kind::Bool),
    ("sidebar.show_clock", Kind::Bool),
    ("sidebar.show_themes", Kind::Bool),
    ("sidebar.show_config", Kind::Bool),
    ("sidebar.show_visualizer", Kind::Bool),
    ("sidebar.scale", Kind::Number(RAIL_SCALE.0, RAIL_SCALE.1)),
    ("sidebar.icon_scale", Kind::Number(ICON_SCALE.0, ICON_SCALE.1)),
    ("sidebar.icon_backgrounds", Kind::Bool),
    ("sidebar.pills", Kind::Bool),
    ("editor.auto_close", Kind::Bool),
    ("editor.format_on_save", Kind::Bool),
    ("editor.auto_save", Kind::Bool),
    ("editor.inlay_hints", Kind::Bool),
    ("editor.theme_syntax", Kind::Bool),
    ("editor.match_brackets", Kind::Bool),
    ("alerts.missing_tools", Kind::Bool),
    ("alerts.tile_limit", Kind::Bool),
    ("sidebar.transparency", Kind::Number(0.0, TRANSPARENCY_MAX)),
    ("sidebar.blur", Kind::Number(0.0, BLUR_MAX)),
    ("appearance.tile_transparency", Kind::Number(0.0, TRANSPARENCY_MAX)),
    ("appearance.tile_blur", Kind::Number(0.0, BLUR_MAX)),
];

pub fn kind(key: &str) -> Option<Kind> {
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, kind)| *kind)
}

/// `value` checked against `key`'s kind (numbers clamped); `None` = unknown key or wrong type.
pub fn checked(key: &str, value: Value) -> Option<Value> {
    match (kind(key)?, value) {
        (Kind::Bool, v @ Value::Bool(_)) | (Kind::Text, v @ Value::Text(_)) => Some(v),
        (Kind::Number(min, max), Value::Number(n)) if n.is_finite() => Some(Value::Number(n.clamp(min, max))),
        (Kind::Choice(allowed), Value::Text(t)) if allowed.contains(&t.as_str()) => Some(Value::Text(t)),
        _ => None,
    }
}

/// The value `config` has for `key` (what Settings shows).
pub fn get(config: &Config, key: &str) -> Option<Value> {
    let (a, s, t) = (&config.appearance, &config.sidebar, &config.tiling);
    Some(match key {
        "appearance.ui_font" => Value::Text(a.ui_font.clone()),
        "appearance.ui_scale" => Value::Number(a.ui_scale),
        "appearance.code_font" => Value::Text(a.code_font.clone()),
        "appearance.code_size" => Value::Number(a.code_size),
        "appearance.window_radius" => Value::Number(a.window_radius as f64),
        "appearance.animations" => Value::Bool(a.animations),
        "appearance.motion" => Value::Text(a.motion.key().into()),
        "appearance.motion_speed" => Value::Number(a.motion_speed),
        "appearance.rice" => Value::Text(a.rice.key().into()),
        "appearance.tile_border" => Value::Number(a.tile_border),
        "appearance.tile_border_color" => Value::Text(a.tile_border_color.key().into()),
        "appearance.frame_border" => Value::Number(a.frame_border),
        "appearance.frame_border_color" => Value::Text(a.frame_border_color.key().into()),
        "tiling.gaps_in" => Value::Number(t.gaps_in as f64),
        "tiling.gaps_out" => Value::Number(t.gaps_out as f64),
        "tiling.max_tiles" => Value::Number(t.max_tiles as f64),
        "tiling.peek" => Value::Number(t.peek),
        "sidebar.start_expanded" => Value::Bool(s.start_expanded),
        "sidebar.show_launcher" => Value::Bool(s.show_launcher),
        "sidebar.show_workspaces" => Value::Bool(s.show_workspaces),
        "sidebar.show_label" => Value::Bool(s.show_label),
        "sidebar.show_clock" => Value::Bool(s.show_clock),
        "sidebar.show_themes" => Value::Bool(s.show_themes),
        "sidebar.show_config" => Value::Bool(s.show_config),
        "sidebar.show_visualizer" => Value::Bool(s.show_visualizer),
        "sidebar.scale" => Value::Number(s.scale),
        "sidebar.icon_scale" => Value::Number(s.icon_scale),
        "sidebar.icon_backgrounds" => Value::Bool(s.icon_backgrounds),
        "sidebar.pills" => Value::Bool(s.pills),
        "editor.auto_close" => Value::Bool(config.editor.auto_close),
        "editor.format_on_save" => Value::Bool(config.editor.format_on_save),
        "editor.auto_save" => Value::Bool(config.editor.auto_save),
        "editor.inlay_hints" => Value::Bool(config.editor.inlay_hints),
        "editor.theme_syntax" => Value::Bool(config.editor.theme_syntax),
        "editor.match_brackets" => Value::Bool(config.editor.match_brackets),
        "alerts.missing_tools" => Value::Bool(config.alerts.missing_tools),
        "alerts.tile_limit" => Value::Bool(config.alerts.tile_limit),
        // Unset = the theme's look, which only the app knows.
        "sidebar.transparency" => Value::Number(s.transparency?),
        "sidebar.blur" => Value::Number(s.blur?),
        "appearance.tile_transparency" => Value::Number(a.tile_transparency?),
        "appearance.tile_blur" => Value::Number(a.tile_blur?),
        _ => return None,
    })
}

fn set(config: &mut Config, key: &str, value: Value) {
    let Some(value) = checked(key, value) else { return };
    let (a, s, t) = (&mut config.appearance, &mut config.sidebar, &mut config.tiling);
    match (key, value) {
        ("appearance.ui_font", Value::Text(v)) => a.ui_font = v,
        ("appearance.ui_scale", Value::Number(v)) => a.ui_scale = v,
        ("appearance.code_font", Value::Text(v)) => a.code_font = v,
        ("appearance.code_size", Value::Number(v)) => a.code_size = v,
        ("appearance.window_radius", Value::Number(v)) => a.window_radius = v.round() as u32,
        ("appearance.animations", Value::Bool(v)) => a.animations = v,
        ("appearance.motion", Value::Text(v)) => a.motion = Curve::from_key(&v).unwrap_or_default(),
        ("appearance.motion_speed", Value::Number(v)) => a.motion_speed = v,
        ("appearance.rice", Value::Text(v)) => a.rice = Rice::from_key(&v).unwrap_or_default(),
        ("appearance.tile_border", Value::Number(v)) => a.tile_border = v,
        ("appearance.tile_border_color", Value::Text(v)) => a.tile_border_color = Edge::from_key(&v).unwrap_or_default(),
        ("appearance.frame_border", Value::Number(v)) => a.frame_border = v,
        ("appearance.frame_border_color", Value::Text(v)) => a.frame_border_color = Edge::from_key(&v).unwrap_or_default(),
        ("tiling.gaps_in", Value::Number(v)) => t.gaps_in = v.round() as u32,
        ("tiling.gaps_out", Value::Number(v)) => t.gaps_out = v.round() as u32,
        ("tiling.max_tiles", Value::Number(v)) => t.max_tiles = v.round() as u32,
        ("tiling.peek", Value::Number(v)) => t.peek = v,
        ("sidebar.start_expanded", Value::Bool(v)) => s.start_expanded = v,
        ("sidebar.show_launcher", Value::Bool(v)) => s.show_launcher = v,
        ("sidebar.show_workspaces", Value::Bool(v)) => s.show_workspaces = v,
        ("sidebar.show_label", Value::Bool(v)) => s.show_label = v,
        ("sidebar.show_clock", Value::Bool(v)) => s.show_clock = v,
        ("sidebar.show_themes", Value::Bool(v)) => s.show_themes = v,
        ("sidebar.show_config", Value::Bool(v)) => s.show_config = v,
        ("sidebar.show_visualizer", Value::Bool(v)) => s.show_visualizer = v,
        ("sidebar.scale", Value::Number(v)) => s.scale = v,
        ("sidebar.icon_scale", Value::Number(v)) => s.icon_scale = v,
        ("sidebar.icon_backgrounds", Value::Bool(v)) => s.icon_backgrounds = v,
        ("sidebar.pills", Value::Bool(v)) => s.pills = v,
        ("editor.auto_close", Value::Bool(v)) => config.editor.auto_close = v,
        ("editor.format_on_save", Value::Bool(v)) => config.editor.format_on_save = v,
        ("editor.auto_save", Value::Bool(v)) => config.editor.auto_save = v,
        ("editor.inlay_hints", Value::Bool(v)) => config.editor.inlay_hints = v,
        ("editor.theme_syntax", Value::Bool(v)) => config.editor.theme_syntax = v,
        ("editor.match_brackets", Value::Bool(v)) => config.editor.match_brackets = v,
        ("alerts.missing_tools", Value::Bool(v)) => config.alerts.missing_tools = v,
        ("alerts.tile_limit", Value::Bool(v)) => config.alerts.tile_limit = v,
        ("sidebar.transparency", Value::Number(v)) => s.transparency = Some(v),
        ("sidebar.blur", Value::Number(v)) => s.blur = Some(v),
        ("appearance.tile_transparency", Value::Number(v)) => a.tile_transparency = Some(v),
        ("appearance.tile_blur", Value::Number(v)) => a.tile_blur = Some(v),
        _ => {}
    }
}

/// `config` with the GUI's settings applied, except keys `tileide.lua` set.
pub fn apply(mut config: Config, settings: &BTreeMap<String, Value>) -> Config {
    for (key, value) in settings {
        if !config.explicit.contains(key) {
            set(&mut config, key, value.clone());
        }
    }
    config
}

/// The Lua table and field that set `key` (`sidebar.start_expanded` is `start_state`).
fn lua_field(key: &str) -> Option<(&str, &str)> {
    match key {
        "sidebar.start_expanded" => Some(("sidebar", "start_state")),
        _ => key.split_once('.'),
    }
}

/// The code part of a Lua line (before a `--` comment outside strings).
fn code_of(line: &str) -> &str {
    let mut quote = None;
    let bytes = line.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        match (quote, c) {
            (None, b'"' | b'\'') => quote = Some(c),
            (Some(q), _) if c == q && (i == 0 || bytes[i - 1] != b'\\') => quote = None,
            (None, b'-') if bytes.get(i + 1) == Some(&b'-') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `source` (a `tileide.lua`) with the line that sets `key` commented out, so Settings
/// can change it (the "Unlock" button). Only a line holding that one field, inside a
/// `tileide.<table> { … }` call, is touched; anything else returns `None` (then the
/// user edits the file).
pub fn release(source: &str, key: &str) -> Option<String> {
    let (table, field) = lua_field(key)?;
    let call = format!("tileide.{table}");
    let mut inside = false;
    let mut depth = 0i32;
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    let mut target = None;
    for (n, line) in lines.iter().enumerate() {
        let code = code_of(line);
        let mut rest = code;
        if !inside && let Some(at) = code.find(&call) {
            inside = true;
            depth = 0;
            rest = &code[at + call.len()..];
        }
        if !inside {
            continue;
        }
        let assignment = rest.trim().trim_end_matches(',').trim();
        let opens = rest.matches('{').count() as i32;
        let closes = rest.matches('}').count() as i32;
        if let Some((name, value)) = assignment.split_once('=')
            && name.trim() == field
            && !value.contains('=')
            && depth == 1
            && opens == 0
            && closes == 0
        {
            target = Some(n);
            break;
        }
        depth += opens - closes;
        if depth <= 0 && (opens > 0 || closes > 0) {
            inside = false;
        }
    }
    let n = target?;
    let line = &lines[n];
    let indent = &line[..line.len() - line.trim_start().len()];
    lines[n] = format!("{indent}-- {}  -- now set in Settings", line.trim_start());
    let mut out = lines.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_reads_back() {
        let config = Config::default();
        for (key, _) in KEYS {
            // Keys unset by default (the theme decides) read back once set.
            let v = get(&config, key).or_else(|| matches!(kind(key), Some(Kind::Number(..))).then_some(Value::Number(12.0))).unwrap_or_else(|| panic!("{key} has no getter"));
            let mut changed = Config::default();
            set(&mut changed, key, v.clone());
            assert_eq!(get(&changed, key), Some(v), "{key}");
        }
    }

    #[test]
    fn releases_one_field_line() {
        let src = "tileide.appearance {\n  window_radius = 14,  -- corners\n  animations = true,\n}\ntileide.tiling {\n  gaps_in = 10,\n  gaps_out = 16,\n}\ntileide.sidebar { start_state = \"collapsed\" }\n";
        let out = release(src, "appearance.window_radius").unwrap();
        assert!(out.contains("  -- window_radius = 14,  -- corners  -- now set in Settings\n  animations = true,"), "{out}");
        assert!(release(src, "tiling.gaps_out").unwrap().contains("-- gaps_out = 16,"));
        // gaps_in in another table's call isn't touched.
        assert_eq!(release("tileide.appearance {\n  gaps_in = 1,\n}\n", "tiling.gaps_in"), None);
        // Several fields on one line: left for the user.
        assert_eq!(release(src, "sidebar.start_expanded"), None);
        assert_eq!(release("tileide.appearance { window_radius = 4 }\n", "appearance.window_radius"), None);
        // A commented-out line doesn't count.
        assert_eq!(release("tileide.tiling {\n  -- gaps_in = 3,\n}\n", "tiling.gaps_in"), None);
        let loaded = |src: &str| {
            let dir = std::env::temp_dir().join(format!("tileide-release-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("tileide.lua"), src).unwrap();
            crate::load(&dir.join("tileide.lua"), crate::Limits::default()).unwrap().config
        };
        let config = loaded(&out);
        assert!(!config.explicit.contains("appearance.window_radius") && config.explicit.contains("appearance.animations"));
    }

    #[test]
    fn applies_unless_the_config_set_it() {
        let mut settings = BTreeMap::new();
        settings.insert("tiling.gaps_in".to_string(), Value::Number(500.0));
        settings.insert("appearance.motion".to_string(), Value::Text("snappy".into()));
        settings.insert("appearance.code_font".to_string(), Value::Bool(true));
        settings.insert("no.such".to_string(), Value::Bool(true));
        let applied = apply(Config::default(), &settings);
        assert_eq!(applied.tiling.gaps_in, 60, "clamped");
        assert_eq!(applied.appearance.motion, Curve::Snappy);
        assert_eq!(applied.appearance.code_font, "monospace", "wrong type ignored");
        let mut pinned = Config::default();
        pinned.explicit.insert("tiling.gaps_in".into());
        assert_eq!(apply(pinned, &settings).tiling.gaps_in, 10, "tileide.lua wins");
    }
}
