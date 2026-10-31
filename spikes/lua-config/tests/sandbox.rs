//! Gate 1 checks for the config engine: sample loads, errors carry file:line,
//! and deliberate sandbox escapes fail. Runs per flavour:
//!   cargo test --no-default-features --features luau

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tileide_lua_spike::{Limits, Loaded, load};

const LIMITS: Limits = Limits { time: Duration::from_millis(100), memory: 16 * 1024 * 1024 };

/// A private config dir per test, holding `tileide.lua` plus optional extra files.
fn config_dir(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tileide-spike-{}-{test}", std::process::id()));
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

#[test]
fn sample_config_loads_and_rice_overrides() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/config/tileide.lua");
    let loaded = load(&path, Limits::default()).expect("sample config");
    let c = &loaded.config;
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(c.appearance.theme, "nord", "rice overrides the theme");
    assert_eq!(c.appearance.window_radius, 14);
    assert_eq!(c.appearance.gap_outer, 30, "keys the rice leaves alone survive");
    assert_eq!(c.tiling.gaps_in, 10);
    assert_eq!(c.keys.modifier, "ALT");
    assert_eq!(c.binds.len(), 24);
    assert_eq!(c.workspaces[&2].folder.as_deref(), Some("~/notes"));
    assert_eq!(c.lsp["rust"], "rust-analyzer");
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
    assert!(message.contains("tile_opacity"), "{message}");
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

/// One C call with a backtracking pattern never returns to the VM, so the
/// instruction hook of Lua 5.4 / 5.5 / LuaJIT cannot stop it (reproduce with
/// `timeout 15 cargo test -- --ignored`). Luau's string library checks the
/// interrupt while matching, so there the time limit holds.
#[test]
#[cfg_attr(not(feature = "luau"), ignore = "hangs: the hook cannot interrupt a single C call")]
fn pattern_backtracking_is_not_interruptible() {
    fails("pattern", "local s = ('a'):rep(26) s:find(('a-'):rep(26) .. 'b')");
}
