// SPDX-License-Identifier: GPL-3.0-or-later
//! Config engine checks: the sample loads, errors carry file:line, deliberate sandbox
//! escapes fail, and theme and state files round-trip.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tileide_config::model::{Action, MIN_TILE_OPACITY};
use tileide_config::{Limits, Loaded, State, find_themes, load, load_state, load_theme, save_state};
use tileide_core::layout::LayoutKind;
use tileide_core::theme::{Glass, contrast};

const LIMITS: Limits = Limits { time: Duration::from_millis(100), memory: 16 * 1024 * 1024 };

/// A private config dir per test, holding `tileide.lua` plus optional extra files.
fn config_dir(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tileide-config-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for (name, body) in files {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    dir
}

fn run(test: &str, source: &str) -> Result<Loaded, String> {
    let dir = config_dir(test, &[("tileide.lua", source)]);
    load(&dir.join("tileide.lua"), LIMITS)
}

fn fails(test: &str, source: &str) -> String {
    match run(test, source) {
        Ok(_) => panic!("{test}: config should have failed:\n{source}"),
        Err(message) => message,
    }
}

fn sample_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config")
}

#[test]
fn sample_config_loads_cleanly() {
    let loaded = load(&sample_dir().join("tileide.lua"), Limits::default()).expect("sample config");
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let c = &loaded.config;
    assert_eq!(c.appearance.theme, None, "the sample leaves the theme to the tray");
    assert_eq!((c.tiling.gaps_in, c.tiling.gaps_out), (10, 16));
    assert_eq!(c.workspaces[&1].name.as_deref(), Some("code"));
}

#[test]
fn rice_overrides_earlier_values() {
    let dir = config_dir("rice", &[
        ("tileide.lua", "tileide.tiling { gaps_in = 4, gaps_out = 7 }\nrequire('rices.r')"),
        ("rices/r.lua", "tileide.appearance { theme = 'miasma' }\ntileide.tiling { gaps_in = 12 }"),
    ]);
    let c = load(&dir.join("tileide.lua"), LIMITS).unwrap().config;
    assert_eq!(c.appearance.theme.as_deref(), Some("miasma"));
    assert_eq!((c.tiling.gaps_in, c.tiling.gaps_out), (12, 7), "keys the rice leaves alone survive");
}

#[test]
fn background_set_is_tracked() {
    assert!(!run("bg-unset", "").unwrap().config.background_set);
    assert!(run("bg-set", "tileide.background { mode = 'solid', source = '112233' }").unwrap().config.background_set);
}

