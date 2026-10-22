-- (#401) Scripts kept for a chore: a file in Kumi's bridge folder, a Tools menu entry and a shortcut that run it, registered
-- again when the bridge starts; a run is the file as it is now, in one callback; removing one takes all three away.
local T = ...
local mock = T.mock
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness
local K = assert(loadfile(T.tests_dir .. "change_kit.lua"))(T)
local json = T.load_payload_module("json")

local ENTRY, BINDING = "Main Menu:Tools:Kumi:Hats - tidy", "Global:Kumi:Hats - tidy"

test("chores: a script kept is a menu entry and a shortcut running its file, again after a reload, and removed whole", function()
  local env, peer = K.setup()
  local asked = { slug = "tidy-hats", name = "Hats: tidy", description = "Names the hat track", code = "song:track(2).name = 'Hat (kept)'" }
  local kept = K.call(peer, "script_install", asked)
  ok(kept.result, json.encode(kept.error or {}))
  local path = env.dir .. "/kumi/scripts/tidy-hats.lua"
  eq(kept.result.file, path, "in Kumi's bridge folder, beside its discovery folder")
  eq(kept.result.menu, "Tools > Kumi > Hats - tidy", "Renoise splits a menu's names at ':'")
  ok(kept.result.where:find("Tools > Kumi > Hats - tidy in Renoise's menu", 1, true), kept.result.where)
  ok(mock.menu_entries[ENTRY] and mock.keybindings[BINDING], "in the Tools menu and the Keys preferences")
  eq(K.steps(), 0, "kept, not a change of the song")
  -- The menu runs the file (with `song`, as run_lua gave it), a held shortcut once.
  mock.menu_entries[ENTRY].invoke()
  eq(mock.song:track(2).name, "Hat (kept)")
  mock.song:track(2).name = "Hat"
  mock.keybindings[BINDING].invoke(true)
  eq(mock.song:track(2).name, "Hat", "a repeat runs nothing")
  mock.keybindings[BINDING].invoke(false)
  eq(mock.song:track(2).name, "Hat (kept)")
  -- Kept already: written over only when asked; a script that stops says so on the status bar, never in a dialog.
  eq(K.call(peer, "script_install", asked).error.code, "failed")
  local broken = { slug = "tidy-hats", name = "Hats: tidy", code = "error('no hats')", replace = true }
  ok(K.call(peer, "script_install", broken).result)
  mock.menu_entries[ENTRY].invoke()
  ok(mock.statuses[#mock.statuses]:find("Kumi: Hats: tidy: stopped: .*no hats"), mock.statuses[#mock.statuses])
  -- A run is one step of Renoise's undo, named for it; what it prints goes to the status bar.
  local printing = { slug = "tidy-hats", name = "Hats: tidy", code = "song:track(2).name = 'Hat 2' print('renamed', 1)", replace = true }
  ok(K.call(peer, "script_install", printing).result)
  mock.menu_entries[ENTRY].invoke()
  eq(K.newest(), "Kumi: Hats: tidy")
  eq(mock.statuses[#mock.statuses], "Kumi: Hats: tidy: renamed\t1")
  -- One that runs on is stopped past its time, and says so.
  local forever = { slug = "tidy-hats", name = "Hats: tidy", code = "while true do end", replace = true }
  ok(K.call(peer, "script_install", forever).result)
  mock.clock_step_ms = 10
  mock.menu_entries[ENTRY].invoke()
  mock.clock_step_ms = 0
  ok(mock.statuses[#mock.statuses]:find("Kumi stopped it: it ran longer than 5 s", 1, true), mock.statuses[#mock.statuses])
  -- A reload registers it again, by the new payload.
  local before = mock.menu_entries[ENTRY].invoke
  eq(K.call(peer, "renoise.reload").result.reloading, true)
  H.tick()
  ok(mock.menu_entries[ENTRY] and mock.menu_entries[ENTRY].invoke ~= before, "registered again")
  -- Removed: its entry, shortcut and file; a second time, gone.
  ok(K.call(peer, "script_remove", { slug = "tidy-hats" }).result)
  ok(not mock.menu_entries[ENTRY] and not mock.keybindings[BINDING] and not io.exists(path), "all three gone")
  eq(K.call(peer, "script_remove", { slug = "tidy-hats" }).error.code, "gone")
  for _, wrong in ipairs({
    { slug = "../x", name = "x", code = "return 1" },
    { slug = "x", name = "two\nlines", code = "return 1" },
    { slug = "x", name = "x", code = "this isn't Lua" },
  }) do
    eq(K.call(peer, "script_install", wrong).error.code, "bad-params", json.encode(wrong))
  end
  H.no_dialogs()
end)
