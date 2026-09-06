-- Merge these fields into an existing configuration; do not replace its settings.
local wezterm = require 'wezterm'
return {
  font_dirs = { wezterm.config_dir .. '/fonts' },
  font = wezterm.font_with_fallback { 'JetBrains Mono', 'QuotaDeck Icons' },
}
