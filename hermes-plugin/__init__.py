"""Kumi for Hermes: Kumi's work in a DAW (Ableton Live, REAPER, Renoise) as a Hermes plugin (RESEARCH.md §3).

register(ctx) only wires: it does no I/O, starts no process and reads nothing of Hermes's. Every tool and hook is
registered every time (catalog rule 6); a tool that can't work yet says why when it's called.
"""
from __future__ import annotations

from . import runtime, tools


def register(ctx) -> None:
    runtimes = runtime.Runtimes(events=tools.broadcast)
    host = tools.Host(ctx, runtimes)
    for name, schema, function, emoji in tools.TOOLS:
        ctx.register_tool(name=name, toolset=tools.TOOLSET, schema=schema, handler=tools.handler(function, host),
                          emoji=emoji)
    ctx.register_hook("on_session_finalize", host.on_session_finalize)
    ctx.on_unload(runtimes.stop_all)
