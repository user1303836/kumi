-- Kumi's changes to devices (M4 n-devices): presets, preset data, names and looks; the Doofer, Splitter and Notepad edits
-- Kumi's host composes (written here, fenced by the shape the host edited from); preset_read. Against the mock Renoise.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local next_id = 9000

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

local function call(peer, method, params, ticks)
  next_id = next_id + 1
  local id = next_id
  H.send(peer, { type = "request", id = id, method = method, params = params or {} })
  peer.events = peer.events or {}
  for _ = 1, ticks or 400 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      local frame = assert(json.decode(line))
      if frame.type == "response" and frame.id == id then return frame end
      if frame.type == "event" then peer.events[#peer.events + 1] = frame end
    end
    mock.advance(10)
  end
end

-- An EQ 5 with Renoise's XML preset data (two gains saved as Gain0/Gain1, two presets with their own values), a Gainer, and
-- a Doofer whose chain is in its XML.
local function eq5(extra)
  local data = {
    name = "EQ 5", path = "Audio/Effects/Native/EQ 5", slot = "Eq5Device",
    params = {
      { name = "Gain 1 (100 Hz)", element = "Gain0", value = 0, min = -20, max = 20 },
      { name = "Gain 2 (1.00 kHz)", element = "Gain1", value = 0, min = -20, max = 20 },
    },
    presets = { "Init", "Bright" }, preset = 1, preset_values = { [1] = { 0, 0 }, [2] = { -2, 6 } }, maximized = true,
  }
  for k, v in pairs(extra or {}) do data[k] = v end
  return data
end

local function gainer()
  return {
    name = "Gainer", path = "Audio/Effects/Native/Gainer", slot = "GainerDevice",
    params = { { name = "Gain", element = "Volume", value = 1, min = 0, max = 4 }, { name = "Panning", element = "Panning", value = 0.5, min = 0, max = 1 } },
    presets = { "Init" }, preset = 1,
  }
end

local DOOFER_BODY = "    <Macro0>\n      <Value>50</Value>\n      <Name>Macro 1</Name>\n    </Macro0>\n    <NumActiveMacros>4</NumActiveMacros>\n"
  .. "    <DeviceChain>\n      <Devices>\n        <GainerDevice type=\"GainerDevice\">\n          <Volume>\n            <Value>1.0</Value>\n"
  .. "          </Volume>\n        </GainerDevice>\n      </Devices>\n    </DeviceChain>"

local function doofer()
  local params = {}
  for k = 1, 8 do params[k] = { name = "Macro " .. k, value = 50, min = 0, max = 100, element = "Macro" .. (k - 1) } end
  return { name = "Doofer", path = "Audio/Effects/Native/Doofer", slot = "DooferDevice", params = params, xml_body = DOOFER_BODY, presets = { "Init" }, preset = 1 }
end

local function setup(tracks, sends)
  local env = H.setup({ song = {
    name = "Lab", file_name = "/s/lab.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = tracks or {
      { name = "Kick" }, { name = "Bass", devices = { eq5(), gainer() } }, { name = "Pad", devices = { doofer() } },
    },
    send_tracks = sends or 1, patterns = 2, pattern_lines = 16, instruments = 1,
  } })
  local peer = H.connect(env)
  H.frames(peer)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

local function place(peer)
  local answer = call(peer, "focus")
  local focus = answer.result
  if not focus then error("focus: " .. json.encode(answer.error or {}), 2) end
  return focus.epoch, focus.layout
end

local function steps() return #mock.data.undo.stack end
local function newest() local top = mock.data.undo.stack[#mock.data.undo.stack] return top and top.desc end

local function preview(peer, kind, input)
  local epoch, layout = place(peer)
  local response = call(peer, "change_preview", { kind = kind, input = input, epoch = epoch, layout = layout })
  ok(response and response.result, kind .. " preview: " .. json.encode(response and response.error or {}))
  return response.result, epoch, layout
end

local function apply(peer, kind, input, previewed, desc, epoch, layout)
  return call(peer, "change_apply", { kind = kind, input = input, fence = previewed.fence, desc = desc, epoch = epoch, layout = layout })
end

-- Preview and apply; the record the host would keep (with the apply's epoch and layout).
local function change(peer, kind, input, desc)
  local previewed, epoch, layout = preview(peer, kind, input)
  local done = apply(peer, kind, input, previewed, desc or ("Kumi: " .. kind .. " (k0001)"), epoch, layout)
  ok(done.result, kind .. " apply: " .. json.encode(done.error or {}))
  local record = done.result
  record.prior = previewed.prior
  return record, previewed
end

local function restore(peer, kind, record, desc, more)
  local params = {
    kind = kind, target = record.target, prior = record.prior, applied = record.applied, created = record.created,
    material = record.material, desc = desc or ("Kumi: undo " .. kind .. " (k0002)"), epoch = record.epoch, layout = record.layout,
  }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, "change_restore", params)
end

local function failure(response, code, pattern)
  ok(response and response.error, "expected an error, got " .. json.encode(response and response.result or {}))
  eq(response.error.code, code, "error code (" .. tostring(response.error.message) .. ")")
  if pattern then ok(tostring(response.error.message):find(pattern), response.error.message) end
end

local function device_data(t, d) return mock.data.tracks[t].devices[d] end
local function value(t, d, k) return device_data(t, d).params[k].value end

-- device-preset

test("devices m4: a preset is chosen in one step and its undo writes back the settings it replaced", function()
  local _, peer = setup()
  local epoch = place(peer)
  -- (the producer's own settings first: Gain 1 at -7)
  device_data(2, 2).params[1].value = -7
  local record, previewed = change(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = "bright" }, "Kumi: EQ 5 on Bass: preset “Bright” (k1)")
  eq(previewed.display.title, "EQ 5 on Bass: preset \226\128\156Bright\226\128\157")
  eq(previewed.prior.preset, 1)
  eq(record.applied.preset, 2)
  eq(value(2, 2, 1), -2)
  eq(value(2, 2, 2), 6)
  eq(steps(), 1)
  eq(newest(), "Kumi: EQ 5 on Bass: preset “Bright” (k1)")
  ok(record.material.presetData:find("<Value>-7</Value>", 1, true), "the replaced settings are kept")
  eq(record.material.track, 2)
  eq(record.target.kind, "device")
  local back = restore(peer, "device-preset", record)
  ok(back.result, json.encode(back.error or {}))
  eq(value(2, 2, 1), -7, "the producer's gain is back")
  eq(value(2, 2, 2), 0)
  eq(steps(), 2)
  -- Asked again: back already, nothing written.
  local again = restore(peer, "device-preset", record)
  ok(again.result and again.result.undo.entry == false, json.encode(again))
  eq(steps(), 2)
  H.no_dialogs()
end)

test("devices m4: the undo of a preset chosen over another preset chooses that one again (written settings leave none chosen)", function()
  local _, peer = setup()
  local epoch = place(peer)
  local record = change(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = "bright" })
  eq(device_data(2, 2).preset, 2)
  ok(restore(peer, "device-preset", record).result)
  eq(device_data(2, 2).preset, 1, "Init again, as it was")
  eq(value(2, 2, 1), 0)
  eq(value(2, 2, 2), 0)
  H.no_dialogs()
end)

test("devices m4: a preset is named by number, name or the last part of a name; a wrong one is refused with the list", function()
  local _, peer = setup()
  local epoch = place(peer)
  local by_number = preview(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = 2 })
  eq(by_number.display.after.preset, "Bright")
  local response = call(peer, "change_preview", { kind = "device-preset", input = { ref = epoch .. ":device:2.2", preset = "Dark" }, epoch = epoch, layout = select(2, place(peer)) })
  failure(response, "bad-params", "Init, Bright")
  response = call(peer, "change_preview", { kind = "device-preset", input = { ref = epoch .. ":device:2.2", preset = 3 }, epoch = epoch, layout = select(2, place(peer)) })
  failure(response, "bad-params", "from 1 to 2")
  -- The mixer device is the track's fader.
  response = call(peer, "change_preview", { kind = "device-preset", input = { ref = epoch .. ":device:2.1", preset = 1 }, epoch = epoch, layout = select(2, place(peer)) })
  failure(response, "bad-params", "mixer")
end)

test("devices m4: a preset choice refuses a device whose settings moved since the preview (its fence), writing nothing", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = epoch .. ":device:2.2", preset = "Bright" }
  local previewed = preview(peer, "device-preset", input)
  device_data(2, 2).params[2].value = 3 -- (the producer, between preview and apply)
  local done = apply(peer, "device-preset", input, previewed, "Kumi: x (k1)", epoch, layout)
  failure(done, "changed")
  eq(value(2, 2, 2), 3)
  eq(steps(), 0, "nothing written")
end)

test("devices m4: the undo of a preset refuses settings the producer changed since, and discard puts them back anyway", function()
  local _, peer = setup()
  local epoch = place(peer)
  local record = change(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = 2 })
  device_data(2, 2).params[1].value = 11
  local refused = restore(peer, "device-preset", record)
  failure(refused, "changed", "changed in Renoise")
  eq(value(2, 2, 1), 11, "nothing written")
  local forced = restore(peer, "device-preset", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(value(2, 2, 1), 0)
end)

test("devices m4: the undo of a preset finds the device through devices and tracks made, moved and deleted since", function()
  local _, peer = setup()
  local epoch = place(peer)
  local record = change(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = 2 })
  local song = mock.song
  -- A device made before it on its track, one on another track, a track made before its track, a swap of the two after.
  song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2)
  song:track(1):insert_device_at("Audio/Effects/Native/Gainer", 2)
  mock.api.insert_track_at(1)
  H.tick(2)
  local layout_now = select(2, place(peer))
  ok(layout_now > record.layout, "the layout moved")
  local back = restore(peer, "device-preset", record)
  ok(back.result, json.encode(back.error or {}))
  eq(device_data(3, 3).params[1].value, 0, "the EQ (now track 3, device 3) has its settings back")
  eq(device_data(3, 3).name, "EQ 5")
end)

test("devices m4: the undo of a preset refuses what it can't follow (the EQ deleted, another device at its place)", function()
  local _, peer = setup()
  local epoch = place(peer)
  local record = change(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = 2 })
  mock.song:track(2):delete_device_at(2)
  H.tick(2)
  failure(restore(peer, "device-preset", record), "gone", "deleted")
  -- Its track deleted.
  local _, peer2 = setup()
  local epoch2 = place(peer2)
  local record2 = change(peer2, "device-preset", { ref = epoch2 .. ":device:2.2", preset = 2 })
  mock.api.delete_track_at(2)
  H.tick(2)
  failure(restore(peer2, "device-preset", record2), "gone")
end)

test("devices m4: Renoise's own undo of a preset choice is told, and Kumi's undo then finds it back already", function()
  local _, peer = setup()
  local epoch = place(peer)
  local record = change(peer, "device-preset", { ref = epoch .. ":device:2.2", preset = 2 }, "Kumi: preset (kabc1)")
  peer.events = {}
  mock.api.undo()
  H.tick(4)
  local told
  for _, event in ipairs(peer.events) do
    if event.name == "undone_in_daw" then told = event.data end
  end
  call(peer, "ping")
  for _, event in ipairs(peer.events) do
    if event.name == "undone_in_daw" then told = event.data end
  end
  ok(told, "undone_in_daw")
  eq(told.descs[1], "Kumi: preset (kabc1)")
  eq(told.how, "undo")
  local back = restore(peer, "device-preset", record)
  ok(back.result and back.result.undo.entry == false, json.encode(back))
end)

-- device-preset-data

test("devices m4: preset data is written whole in one step and put back; another device's data is refused before Renoise crashes", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local current = mock.song:track(2):device(2).active_preset_data
  local wanted = current:gsub("(<Gain1>%s*<Value>)[^<]*", "%1-4.5", 1)
  local record = change(peer, "device-preset-data", { ref = epoch .. ":device:2.2", presetData = wanted })
  eq(value(2, 2, 2), -4.5)
  eq(steps(), 1)
  local gainer_data = mock.song:track(2):device(3).active_preset_data
  local response = call(peer, "change_preview", { kind = "device-preset-data", input = { ref = epoch .. ":device:2.2", presetData = gainer_data }, epoch = epoch, layout = layout })
  failure(response, "bad-params", "GainerDevice")
  local newer = current:gsub('doc_version="14"', 'doc_version="15"', 1)
  response = call(peer, "change_preview", { kind = "device-preset-data", input = { ref = epoch .. ":device:2.2", presetData = newer }, epoch = epoch, layout = layout })
  failure(response, "bad-params", "newer")
  response = call(peer, "change_preview", { kind = "device-preset-data", input = { ref = epoch .. ":device:2.2", presetData = "<oops" }, epoch = epoch, layout = layout })
  failure(response, "bad-params", "isn't a device preset")
  local back = restore(peer, "device-preset-data", record)
  ok(back.result, json.encode(back.error or {}))
  eq(value(2, 2, 2), 0)
  H.no_dialogs()
end)

