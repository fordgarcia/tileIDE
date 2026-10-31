# Decisions log

One line per decision, newest last. Details live in the linked section; the spike results that drove each decision are in `../spikes/*/RESULTS.md`.

| Date | Decision | Where |
|---|---|---|
| 2026-10-31 | Name: **TileIDE** (Veil was a placeholder) | README |
| 2026-10-31 | **Rust is the whole app; Lua is for config and design only**, never a plugin language | `project_context.md` §5.2 |
| 2026-10-31 | **Arch Linux first** (PKGBUILD → AUR); other distros later | §5.2, `versions/v1.0.md` |
| 2026-10-31 | Display modes: full-screen (nothing floats over it) and windowed (a normal WM-tiled window); own modifier `ALT`, never `SUPER` | §4.5 |
| 2026-10-31 | Full-screen uses a **fullscreen toplevel**, not layer-shell (spike 1) | §4.5 |
| 2026-10-31 | Background: **windowed = none** (transparent); **full-screen = the app background across the whole canvas** | §4.4 |
| 2026-10-31 | **Frame** like Caelestia: rail + thin top/right/bottom border as one shape; the drawer = the rail widening | `design/design.md` §4.1 |
| 2026-10-31 | **Windowed mode keeps TileIDE's own gap ring, outline and rail** | §4.5 |
| 2026-10-31 | **Toolkit: R1, GTK4** (GtkSourceView + VTE). R2 skipped: R1 passed spikes 1, 3 and 4 | §5.2 |
| 2026-10-31 | GTK renderer: **cairo** (37–71 ms start, 0 dropped frames, lowest RAM) | spike 3/4 results |
| 2026-10-31 | **Lua flavour: Luau** (only flavour whose time limit holds for every config) | §6.3 |
| 2026-10-31 | Drawer **pushes** the window (Q3); width resizable with the mouse | §4.2 |
| 2026-10-31 | **UI font: Space Grotesk**; code stays monospace | `design/design.md` §4.4 |
| 2026-10-31 | A new TileIDE starts with **no tiles and empty workspaces**; the Config workspace is optional | §6.2 |
| 2026-10-31 | Workspaces: **`Alt+1…9, 0`** switches to that workspace, creating it if needed; a **+ button** under the workspace indicators creates the next free one | §6.2 |
| 2026-10-31 | **No session restore for now** (reopening past workspaces, like VS Code). Parked in the roadmap | §6.2, `versions/v1.0.md` |
| 2026-10-31 | Files section gets a VS Code-style toolbar: new file, new folder, refresh, collapse all | `versions/v0.1.md` |
