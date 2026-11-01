# Changelog

## 0.3.0-alpha.5 (2026-11-01): editor look, dialogs

### Changed
- Editors: the line-number gutter has no background (the tile shows through), and the current line is a faint white band instead of a dark one. Line numbers use the theme's muted colour; the current one its text colour.
- **Confirmations are TileIDE's own** (save changes, replace a terminal, quit with unsaved files): a rounded card over a dimmed window with pill buttons. Cancel is quiet, losing something (Don't Save, Replace, Quit Without Saving) is outlined in the theme's error colour, Save is filled with the accent. Enter picks the default, Esc or a click outside cancels, ←/→ and Tab move between buttons.
- Replacing a terminal from `Alt+F` now defaults to Cancel (ending a shell is never the default).

## 0.3.0-alpha.4 (2026-11-01): quick open

### Added
- **Quick open.** A rounded search box over the window, like a browser's new-tab bar, searches the workspace's files as you type (fuzzy: `tilev` finds `tile_view.rs`). ↑/↓ or Ctrl+N/P select, Enter opens, Esc or a click outside closes.
  - `Alt+E` opens the file in a new tile; Enter on an empty search still gives a blank file.
  - `Alt+F` (new action `search`) opens it in the focused tile. An editor with unsaved changes asks to save first; a terminal asks before it is replaced, since its shell ends. A file already open elsewhere is focused instead.
  - Hidden files and `target`, `node_modules`, `__pycache__`, `build`, `dist`, `venv` are skipped; indexing runs off the main thread.

## 0.3.0-alpha.3 (2026-11-01): warm themes, smooth motion

### Changed
- **Warm themes.** Dusk (the default) moves from teal to warm charcoal with a peach accent. Dune Teal and Miasma are slightly warmer. Black Glass has a warm charcoal frame and a warm-white accent.
- **Pale Void is now Pale Ember** (pale peach on brown-black). A `state.lua` or config that names `pale-void` falls back to the default theme with a notice; pick Pale Ember in the tray.

### Added
- **Smooth motion by default.** Tiles glide to their new place when they open, close, resize, move or swap, on `Alt+V`, and when columns scroll. New tiles, a switched-to workspace, the theme tray and the overview fade in. All use the drawer's 200 ms ease-out. `tileide.appearance { animations = false }` turns it off, and so does the desktop's "no animations" setting.

## 0.3.0-alpha.2 (2026-11-01): fixes

### Fixed
- Rail buttons crowded the pills' edges: buttons are now 28px with 6px padding on each side (the logo button too).
- `Alt+V` with one tile shrank it to half the width. A lone column now fills the code region; column widths apply from two columns on.

## 0.3.0-alpha.1 (2026-11-01): v0.3 phase 1, new look

### Added
- **New logo** (a smiling sword with a sparkle). It is the app icon and the Files opener at the top of the rail.
- **Black Glass** theme: a charcoal frame and smoked-glass tiles. Over a full-screen background (image, gradient or solid), each tile shows a blurred, saturated copy of the background behind it. In see-through and windowed mode TileIDE can't see what's behind the tiles, so the glass is more opaque (`min_tint`). Any theme can turn on glass with a `glass = { … }` table.
- **Pale Void** theme: near-black indigo with a pale lilac accent.
- `tileide render … --theme FILE.lua` renders the offscreen preview in any theme.

### Changed
- The rail's modules sit on two pills: the opener and the workspaces at the top; the palette, the gear and the clock at the bottom. The hairline under the opener is gone.

## 0.2.0 (2026-11-01): config, columns, themes

