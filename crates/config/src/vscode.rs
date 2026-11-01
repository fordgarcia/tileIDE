// SPDX-License-Identifier: GPL-3.0-or-later
//! VS Code colour theme import (v0.3 phase 7): a theme `.json` or an extension `.vsix`
//! (Open VSX) becomes a TileIDE theme file. Only data is read; nothing is run. The
//! mapping (workbench keys and TextMate scopes → TileIDE tokens) is in `map`; what a
//! theme leaves out falls back to colours derived from the rest, as in any theme file.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tileide_core::theme::{ANSI_KEYS, Rgb, SYNTAX_KEYS, contrast, over, rgb};

/// The header that marks a file as imported, so a re-import may overwrite it.
pub const HEADER: &str = "-- Imported from VS Code";

/// One converted theme: its file name (no extension), Lua text, and contrast warnings.
#[derive(Debug)]
pub struct Converted {
    pub stem: String,
    pub name: String,
    pub lua: String,
    pub warnings: Vec<String>,
}

/// VS Code's JSON with comments and trailing commas → plain JSON.
pub fn strip_jsonc(text: &str) -> String {
    let no_comments = scan(text, |chars, i, out| match (chars[*i], chars.get(*i + 1)) {
        ('/', Some('/')) => {
            while *i < chars.len() && chars[*i] != '\n' {
                *i += 1;
            }
        }
        ('/', Some('*')) => {
            *i += 2;
            while *i + 1 < chars.len() && !(chars[*i] == '*' && chars[*i + 1] == '/') {
                *i += 1;
            }
            *i += 2;
        }
        (c, _) => {
            out.push(c);
            *i += 1;
        }
    });
    // A comma whose next non-space character closes the object or array is dropped.
    scan(&no_comments, |chars, i, out| {
        let closes = chars[*i] == ',' && chars[*i + 1..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| *c == '}' || *c == ']');
        if !closes {
            out.push(chars[*i]);
        }
        *i += 1;
    })
}

/// Copies `text`, passing everything outside string literals to `outside`, which
/// appends what it keeps and advances the index.
fn scan(text: &str, mut outside: impl FnMut(&[char], &mut usize, &mut String)) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '"' {
            outside(&chars, &mut i, &mut out);
            continue;
        }
        out.push('"');
        i += 1;
        while i < chars.len() {
            let c = chars[i];
            out.push(c);
            i += 1;
            if c == '\\' && i < chars.len() {
                out.push(chars[i]);
                i += 1;
            } else if c == '"' {
                break;
            }
        }
    }
    out
}

pub fn parse(text: &str) -> Result<Value, String> {
    serde_json::from_str(&strip_jsonc(text.trim_start_matches('\u{feff}'))).map_err(|e| format!("not a VS Code theme (JSON error: {e})"))
}

/// `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`; alpha is composited over `under`.
fn colour(text: &str, under: Rgb) -> Option<Rgb> {
    let hex = text.strip_prefix('#')?;
    let digits: String = match hex.len() {
        3 | 4 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => hex.to_string(),
        _ => return None,
    };
    let value = u32::from_str_radix(&digits[..6], 16).ok()?;
    let alpha = if digits.len() == 8 { u8::from_str_radix(&digits[6..], 16).ok()? as f64 / 255.0 } else { 1.0 };
    Some(over(rgb(value), alpha, under))
}

fn hex((r, g, b): Rgb) -> String {
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", byte(r), byte(g), byte(b))
}

/// The first of `keys` set in the theme's `colors`.
fn first(colors: &Value, keys: &[&str], under: Rgb) -> Option<Rgb> {
    keys.iter().find_map(|k| colors[*k].as_str().and_then(|v| colour(v, under)))
}

/// TextMate scopes per syntax slot (`SYNTAX_KEYS` order), most specific first.
const SCOPES: [&[&str]; 8] = [
    &["keyword.control", "keyword", "storage.modifier"],
    &["entity.name.type", "support.type", "storage.type", "entity.name.class"],
    &["string"],
    &["constant.numeric", "constant.language", "constant"],
    &["entity.name.function", "support.function", "meta.function-call"],
    &["meta.preprocessor", "keyword.control.directive", "keyword.other.preprocessor", "keyword.control.import"],
    &["support.function.builtin", "variable.language", "support.variable"],
    &["comment"],
];

