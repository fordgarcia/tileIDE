-- ~/.config/tileide/tileide.lua
-- Saved changes apply at once. A mistake shows an error banner and keeps the last
-- working config. Check it from a shell with: tileide --check-config
tileide.keys { mod = "ALT" }               -- TileIDE's own modifier; SUPER stays with your WM
local mod = "ALT"

-- Settings (Alt+, or the gear above the clock) changes most of what follows, live, and
-- keeps it in state.lua. A key set here wins and shows locked in Settings, so these are
-- left commented: uncomment one to pin it.
tileide.appearance {
  -- theme = "miasma",                    -- themes/<name>.lua. Unset, the theme tray (Alt+T) decides;
                                          -- set here, it always wins over the tray.
  -- window_radius = 14,                  -- tile corners; the frame's corners follow (14 + gaps_out)
  -- animations    = true,                -- tiles glide, workspaces and overlays fade
  -- motion        = "fluid",             -- fluid (soft springs, iOS-like) | snappy
  -- motion_speed  = 1.0,                 -- 0.5 .. 2 (higher = faster)
  -- ui_font       = "Space Grotesk",     -- the drawer, cards and rail
  -- ui_scale      = 1.0,                 -- 0.8 .. 1.5, TileIDE's own text
  -- code_font     = "monospace",         -- editors and terminals
  -- code_size     = 11,                  -- points; Alt+= / Alt+- change it
}

tileide.sidebar {
  -- start_state     = "collapsed",       -- collapsed | expanded (the Files drawer)
  -- show_launcher   = true,
  -- show_workspaces = true,
  -- show_label      = true,
  -- show_clock      = true,
  show_files      = true,
  -- show_visualizer = true,              -- the audio waveform while something plays
  -- scale           = 1.0,               -- 0.8 .. 1.5, the rail and everything on it
  -- icon_scale      = 1.0,               -- 0.8 .. 1.25, the rail's icons
  -- icon_backgrounds = true,             -- the circles and squares behind them
  -- show_themes     = true,              -- the palette button
  -- show_config     = true,              -- the Config folder under the workspace dots
}

-- Full-screen only; a windowed TileIDE shows your desktop. Settings > Themes &
-- background picks one too; calling tileide.background here wins over that pick.
-- tileide.background {
--   mode         = "see-through",        -- see-through | image | gradient | solid
--   source       = "~/Pictures/wall.jpg",-- image: a path; solid: "1A1B26"; gradient: "1A1B26,3B4261"
--   dim          = 0,                    -- % darkening
--   blur         = 0,                    -- px
--   tile_opacity = 0.92,                 -- 0.88 .. 1
--   fit          = "cover",              -- cover | contain | tile
-- }

tileide.tiling {
  layout       = "dwindle",               -- dwindle | columns (scrolling, niri-style); Alt+V toggles
  -- gaps_in   = 10,
  -- gaps_out  = 16,
  column_width = 0.5,                     -- new columns, as a share of the width
}

-- Alt+R runs the focused file. Add or replace a runner by extension ({file}, {dir},
-- {stem}, {out} = a binary path); the command runs through sh -c.
-- tileide.runner(".py", "python3 -u {file}")

-- keybinds: tileide.bind(mods, key, action, arg). These replace the built-in key with
-- the same mods; everything else keeps its default (see the README).
tileide.bind(mod, "V", "layout", "toggle")
tileide.bind(mod, "S", "split",  "vertical")   -- the next tile opens below (stacks in columns)

-- workspaces 1..10 (Alt+0 is 10)
tileide.workspace(1, { name = "code" })
-- tileide.workspace(2, { name = "notes", folder = "~/notes", layout = "columns", theme = "dune-teal" })

-- language servers (rust, c, python, typescript, java) start by themselves when one is
-- installed and the workspace has a folder; change the command or turn one off here
-- tileide.lsp("python", { cmd = "pylsp" })
-- tileide.lsp("java", { enabled = false })

-- a rice is just another Lua file; require() is limited to ~/.config/tileide
-- require("rices.nord")
