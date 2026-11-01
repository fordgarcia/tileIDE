# Changelog

## 0.4.0-alpha.9 (2026-11-01): sidebar and tile transparency + blur, DataFrame tables

### Changed
- **Settings → Themes & background:** Dim, Blur and Tiles are replaced by two groups, **Sidebar** (the rail, border and Files drawer) and **Tiles & terminals**, each with **Transparency** (0–90%) and **Blur** (0–60 px). They work in every mode and every theme. Until you move a slider each follows the theme's own look (Black Glass: 44% see-through tiles, 30 px blur). Also in `tileide.lua`: `tileide.sidebar { transparency, blur }`, `tileide.appearance { tile_transparency, tile_blur }`; set there, the slider is locked.
- Blur is drawn by TileIDE over a background it paints itself (full-screen with Image, Gradient or Solid): the sidebar and the tiles each get their own blurred copy, so you can have sharp see-through tiles beside a frosted sidebar. Over the desktop (See-through, or windowed) the page says blur comes from the compositor (Hyprland: `decoration:blur`).
- `background.dim` and `background.blur` still work from `tileide.lua` (they act on the whole background); `background.tile_opacity` is used only while `tile_transparency` is unset. Readability is still guarded: a background too bright behind see-through tiles is dimmed automatically, and the page says by how much.

### Fixed
- **The Tiles slider did nothing on glass themes** (Black Glass ignored it), and Dim/Blur/Tiles were greyed out in windowed mode. Transparency now applies in both modes and to every theme.

### Added
- **DataFrames show as tables** in notebook outputs (from their HTML): a header row, the index in bold, striped rows, wide frames scroll sideways, and pandas' "N rows × M columns" note underneath. Other HTML still shows as plain text.

## 0.4.0-alpha.8 (2026-11-01): notebooks stay put, arrow keys between cells, Markdown rendered

### Added
- **↑ / ↓ move between cells**, as in JupyterLab: on a cell's first line ↑ goes to the cell above (cursor on its last line), on its last line ↓ goes to the cell below. Inside a cell the arrows move the cursor as usual; with the completion list open they pick from it.
- **Markdown cells render:** headings, bold/italic, `code`, code blocks, lists, quotes, links and tables show formatted. Running a text cell (Shift+Enter, Ctrl+Enter, ▶) renders it; Enter or a double click edits it again. Notebooks open with their text cells rendered.

### Fixed
- **Running a cell no longer moves the page.** The old output stays (faded) until the new output replaces it, so the cell doesn't shrink and spring back; the last cell no longer jumps up. Output that grows above the cell you are typing in doesn't scroll either.
- Clicking a cell or running it never scrolls; the page only moves when the cursor would leave the screen (typing past the bottom now scrolls with you).
- Shift+Enter scrolls only as far as needed to show the next cell's first line.

## 0.4.0-alpha.7 (2026-11-01): rename, references, quick fixes, kernel completion

### Added
- **Rename** (`F2`): renames a variable, function or class everywhere the language server knows of it, across files. Open files change in their editors (unsaved); files that aren't open are rewritten on disk.
- **Find references** (`Shift+F12`): every use of the name at the cursor in a list (`app.py:12  x = area(3)`, or `cell 3:2` in a notebook); `↑`/`↓` and Enter go there.
- **Quick fixes** (`Ctrl+.`): fixes and refactors for the cursor's line from every server on the file, e.g. Ruff's "Remove unused import", "Organize imports", "Fix all auto-fixable problems". Preferred fixes come first.
- **Completion from the running kernel** in notebook cells: names that only exist at run time, such as a DataFrame's columns (`df.pr` → `price`), join pyright's suggestions (marked "kernel").
- **Format a notebook cell** with `Ctrl+Shift+I` (Ruff). Cells with `%` or `!` lines are left alone, so magics are never lost.

