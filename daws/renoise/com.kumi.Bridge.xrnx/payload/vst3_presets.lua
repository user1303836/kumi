-- Renoise 3.5.4 stores a complete standard VST3 preset, base64, in ParameterChunk for both effect and instrument devices.
-- Keep the device's own XML wrapper, including its display and routing metadata. The host checks the entire container
-- before sending vst3Data; the tool independently checks its header/class before handing it to the installed plug-in.
local import = ...
local base = import("changes_base")
local M = {}
local MAX = 8 * 1024 * 1024
local alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
local digits = {}
for n = 1, #alphabet do digits[alphabet:sub(n, n)] = n - 1 end

local function class_id(data)
  if type(data) ~= "string" or #data < 64 or #data > MAX or #data % 4 ~= 0 or data:find("[^A-Za-z0-9+/=]") then
    base.bad("vst3Data must be a bounded base64 VST3 preset checked by Kumi's host")
  end
  local head = {}
  for n = 1, 64, 4 do
    local a, b, c, d = digits[data:sub(n,n)], digits[data:sub(n+1,n+1)], digits[data:sub(n+2,n+2)], digits[data:sub(n+3,n+3)]
    if not a or not b or not c or not d then base.bad("invalid VST3 preset header") end
    local value = a * 262144 + b * 4096 + c * 64 + d
    head[#head+1] = string.char(math.floor(value / 65536), math.floor(value / 256) % 256, value % 256)
  end
  head = table.concat(head)
  local id = head:sub(9, 40)
  if head:sub(1,8) ~= "VST3\1\0\0\0" or #id ~= 32 or id:find("[^0-9a-fA-F]") then base.bad("invalid VST3 preset header or class ID") end
  return id:upper()
end

function M.compose(device, current, input)
  local id = class_id(input.vst3Data)
  if input.presetData ~= nil then base.bad("give one preset source") end
  if type(current) ~= "string" or #current > MAX or not current:find('<DeviceSlot%s+type="AudioPluginDevice"')
      or current:match('<FilterDevicePreset%s+doc_version="(%d+)"') ~= "14"
      or current:match("<PluginType>([^<]+)</PluginType>") ~= "VST3" then
    base.bad("VST3 preset loading requires a Renoise 3.5 VST3 device (preset document version 14)")
  end
  local own = current:match("<PluginIdentifier>([^<]*)</PluginIdentifier>")
  if own ~= id or not device.device_path:match("/VST3/" .. id .. "$") then
    base.bad("that VST3 preset is for another plug-in (class " .. id .. ")")
  end
  if current:match("<ParameterChunkType>([^<]*)</ParameterChunkType>") ~= "Chunk" then base.bad("the VST3 device does not expose a state chunk") end
  local wanted, count = current:gsub("<ParameterChunk>.-</ParameterChunk>", function()
    return "<ParameterChunk><![CDATA[" .. input.vst3Data .. "]]></ParameterChunk>"
  end)
  if count ~= 1 or #wanted > MAX then base.bad("the VST3 device's preset data is invalid or exceeds the undo state limit") end
  return wanted
end

return M
