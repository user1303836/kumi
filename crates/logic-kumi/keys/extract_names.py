#!/usr/bin/env python3
"""The English names of Logic Pro's key commands, by their ids, read out of the installed Logic.

Logic keeps its key command ids and English names together only in `Logic.framework`'s binary: static tables of
40-byte records `{id, name (a CFString), 0, handler, index}` in its data segments, and the same records built on the
stack by the code that registers the commands (`strh w, [sp, #o]` for the id, `str x, [sp, #o + 8]` for the name).
Nothing in the bundle's resources maps one to the other; the `.lproj` tables translate the names, and the key command
presets and the producer's preferences bind the ids to keys. This script reads both kinds of table from the arm64
slice and writes `src/keys/names.json`, which the bridge's resolver (`src/keys.rs`) is built with. A new Logic is a
new run: the ids are stable across versions, the names move with the menus.

    python3 crates/logic-kumi/keys/extract_names.py ["/Applications/Logic Pro.app"] [--out crates/logic-kumi/src/keys/names.json]

Only the arm64 slice is read (the tables are the same in both). Names with a line break or a format marker are
dialog strings that share a stack slot, not commands, and are left out. The U.S. factory preset's bindings go to
`src/keys/us-bindings.json` beside the names, spelled as the bridge's `Chord::parse` reads them (`cmd-t`,
`opt-'`, `shift-return`, a virtual key code as a number): the preset's `LogicBinaryPreferences` blob ends with 5000
six-byte records indexed by id, `key, modifier, 0, charcode (two bytes), flags`.
"""
import argparse
import json
import struct
import sys
from pathlib import Path

ARM64 = 0x0100000C
ID_LIMIT = 10000


def arm64_slice(data: bytes) -> bytes:
    """The arm64 slice of a fat binary, or the binary itself."""
    if data[:4] == b"\xca\xfe\xba\xbe":
        count = struct.unpack_from(">I", data, 4)[0]
        for n in range(count):
            cputype, _subtype, offset, size, _align = struct.unpack_from(">IIIII", data, 8 + n * 20)
            if cputype == ARM64:
                return data[offset:offset + size]
        raise SystemExit("the binary has no arm64 slice")
    return data


class MachO:
    def __init__(self, data: bytes):
        self.data = data
        magic, _cpu, _sub, _filetype, ncmds, _sizeofcmds, _flags, _reserved = struct.unpack_from("<IIIIIIII", data, 0)
        if magic != 0xFEEDFACF:
            raise SystemExit("not a 64-bit Mach-O")
        self.segments = []
        off = 32
        for _ in range(ncmds):
            cmd, cmdsize = struct.unpack_from("<II", data, off)
            if cmd == 0x19:
                segname = data[off + 8:off + 24].rstrip(b"\0").decode()
                vmaddr, vmsize, fileoff, _filesize, _maxprot, _initprot, nsects, _flags = struct.unpack_from("<QQQQiiII", data, off + 24)
                sections = []
                so = off + 72
                for _ in range(nsects):
                    sectname = data[so:so + 16].rstrip(b"\0").decode()
                    addr, size, soff = struct.unpack_from("<QQI", data, so + 32)
                    sections.append((sectname, addr, size, soff))
                    so += 80
                self.segments.append((segname, vmaddr, vmsize, fileoff, sections))
            off += cmdsize

    def section(self, segname: str, sectname: str):
        for name, _vmaddr, _vmsize, _fileoff, sections in self.segments:
            if name == segname:
                for sname, addr, size, soff in sections:
                    if sname == sectname:
                        return addr, size, soff
        raise SystemExit(f"no {segname},{sectname} section")

    def file_offset(self, vm: int):
        for _name, vmaddr, vmsize, fileoff, _sections in self.segments:
            if vmaddr <= vm < vmaddr + vmsize:
                return fileoff + (vm - vmaddr)
        return None


