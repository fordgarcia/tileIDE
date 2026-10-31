# Spike 1 results: TileIDE canvas with GTK4 (R1)

v0.0 items 1–2 (`../../project_contexts/versions/v0.0.md`). Measured on 2026-10-31: Hyprland 0.56.2, eDP-1 1920×1080 at scale 1.5 (1280×720 logical), GTK 4.24.1, gtk4-rs 0.11.5, gtk4-layer-shell 0.8.1.

## What the spike draws
The layout skeleton from `annotated_layout.png`, using the `design.md` §4.1 tokens:
- Rail (1) flush with the left edge, holding launcher, workspace pill and dots, a vertical label and the clock.
- A transparent gap ring (2).
- A translucent window with a 1px accent outline (3).
- Three tiles (4a) with sample code. The focused tile has a thicker outline, so focus doesn't depend on colour alone.

`--solid` gives the opaque fallback.

```sh
cargo run --release -- windowed | fullscreen | layer [--solid] [--seconds N]   # Esc or q quits
```

## Display modes (spec §4.5)
| Mode | Result |
|---|---|
| **windowed** (normal toplevel) | ✅ Hyprland tiles it like any other app (612×668 next to VS Code, foot and Brave). Other windows don't affect its inner tiles. |
| **fullscreen** (fullscreen toplevel) | ✅ **Best full-screen option.** Covers the bar, Hyprland hides the workspace's other windows, and the wallpaper shows through the gap ring as designed. |
| **layer** (layer-shell, Top layer, exclusive keyboard) | ❌ Waybar (also on the Top layer) is still drawn over it, and the **other windows show through** every transparent area instead of the wallpaper. Reject for full-screen mode. |

**Decision proposed:** full-screen mode is a fullscreen toplevel, not layer-shell. That means `gtk4-layer-shell` is no longer needed.
**[VERIFY]** That a window opened while TileIDE is full-screen stays behind it. Hyprland's default `misc:new_window_takes_over_fullscreen = 0` suggests it does.

## Blur
**Not verified:** `decoration:blur:enabled` is `false` in your Hyprland config, so the screenshots show the unblurred wallpaper. Transparency itself works. To check blur, enable it in your Hyprland config and rerun `fullscreen`.

## Startup and memory (first frame, windowed)
| GTK renderer | First frame | RSS at first frame |
|---|---|---|
| Vulkan (GTK default here) | **~900 ms** | 52 MB |
| GL (`GSK_RENDERER=gl`) | 66–72 ms | 110 MB |
| Cairo (`GSK_RENDERER=cairo`) | 37 ms | 41 MB |

The default Vulkan renderer costs about 0.9 s on every start. The real app must choose its renderer (GL is the likely default; measure animation smoothness in spike 3 before picking cairo). Binary: 464 KB stripped, linking the system GTK.

## Open design questions found
- **Windowed mode doubles the gaps.** Hyprland draws its own gap and border around the window, and TileIDE then draws its gap ring and outline inside. Should windowed mode drop TileIDE's gap ring and outline and leave them to the WM?
- **The rail in windowed mode** sits inside the WM window. Is that wanted, or should windowed mode be just the code region?

## Not covered (later spikes)
Drawer and fillet animation (spike 3), real tiling, editor and terminal tiles (spike 4), R2 comparison (spike 5), background layer (spike 6).

---

# Spike 3 results: rail ⇄ drawer (R1, same crate)

v0.0 item 3. Code: `src/draw.rs` (`rail_drawer_path`), and the animation in `src/main.rs`.

```sh
cargo run --release -- fullscreen [--push]           # b or click the launcher; Esc collapses
cargo run --release -- fullscreen --bench-drawer 8   # frame statistics
cargo run --release -- render out.png 0.5 --push     # one offscreen frame on a checkerboard
```

## Shape
Rail and drawer are **one closed outline**. The drawer joins the rail through concave fillets (`drawer.fillet` 20) and has convex outer corners (`window.radius` 20). Both radii scale with the open width, so a half-open drawer never has corners wider than itself, and the rail's own corner shrinks from 20 to 10 so it fits above the fillet. Offscreen renders at 0 / 0.15 / 0.5 / 1 show a seamless join with no kinks (checked visually). Cairo paths are enough; no custom shader is needed.

## Motion
200 ms ease-out (cubic), driven by the GTK frame clock. Reversing mid-animation continues from the current position and takes proportionally less time (unit-tested). Drawer content fades in over the last 60% of the expansion.

