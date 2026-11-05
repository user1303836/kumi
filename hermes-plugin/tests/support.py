"""What the plugin's tests share: the plugin loaded as Hermes loads it (a package, though its folder's name has a
hyphen), a context that records what register() does, and a runtime installed as setup installs one, whose `kumi` is
fixture_runtime.py."""
from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import stat
import sys
from typing import Any

TESTS = Path(__file__).resolve().parent
PLUGIN_DIR = TESTS.parent
PACKAGE = "kumi_hermes_plugin"
FIXTURE = TESTS / "fixture_runtime.py"
VERSION = "1.24.66"


def plugin():
    """The plugin's package (its __init__, with tools, runtime and the rest as its modules)."""
    if PACKAGE not in sys.modules:
        spec = importlib.util.spec_from_file_location(PACKAGE, PLUGIN_DIR / "__init__.py",
                                                      submodule_search_locations=[str(PLUGIN_DIR)])
        module = importlib.util.module_from_spec(spec)
        sys.modules[PACKAGE] = module
        spec.loader.exec_module(module)
    return sys.modules[PACKAGE]


def module(name: str):
    plugin()
    return importlib.import_module(f"{PACKAGE}.{name}")


class Context:
    """Hermes's PluginContext as far as the plugin may use it: registrations are recorded, settings come from a dict,
    and anything else (ctx.llm, ctx.state, …) raises as the validator's recording context does."""

    def __init__(self, settings: dict | None = None, profile_name: str = "default"):
        self.tools: dict[str, dict] = {}
        self.hooks: dict[str, list] = {}
        self.unloads: list = []
        self.commands: dict[str, Any] = {}
        self.skills: dict[str, Path] = {}
        self.prompt: dict[str, str] = {}
        self.settings = dict(settings or {})
        self.profile_name = profile_name

    def register_tool(self, name, toolset, schema, handler, check_fn=None, requires_env=None, is_async=False,
                      description="", emoji="", override=False):
        self.tools[name] = {"toolset": toolset, "schema": schema, "handler": handler, "check_fn": check_fn}

    def register_hook(self, hook_name, callback, *, hold_stream=False):
        self.hooks.setdefault(hook_name, []).append(callback)

    def on_unload(self, callback):
        self.unloads.append(callback)

    def register_command(self, name, handler, description="", args_hint="", argument_mode=None):
        self.commands[name] = handler

    def register_skill(self, name, path, description="", frontmatter=None):
        if not Path(path).is_file():
            raise FileNotFoundError(f"SKILL.md not found at {path}")
        self.skills[name] = Path(path)

    def register_system_prompt_section(self, id, content, *, position="after_memory", max_chars=4000):
        if len(content) > max_chars:
            raise ValueError(f"system prompt section {id} is over {max_chars} characters")
        self.prompt[id] = content

    def get_config(self, key: str, default: Any = None) -> Any:
        return self.settings.get(key, default)

    def set_config(self, key: str, value: Any) -> None:
        self.settings[key] = value

    def __getattr__(self, name: str):
        raise AttributeError(f"the plugin used ctx.{name}, which register() and the tools never need")


def install(data_dir: Path, *fixture_args: str, version: str = VERSION, target: str | None = None) -> Path:
    """A runtime set up in `data_dir` as setup leaves it (runtime/<version>/kumi and runtime/current.json), whose
    `kumi` runs the fixture runtime with `fixture_args` after the runtime's own arguments. Returns `kumi`."""
    host_target = module("setup").host_target()
    folder = data_dir / "runtime" / version
    folder.mkdir(parents=True, exist_ok=True)
    extra = " ".join(f'"{arg}"' for arg in fixture_args)
    if os.name == "nt":
        kumi = folder / "kumi.bat"
        kumi.write_text(f'@"{sys.executable}" "{FIXTURE}" %* {extra}\r\n', encoding="utf-8")
    else:
        kumi = folder / "kumi"
        kumi.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FIXTURE}" "$@" {extra}\n', encoding="utf-8")
        kumi.chmod(kumi.stat().st_mode | stat.S_IXUSR)
    receipt = {"schema": 1, "version": version, "target": target or host_target,
               "kumi": kumi.relative_to(data_dir).as_posix(), "sha256": "0" * 64, "installed_at": 0}
    (data_dir / "runtime" / "current.json").write_text(json.dumps(receipt), encoding="utf-8")
    return kumi


def requests(state_dir: Path) -> list[dict]:
    """Every message the fixture runtime received, in order."""
    path = state_dir / "fixture-requests.jsonl"
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()] if path.exists() else []


def host(test, settings: dict | None = None, *fixture_args: str, installed: bool = True, hook_timeout: float = 30.0):
    """A plugin host as the tools see it, on a fresh plugin-data folder with the fixture runtime set up (unless
    `installed` is false). Returns (host, context, data folder); everything is cleaned up after `test`."""
    import tempfile
    folder = tempfile.TemporaryDirectory()
    test.addCleanup(folder.cleanup)
    data = Path(folder.name)
    if installed:
        install(data, *fixture_args)
    ctx = Context(settings if settings is not None else {"daw": "ableton"})
    runtimes = module("runtime").Runtimes(environ=lambda: {"PATH": "/usr/bin:/bin"}, start_timeout=10)
    test.addCleanup(runtimes.stop_all)
    return (module("tools").Host(ctx, runtimes, data_dir=lambda: data, pinned=lambda: VERSION,
                                 hook_timeout=lambda: hook_timeout), ctx, data)


_IDS = iter(range(1, 1 << 30))


def call(host, name: str, args=None, *, answer: str = "once", **context) -> dict:
    """A tool called as Hermes calls it: Kumi's pre_tool_call hook with a fresh tool-call id (as Hermes gives a hook),
    the producer giving `answer` when the hook asks, then the handler with the session and task only. A blocked or
    denied call comes back as {"blocked": message}."""
    policy = module("policy")
    ident = f"call-{next(_IDS)}"
    hook = {"session_id": context.get("session_id"), "task_id": context.get("task_id"), "tool_call_id": ident,
            "turn_id": "u1", "api_request_id": "a1", "middleware_trace": []}
    decision = policy.pre_tool_call(host, tool_name=name, args=args if isinstance(args, dict) else {}, **hook)
    if decision is not None and decision["action"] == "block":
        return {"blocked": decision["message"]}
    if decision is not None:
        policy.post_approval_response(host, tool_call_id=ident, choice=answer)
        if answer not in policy.APPROVED:
            return {"blocked": decision["message"], "answer": answer}
    return handle(host, name, args, **context)


def handle(host, name: str, args=None, **context) -> dict:
    """A tool's handler called as Hermes calls it, its JSON answer read back (no hook: for a call that skipped it)."""
    tools = module("tools")
    function = next(function for tool, _, function, _ in tools.TOOLS if tool == name)
    return json.loads(tools.handler(name, function, host)(args, **context))
