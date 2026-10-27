-- Synthetic standard VST3 containers; no installed plug-ins or files from this machine.
local T = ...
local test, eq, ok = T.test, T.eq, T.ok
local I, mock = T.instruments, T.mock
local ID = "00112233445566778899AABBCCDDEEFF"
local A = "VlNUMwEAAAAwMDExMjIzMzQ0NTU2Njc3ODg5OUFBQkJDQ0RERUVGRjMAAAAAAAAAYWJjTGlzdAEAAABDb21wMAAAAAAAAAADAAAAAAAAAA=="
local B = "VlNUMwEAAAAwMDExMjIzMzQ0NTU2Njc3ODg5OUFBQkJDQ0RERUVGRjMAAAAAAAAAZGVmTGlzdAEAAABDb21wMAAAAAAAAAADAAAAAAAAAA=="
local PATH = "Audio/Generators/VST3/" .. ID
local function xml(chunk)
  return '<FilterDevicePreset doc_version="14"><DeviceSlot type="AudioPluginDevice"><IsMaximized>false</IsMaximized>'
    .. '<PluginType>VST3</PluginType><PluginIdentifier>' .. ID .. '</PluginIdentifier><PluginEditorWindowPosition>10,20</PluginEditorWindowPosition>'
    .. '<ParameterChunkType>Chunk</ParameterChunkType><ParameterChunk><![CDATA[' .. chunk .. ']]></ParameterChunk></DeviceSlot></FilterDevicePreset>'
end
local function setup()
  mock.plugin_catalog = { [PATH] = { name = "Synthetic synth", params = {}, presets = {}, preset_data = xml(A) } }
  local _, peer = I.setup()
  I.change(peer, "plugin-load", { ref = I.iref(I.place(peer), 3), path = PATH })
  return peer
end
local function input(peer) return { ref = I.iref(I.place(peer), 3), vst3Data = B, presetName = "Created" } end

test("vst3 presets: compose preserves the host wrapper and rejects mismatched identity before writing", function()
  local codec = T.load_payload_module("vst3_presets")
  eq(codec.compose({ device_path = PATH }, xml(A), { vst3Data = B }), xml(B))
  for _, source in ipairs({ (xml(A):gsub(ID, string.rep("F", 32))), (xml(A):gsub(">VST3<", ">VST<")), (xml(A):gsub('doc_version="14"', 'doc_version="15"')) }) do
    local success = pcall(function() codec.compose({ device_path = PATH }, source, { vst3Data = B }) end)
    ok(not success, "unverified wrapper or identity must be refused")
  end
  ok(not pcall(function() codec.compose({ device_path = PATH }, xml(A), { vst3Data = "not base64" }) end))
end)

test("vst3 presets: instrument load and undo preserve arbitrary state beyond parameters", function()
  local peer = setup()
  local record = I.change(peer, "plugin-preset-data", input(peer))
  eq(mock.data.instruments[3].plugin.preset_data, xml(B))
  ok(I.restore(peer, "plugin-preset-data", record).result)
  eq(mock.data.instruments[3].plugin.preset_data, xml(A))
  local steps = I.steps()
  ok(I.restore(peer, "plugin-preset-data", record).result)
  eq(I.steps(), steps, "already restored: no additional write")
end)

test("vst3 presets: preview and undo refuse a later state and a replacement plug-in", function()
  local peer = setup()
  local asked = input(peer)
  local preview, epoch, layout = I.preview(peer, "plugin-preset-data", asked)
  mock.data.instruments[3].plugin.preset_data = xml(A):gsub("10,20", "30,40")
  I.failure(I.apply(peer, "plugin-preset-data", asked, "preset", preview.fence, epoch, layout), "changed")
  mock.data.instruments[3].plugin.preset_data = xml(A)
  local record = I.change(peer, "plugin-preset-data", input(peer))
  mock.data.instruments[3].plugin.preset_data = xml(B):gsub("10,20", "30,40")
  I.failure(I.restore(peer, "plugin-preset-data", record), "changed")
  mock.data.instruments[3].plugin.path = "Audio/Generators/VST3/" .. string.rep("F", 32)
  I.failure(I.restore(peer, "plugin-preset-data", record, nil, { discard = true }), "changed")
end)

test("vst3 presets: a plug-in rejection restores the earlier complete state", function()
  local peer = setup()
  local asked = input(peer)
  local preview, epoch, layout = I.preview(peer, "plugin-preset-data", asked)
  mock.data.instruments[3].plugin.reject_preset_data = xml(B)
  I.failure(I.apply(peer, "plugin-preset-data", asked, "preset", preview.fence, epoch, layout), "failed")
  eq(mock.data.instruments[3].plugin.preset_data, xml(A), "a partially accepted state must be restored")
end)
