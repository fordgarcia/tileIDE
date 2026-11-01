-- ~/.config/tileide/tileide.lua
-- Saved changes apply at once. A mistake shows an error banner and keeps the last
-- working config. Check it from a shell with: tileide --check-config
tileide.keys { mod = "ALT" }               -- TileIDE's own modifier; SUPER stays with your WM
local mod = "ALT"

tileide.appearance {
  -- theme = "miasma",                    -- themes/<name>.lua. Unset, the theme tray (Alt+T) decides;
                                          -- set here, it always wins over the tray.
  window_radius = 14,                     -- tile corners; the frame's corners follow (14 + gaps_out)
  animations    = true,                   -- tiles glide, workspaces and overlays fade in
}

tileide.sidebar {
  start_state     = "collapsed",          -- collapsed | expanded (the Files drawer)
  show_launcher   = true,
  show_workspaces = true,
  show_label      = true,
  show_clock      = true,
  show_files      = true,
  show_themes     = true,                 -- the palette button
  show_config     = true,                 -- the gear
}

-- Full-screen only; a windowed TileIDE shows your desktop.
tileide.background {
  mode         = "see-through",           -- see-through | image | gradient | solid
  -- source    = "~/Pictures/wall.jpg",   -- image: a path; solid: "1A1B26"; gradient: "1A1B26,3B4261"
  dim          = 0,                       -- % darkening
  blur         = 0,                       -- px
  tile_opacity = 0.92,                    -- 0.88 .. 1
  fit          = "cover",                 -- cover | contain | tile
}

tileide.tiling {
  layout       = "dwindle",               -- dwindle | columns (scrolling, niri-style); Alt+V toggles
  gaps_in      = 10,
  gaps_out     = 16,
  column_width = 0.5,                     -- new columns, as a share of the width
}

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
