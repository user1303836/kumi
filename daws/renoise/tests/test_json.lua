local T = ...
local json = T.load_payload_module("json")
local test, eq, ok, same = T.test, T.eq, T.ok, T.same

local function enc(v, o) return assert(json.encode(v, o)) end

test("json: scalars", function()
  eq(enc(true), "true")
  eq(enc(false), "false")
  eq(enc(json.null), "null")
  eq(enc(nil), "null")
  eq(enc(""), '""')
end)

test("json: integers print as integers", function()
  eq(enc(0), "0")
  eq(enc(-0.0), "0")
  eq(enc(42), "42")
  eq(enc(-7), "-7")
  eq(enc(2 ^ 53), "9007199254740992")
  eq(enc(-(2 ^ 53)), "-9007199254740992")
  eq(enc(1e15), "1000000000000000")
end)

test("json: fractions read back exactly in at most 17 digits", function()
  eq(enc(0.1), "0.1")
  eq(enc(1 / 3), "0.3333333333333333")
  eq(enc(0.1 + 0.2), "0.30000000000000004")
  eq(enc(123.456), "123.456")
  eq(enc(1.7976931348623157e308), "1.7976931348623157e+308")
  for _, v in ipairs({ 5e-324, 0.1, 1 / 3, math.pi, -2.5e-8, 6.02214076e23, 2 ^ 60 + 0.0, 1e21, 123456789.123456789 }) do
    eq(tonumber(enc(v)), v, "round trip of " .. tostring(v))
  end
end)

test("json: NaN and infinities are null", function()
  eq(enc(0 / 0), "null")
  eq(enc(math.huge), "null")
  eq(enc(-math.huge), "null")
end)

test("json: escapes quotes, backslashes and every control character", function()
  eq(enc('a"b\\c'), '"a\\"b\\\\c"')
  eq(enc("\n\r\t\b\f"), '"\\n\\r\\t\\b\\f"')
  eq(enc("\0\1\31"), '"\\u0000\\u0001\\u001f"')
  eq(enc("\127/"), '"\127/"')
  for i = 0, 31 do
    local text = enc(string.char(i))
    ok(not text:find("[%z\1-\31]"), "control " .. i .. " left raw")
    eq(json.decode(text), string.char(i))
  end
end)

test("json: valid UTF-8 passes through unchanged", function()
  local s = "é ü 日本 😀 \239\191\189"
  eq(enc(s), '"' .. s .. '"')
end)

test("json: invalid UTF-8 is repaired with U+FFFD per maximal subpart", function()
  local R = "\239\191\189"
  eq(json.repair_utf8("\255"), R)
  eq(json.repair_utf8("a\192\175b"), "a" .. R .. R .. "b")
  eq(json.repair_utf8("\237\160\128"), R .. R .. R, "surrogate")
  eq(json.repair_utf8("\244\144\128\128"), R .. R .. R .. R, "above U+10FFFF")
  eq(json.repair_utf8("\226\130"), R, "truncated")
  eq(json.repair_utf8("\226\130x"), R .. "x", "truncated then ASCII")
  eq(json.repair_utf8("\224\128\128"), R .. R .. R, "overlong")
  eq(json.repair_utf8("\240\159\152"), R, "truncated 4-byte")
  ok(json.valid_utf8(json.decode(enc("\255\254 ok \192"))))
end)

test("json: arrays, objects and explicit empty arrays", function()
  eq(enc({ 1, 2, 3 }), "[1,2,3]")
  eq(enc({}), "{}")
  eq(enc(json.array()), "[]")
  eq(enc(json.object()), "{}")
  eq(enc({ a = json.array(), b = {} }, { sort_keys = true }), '{"a":[],"b":{}}')
  eq(enc({ b = 1, a = { c = "x" } }, { sort_keys = true }), '{"a":{"c":"x"},"b":1}')
end)

test("json: refuses tables JSON can't hold", function()
  local _, err = json.encode({ 1, nil, 3 })
  ok(err, "holes")
  _, err = json.encode({ 1, x = 2 })
  ok(err and err:find("mixes"), "mixed")
  _, err = json.encode({ [2] = "a" })
  ok(err, "sparse")
  _, err = json.encode({ [true] = 1 })
  ok(err, "boolean key")
  _, err = json.encode(function() end)
  ok(err and err:find("function"), "function")
  local cycle = {}
  cycle.self = cycle
  _, err = json.encode(cycle)
  ok(err and err:find("deeply"), "cycle")
end)

test("json: decodes the JSON grammar", function()
  same(json.decode('{"a":[1,2.5,-3e2,true,false,null],"b":{"c":"d"}}'), { a = { 1, 2.5, -300, true, false, json.null }, b = { c = "d" } })
  eq(json.decode("  42  "), 42)
  eq(json.decode('"\\u00e9\\ud83d\\ude00\\/"'), "é😀/")
  eq(json.decode('"\\ud83d\\ude00"'), "\240\159\152\128", "a pair")
  eq(json.decode("[]").n, nil)
  ok(json.is_array(json.decode("[]")))
  eq(enc(json.decode("[]")), "[]", "an empty array stays an array")
  eq(json.decode("null"), json.null)
  eq(json.decode("[null]", { null_as_nil = true })[1], nil)
end)