test("devices m4: the undo of a Doofer's preset data refuses what the producer changed inside it since, and discard writes anyway (#361 N2)", function()
  local _, peer = setup()
  local epoch = place(peer)
  local current = mock.song:track(3):device(2).active_preset_data
  local wanted = current:gsub("(<Macro0>%s*<Value>)[^<]*", "%175", 1)
  local record = change(peer, "device-preset-data", { ref = epoch .. ":device:3.2", presetData = wanted })
  eq(value(3, 2, 1), 75)
  -- the producer puts a device inside it (its macros don't move)
  local inner = "<GainerDevice type=\"GainerDevice\">\n          <Volume>\n            <Value>1.0</Value>\n          </Volume>\n        </GainerDevice>"
  device_data(3, 2).preset_data = device_data(3, 2).preset_data:gsub("</Devices>", inner .. "\n      </Devices>", 1)
  local theirs = device_data(3, 2).preset_data
  local before = steps()
  failure(restore(peer, "device-preset-data", record), "changed", "inside")
  eq(device_data(3, 2).preset_data, theirs, "what they put inside stays")
  eq(steps(), before)
  local forced = restore(peer, "device-preset-data", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(value(3, 2, 1), 50)
  H.no_dialogs()
end)

test("devices m4: a Doofer's preset data with a routing device inside or a macro mapped past its chain is refused before Renoise crashes (#361 N17)", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local current = mock.song:track(3):device(2).active_preset_data
  local function asked(xml)
    return call(peer, "change_preview", { kind = "device-preset-data", input = { ref = epoch .. ":device:3.2", presetData = xml }, epoch = epoch, layout = layout })
  end
  local function mapped(chain, device)
    return (current:gsub("<Name>Macro 1</Name>", string.format("<Name>Macro 1</Name>\n      <Mappings>\n        <Mapping>\n          <DestChainIndex>%d</DestChainIndex>\n"
      .. "          <DestDeviceIndex>%d</DestDeviceIndex>\n        </Mapping>\n      </Mappings>", chain, device), 1))
  end
  local send = current:gsub("</Devices>", "<SendDevice type=\"SendDevice\">\n        </SendDevice>\n      </Devices>", 1)
  failure(asked(send), "bad-params", "routing")
  failure(asked(mapped(-1, 0)), "bad-params", "chain %-1")
  failure(asked(mapped(0, 5)), "bad-params", "device 5")
  ok(asked(mapped(0, 0)).result, "a mapping to its one device is fine")
  eq(steps(), 0)
end)

test("devices m4: preset data a plug-in of another kind would get is refused", function()
  local plugin = { name = "AUDelay", path = "Audio/Effects/AU/aufx:dely:appl", slot = "AudioPluginDevice",
    params = { { name = "Dry/Wet Mix", value = 0.5, min = 0, max = 1 } },
    xml_body = "    <PluginIdentifier>aufx:dely:appl</PluginIdentifier>" }
  local _, peer = setup({ { name = "Kick", devices = { plugin } } })
  local epoch, layout = place(peer)
  local data = mock.song:track(1):device(2).active_preset_data:gsub("aufx:dely:appl", "aufx:dist:appl")
  local response = call(peer, "change_preview", { kind = "device-preset-data", input = { ref = epoch .. ":device:1.2", presetData = data }, epoch = epoch, layout = layout })
  failure(response, "bad-params", "aufx:dist:appl")
end)

-- device-set

test("devices m4: a device is renamed and collapsed in one step, put back, and refused when renamed again since", function()
  local _, peer = setup()
  local epoch = place(peer)
  local record, previewed = change(peer, "device-set", { ref = epoch .. ":device:2.2", name = "Low cut", maximized = false }, "Kumi: rename (k1)")
  eq(previewed.display.title, "Renamed \226\128\156EQ 5\226\128\157 on \226\128\156Bass\226\128\157 \226\134\146 \226\128\156Low cut\226\128\157, collapsed")
  eq(device_data(2, 2).display_name, "Low cut")
  eq(device_data(2, 2).maximized, false)
  eq(steps(), 1)
  local back = restore(peer, "device-set", record)
  ok(back.result, json.encode(back.error or {}))
  eq(device_data(2, 2).display_name, "EQ 5")
  local record2 = change(peer, "device-set", { ref = place(peer) .. ":device:2.2", name = "Air" })
  mock.song:track(2):device(2).display_name = "Producer's"
  failure(restore(peer, "device-set", record2), "changed", "renamed")
  eq(device_data(2, 2).display_name, "Producer's")
end)

test("devices m4: a collapse alone is titled by the device; the same look again is unchanged and makes no step", function()
  local _, peer = setup()
  local epoch = place(peer)
  device_data(2, 2).maximized = true
  local previewed = preview(peer, "device-set", { ref = epoch .. ":device:2.2", maximized = false })
  eq(previewed.display.title, "EQ 5 on Bass collapsed")
  local same = preview(peer, "device-set", { ref = epoch .. ":device:2.2", maximized = true })
  eq(same.display.unchanged, true)
  local done = change(peer, "device-set", { ref = epoch .. ":device:2.2", maximized = true })
  eq(done.undo.entry, false)
  eq(steps(), 0)
end)

-- The host's XML kinds

test("devices m4: preset_read gives the host a device's preset data, its shape and where a record finds it", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout })
  ok(read.result, json.encode(read.error or {}))
  ok(read.result.presetData:find('<DeviceSlot type="DooferDevice">', 1, true))
  eq(read.result.path, "Audio/Effects/Native/Doofer")
  eq(read.result.at.track, 3)
  eq(read.result.parameters[1].name, "Macro 1")
  ok(read.result.shape:match("^%x+:%d+$"), read.result.shape)
  -- A macro moved (a value) leaves the shape as it was; a name changed doesn't.
  mock.song:track(3):device(2):parameter(1).value = 80
  local moved = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result
  eq(moved.shape, read.result.shape)
  ok(moved.print ~= read.result.print)
  -- By a record's place, after a device made before it.
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 2)
  H.tick(2)
  local e2, l2 = place(peer)
  local by_place = call(peer, "preset_read", { at = read.result.at, epoch = epoch, layout = layout })
  ok(by_place.result, json.encode(by_place.error or {}))
  eq(by_place.result.ref, e2 .. ":device:3.3")
  eq(l2, layout)
end)

test("devices m4: a host-made Doofer edit is written whole if its shape holds, and refused when it changed in between", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result
  local edited = read.presetData:gsub("<Name>Macro 1</Name>", "<Name>Drive</Name>", 1)
  local fence = { id = read.id }
  local input = { ref = epoch .. ":device:3.2", presetData = edited, shape = read.shape, display = { title = "Doofer on Pad: macro 1 named “Drive”" } }
  local done = call(peer, "change_apply", { kind = "doofer-macro", input = input, fence = fence, desc = "Kumi: macro (k1)", epoch = epoch, layout = layout })
  ok(done.result, json.encode(done.error or {}))
  ok(device_data(3, 2).preset_data:find("<Name>Drive</Name>", 1, true))
  eq(done.result.display.title, "Doofer on Pad: macro 1 named “Drive”")
  eq(steps(), 1)
  -- The producer renames the macro in Renoise: the host's next edit from the old read is refused.
  device_data(3, 2).preset_data = device_data(3, 2).preset_data:gsub("<Name>Drive</Name>", "<Name>Theirs</Name>", 1)
  local stale = call(peer, "change_apply", { kind = "doofer-macro", input = input, fence = fence, desc = "Kumi: macro (k2)", epoch = epoch, layout = layout })
  failure(stale, "changed", "read it again")
  eq(steps(), 1)
  -- The restore the host composes, written by the record's place.
  local now = call(peer, "preset_read", { at = done.result.material, epoch = done.result.epoch, layout = done.result.layout }).result
  local material = done.result.material
  material.write = { presetData = now.presetData:gsub("<Name>Theirs</Name>", "<Name>Macro 1</Name>", 1), shape = now.shape }
  local back = call(peer, "change_restore", { kind = "doofer-macro", target = done.result.target, prior = {}, applied = done.result.applied,
    material = material, desc = "Kumi: undo macro (k3)", epoch = done.result.epoch, layout = done.result.layout })
  ok(back.result, json.encode(back.error or {}))
  ok(device_data(3, 2).preset_data:find("<Name>Macro 1</Name>", 1, true))
end)

-- (the tool's byte-by-byte prints counted in KUMI_PRINTS: a line added to changes_material's `print_now`, which every print,
-- shape and content made goes through, and nothing kept does)
local function counting_prints(env_)
  local path = env_.bundle .. "payload/changes_material.lua"
  local f = assert(io.open(path, "rb"))
  local text = f:read("*a")
  f:close()
  local counted, n = text:gsub("local function print_now%(text%)\n", "local function print_now(text)\n  KUMI_PRINTS = (KUMI_PRINTS or 0) + 1\n")
  assert(n == 1)
  f = assert(io.open(path, "wb"))
  f:write(counted)
  f:close()
end

local function setup_counting_prints()
  KUMI_PRINTS = 0
  local env = H.setup({ song = {
    name = "Lab", file_name = "/s/lab.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = { { name = "Kick" }, { name = "Bass", devices = { eq5(), gainer() } }, { name = "Pad", devices = { doofer() } } },
    send_tracks = 1, patterns = 2, pattern_lines = 16, instruments = 1,
  }, patch = counting_prints })
  local peer = H.connect(env)
  H.frames(peer)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

test("devices m4: the journal's poll of a Doofer it keeps prints its text once while the text stays the same (#361 N-S16)", function()
  local _, peer = setup_counting_prints()
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result
  local edited = read.presetData:gsub("<Name>Macro 1</Name>", "<Name>Drive</Name>", 1)
  local input = { ref = epoch .. ":device:3.2", presetData = edited, shape = read.shape, display = { title = "Doofer on Pad: macro 1 named “Drive”" } }
  local done = call(peer, "change_apply", { kind = "doofer-macro", input = input, fence = { id = read.id }, desc = "Kumi: macro (kp16)", epoch = epoch, layout = layout })
  ok(done.result, json.encode(done.error or {}))
  local after_apply = KUMI_PRINTS
  for _ = 1, 5 do mock.advance(1100) end -- five polls
  ok(KUMI_PRINTS - after_apply <= 1, "printed once for the unchanged text, not on each poll: " .. (KUMI_PRINTS - after_apply))
  KUMI_PRINTS = nil
end)

