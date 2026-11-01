// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings changed from the GUI (v0.4 phase 1). They live in `state.lua` as
//! `settings = { ["tiling.gaps_in"] = 12, … }` and apply over the defaults, but never
//! over a key `tileide.lua` sets (Q15: the config wins; the GUI shows those locked).

use std::collections::BTreeMap;

use tileide_core::motion::{Curve, MAX_SPEED, MIN_SPEED};

use crate::model::Config;

/// (min, max) of the interface text scale and the code font size (points).
pub const UI_SCALE: (f64, f64) = (0.8, 1.5);
pub const CODE_SIZE: (f64, f64) = (7.0, 32.0);
/// (min, max) of the rail's size and of its icons within it.
pub const RAIL_SCALE: (f64, f64) = (0.8, 1.5);
pub const ICON_SCALE: (f64, f64) = (0.8, 1.25);

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
    ("tiling.gaps_in", Kind::Number(0.0, 60.0)),
    ("tiling.gaps_out", Kind::Number(0.0, 80.0)),
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
        "tiling.gaps_in" => Value::Number(t.gaps_in as f64),
        "tiling.gaps_out" => Value::Number(t.gaps_out as f64),
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
        ("tiling.gaps_in", Value::Number(v)) => t.gaps_in = v.round() as u32,
        ("tiling.gaps_out", Value::Number(v)) => t.gaps_out = v.round() as u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_reads_back() {
        let config = Config::default();
        for (key, _) in KEYS {
            let v = get(&config, key).unwrap_or_else(|| panic!("{key} has no getter"));
            let mut changed = Config::default();
            set(&mut changed, key, v.clone());
            assert_eq!(get(&changed, key), Some(v), "{key}");
        }
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