test("json: rejects what isn't JSON", function()
  for _, bad in ipairs({ '"\\ud800x"', '"\\ud800"', '"\\udc00"', '"\\ud800\\u0041"', "1e400", "-1e999", "", " ", "[1,]", "{\"a\":1,}", "[01]", "01", "1.", ".5", "-", "+1", "1e", "[1 2]",
    "{a:1}", "{'a':1}", "\"a", "\"a\1\"", "\"\\x\"", "\"\\u12\"", "tru", "nul", "[1] x", "\"\255\"",
    "NaN", "Infinity", "[", "{", "{\"a\"}", "{\"a\" 1}" }) do
    local value, err = json.decode(bad)
    eq(value, nil, "decoded " .. string.format("%q", bad))
    ok(type(err) == "string", "an error for " .. string.format("%q", bad))
  end
end)

test("json: nesting is bounded at the wire's limit, on the way in and out", function()
  eq(json.MAX_DEPTH, 64)
  local function brackets(levels) return string.rep("[", levels) .. string.rep("]", levels) end
  ok(json.decode(brackets(64)), "64 levels")
  local value, err = json.decode(brackets(65))
  eq(value, nil)
  ok(err:find("deeply"), "65 levels")
  value, err = json.decode(brackets(300))
  eq(value, nil)
  ok(err:find("deeply"), "300 levels")
  -- A scalar at the bottom is not a level of its own.
  ok(json.decode(string.rep("[", 64) .. "1" .. string.rep("]", 64)), "64 levels around a number")
  ok(not json.decode(string.rep("{\"a\":", 65) .. "1" .. string.rep("}", 65)), "65 objects around a number")
  local function nested(levels)
    local t = json.array({})
    for _ = 2, levels do t = json.array({ t }) end
    return t
  end
  eq(enc(nested(64)), brackets(64), "64 levels encode")
  local text, enc_err = json.encode(nested(65))
  eq(text, nil)
  ok(enc_err:find("deeply"), "65 levels don't")
end)

test("json: the sliced encoder gives the same text in many small steps", function()
  local big = {}
  for i = 1, 20000 do big[i] = { i, "item " .. i, i / 7, { flag = i % 2 == 0 } } end
  local whole = enc(big, { sort_keys = true })
  local job = json.sliced_encoder(big, { sort_keys = true })
  local done, text
  repeat done, text = job:step(0) until done ~= false
  eq(done, true)
  ok(job.slices > 10, "it took slices")
  eq(text, whole)
end)

test("json: the sliced encoder reports errors", function()
  local job = json.sliced_encoder({ 1, x = 2 })
  local done, err = job:step(10)
  eq(done, nil)
  ok(err:find("mixes"))
end)

test("json: records how a top-level number was written", function()
  local numbers = {}
  local value = json.decode('{"a":1,"b":1.0,"c":-0,"d":1E2,"e":"x","f":[1.5],"g":null,"h":7,"h":"again"}', { numbers = numbers })
  eq(value.a, 1)
  same(numbers, { a = "1", b = "1.0", c = "-0", d = "1E2" }, "text of the numbers, the last of a repeated name")
  numbers = {}
  json.decode('[1,2]', { numbers = numbers })
  eq(next(numbers), nil, "only an object's own members")
end)

test("json: a list with a hole is refused, marked as a list or not", function()
  for _, list in ipairs({ { 1, nil, 3 }, json.array({ 1, nil, 3 }), json.array({ [1] = "a", [3] = "c" }) }) do
    local text, err = json.encode(list)
    eq(text, nil)
    ok(err:find("holes") or err:find("mixes"), tostring(err))
  end
  eq(enc(json.array({ 1, 2, 3 })), "[1,2,3]")
  eq(enc(json.array()), "[]")
end)

test("json: a row encoded on its own stops ROW_DEPTH levels short of MAX_DEPTH, so a raw text never makes a frame too deep (perf N4)", function()
  eq(json.ROW_DEPTH, json.MAX_DEPTH - 8)
  local function nested(levels) local v = 1; for _ = 1, levels do v = { v } end; return v end
  ok(json.encode(nested(json.ROW_DEPTH), { max_depth = json.ROW_DEPTH }) ~= nil, "a row at the row limit encodes")
  eq(json.encode(nested(json.ROW_DEPTH + 1), { max_depth = json.ROW_DEPTH }), nil, "one deeper is refused, and goes as a table")
  ok(json.encode(nested(json.ROW_DEPTH + 1)) ~= nil, "which the document's own encoder still takes within MAX_DEPTH")
end)

test("json: a raw text goes into a document as it is, and a document of raws is the bytes of the document of their values (perf N4)", function()
  eq(json.encode(json.array({ json.raw('{"x":1}') })), '[{"x":1}]')
  eq(json.encode({ r = json.raw("[1, 2]") }), '{"r":[1, 2]}', "verbatim, spaces and all")
  eq(json.encode(json.raw("null")), "null")
  -- (a page's rows: the same row tables, once encoded as they are and once as raws of their own texts, give one text)
  local rows = {}
  for i = 1, 50 do
    rows[i] = { line = i, notes = json.array({ { col = 1, note = "C-4", instrument = "00" }, { col = 2, note = "E-5", volume = "40" } }), effects = json.array({ { col = 1, number = "0G", amount = "10" } }) }
  end
  local doc = { items = json.array(rows), epoch = "1a2b", nextCursor = "1a2b.1.51" }
  local plain = json.encode(doc)
  local raws = {}
  for i, row in ipairs(rows) do raws[i] = json.raw(json.encode(row)) end
  doc.items = json.array(raws)
  local spliced = json.encode(doc)
  eq(spliced, plain, "the same bytes")
  ok(json.is_raw(raws[1]) and not json.is_raw(rows[1]) and not json.is_array(raws[1]))
  -- (the sliced encoder takes them too)
  local sliced_job = json.sliced_encoder(doc)
  local done, text
  repeat done, text = sliced_job:step(1000) until done ~= false
  eq(done, true)
  eq(text, spliced)
end)
