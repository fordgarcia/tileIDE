# Changelog

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
See "Known limits" in the README; also: the config file isn't applied yet, opening an already open file opens a second copy, there is no unsaved-changes prompt, and drawer animations can drop a frame over an image background.
