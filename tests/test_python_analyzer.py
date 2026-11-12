"""Real Jedi inference and AST span QA. Never execute analyzed project modules."""
import ast
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / "skills/ai-env/scripts/analyzers/python.py"
SPEC = importlib.util.spec_from_file_location("aienv_python_analyzer", ADAPTER)
BACKEND = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BACKEND)


class PythonAnalyzerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aienv-python-test-")
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def request(self, texts, **options):
        return dict(schema_version=1, language="python", root=str(self.root), options=options,
                    files=[dict(path=p, text=t, hash=hashlib.sha256(t.encode()).hexdigest()) for p, t in sorted(texts.items())])

    def analyze(self, texts, **options):
        graph = BACKEND.analyze(self.request(texts, **options))
        self.assertEqual(graph["analyzer"]["name"], "jedi+ast")
        self.assertNotEqual(graph["analyzer"]["version"], "unavailable")
        self.assertFalse(graph["analysis_complete"], "Python runtime coverage must always be explicitly partial")
        ids = [n["id"] for n in graph["nodes"]]
        self.assertEqual(len(ids), len(set(ids)))
        for edge in graph["edges"]:
            self.assertIn(edge["source"], ids)
            self.assertIn(edge["target"], ids)
            self.assertIn(edge["path"], texts)
        for node in graph["nodes"]:
            if not node["external"]:
                texts[node["path"]].encode()[node["start_byte"]:node["end_byte"]].decode()
        # analyze() has already removed its snapshot. Provenance may contain
        # runtime/dependency files only, never temporary imported project files.
        for item in graph["inputs"]:
            self.assertTrue(Path(item["path"]).is_file(), item["path"])
            self.assertEqual(hashlib.sha256(Path(item["path"]).read_bytes()).hexdigest(), item["hash"])
        return graph

    def named(self, graph, qualified):
        return next(n for n in graph["nodes"] if n["qualified_name"] == qualified)

    def relation(self, graph, source, target, kind):
        a, b = self.named(graph, source)["id"], self.named(graph, target)["id"]
        return any(e["source"] == a and e["target"] == b and e["kind"] == kind for e in graph["edges"])

    def fixture(self):
        return {"model.py": "class Money:\n    value: int\n\nclass Base:\n    def price(self, amount: Money) -> Money:\n        return amount\n\nclass Store(Base):\n    saved: list[Money]\n    def __init__(self, amount: Money):\n        self.balance: Money = amount\n",
                "decoy.py": "def price(value): return value\n",
                "app.py": "from model import Money as Cash, Store\nimport decoy\n\ndef run(store: Store, amount: Cash) -> Cash:\n    return store.price(amount)\n"}

    def test_crossmodule_receiver_alias_fields_generics_and_inheritance(self):
        graph = self.analyze(self.fixture())
        self.assertFalse(graph["diagnostics"], graph["diagnostics"])
        self.assertTrue(self.relation(graph, "app.run", "model.Base.price", "calls"))
        self.assertFalse(self.relation(graph, "app.run", "decoy.price", "calls"))
        self.assertTrue(self.relation(graph, "app.Cash", "model.Money", "aliases"))
        self.assertTrue(self.relation(graph, "app.decoy", "decoy", "aliases"))
        self.assertTrue(self.relation(graph, "app.run", "model.Money", "uses_type"))
        self.assertTrue(self.relation(graph, "model.Store", "model.Base", "inherits"))
        self.assertTrue(self.relation(graph, "model.Store.saved", "model.Money", "uses_type"))
        self.assertTrue(self.relation(graph, "model.Store.saved", "builtins.list", "uses_type"))
        self.assertTrue(self.relation(graph, "model.Store.balance", "model.Money", "uses_type"))

    def test_authoritative_unsaved_crossmodule_source_overrides_disk(self):
        (self.root / "target.py").write_text("def old(): return 1\nraise RuntimeError('must not execute')\n")
        texts = {"target.py": "def fresh(): return 2\n", "app.py": "from target import fresh\ndef run(): return fresh()\n"}
        graph = self.analyze(texts)
        self.assertTrue(self.relation(graph, "app.run", "target.fresh", "calls"))
        self.assertFalse(any(n["name"] == "old" for n in graph["nodes"]))

    def test_utf8_crlf_declaration_body_is_exact(self):
        text = "# 雪 😀 Ω\r\ndef Métier(value: int) -> int:\r\n    return value + 1\r\n"
        graph = self.analyze({"nested/café.py": text})
        node = self.named(graph, "nested.café.Métier")
        self.assertEqual(text.encode()[node["start_byte"]:node["end_byte"]].decode(), "def Métier(value: int) -> int:\r\n    return value + 1")
        self.assertEqual(node["line"], 2)
        self.assertGreater(node["start_byte"], text.index("def"))

    def test_unknown_callback_and_branch_ambiguity_never_guess(self):
        graph = self.analyze({"app.py": "def one(): return 1\ndef two(): return 2\ndef run(callback, flag):\n    callback()\n    choice = one if flag else two\n    return choice()\n"})
        calls = [e for e in graph["edges"] if e["kind"] == "calls"]
        self.assertFalse(calls, calls)
        unresolved = [u for u in graph["unresolved"] if u["kind"] == "calls"]
        self.assertEqual({u["line"] for u in unresolved}, {4, 6})

    def test_project_initializers_and_module_code_are_never_executed(self):
        marker = self.root / "executed.marker"
        texts = {"pkg/__init__.py": "from .impl import actual\n", "pkg/impl.py": f"from pathlib import Path\nPath({str(marker)!r}).write_text('executed')\ndef actual(): return 3\n", "app.py": "from pkg import actual\ndef run(): return actual()\n", "setup.py": f"open({str(marker)!r}, 'w').write('setup executed')\n"}
        graph = self.analyze(texts)
        self.assertTrue(self.relation(graph, "app.run", "pkg.impl.actual", "calls"))
        self.assertFalse(marker.exists())

    def test_syntax_errors_missing_import_and_quoted_annotations_are_explicit(self):
        graph = self.analyze({"bad.py": "def broken(:\n", "app.py": "from missing import Absent\ndef run(x: 'Absent'):\n    return x.unknown()\n"})
        self.assertTrue(any(d["severity"] == "error" and d["path"] == "bad.py" for d in graph["diagnostics"]))
        self.assertTrue(any("Import target" in u["reason"] for u in graph["unresolved"]))
        self.assertTrue(any("Quoted forward" in u["reason"] for u in graph["unresolved"]))
        self.assertTrue(any(u["kind"] == "calls" for u in graph["unresolved"]))

    def test_stable_id_after_function_body_change_and_recursive_call(self):
        first = self.analyze({"app.py": "def run(n: int):\n    return run(n - 1) if n else 0\n"})
        second = self.analyze({"app.py": "def run(n: int):\n    value = n + 1\n    return run(value - 1) if value else 0\n"})
        self.assertEqual(self.named(first, "app.run")["id"], self.named(second, "app.run")["id"])
        self.assertTrue(self.relation(first, "app.run", "app.run", "calls"))
        self.assertEqual(self.named(first, "app.run")["signature"], self.named(second, "app.run")["signature"])

    def test_default_initializer_calls_belong_to_enclosing_module(self):
        graph = self.analyze({"app.py": "def initialize(): return 1\ndef run(value=initialize()):\n    return value\n"})
        self.assertTrue(self.relation(graph, "app", "app.initialize", "calls"))
        self.assertFalse(self.relation(graph, "app.run", "app.initialize", "calls"))

    def test_decorators_remain_in_contract_and_exact_body_with_partial_warning(self):
        text = "def decorator(fn): return fn\n\n@decorator\ndef run():\n    return 1\n"
        graph = self.analyze({"app.py": text})
        node = self.named(graph, "app.run")
        self.assertEqual(node["signature"], "@decorator def run()")
        self.assertEqual(text.encode()[node["start_byte"]:node["end_byte"]].decode(), "@decorator\ndef run():\n    return 1")
        self.assertEqual(node["line"], 3)
        self.assertTrue(any("Decorator may replace" in u["reason"] for u in graph["unresolved"]))

    def test_selected_file_keeps_import_targets_without_analyzing_other_bodies(self):
        texts = {"app.py": "from target import actual\ndef run(): return actual()\n", "target.py": "def actual(): return missing()\n"}
        graph = self.analyze(texts, files=["app.py"])
        self.assertTrue(self.relation(graph, "app.run", "target.actual", "calls"))
        self.assertFalse(any(u["path"] == "target.py" for u in graph["unresolved"]))
        missing = self.analyze(texts, files=["absent.py"])
        self.assertTrue(any("not supplied" in d["message"] for d in missing["diagnostics"]))

    def test_assignment_alias_and_changed_receiver_select_different_real_methods(self):
        texts = {"types.py": "class Money: pass\nAmount = Money\nclass First:\n    def price(self): return 1\nclass Second:\n    def price(self): return 2\n",
                 "app.py": "from types import First, Second\ndef run():\n    store = First()\n    return store.price()\n"}
        # Avoid a stdlib module name: project files are an isolated static tree.
        texts["model.py"] = texts.pop("types.py")
        texts["app.py"] = texts["app.py"].replace("from types", "from model")
        first = self.analyze(texts)
        self.assertTrue(self.relation(first, "model.Amount", "model.Money", "aliases"))
        self.assertEqual(self.named(first, "model.Amount")["kind"], "alias")
        self.assertTrue(self.relation(first, "app.run", "model.First.price", "calls"))
        texts["app.py"] = texts["app.py"].replace("store = First()", "store = Second()")
        second = self.analyze(texts)
        self.assertTrue(self.relation(second, "app.run", "model.Second.price", "calls"))
        self.assertFalse(self.relation(second, "app.run", "model.First.price", "calls"))

    def test_input_provenance_points_to_existing_hashed_runtime_dependencies(self):
        graph = self.analyze({"app.py": "def run(value: int): return len(str(value))\n"})
        self.assertTrue(any(self.named(graph, "app.run")["id"] == e["source"] and e["kind"] == "calls" for e in graph["edges"]))
        names = {Path(i["path"]).name for i in graph["inputs"]}
        self.assertTrue(any("python" in n for n in names), names)
        self.assertIn("__init__.py", names)
        for item in graph["inputs"]:
            self.assertTrue(Path(item["path"]).is_absolute())
            self.assertEqual(hashlib.sha256(Path(item["path"]).read_bytes()).hexdigest(), item["hash"])

    @unittest.skipUnless(hasattr(ast, "TypeAlias"), "PEP 695 requires Python 3.12+")
    def test_pep695_type_alias_preserves_source_contract(self):
        text = "class Money: pass\ntype Amount = Money\n"
        graph = self.analyze({"model.py": text})
        self.assertTrue(self.relation(graph, "model.Amount", "model.Money", "aliases"))
        node = self.named(graph, "model.Amount")
        self.assertEqual(text.encode()[node["start_byte"]:node["end_byte"]].decode(), "type Amount = Money")

    def test_unavailable_vendor_invalid_hash_and_bad_path_fail_closed(self):
        with patch.object(BACKEND, "VENDOR", self.root / "absent"):
            with self.assertRaisesRegex(ValueError, "Jedi/Parso missing"):
                BACKEND.analyze(self.request({"a.py": "pass\n"}))
        bad = self.request({"a.py": "pass\n"})
        bad["files"][0]["hash"] = "bad"
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            BACKEND.analyze(bad)
        with self.assertRaisesRegex(ValueError, "Noncanonical"):
            BACKEND.analyze(self.request({"../escape.py": "pass\n"}))

    def test_json_subprocess_protocol_and_deterministic_results(self):
        request = self.request(self.fixture())
        result = subprocess.run([sys.executable, str(ADAPTER)], input=json.dumps(request), text=True, capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        graph = json.loads(result.stdout)
        again = self.analyze(self.fixture())
        self.assertEqual(graph["nodes"], again["nodes"])
        self.assertEqual(graph["edges"], again["edges"])


if __name__ == "__main__":
    unittest.main()