### Fixed
- **Adding a notebook cell still jumped to the top.** Two causes: the scroll was computed in already-scrolled coordinates (it also hit Shift+Enter), and "+ Code" added below the *current* cell, which only changed when you clicked into a cell's editor, so after scrolling down the new cell landed under cell 1. Now any click in a cell makes it current, "+ Code" adds below the last visible cell when the current one is out of sight, and the new cell is scrolled into view.
- **Shift+Enter now focuses the next (or new) cell and keeps it in view**, also when the output of the cell you ran pushes it down.
- Edits that share a start position (formatters, quick fixes) are applied in the right order.

## 0.4.0-alpha.6 (2026-11-01): Python and Jupyter as a full IDE

### Added
- **Code suggestions with Tab:** while you type, the best completion shows faded after the cursor (`prin` → `print`); `Tab` (or `Enter`) takes it, `↑`/`↓` pick another from the list.
- **Signature help:** typing a call (`pd.DataFrame(`, `print(`) shows its parameters above the cursor, the current one in bold, with its doc line.
- **Hover shows signatures** in the code font (`class DataFrame(data=None, index=None, …)`), then the documentation.
- **Linting with Ruff** (`ruff server`, optional) beside pyright: its diagnostics show with pyright's. **Format the file** with `Ctrl+Shift+I` (`format` action).
- Unused code (unused imports, variables) is faded instead of marked.
- **Notebook cells get language support:** completion, suggestions, signature help, hover, diagnostics and go to definition, across cells (a name from an earlier cell is known). IPython lines (`%matplotlib`, `!pip`) are understood; rules that don't fit cells (a bare `df` at the end, imports in any cell) stay quiet.
- `Alt+I`, `F12`, `Ctrl+Space` and `Ctrl+Shift+I` work in the current notebook cell.

### Fixed
- **Python completion didn't start when TileIDE was launched from the desktop:** language servers are now also looked for in the project's virtualenvs, `~/.local/bin`, npm/cargo globals and mise's install folders, and a server found that way runs with its own folder first on `PATH` (an npm-installed pyright needs the `node` next to it).
- pyright analyses with the project's virtualenv, so installed packages (pandas, numpy, …) resolve.
- **Adding a notebook cell jumped to the top.** The view now follows the new cell; deleting or moving a cell keeps it in view too.
- **Closing a tile focused the first tile;** focus now goes back to the tile it was split from.

### Known limits
- Not yet: rename, find references, quick fixes, completion from the running kernel (e.g. DataFrame columns), formatting notebook cells.
- JavaScript debugging and launch configs moved to v0.6 (language parity).

## 0.4.0-alpha.5 (2026-11-01): Java and Cargo debugging, venvs, unlockable settings, alerts