/// The foreground of the rule that best matches `wanted`: an exact scope, else one
/// that is a parent of it (`keyword` covers `keyword.control`).
fn scope_colour(rules: &[Value], wanted: &str, under: Rgb) -> Option<Rgb> {
    let scopes = |rule: &Value| -> Vec<String> {
        match &rule["scope"] {
            Value::String(s) => s.split(',').map(|s| s.trim().to_string()).collect(),
            Value::Array(list) => list.iter().filter_map(|s| s.as_str().map(|s| s.trim().to_string())).collect(),
            _ => Vec::new(),
        }
    };
    let foreground = |rule: &Value| rule["settings"]["foreground"].as_str().and_then(|c| colour(c, under));
    let exact = rules.iter().find(|r| scopes(r).iter().any(|s| s == wanted)).and_then(foreground);
    exact.or_else(|| {
        rules
            .iter()
            .filter_map(|r| {
                let best = scopes(r).into_iter().filter(|s| wanted.starts_with(&format!("{s}."))).map(|s| s.len()).max()?;
                Some((best, foreground(r)?))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, c)| c)
    })
}

/// Lowercase words joined by `-`, for the file name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "imported".into() } else { out }
}

fn lua_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " "))
}

/// A parsed VS Code theme → a TileIDE theme file.
pub fn map(theme: &Value, fallback_name: &str, origin: &str) -> Result<Converted, String> {
    let colors = &theme["colors"];
    let rules: Vec<Value> = theme["tokenColors"].as_array().cloned().unwrap_or_default();
    if !colors.is_object() && rules.is_empty() {
        return Err("not a VS Code colour theme (no \"colors\" or \"tokenColors\")".into());
    }
    let declared = theme["type"].as_str().unwrap_or("");
    let black = (0.0, 0.0, 0.0);
    // The editor background first: everything translucent is composited over it.
    let guess_dark = !declared.contains("light");
    let tile = first(colors, &["editor.background"], black).unwrap_or(if guess_dark { rgb(0x1E1E1E) } else { rgb(0xFFFFFF) });
    let dark = match declared {
        "dark" | "hc" | "hcDark" => true,
        "light" | "hcLight" => false,
        _ => tileide_core::theme::luminance(tile) < 0.4,
    };
    let text = first(colors, &["editor.foreground", "foreground"], tile)
        .or_else(|| rules.iter().find(|r| r["scope"].is_null()).and_then(|r| r["settings"]["foreground"].as_str()).and_then(|c| colour(c, tile)))
        .unwrap_or(if dark { rgb(0xD4D4D4) } else { rgb(0x333333) });
    let surface = first(colors, &["sideBar.background", "activityBar.background", "editorGroupHeader.tabsBackground", "panel.background"], tile)
        .unwrap_or_else(|| over(black, if dark { 0.25 } else { 0.06 }, tile));
    let muted = first(colors, &["editorLineNumber.foreground", "descriptionForeground", "tab.inactiveForeground"], tile).unwrap_or_else(|| over(text, 0.6, tile));
    let accent = first(colors, &["button.background", "activityBarBadge.background", "focusBorder", "textLink.foreground", "editorCursor.foreground"], tile).unwrap_or(text);
    let on_accent = first(colors, &["button.foreground", "activityBarBadge.foreground"], accent)
        .unwrap_or_else(|| if contrast(black, accent) > contrast((1.0, 1.0, 1.0), accent) { black } else { (1.0, 1.0, 1.0) });
    let error = first(colors, &["editorError.foreground", "errorForeground", "terminal.ansiRed"], tile).unwrap_or(rgb(0xE5534B));
    let mid = first(colors, &["statusBar.background", "titleBar.activeBackground", "activityBar.background"], tile).unwrap_or_else(|| over(accent, 0.35, surface));

    let mut warnings = Vec::new();
    for (what, c) in [("text", text), ("muted text", muted)] {
        let ratio = contrast(c, tile);
        if ratio < 4.5 {
            warnings.push(format!("{what} is {ratio:.1}:1 on the editor background (below 4.5:1)"));
        }
    }

    let name = theme["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(fallback_name).trim().to_string();
    let mut lua = format!("{HEADER} ({origin}). Re-importing replaces this file; copy it to keep edits.\nreturn {{\n");
    lua += &format!("  name      = {},\n  mode      = \"{}\",\n", lua_string(&name), if dark { "dark" } else { "light" });
    for (key, c) in [("surface", surface), ("tile", tile), ("text", text), ("muted", muted), ("accent", accent), ("on_accent", on_accent), ("error", error), ("strip_mid", mid)] {
        lua += &format!("  {key:<9} = \"{}\",\n", hex(c));
    }
    let syntax: Vec<String> = SYNTAX_KEYS
        .iter()
        .zip(SCOPES)
        .filter_map(|(key, scopes)| scopes.iter().find_map(|s| scope_colour(&rules, s, tile)).map(|c| format!("    {key} = \"{}\",\n", hex(c))))
        .collect();
    if !syntax.is_empty() {
        lua += &format!("  syntax = {{\n{}  }},\n", syntax.concat());
    }
    let vscode_ansi = |key: &str| {
        let mut name = String::from("terminal.ansi");
        for part in key.split('_') {
            let mut chars = part.chars();
            name.extend(chars.next().map(|c| c.to_ascii_uppercase()));
            name.push_str(chars.as_str());
        }
        name
    };
    let terminal: Vec<String> = ANSI_KEYS
        .iter()
        .filter_map(|key| first(colors, &[&vscode_ansi(key)], tile).map(|c| format!("    {key} = \"{}\",\n", hex(c))))
        .collect();
    if !terminal.is_empty() {
        lua += &format!("  terminal = {{\n{}  }},\n", terminal.concat());
    }
    lua += "}\n";
    Ok(Converted { stem: slug(&name), name, lua, warnings })
}

/// Reads a theme JSON, following one `include` (a base theme next to it) if present.
fn read_theme(read: &dyn Fn(&str) -> Result<String, String>, path: &str) -> Result<Value, String> {
    let mut theme = parse(&read(path)?)?;
    if let Some(include) = theme["include"].as_str() {
        let base_path = Path::new(path).parent().unwrap_or(Path::new("")).join(include);
        if let Ok(base) = read(&base_path.to_string_lossy()).and_then(|t| parse(&t)) {
            theme = merge(base, theme);
        }
    }
    Ok(theme)
}

/// `child`'s colours over `base`'s; token rules appended (later rules win in VS Code,
/// and `scope_colour` takes the first exact match, so the child's go first).
fn merge(base: Value, child: Value) -> Value {
    let mut out = child.clone();
    let mut colors = base["colors"].as_object().cloned().unwrap_or_default();
    colors.extend(child["colors"].as_object().cloned().unwrap_or_default());
    out["colors"] = Value::Object(colors);
    let mut rules = child["tokenColors"].as_array().cloned().unwrap_or_default();
    rules.extend(base["tokenColors"].as_array().cloned().unwrap_or_default());
    out["tokenColors"] = Value::Array(rules);
    if out["type"].is_null() {
        out["type"] = base["type"].clone();
    }
    out
}

/// A `.json` theme or every colour theme in a `.vsix`, converted.
pub fn convert_file(path: &Path) -> Result<Vec<Converted>, String> {
    let file = path.file_name().map_or_else(|| "theme".into(), |n| n.to_string_lossy().into_owned());
    let is_vsix = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("vsix") || e.eq_ignore_ascii_case("zip"));
    if !is_vsix {
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let read = move |p: &str| std::fs::read_to_string(dir.join(Path::new(p).file_name().unwrap_or_default())).map_err(|e| format!("{p}: {e}"));
        let stem = path.file_stem().map_or_else(|| "Imported".into(), |s| s.to_string_lossy().into_owned());
        let theme = read_theme(&read, &file)?;
        return Ok(vec![map(&theme, &stem, &file)?]);
    }
    let archive = path.to_path_buf();
    let read = move |member: &str| vsix_member(&archive, member);
    let manifest = parse(&read("extension/package.json")?)?;
    let themes = manifest["contributes"]["themes"].as_array().cloned().unwrap_or_default();
    if themes.is_empty() {
        return Err(format!("{file} contributes no colour themes (icon themes and other extensions can't be imported)"));
    }
    themes
        .iter()
        .filter_map(|t| Some((t["label"].as_str().unwrap_or("Imported").to_string(), t["path"].as_str()?.to_string())))
        .map(|(label, rel)| {
            let member = Path::new("extension").join(rel.trim_start_matches("./"));
            let mut theme = read_theme(&read, &member.to_string_lossy())?;
            // The extension's label is the name people know it by.
            theme["name"] = Value::String(label.clone());
            map(&theme, &label, &file)
        })
        .collect()
}

