-- ~/.config/tileide/tileide.lua  (sample from project_context.md §6.3)
tileide.keys { mod = "ALT" }               -- TileIDE's own modifier; SUPER stays with your WM (§4.5)
local mod = "ALT"

tileide.appearance {
  theme         = "dusk",                 -- themes/dusk.lua
  opacity       = 0.82,
  blur_hint     = true,                   -- asks compositor; ignored if unsupported
  window_radius = 22,
  gap_rail      = 24,                     -- 2a: rail <-> window
  gap_outer     = 30,                     -- 2b: window <-> screen edge (top, right, bottom)
}

tileide.sidebar {
  start_state   = "collapsed",
  drawer_fillet = 22,                     -- concave join between rail and drawer
  show_launcher   = true,
  show_workspaces = true,
  show_label      = true,
  show_clock      = true,                 -- read-only data
  show_system     = true,                 -- read-only data: network, bluetooth, battery
  show_files      = true,                 -- drawer sections
  show_extensions = true,
  show_themes     = true,
  show_config     = true,
}

tileide.background {
  mode         = "see-through",           -- see-through | desktop | image | gradient | solid
  source       = "~/Pictures/Backgrounds/dusk.jpg",
  dim          = 30,                      -- % darkening, protects legibility
  blur         = 0,                       -- px, app-drawn modes only
  tile_opacity = 0.92,
}

tileide.tiling { layout = "dwindle", gaps_in = 8, gaps_out = 12 }

-- keybinds: tileide.bind(mods, key, action, arg)
for i = 1, 9 do
  tileide.bind(mod,            tostring(i), "workspace", i)
  tileide.bind(mod .. " SHIFT", tostring(i), "moveto",    i)
end
tileide.bind(mod, "B",  "sidebar",   "toggle")
tileide.bind(mod, "V",  "split",     "vertical")
tileide.bind(mod, "H",  "focus",     "left")
tileide.bind(mod, "C",  "workspace", "config")
tileide.bind(mod, "P",  "palette")
tileide.bind("",  "F5", "run")

-- workspaces
tileide.workspace(1, { name = "code",  layout = "dwindle" })
tileide.workspace(2, { name = "notes", folder = "~/notes",
                       extensions = "markdown-only",
                       background = "~/Pictures/Backgrounds/paper.jpg" })

-- extension policy, language servers, toolchains
tileide.extension("tileide.markdown-preview", { mode = "on-demand" })
tileide.lsp("rust", { cmd = "rust-analyzer" })
tileide.toolchain("python", { path = "/usr/bin/python3" })   -- discovered automatically; this overrides

-- a rice is just another Lua file; require() is limited to the config dir
require("rices.nord")