#[test]
fn later_version_calls_are_accepted() {
    let loaded = run("later", "tileide.lsp('rust', { cmd = 'rust-analyzer' })\ntileide.appearance { opacity = 0.8, blur_hint = true }").unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn lsp_overrides_parse() {
    let c = run("lsp", "tileide.lsp('rust', { cmd = 'ra --flag' })\ntileide.lsp('python', { cmd = { 'pylsp', '-v' } })\ntileide.lsp('java', { enabled = false })").unwrap().config;
    assert_eq!(c.lsp["rust"].cmd.as_deref(), Some(&["ra".to_string(), "--flag".to_string()][..]));
    assert_eq!(c.lsp["python"].cmd.as_deref(), Some(&["pylsp".to_string(), "-v".to_string()][..]));
    assert!(!c.lsp["java"].enabled && c.lsp["java"].cmd.is_none());
    assert!(run("lsp-bad", "tileide.lsp('rust', { cmd = '' })").is_err());
}

#[test]
fn binds_parse_into_actions_and_replace_defaults() {
    let c = run("binds", "tileide.bind('ALT', 'V', 'layout', 'columns')\ntileide.bind('alt shift', 'q', 'workspace', 3)\ntileide.bind('ALT', 'T', 'focus', 'left')").unwrap().config;
    let binds = c.effective_binds();
    let find = |mods: &[&str], key: &str| binds.iter().filter(|b| b.mods == mods && b.key.eq_ignore_ascii_case(key)).map(|b| b.action.clone()).collect::<Vec<_>>();
    assert_eq!(find(&["ALT"], "V"), vec![Action::Layout(Some(LayoutKind::Columns))]);
    assert_eq!(find(&["ALT", "SHIFT"], "q"), vec![Action::Workspace(3)]);
    assert_eq!(find(&["ALT"], "T").len(), 1, "a user bind replaces the default on the same key");
    assert_eq!(find(&["ALT"], "0"), vec![Action::Workspace(10)], "defaults stay");
}

#[test]
fn bind_errors_name_the_problem() {
    assert!(fails("bind-action", "tileide.bind('ALT', 'X', 'explode')").contains("unknown action"));
    assert!(fails("bind-arg", "tileide.bind('ALT', 'X', 'focus', 'sideways')").contains("left, right, up or down"));
    assert!(fails("bind-mod", "tileide.bind('HYPER', 'X', 'close')").contains("unknown modifier"));
}

#[test]
fn future_actions_warn_but_load() {
    let loaded = run("future", "tileide.bind('ALT', 'P', 'palette')").unwrap();
    assert!(loaded.warnings.iter().any(|w| w.contains("not available")), "{:?}", loaded.warnings);
}

#[test]
fn workspace_options() {
    let c = run("ws", "tileide.workspace(10, { name = 'zero', layout = 'columns', theme = 'miasma', folder = '~/x' })").unwrap().config;
    let w = &c.workspaces[&10];
    assert_eq!((w.name.as_deref(), w.layout, w.theme.as_deref()), (Some("zero"), Some(LayoutKind::Columns), Some("miasma")));
    assert!(fails("ws-id", "tileide.workspace(11, {})").contains("1..10"));
}

#[test]
fn shipped_themes_load_and_pass_contrast() {
    let themes = find_themes(&[sample_dir().join("themes")]);
    assert!(themes.len() >= 3, "{themes:?}");
    for (name, path) in themes {
        let (t, warnings) = load_theme(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(warnings.is_empty(), "{name}: {warnings:?}");
        for bg in [t.surface, t.tile] {
            assert!(contrast(t.text, bg) >= 4.5, "{name}: text");
            assert!(contrast(t.muted, bg) >= 4.5, "{name}: muted");
        }
        assert!(contrast(t.on_accent, t.accent) >= 4.5, "{name}: on_accent");
        let ansi = t.terminal_palette();
        for i in (1..=6).chain(8..=14) {
            assert!(contrast(ansi[i], t.tile) >= 4.5, "{name}: terminal colour {i}");
        }
    }
}

#[test]
fn theme_errors_and_layout_keys() {
    let dir = config_dir("theme-bad", &[
        ("missing.lua", "return { mode = 'dark' }"),
        ("layout.lua", "return { mode = 'light', surface = '#ffffff', tile = '#ffffff', text = '#000000', muted = '#555555', accent = '#0000ff', gaps_in = 3 }"),
        ("colour.lua", "return { mode = 'dark', surface = 'red' }"),
    ]);
    assert!(load_theme(&dir.join("missing.lua")).unwrap_err().contains("theme.surface: missing"));
    assert!(load_theme(&dir.join("colour.lua")).unwrap_err().contains("#RRGGBB"));
    let (t, warnings) = load_theme(&dir.join("layout.lua")).unwrap();
    assert!(!t.dark && t.name == "layout", "name defaults to the file name");
    assert!(warnings.iter().any(|w| w.contains("gaps_in")), "layout keys are not theme keys: {warnings:?}");
}

#[test]
fn animations_default_on_and_can_be_turned_off() {
    assert!(run("anim-default", "").unwrap().config.appearance.animations);
    assert!(!run("anim-off", "tileide.appearance { animations = false }").unwrap().config.appearance.animations);
    assert!(fails("anim-bad", "tileide.appearance { animations = 'no' }").contains("expected boolean"));
}

#[test]
fn glass_table_defaults_and_warnings() {
    let dir = config_dir("theme-glass", &[
        ("g.lua", "return { mode = 'dark', surface = '#111111', tile = '#222222', text = '#ffffff', muted = '#aaaaaa', accent = '#ffffff', glass = { blur = 12, shine = 1 } }"),
        ("bad.lua", "return { mode = 'dark', surface = '#111111', tile = '#222222', text = '#ffffff', muted = '#aaaaaa', accent = '#ffffff', glass = { opacity = 2 } }"),
    ]);
    let (t, warnings) = load_theme(&dir.join("g.lua")).unwrap();
    let g = t.glass.expect("glass table read");
    assert_eq!((g.blur, g.opacity), (12.0, Glass::default().opacity), "unset keys keep the design's values");
    assert!(warnings.iter().any(|w| w.contains("theme.glass") && w.contains("shine")), "{warnings:?}");
    assert!(load_theme(&dir.join("bad.lua")).unwrap_err().contains("theme.glass.opacity"));
    let (black, _) = load_theme(&sample_dir().join("themes/black-glass.lua")).unwrap();
    assert!(black.glass.is_some());
}

#[test]
fn written_themes_read_back_unchanged() {
    let dir = config_dir("theme-write", &[]);
    fs::create_dir_all(&dir).unwrap();
    for name in ["black-glass", "dusk", "miasma", "pale-ember"] {
        let (mut theme, _) = load_theme(&sample_dir().join(format!("themes/{name}.lua"))).unwrap();
        theme.name = format!("{} \"copy\"", theme.name);
        let path = dir.join(format!("{name}.lua"));
        fs::write(&path, tileide_config::theme_lua(&theme, "Made in the theme editor.")).unwrap();
        let (back, warnings) = load_theme(&path).unwrap();
        assert!(warnings.is_empty(), "{name}: {warnings:?}");
        assert_eq!(back, theme, "{name}");
    }
}

#[test]
fn user_themes_shadow_packaged_ones() {
    let user = config_dir("themes-user", &[("miasma.lua", "")]);
    let found = find_themes(&[user.clone(), sample_dir().join("themes")]);
    assert_eq!(found["miasma"], user.join("miasma.lua"));
    assert!(found.contains_key("dune-teal"));
}

#[test]
fn state_round_trips_and_tolerates_odd_names() {
    let dir = config_dir("state", &[]);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.lua");
    assert_eq!(load_state(&path).unwrap(), State::default(), "missing file = empty state");
    let state = State { theme: Some("we\"ird\\name\n".into()), recent_folders: vec!["/home/a b".into(), "/tmp/\"q\"".into()], ..State::default() };
    save_state(&path, &state).unwrap();
    assert_eq!(load_state(&path).unwrap(), state);

    let mut bg = tileide_config::Config::default().background;
    bg.mode = "image".into();
    bg.source = Some("~/Pictures/a \"b\".png".into());
    bg.dim = 20;
    let mut state = State { theme: Some("dusk".into()), background: Some(bg.clone()), ..State::default() };
    state.workspaces.insert(2, tileide_config::WorkspacePick { theme: Some("miasma".into()), background: None });
    state.workspaces.insert(10, tileide_config::WorkspacePick { theme: None, background: Some(bg) });
    save_state(&path, &state).unwrap();
    assert_eq!(load_state(&path).unwrap(), state, "picker choices round-trip");

    let mut state = State::default();
    let mut pick = tileide_config::ExtensionPick { enabled: false, ..Default::default() };
    pick.workspaces.insert(3, tileide_core::extension::Mode::Off);
    pick.workspaces.insert(4, tileide_core::extension::Mode::On);
    state.extensions.insert("word-count".into(), pick);
    let mut on_here = tileide_config::ExtensionPick::default();
    on_here.workspaces.insert(1, tileide_core::extension::Mode::On);
    state.extensions.insert("we\"ird".into(), on_here);
    save_state(&path, &state).unwrap();
    assert_eq!(load_state(&path).unwrap(), state, "extension choices round-trip");

    let mut state = State::default();
    use tileide_config::settings::Value;
    state.settings.insert("appearance.code_font".into(), Value::Text("Iosevka \"Term\"".into()));
    state.settings.insert("appearance.code_size".into(), Value::Number(13.5));
    state.settings.insert("sidebar.show_clock".into(), Value::Bool(false));
    state.settings.insert("appearance.motion".into(), Value::Text("snappy".into()));
    save_state(&path, &state).unwrap();
    assert_eq!(load_state(&path).unwrap(), state, "GUI settings round-trip");
    fs::write(&path, "return { settings = { [\"tiling.gaps_in\"] = 30, [\"appearance.motion\"] = \"bouncy\", [\"x.y\"] = 1 } }").unwrap();
    let loaded = load_state(&path).unwrap();
    assert_eq!(loaded.settings.len(), 1, "unknown keys and bad choices are dropped: {:?}", loaded.settings);

    let mut state = State::default();
    for i in 0..15 {
        state.remember_folder(&format!("/f{i}"));
    }
    state.remember_folder("/f3");
    assert_eq!(state.recent_folders.len(), tileide_config::RECENT_FOLDERS);
    assert_eq!(&state.recent_folders[..2], ["/f3", "/f14"], "a repick moves to the front, once");
    fs::write(&path, "return {{{").unwrap();
    assert!(load_state(&path).is_err(), "a broken state file is an error the app shows and ignores");
}

#[test]
fn state_file_cannot_escape_the_sandbox() {
    let dir = config_dir("state-escape", &[("state.lua", "os.execute('id') return {}")]);
    assert!(load_state(&dir.join("state.lua")).is_err());
}

#[test]
fn empty_config_gives_defaults() {
    let loaded = run("empty", "").unwrap();
    assert_eq!(loaded.config, Default::default());
}

#[test]
fn api_error_points_at_file_and_line() {
    let message = fails("api-error", "-- line 1\n\ntileide.appearance { window_radius = \"big\" }\n");
    assert!(message.contains("tileide.lua:3:"), "{message}");
    assert!(message.contains("appearance.window_radius: expected integer, got string"), "{message}");
}

#[test]
fn syntax_error_points_at_file_and_line() {
    let message = fails("syntax", "tileide.tiling { gaps_in = 8 }\ntileide.tiling { gaps_in = }\n");
    assert!(message.contains("tileide.lua:2:"), "{message}");
}

#[test]
fn lua_runtime_error_points_at_file_and_line() {
    let message = fails("runtime", "local x = nil\nlocal y = x.field\n");
    assert!(message.contains("tileide.lua:2:"), "{message}");
}

#[test]
fn unknown_key_warns_but_loads() {
    let loaded = run("unknown", "tileide.tiling { gaps_in = 4, gap_inn = 3 }").unwrap();
    assert_eq!(loaded.config.tiling.gaps_in, 4);
    assert_eq!(loaded.warnings.len(), 1);
    assert!(loaded.warnings[0].contains("tileide.lua:1:") && loaded.warnings[0].contains("gap_inn"), "{:?}", loaded.warnings);
}

#[test]
fn legibility_floor_is_enforced() {
    let message = fails("opacity", "tileide.background { tile_opacity = 0.5 }");
    assert!(message.contains("tile_opacity") && message.contains(&MIN_TILE_OPACITY.to_string()), "{message}");
}

#[test]
fn super_modifier_warns() {
    let loaded = run("super", "tileide.keys { mod = \"SUPER\" }").unwrap();
    assert!(loaded.warnings.iter().any(|w| w.contains("window manager")));
}

#[test]
fn api_table_is_read_only() {
    fails("readonly", "tileide.bind = function() end");
}

// --- sandbox escapes: every one of these must fail ---

#[test]
fn no_file_access() {
    fails("io-open", "local f = io.open('/etc/passwd') print(f:read('a'))");
}

#[test]
fn no_process_execution() {
    fails("os-execute", "os.execute('touch /tmp/tileide-pwned')");
    fails("io-popen", "io.popen('id')");
}

#[test]
fn no_native_libraries() {
    fails("loadlib", "package.loadlib('libc.so.6', 'system')");
    fails("ffi", "local ffi = require('ffi')");
}

#[test]
fn no_code_loading_from_strings_or_files() {
    fails("load", "load('return 1')()");
    fails("loadstring", "loadstring('return 1')()");
    fails("dofile", "dofile('/etc/passwd')");
    fails("loadfile", "loadfile('/etc/passwd')");
}

#[test]
fn no_bytecode() {
    fails("dump", "local f = string.dump(function() end)");
}

#[test]
fn no_debug_library() {
    fails("debug", "debug.sethook()");
}

#[test]
fn require_stays_inside_config_dir() {
    for (test, name) in [
        ("req-parent", "../../../etc/passwd"),
        ("req-absolute", "/etc/passwd"),
        ("req-dots", "..etc"),
        ("req-missing", "rices.missing"),
    ] {
        let message = fails(test, &format!("require({name:?})"));
        assert!(message.contains("tileide.lua:1:"), "{test}: {message}");
    }
}

#[test]
fn require_rejects_symlink_escape() {
    let dir = config_dir("req-symlink", &[("tileide.lua", "require('evil')")]);
    let outside = config_dir("req-symlink-target", &[("evil.lua", "tileide.tiling { gaps_in = 99 }")]);
    std::os::unix::fs::symlink(outside.join("evil.lua"), dir.join("evil.lua")).unwrap();
    let message = load(&dir.join("tileide.lua"), LIMITS).unwrap_err();
    assert!(message.contains("outside the config directory"), "{message}");
}

#[test]
fn errors_in_required_files_name_the_relative_path() {
    let dir = config_dir("req-error", &[("tileide.lua", "require('rices.bad')"), ("rices/bad.lua", "\ntileide.tiling { gaps_in = -1 }")]);
    let message = load(&dir.join("tileide.lua"), LIMITS).unwrap_err();
    assert!(message.starts_with("rices/bad.lua:2:"), "{message}");
}

#[test]
fn require_detects_cycles() {
    let dir = config_dir("req-cycle", &[("tileide.lua", "require('a')"), ("a.lua", "require('a')")]);
    let message = load(&dir.join("tileide.lua"), LIMITS).unwrap_err();
    assert!(message.contains("cycle"), "{message}");
}

#[test]
fn endless_loop_hits_time_limit() {
    for (test, source) in [
        ("loop-while", "while true do end"),
        ("loop-for", "local x = 0 for i = 1, math.huge do x = x + i end"),
        ("loop-recursion", "local function f(n) return f(n + 1) end f(1)"),
        ("loop-pcall", "while true do pcall(function() while true do end end) end"),
    ] {
        let started = Instant::now();
        let message = fails(test, source);
        assert!(started.elapsed() < Duration::from_secs(2), "{test} took {:?}", started.elapsed());
        assert!(
            message.contains("time limit") || message.contains("stack overflow") || message.contains("memory"),
            "{test}: {message}"
        );
    }
}

#[test]
fn memory_bomb_hits_memory_limit() {
    for (test, source) in [
        ("mem-rep", "local s = string.rep('x', 1024 * 1024 * 1024)"),
        ("mem-table", "local t = {} for i = 1, 1e9 do t[i] = i end"),
        ("mem-concat", "local s = 'x' while true do s = s .. s end"),
    ] {
        let message = fails(test, source);
        assert!(message.contains("memory") || message.contains("time limit"), "{test}: {message}");
    }
}

/// One C call with a backtracking pattern: Luau's string library checks the
/// interrupt while matching, so the time limit holds (spike 2: other flavours hang).
#[test]
fn pattern_backtracking_hits_time_limit() {
    fails("pattern", "local s = ('a'):rep(26) s:find(('a-'):rep(26) .. 'b')");
}

#[test]
fn runners_and_settings_keys_load() {
    let dir = config_dir("runner", &[("tileide.lua", "tileide.runner('.PY', 'uv run {file}')\ntileide.appearance { code_size = 14, motion = 'snappy' }\ntileide.bind('ALT', 'R', 'run')")]);
    let loaded = load(&dir.join("tileide.lua"), Limits::default()).unwrap();
    assert_eq!(loaded.config.runners.get(".py").map(String::as_str), Some("uv run {file}"));
    assert!(loaded.config.explicit.contains("appearance.code_size") && loaded.config.explicit.contains("appearance.motion"));
    assert!(!loaded.config.explicit.contains("tiling.gaps_in"));
    assert!(loaded.config.binds.iter().any(|b| b.action == Action::Run));
    let launch = config_dir("launch", &[("tileide.lua", "tileide.launch { name = 'dev', program = 'main.py', args = { '-v' }, env = { PORT = '8080' }, cwd = 'out' }")]);
    let launches = load(&launch.join("tileide.lua"), Limits::default()).unwrap().config.launches;
    assert_eq!((launches[0].name.as_str(), launches[0].args.as_slice(), launches[0].env.get("PORT").map(String::as_str)), ("dev", &["-v".to_string()][..], Some("8080")));
    let twice = config_dir("launch-twice", &[("tileide.lua", "tileide.launch { name = 'a', program = 'x' }\ntileide.launch { name = 'a', program = 'y' }")]);
    assert!(load(&twice.join("tileide.lua"), Limits::default()).unwrap_err().contains("defined twice"));
    let bad = config_dir("runner-bad", &[("tileide.lua", "tileide.runner('py', 'x')")]);
    assert!(load(&bad.join("tileide.lua"), Limits::default()).unwrap_err().contains("must look like"));
}
