-- Black Glass: warm charcoal frame, smoked-glass tiles over the wallpaper (from the
-- tileIDE logo). Colours only. The glass blur needs a full-screen background
-- (tileide.background { mode = "image" | "gradient" | "solid" }); see-through and
-- windowed get the fill at min_tint instead, plus whatever blur the compositor adds.
return {
  name      = "Black Glass",
  mode      = "dark",
  surface   = "#1F1C1A",
  tile      = "#302B27",
  text      = "#F4EFEA",
  muted     = "#B0A79E",
  accent    = "#FFF1E0",
  on_accent = "#1F1C1A",
  error     = "#FF8A70",
  strip_mid = "#5C3A1E",
  glass = {
    opacity        = 0.56,       -- tile colour, top left
    bottom         = "#120C08",  -- bottom right
    bottom_opacity = 0.66,
    edge_opacity   = 0.24,       -- unfocused border (text colour)
    focus_opacity  = 0.60,       -- focused border (accent colour)
    blur           = 30,         -- px
    saturate       = 1.5,
    min_tint       = 0.80,       -- fill floor when TileIDE can't see the background
  },
}
