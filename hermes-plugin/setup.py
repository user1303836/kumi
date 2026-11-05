"""Kumi's runtime for the Hermes plugin: which release it pins, and setup (C8). Owned by W5. Stdlib only and
import-safe: plugin_api.py loads this file by path, so it imports no sibling module."""
from __future__ import annotations

import json
from pathlib import Path
import platform
import re

PIN_FILE = "kumi_release.json"
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
_ARCH = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}
_OS = {"darwin": "apple-darwin", "windows": "pc-windows-msvc", "linux": "unknown-linux-gnu"}


def host_target(system: str | None = None, machine: str | None = None) -> str | None:
    """This machine's Rust target triple, as Kumi's release names its bundles, or None where Kumi has no build."""
    arch = _ARCH.get((machine or platform.machine()).lower())
    family = _OS.get((system or platform.system()).lower())
    return f"{arch}-{family}" if arch and family else None


def pinned_version(plugin_dir: Path | None = None) -> str | None:
    """The Kumi version kumi_release.json pins, or None when there is no valid pin file."""
    try:
        pin = json.loads(((plugin_dir or Path(__file__).resolve().parent) / PIN_FILE).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    version = pin.get("kumi") if isinstance(pin, dict) and pin.get("schema") == 1 else None
    return version if isinstance(version, str) and VERSION.fullmatch(version) else None
