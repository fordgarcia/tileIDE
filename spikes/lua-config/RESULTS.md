# Spike 2 results: embedded Lua config

v0.0 item 6 (`../../project_contexts/versions/v0.0.md`). Measured on 2026-10-31 on the dev machine (Arch / Hyprland 0.56.2, rustc 1.99.1, mlua 0.12.2 vendored).

## What the spike does
- Runs `tileide.lua` in a **fresh** sandboxed state on every load. The `tileide.*` calls fill a typed `Config` (`src/model.rs`), and every option has a default.
- The sample from spec §6.3 loads, including `require("rices.nord")` overriding values.
- Errors read `file:line: section.key: message`, using the path relative to the config dir (e.g. `rices/bad.lua:2:`). Unknown keys only warn.
- `watch` hot-reloads on save. A broken save prints the error and keeps the last good config.
- `check` is the prototype of `tileide --check-config`.

## Commands
```sh
cargo run -- check examples/config/tileide.lua [--dump]
cargo run -- watch examples/config/tileide.lua
cargo test                                                   # Lua 5.4
cargo test --no-default-features --features luau --target-dir target/luau
```

## Numbers (release build, stripped)
| Flavour | Binary | Lua's share¹ | Cold load² (median of 500) | Tests |
|---|---|---|---|---|
| Lua 5.4 | 1278 KiB | ≈ 0.7 MB | 0.15 ms | 21/22 (pattern gap) |
| Lua 5.5 | 1284 KiB | ≈ 0.7 MB | 0.16 ms | 21/22 (pattern gap) |
| LuaJIT | 1530 KiB | ≈ 0.95 MB | 0.16 ms (JIT must be off) | 21/22 (pattern gap) |
| **Luau** | 2604 KiB | ≈ 2.0 MB | 0.42 ms | **22/22** |

¹ Compared with a 579 KiB baseline binary with the same `notify` dependency and no Lua.
² New state + sandbox + the full sample config + rice.

Hot reload: a saved change is applied about **31 ms after the save**, 30 ms of which is the deliberate debounce (spec budget: < 100 ms).

## Sandbox findings
All flavours block the following (tests in `tests/sandbox.rs`): `io`, `os`, `package` / `loadlib` / `ffi`, `debug`, `load` / `loadstring` / `dofile` / `loadfile`, bytecode (`string.dump`, text-only chunks), `require` outside the config dir (`..`, absolute paths, symlinks), require cycles, writes to the `tileide` table, endless loops / recursion, and memory bombs (32 MB limit).

1. **pcall escape (found and fixed).** `while true do pcall(function() while true do end end) end` ran forever: the hook always fires inside the inner loop, and `pcall` swallowed the time-limit error. Fix: `pcall` / `xpcall` re-raise after the deadline (`GUARD` in `src/sandbox.rs`).
2. **LuaJIT needs `jit.off()`.** With the JIT on, compiled loops never call the hook, and the endless-loop test hung (verified). LuaJIT therefore loses its only advantage here.
3. **Pattern backtracking (Lua 5.4 / 5.5 / LuaJIT only).** `('a'):rep(26):find(('a-'):rep(26)..'b')` is one C call that runs for more than 20 s, and the hook cannot stop it. **Luau stops it at the time limit** (its string library checks the interrupt). In the other flavours, an untrusted mod could hang the app this way unless string patterns are removed or the config runs in a separate process.

## Recommendation: Luau
- It is the only flavour where the time limit is a real guarantee. That matters for v0.5 (shared rices and mods are untrusted, T14).
- It has a built-in sandbox mode (read-only built-ins, no `io` library at all).
- Its syntax is Lua 5.1 with additions, so it's familiar to Neovim-style config users.
- Costs: about +1.3 MB binary vs Lua 5.4, and 0.4 ms vs 0.15 ms per load (both negligible against the budgets). It has no integer subtype (handled), and a few syntax differences from 5.4 (no `goto`, no `<const>`).
- Runner-up: **Lua 5.4**, if you'd rather have standard Lua. It would then need string patterns removed from the mod tier.

## Not covered (left for later versions)
- The mod tier's stricter API (setters only), `tileide.host` / `tileide.env`, and the opt-in for running commands (Q16).
- The typed model uses validated strings for enums; real enums come in v0.2.
