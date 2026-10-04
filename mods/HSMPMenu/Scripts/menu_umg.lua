-- HSMPMenu's UMG widget helpers (no state): construction, cloning the native look,
-- visibility, child lookup and slot geometry. Loaded by main.lua (load_module
-- "menu_umg"); every call is defensive (pcall), as the rest of the menu.
local U = {}

function U.construct(class_path, outer, name)
    if outer == nil then return nil end   -- never construct into a nil / freed outer
    local cls = StaticFindObject(class_path)
    if not cls or not cls:IsValid() then return nil end
    return StaticConstructObject(cls, outer, FName(name, FNAME_Add), 0, 0, nil, false)
end

function U.clone_button_look(dst, src) pcall(function() dst.WidgetStyle = src.WidgetStyle end) end
function U.clone_text_look(dst, src)
    pcall(function() dst.Font = src.Font end)
    pcall(function() dst.ColorAndOpacity = src.ColorAndOpacity end)
    pcall(function() dst.ShadowColorAndOpacity = src.ShadowColorAndOpacity end)
    pcall(function() dst.ShadowOffset = src.ShadowOffset end)
end

function U.set_vis(w, v)
    if w and w:IsValid() then pcall(function() w:SetVisibility(v) end) end
end

function U.find_child(root, name)
    local n; pcall(function() n = root:GetChildrenCount() end); if not n then return nil end
    for i = 0, n - 1 do
        local c; pcall(function() c = root:GetChildAt(i) end)
        if c and c:IsValid() and c:GetFName():ToString() == name then return c end
    end
    return nil
end

function U.find_text_child(button)
    local n; pcall(function() n = button:GetChildrenCount() end); if not n then return nil end
    for i = 0, n - 1 do
        local c; pcall(function() c = button:GetChildAt(i) end)
        if c and c:IsValid() then
            local cls = c:GetClass():GetFName():ToString()
            if cls == "TextBlock" or cls:find("Text") then return c end
        end
    end
    return nil
end

function U.slot_rect(btn)
    if not btn then return nil end
    local slot = btn.Slot; if not slot or not slot:IsValid() then return nil end
    local x, y, w, h
    pcall(function() local p = slot:GetPosition(); x = p.X; y = p.Y end)
    pcall(function() local s = slot:GetSize();     w = s.X; h = s.Y end)
    return x, y, w, h
end

function U.copy_slot_anchoring(src_btn, dst_slot)
    local src_slot = src_btn.Slot; if not src_slot or not src_slot:IsValid() then return end
    pcall(function() local a  = src_slot:GetAnchors();  if a  then dst_slot:SetAnchors(a) end end)
    pcall(function() local al = src_slot:GetAlignment(); if al then dst_slot:SetAlignment(al) end end)
    pcall(function() dst_slot.ZOrder = src_slot.ZOrder end)
end

return U
