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
  mock.data.instruments[3].plugin.preset_data = xml(A):gsub("</DeviceSlot>", "<Hidden>edit</Hidden></DeviceSlot>")
  I.failure(I.apply(peer, "plugin-preset-data", asked, "preset", preview.fence, epoch, layout), "changed")
  mock.data.instruments[3].plugin.preset_data = xml(A)
  local record = I.change(peer, "plugin-preset-data", input(peer))
  mock.data.instruments[3].plugin.preset_data = xml(B):gsub("</DeviceSlot>", "<Hidden>edit</Hidden></DeviceSlot>")
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

test("vst3 presets: a clean instrument restore accepts normalized bytes while earlier edits remain fenced", function()
  local peer = setup()
  local record = I.change(peer, "plugin-preset-data", input(peer))
  mock.data.instruments[3].plugin.normalize_preset_data = function(value) return (value:gsub("</DeviceSlot>", "<Version>normalized</Version></DeviceSlot>")) end
  local restored = I.restore(peer, "plugin-preset-data", record)
  ok(restored.result)
  ok(restored.result.display.note:find("normalized", 1, true))
  eq(mock.data.instruments[3].plugin.preset_data, (xml(A):gsub("</DeviceSlot>", "<Version>normalized</Version></DeviceSlot>")))
end)

local function effect_setup()
  local H = T.harness
  local env = H.setup({ song = { name = "VST3", mixer_device = true, tracks = {
    { name = "Mastering", devices = { { name = "Synthetic effect", path = "Audio/Effects/VST3/" .. ID,
      params = {}, presets = {}, preset_data = xml(A) } } },
  }, send_tracks = 0, patterns = 1, pattern_lines = 16, instruments = 1 } })
  local peer = H.connect(env)
  H.frames(peer)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return peer, mock.data.tracks[1].devices[2]
end
local function effect_input(peer)
  local epoch = I.place(peer)
  return { ref = epoch .. ":device:1.2", vst3Data = B, presetName = "Created" }
end

test("vst3 presets: effects load and restore full state and fence later hidden edits", function()
  local peer, device = effect_setup()
  local record = I.change(peer, "device-preset-data", effect_input(peer))
  eq(device.preset_data, xml(B))
  device.preset_data = xml(B):gsub("</DeviceSlot>", "<Hidden>edited</Hidden></DeviceSlot>")
  I.failure(I.restore(peer, "device-preset-data", record), "changed")
  device.preset_data = xml(B)
  ok(I.restore(peer, "device-preset-data", record).result)
  eq(device.preset_data, xml(A))
  local steps = I.steps()
  ok(I.restore(peer, "device-preset-data", record).result)
  eq(I.steps(), steps)
end)

test("vst3 presets: an effect partial load failure rolls back and clean normalized undo succeeds", function()
  local peer, device = effect_setup()
  local asked = effect_input(peer)
  local preview, epoch, layout = I.preview(peer, "device-preset-data", asked)
  device.reject_preset_data = xml(B)
  I.failure(I.apply(peer, "device-preset-data", asked, "preset", preview.fence, epoch, layout), "failed")
  eq(device.preset_data, xml(A))
  device.reject_preset_data = nil
  local record = I.change(peer, "device-preset-data", effect_input(peer))
  device.normalize_preset_data = function(value) return (value:gsub("</DeviceSlot>", "<Version>normalized</Version></DeviceSlot>")) end
  local restored = I.restore(peer, "device-preset-data", record)
  ok(restored.result)
  ok(restored.result.display.note:find("normalized", 1, true))
  eq(device.preset_data, (xml(A):gsub("</DeviceSlot>", "<Version>normalized</Version></DeviceSlot>")))
end)


test("vst3 presets: instrument preview and undo tolerate editor window moves", function()
  local peer = setup()
  local asked = input(peer)
  local preview, epoch, layout = I.preview(peer, "plugin-preset-data", asked)
  mock.data.instruments[3].plugin.preset_data = xml(A):gsub("10,20", "30,40")
  local applied = I.apply(peer, "plugin-preset-data", asked, "preset", preview.fence, epoch, layout)
  ok(applied.result)
  -- Use the journal path for a fresh record, then move the view again after its apply.
  mock.data.instruments[3].plugin.preset_data = xml(A)
  local record = I.change(peer, "plugin-preset-data", input(peer))
  mock.data.instruments[3].plugin.preset_data = xml(B):gsub("10,20", "50,60")
  mock.data.instruments[3].plugin.normalize_preset_data = function(value) return (value:gsub("10,20", "70,80")) end
  local restored = I.restore(peer, "plugin-preset-data", record)
  ok(restored.result)
  eq(restored.result.display.note, nil, "view changes are not state normalization")
end)

test("vst3 presets: undo refuses a silent no-op instrument restore", function()
  local peer = setup()
  local record = I.change(peer, "plugin-preset-data", input(peer))
  mock.data.instruments[3].plugin.ignore_preset_data = xml(A)
  I.failure(I.restore(peer, "plugin-preset-data", record), "failed")
  eq(mock.data.instruments[3].plugin.preset_data, xml(B))
end)

test("vst3 presets: undo refuses a silent no-op effect restore", function()
  local peer, device = effect_setup()
  local record = I.change(peer, "device-preset-data", effect_input(peer))
  device.ignore_preset_data = xml(A)
  I.failure(I.restore(peer, "device-preset-data", record), "failed")
  eq(device.preset_data, xml(B))
end)

test("vst3 presets: a native effect undo does not claim VST3 normalization", function()
  local peer, device = effect_setup()
  local asked = effect_input(peer)
  asked.vst3Data, asked.presetData = nil, xml(B)
  device.path = "Audio/Effects/Native/Test"
  local record = I.change(peer, "device-preset-data", asked)
  device.normalize_preset_data = function(value) return (value:gsub("</DeviceSlot>", "<Version>normalized</Version></DeviceSlot>")) end
  local restored = I.restore(peer, "device-preset-data", record)
  ok(restored.result)
  eq(restored.result.display.note, nil)
end)