def cfstrings(macho: MachO) -> dict:
    """Every constant CFString, by its address: the text it holds."""
    cs_addr, cs_size, _ = macho.section("__TEXT", "__cstring")
    cf_addr, cf_size, cf_off = macho.section("__DATA_CONST", "__cfstring")
    out = {}
    for n in range(cf_size // 32):
        _isa, _flags, ptr, _length = struct.unpack_from("<QQQQ", macho.data, cf_off + n * 32)
        ptr &= 0xFFFFFFFFF  # a chained fixup: the target in the low 36 bits
        if cs_addr <= ptr < cs_addr + cs_size:
            start = macho.file_offset(ptr)
            end = macho.data.find(b"\0", start)
            try:
                out[cf_addr + n * 32] = macho.data[start:end].decode("utf-8")
            except UnicodeDecodeError:
                pass
    return out


def data_tables(macho: MachO, cf: dict) -> dict:
    """The static tables: `{id, name, 0, handler, index}` records in the data segments."""
    text_addr, text_size, _ = macho.section("__TEXT", "__text")
    names = {}
    for segname, _vmaddr, _vmsize, _fileoff, sections in macho.segments:
        if segname not in ("__DATA_CONST", "__DATA"):
            continue
        for _sname, _addr, size, soff in sections:
            for fo in range(soff, soff + size - 39, 8):
                cid, name, zero, fn, index = struct.unpack_from("<QQQQQ", macho.data, fo)
                if not 0 < cid < ID_LIMIT or zero != 0 or index > 4096:
                    continue
                name &= 0xFFFFFFFFF
                fn &= 0xFFFFFFFFF
                if name in cf and text_addr <= fn < text_addr + text_size:
                    names.setdefault(int(cid), cf[name])
    return names


def registrations(macho: MachO, cf: dict) -> dict:
    """The records the registration code builds on the stack: a light tracker of the registers and stack slots."""
    text_addr, text_size, text_off = macho.section("__TEXT", "__text")
    count = text_size // 4
    ins = struct.unpack_from("<" + "I" * count, macho.data, text_off)
    regs = {}
    slots = {}
    names = {}

    def store(slot, kind, value):
        slots[slot] = (kind, value)
        other = slots.get(slot + 8 if kind == "id" else slot - 8)
        if kind == "id" and other and other[0] == "cf":
            names.setdefault(value, cf[other[1]])
        elif kind == "cf" and other and other[0] == "id":
            names.setdefault(other[1], cf[value])

    for k in range(count):
        i = ins[k]
        rd, rn = i & 31, (i >> 5) & 31
        if i == 0xD65F03C0:  # ret: another function
            regs.clear()
            slots.clear()
        elif (i & 0x9F000000) == 0x90000000:  # adrp
            immlo, immhi = (i >> 29) & 3, (i >> 5) & 0x7FFFF
            imm = (immhi << 2) | immlo
            if imm & (1 << 20):
                imm -= 1 << 21
            regs[rd] = ("page", ((text_addr + k * 4) & ~0xFFF) + (imm << 12))
        elif (i & 0xFF800000) == 0x91000000:  # add xd, xn, #imm
            imm = (i >> 10) & 0xFFF
            if (i >> 22) & 1:
                imm <<= 12
            src = regs.get(rn)
            if rn == 31:
                regs[rd] = ("sp", imm)
            elif src and src[0] == "page":
                regs[rd] = ("cf", src[1] + imm) if src[1] + imm in cf else ("other", 0)
            elif src and src[0] == "sp":
                regs[rd] = ("sp", src[1] + imm)
            else:
                regs.pop(rd, None)
        elif (i & 0x7F800000) == 0x52800000 and ((i >> 21) & 3) == 0:  # movz w/x, #imm16
            regs[rd] = ("imm", (i >> 5) & 0xFFFF)
        elif (i & 0x7FE0FFE0) == 0x2A0003E0:  # mov wd, wm
            src = regs.get((i >> 16) & 31)
            if src:
                regs[rd] = src
            else:
                regs.pop(rd, None)
        elif (i & 0xFFC00000) in (0x79000000, 0xF9000000):  # strh / str x, [base, #imm]
            base = 0 if rn == 31 else (regs.get(rn)[1] if regs.get(rn, ("",))[0] == "sp" else None)
            if base is None:
                continue
            scale = 2 if (i & 0xFFC00000) == 0x79000000 else 8
            value = regs.get(rd)
            slot = base + ((i >> 10) & 0xFFF) * scale
            if scale == 2 and value and value[0] == "imm" and 0 < value[1] < ID_LIMIT:
                store(slot, "id", value[1])
            elif scale == 8 and value and value[0] == "cf":
                store(slot, "cf", value[1])
        elif (i & 0x3B000000) == 0x39000000 or (i & 0x1F000000) in (0x0B000000, 0x11000000, 0x1B000000, 0x0A000000, 0x12000000, 0x13000000):
            regs.pop(rd, None)  # a load or an ALU op wrote the register: its value is unknown now
    return names


PRESET = "Contents/Resources/Key Commands/U.S..logikcs"
PRESET_IDS = 5000
NAMED_KEYS = {
    0xF700: "up", 0xF701: "down", 0xF702: "left", 0xF703: "right", 0xF728: "forwarddelete", 0xF729: "home", 0xF72B: "end",
    0xF72C: "pageup", 0xF72D: "pagedown", 0xF746: "help", 127: "delete", 8: "delete", 13: "return", 3: "enter", 9: "tab",
    27: "escape", 32: "space",
}


def spelled(key: int, modifier: int, flags: int, char_code: int):
    """A record as the bridge's `Chord::parse` reads it, or None when the helper can't post the key."""
    if flags & 1:
        name = str(key)
    elif char_code in NAMED_KEYS:
        name = NAMED_KEYS[char_code]
    elif 0xF704 <= char_code <= 0xF717:
        name = f"f{char_code - 0xF704 + 1}"
    elif 33 <= char_code <= 126:
        name = chr(char_code).lower()
    else:
        return None
    mods = [name_ for bit, name_ in ((32, "cmd"), (1, "shift"), (8, "opt"), (4, "ctrl")) if modifier & bit]
    return "-".join(mods + [name])


def preset_bindings(preset: Path) -> dict:
    """The factory preset's bindings by id: its blob's last 5000 records."""
    import plistlib
    blob = plistlib.load(open(preset, "rb"))["LogicBinaryPreferences"]
    base = len(blob) - PRESET_IDS * 6
    out = {}
    for cid in range(PRESET_IDS):
        record = blob[base + cid * 6:base + cid * 6 + 6]
        if not any(record):
            continue
        chord = spelled(record[0], record[1], record[5], record[3] | (record[4] << 8))
        if chord:
            out[str(cid)] = chord
    return out


def command_like(name: str) -> bool:
    return bool(name) and "\n" not in name and "%" not in name and len(name) <= 120


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("bundle", nargs="?", default="/Applications/Logic Pro.app")
    parser.add_argument("--out", default=str(Path(__file__).resolve().parents[1] / "src/keys/names.json"))
    args = parser.parse_args(argv)
    bundle = Path(args.bundle)
    binary = bundle / "Contents/Frameworks/Logic.framework/Versions/A/Logic"
    if not binary.is_file():
        raise SystemExit(f"{binary} isn't there")
    version = ""
    info = bundle / "Contents/Info.plist"
    try:
        import plistlib
        version = plistlib.load(open(info, "rb")).get("CFBundleShortVersionString", "")
    except Exception:
        pass
    macho = MachO(arm64_slice(binary.read_bytes()))
    cf = cfstrings(macho)
    names = data_tables(macho, cf)
    for cid, name in registrations(macho, cf).items():
        names.setdefault(cid, name)
    names = {cid: name for cid, name in names.items() if command_like(name)}
    out = {"logic": version, "commands": {str(cid): names[cid] for cid in sorted(names)}}
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(names)} key command names from Logic Pro {version} written to {args.out}", file=sys.stderr)
    preset = bundle / PRESET
    if preset.is_file():
        bindings = preset_bindings(preset)
        bindings_out = Path(args.out).with_name("us-bindings.json")
        bindings_out.write_text(json.dumps({"logic": version, "preset": preset.name, "bindings": bindings}, indent=1) + "\n")
        print(f"{len(bindings)} factory bindings written to {bindings_out}", file=sys.stderr)


if __name__ == "__main__":
    main()
