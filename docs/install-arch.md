# Installing TileIDE on Arch Linux

TileIDE is built from your clone with `makepkg`. The repository is private, so there is no AUR package yet.

## 1. Dependencies
```sh
sudo pacman -S --needed base-devel git rustup gtk4 gtksourceview5 vte4
rustup default stable
```
Optional: `otf-space-grotesk` (AUR), the UI font. Without it the default sans is used.

## 2. Build and install
```sh
git clone https://github.com/fordgarcia/tileIDE.git
cd tileIDE/packaging/arch
makepkg -si
```
`makepkg` builds in release mode, runs the whole test suite (`cargo test --workspace`), and installs:

| Path | What |
|---|---|
| `/usr/bin/tileide` | the program |
| `/usr/share/applications/dev.tileide.TileIDE.desktop` | launcher entry |
| `/usr/share/icons/hicolor/scalable/apps/dev.tileide.TileIDE.svg` | icon |
| `/usr/share/tileide/config/` | sample config, copied to `~/.config/tileide/` on first run |

## 3. Update
```sh
cd tileIDE && git pull
cd packaging/arch && makepkg -si
```

## 4. Remove
```sh
sudo pacman -R tileide
```
Your `~/.config/tileide/` is left alone; delete it yourself if you want it gone.

## Hyprland notes
- Full-screen is the default. With `misc:on_focus_under_fullscreen = 1` (Hyprland's default), opening another app on TileIDE's workspace takes TileIDE out of full-screen. Set it to `0` to open new windows behind instead, or use `tileide --windowed`.
- Translucency needs no setup. Blur behind TileIDE needs `decoration:blur:enabled = true`.