test("devices m4: Renoise's own undo and redo of a value inside a Doofer (no notifier tells them) are told within a second or so", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result
  -- (the Gainer's gain inside it: no parameter of the Doofer's own moves)
  local edited, n = read.presetData:gsub("<Value>1.0</Value>", "<Value>0.5</Value>", 1)
  eq(n, 1)
  local input = { ref = epoch .. ":device:3.2", presetData = edited, shape = read.shape, display = { title = "Gain inside" } }
  local done = call(peer, "change_apply", { kind = "doofer-device-set", input = input, fence = { id = read.id }, desc = "Kumi: inside (k9)", epoch = epoch, layout = layout })
  ok(done.result, json.encode(done.error or {}))
  local function told()
    mock.advance(1100)
    call(peer, "ping")
    local out = {}
    for _, event in ipairs(peer.events or {}) do
      if event.name == "undone_in_daw" then out[#out + 1] = event.data.how .. ":" .. event.data.descs[1] end
    end
    peer.events = {}
    return table.concat(out, ",")
  end
  told()
  mock.song:undo()
  ok(mock.song:track(3):device(2).active_preset_data:find("<Value>1.0</Value>", 1, true), "Renoise's undo")
  eq(told(), "undo:Kumi: inside (k9)")
  mock.song:redo()
  eq(told(), "redo:Kumi: inside (k9)")
  eq(told(), "", "told once")
end)

test("devices m4: host-made edits go only to the devices they are for, with their own device type's data", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local eq_read = call(peer, "preset_read", { ref = epoch .. ":device:2.2", epoch = epoch, layout = layout }).result
  local response = call(peer, "change_apply", { kind = "doofer-macro", input = { ref = epoch .. ":device:2.2", presetData = eq_read.presetData, shape = eq_read.shape },
    fence = { id = eq_read.id }, desc = "Kumi: x (k1)", epoch = epoch, layout = layout })
  failure(response, "bad-params", "isn't a device")
  local doofer_read = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result
  response = call(peer, "change_apply", { kind = "doofer-macro", input = { ref = epoch .. ":device:3.2", presetData = eq_read.presetData, shape = doofer_read.shape },
    fence = { id = doofer_read.id }, desc = "Kumi: x (k1)", epoch = epoch, layout = layout })
  failure(response, "bad-params", "crash")
  response = call(peer, "change_preview", { kind = "doofer-macro", input = { ref = epoch .. ":device:3.2" }, epoch = epoch, layout = layout })
  failure(response, "bad-params", "host")
  eq(steps(), 0)
  H.no_dialogs()
end)

test("devices m4: a Doofer holding a plug-in takes the host's data without it, and its read names the plug-in as Renoise lists it", function()
  -- (lab 3.5.4: the inner plug-in's identifier is no part of what the Doofer is; Renoise's XML gives the plug-in no name)
  local d = doofer()
  d.xml_body = "    <DeviceChain>\n      <Devices>\n        <GainerDevice type=\"GainerDevice\"/>\n        <AudioPluginDevice type=\"AudioPluginDevice\">\n"
    .. "          <PluginType>AU</PluginType>\n          <PluginIdentifier>aufx:dely:appl</PluginIdentifier>\n        </AudioPluginDevice>\n      </Devices>\n    </DeviceChain>"
  local _, peer = setup({ { name = "Pad", devices = { d } } })
  mock.device_catalog = { ["Audio/Effects/AU/aufx:dely:appl"] = {} }
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:1.2", epoch = epoch, layout = layout }).result
  eq(read.plugins and read.plugins["aufx:dely:appl"], "aufx:dely:appl", "named by Renoise's list (the mock's short name is the path's last part)")
  local without = read.presetData:gsub("%s*<AudioPluginDevice.-</AudioPluginDevice>", "", 1)
  ok(not without:find("PluginIdentifier", 1, true))
  local done = call(peer, "change_apply", { kind = "doofer-device-delete", input = { ref = epoch .. ":device:1.2", presetData = without, shape = read.shape },
    fence = { id = read.id }, desc = "Kumi: delete (k1)", epoch = epoch, layout = layout })
  ok(done.result, json.encode(done.error or {}))
  ok(not device_data(1, 2).preset_data:find("PluginIdentifier", 1, true), "written")
  mock.device_catalog = nil
end)

-- Routing (M4 n-devices): every Receiver checked, #Sidechain's track a place, and the routes a delete clears set again.

local function params_of(list)
  local out = {}
  for i, entry in ipairs(list) do out[i] = { name = entry[1], value = entry[2], min = entry[3] or 0, max = entry[4] or 1 } end
  return out
end

local function hydra()
  local list = { { "Input", 0.5 } }
  for n = 1, 2 do
    for _, part in ipairs({ { " Track", -1, 0, 255 }, { " Effect", -1, 0, 255 }, { " Parameter", -1, 0, 255 }, { " Min", 0 }, { " Max", 1 } }) do
      list[#list + 1] = { "Out" .. n .. part[1], part[2], part[3], part[4] }
    end
  end
  return { name = "*Hydra", path = "Audio/Effects/Native/*Hydra", params = params_of(list) }
end

local function sidechain()
  return { name = "#Sidechain", path = "Audio/Effects/Native/#Sidechain",
    params = params_of({ { "Amount", 1 }, { "Panning", 0.5 }, { "Dest. Track", -1, 0, 255 }, { "Dest. Effect", -1, 0, 255 } }) }
end

local function multiband()
  return { name = "#Multiband Send", path = "Audio/Effects/Native/#Multiband Send",
    params = params_of({ { "Amount 1", 1 }, { "Receiver 1", 0, 0, 255 }, { "Amount 2", 1 }, { "Receiver 2", 0, 0, 255 }, { "Amount 3", 1 }, { "Receiver 3", 0, 0, 255 }, { "Low", 0.33 }, { "High", 0.78 } }) }
end

local function plain_gainer()
  return { name = "Gainer", path = "Audio/Effects/Native/Gainer", params = params_of({ { "Gain", 1, 0, 4 }, { "Panning", 0.5 } }) }
end

local function values(t, d, from, to)
  local out = {}
  for k = from, to do out[#out + 1] = tostring(device_data(t, d).params[k].value) end
  return table.concat(out, "/")
end

test("devices m4: every Receiver of a #Multiband Send is checked against the send tracks, and none (-1) is refused (SHOULD 3)", function()
  local _, peer = setup({ { name = "Kick", devices = { multiband() } }, { name = "Bass" } })
  local epoch, layout = place(peer)
  for _, case in ipairs({ { value = -1 }, { value = 1 }, { value = 0.5 } }) do
    local response = call(peer, "change_preview", { kind = "device-param", input = { ref = epoch .. ":param:1.2.4", value = case.value }, epoch = epoch, layout = layout })
    failure(response, "bad-params", "Receiver 2 is a send track's number from 0 to 0")
  end
  local fine = call(peer, "change_preview", { kind = "device-param", input = { ref = epoch .. ":param:1.2.6", value = 0 }, epoch = epoch, layout = layout })
  ok(fine.result, json.encode(fine.error or {}))
end)

test("devices m4: #Sidechain's Dest. Track reads its text from 1 and is kept as the track it names through tracks made since", function()
  local _, peer = setup({ { name = "Kick", devices = { sidechain() } }, { name = "Bass", devices = { plain_gainer() } }, { name = "Pad", devices = { plain_gainer() } } })
  local epoch = place(peer)
  local record, previewed = change(peer, "device-param", { ref = epoch .. ":param:1.2.3", valueString = "3" })
  eq(device_data(1, 2).params[3].value, 2, "track 3 is the value 2")
  ok(type(record.applied.at) == "string" and record.applied.at:match(":track:3$"), json.encode(record.applied))
  eq(previewed.prior.value, -1)
  -- A track made before it: Renoise follows it (the value is 3 now); Kumi's undo still finds what it wrote.
  mock.api.insert_track_at(1)
  H.tick(2)
  eq(device_data(2, 2).params[3].value, 3)
  local back = restore(peer, "device-param", record)
  ok(back.result, json.encode(back.error or {}))
  eq(device_data(2, 2).params[3].value, -1, "None again")
end)

test("devices m4: a deleted track's routings from *Hydra outputs and #Sidechain are set again by Kumi's undo (S-5)", function()
  local kick = { name = "Kick", devices = { hydra(), sidechain() } }
  local _, peer = setup({ kick, { name = "Bass", devices = { plain_gainer() } }, { name = "Pad" } })
  -- Out2 drives Bass's Gainer's Gain; the #Sidechain feeds Bass's Gainer.
  local h, sc = device_data(1, 2), device_data(1, 3)
  h.params[7].value, h.params[8].value, h.params[9].value = 1, 1, 1
  sc.params[3].value, sc.params[4].value = 1, 1
  local epoch = place(peer)
  local record, previewed = change(peer, "track-delete", { ref = epoch .. ":track:2" })
  ok(previewed and record.material and #record.material.routes == 2, json.encode(record.material and record.material.routes or {}))
  eq(values(1, 2, 7, 9), "-1/-1/-1", "Renoise let go of Out2")
  eq(values(1, 3, 3, 4), "-1/-1", "and of the #Sidechain")
  local back = restore(peer, "track-delete", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 7, 9), "1/1/1", "Out2 drives the Gainer again")
  eq(values(1, 3, 3, 4), "1/1", "the #Sidechain feeds it again")
  ok(back.result.display.note and back.result.display.note:find("2 routings aimed at it set again"), json.encode(back.result.display))
end)

test("devices m4: a deleted device's own *Hydra outputs, and an output aimed at a deleted device, are aimed again by Kumi's undo", function()
  local _, peer = setup({ { name = "Kick", devices = { hydra() } }, { name = "Bass", devices = { plain_gainer(), plain_gainer() } } })
  -- (a device made again has its device's parameters, at their defaults)
  mock.device_catalog = { ["Audio/Effects/Native/*Hydra"] = hydra().params, ["Audio/Effects/Native/Gainer"] = plain_gainer().params }
  local h = device_data(1, 2)
  h.params[2].value, h.params[3].value, h.params[4].value = 1, 2, 1 -- Out1: Bass, its second Gainer (device 3), Gain
  local epoch = place(peer)
  -- The Gainer it drives deleted, then put back.
  local record = change(peer, "device-delete", { ref = epoch .. ":device:2.3" })
  eq(values(1, 2, 2, 4), "1/-1/-1")
  local back = restore(peer, "device-delete", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 2, 4), "1/2/1", "aimed at the Gainer again")
  -- The *Hydra itself deleted and put back: its output is aimed again (its preset data doesn't hold it).
  local record2 = change(peer, "device-delete", { ref = place(peer) .. ":device:1.2" })
  local back2 = restore(peer, "device-delete", record2)
  ok(back2.result, json.encode(back2.error or {}))
  eq(values(1, 2, 2, 4), "1/2/1", "its own output again")
  mock.device_catalog = nil
end)

-- meta-destination

local function lfo()
  return { name = "*LFO", path = "Audio/Effects/Native/*LFO",
    params = params_of({ { "Dest. Track", -1, 0, 255 }, { "Dest. Effect", -1, 0, 255 }, { "Dest. Parameter", -1, 0, 255 }, { "Amplitude", 0.5 } }) }
end

local function meta_song()
  return setup({
    { name = "Kick", devices = { lfo(), hydra(), sidechain() } },
    { name = "Bass", devices = { plain_gainer() } },
    { name = "Pad", devices = { plain_gainer(), eq5() } },
  })
end

test("devices m4: a meta device is aimed at a device's parameter by refs, and Kumi's undo aims it back through devices and tracks made since", function()
  local _, peer = meta_song()
  local epoch = place(peer)
  local record, previewed = change(peer, "meta-destination", { ref = epoch .. ":device:1.2", targetRef = epoch .. ":device:2.2", param = "Gain" })
  eq(previewed.display.title, "*LFO on Kick drives \226\128\156Gain\226\128\157 of \226\128\156Gainer\226\128\157 on \226\128\156Bass\226\128\157")
  eq(values(1, 2, 1, 3), "1/1/1")
  eq(steps(), 1)
  -- A device made before the Gainer and a track made before Bass: Renoise follows them (2/2/1 then 3/2/1).
  mock.song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2)
  mock.api.insert_track_at(2)
  H.tick(2)
  eq(values(1, 2, 1, 3), "2/2/1")
  local back = restore(peer, "meta-destination", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 1, 3), "-1/-1/-1", "aimed nowhere, as before")
  local again = restore(peer, "meta-destination", record)
  ok(again.result and again.result.undo.entry == false, "back already: " .. json.encode(again))
end)

test("devices m4: a meta device aimed at a track but no device yet gets that track back from Kumi's undo (#361 N-N7)", function()
  local _, peer = meta_song()
  local l = device_data(1, 2)
  l.params[1].value = 2 -- the producer's half-made aim: Pad (track 3), no device yet
  local epoch = place(peer)
  local record = change(peer, "meta-destination", { ref = epoch .. ":device:1.2", targetRef = epoch .. ":device:2.2", param = "Gain" })
  eq(values(1, 2, 1, 3), "1/1/1")
  local back = restore(peer, "meta-destination", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 1, 3), "2/-1/-1", "Pad again, no device: as the producer left it")
end)

test("devices m4: a meta device aimed elsewhere before is aimed back there, followed; gone or changed since, refused", function()
  local _, peer = meta_song()
  -- Before Kumi: the LFO drives Pad's EQ (device 3) Gain 2.
  local l = device_data(1, 2)
  l.params[1].value, l.params[2].value, l.params[3].value = 2, 2, 2
  local epoch = place(peer)
  local record = change(peer, "meta-destination", { ref = epoch .. ":device:1.2", targetRef = epoch .. ":device:2.2", param = 1 })
  eq(values(1, 2, 1, 3), "1/1/1")
  -- The producer deletes the Gainer before Pad's EQ: the EQ is device 2 now.
  mock.song:track(3):delete_device_at(2)
  H.tick(2)
  local back = restore(peer, "meta-destination", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 1, 3), "2/1/2", "back on the EQ, at its new place")
  -- Aimed again; then the EQ (where it pointed before) deleted: Kumi can't aim it back.
  local record2 = change(peer, "meta-destination", { ref = place(peer) .. ":device:1.2", targetRef = place(peer) .. ":device:2.2", param = 1 })
  mock.song:track(3):delete_device_at(2)
  H.tick(2)
  failure(restore(peer, "meta-destination", record2), "gone", "can't aim it back")
  eq(values(1, 2, 1, 3), "1/1/1", "nothing written")
  -- Re-aimed by the producer since: refused, and discard aims it back anyway.
  local _, peer3 = meta_song()
  local e3 = place(peer3)
  local record3 = change(peer3, "meta-destination", { ref = e3 .. ":device:1.2", targetRef = e3 .. ":device:2.2", param = 2 })
  device_data(1, 2).params[3].value = 1
  failure(restore(peer3, "meta-destination", record3), "changed", "changed in Renoise")
  local forced = restore(peer3, "meta-destination", record3, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(values(1, 2, 1, 3), "-1/-1/-1")
end)

test("devices m4: a meta destination is fenced, outputs are named, #Sidechain feeds a device, and nothing aims at itself", function()
  local _, peer = meta_song()
  local epoch, layout = place(peer)
  local input = { ref = epoch .. ":device:1.2", targetRef = epoch .. ":device:2.2", param = 1 }
  local previewed = preview(peer, "meta-destination", input)
  device_data(1, 2).params[1].value = 2 -- (the producer re-aims it between preview and apply)
  failure(apply(peer, "meta-destination", input, previewed, "Kumi: x (k1)", epoch, layout), "changed")
  eq(steps(), 0)
  -- *Hydra: which output.
  local response = call(peer, "change_preview", { kind = "meta-destination", input = { ref = epoch .. ":device:1.3", targetRef = epoch .. ":device:2.2", param = 1 }, epoch = epoch, layout = layout })
  failure(response, "bad-params", "give out")
  local out2 = change(peer, "meta-destination", { ref = epoch .. ":device:1.3", out = 2, targetRef = epoch .. ":device:3.3", param = "Gain 2" })
  eq(values(1, 3, 7, 9), "2/2/2")
  eq(out2.display.title, "*Hydra on Kick: output 2 drives \226\128\156Gain 2 (1.00 kHz)\226\128\157 of \226\128\156EQ 5\226\128\157 on \226\128\156Pad\226\128\157")
  -- #Sidechain: a device, no parameter.
  local e2, l2 = place(peer)
  failure(call(peer, "change_preview", { kind = "meta-destination", input = { ref = e2 .. ":device:1.4", targetRef = e2 .. ":device:2.2", param = 1 }, epoch = e2, layout = l2 }), "bad-params", "leave param out")
  local fed = change(peer, "meta-destination", { ref = e2 .. ":device:1.4", targetRef = e2 .. ":device:2.2" })
  eq(values(1, 4, 3, 4), "1/1")
  eq(fed.display.title, "#Sidechain on Kick feeds \226\128\156Gainer\226\128\157 on \226\128\156Bass\226\128\157")
  -- None, and itself.
  local none = change(peer, "meta-destination", { ref = e2 .. ":device:1.4", targetRef = json.null })
  eq(values(1, 4, 3, 4), "-1/-1")
  eq(none.display.title, "#Sidechain on Kick sends nowhere")
  failure(call(peer, "change_preview", { kind = "meta-destination", input = { ref = e2 .. ":device:1.2", targetRef = e2 .. ":device:1.2", param = 1 }, epoch = e2, layout = l2 }), "bad-params", "itself")
end)

-- doofer-make

local DOOFER_XML = '<?xml version="1.0" encoding="UTF-8"?>\n<FilterDevicePreset doc_version="14">\n  <DeviceSlot type="DooferDevice">\n'
  .. '    <Macro0>\n      <Value>50</Value>\n      <Name>Tone</Name>\n    </Macro0>\n    <DeviceChain>\n      <Devices>\n'
  .. '        <Eq5Device type="Eq5Device"/>\n        <GainerDevice type="GainerDevice"/>\n      </Devices>\n    </DeviceChain>\n  </DeviceSlot>\n</FilterDevicePreset>\n'

local function make_song(extra)
  local env, peer = setup(extra or {
    { name = "Kick", devices = { lfo() } },
    { name = "Bass", devices = { eq5(), gainer(), lfo() } },
    { name = "Pad" },
  })
  local macros = {}
  for k = 1, 8 do macros[k] = { name = "Macro " .. k, value = 50, min = 0, max = 100, element = "Macro" .. (k - 1) } end
  mock.device_catalog = { ["Audio/Effects/Native/Doofer"] = macros, ["Audio/Effects/Native/EQ 5"] = eq5().params,
    ["Audio/Effects/Native/Gainer"] = gainer().params, ["Audio/Effects/Native/*LFO"] = lfo().params }
  mock.device_slots = { ["Audio/Effects/Native/Doofer"] = { slot = "DooferDevice" }, ["Audio/Effects/Native/EQ 5"] = { slot = "Eq5Device" },
    ["Audio/Effects/Native/Gainer"] = { slot = "GainerDevice" } }
  return env, peer
end

local function make(peer, refs_list, more)
  local epoch, layout = place(peer)
  local shapes, ids = {}, {}
  for i, reference in ipairs(refs_list) do
    local read = call(peer, "preset_read", { ref = reference, epoch = epoch, layout = layout }).result
    shapes[i], ids[i] = read.shape, read.id
  end
  local input = { trackRef = epoch .. ":track:2", deviceRefs = refs_list, shapes = shapes, presetData = DOOFER_XML, type = "doofer",
    display = { title = "Made a Doofer" } }
  for k, v in pairs(more or {}) do input[k] = v end
  return call(peer, "change_apply", { kind = "doofer-make", input = input, fence = { ids = ids }, desc = "Kumi: make (k1)", epoch = epoch, layout = layout })
end

local function paths(t)
  local out = {}
  for d = 2, #mock.data.tracks[t].devices do out[#out + 1] = mock.data.tracks[t].devices[d].name end
  return table.concat(out, ", ")
end

test("devices m4: a Doofer is made of devices on a track in one step, and Kumi's undo takes it apart with their settings and names", function()
  local _, peer = make_song()
  local epoch = place(peer)
  device_data(2, 2).params[1].value = -5
  device_data(2, 3).display_name = "Trim"
  local done = make(peer, { epoch .. ":device:2.2", epoch .. ":device:2.3" }, { name = "Tone" })
  ok(done.result, json.encode(done.error or {}))
  eq(paths(2), "Doofer, *LFO")
  eq(device_data(2, 2).display_name, "Tone")
  ok(device_data(2, 2).preset_data:find("<Name>Tone</Name>", 1, true))
  eq(done.result.created.ref, done.result.epoch .. ":device:2.2")
  eq(steps(), 1, "one step")
  eq(#done.result.material.devices, 2)
  -- Kumi's undo, after a device made on another track (an unrelated device change).
  mock.song:track(1):insert_device_at("Audio/Effects/Native/Gainer", 2)
  H.tick(2)
  local back = restore(peer, "doofer-make", done.result)
  ok(back.result, json.encode(back.error or {}))
  eq(paths(2), "EQ 5, Gainer, *LFO")
  eq(value(2, 2, 1), -5, "the EQ's settings")
  eq(device_data(2, 3).display_name, "Trim", "the Gainer's name")
  eq(back.result.display.note, nil, "at their own places")
end)

test("devices m4: a made Doofer's undo refuses a knob turned inside it since, which taking it apart would lose; discard takes it apart (#361 N9)", function()
  local _, peer = make_song()
  local epoch = place(peer)
  local done = make(peer, { epoch .. ":device:2.2", epoch .. ":device:2.3" })
  ok(done.result, json.encode(done.error or {}))
  -- the producer turns a macro inside (a value: the shape holds)
  device_data(2, 2).params[1].value = 80
  local before = steps()
  failure(restore(peer, "doofer-make", done.result), "changed", "settings inside")
  eq(paths(2), "Doofer, *LFO", "left as it is")
  eq(steps(), before)
  local forced = restore(peer, "doofer-make", done.result, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(paths(2), "EQ 5, Gainer, *LFO")
end)

test("devices m4: a Doofer's preset data with a routing device inside is refused before the container is made, leaving no step (#361 N-N2)", function()
  local _, peer = make_song()
  local epoch = place(peer)
  local bad_xml = DOOFER_XML:gsub("</Devices>", "<SendDevice type=\"SendDevice\"/>\n      </Devices>", 1)
  local refused = make(peer, { epoch .. ":device:2.2", epoch .. ":device:2.3" }, { presetData = bad_xml })
  failure(refused, "bad-params", "routing")
  eq(paths(2), "EQ 5, Gainer, *LFO", "nothing made")
  eq(steps(), 0, "no step in Renoise's undo")
end)

test("devices m4: a Doofer isn't made of a device with automation, or one something outside aims at; a meta device inside keeps its aim", function()
  local _, peer = make_song()
  local epoch = place(peer)
  -- Kick's LFO aims at Bass's Gainer: it would lose its target.
  local l = device_data(1, 2)
  l.params[1].value, l.params[2].value, l.params[3].value = 1, 2, 1
  local refused = make(peer, { epoch .. ":device:2.2", epoch .. ":device:2.3" })
  failure(refused, "bad-params", "lose its target")
  l.params[1].value, l.params[2].value, l.params[3].value = -1, -1, -1
  -- Automation.
  device_data(2, 2).params[1].automated = true
  failure(make(peer, { epoch .. ":device:2.2" }), "bad-params", "automation")
  device_data(2, 2).params[1].automated = false
  -- Bass's own LFO aims at its Gainer: both go in, and the undo aims it again.
  local own = device_data(2, 4)
  own.params[1].value, own.params[2].value, own.params[3].value = 1, 2, 1
  local done = make(peer, { epoch .. ":device:2.3", epoch .. ":device:2.4" })
  ok(done.result, json.encode(done.error or {}))
  eq(paths(2), "EQ 5, Doofer")
  local back = restore(peer, "doofer-make", done.result)
  ok(back.result, json.encode(back.error or {}))
  eq(paths(2), "EQ 5, Gainer, *LFO")
  eq(values(2, 4, 1, 3), "1/2/1", "the LFO drives the Gainer again")
  eq(steps(), 2)
end)

test("devices m4: a made Doofer's undo puts the devices back together when the track's devices changed, refuses an edited inside, and sees Renoise's own undo", function()
  local _, peer = make_song()
  local epoch = place(peer)
  local done = make(peer, { epoch .. ":device:2.2", epoch .. ":device:2.3" })
  ok(done.result, json.encode(done.error or {}))
  -- A device made on the track before the Doofer since.
  mock.song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2)
  H.tick(2)
  local back = restore(peer, "doofer-make", done.result)
  ok(back.result, json.encode(back.error or {}))
  eq(paths(2), "Gainer, EQ 5, Gainer, *LFO")
  ok(back.result.display.note and back.result.display.note:find("together"), json.encode(back.result.display))
  -- Edited inside since: refused.
  local _, peer2 = make_song()
  local e2 = place(peer2)
  local done2 = make(peer2, { e2 .. ":device:2.2", e2 .. ":device:2.3" })
  device_data(2, 2).preset_data = device_data(2, 2).preset_data:gsub("<Name>Tone</Name>", "<Name>Theirs</Name>", 1)
  failure(restore(peer2, "doofer-make", done2.result), "changed", "inside")
  -- Renoise's own undo took it back: back already.
  local _, peer3 = make_song()
  local e3 = place(peer3)
  local done3 = make(peer3, { e3 .. ":device:2.2", e3 .. ":device:2.3" })
  mock.api.undo()
  H.tick(2)
  eq(paths(2), "EQ 5, Gainer, *LFO")
  local again = restore(peer3, "doofer-make", done3.result)
  ok(again.result and again.result.undo.entry == false, json.encode(again))
  mock.device_catalog, mock.device_slots = nil, nil
end)

test("devices m4: a Doofer isn't made when a device changed since the host read it (its shape), or another is at its place (its id)", function()
  local _, peer = make_song()
  local epoch, layout = place(peer)
  local reference = epoch .. ":device:2.2"
  local read = call(peer, "preset_read", { ref = reference, epoch = epoch, layout = layout }).result
  -- A structural edit by the producer (an element comes): the EQ's shape moves. (Folding it is no such edit: the view.)
  device_data(2, 2).preset_data = mock.song:track(2):device(2).active_preset_data:gsub("</DeviceSlot>", "  <Gain2>\n      <Value>0</Value>\n    </Gain2>\n  </DeviceSlot>", 1)
  local input = { trackRef = epoch .. ":track:2", deviceRefs = { reference }, shapes = { read.shape }, presetData = DOOFER_XML, type = "doofer" }
  failure(call(peer, "change_apply", { kind = "doofer-make", input = input, fence = { ids = { read.id } }, desc = "Kumi: make (k1)", epoch = epoch, layout = layout }), "changed", "read it again")
  input.shapes = nil
  failure(call(peer, "change_apply", { kind = "doofer-make", input = input, fence = { ids = { "track:Other|Audio/Effects/Native/EQ 5" } }, desc = "Kumi: make (k1)", epoch = epoch, layout = layout }), "changed")
  eq(steps(), 0, "nothing written")
  eq(paths(2), "EQ 5, Gainer, *LFO")
  mock.device_catalog, mock.device_slots = nil, nil
end)

-- instrument-macro-map

local function mapped_song()
  local env, peer = setup()
  local instrument = mock.data.instruments[1]
  instrument.name = "Lead"
  instrument.macros[1].mappings = {
    { param = "Cutoff", min = 0, max = 1, scaling = 3 },
    { param = "Resonance", min = 0.2, max = 0.8, scaling = 3 },
  }
  return env, peer
end

local function ranges(k)
  local out = {}
  for _, mapping in ipairs(mock.data.instruments[1].macros[k].mappings or {}) do
    out[#out + 1] = string.format("%s %g-%g %d", mapping.param, mapping.min, mapping.max, mapping.scaling)
  end
  return table.concat(out, ", ")
end

-- renoise.app()'s instrument files for these tests: the file holds the instrument's name and macro mappings; a load names
-- the instrument after the file and tells an insert after its slot and a remove of it, as Renoise 3.5.4 does.
local function with_instrument_files(fn)
  local saved_save, saved_load = mock.app_save_instrument, mock.app_load_instrument
  local fields = mock.data.song_fields
  -- (a mapping as the file keeps it: no observables)
  local function plain(mappings)
    local out = {}
    for j, m in ipairs(mappings or {}) do out[j] = { param = m.param, name = m.name, min = m.min, max = m.max, scaling = m.scaling } end
    return out
  end
  mock.app_save_instrument = function(path)
    local instrument = mock.data.instruments[fields.selected_instrument_index]
    local macros = {}
    for k = 1, 8 do macros[k] = plain(instrument.macros[k].mappings) end
    local f = assert(io.open(path, "wb"))
    f:write(json.encode({ name = instrument.name, macros = macros }))
    f:close()
    return true
  end
  mock.app_load_instrument = function(path)
    local f = io.open(path, "rb")
    if not f then return false end
    local data = json.decode(f:read("*a"))
    f:close()
    local i = fields.selected_instrument_index
    local instrument = mock.data.instruments[i]
    local before = { name = instrument.name, macros = {} }
    for k = 1, 8 do before.macros[k] = instrument.macros[k].mappings or {} end
    local after = { name = path:match("([^/]+)%.xrni$"), macros = data.macros }
    local function put(state)
      instrument.name = state.name
      for k = 1, 8 do instrument.macros[k].mappings = state.macros[k] end
    end
    -- (the loaded instrument goes in after the slot and the old one out, as Renoise tells it; here the same data stands for
    -- both, so the list keeps its contents while the tool sees an object replaced)
    local function told(first, second)
      table.insert(mock.data.instruments, first, instrument)
      mock.fire(fields.instruments_observable, { type = "insert", index = first })
      table.remove(mock.data.instruments, second)
      mock.fire(fields.instruments_observable, { type = "remove", index = second })
    end
    put(after)
    told(i + 1, i)
    -- (Renoise's undo tells the old instrument back at i and the loaded one's remove after it; its redo the other way: lab)
    mock.record(function() put(before) told(i, i + 1) end, function() put(after) told(i + 1, i) end)
    return true
  end
  local ok_, err = pcall(fn)
  mock.app_save_instrument, mock.app_load_instrument = saved_save, saved_load
  if not ok_ then error(err, 0) end
end

-- What Kumi's host does with an exported file (here: the JSON above): the macro's mappings made `mappings`, written beside it.
local function host_edit(file, k, mappings)
  local f = assert(io.open(file, "rb"))
  local data = json.decode(f:read("*a"))
  f:close()
  os.remove(file)
  data.macros[k] = mappings
  local edited = file:gsub("%.xrni$", ".kumi.xrni")
  local out = assert(io.open(edited, "wb"))
  out:write(json.encode(data))
  out:close()
  return edited
end

test("devices m4: Kumi's records inside an instrument still find their places after a macro map loads it again from a file (#361 N-S5)", function()
  local env, peer = mapped_song()
  mock.song:instrument(1):insert_sample_at(1).name = "Saw"
  with_instrument_files(function()
    mock.data.song_fields.selected_instrument_index = 1
    local tuned = change(peer, "sample-set", { ref = place(peer) .. ":sample:1.1", transpose = 3 })
    local epoch, layout = place(peer)
    local export = call(peer, "instrument_export", { ref = epoch .. ":instrument:1", epoch = epoch, layout = layout })
    ok(export.result, json.encode(export.error or {}))
    local mappings = { { param = "Cutoff", min = 0, max = 1, scaling = 3 }, { param = "Resonance", min = 0.2, max = 0.8, scaling = 3 } , { param = "Drive", min = 0, max = 0.5, scaling = 1 } }
    local input = { ref = epoch .. ":instrument:1", macro = 1, file = host_edit(export.result.file, 1, mappings), print = export.result.print, display = { title = "Lead macro 1: mapped to Drive" } }
    local done = call(peer, "change_apply", { kind = "instrument-macro-map", input = input, fence = {}, desc = "Kumi: map (ks51)", epoch = epoch, layout = layout })
    ok(done.result, json.encode(done.error or {}))
    H.tick(2)
    local back = restore(peer, "sample-set", tuned)
    ok(back.result, "the sample is where it was: " .. json.encode(back.error or {}))
    eq(mock.song:instrument(1):sample(1).transpose, 0)
  end)
end)

test("devices m4: a macro's mappings get their ranges and scaling in one step, and Kumi's undo writes back what they were", function()
  local _, peer = mapped_song()
  local epoch = place(peer)
  local record = change(peer, "instrument-macro-map", { ref = epoch .. ":instrument:1", macro = 1, set = { { mapping = 2, min = 0.25, max = 0.5, scaling = "exp-fast" } } })
  eq(ranges(1), "Cutoff 0-1 3, Resonance 0.25-0.5 5")
  eq(steps(), 1)
  ok(record.display.title:find("Lead macro 1"), record.display.title)
  local back = restore(peer, "instrument-macro-map", record)
  ok(back.result, json.encode(back.error or {}))
  eq(ranges(1), "Cutoff 0-1 3, Resonance 0.2-0.8 3")
  eq(steps(), 2)
  -- Back already: nothing written.
  local again = restore(peer, "instrument-macro-map", record)
  ok(again.result and again.result.undo.entry == false, json.encode(again))
  -- The producer's edit since: refused, unless discarded.
  local record2 = change(peer, "instrument-macro-map", { ref = epoch .. ":instrument:1", macro = 1, set = { { mapping = 1, max = 0.5 } } })
  mock.data.instruments[1].macros[1].mappings[1].max = 0.7
  failure(restore(peer, "instrument-macro-map", record2), "changed", "since Kumi set them")
  local discarded = restore(peer, "instrument-macro-map", record2, nil, { discard = true })
  ok(discarded.result, json.encode(discarded.error or {}))
  eq(ranges(1), "Cutoff 0-1 3, Resonance 0.2-0.8 3")
  -- A mapping that isn't there, a scaling that isn't one: refused before anything is written.
  local e = place(peer)
  failure(call(peer, "change_preview", { kind = "instrument-macro-map", input = { ref = e .. ":instrument:1", macro = 1, set = { { mapping = 3, min = 0 } } }, epoch = e }), "bad-params", "from 1 to 2")
  failure(call(peer, "change_preview", { kind = "instrument-macro-map", input = { ref = e .. ":instrument:1", macro = 1, set = { { mapping = 1, scaling = "cubic" } } }, epoch = e }), "bad-params", "scaling")
end)

test("devices m4: the API's mappings are fenced: one the producer adds between the preview and the apply refuses it", function()
  local _, peer = mapped_song()
  local input = { ref = place(peer) .. ":instrument:1", macro = 1, set = { { mapping = 1, min = 0.1 } } }
  local previewed, epoch, layout = preview(peer, "instrument-macro-map", input)
  table.insert(mock.data.instruments[1].macros[2].mappings or {}, { param = "Drive", min = 0, max = 1, scaling = 3 })
  mock.data.instruments[1].macros[2].mappings = { { param = "Drive", min = 0, max = 1, scaling = 3 } }
  failure(apply(peer, "instrument-macro-map", input, previewed, "Kumi: map (k1)", epoch, layout), "changed", "since the preview")
  eq(steps(), 0)
end)

test("devices m4: macro_read lists each macro's mappings as Renoise's API reads them", function()
  local _, peer = mapped_song()
  local epoch, layout = place(peer)
  local read = call(peer, "macro_read", { ref = epoch .. ":instrument:1", epoch = epoch, layout = layout }).result
  eq(#read.macros, 8)
  eq(read.macros[1].name, "Macro 1")
  eq(read.macros[1].mappings[2].param, "Resonance")
  eq(read.macros[1].mappings[2].mapping, 2)
  eq(read.macros[1].mappings[2].scaling, "linear")
  eq(#read.macros[2].mappings, 0)
  ok(json.is_array(read.macros[2].mappings), "an empty list goes out as [] (lab: {} broke the host's read)")
end)

test("devices m4: mappings added through the instrument's file load back into the same slot in one step, its name and the selection kept, and Kumi's undo loads the host's restore file", function()
  local env, peer = mapped_song()
  mock.data.instruments[2] = nil
  with_instrument_files(function()
    mock.data.song_fields.selected_instrument_index = 1
    local epoch, layout = place(peer)
    local export = call(peer, "instrument_export", { ref = epoch .. ":instrument:1", epoch = epoch, layout = layout })
    ok(export.result, json.encode(export.error or {}))
    local work = env.discovery_dir:gsub("/discovery$", "") .. "/work/"
    eq(export.result.file:sub(1, #work), work, "in Kumi's work folder")
    eq(export.result.label, "Lead")
    eq(#export.result.macros[1].mappings, 2)
    -- The host adds a mapping and asks it loaded.
    local mappings = { { param = "Cutoff", min = 0, max = 1, scaling = 3 }, { param = "Resonance", min = 0.2, max = 0.8, scaling = 3 },
      { param = "Drive", min = 0, max = 0.5, scaling = 1 } }
    local edited = host_edit(export.result.file, 1, mappings)
    local input = { ref = epoch .. ":instrument:1", macro = 1, file = edited, print = export.result.print, display = { title = "Lead macro 1: mapped to Drive" } }
    local done = call(peer, "change_apply", { kind = "instrument-macro-map", input = input, fence = {}, desc = "Kumi: map (k1)", epoch = epoch, layout = layout })
    ok(done.result, json.encode(done.error or {}))
    eq(ranges(1), "Cutoff 0-1 3, Resonance 0.2-0.8 3, Drive 0-0.5 1")
    eq(mock.data.instruments[1].name, "Lead", "its name kept")
    eq(steps(), 1, "one step")
    eq(io.exists(edited), false, "the file removed")
    eq(done.result.display.title, "Lead macro 1: mapped to Drive")
    -- Kumi's undo: the host's restore file (the mappings as they were), made from the instrument as it is now.
    local again = call(peer, "instrument_export", { at = done.result.material.at, epoch = done.result.epoch, layout = done.result.layout })
    ok(again.result, json.encode(again.error or {}))
    local restore_file = host_edit(again.result.file, 1, { mappings[1], mappings[2] })
    local material = done.result.material
    material.write = { file = restore_file, print = again.result.print, display = { title = "Lead macro 1's mappings put back" } }
    local back = call(peer, "change_restore", { kind = "instrument-macro-map", target = done.result.target, prior = {}, applied = done.result.applied,
      material = material, desc = "Kumi: undo map (k2)", epoch = done.result.epoch, layout = done.result.layout })
    ok(back.result, json.encode(back.error or {}))
    eq(ranges(1), "Cutoff 0-1 3, Resonance 0.2-0.8 3")
    eq(mock.data.instruments[1].name, "Lead")
    eq(steps(), 2)
  end)
end)

test("devices m4: Renoise's undo and redo of mappings added through the instrument's file keep the instrument's place (no delete)", function()
  local env, peer = mapped_song()
  with_instrument_files(function()
    mock.data.song_fields.selected_instrument_index = 1
    local epoch, layout = place(peer)
    local export = call(peer, "instrument_export", { ref = epoch .. ":instrument:1", epoch = epoch, layout = layout })
    local mappings = { { param = "Cutoff", min = 0, max = 1, scaling = 3 }, { param = "Resonance", min = 0.2, max = 0.8, scaling = 3 },
      { param = "Drive", min = 0, max = 0.5, scaling = 1 } }
    local input = { ref = epoch .. ":instrument:1", macro = 1, file = host_edit(export.result.file, 1, mappings), print = export.result.print, display = { title = "Lead macro 1: mapped to Drive" } }
    local done = call(peer, "change_apply", { kind = "instrument-macro-map", input = input, fence = {}, desc = "Kumi: map (k3)", epoch = epoch, layout = layout })
    ok(done.result, json.encode(done.error or {}))
    for _, step in ipairs({ "undo", "redo" }) do
      if step == "undo" then mock.song:undo() else mock.song:redo() end
      eq(ranges(1), step == "undo" and "Cutoff 0-1 3, Resonance 0.2-0.8 3" or "Cutoff 0-1 3, Resonance 0.2-0.8 3, Drive 0-0.5 1", step)
      local found = call(peer, "instrument_export", { at = done.result.material.at, epoch = done.result.epoch, layout = done.result.layout })
      ok(found.result, step .. ": the instrument is where it was: " .. json.encode(found.error or {}))
    end
  end)
end)

test("devices m4: an instrument file is loaded only from Kumi's work folder, and only when the instrument is as the host read it", function()
  local env, peer = mapped_song()
  with_instrument_files(function()
    mock.data.song_fields.selected_instrument_index = 1
    local epoch, layout = place(peer)
    local export = call(peer, "instrument_export", { ref = epoch .. ":instrument:1", epoch = epoch, layout = layout }).result
    local edited = host_edit(export.file, 1, {})
    local function apply_file(file, print_)
      return call(peer, "change_apply", { kind = "instrument-macro-map", input = { ref = epoch .. ":instrument:1", macro = 1, file = file, print = print_ },
        fence = {}, desc = "Kumi: map (k1)", epoch = epoch, layout = layout })
    end
    failure(apply_file(env.dir .. "/elsewhere.xrni", export.print), "bad-params", "work folder")
    failure(apply_file(edited:gsub("[^/]+$", "../x.xrni"), export.print), "bad-params", "work folder")
    mock.data.instruments[1].macros[1].mappings[1].max = 0.9
    failure(apply_file(edited, export.print), "changed", "read it again")
    eq(io.exists(edited), false, "the host's file removed, loaded or not")
    eq(steps(), 0)
    eq(ranges(1), "Cutoff 0-0.9 3, Resonance 0.2-0.8 3")
    -- A preview of a file change goes through Kumi's host.
    failure(call(peer, "change_preview", { kind = "instrument-macro-map", input = { ref = epoch .. ":instrument:1", macro = 1, file = edited }, epoch = epoch }), "bad-params", "host")
  end)
end)

test("devices m4: Kumi's own load of an instrument file isn't taken for a delete: a change made to the instrument before still finds it", function()
  local _, peer = mapped_song()
  with_instrument_files(function()
    mock.data.song_fields.selected_instrument_index = 1
    local epoch = place(peer)
    local first = change(peer, "instrument-macro-map", { ref = epoch .. ":instrument:1", macro = 1, set = { { mapping = 1, min = 0.1 } } })
    local e2, l2 = place(peer)
    local export = call(peer, "instrument_export", { ref = e2 .. ":instrument:1", epoch = e2, layout = l2 }).result
    local mappings = mock.data.instruments[1].macros[1].mappings
    local edited = host_edit(export.file, 2, { { param = "Drive", min = 0, max = 1, scaling = 3 } })
    local done = call(peer, "change_apply", { kind = "instrument-macro-map", input = { ref = e2 .. ":instrument:1", macro = 2, file = edited, print = export.print },
      fence = {}, desc = "Kumi: map (k2)", epoch = e2, layout = l2 })
    ok(done.result, json.encode(done.error or {}))
    eq(#mappings, 2)
    -- The first change's undo still finds the instrument (its place), and writes its range back.
    local back = restore(peer, "instrument-macro-map", first)
    ok(back.result, json.encode(back.error or {}))
    eq(ranges(1), "Cutoff 0-1 3, Resonance 0.2-0.8 3")
  end)
end)

-- Actions: a device's editor window, its settings saved as a preset

test("devices m4: a plug-in's editor window is shown and hidden on request, and a device without one is refused", function()
  local _, peer = setup({ { name = "Kick", devices = { { name = "AU: Apple: AUDelay", short_name = "AUDelay", path = "Audio/Effects/AU/aufx:dely:appl", editor = true, params = {} }, gainer() } } })
  local epoch, layout = place(peer)
  local shown = call(peer, "device_editor", { ref = epoch .. ":device:1.2", epoch = epoch, layout = layout })
  ok(shown.result and shown.result.visible == true, json.encode(shown))
  eq(device_data(1, 2).editor_visible, true)
  local hidden = call(peer, "device_editor", { ref = epoch .. ":device:1.2", show = false, epoch = epoch, layout = layout })
  ok(hidden.result and hidden.result.visible == false, json.encode(hidden))
  failure(call(peer, "device_editor", { ref = epoch .. ":device:1.3", epoch = epoch, layout = layout }), "bad-params", "no editor window")
  eq(steps(), 0, "no step of Renoise's undo")
end)

test("devices m4: a native device's settings are saved as a preset in the user library's folder for it, one of that name is replaced only when asked, and a plug-in's is refused", function()
  local env, peer = setup({ { name = "Kick", devices = { eq5(), lfo(), { name = "AU: Apple: AUDelay", short_name = "AUDelay", path = "Audio/Effects/AU/aufx:dely:appl", preset_data = "<FilterDevicePreset/>", params = {} } } } })
  local home = env.dir .. "/home"
  os.mkdir(home)
  local real_getenv = os.getenv
  os.getenv = function(name)
    if name == "HOME" then return home end
    return real_getenv(name)
  end
  local done, err = pcall(function()
    local epoch, layout = place(peer)
    local function save(d, name, replace)
      return call(peer, "preset_save", { ref = epoch .. ":device:1." .. d, name = name, replace = replace, epoch = epoch, layout = layout })
    end
    failure(save(2, "Bright"), "failed", "user library")
    os.execute("mkdir -p '" .. home .. "/Documents/Renoise/User Library'")
    local saved = save(2, "Bright")
    ok(saved.result, json.encode(saved.error or {}))
    eq(saved.result.file, home .. "/Documents/Renoise/User Library/Effect Presets/EQ 5/Bright.xrdp")
    ok(saved.result.note:find("Effect Presets/EQ 5", 1, true) and saved.result.note:find("preferences", 1, true), "where it went, said (#361 N-N8)")
    local f = assert(io.open(saved.result.file, "rb"))
    eq(f:read("*a"), mock.song:track(1):device(2).active_preset_data, "its preset data")
    f:close()
    failure(save(2, "Bright"), "bad-params", "already")
    ok(save(2, "Bright", true).result, "replaced when asked")
    -- A meta device's folder has no *; a plug-in's is its short name.
    eq(save(3, "Slow").result.file, home .. "/Documents/Renoise/User Library/Effect Presets/LFO/Slow.xrdp")
    -- A plug-in's: Renoise 3.5.4 lists only the presets it saves itself for one (lab), so Kumi doesn't write one.
    failure(save(4, "Long"), "bad-params", "plug%-in")
    failure(save(2, "../x"), "bad-params", "name")
    failure(save(2, "a:b"), "bad-params", "name")
    eq(steps(), 0)
  end)
  os.getenv = real_getenv
  if not done then error(err, 0) end
end)

-- device-params (M5's listening sets several of one device's parameters at once)

local Q1, Q2 = "\226\128\156", "\226\128\157"

-- Renoise's own undo and redo told since the last look (`undone_in_daw`).
local function told_in_daw(peer)
  call(peer, "ping")
  local out = {}
  for _, event in ipairs(peer.events or {}) do
    if event.name == "undone_in_daw" then out[#out + 1] = event.data end
  end
  peer.events = {}
  return out
end

local function eq_knobs(epoch) return { ref = epoch .. ":device:2.2", values = { { param = 1, value = -6 }, { param = "Gain 2", valueString = "3" } } } end

test("devices m4: several parameters of one device are set in one step, read back in Renoise's words, and Kumi's undo puts each back", function()
  local _, peer = setup()
  local epoch = place(peer)
  local previewed = preview(peer, "device-params", eq_knobs(epoch))
  eq(previewed.display.title, "EQ 5 2 parameters on " .. Q1 .. "Bass" .. Q2)
  eq(previewed.display.lines[2], "Gain 2 (1.00 kHz) → 3")
  eq(previewed.prior.params["1"].value, 0)
  eq(steps(), 0, "a preview writes nothing")
  local record = change(peer, "device-params", eq_knobs(epoch), "Kumi: knobs (k1)")
  eq(steps(), 1, "one step")
  eq(newest(), "Kumi: knobs (k1)")
  eq(value(2, 2, 1), -6)
  eq(value(2, 2, 2), 3)
  eq(record.applied.params["2"].valueString, "3.00", "read back in Renoise's words")
  eq(record.display.lines[1], "Gain 1 (100 Hz) → \226\136\1466.00", "Renoise's -6.00 with the true minus")
  local one = preview(peer, "device-param", { ref = epoch .. ":param:2.2.1", value = -6 })
  ok(one.display.title:find("Gain 1 (100 Hz) \226\136\1466", 1, true), one.display.title)
  eq(record.material.track, 2)
  local back = restore(peer, "device-params", record)
  ok(back.result, json.encode(back.error or {}))
  eq(value(2, 2, 1), 0)
  eq(value(2, 2, 2), 0)
  eq(steps(), 2)
  eq(back.result.display.title, "EQ 5 2 parameters on " .. Q1 .. "Bass" .. Q2 .. " put back")
  local again = restore(peer, "device-params", record)
  ok(again.result and again.result.undo.entry == false, "back already: " .. json.encode(again))
  eq(steps(), 2)
  -- Values that are as asked already: unchanged, no step.
  local same = change(peer, "device-params", { ref = place(peer) .. ":device:2.2", values = { { param = 1, value = 0 } } })
  ok(same.display.unchanged and same.undo.entry == false, json.encode(same.display))
  eq(steps(), 2)
end)

test("devices m4: device-params refuses what device-param refuses, and a parameter given twice, before anything is written", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local function asked(values, ref)
    return call(peer, "change_preview", { kind = "device-params", input = { ref = ref or (epoch .. ":device:2.2"), values = values }, epoch = epoch, layout = layout })
  end
  failure(asked({ { param = 1, value = 1 }, { param = "Gain 1", value = 2 } }), "bad-params", "given twice")
  failure(asked({}), "bad-params", "values")
  local many = {}
  for k = 1, 257 do many[k] = { param = 1 + k % 2, value = 0 } end
  failure(asked(many), "bad-params", "at most 256")
  failure(asked({ { param = 1, value = 99 } }), "bad-params", "value must be from")
  failure(asked({ { param = "Nope", value = 1 } }), "bad-params", "no parameter")
  failure(asked({ { param = 1 } }), "bad-params", "value")
  failure(asked({ { param = 1, value = 0 } }, epoch .. ":device:2.1"), "bad-params", "mixer device")
  eq(steps(), 0)
end)

test("devices m4: device-params refuses a parameter the producer moved between the preview and the apply (its fence), writing nothing", function()
  local _, peer = setup()
  local previewed, epoch, layout = preview(peer, "device-params", eq_knobs(place(peer)))
  mock.song:track(2):device(2):parameter(2).value = 5
  local before = steps()
  failure(apply(peer, "device-params", eq_knobs(epoch), previewed, "Kumi: knobs (k1)", epoch, layout), "changed", "Gain 2")
  eq(steps(), before, "nothing written")
  eq(value(2, 2, 1), 0)
  eq(value(2, 2, 2), 5)
end)

test("devices m4: device-params' undo refuses a parameter changed since, puts back with discard, and writes only those still as Kumi set them", function()
  local _, peer = setup()
  local record = change(peer, "device-params", eq_knobs(place(peer)))
  mock.song:track(2):device(2):parameter(2).value = 5
  local before = steps()
  failure(restore(peer, "device-params", record), "changed", "Gain 2 %(1.00 kHz%) of " .. Q1 .. "EQ 5 on Bass" .. Q2)
  eq(steps(), before)
  eq(value(2, 2, 1), -6, "nothing written")
  local back = restore(peer, "device-params", record, nil, { discard = true })
  ok(back.result, json.encode(back.error or {}))
  eq(value(2, 2, 1), 0)
  eq(value(2, 2, 2), 0)
  -- One back already (the producer set Gain 1 as it was), the other still as Kumi set it: only that one is written.
  local record2 = change(peer, "device-params", eq_knobs(place(peer)))
  mock.song:track(2):device(2):parameter(1).value = 0
  local back2 = restore(peer, "device-params", record2)
  ok(back2.result, json.encode(back2.error or {}))
  eq(#back2.result.display.lines, 1)
  eq(back2.result.display.lines[1], "Gain 2 (1.00 kHz) → 0.00")
  eq(value(2, 2, 2), 0)
end)

test("devices m4: device-params' undo finds the device by its place through every move since, and refuses one deleted or replaced", function()
  local disruptions = {
    { "a device made before it", function(song) song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2) end, 2, 3 },
    { "a track made before its track", function() mock.api.insert_track_at(1) end, 3, 2 },
    { "a swap with the device after it", function(song) song:track(2):swap_devices_at(2, 3) end, 2, 3 },
    { "the track before its track deleted", function() mock.api.delete_track_at(1) end, 1, 2 },
    { "its track swapped with the next", function() mock.api.swap_tracks_at(2, 3) end, 3, 2 },
  }
  for _, case in ipairs(disruptions) do
    local _, peer = setup()
    local record = change(peer, "device-params", eq_knobs(place(peer)))
    case[2](mock.song)
    H.tick(2)
    local back = restore(peer, "device-params", record)
    ok(back.result, case[1] .. ": " .. json.encode(back.error or {}))
    eq(device_data(case[3], case[4]).name, "EQ 5", case[1])
    eq(device_data(case[3], case[4]).params[1].value, 0, case[1])
    eq(device_data(case[3], case[4]).params[2].value, 0, case[1])
  end
  -- Deleted: gone; its track deleted: gone; another device made where it was after it went: gone (the log saw it go).
  local _, peer = setup()
  local record = change(peer, "device-params", eq_knobs(place(peer)))
  mock.song:track(2):delete_device_at(2)
  H.tick(2)
  failure(restore(peer, "device-params", record), "gone", "deleted")
  mock.song:track(2):insert_device_at("Audio/Effects/Native/EQ 5", 2)
  H.tick(2)
  failure(restore(peer, "device-params", record), "gone", "deleted")
  local _, peer2 = setup()
  local record2 = change(peer2, "device-params", eq_knobs(place(peer2)))
  mock.api.delete_track_at(2)
  H.tick(2)
  failure(restore(peer2, "device-params", record2), "gone")
end)

test("devices m4: device-params aims a meta device as a place, and Kumi's undo aims it back through the tracks and devices made since", function()
  local _, peer = meta_song()
  local epoch = place(peer)
  -- The LFO (aimed nowhere) to Bass's Gainer's Gain, and its amplitude, in one step.
  local input = { ref = epoch .. ":device:1.2", values = {
    { param = "Dest. Track", valueString = "2" }, { param = "Dest. Effect", value = 1 }, { param = "Dest. Parameter", valueString = "1" }, { param = "Amplitude", value = 0.8 },
  } }
  local record = change(peer, "device-params", input)
  eq(values(1, 2, 1, 4), "1/1/1/0.8")
  eq(steps(), 1)
  eq(record.display.title, "*LFO 4 parameters on " .. Q1 .. "Kick" .. Q2)
  eq(record.display.lines[5], "Dest. → " .. Q1 .. "Gain" .. Q2 .. " of " .. Q1 .. "Gainer" .. Q2 .. " on " .. Q1 .. "Bass" .. Q2)
  ok(record.material.prior.destinations["Dest. "].none, json.encode(record.material.prior))
  -- Renoise follows a device made before the Gainer and a track made before Bass (2/2/1, then 3/2/1).
  mock.song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2)
  mock.api.insert_track_at(2)
  H.tick(2)
  eq(values(1, 2, 1, 3), "2/2/1")
  local back = restore(peer, "device-params", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 1, 4), "-1/-1/-1/0.5", "aimed nowhere and its amplitude, as before")
  -- Aimed nowhere by Track -1 alone: Effect and Parameter go with it (as meta-destination does); a device can't drive itself.
  -- (Bass is track 3 now, its new Gainer device 2)
  local l = device_data(1, 2)
  l.params[1].value, l.params[2].value, l.params[3].value = 2, 1, 1
  local e2, layout2 = place(peer)
  local off = change(peer, "device-params", { ref = e2 .. ":device:1.2", values = { { param = "Dest. Track", value = -1 } } })
  eq(values(1, 2, 1, 3), "-1/-1/-1")
  local back2 = restore(peer, "device-params", off)
  ok(back2.result, json.encode(back2.error or {}))
  eq(values(1, 2, 1, 3), "2/1/1")
  local own = call(peer, "change_preview", { kind = "device-params", input = { ref = place(peer) .. ":device:1.2", values = { { param = "Dest. Track", value = 0 }, { param = "Dest. Effect", value = 1 } } }, epoch = e2, layout = layout2 })
  failure(own, "bad-params", "drive itself")
end)

test("devices m4: device-params' undo of a destination aimed elsewhere before aims it back where that device is now, and refuses it gone", function()
  local _, peer = meta_song()
  -- Before Kumi: the LFO drives Pad's EQ (device 3) Gain 2.
  local l = device_data(1, 2)
  l.params[1].value, l.params[2].value, l.params[3].value = 2, 2, 2
  local epoch = place(peer)
  local input = { ref = epoch .. ":device:1.2", values = { { param = "Dest. Track", value = 1 }, { param = "Dest. Effect", value = 1 }, { param = "Dest. Parameter", value = 1 } } }
  local record = change(peer, "device-params", input)
  eq(values(1, 2, 1, 3), "1/1/1")
  -- The producer deletes the Gainer before Pad's EQ: the EQ is device 2 now.
  mock.song:track(3):delete_device_at(2)
  H.tick(2)
  local back = restore(peer, "device-params", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(1, 2, 1, 3), "2/1/2", "the EQ where it is now")
  -- Gone: the EQ deleted after Kumi aimed the LFO elsewhere.
  local record2 = change(peer, "device-params", { ref = place(peer) .. ":device:1.2", values = { { param = "Dest. Track", value = 1 } } })
  mock.song:track(3):delete_device_at(2)
  H.tick(2)
  failure(restore(peer, "device-params", record2), "gone", "drove before")
end)

test("devices m4: device-params checks every Receiver against the send tracks, text too, and keeps it as the send track it names", function()
  local _, peer = setup({ { name = "Kick", devices = { multiband() } }, { name = "Bass" } }, 2)
  local epoch, layout = place(peer)
  local bad_text = call(peer, "change_preview", { kind = "device-params", input = { ref = epoch .. ":device:1.2", values = { { param = "Receiver 1", valueString = "5" } } }, epoch = epoch, layout = layout })
  failure(bad_text, "bad-params", "Receiver 1 is a send track's number from 0 to 1")
  local record = change(peer, "device-params", { ref = epoch .. ":device:1.2", values = { { param = "Receiver 1", valueString = "1" }, { param = "Amount 1", value = 0.5 } } })
  eq(values(1, 2, 1, 2), "0.5/1")
  ok(record.applied.params["2"].to, json.encode(record.applied))
  -- A track made before the sends: the send keeps its number among them, and the record its place.
  mock.api.insert_track_at(1)
  H.tick(2)
  local back = restore(peer, "device-params", record)
  ok(back.result, json.encode(back.error or {}))
  eq(values(2, 2, 1, 2), "1/0")
end)

test("devices m4: Renoise's own undo of device-params is told, Kumi's undo then finds it back already, and the producer's undo of Kumi's undo is a redo", function()
  local _, peer = setup()
  local record = change(peer, "device-params", eq_knobs(place(peer)), "Kumi: knobs (kab1)")
  told_in_daw(peer)
  mock.api.undo()
  H.tick(4)
  local told = told_in_daw(peer)
  eq(#told, 1)
  eq(told[1].how, "undo")
  eq(told[1].descs[1], "Kumi: knobs (kab1)")
  local back = restore(peer, "device-params", record)
  ok(back.result and back.result.undo.entry == false, json.encode(back))
  -- Again, Kumi's undo this time; then the producer takes Kumi's undo back: the change is there again, told as a redo.
  local other = { ref = place(peer) .. ":device:2.2", values = { { param = 1, value = -3 }, { param = 2, value = 2 } } }
  local record2 = change(peer, "device-params", other, "Kumi: knobs (kab2)")
  told_in_daw(peer)
  local back2 = restore(peer, "device-params", record2)
  ok(back2.result, json.encode(back2.error or {}))
  H.tick(2)
  eq(#told_in_daw(peer), 0, "Kumi's own undo isn't told")
  mock.api.undo()
  H.tick(4)
  local redone = told_in_daw(peer)
  eq(#redone, 1)
  eq(redone[1].how, "redo")
  eq(redone[1].descs[1], "Kumi: knobs (kab2)")
end)

test("devices m4: device-params refuses a destination the producer re-aimed between the preview and the apply, though only its Parameter was asked", function()
  local _, peer = meta_song()
  local l = device_data(1, 2)
  l.params[1].value, l.params[2].value, l.params[3].value = 1, 1, 1
  local input = { ref = place(peer) .. ":device:1.2", values = { { param = "Dest. Parameter", value = 2 } } }
  local previewed, epoch, layout = preview(peer, "device-params", input)
  mock.song:track(1):device(2):parameter(1).value = 2
  local before = steps()
  failure(apply(peer, "device-params", input, previewed, "Kumi: aim (k1)", epoch, layout), "changed", "Dest. Track")
  eq(steps(), before, "nothing written")
  eq(values(1, 2, 1, 3), "2/1/1")
end)

test("devices m4: a container's shape leaves out its inner devices' SelectedPresetIsModified and IsSelected, which Renoise flips by itself", function()
  -- (lab 2026-10-20: Renoise turns an inner device's SelectedPresetIsModified true when it writes the container's data again,
  -- so a made Splitter's undo refused after a splitter-set and its undo)
  local _, peer = setup()
  local data = device_data(3, 2)
  local epoch, layout = place(peer)
  local function read() return call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result end
  local first = read()
  data.preset_data = first.presetData:gsub('<GainerDevice type="GainerDevice">', '<GainerDevice type="GainerDevice">\n          <SelectedPresetIsModified>false</SelectedPresetIsModified>\n          <IsSelected>false</IsSelected>', 1)
  local flagged = read()
  data.preset_data = flagged.presetData:gsub("<SelectedPresetIsModified>false<", "<SelectedPresetIsModified>true<", 1):gsub("<IsSelected>false<", "<IsSelected>true<", 1)
  local flipped = read()
  ok(flipped.presetData ~= flagged.presetData and flipped.print ~= flagged.print, "the data differs")
  eq(flipped.shape, flagged.shape, "the shape doesn't")
  data.preset_data = flipped.presetData:gsub("<Name>Macro 1</Name>", "<Name>Drive</Name>", 1)
  ok(read().shape ~= flipped.shape, "a name still moves it")
end)

-- Device identity (kumi-51 follow-up): the M3 device kinds' undos find their device by its place, so no device made,
-- deleted or moved since on any track makes them stale.

local COMPRESSOR = "Audio/Effects/Native/Compressor"

local function names_on(t)
  local out = {}
  for d = 2, #mock.data.tracks[t].devices do out[#out + 1] = mock.data.tracks[t].devices[d].name end
  return table.concat(out, ",")
end

-- What the producer does between Kumi's change and its undo; `t`, `d` say where Bass's devices are then.
local DISRUPTIONS = {
  { "a device made before it", function(song) song:track(2):insert_device_at("Audio/Effects/Native/Delay", 2) end },
  { "the device before it deleted", function(song) song:track(2):delete_device_at(2) end },
  { "a swap of it with its neighbour", function(song) song:track(2):swap_devices_at(2, 3) end },
  { "a device made on another track", function(song) song:track(1):insert_device_at("Audio/Effects/Native/Delay", 2) end },
  { "a track made before its track", function() mock.api.insert_track_at(1) end },
  { "a device made after it", function(song) song:track(2):insert_device_at("Audio/Effects/Native/Delay", #song:track(2).devices + 1) end },
}

-- Each disruption in a fresh song: `make(peer, epoch)` makes the change (on Bass's Gainer, 2.3), `check(where)` says it was
-- undone, where `where(name)` finds a device on Bass by name now.
local function through_disruptions(kind, make, check, skip)
  for _, case in ipairs(DISRUPTIONS) do
   if case[1] ~= skip then
    local _, peer = setup()
    mock.device_catalog = { [COMPRESSOR] = { { name = "Ratio", value = 4, min = 1, max = 20 } } }
    local record = make(peer, place(peer))
    case[2](mock.song)
    H.tick(2)
    local back = restore(peer, kind, record)
    ok(back.result, kind .. ", " .. case[1] .. ": " .. json.encode(back.error or {}))
    local bass = mock.data.tracks[2].name == "Bass" and 2 or 3
    check(function(name)
      for d, data in ipairs(mock.data.tracks[bass].devices) do
        if data.name == name then return data, d, bass end
      end
    end, case[1])
    mock.device_catalog = nil
   end
  end
end

test("devices m4: Kumi's undo of a device bypass and a device parameter finds the device through every device change since", function()
  through_disruptions("device-bypass", function(peer, epoch)
    return change(peer, "device-bypass", { ref = epoch .. ":device:2.3", bypass = true })
  end, function(where, case)
    eq(where("Gainer").active ~= false, true, "bypass undone: " .. case)
  end)
  through_disruptions("device-param", function(peer, epoch)
    return change(peer, "device-param", { ref = epoch .. ":param:2.3.1", value = 2 })
  end, function(where, case)
    eq(where("Gainer").params[1].value, 1, "gain back: " .. case)
  end)
  -- Found by its place, it is still fenced by its value: the producer's gain since refuses the undo.
  local _, peer = setup()
  local record = change(peer, "device-param", { ref = place(peer) .. ":param:2.3.1", value = 2 })
  mock.song:track(2):insert_device_at("Audio/Effects/Native/Delay", 2)
  mock.song:track(2):device(4):parameter(1).value = 3
  H.tick(2)
  failure(restore(peer, "device-param", record), "changed")
  eq(device_data(2, 4).params[1].value, 3)
end)

test("devices m4: Kumi's undo of a device added finds it through every device change since, and refuses it changed", function()
  through_disruptions("device-add", function(peer, epoch)
    return change(peer, "device-add", { ref = epoch .. ":track:2", path = COMPRESSOR, index = 3 })
  end, function(where, case)
    ok(where("Compressor") == nil, "the Compressor went: " .. case)
    ok(where("Gainer") ~= nil, "the Gainer stays: " .. case)
  end)
  -- Its settings changed since: refused, unless discard.
  local _, peer = setup()
  mock.device_catalog = { [COMPRESSOR] = { { name = "Ratio", value = 4, min = 1, max = 20 } } }
  local record = change(peer, "device-add", { ref = place(peer) .. ":track:2", path = COMPRESSOR, index = 3 })
  mock.song:track(2):device(3):parameter(1).value = 8
  H.tick(2)
  failure(restore(peer, "device-add", record), "changed", "changed in Renoise")
  eq(names_on(2), "EQ 5,Compressor,Gainer")
  ok(restore(peer, "device-add", record, nil, { discard = true }).result)
  eq(names_on(2), "EQ 5,Gainer")
  mock.device_catalog = nil
end)

test("devices m4: Kumi's undo of a device deleted puts it back through every device change since, and the changes made to it before follow it", function()
  through_disruptions("device-delete", function(peer, epoch)
    return change(peer, "device-delete", { ref = epoch .. ":device:2.3" })
  end, function(where, case)
    ok(where("Gainer") ~= nil, "the Gainer is back: " .. case)
  end, "a swap of it with its neighbour")
  -- A parameter change before the delete: after Kumi puts the device back, that change's undo still finds it.
  local _, peer = setup()
  mock.device_catalog = { ["Audio/Effects/Native/Gainer"] = gainer().params }
  local param = change(peer, "device-param", { ref = place(peer) .. ":param:2.3.1", value = 2 })
  local deleted = change(peer, "device-delete", { ref = place(peer) .. ":device:2.3" })
  mock.song:track(2):insert_device_at("Audio/Effects/Native/EQ 5", 2)
  H.tick(2)
  ok(restore(peer, "device-delete", deleted).result)
  local back = restore(peer, "device-param", param)
  ok(back.result, json.encode(back.error or {}))
  eq(device_data(2, 4).name, "Gainer")
  eq(device_data(2, 4).params[1].value, 1)
  mock.device_catalog = nil
end)

test("devices m4: Kumi's undo of a device moved puts it back after the device it followed, through every change since, and refuses it moved again", function()
  local cases = {
    { "a device made before both", function(song) song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2) end, "EQ 5,Gainer,Gainer" },
    { "a device made on another track", function(song) song:track(1):insert_device_at("Audio/Effects/Native/Gainer", 2) end, "EQ 5,Gainer" },
    { "a track made before its track", function() mock.api.insert_track_at(1) end, "EQ 5,Gainer" },
    { "a device made after both", function(song) song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 4) end, "EQ 5,Gainer,Gainer" },
  }
  for _, case in ipairs(cases) do
    local _, peer = setup()
    local record = change(peer, "device-move", { ref = place(peer) .. ":device:2.2", index = 3 })
    eq(names_on(2), "Gainer,EQ 5")
    case[2](mock.song)
    H.tick(2)
    local back = restore(peer, "device-move", record)
    ok(back.result, case[1] .. ": " .. json.encode(back.error or {}))
    local bass = mock.data.tracks[2].name == "Bass" and 2 or 3
    eq(names_on(bass), case[3], case[1])
  end
  -- Moved back by the producer: back already. Moved elsewhere: refused, unless discard.
  local _, peer = setup()
  local record = change(peer, "device-move", { ref = place(peer) .. ":device:2.2", index = 3 })
  mock.song:track(2):swap_devices_at(2, 3)
  H.tick(2)
  local again = restore(peer, "device-move", record)
  ok(again.result and again.result.undo.entry == false, json.encode(again))
  local _, peer2 = setup({ { name = "Kick" }, { name = "Bass", devices = { eq5(), gainer(), plain_gainer() } } })
  local record2 = change(peer2, "device-move", { ref = place(peer2) .. ":device:2.2", index = 3 })
  eq(names_on(2), "Gainer,EQ 5,Gainer")
  mock.song:track(2):swap_devices_at(3, 4)
  H.tick(2)
  failure(restore(peer2, "device-move", record2), "changed", "moved")
  ok(restore(peer2, "device-move", record2, nil, { discard = true }).result)
  eq(names_on(2), "EQ 5,Gainer,Gainer")
end)

test("devices m4: preset data Kumi's host read from a preset file is titled by the preset's name", function()
  local _, peer = setup()
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:2.2", epoch = epoch, layout = layout }).result
  local input = { ref = epoch .. ":device:2.2", presetData = read.presetData:gsub("<Gain0>%s*<Value>[^<]*</Value>", "<Gain0>\n      <Value>-4.0</Value>", 1), presetName = "Darker" }
  local previewed = preview(peer, "device-preset-data", input)
  eq(previewed.display.title, "Loaded the preset " .. Q1 .. "Darker" .. Q2 .. " into " .. Q1 .. "EQ 5 on Bass" .. Q2)
  local record = change(peer, "device-preset-data", input)
  eq(record.display.title, previewed.display.title)
  failure(call(peer, "change_preview", { kind = "device-preset-data", input = { ref = epoch .. ":device:2.2" }, epoch = epoch, layout = layout }), "bad-params", "or path")
end)

-- Library loaders (kumi-51 follow-up): a device chain onto a track, a phrase and a modulation set into an instrument, from
-- files Kumi's host checked (here: written in the test's folder).

local CHAIN = '<?xml version="1.0" encoding="UTF-8"?>\n<RenoiseDeviceChain doc_version="21">\n  <Devices>\n    <CompressorDevice type="CompressorDevice">\n'
  .. '      <IsMaximized>true</IsMaximized>\n    </CompressorDevice>\n    <Eq10Device type="Eq10Device">\n    </Eq10Device>\n  </Devices>\n</RenoiseDeviceChain>\n'
local PHRASE = '<?xml version="1.0" encoding="UTF-8"?>\n<InstrumentPhrase doc_version="1">\n  <Name>Rock 05 (16ths)</Name>\n  <NumberOfLines>64</NumberOfLines>\n</InstrumentPhrase>\n'
local MODSET = '<?xml version="1.0" encoding="UTF-8"?>\n<SampleModulationSet doc_version="2">\n  <Devices>\n'
  .. '    <SampleAhdsrModulationDevice type="SampleAhdsrModulationDevice">\n    </SampleAhdsrModulationDevice>\n'
  .. '    <SampleLfoModulationDevice type="SampleLfoModulationDevice">\n    </SampleLfoModulationDevice>\n  </Devices>\n</SampleModulationSet>\n'

local function library_file(env, name, text)
  local path = env.dir .. "/" .. name
  local handle = assert(io.open(path, "wb"))
  handle:write(text)
  handle:close()
  return path
end

local function mixer_volume(t) return mock.data.tracks[t].devices[1].params[2].value end

test("devices m4: a device chain is loaded onto a track in one step, the track's volume and pan kept, and Kumi's undo takes it off", function()
  local env, peer = setup()
  local file = library_file(env, "Bright Vox.xrnt", CHAIN)
  mock.data.tracks[1].devices[1].params[2].value = 0.5
  mock.data.song_fields.selected_track_index = 3
  local input = { trackRef = place(peer) .. ":track:1", file = file, fileName = "Bright Vox" }
  local previewed = preview(peer, "device-chain-load", input)
  eq(previewed.display.title, "Loaded the device chain " .. Q1 .. "Bright Vox" .. Q2 .. " onto " .. Q1 .. "Kick" .. Q2 .. " (2 devices)")
  eq(steps(), 0)
  local record = change(peer, "device-chain-load", input, "Kumi: chain (k1)")
  eq(names_on(1), "Compressor,EQ 10")
  eq(steps(), 1, "one step")
  eq(mixer_volume(1), 0.5, "the volume as it was (Renoise's load resets it)")
  eq(mock.data.song_fields.selected_track_index, 3, "the selection as it was")
  eq(record.display.title, "Loaded the device chain " .. Q1 .. "Bright Vox" .. Q2 .. " onto " .. Q1 .. "Kick" .. Q2 .. " (2 devices)")
  local back = restore(peer, "device-chain-load", record)
  ok(back.result, json.encode(back.error or {}))
  eq(names_on(1), "")
  eq(mixer_volume(1), 0.5)
  eq(back.result.display.title, "Took the device chain " .. Q1 .. "Bright Vox" .. Q2 .. " off " .. Q1 .. "Kick" .. Q2)
  -- What the host didn't check: a path, a file that isn't there, a file of another kind.
  local epoch, layout = place(peer)
  local function asked(given) return call(peer, "change_preview", { kind = "device-chain-load", input = given, epoch = epoch, layout = layout }) end
  failure(asked({ trackRef = epoch .. ":track:1", path = file }), "bad-params", "host")
  failure(asked({ trackRef = epoch .. ":track:1", file = env.dir .. "/Missing.xrnt" }), "failed", "isn't there")
  failure(asked({ trackRef = epoch .. ":track:1", file = library_file(env, "Not.xrnt", PHRASE) }), "bad-params", "isn't a device chain")
end)

test("devices m4: a device chain replaces a track's devices only when asked, and Kumi's undo puts them back with their settings", function()
  local env, peer = setup()
  local file = library_file(env, "Bright Vox.xrnt", CHAIN)
  mock.device_catalog = { ["Audio/Effects/Native/Gainer"] = gainer().params, ["Audio/Effects/Native/EQ 5"] = eq5().params }
  mock.device_slots = { ["Audio/Effects/Native/Gainer"] = { slot = "GainerDevice" }, ["Audio/Effects/Native/EQ 5"] = { slot = "Eq5Device" } }
  mock.data.tracks[2].devices[3].params[1].value = 2
  local epoch, layout = place(peer)
  local refused = call(peer, "change_preview", { kind = "device-chain-load", input = { trackRef = epoch .. ":track:2", file = file }, epoch = epoch, layout = layout })
  failure(refused, "bad-params", "replace: true")
  local input = { trackRef = epoch .. ":track:2", file = file, fileName = "Bright Vox", replace = true }
  local record = change(peer, "device-chain-load", input)
  eq(names_on(2), "Compressor,EQ 10")
  ok(record.display.title:find("in place of its 2 devices", 1, true), record.display.title)
  local back = restore(peer, "device-chain-load", record)
  ok(back.result, json.encode(back.error or {}))
  eq(names_on(2), "EQ 5,Gainer")
  eq(device_data(2, 3).params[1].value, 2, "the Gainer's gain as it was")
  ok(back.result.display.title:find("put back its 2 devices", 1, true), back.result.display.title)
  mock.device_catalog, mock.device_slots = nil, nil
end)

test("devices m4: a device chain's load refuses a track whose devices changed since the preview, and its undo one whose loaded devices changed", function()
  local env, peer = setup()
  local file = library_file(env, "Bright Vox.xrnt", CHAIN)
  local input = { trackRef = place(peer) .. ":track:2", file = file, replace = true }
  local previewed, epoch, layout = preview(peer, "device-chain-load", input)
  mock.song:track(2):device(3):parameter(1).value = 3
  local before = steps()
  failure(apply(peer, "device-chain-load", input, previewed, "Kumi: chain (k1)", epoch, layout), "changed", "devices changed")
  eq(steps(), before)
  eq(names_on(2), "EQ 5,Gainer")
  -- Its undo: a loaded device changed since refuses, unless discard; nothing else counts.
  local record = change(peer, "device-chain-load", input)
  mock.song:track(2):device(2).is_active = false
  H.tick(2)
  failure(restore(peer, "device-chain-load", record), "changed", "Compressor")
  ok(restore(peer, "device-chain-load", record, nil, { discard = true }).result)
  eq(names_on(2), "EQ 5,Gainer")
end)

test("devices m4: Kumi's undo of a device chain finds its devices through the device and track changes since, and sees Renoise's own undo", function()
  for _, case in ipairs({
    { "a device made before them", function(song) song:track(1):insert_device_at("Audio/Effects/Native/Delay", 2) end, "Delay" },
    { "a swap of the two", function(song) song:track(1):swap_devices_at(2, 3) end, "" },
    { "a device made on another track", function(song) song:track(2):insert_device_at("Audio/Effects/Native/Delay", 2) end, "" },
    { "a track made before it", function() mock.api.insert_track_at(1) end, "" },
  }) do
    local env, peer = setup()
    local file = library_file(env, "Bright Vox.xrnt", CHAIN)
    local record = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:1", file = file })
    case[2](mock.song)
    H.tick(2)
    local back = restore(peer, "device-chain-load", record)
    ok(back.result, case[1] .. ": " .. json.encode(back.error or {}))
    local kick = mock.data.tracks[1].name == "Kick" and 1 or 2
    eq(names_on(kick), case[3], case[1])
  end
  local env, peer = setup()
  local record = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:2", file = library_file(env, "Bright Vox.xrnt", CHAIN), replace = true })
  mock.api.undo()
  H.tick(2)
  eq(names_on(2), "EQ 5,Gainer")
  local back = restore(peer, "device-chain-load", record)
  ok(back.result and back.result.undo.entry == false, json.encode(back))
end)

test("devices m4: a phrase file is loaded as a new phrase at its place, the selection kept, and Kumi's undo takes it out", function()
  local env, peer = setup()
  local file = library_file(env, "Rock 05.xrnz", PHRASE)
  local instrument = mock.data.instruments[1]
  local epoch = place(peer)
  local first = change(peer, "phrase-load", { ref = epoch .. ":instrument:1", file = file, fileName = "Rock 05" })
  eq(#instrument.phrases, 1)
  eq(instrument.phrases[1].name, "Rock 05 (16ths)")
  eq(instrument.phrases[1].lines, 64)
  eq(steps(), 1)
  -- Before another phrase: the one there moves up; the producer's selection follows it.
  instrument.phrases[1].name = "Mine"
  mock.data.song_fields.selected_phrase_index = 1
  local second = change(peer, "phrase-load", { ref = place(peer) .. ":instrument:1", file = file, index = 1 })
  eq(instrument.phrases[1].name, "Rock 05 (16ths)")
  eq(instrument.phrases[2].name, "Mine")
  eq(mock.data.song_fields.selected_phrase_index, 2, "the selected phrase is still Mine")
  ok(second.display.title:find("Loaded the phrase", 1, true), second.display.title)
  local back = restore(peer, "phrase-load", second)
  ok(back.result, json.encode(back.error or {}))
  eq(#instrument.phrases, 1)
  eq(instrument.phrases[1].name, "Mine")
  -- The first one's undo finds it (Mine now) and refuses it: it was renamed since.
  failure(restore(peer, "phrase-load", first), "changed")
  eq(#instrument.phrases, 1)
end)

test("devices m4: a modulation set file is loaded as a new set named after it, and Kumi's undo takes it out unless a sample uses it", function()
  local env, peer = setup()
  local file = library_file(env, "Autopaned.xrno", MODSET)
  local instrument = mock.data.instruments[1]
  local count = #instrument.sets
  local record = change(peer, "modset-load", { ref = place(peer) .. ":instrument:1", file = file, fileName = "Autopaned" })
  eq(#instrument.sets, count + 1)
  eq(instrument.sets[count + 1].name, "Autopaned")
  eq(#instrument.sets[count + 1].devices, 2)
  eq(instrument.sets[1].name, "Set 01", "the set there is untouched")
  eq(steps(), 1)
  ok(record.display.title:find("(2 devices)", 1, true), record.display.title)
  local back = restore(peer, "modset-load", record)
  ok(back.result, json.encode(back.error or {}))
  eq(#instrument.sets, count)
  -- With a name; a device taken out of it since refuses the undo, unless discard.
  local named = change(peer, "modset-load", { ref = place(peer) .. ":instrument:1", file = file, name = "Wide" })
  eq(instrument.sets[count + 1].name, "Wide")
  mock.song:instrument(1).sample_modulation_sets[count + 1]:delete_device_at(1)
  H.tick(2)
  failure(restore(peer, "modset-load", named), "changed")
  eq(#instrument.sets, count + 1)
  ok(restore(peer, "modset-load", named, nil, { discard = true }).result)
  eq(#instrument.sets, count)
  local epoch, layout = place(peer)
  failure(call(peer, "change_preview", { kind = "modset-load", input = { ref = epoch .. ":instrument:1", file = file, name = "a\nb" }, epoch = epoch, layout = layout }), "bad-params", "one line")
end)

test("devices m4: Kumi's undo of a loaded phrase or modulation set finds it through the phrases, sets and instruments made since", function()
  local cases = {
    { "phrase-load", "a phrase made before it", function() mock.song:instrument(1):insert_phrase_at(1) end },
    { "phrase-load", "an instrument made before it", function() mock.song:insert_instrument_at(1) end },
    { "modset-load", "a set made before it", function() mock.song:instrument(1):insert_sample_modulation_set_at(1) end },
    { "modset-load", "an instrument made before it", function() mock.song:insert_instrument_at(1) end },
  }
  for _, case in ipairs(cases) do
    local env, peer = setup()
    local file = case[1] == "phrase-load" and library_file(env, "Rock 05.xrnz", PHRASE) or library_file(env, "Autopaned.xrno", MODSET)
    local record = change(peer, case[1], { ref = place(peer) .. ":instrument:1", file = file, fileName = "Loaded" })
    case[3]()
    H.tick(2)
    local back = restore(peer, case[1], record)
    ok(back.result, case[2] .. ": " .. json.encode(back.error or {}))
    for _, instrument in ipairs(mock.data.instruments) do
      for _, phrase in ipairs(instrument.phrases) do ok(phrase.name ~= "Rock 05 (16ths)", case[2] .. ": the loaded phrase went") end
      for _, set in ipairs(instrument.sets) do ok(set.name ~= "Loaded", case[2] .. ": the loaded set went") end
    end
  end
end)

test("devices m4: a phrase script Kumi's host read from a library file is titled by the file's name", function()
  local _, peer = setup()
  mock.song:instrument(1):insert_phrase_at(1)
  local epoch, layout = place(peer)
  local input = { ref = epoch .. ":phrase:1.1", code = 'return cycle("c4 e4 g4")', codeName = "Arp" }
  local previewed = preview(peer, "phrase-script", input)
  ok(previewed.display.title:find("Loaded the phrase script " .. Q1 .. "Arp" .. Q2 .. " into ", 1, true), previewed.display.title)
  local record = change(peer, "phrase-script", input)
  ok(record.display.title:find("(1 line)", 1, true), record.display.title)
  failure(call(peer, "change_preview", { kind = "phrase-script", input = { ref = epoch .. ":phrase:1.1" }, epoch = epoch, layout = layout }), "bad-params", "or path")
end)

test("devices m4: changes made to devices before a chain replaced them are still undone after Kumi's undo of the chain puts them back", function()
  local env, peer = setup()
  mock.device_catalog = { ["Audio/Effects/Native/Gainer"] = gainer().params, ["Audio/Effects/Native/EQ 5"] = eq5().params }
  mock.device_slots = { ["Audio/Effects/Native/Gainer"] = { slot = "GainerDevice" }, ["Audio/Effects/Native/EQ 5"] = { slot = "Eq5Device" } }
  local param = change(peer, "device-param", { ref = place(peer) .. ":param:2.3.1", value = 2 })
  local bypass = change(peer, "device-bypass", { ref = place(peer) .. ":device:2.2", bypass = true })
  local chain = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:2", file = library_file(env, "Bright Vox.xrnt", CHAIN), replace = true })
  ok(restore(peer, "device-chain-load", chain).result)
  eq(names_on(2), "EQ 5,Gainer")
  -- A device made before them since: the earlier changes' undos still find each.
  mock.song:track(2):insert_device_at("Audio/Effects/Native/Delay", 2)
  H.tick(2)
  local back = restore(peer, "device-param", param)
  ok(back.result, json.encode(back.error or {}))
  eq(device_data(2, 4).params[1].value, 1, "the Gainer's gain as before the change")
  local back2 = restore(peer, "device-bypass", bypass)
  ok(back2.result, json.encode(back2.error or {}))
  eq(device_data(2, 3).active ~= false, true, "the EQ switched on again")
  mock.device_catalog, mock.device_slots = nil, nil
end)

test("devices m4: Renoise's own undo of a device chain is one step, told as such, and the producer's undo of Kumi's undo of it is told as a redo", function()
  local env, peer = setup()
  local file = library_file(env, "Bright Vox.xrnt", CHAIN)
  local function told()
    call(peer, "ping")
    local out = {}
    for _, event in ipairs(peer.events or {}) do
      if event.name == "undone_in_daw" then out[#out + 1] = event.data end
    end
    peer.events = {}
    return out
  end
  local record = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:2", file = file, replace = true }, "Kumi: chain (kc1)")
  eq(steps(), 1, "one step: the devices taken off and the chain loaded")
  told()
  mock.api.undo()
  H.tick(4)
  eq(names_on(2), "EQ 5,Gainer", "one undo brings back the devices it replaced")
  local events = told()
  eq(#events, 1)
  eq(events[1].how, "undo")
  eq(events[1].descs[1], "Kumi: chain (kc1)")
  ok(restore(peer, "device-chain-load", record).result.undo.entry == false, "back already")
  -- Kumi's own undo is no event; the producer's undo of it brings the chain back, told as a redo.
  local again = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:2", file = file, replace = true }, "Kumi: chain (kc2)")
  told()
  ok(restore(peer, "device-chain-load", again).result)
  H.tick(2)
  eq(#told(), 0)
  mock.api.undo()
  H.tick(4)
  eq(names_on(2), "Compressor,EQ 10")
  local redone = told()
  eq(#redone, 1)
  eq(redone[1].how, "redo")
  eq(redone[1].descs[1], "Kumi: chain (kc2)")
end)

test("devices m4: after Renoise's own undo and redo of a device chain, Kumi's undo still takes it off (the track holds exactly what it loaded)", function()
  local env, peer = setup()
  local record = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:2", file = library_file(env, "Bright Vox.xrnt", CHAIN), replace = true })
  mock.api.undo()
  H.tick(2)
  mock.api.redo()
  H.tick(2)
  eq(names_on(2), "Compressor,EQ 10")
  local back = restore(peer, "device-chain-load", record)
  ok(back.result, json.encode(back.error or {}))
  eq(names_on(2), "EQ 5,Gainer")
  -- Not when the producer changed one of them since: then the devices aren't the ones Kumi loaded, and it says so.
  local again = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:2", file = library_file(env, "Bright Vox.xrnt", CHAIN), replace = true })
  mock.api.undo()
  mock.api.redo()
  mock.song:track(2):device(2).is_active = false
  H.tick(2)
  failure(restore(peer, "device-chain-load", again), "gone")
end)

test("devices m4: device-params reads a big plug-in's parameter names once a request, not once a name (#361 N-N6)", function()
  local list = {}
  for k = 1, 400 do list[k] = { "Knob " .. k, 0.5 } end
  local big = { name = "Big Synth FX", path = "Audio/Effects/AU/aufx:big :kumi", params = params_of(list) }
  local _, peer = setup({ { name = "Kick" }, { name = "Bass", devices = { big } } })
  local epoch = place(peer)
  local values = {}
  for k = 1, 60 do values[k] = { param = "Knob " .. (k * 5), value = 0.25 } end
  mock.parameter_name_reads = 0
  local previewed = preview(peer, "device-params", { ref = epoch .. ":device:2.2", values = values })
  ok(previewed, "previewed")
  ok(mock.parameter_name_reads < 2 * 400, "names read about once each: " .. mock.parameter_name_reads)
end)

-- #361 final review: what a Doofer, a Splitter, a Notepad or a plug-in holds beyond its parameters is the producer's work too:
-- Kumi's undo of making one refuses once that changed (discard deletes it anyway).
local INNER = "<GainerDevice type=\"GainerDevice\">\n          <Volume>\n            <Value>1.0</Value>\n          </Volume>\n        </GainerDevice>"

local function deep_catalog()
  local macros = {}
  for k = 1, 8 do macros[k] = { name = "Macro " .. k, value = 50, min = 0, max = 100, element = "Macro" .. (k - 1) } end
  mock.device_catalog = { ["Audio/Effects/Native/Doofer"] = macros, ["Audio/Effects/Native/Gainer"] = gainer().params }
  mock.device_slots = { ["Audio/Effects/Native/Doofer"] = { slot = "DooferDevice", body = DOOFER_BODY }, ["Audio/Effects/Native/Gainer"] = { slot = "GainerDevice" } }
end

test("devices m4: Kumi's undo of a Doofer it added refuses once the producer put a device inside it, and discard deletes it anyway", function()
  local _, peer = setup()
  deep_catalog()
  local record = change(peer, "device-add", { ref = place(peer) .. ":track:1", path = "Audio/Effects/Native/Doofer" })
  eq(names_on(1), "Doofer")
  local data = device_data(1, 2)
  data.preset_data = mock.song:track(1):device(2).active_preset_data:gsub("</Devices>", INNER .. "\n      </Devices>", 1)
  local before = steps()
  failure(restore(peer, "device-add", record), "changed", "inside it")
  eq(names_on(1), "Doofer", "what they put inside stays")
  eq(steps(), before)
  local forced = restore(peer, "device-add", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(names_on(1), "")
  -- (one nobody touched goes as before)
  local again = change(peer, "device-add", { ref = place(peer) .. ":track:1", path = "Audio/Effects/Native/Doofer" })
  ok(restore(peer, "device-add", again).result)
  eq(names_on(1), "")
  mock.device_catalog, mock.device_slots = nil, nil
end)

test("devices m4: Kumi's undo of a device chain it loaded refuses once the producer changed what a Doofer in it holds", function()
  local env, peer = setup()
  deep_catalog()
  local text = '<?xml version="1.0" encoding="UTF-8"?>\n<RenoiseDeviceChain doc_version="21">\n  <Devices>\n    <DooferDevice type="DooferDevice">\n'
    .. '    </DooferDevice>\n  </Devices>\n</RenoiseDeviceChain>\n'
  local file = library_file(env, "Doofed.xrnt", text)
  local record = change(peer, "device-chain-load", { trackRef = place(peer) .. ":track:1", file = file, fileName = "Doofed" })
  eq(names_on(1), "Doofer")
  local data = device_data(1, 2)
  data.preset_data = mock.song:track(1):device(2).active_preset_data:gsub("</Devices>", INNER .. "\n      </Devices>", 1)
  failure(restore(peer, "device-chain-load", record), "changed", "Doofer")
  eq(names_on(1), "Doofer")
  ok(restore(peer, "device-chain-load", record, nil, { discard = true }).result)
  eq(names_on(1), "")
  mock.device_catalog, mock.device_slots = nil, nil
end)

test("devices m4: what a Doofer shows (folded, its devices shown or not, where its editor sits) is no change to what it holds: Kumi's undo of adding one goes", function()
  local _, peer = setup()
  deep_catalog()
  mock.device_slots["Audio/Effects/Native/Doofer"].body = DOOFER_BODY
    .. "\n    <ShowDevices>true</ShowDevices>\n    <PluginEditorWindowPosition>0,0</PluginEditorWindowPosition>"
  local record = change(peer, "device-add", { ref = place(peer) .. ":track:1", path = "Audio/Effects/Native/Doofer" })
  local data = device_data(1, 2)
  -- (the producer folds it, hides its devices and moves its editor: Renoise writes the view into its preset data)
  data.preset_data = mock.song:track(1):device(2).active_preset_data:gsub("<IsMaximized>true</IsMaximized>", "<IsMaximized>false</IsMaximized>", 1)
    :gsub("<ShowDevices>true</ShowDevices>", "<ShowDevices>false</ShowDevices>", 1)
    :gsub("<PluginEditorWindowPosition>0,0</PluginEditorWindowPosition>", "<PluginEditorWindowPosition>120,80</PluginEditorWindowPosition>", 1)
  local back = restore(peer, "device-add", record)
  ok(back.result, json.encode(back.error or {}))
  eq(names_on(1), "")
  mock.device_catalog, mock.device_slots = nil, nil
end)

test("devices m4: a Doofer's shape leaves out what Renoise's view shows of it, as the host's shape_of does (the same number)", function()
  local presets = T.load_payload_module("changes_presets")
  local viewed = "<a><IsMaximized>true</IsMaximized><Value>1</Value><ShowDevices>false</ShowDevices><CurrentlyVisibleChain>1</CurrentlyVisibleChain></a>"
  eq(presets.shape_of(viewed), "4d722c4cf7fe3489:67")
  local folded = viewed:gsub("<IsMaximized>true", "<IsMaximized>false"):gsub("<ShowDevices>false", "<ShowDevices>true")
  eq(presets.shape_of(folded), presets.shape_of(viewed))
end)

test("devices m4: a text's print and hash are the host's numbers, made sixteen bytes at a time as they were one at a time (perf N1)", function()
  local material = T.load_payload_module("changes_material")
  local base = T.load_payload_module("changes_base")
  eq(material.print_of("abc"), "0b885c8b3025f862:3") -- (as presets/tests.rs has them)
  eq(material.print_of(""), "0000150500000000:0")
  eq(base.hash("abc"), "0b885c8b3025f862")
  -- (the print as it was made before: a byte at a time; the hash is its first 16 digits)
  local function one_at_a_time(text)
    local a, b = 5381, 0
    for i = 1, #text do
      local c = text:byte(i)
      a = (a * 33 + c) % 4294967296
      b = (b * 65599 + c) % 4294967296
    end
    return string.format("%08x%08x:%d", a, b, #text)
  end
  local seed = 1
  local function random_text(n)
    local t = {}
    for i = 1, n do
      seed = (seed * 1103515245 + 12345) % 2147483648
      t[i] = string.char(math.floor(seed / 65536) % 256)
    end
    return table.concat(t)
  end
  for n = 0, 70 do
    local text = random_text(n)
    eq(material.print_now(text), one_at_a_time(text), n .. " bytes")
    eq(base.hash(text), one_at_a_time(text):sub(1, 16), n .. " bytes hashed")
  end
  for _, n in ipairs({ 4095, 4096, 4097, 65537 }) do
    local text = random_text(n)
    eq(material.print_now(text), one_at_a_time(text), n .. " bytes")
    eq(base.hash(text), one_at_a_time(text):sub(1, 16), n .. " bytes hashed")
  end
  local biggest = string.rep(string.char(255), 1000)
  eq(material.print_now(biggest), one_at_a_time(biggest), "every byte at its biggest")
  -- (a device's state: its parameters' values hashed in turn, as before)
  setup()
  local device = mock.song:track(2):device(2)
  local values = {}
  for k = 1, #device.parameters do values[k] = string.format("%.9g|", device:parameter(k).value) end
  ok(#values >= 2, "a device with parameters")
  eq(material.device_state(device).state, one_at_a_time(table.concat(values)):sub(1, 16))
end)

test("devices m4: a plug-in's state is hashed once a text, kept with the text's prints, as changes_base hashes it (second sweep RN2)", function()
  local material = T.load_payload_module("changes_material")
  local base = T.load_payload_module("changes_base")
  local state = "<PluginState>" .. string.rep("c3ludGggc3RhdGU=", 8192) .. "</PluginState>"
  local sums, summed = base.sums, 0
  base.sums = function(text) summed = summed + 1 return sums(text) end
  local first, again = material.hash_of(state), material.hash_of(state)
  base.sums = sums
  eq(summed, 1, "summed once")
  eq(again, first)
  eq(first, base.hash(state), "the hash a fence, a record and the journal kept before")
  eq(material.prints_of(state).hash, first, "kept with the text's prints")
end)

test("devices m4: a text's prints are made once and kept by the text, within bounds (perf N1)", function()
  local material = T.load_payload_module("changes_material")
  local presets = T.load_payload_module("changes_presets")
  local count, bytes = material.PRINTS_KEPT, material.PRINTS_BYTES
  -- (the kept prints are the module's, shared by every test here: let the earlier ones go to start from one known text)
  local function only(text_)
    material.PRINTS_KEPT = 1
    material.prints_of(text_)
  end
  local text = "<a><Value>1</Value><IsSelected>true</IsSelected><b>" .. string.rep("x", 100) .. "</b></a>"
  only("first")
  material.PRINTS_KEPT = 4
  local kept = material.prints_of(text)
  ok(kept == material.prints_of(text), "the same record for the same text")
  ok(kept.print == nil and kept.shape == nil and kept.content == nil, "nothing made before it is asked for")
  eq(material.print_of(text), kept.print)
  eq(presets.shape_of(text), kept.shape)
  eq(material.content_of(text), kept.content)
  ok(kept.print ~= kept.content and kept.content ~= kept.shape, "a print, a content and a shape")
  -- (past PRINTS_KEPT texts, the ones asked for least recently go first: "first", then the fillers; the text, asked for
  -- again just before, stays with its prints, and a filler that went gets a new, empty record)
  local filler = material.prints_of("filler 1")
  material.prints_of("filler 2")
  material.prints_of("filler 3")
  ok(material.prints_of(text) == kept, "kept while there is room")
  material.prints_of("one more")
  ok(material.prints_of(text) == kept and kept.print ~= nil, "the text asked for lately stays, prints and all")
  local again = material.prints_of("filler 1")
  ok(again ~= filler and again.print == nil, "the one asked for longest ago went")
  -- (a text bigger than PRINTS_BYTES by itself isn't kept; past PRINTS_BYTES of kept texts the least recently asked for go
  -- until the new one fits, and the rest stay)
  only("first")
  material.PRINTS_KEPT, material.PRINTS_BYTES = 1000, 1000
  local big = string.rep("b", 1001)
  ok(material.prints_of(big) ~= material.prints_of(big), "not kept")
  local a, b, c = string.rep("a", 400), string.rep("b", 400), string.rep("c", 400)
  local kept_a, kept_b = material.prints_of(a), material.prints_of(b)
  ok(material.prints_of(a) == kept_a, "asked for again: the most recent")
  material.prints_of(c)
  ok(material.prints_of(a) == kept_a, "the one asked for lately stays")
  ok(material.prints_of(b) ~= kept_b, "the one asked for longest ago went for room")
  material.PRINTS_KEPT, material.PRINTS_BYTES = count, bytes
end)

test("devices m4: a container change prints each text once, and Renoise's undo of it prints nothing printed before (perf N1)", function()
  local _, peer = setup_counting_prints()
  local epoch, layout = place(peer)
  local read = call(peer, "preset_read", { ref = epoch .. ":device:3.2", epoch = epoch, layout = layout }).result
  local edited = read.presetData:gsub("<Name>Macro 1</Name>", "<Name>Drive</Name>", 1)
  local input = { ref = epoch .. ":device:3.2", presetData = edited, shape = read.shape, display = { title = "Doofer on Pad: macro 1 named “Drive”" } }
  local before = KUMI_PRINTS
  local done = call(peer, "change_apply", { kind = "doofer-macro", input = input, fence = { id = read.id }, desc = "Kumi: macro (kp17)", epoch = epoch, layout = layout })
  ok(done.result, json.encode(done.error or {}))
  -- (the content of what it replaced, whose shape the read made already; the shape and the content of what it wrote)
  ok(KUMI_PRINTS - before <= 3, "three prints for the change, not six: " .. (KUMI_PRINTS - before))
  -- (Renoise's undo puts the text the change replaced back: its prints are kept, so the journal's check and its polls make none)
  before = KUMI_PRINTS
  mock.song:undo()
  H.tick(4)
  for _ = 1, 3 do mock.advance(1100) end
  eq(KUMI_PRINTS - before, 0, "prints made on Renoise's undo")
  KUMI_PRINTS = nil
end)
