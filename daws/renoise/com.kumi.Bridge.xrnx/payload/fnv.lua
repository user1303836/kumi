-- 32-bit FNV-1a: the fingerprint the digest (digest.lua) and the scratch print (changes_scratch.lua) are made of. Only this
-- tool compares it, with itself, and it stays the same from one version to the next: Kumi's host keeps a song's digest to
-- compare with the next session's, and the reads vectors (daw-mcp-server/tests/vectors) hold the tool's answers.
--
-- Renoise runs plain Lua 5.1, where a call into the bit library costs more than the arithmetic around it, and the digest
-- feeds a text for every note column of every line it reads (scratch's print every device's preset data too). A byte's
-- step makes one such call (the xor); the multiply by 16777619 = 2^24 + 403 is done in doubles, (h mod 256) × 2^24 +
-- h × 403, exact under 2^53 and the same modulo 2^32; the bytes come sixteen to a string.byte call, the tail in one more.
-- Twice the speed of a step of five calls (Lua 5.1.5 with LuaBitOp: 3.6 ms, then 1.7, over 2,000 texts of 20 bytes;
-- 95 ms, then 44, over a megabyte).

local bit = require("bit")
local bxor, tobit, tohex = bit.bxor, bit.tobit, bit.tohex
local byte = string.byte
local M32 = 4294967296

local M = {}

-- The sum to start from (the FNV offset basis), as an unsigned number: `feed` and `mark` take and give one.
M.OFFSET = 2166136261

-- `h` with one byte `c` taken in (the digest ends each text it feeds with 31).
function M.mark(h, c)
  h = bxor(h, c)
  return ((h % 256) * 16777216 + h * 403) % M32
end

-- `h` with the bytes of `text` taken in.
function M.feed(h, text)
  local n, i = #text, 1
  while i + 15 <= n do
    local c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15, c16 = byte(text, i, i + 15)
    h = bxor(h, c1); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c2); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c3); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c4); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c5); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c6); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c7); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c8); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c9); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c10); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c11); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c12); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c13); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c14); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c15); h = ((h % 256) * 16777216 + h * 403) % M32
    h = bxor(h, c16); h = ((h % 256) * 16777216 + h * 403) % M32
    i = i + 16
  end
  -- (the last fifteen or fewer: each line takes one byte in and stops when there is no next)
  if i <= n then
    local c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15 = byte(text, i, n)
    repeat
      h = bxor(h, c1); h = ((h % 256) * 16777216 + h * 403) % M32; if not c2 then break end
      h = bxor(h, c2); h = ((h % 256) * 16777216 + h * 403) % M32; if not c3 then break end
      h = bxor(h, c3); h = ((h % 256) * 16777216 + h * 403) % M32; if not c4 then break end
      h = bxor(h, c4); h = ((h % 256) * 16777216 + h * 403) % M32; if not c5 then break end
      h = bxor(h, c5); h = ((h % 256) * 16777216 + h * 403) % M32; if not c6 then break end
      h = bxor(h, c6); h = ((h % 256) * 16777216 + h * 403) % M32; if not c7 then break end
      h = bxor(h, c7); h = ((h % 256) * 16777216 + h * 403) % M32; if not c8 then break end
      h = bxor(h, c8); h = ((h % 256) * 16777216 + h * 403) % M32; if not c9 then break end
      h = bxor(h, c9); h = ((h % 256) * 16777216 + h * 403) % M32; if not c10 then break end
      h = bxor(h, c10); h = ((h % 256) * 16777216 + h * 403) % M32; if not c11 then break end
      h = bxor(h, c11); h = ((h % 256) * 16777216 + h * 403) % M32; if not c12 then break end
      h = bxor(h, c12); h = ((h % 256) * 16777216 + h * 403) % M32; if not c13 then break end
      h = bxor(h, c13); h = ((h % 256) * 16777216 + h * 403) % M32; if not c14 then break end
      h = bxor(h, c14); h = ((h % 256) * 16777216 + h * 403) % M32; if not c15 then break end
      h = bxor(h, c15); h = ((h % 256) * 16777216 + h * 403) % M32
    until true
  end
  return h
end

-- The sum as 8 hex digits (bit.tohex of it, as the prints always were).
function M.hex(h)
  return tohex(tobit(h))
end

return M