## Frame timing (full-screen 1920×1080 at scale 1.5, 60 Hz panel, 8 toggles each)
| Renderer | Variant | Frames | Interval median / max | Dropped (>25 ms) | RSS |
|---|---|---|---|---|---|
| **cairo** | overlay | 105 | 16.62 / 16.80 ms | **0** | 41 MB |
| **cairo** | push | 106 | 16.65 / 16.72 ms | **0** | 41 MB |
| gl | overlay | 106 | 16.63 / 17.26 ms | 0 | 110 MB |
| gl | push | 104 | 16.53 / 32.37 ms | 1 | 110 MB |
| vulkan | overlay | 95 | 16.68 / 35.64 ms | 3 | 52 MB |
| vulkan | push | 102 | 16.66 / 29.35 ms | 1 | 53 MB |

The draw callback (building the cairo paths) costs a median of 0.6–0.7 ms per frame. **For this UI, the cairo renderer is fastest to start (37 ms), smoothest, and lightest.** It still has to be re-checked once real editor text and a terminal are on screen (spike 4), and on a high-refresh monitor.

## Push vs overlay (Q3), from the screenshots
- **Overlay:** the drawer (68% opaque) lies over the code, so tile text shows through and the drawer is hard to read. Overlay needs a nearly opaque drawer (the L4 92–98% level) or app-side blur.
- **Push:** stays clean. The window shrinks, `gap.rail` is kept, and the tiles reflow every frame without dropped frames. **Recommended.**

## Found
- **Text must use Pango.** Cairo's basic text API has no font fallback (▾ ▸ rendered as boxes) and no shaping. The spike now uses ASCII; the real app needs Pango (or the toolkit's text stack) for every label.

---

# Frame update: Caelestia border (2026-10-31)

Reference: `project_contexts/design/caelestia-reference.png`. The rail and a thin, near-solid border on the top, right and bottom edges are **one shape**: the canvas minus a rounded hole (`frame_path` in `src/draw.rs`, even-odd fill). The drawer is the rail **widening**, and the hole's rounded corners slide with it, so the join is seamless at every width without separate fillet geometry. New tokens: `frame.border` 10, `frame.radius` 25, `frame.opacity` 0.95; `gap.outer` is now measured from the frame's inner edge (20). Recorded in `design.md` §4.1.

---

# Spike 4 results: real tiles (R1)

v0.0 item 4. `src/tiles.rs` (pure tile tree, 6 unit tests) + `src/tile_view.rs` (GTK widgets).

```sh
GSK_RENDERER=cairo cargo run --release -- fullscreen --push --real-tiles
# Alt+Return terminal · Alt+E editor · Alt+W close · Alt+H/J/K/L focus · Alt+Shift+H/J/K/L resize · Alt+B drawer · Alt+Esc quit
```

## What works (verified on screen, keys driven by `wtype` after checking that the spike had focus)
- **Tile tree:** binary splits with ratios; dwindle picks the split axis from the focused tile's shape; close hands the space and focus to the sibling; directional focus; keyboard resize moves the nearest divider on that side; ratios clamped to 10–90%.
- **Layout:** tiles are placed with exact gaps by our own code (a `gtk::Fixed` over the canvas), so GTK's layout engine is not in the way (T10). Pushing the drawer reflows the tiles every frame.
- **Editor tile:** GtkSourceView 5 with Rust and Lua highlighting, line numbers, current line, and typing.
- **Terminal tile:** VTE (gtk4) running `$SHELL`. A typed `echo` ran.
- **Keybinds:** a capture-phase controller lets TileIDE's `Alt` bindings win over the focused editor or terminal. None of them collide with this machine's 225 Hyprland binds.
- **Translucency:** the editor and terminal backgrounds are transparent over the 92% tile surface.

## Numbers (full-screen, 2 editors + 1 terminal, 8 drawer toggles)
| Renderer | First frame | RSS | Push: dropped | Overlay: dropped |
|---|---|---|---|---|
| **cairo** | 71 ms | 54 MB | **0** (max 16.8 ms) | 0 (max 17.4 ms) |
| gl | 108–115 ms | 124 MB | 3 (max 53.9 ms) | 0 |

**Cairo stays the best renderer with real widgets.** Not measured yet: typing latency, very large files, input methods (IME), and a high-refresh monitor.

## Found
1. **Startup focus:** keyboard focus grabbed before the window is shown is lost; GTK focuses the first editor instead. Fixed by re-grabbing on an idle callback after `show`.
2. **Terminal background:** the GTK theme's CSS paints the `vte-terminal` node opaque grey (measured 30,30,30). Fixed with `.tile vte-terminal { background-color: transparent }`; the terminal now matches the editor (26,30,37).
3. **Overlay drawer vs widgets:** real tile widgets sit *above* the canvas drawing, so an overlay drawer would be hidden under them. Overlay would need the drawer as its own widget layered above the tiles. Push needs nothing extra, which is another reason to choose push (Q3).
4. Not handled in the spike: closing a terminal tile when its shell exits, tab strips (Q5), and file save.