### Added
- **Java debugging** (`F5` on a `.java` file) with the JDK's own `jdb`: TileIDE compiles the file with `javac -g` (other classes of its source tree are compiled as needed, packages are followed), then debugs it with breakpoints, stepping, the call stack and variables (objects and arrays expand). No Java debug adapter or language server is needed; TileIDE drives jdb itself (`tileide dap-jdb`, started by TileIDE).
- **Cargo project debugging:** `F5` on a `.rs` file inside a Cargo project builds that binary (`cargo build -p … --bin …`; the binary is the one built from the file, else the package's) and debugs it with gdb.
- **Settings → Alerts → Missing tools** (`tileide.alerts { missing_tools = false }`): turns off the banner popups about a missing language server, debugger, Jupyter or Python module. The message still shows where the action ran (run card, debug tile, notebook cell).
- A Python script or notebook cell that stops on `ModuleNotFoundError` gets the install command for the Python that ran it (`<venv>/bin/python -m pip install x` in a virtualenv, `sudo pacman -S python-x` for the system Python).
- **Unlock** on a setting your `tileide.lua` sets: TileIDE comments that line out (`-- … -- now set in Settings`), keeps its value, and the row becomes editable. A key that shares its line with other keys, or comes from a rice file, is left for you to remove.
- Settings → Toolchains lists jdb, and finds Jupyter/debugpy for the distro's `/usr/bin/python3` too.

### Fixed
- **Virtualenvs in parent folders weren't found.** Running (`Alt+R`), debugging (`F5`) and notebooks now use the nearest virtualenv at or above the file's folder (up to the workspace folder): `.venv`, `venv`, `env`, or any folder with a `pyvenv.cfg`; an active `$VIRTUAL_ENV` comes first. Before, `Alt+R` and `F5` always used `python3` and notebooks looked only in the workspace folder itself.
- Notebooks and Python debugging also try `/usr/bin/python3` when `python3` on PATH is another Python (mise, pyenv, asdf): that's where `pacman` installs `python-ipykernel` and `python-debugpy`.
- A virtualenv without Jupyter no longer silently gives a kernel without its packages: an alert says so and how to add `ipykernel` to it.

### Known limits
- Java: single source trees compiled by `javac` (no Maven/Gradle classpath); breakpoints in nested or anonymous classes aren't set; the program has no stdin. JavaScript debugging is not built.
- gdb prints "No source file named …" for a breakpoint set before the program loads; the breakpoint is still hit.

## 0.4.0-alpha.4 (2026-11-01): notebooks, debugging, toolchains

### Added
- **Jupyter notebooks:** `.ipynb` files open as a notebook tile: Markdown and code cells, each with its outputs underneath (text, results, errors with tracebacks, images such as plots). `Shift+Enter` runs a cell and moves on, `Ctrl+Enter` runs it in place, `Alt+R` (or *Run all*) runs every cell; *Interrupt*, *Restart*, *+ Code*, *+ Text*, and per cell: run, move up/down, code ⇄ text, delete. The kernel starts on the first run, using your Jupyter (the workspace's `.venv` first, then `python3`). `Ctrl+S` saves valid nbformat 4 that opens in Jupyter and VS Code; unknown fields are kept.
- **Debugging:** `F9` sets a breakpoint (a dot in the gutter), `F5` builds the focused file with debug info and starts its debugger, or continues; `F10` step over, `F11` step into, `Shift+F11` step out, `Shift+F5` stop. A debug tile opens beside the code with the call stack (click a frame to go there), variables (click to expand) and the program's output; the paused line is highlighted. C, C++ and single-file Rust use gdb's built-in debugger (gdb 14+); Python uses debugpy. Actions: `debug`, `debug_stop`, `breakpoint`, `step_over`, `step_into`, `step_out`.
- **Settings → Toolchains** (action `toolchains`): which interpreters, compilers, debuggers and Jupyter this machine has (version and path), and the `pacman` command for the missing ones.

- **Icon backgrounds** can be turned off (Settings → Sidebar, or `tileide.sidebar { icon_backgrounds = false }`): the rail's icons then stand alone, and an active one is drawn in the accent colour.

### Changed
- **Opening a file is smoother:** the Fluid motion now leaves at once instead of easing in from rest (it read as a delay after Enter), fades ease out, and durations are a little shorter (glide 380 ms, fade in 240 ms, out 160 ms). The editor's languages and colour scheme load right after startup, so the first file opened no longer stalls (21–34 ms → 10 ms), and a new file's language server starts once its tile has settled, not during the glide. Measured on Hyprland at 60 Hz: 16 of 16 opening glides without a dropped frame.
- The file-tree button at the top of the rail shows a **file-tree icon**; the TileIDE **logo** now sits in the middle of the rail where the workspace name was (Settings → Sidebar → Logo; the audio visualizer still takes its place while music plays).

### Fixed
- **TileIDE froze when you closed an editor that had language support** (a Rust, C/C++, Python, JS/TS or Java file in a workspace with a folder; it happened even when the language server had failed to start). The window stopped redrawing and couldn't be closed. Cause: the completion popup was attached to the editor and removed only after GTK's own teardown, whose cleanup loop (`while (first_child) gtk_text_view_remove(...)`) never ends on a child it didn't add itself. TileIDE now removes its popups before an editor goes away: on close, on Alt+F replace, and when a workspace is removed.
- `Alt+Shift+W` crashed TileIDE when the workspace had no unsaved files and no terminals (a borrow held across the removal). Present since 0.3.0-alpha.7.

### Known limits
- Notebooks: Markdown cells are edited as text (not rendered); rich HTML outputs show their text form; no language server inside cells.
- Debugging: no Java or JavaScript adapter yet; Cargo projects aren't debugged (single `.rs` files are); Python debugging uses `python3` on PATH, not the workspace's `.venv`.

## 0.4.0-alpha.3 (2026-11-01): audio visualizer, sidebar size

### Added
- **Audio visualizer:** while something plays, a symmetrical waveform (frequency bands mirrored around the middle, each bar drawn equally on both sides of a centre line) takes the place of the workspace name in the rail; with the drawer open it sits under the file tree instead. It appears only while there is sound and fades out a few seconds after it stops. Nothing is captured while nothing plays: `pactl subscribe` reports playback, and only then does `parec` read the speakers' monitor. Turn it off in Settings → Sidebar or with `tileide.sidebar { show_visualizer = false }`.
- **Sidebar size** and **Icon size** in Settings → Sidebar (`tileide.sidebar { scale = 1.2, icon_scale = 1.1 }`): the rail's width with everything on it (80–150%), and its icons and dots within it (80–125%). The drawer, peek and banner move with the rail.
- `tileide render … --visualizer --rail-scale N --icon-scale N` previews them offscreen.

### Changed
- The Config workspace's rail entry is a **folder with a cog** instead of a second gear, so it can't be mistaken for Settings (the gear above the clock).

### Known limits
- The visualizer's capture is an input stream: status bars that show "recording" for any input may light up while music plays.

## 0.4.0-alpha.2 (2026-11-01): run, empty workspace

### Added
- **Run** (`Alt+R`, action `run`): runs the focused file (saved first) and shows its output on a large card over a dimmed window, with padding, a status chip (Running… / Exit 0 · 0.42 s / Exit 2 / Stopped) and the keys always in view: `Ctrl+C` stop, `Alt+R` run again, `Esc` close (stops a running program). Built in: Python, JavaScript (Node), shell, Lua, Ruby, Go, Java (single file), C and C++ (compile, then run), Rust (`rustc`, or `cargo run` inside a Cargo project). With no file focused it runs the workspace's Cargo project, or the last run again.
- `tileide.runner(".ext", "command {file}")` adds or replaces a runner (`{file}`, `{dir}`, `{stem}`, `{out}`; run by `sh -c`).
- A missing compiler or interpreter says which package to install (`sudo pacman -S …`); nothing is installed for you.

### Changed
- **An empty workspace looks like an empty editor:** one tile fills the code region with a faded logo and the main keys centred in it (like VS Code with no file open), instead of a small card.

### Known limits
- Programs get no input (stdin): run interactive ones in a terminal tile. Output is plain text (colours are removed); stdout and stderr share one stream so lines stay in order.

## 0.4.0-alpha.1 (2026-11-01): Settings, keybinds card, rail rework

### Added
- **Settings** (`Alt+,`, action `settings`, or the gear above the clock): a large card in the quick-open style. Search on top filters every setting; sections on the left. Everything applies live and is kept in `state.lua`:
  - Appearance: interface font and size, code font and size.
  - Layout: rounded corners, gaps between tiles and at the edge.
  - Motion: animations on/off, style (**Fluid**, the new default: soft spring glides and eased fades, iOS-like; or **Snappy**, the previous timing) and speed (0.5–2×).
  - Sidebar: show or hide each rail item; start with the files open.
  - **Themes & background** and **Extensions** are pages of Settings now (`Alt+Shift+T` / `Alt+Shift+X` open them there).
  - A key `tileide.lua` sets wins and shows "Set in tileide.lua" (locked). The sample config leaves these keys commented for that reason.
- New `tileide.appearance` keys: `motion`, `motion_speed`, `ui_font`, `ui_scale`, `code_font`, `code_size`.
- **Keybinds card** (`Alt+K`, action `keybinds`): every active key, filtered as you type. Esc closes.
- **`Alt+=` / `Alt+-`** (action `font_size`, `up`/`down`): the code font of every editor and terminal, one point at a time (saved).
- The workspace peek has an **×** that removes that workspace (asks first if it holds work).

### Changed
- **Focus, resize and move use the arrow keys:** `Alt+←↑↓→`, `Alt+Shift+arrows`, `Alt+Ctrl+Shift+arrows` (were H/J/K/L), so `Alt+K` is free for the keybinds card. Binds in your `tileide.lua` still win.
- **The Config workspace's gear sits under the workspace dots** (above `+`); the gear above the clock opens Settings.
- **Hovering a workspace dot shows its peek;** a click goes to the workspace (it used to take a click to peek and a second to go).
- The drawer holds only Files; its Files | Themes | Extensions switcher is gone (both moved into Settings).
- The drawer's open/close follows the motion style too.

## 0.3.0 (2026-11-01): extensions, drawer fixes

### Added
- **Extensions**: programs in any language that TileIDE starts only when needed and talks to over stdin/stdout (JSON-RPC, the framing language servers use). Each runs as its own process, so a crash or a slow one can't take TileIDE down. Format and protocol: [docs/extensions.md](docs/extensions.md); an example in `examples/extensions/word-count/`.
- **Extensions section in the drawer** (`Alt+Shift+X`, action `extensions_section`, or the Extensions tab): search, install from a folder or a git URL, remove, an on/off switch, and a per-workspace mode (On demand, Always here, Off here). Choices are saved in `state.lua`.
- **Weight meter:** startup time, memory and CPU of every running extension, live while the section is open, with Suspend/Resume and Stop. Extensions that ask to start with TileIDE are marked.
- Run an extension command from a key: `tileide.bind("ALT", "Y", "extension", "word-count.count")`.
- [docs/compatibility.md](docs/compatibility.md): what imports and extensions work, and what doesn't.

### Fixed
- The drawer's content no longer spills over the code region: the panel is exactly as wide as the drawer, rows of buttons wrap onto a second line, and long file names are shortened inside it (checked at 200, 220 and 600 px).
- The Themes section is filled whenever it shows. Opened from its tab it showed an empty theme list, every background row at once, red placeholder colours and sliders at 0.

## 0.3.0-alpha.9 (2026-11-01): Themes section, background picker, VS Code themes

### Added
- **Themes section in the drawer** (`Alt+Shift+T`, action `themes_section`, or the drawer's Files | Themes switch). Hover a theme (or move to it with ↑/↓) to preview it on the real window; click or Enter applies it to **all workspaces** or **this workspace** only. Picks are saved in `state.lua`; `tileide.lua` still wins, and the section says when it does.
- **Background picker** in the same section (full-screen mode): None, Image, Gradient or Solid; an image file chooser and fit; colour pickers; Dim, Blur and Tiles sliders; all live. It shows the text contrast the legibility check measured and says when it dimmed the background to keep text readable. Per-workspace like themes.
- **Import VS Code theme…**: a theme `.json` or an extension `.vsix` (e.g. from Open VSX) becomes a theme file in `~/.config/tileide/themes/` (every colour theme in a `.vsix`). Workbench colours, syntax colours and terminal colours are mapped; nothing from the extension is run. Low-contrast imports are reported.
- Theme files may set their own syntax and terminal colours: `syntax = { keyword = "#…", … }` and `terminal = { red = "#…", … }`. Anything left out is still derived from the theme.

### Changed
- The editor's line-number column is transparent like the rest of the tile (the GTK theme still painted its border area).
- The Arch package depends on `libarchive` (for `bsdtar`, to read `.vsix` files).

## 0.3.0-alpha.8 (2026-11-01): language servers

### Added
- **Language servers** for Rust (rust-analyzer), C/C++ (clangd), Python (basedpyright, pyright or pylsp), JavaScript/TypeScript (typescript-language-server) and Java (jdtls). TileIDE starts the one you have installed when a matching file opens, one per workspace folder; a workspace without a folder starts none. A missing or failing server shows a notice with how to get it.
  - **Diagnostics:** a squiggle under the code plus an icon in the gutter (shape, not just colour); hover either for the message.
  - **Hover:** the mouse shows types and docs; `Alt+I` (action `hover`) shows them at the cursor.
  - **Completion:** opens on `.`, `::` and other trigger characters, after two letters of a word, or with `Ctrl+Space` (action `complete`). Typing filters it; ↑/↓ select, Enter or Tab insert, Esc closes.
  - **Go to definition:** `F12` (action `definition`) or Ctrl+click; another file opens in a tile.
  - `tileide.lsp(name, { cmd = "…", enabled = false })` changes a server's command or turns it off.
- Servers stop when no workspace uses their folder any more, and when TileIDE quits.

### Fixed
- A workspace whose folder was picked with `Alt+D` is no longer removed when you leave it before opening a tile. Only workspaces with neither tiles nor a folder are removed.

## 0.3.0-alpha.7 (2026-11-01): workspace folders, themed editors

### Added
- **Editors match the theme.** Code text, syntax colours, cursor, selection and bracket matches come from the theme, using the same hues as the terminal (keywords, types, strings, numbers, functions, comments), so code reads alike in an editor and in `bat` or `git diff`.
- **Folder search** (`Alt+D`, new action `folder`): the quick-open card lists folders, recent ones first (kept in `state.lua`), then folders under `~` (6 levels, hidden and build folders skipped). A typed path (`~/notes`, `/etc`) works too. Enter sets the workspace's folder: the Files drawer, new terminals and quick open follow it.
- **Remove a workspace:** `Alt+Shift+W` (new action `workspace_close`) or the × on its overview card. Unsaved files or open terminals are named in a confirmation first.
- Rail dots grow and fade in when a workspace is created and shrink out when it is removed.

### Changed
- **New workspaces start empty, without a folder**; the folder search opens by itself. Only the launch workspace gets the folder TileIDE was opened on; workspaces defined in `tileide.lua` keep their `folder`. The Files drawer says "No folder · Alt+D" until one is picked.
- **Leaving a workspace with no tiles removes it**, except the last one and workspaces defined in `tileide.lua`. Numbers never shift (removing 3 leaves 1, 2, 4); `+` takes the lowest free number.

## 0.3.0-alpha.6 (2026-11-01): calmer motion, themed terminals

### Changed
- **Motion retuned to feel calm.** Fades use a gentler curve (they used to reach half opacity in 40 ms and read as a pop). Things that appear fade in over 180 ms; things that leave fade out over 120 ms; tiles glide over 220 ms.
- **Workspace switches crossfade:** the workspace you leave fades out while the next fades in, instead of a cut to the bare background.
- **Everything leaves gently:** closing a tile fades it while its neighbours glide into the space; the theme tray, overview, quick open and confirmations fade out instead of vanishing.
- **A new tile grows out of the tile it splits from**, instead of appearing on top of its neighbours.
- **Theme changes crossfade** (tray pick or config edit), instead of snapping.
- Fades can be interrupted: switching workspaces quickly no longer flickers, and a fade reversed halfway continues from where it is.
- Idle windows no longer wake 60 times a second for tile animation; the frame callback only runs while something moves.

### Added
- **Terminals match the theme.** The 16 ANSI colours (prompts, `ls`, `git`, compilers) come from the theme: red is its error colour, the other hues are soft and leaned toward its accent, and each is checked to stay readable on the tile (≥ 4.5:1). The cursor uses the accent; the selection is an accent tint.

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
