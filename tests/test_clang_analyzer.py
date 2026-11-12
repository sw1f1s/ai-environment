"""Real libclang regressions; no mock semantic graph and no target build."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ADAPTER = Path(__file__).resolve().parents[1] / "skills/ai-env/scripts/analyzers/clang.py"
SPEC = importlib.util.spec_from_file_location("aienv_clang_analyzer", ADAPTER)
BACKEND = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BACKEND)


class ClangAnalyzerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.clang = BACKEND.Clang()
        except (ValueError, OSError) as error:
            raise unittest.SkipTest(str(error))

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="ae-clang-test-")
        self.root = Path(self.temporary.name).resolve()

    def tearDown(self):
        self.temporary.cleanup()

    def request(self, sources, language="cpp", options=None, disk_sources=None):
        for path, source in (disk_sources or sources).items():
            target = self.root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(source.encode())
        return {"schema_version": 1, "language": language, "root": str(self.root),
                "files": [{"path": path, "text": source, "hash": hashlib.sha256(source.encode()).hexdigest()}
                          for path, source in sources.items()],
                "options": {"flags": ["-std=c11" if language == "c" else "-std=c++20"], **(options or {})}}

    def run_graph(self, *args, **kwargs):
        result = BACKEND.Graph(self.request(*args, **kwargs), self.clang).analyze()
        ids = {node["id"] for node in result["nodes"]}
        self.assertEqual(len(ids), len(result["nodes"]))
        for edge in result["edges"]:
            self.assertIn(edge["source"], ids)
            self.assertIn(edge["target"], ids)
        return result

    def nodes(self, graph, qualified):
        return [node for node in graph["nodes"] if node["qualified_name"] == qualified]

    def test_overloads_namespace_resolution_and_cross_tu_definition_merge(self):
        sources = {
            "include/api.hpp": "#pragma once\nnamespace w { struct Money { int cents; }; using Amount=Money; int refund(int,int); Money refund(Money,Money); }\nnamespace decoy { int refund(int,int); }\n",
            "src/refund.cpp": '#include "../include/api.hpp"\nnamespace w { int refund(int s,int a) { return s+a; } Money refund(Money s,Money a) { return {s.cents+a.cents}; } }\n',
            "src/app.cpp": '#include "../include/api.hpp"\nint run() { return w::refund(1,2) + w::refund(w::Money{1},w::Money{2}).cents; }\n'}
        graph = self.run_graph(sources)
        self.assertTrue(graph["analysis_complete"], graph["diagnostics"] + graph["unresolved"])
        refunds = self.nodes(graph, "w::refund")
        self.assertEqual(len(refunds), 2)
        self.assertTrue(all(n["definition"] and n["path"] == "src/refund.cpp" for n in refunds))
        run = self.nodes(graph, "run")[0]
        targets = {e["target"] for e in graph["edges"] if e["kind"] == "calls" and e["source"] == run["id"]}
        self.assertEqual(targets, {n["id"] for n in refunds})
        alias, money = self.nodes(graph, "w::Amount")[0], self.nodes(graph, "w::Money")[0]
        self.assertTrue(any(e["source"] == alias["id"] and e["target"] == money["id"] and e["kind"] == "aliases" for e in graph["edges"]))

    def test_c_typedef_function_and_pointer_field_type_edges(self):
        sources = {"api.h": "typedef unsigned Count; typedef struct Ledger { Count value; struct Ledger *next; } Ledger; Count bump(Count);\n",
                   "bump.c": '#include "api.h"\nCount bump(Count n) { return n+1; }\n',
                   "app.c": '#include "api.h"\nCount process(Ledger *p) { return bump(p->value); }\n'}
        graph = self.run_graph(sources, language="c")
        self.assertTrue(graph["analysis_complete"], graph["diagnostics"])
        bump, process = self.nodes(graph, "bump")[0], self.nodes(graph, "process")[0]
        self.assertTrue(bump["definition"])
        self.assertTrue(any(e["source"] == process["id"] and e["target"] == bump["id"] and e["kind"] == "calls" for e in graph["edges"]))
        target_names = {n["id"]: n["name"] for n in graph["nodes"]}
        used = {target_names[e["target"]] for e in graph["edges"] if e["source"] == process["id"] and e["kind"] == "uses_type"}
        self.assertIn("Ledger", used)
        self.assertIn("Count", used)

    def test_template_implementation_impact_reaches_concrete_specialization_callers(self):
        graph = self.run_graph({"template.cpp": '''template<class T> T identity(T value) { return value; }
int numbers() { return identity(1); }
double fractions() { return identity(1.5); }
int unrelated() { return 42; }
'''})
        self.assertTrue(graph["analysis_complete"], graph["diagnostics"] + graph["unresolved"])
        templates = self.nodes(graph, "identity")
        primary = next(n for n in templates if "@FT@" in n["id"])
        specializations = {n["id"] for n in templates if n["id"] != primary["id"]}
        self.assertEqual(len(specializations), 2)
        self.assertEqual(specializations, {e["source"] for e in graph["edges"] if e["kind"] == "references" and e["target"] == primary["id"]})
        self.assertEqual(specializations, {e["target"] for e in graph["edges"] if e["kind"] == "calls"})
        impacted = {primary["id"]}
        for _ in range(2):
            impacted |= {e["source"] for e in graph["edges"] if e["target"] in impacted}
        names = {n["name"] for n in graph["nodes"] if n["id"] in impacted}
        self.assertTrue({"numbers", "fractions"} <= names)
        self.assertNotIn("unrelated", names)

    def test_inheritance_virtual_static_target_and_template_calls(self):
        source = """struct Money { int cents; };
