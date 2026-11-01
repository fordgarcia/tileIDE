<div align="center">

<img src="dino.svg" alt="A pixel dinosaur in a red scarf runs along the ground and jumps over cacti" width="100%">

</div>

<br>
<br>

# Welcome to TileIDE! 
This is an IDE tailored to how ricers experience unique and aesthetic desktop designs. 
Here, users can have workspaces, tiling-level experience, extension, mods, and even
have the config files of the application in a workspace by default (we really need this)!
This is like you riced a desktop for a year, and you can code with it.


## Features
* Tiling / split windows with custom super-key
* Typical tiling window manager config and design control
* Workspaces w/ default config workspace for application configs (you can turn this off)
* VIM level controls (optional)
* Typical IDE features such as extensions, mods, CLI, debugger, and execute
* Community extensions and mods


## Why I Made This
Tired of switching tiling window managers and customizing configs of IDE per switch. <br>
(tired of larping other distros)

<br>

> **Status: v0.4.0-alpha.4** (Settings, Run, notebooks, debugger, audio visualizer). Tiling editors and terminals (dwindle or niri-style scrolling columns), workspaces, a Files drawer, themes (and VS Code theme import), language servers, extensions, and a Lua config that applies as you save it. No debugger yet. Install details: [docs/install-arch.md](docs/install-arch.md).

![TileIDE](docs/1.png)
![TileIDE](docs/2.png)
![TileIDE](docs/3.png)
![TileIDE](docs/4.png)
@creadits to the artist of the wallpaper


# Quick Start
Arch
```
sudo pacman -S --needed base-devel git rustup gtk4 gtksourceview5 vte4
rustup default stable
git clone https://github.com/fordgarcia/tileIDE.git
cd tileIDE/packaging/arch && makepkg -si
tileide .
```
Optional font: `otf-space-grotesk` (AUR). Uninstall with `sudo pacman -R tileide`.

Debian / Ubuntu (untested; needs GTK 4.12 or newer, e.g. Ubuntu 24.04+)
```
sudo apt install build-essential git libgtk-4-dev libgtksourceview-5-dev libvte-2.91-gtk4-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/fordgarcia/tileIDE.git
cd tileIDE && cargo install --path .
tileide .
```

## Usage
```
tileide                  # open the current folder (full-screen)
tileide ~/code/project   # open a folder
tileide src/main.rs      # open a file
tileide --windowed .     # open as a normal window
tileide --help           # all options, incl. backgrounds (--bg image:FILE)
```

| Keys | Action |
|---|---|
| `Alt+Return` / `Alt+E` | new terminal / open a file in a new tile (search; Enter on an empty search = blank file) |
| `Alt+F` | open a file in the focused tile (search) |
| `Alt+W` / `Ctrl+S` | close tile (asks if unsaved) / save |
| `Alt+←↑↓→` (+`Shift`) | move focus (resize) |
| `Alt+Ctrl+Shift+←↑↓→` | move the tile (column) |
| `Alt+V` / `Alt+S` | dwindle ⇄ scrolling columns / next tile opens below |
| `Alt+T` / `Alt+O` | theme tray / workspace overview |
| `Alt+1…9, 0` | workspaces |
| `Alt+B` / `Alt+C` | files drawer / config workspace |
| `Alt+D` / `Alt+Shift+W` | pick the workspace's folder / remove the workspace |
| `Alt+R` | run the focused file (output card; `Ctrl+C` stop, `Esc` close); in a notebook, run all cells |
| `F9` / `F5` | breakpoint / debug (start or continue); `F10` over, `F11` into, `Shift+F11` out, `Shift+F5` stop |
| `Alt+,` / `Alt+K` | settings / keybinds |
| `Alt+=` / `Alt+-` | code font bigger / smaller |
| `Alt+Shift+T` / `Alt+Shift+X` | settings: themes & background / extensions |
| `Alt+I` / `F12` / `Ctrl+Space` | hover info / go to definition / complete (language servers) |
| `Alt+Z` / `Alt+Escape` | word wrap / quit (asks if unsaved) |

Mouse: hovering a workspace dot previews its files (click to go, × to remove it); the file-tree button at the top opens the drawer; the folder-with-a-cog under the dots is the config workspace; the palette opens the theme tray; the gear above the clock opens Settings.

Config lives in `~/.config/tileide/` (created on first run): `tileide.lua`, `rices/` (layout and design) and `themes/` (colours only). Saved changes apply at once; `tileide --check-config` checks the file from a shell. Every key above can be rebound with `tileide.bind(mods, key, action, arg)`. Actions: `workspace` (1–10 or `"config"`), `sidebar`, `layout` (`toggle`/`dwindle`/`columns`), `split` (`vertical`/`horizontal`), `focus`/`move`/`resize` (`left`/`right`/`up`/`down`), `close`, `terminal`, `editor`, `search`, `wrap`, `save`, `quit`, `overview`, `themes`, `themes_section`, `folder`, `workspace_close`, `hover`, `definition`, `complete`, `extensions_section`, `extension` (a command id), `settings`, `keybinds`, `font_size` (`up`/`down`), `run`, `debug`, `debug_stop`, `breakpoint`, `step_over`, `step_into`, `step_out`, `toolchains`. Add or change what `Alt+R` runs with `tileide.runner(".py", "python3 -u {file}")`.

Extensions are programs in any language that run in their own process and start only when needed; the Extensions section shows what each one costs. See [docs/extensions.md](docs/extensions.md) and [docs/compatibility.md](docs/compatibility.md).

**Known limits:** tested on Hyprland only; on Hyprland, opening another app on TileIDE's workspace takes it out of full-screen.

# How to contribute
* You can create community mods and extensions: here you can basically call it a custom rice plugin and other packages such as code formatters, linters, and etc. Basically think of this as VSCODE + Zen
* For versions or releases, I will think about it once we're in the stable releases.

# License
[GPL-3.0-or-later](LICENSE)
