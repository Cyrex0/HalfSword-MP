-- The resolutions and DPI scales every HSMP UI layout is tested at (ui-scale).
-- Used by menu_ui.lua (every menu screen) and ui_scale.lua (HUD, panels, the
-- scaling rule itself).
--
--   RES   physical window sizes: 16:9 from 1280x720 to 4K, ultrawide 21:9 and
--         32:9 (5120x1440, the user's monitor), 16:10, 4:3, 5:4 and small
--         windows down to the floor window (1024x576) and below it (800x600)
--   DPI   the viewport's DPI scale: 1.0 .. 2.0 (Windows 100-200 % when UE applies
--         it) and false = the default ShortSide curve (short side / 1080)

local R = {}

R.RES = {
    { 1280, 720,  "720p" },
    { 1920, 1080, "1080p" },
    { 2560, 1440, "1440p" },
    { 3840, 2160, "4K" },
    { 3440, 1440, "ultrawide 21:9" },
    { 5120, 1440, "super-ultrawide 32:9" },
    { 2560, 1080, "ultrawide 1080" },
    { 1680, 1050, "16:10" },
    { 1280, 800,  "16:10 small" },
    { 1366, 768,  "laptop" },
    { 1600, 900,  "window 900p" },
    { 1024, 576,  "floor window" },
    { 1024, 768,  "4:3 window" },
    { 1280, 1024, "5:4" },
    { 800,  600,  "tiny window" },
}

R.DPI = { 1.0, 1.25, 1.5, 2.0, false }

-- dpi false -> UE's default curve (short side / 1080)
function R.eff_dpi(vw, vh, dpi)
    if dpi then return dpi end
    return math.min(vw, vh) / 1080
end

function R.tag(vw, vh, dpi)
    return string.format("%dx%d dpi %s", vw, vh, dpi and tostring(dpi) or "auto")
end

return R