/// One file out of a `.vsix` (a zip) via `bsdtar`, part of libarchive (which pacman needs).
fn vsix_member(archive: &Path, member: &str) -> Result<String, String> {
    let output = Command::new("bsdtar").arg("-xOf").arg(archive).arg(member).output().map_err(|e| format!("cannot run bsdtar to open the .vsix: {e}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(format!("{member} is not in the .vsix"));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{member} is not text"))
}

/// Converts `path` and writes the themes into `dir` (the user's `themes/`). An existing
/// file is replaced only if an earlier import wrote it; otherwise the new one gets
/// a `-vscode` suffix. Returns (file written, theme name, warnings) per theme.
pub fn import(path: &Path, dir: &Path) -> Result<Vec<(PathBuf, String, Vec<String>)>, String> {
    let converted = convert_file(path)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut written = Vec::new();
    for c in converted {
        let mut target = dir.join(format!("{}.lua", c.stem));
        let ours = |p: &Path| std::fs::read_to_string(p).is_ok_and(|t| t.starts_with(HEADER));
        if target.exists() && !ours(&target) {
            target = dir.join(format!("{}-vscode.lua", c.stem));
        }
        std::fs::write(&target, &c.lua).map_err(|e| format!("{}: {e}", target.display()))?;
        written.push((target, c.name, c.warnings));
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonc_comments_and_trailing_commas_go() {
        let text = "{\n // a comment\n \"a\": \"//not a comment\", /* block */ \"b\": [1, 2,],\n}";
        assert_eq!(parse(text).unwrap(), json!({"a": "//not a comment", "b": [1, 2]}));
        assert_eq!(parse("{\"s\": \"q\\\"uote, }\"}").unwrap(), json!({"s": "q\"uote, }"}));
    }

    #[test]
    fn colours_parse_with_alpha() {
        assert_eq!(colour("#fff", (0.0, 0.0, 0.0)), Some((1.0, 1.0, 1.0)));
        assert_eq!(colour("#FF000080", (0.0, 0.0, 0.0)).map(hex).as_deref(), Some("#800000"));
        assert_eq!(colour("red", (0.0, 0.0, 0.0)), None);
    }

    #[test]
    fn scopes_match_exactly_or_by_parent() {
        let rules = vec![
            json!({"scope": "keyword", "settings": {"foreground": "#111111"}}),
            json!({"scope": ["keyword.control", "storage"], "settings": {"foreground": "#222222"}}),
            json!({"scope": "string, string.quoted", "settings": {"foreground": "#333333"}}),
        ];
        let at = |s| scope_colour(&rules, s, (0.0, 0.0, 0.0)).map(hex);
        assert_eq!(at("keyword.control").as_deref(), Some("#222222"), "exact wins");
        assert_eq!(at("keyword.operator").as_deref(), Some("#111111"), "parent scope covers children");
        assert_eq!(at("string").as_deref(), Some("#333333"), "comma lists split");
        assert_eq!(at("comment"), None);
    }

    #[test]
    fn a_theme_maps_to_a_loadable_file() {
        let theme = json!({
            "name": "Night Owl \"Test\"", "type": "dark",
            "colors": {"editor.background": "#011627", "editor.foreground": "#d6deeb", "sideBar.background": "#011627",
                       "button.background": "#7e57c2", "editorLineNumber.foreground": "#4b6479", "terminal.ansiRed": "#ef5350", "terminal.ansiBrightBlue": "#82aaff"},
            "tokenColors": [{"scope": "keyword", "settings": {"foreground": "#c792ea"}}, {"scope": ["comment"], "settings": {"foreground": "#637777"}}]
        });
        let c = map(&theme, "x", "night-owl.json").unwrap();
        assert_eq!(c.stem, "night-owl-test");
        assert!(c.lua.starts_with(HEADER));
        assert!(c.lua.contains("tile      = \"#011627\""), "{}", c.lua);
        assert!(c.lua.contains("keyword = \"#C792EA\""));
        assert!(c.lua.contains("bright_blue = \"#82AAFF\""));
        assert!(!c.warnings.is_empty(), "the dim line numbers fail 4.5:1 and are reported");

        let dir = std::env::temp_dir().join(format!("tileide-vscode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("night-owl.lua");
        std::fs::write(&file, &c.lua).unwrap();
        let (t, warnings) = crate::load_theme(&file).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!((t.name.as_str(), t.dark), ("Night Owl \"Test\"", true));
        assert_eq!(t.code[0].map(hex).as_deref(), Some("#C792EA"));
        assert_eq!(t.code[7].map(hex).as_deref(), Some("#637777"));
        assert_eq!(t.ansi[1].map(hex).as_deref(), Some("#EF5350"));
        assert_eq!(t.ansi[2], None, "unset terminal colours stay derived");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_vsix_yields_each_colour_theme() {
        if Command::new("bsdtar").arg("--version").output().is_err() {
            eprintln!("bsdtar missing: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tileide-vsix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ext = dir.join("extension/themes");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(dir.join("extension/package.json"), r##"{"contributes": {"themes": [
            {"label": "Ember Dark", "uiTheme": "vs-dark", "path": "./themes/dark.json"},
            {"label": "Ember Light", "uiTheme": "vs", "path": "./themes/light.json"}]}}"##).unwrap();
        std::fs::write(ext.join("base.json"), r##"{"colors": {"editor.foreground": "#EEEEEE"}, "tokenColors": [{"scope": "string", "settings": {"foreground": "#A5D6A7"}}]}"##).unwrap();
        std::fs::write(ext.join("dark.json"), r##"{"include": "./base.json", "type": "dark", "colors": {"editor.background": "#1A1210"}}"##).unwrap();
        std::fs::write(ext.join("light.json"), r##"{"type": "light", "colors": {"editor.background": "#FFF8F0", "editor.foreground": "#2B241C"}}"##).unwrap();
        let vsix = dir.join("ember.vsix");
        let status = Command::new("bsdtar").current_dir(&dir).args(["--format", "zip", "-cf"]).arg(&vsix).arg("extension").status().unwrap();
        assert!(status.success());
        let themes = convert_file(&vsix).unwrap();
        assert_eq!(themes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Ember Dark", "Ember Light"]);
        assert!(themes[0].lua.contains("text      = \"#EEEEEE\""), "the included base theme's colours apply: {}", themes[0].lua);
        assert!(themes[0].lua.contains("string = \"#A5D6A7\""));
        assert!(themes[1].lua.contains("mode      = \"light\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn import_never_overwrites_a_hand_written_theme() {
        let dir = std::env::temp_dir().join(format!("tileide-vscode-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("dusk.json");
        std::fs::write(&source, r##"{"name": "Dusk", "colors": {"editor.background": "#202020"}, /* c */ }"##).unwrap();
        let themes = dir.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("dusk.lua"), "return { name = 'Mine' }").unwrap();
        let first = import(&source, &themes).unwrap();
        assert_eq!(first[0].0, themes.join("dusk-vscode.lua"));
        let again = import(&source, &themes).unwrap();
        assert_eq!(again[0].0, themes.join("dusk-vscode.lua"));
        std::fs::write(themes.join("dusk.lua"), format!("{HEADER} (old)\nreturn {{}}")).unwrap();
        assert_eq!(import(&source, &themes).unwrap()[0].0, themes.join("dusk.lua"), "an earlier import is replaced");
        assert!(import(&dir.join("missing.json"), &themes).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