struct Store { virtual int price() const { return 1; } };
struct DiscountStore: Store { int price() const override { return 2; } };
template<class T> T identity(T value) { return value; }
struct Receipt { Money *amount; };
int reserve(const Store &s) { return s.price(); }
int run() { return identity(3); }
"""
        graph = self.run_graph({"store.cpp": source})
        self.assertTrue(graph["analysis_complete"], graph["diagnostics"] + graph["unresolved"])
        store, derived = self.nodes(graph, "Store")[0], self.nodes(graph, "DiscountStore")[0]
        self.assertTrue(any(e["kind"] == "inherits" and e["source"] == derived["id"] and e["target"] == store["id"] for e in graph["edges"]))
        reserve = self.nodes(graph, "reserve")[0]
        calls = [e for e in graph["edges"] if e["source"] == reserve["id"] and e["kind"] == "calls"]
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0]["dispatch"], "virtual")
        self.assertEqual(next(n["qualified_name"] for n in graph["nodes"] if n["id"] == calls[0]["target"]), "Store::price")
        run = self.nodes(graph, "run")[0]
        self.assertTrue(any(e["kind"] == "calls" and e["source"] == run["id"] and "identity" in e["target"] for e in graph["edges"]))
        receipt, money = self.nodes(graph, "Receipt")[0], self.nodes(graph, "Money")[0]
        self.assertTrue(any(e["kind"] == "uses_type" and e["source"] == receipt["id"] and e["target"] == money["id"] for e in graph["edges"]))

    def test_utf8_crlf_byte_ranges_and_unsaved_overlay_are_authoritative(self):
        source = '// 😀 Ω漢\r\nint café(int value) {\r\n  return value + 7; // λ\r\n}\r\n'
        disk = '// stale file\nint wrong() { return 0; }\n'
        graph = self.run_graph({"unicode.cpp": source}, disk_sources={"unicode.cpp": disk})
        node = self.nodes(graph, "café")[0]
        self.assertEqual(node["start_byte"], source.encode().index(b"int caf"))
        self.assertEqual(source.encode()[node["start_byte"]:node["end_byte"]].decode(), source[source.index("int café"):source.rindex("}")+1])
        self.assertEqual(node["line"], 2)
        self.assertEqual(node["end_line"], 4)
        self.assertFalse(self.nodes(graph, "wrong"))
        changed = self.run_graph({"unicode.cpp": source.replace("+ 7", "+ 9")})
        self.assertEqual(node["id"], self.nodes(changed, "café")[0]["id"])

    def test_implicit_copy_default_constructors_have_real_callable_targets(self):
        source = "struct Money{int n;}; Money refund(Money a){return a;} int run(){Money cash{1}; Money reserved; auto r=refund(cash); return r.n;}\n"
        graph = self.run_graph({"implicit.cpp": source})
        self.assertTrue(graph["analysis_complete"], graph["unresolved"] + graph["diagnostics"])
        constructors = {n["id"] for n in graph["nodes"] if n["qualified_name"] == "Money::Money"}
        self.assertGreaterEqual(len(constructors), 2)
        self.assertTrue(any(e["target"] in constructors and e["kind"] == "calls" for e in graph["edges"]))

    def test_system_template_contracts_do_not_dump_standard_library_graph(self):
        source = "#include <vector>\nstruct Item { int n; };\nstd::vector<Item> make() { return {}; }\nint count(std::vector<Item> const &v){return v.size();}\n"
        graph = self.run_graph({"standard.cpp": source})
        self.assertTrue(graph["analysis_complete"], graph["diagnostics"] + graph["unresolved"])
        self.assertLess(len(graph["nodes"]), 100)
        item, make = self.nodes(graph, "Item")[0], self.nodes(graph, "make")[0]
        self.assertTrue(any(e["source"] == make["id"] and e["target"] == item["id"] and e["kind"] == "uses_type" for e in graph["edges"]))
        self.assertTrue(any(n["external"] and "vector" in n["qualified_name"] for n in graph["nodes"]))
        self.assertGreater(len(graph["inputs"]), 2)
        self.assertTrue(all(Path(i["path"]).is_file() for i in graph["inputs"]))

    def test_compile_database_cwd_defines_include_and_build_flags(self):
        (self.root / "build").mkdir()
        (self.root / "include").mkdir()
        header = self.root / "include/external.hpp"
        header.write_text("inline int selected() { return 5; }\n")
        database = self.root / "compile_commands.json"
        database.write_text(json.dumps([{"directory": str(self.root / "build"), "file": "../main.cpp", "arguments": ["clang++", "-std=c++20", "-I../include", "-DENABLED=1", "-c", "../main.cpp", "-o", "forbidden.o", "-MMD", "-MF", "forbidden.d"]}]))
        graph = self.run_graph({"main.cpp": '#include "external.hpp"\n#if ENABLED\nint active(){return selected();}\n#else\nint wrong(){return 0;}\n#endif\n'}, options={"flags": [], "compile_commands": str(database)})
        self.assertTrue(self.nodes(graph, "active"))
        self.assertFalse(self.nodes(graph, "wrong"))
        selected = self.nodes(graph, "selected")[0]
        self.assertTrue(selected["external"])
        self.assertIsNone(selected["path"])
        self.assertFalse(graph["analysis_complete"])  # unindexed project header explicitly partial
        provenance = {item["path"]: item["hash"] for item in graph["inputs"]}
        self.assertEqual(provenance[str(database)], hashlib.sha256(database.read_bytes()).hexdigest())
        self.assertEqual(provenance[str(header)], hashlib.sha256(header.read_bytes()).hexdigest())
        self.assertFalse((self.root / "build/forbidden.o").exists())
        self.assertFalse((self.root / "build/forbidden.d").exists())

    def test_command_string_tokenization_does_not_execute_shell(self):
        (self.root / "include space").mkdir()
        (self.root / "include space/api.h").write_text("int called();\n")
        database = self.root / "compile_commands.json"
        database.write_text(json.dumps([{"directory": str(self.root), "file": "main.cpp", "command": "clang++ -std=c++20 -I'include space' -c main.cpp -o artifact.o"}]))
        graph = self.run_graph({"main.cpp": '#include "api.h"\nint caller(){return called();}\n'}, options={"compile_commands": str(database), "flags": []})
        self.assertTrue(self.nodes(graph, "caller"))
        self.assertFalse(any(d["severity"] == "error" for d in graph["diagnostics"]), graph["diagnostics"])
        for unsafe in ("@args.rsp", "-fplugin=/tmp/plugin.so", "-Xclang", "-fmodule-file=module.pcm", "--config=compiler.cfg", "-ivfsoverlay"):
            with self.subTest(unsafe=unsafe):
                rejected = self.run_graph({"main.cpp": "int safe(){return 0;}\n"}, options={"flags": [unsafe]})
                self.assertFalse(rejected["analysis_complete"])
                self.assertEqual(rejected["nodes"], [])
                self.assertTrue(any("Unsafe/unsupported" in d["message"] for d in rejected["diagnostics"]))

    def test_external_input_change_between_real_translation_units_is_detected(self):
        with tempfile.TemporaryDirectory(prefix="ae-clang-header-") as directory:
            header = Path(directory) / "api.hpp"
            old = b"#define VALUE 1\n"
            header.write_bytes(old)
            request = self.request({"a.cpp": '#include "api.hpp"\nint a(){return VALUE;}\n',
                                    "b.cpp": '#include "api.hpp"\nint b(){return VALUE;}\n'},
                                   options={"flags": ["-std=c++20", "-I", directory]})
            parse = self.clang.parseTranslationUnit2
            count = 0
            def actual_parse(*args):
                nonlocal count
                count += 1
                if count == 2:
                    header.write_bytes(b"#define VALUE 2\n")
                return parse(*args)
            with patch.object(self.clang, "parseTranslationUnit2", side_effect=actual_parse):
                graph = BACKEND.Graph(request, self.clang).analyze()
            self.assertFalse(graph["analysis_complete"])
            self.assertTrue(any("changed between translation units" in d["message"] for d in graph["diagnostics"]))
            provenance = {item["path"]: item["hash"] for item in graph["inputs"]}
            self.assertEqual(provenance[str(header.resolve())], hashlib.sha256(old).hexdigest())

    def test_external_header_and_search_directory_symlink_retarget_invalidates_provenance(self):
        for mode in ("directory", "header"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory(prefix="ae-clang-alias-") as directory:
                external = Path(directory)
                for version, name in (("v1", "First"), ("v2", "Second")):
                    target = external / version
                    target.mkdir()
                    (target / "api.hpp").write_text("struct " + name + " {}; using Selected=" + name + "; inline int chosen(Selected){return 1;}\n")
                include = external / "include"
                if mode == "directory":
                    include.symlink_to(external / "v1", target_is_directory=True)
                else:
                    include.mkdir()
                    (include / "api.hpp").symlink_to(external / "v1/api.hpp")
                request = self.request({"main.cpp": '#include <api.hpp>\nint run(){return chosen(Selected{});}\n'},
                                       options={"flags": ["-std=c++20", "-I" + str(include)]})
                old = BACKEND.Graph(request, self.clang).analyze()
                self.assertTrue(old["analysis_complete"], old["diagnostics"] + old["unresolved"])
                alias = str(include / "api.hpp")
                inputs = {item["path"]: item["hash"] for item in old["inputs"]}
                self.assertIn(alias, inputs)
                if mode == "directory":
                    include.unlink()
                    include.symlink_to(external / "v2", target_is_directory=True)
                else:
                    (include / "api.hpp").unlink()
                    (include / "api.hpp").symlink_to(external / "v2/api.hpp")
                self.assertNotEqual(hashlib.sha256(Path(alias).read_bytes()).hexdigest(), inputs[alias])
                # Old canonical target still exists unchanged: checking it alone
                # would incorrectly report a fresh cache.
                self.assertEqual(inputs[str((external / "v1/api.hpp").resolve())], hashlib.sha256((external / "v1/api.hpp").read_bytes()).hexdigest())
                new = BACKEND.Graph(request, self.clang).analyze()
                self.assertTrue(new["analysis_complete"], new["diagnostics"])
                # These external callable USRs encode the real resolved argument
                # types; no traversal/dump of unreferenced external bodies.
                old_call = self.nodes(old, "chosen")[0]
                new_call = self.nodes(new, "chosen")[0]
                self.assertIn("First", old_call["id"])
                self.assertIn("Second", new_call["id"])
                self.assertNotEqual(old_call["id"], new_call["id"])

    def test_missing_selected_tu_and_header_only_report_actionable_diagnostics(self):
        graph = self.run_graph({"api.hpp": "int declared();\n"})
        self.assertFalse(graph["analysis_complete"])
        self.assertTrue(any("No translation units" in d["message"] for d in graph["diagnostics"]))
        missing = self.run_graph({"present.cpp": "int real(){return 0;}\n"}, options={"files": ["missing.cpp"]})
        self.assertFalse(missing["analysis_complete"])
        self.assertTrue(any("absent from indexed inputs" in d["message"] for d in missing["diagnostics"]))

    def test_sdk_sysroot_alias_is_kept_in_actual_header_provenance(self):
        with tempfile.TemporaryDirectory(prefix="ae-clang-sdk-") as directory:
            external = Path(directory)
            for version, name in (("sdk-v1", "first"), ("sdk-v2", "second")):
                include = external / version / "usr/include"
                include.mkdir(parents=True)
                (include / "api.h").write_text("int " + name + "(void);\n#define chosen " + name + "\n")
            sdk = external / "MacOSX.sdk"
            sdk.symlink_to(external / "sdk-v1", target_is_directory=True)
            request = self.request({"main.c": '#include <api.h>\nint run(void){return chosen();}\n'}, language="c",
                                   options={"flags": ["-std=c11", "-isysroot", str(sdk)]})
            old = BACKEND.Graph(request, self.clang).analyze()
            self.assertTrue(old["analysis_complete"], old["diagnostics"])
            alias = str(sdk / "usr/include/api.h")
            provenance = {i["path"]: i["hash"] for i in old["inputs"]}
            self.assertIn(alias, provenance)
            sdk.unlink()
            sdk.symlink_to(external / "sdk-v2", target_is_directory=True)
            self.assertNotEqual(provenance[alias], hashlib.sha256(Path(alias).read_bytes()).hexdigest())
            new = BACKEND.Graph(request, self.clang).analyze()
            self.assertTrue(new["analysis_complete"], new["diagnostics"])
            self.assertTrue(self.nodes(old, "first"))
            self.assertTrue(self.nodes(new, "second"))

    def test_compiler_errors_and_indirect_calls_are_partial_not_guessed(self):
        graph = self.run_graph({"invalid.cpp": "int run(){return missing(1);}\n"})
        self.assertFalse(graph["analysis_complete"])
        self.assertTrue(any(d["severity"] == "error" and "missing" in d["message"] for d in graph["diagnostics"]))
        indirect = self.run_graph({"pointer.cpp": "int apply(int (*fn)(int), int x) { return fn(x); }\n"})
        self.assertFalse(indirect["analysis_complete"])
        self.assertTrue(indirect["unresolved"])
        self.assertFalse(any(e["kind"] == "calls" for e in indirect["edges"]))

    def test_stdio_protocol_is_exact_json_with_real_version_and_provenance(self):
        request = self.request({"main.c": "int twice(int n){return n+n;}\n"}, language="c")
        result = subprocess.run([sys.executable, str(ADAPTER)], input=json.dumps(request), text=True,
                                capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        graph = json.loads(result.stdout)
        self.assertEqual(graph["analyzer"]["name"], "libclang")
        self.assertIn("clang", graph["analyzer"]["version"].lower())
        self.assertTrue(graph["analysis_complete"], graph["diagnostics"])
        self.assertEqual(len(graph["inputs"]), 1)
        self.assertEqual(graph["inputs"][0]["hash"], hashlib.sha256(self.clang.path.read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
