"""scripts/models-catalog.py: the providers the table names, only the fields Kumi reads, in an order that diffs well;
the committed snapshot is what the script makes of itself. No network: models.dev's answer is a small made-up one.
"""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("models_catalog", ROOT / "scripts/models-catalog.py")
catalog = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = catalog
SPEC.loader.exec_module(catalog)

ANSWER = {
    "openai": {
        "id": "openai", "name": "OpenAI", "npm": "@ai-sdk/openai", "env": ["OPENAI_API_KEY"], "doc": "https://platform.openai.com",
        "models": {
            "gpt-6-luna": {
                "id": "gpt-6-luna", "name": "GPT-6 Luna", "description": "made up", "family": "gpt", "tool_call": True,
                "reasoning": True, "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "high"]}],
                "modalities": {"input": ["text", "image"], "output": ["text"]},
                "limit": {"context": 400000, "input": 272000, "output": 128000},
                "cost": {"input": 1.25, "output": 10, "cache_read": 0.125, "tiers": [{"input": 2.5}]},
                "experimental": {"modes": {"fast": {}}}, "open_weights": False,
            },
            "text-embedding-4": {"id": "text-embedding-4", "name": "Embedding", "type": "embedding", "tool_call": False},
        },
    },
    "opencode": {
        "id": "opencode", "npm": "@ai-sdk/openai-compatible", "api": "https://opencode.ai/zen/v1", "env": ["OPENCODE_API_KEY"],
        "models": {"gemini-9": {"name": "Gemini 9", "provider": {"npm": "@ai-sdk/google", "api": "https://example.test", "body": {}}}},
    },
    "elsewhere": {"id": "elsewhere", "models": {"x": {"name": "X"}}},
}


class ModelsCatalog(unittest.TestCase):
    def test_the_providers_are_the_ones_the_tables_rows_name(self):
        table = 'catalog: Some("openai"),\n    catalog: None,\n  catalog: Some("opencode"),\n catalog: Some("openai"),'
        self.assertEqual(catalog.named_providers(table), ["openai", "opencode"])

    def test_only_what_kumi_reads_is_kept(self):
        snapshot = catalog.cut(ANSWER, ["openai", "opencode"])
        self.assertEqual(sorted(snapshot), ["openai", "opencode"])
        self.assertEqual(snapshot["openai"]["env"], ["OPENAI_API_KEY"])
        self.assertNotIn("doc", snapshot["openai"])
        self.assertEqual(snapshot["openai"]["models"]["gpt-6-luna"], {
            "name": "GPT-6 Luna", "tool_call": True, "reasoning": True,
            "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "high"]}],
            "modalities": {"input": ["text", "image"], "output": ["text"]}, "limit": {"context": 400000, "output": 128000},
            "cost": {"input": 1.25, "output": 10, "cache_read": 0.125},
        })
        self.assertEqual(snapshot["openai"]["models"]["text-embedding-4"], {"name": "Embedding", "type": "embedding", "tool_call": False})
        self.assertEqual(snapshot["opencode"]["models"]["gemini-9"]["provider"], {"npm": "@ai-sdk/google", "api": "https://example.test"})

    def test_a_provider_models_dev_lacks_stops_the_snapshot(self):
        with self.assertRaises(SystemExit):
            catalog.cut(ANSWER, ["nowhere"])

    def test_the_text_is_the_same_whatever_order_the_answer_came_in(self):
        reordered = json.loads(json.dumps(ANSWER), object_pairs_hook=lambda pairs: dict(reversed(pairs)))
        self.assertEqual(catalog.text(catalog.cut(ANSWER, ["openai"])), catalog.text(catalog.cut(reordered, ["openai"])))
        self.assertTrue(catalog.text({}).endswith("\n"))

    def test_the_committed_snapshot_is_what_the_script_makes_of_it(self):
        committed = catalog.SNAPSHOT.read_text(encoding="utf-8")
        names = catalog.named_providers(catalog.TABLE.read_text(encoding="utf-8"))
        self.assertEqual(sorted(json.loads(committed)), names, "the snapshot holds the providers the table names")
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "api.json"
            source.write_text(committed, encoding="utf-8")
            out = Path(folder) / "out.json"
            catalog.main(["--from", str(source), "--out", str(out)])
            self.assertEqual(out.read_text(encoding="utf-8"), committed)


if __name__ == "__main__":
    unittest.main()