### Added
- **The config is applied.** `~/.config/tileide/tileide.lua` runs in a sandboxed Luau (no file, process or network access; time and memory limits). Saved changes apply at once. A mistake shows a banner with `file:line` and the last working config stays active. `tileide --check-config [FILE]` checks a config from the shell.
- Configurable: layout (`dwindle` or `columns`), gaps, tile radius, column width, every key (`tileide.bind`), sidebar modules, the full-screen background, and per-workspace name, folder, layout, theme and background (`tileide.workspace(n, { … })`). Rices in `rices/` can override any of it.
- **Scrolling columns layout** (niri-style). `Alt+V` switches a workspace between dwindle and columns, keeping its tiles. In columns, `Alt+H/L` moves between columns (the view scrolls), `Alt+J/K` within one, `Alt+Shift+H/L` resizes, `Alt+Ctrl+Shift+H/J/K/L` moves.
- `Alt+S`: the next tile opens below the focused one (in columns, it stacks in the column).
- `Alt+Ctrl+Shift+H/J/K/L` swaps tiles in dwindle too.
- **Themes** (`themes/*.lua`, colours only) with **Dune Teal** (light) and **Miasma** next to **Dusk**. The palette button or `Alt+T` opens the theme tray: ←/→ browse, Enter applies, Esc closes. The pick is remembered in `state.lua`, unless `tileide.lua` sets a theme, which always wins.
- **Workspace overview** (`Alt+O`): one card per workspace with its name, folder, tiles and layout.

### Changed
- Bare colours: no glows. The focused tile has a plain accent border; the workspace pill has no glow.
- Narrower sidebar for more desktop: the rail is 48px, the Files drawer 220px by default.
- In the columns layout, tiles scrolled past an edge slide under the frame's rounded corner instead of being cut straight.
- Terminals start in their workspace's folder.
- The Files drawer remembers which folders were open in each workspace's tree.
- The vertical label shows the workspace's name.

## 0.1.0 (2026-11-01): shell

The redesigned shell from `themes/theme-design.md`, and the v0.1 exit criteria.

### Changed
- New layout: a 60px rail and a 288px drawer. Tiles sit straight in the frame's rounded hole (radius 30), 16px from its edge and 10px apart, with 14px corners. The outlined window inside the frame is gone.
- Redrawn rail: a file-tree opener (accent ring, filled while the tree is open), a hairline, a glowing pill for the current workspace, dots for the others, a dashed + ring, a letter-spaced vertical label, the gear in a rounded square, and the clock with a hairline between hour and minute.
- Focused tile: 2px accent border with a 4px halo; terminals get 18/24px padding.
- All colours come from one theme (the built-in "Dusk"), ready for theme files in 0.2.
- The source is a Cargo workspace: `crates/core` (tile tree, drawer, tokens, theme) builds and tests without a display.

### Fixed
- Unsaved changes are never lost silently: closing a modified editor asks to save, and quitting asks while any file is unsaved.
- Opening a file that is already open focuses its tile instead of opening a second copy.
- The empty-workspace hint sits on a card, so it stays readable on any background.

## 0.0.0 (2026-10-31): first preview

The go/no-go prototype, packaged so it can be tried.

### Added
- Canvas: a rail and a thin frame drawn as one shape (Caelestia-style), with a gap ring and an outlined window.
- Tiling: editor (GtkSourceView) and terminal (VTE) tiles; split, close, focus and resize from the keyboard.
- Workspaces 1–10 (`Alt+1…9, 0`, or the `+` button) plus a config workspace (`Alt+C` or the gear).
- Files drawer for the active workspace: new file/folder, refresh, collapse, resizable by drag.
- Workspace preview: click a workspace dot to see its files without switching.
- Word wrap (`Alt+Z`), save (`Ctrl+S`); terminals close when their shell exits.
- Full-screen background: image, gradient or solid colour, with dim, tint, blur, fit, tile opacity, and automatic dimming that keeps text at ≥ 4.5:1 contrast.
- `tileide` command: open a folder or a file, full-screen or windowed. It returns to the shell like `code`.
- First run copies a sample config to `~/.config/tileide/`.
- Arch Linux PKGBUILD, desktop entry and icon.

### Known limitations
See "Known limits" in the README; also: the config file isn't applied yet, opening an already open file opens a second copy (fixed in 0.1.0), there is no unsaved-changes prompt (fixed in 0.1.0), and drawer animations can drop a frame over an image background.
