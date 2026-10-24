#!/usr/bin/env python3
"""Writes crates/kumi-ai/data/models-catalog.json, the models.dev catalog Kumi carries (https://models.dev, MIT): its
providers that a row of Kumi's table names (`catalog: Some("…")` in crates/kumi-ai/src/providers/table.rs), with only the
fields Kumi reads, keys sorted, so a refresh reads as a small diff. Kumi refreshes it at run time the same way.

    python3 scripts/models-catalog.py                 # fetches models.dev
    python3 scripts/models-catalog.py --from api.json # from a saved copy
"""
import argparse
import json
from pathlib import Path
import re
import sys
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
TABLE = ROOT / "crates/kumi-ai/src/providers/table.rs"
SNAPSHOT = ROOT / "crates/kumi-ai/data/models-catalog.json"
SOURCE = "https://models.dev/api.json"
# What Kumi reads of a provider, and of each model; a nested field keeps only the keys listed.
PROVIDER_FIELDS = ("api", "env", "npm")
MODEL_FIELDS = {
    "name": None,
    "tool_call": None,
    "reasoning": None,
    "reasoning_options": None,
    "modalities": ("input", "output"),
    "limit": ("context", "output"),
    "cost": ("input", "output", "cache_read", "cache_write"),
    "type": None,
    "provider": ("npm", "api"),
    "status": None,
    "release_date": None,
    "structured_output": None,
    "attachment": None,
}


def named_providers(table: str) -> list[str]:
    """The models.dev providers the table's rows name, each once, sorted."""
    return sorted(set(re.findall(r'catalog:\s*Some\("([^"]+)"\)', table)))


def cut_model(model: dict) -> dict:
    kept = {}
    for field, keys in MODEL_FIELDS.items():
        value = model.get(field)
        if value is None:
            continue
        if keys is not None:
            if not isinstance(value, dict):
                continue
            value = {key: value[key] for key in keys if value.get(key) is not None}
            if not value:
                continue
        kept[field] = value
    return kept


def cut(catalog: dict, providers: list[str]) -> dict:
    """`catalog` (models.dev's api.json) with only `providers`, and of them only what Kumi reads."""
    snapshot = {}
    for name in providers:
        provider = catalog.get(name)
        if not isinstance(provider, dict):
            raise SystemExit(f"models.dev has no provider {name}, which crates/kumi-ai/src/providers/table.rs names")
        kept = {field: provider[field] for field in PROVIDER_FIELDS if provider.get(field) is not None}
        kept["models"] = {model_id: cut_model(model) for model_id, model in (provider.get("models") or {}).items() if isinstance(model, dict)}
        snapshot[name] = kept
    return snapshot


def text(snapshot: dict) -> str:
    return json.dumps(snapshot, sort_keys=True, indent=1, ensure_ascii=False) + "\n"


def fetch(url: str) -> dict:
    request = urllib.request.Request(url, headers={"user-agent": "kumi models-catalog.py", "accept": "application/json"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--from", dest="source", help="read models.dev's api.json from this file instead of fetching it")
    parser.add_argument("--out", default=str(SNAPSHOT), help="where the snapshot goes")
    args = parser.parse_args(argv)
    catalog = json.loads(Path(args.source).read_text(encoding="utf-8")) if args.source else fetch(SOURCE)
    snapshot = cut(catalog, named_providers(TABLE.read_text(encoding="utf-8")))
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text(snapshot), encoding="utf-8")
    models = sum(len(provider["models"]) for provider in snapshot.values())
    print(f"{out}: {len(snapshot)} providers, {models} models, {out.stat().st_size:,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
