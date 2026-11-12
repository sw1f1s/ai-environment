"""Actual Roslyn integration, with no target-project build or network access."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / 'skills/ai-env/scripts/analyzers/csharp.py'
SOURCE = ADAPTER.with_suffix('')
HELPER = ADAPTER.parent.parent / 'bin/roslyn/GraphAnalyzer.dll'
SPEC = importlib.util.spec_from_file_location('aienv_csharp_analyzer', ADAPTER)
BACKEND = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BACKEND)

BUILD_SPEC = importlib.util.spec_from_file_location('aienv_roslyn_builder', SOURCE / 'build.py')
BUILDER = importlib.util.module_from_spec(BUILD_SPEC)
BUILD_SPEC.loader.exec_module(BUILDER)


class CSharpAnalyzerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        dotnet = os.environ.get('AI_ENV_DOTNET') or shutil.which('dotnet') or next((str(p) for p in (Path('/usr/local/share/dotnet/dotnet'), Path('/usr/share/dotnet/dotnet')) if p.is_file()), None)
        if not dotnet:
            raise unittest.SkipTest('C# analyzer requires a local .NET 8 runtime/SDK')
        newest = max(p.stat().st_mtime_ns for p in SOURCE.glob('*') if p.suffix in ('.cs', '.csproj'))
        if not HELPER.exists() or HELPER.stat().st_mtime_ns < newest:
            result = subprocess.run([sys.executable, str(SOURCE / 'build.py')], capture_output=True, text=True, timeout=120)
            if result.returncode:
                raise AssertionError('Roslyn helper build failed: ' + result.stderr[-4000:])

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='aienv-roslyn-test-')
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def request(self, texts, **options):
        return {'schema_version': 1, 'language': 'csharp', 'root': str(self.root),
                'files': [{'path': path, 'hash': hashlib.sha256(text.encode()).hexdigest(), 'text': text} for path, text in sorted(texts.items())], 'options': options}

    def analyze(self, texts, **options):
        graph = BACKEND.analyze(self.request(texts, **options))
        self.assertEqual(graph['schema_version'], 1)
        self.assertEqual(graph['analyzer']['name'], 'roslyn')
        self.assertNotEqual(graph['analyzer']['version'], 'unavailable')
        ids = [node['id'] for node in graph['nodes']]
        self.assertEqual(len(ids), len(set(ids)))
        for edge in graph['edges']:
            self.assertIn(edge['source'], ids)
            self.assertIn(edge['target'], ids)
        return graph

    def fixture(self):
        base = ROOT / 'tests/fixtures/semantic/csharp'
        return {p.name: p.read_bytes().decode() for p in base.glob('*.cs')}

    def test_overloads_interface_dispatch_aliases_generics_and_inheritance(self):
        graph = self.analyze(self.fixture())
        self.assertTrue(graph['analysis_complete'], graph['diagnostics'])
        refunds = [n for n in graph['nodes'] if n['name'] == 'Refund']
        self.assertEqual(len(refunds), 2)
        self.assertTrue(all('static' in n['signature'] for n in refunds))
        self.assertTrue(all(n['id'].startswith('csharp:M:Warehouse.Engine.Refund(') for n in refunds))
        self.assertEqual({e['target'] for e in graph['edges'] if e['kind'] == 'calls' and e['target'] in {n['id'] for n in refunds}}, {n['id'] for n in refunds})
        price = next(n['id'] for n in graph['nodes'] if n['qualified_name'] == 'Warehouse.IPricer.Price')
        self.assertTrue(any(e['target'] == price and e['dispatch'] == 'virtual' for e in graph['edges']))
        money = 'csharp:T:Warehouse.Money'
        alias = next(n['id'] for n in graph['nodes'] if n['kind'] == 'alias' and n['name'] == 'Amount')
        self.assertTrue(any(e['source'] == alias and e['target'] == money and e['kind'] == 'aliases' for e in graph['edges']))
        self.assertTrue(any(e['source'] == 'csharp:T:Warehouse.DiscountPricer' and e['target'] == 'csharp:T:Warehouse.BasePricer' and e['kind'] == 'inherits' for e in graph['edges']))
        saved = next(n['id'] for n in graph['nodes'] if n['qualified_name'] == 'Warehouse.Receipt.Saved')
        types = {e['target'] for e in graph['edges'] if e['source'] == saved and e['kind'] == 'uses_type'}
        self.assertIn('csharp:T:Warehouse.Box`1', types)
        self.assertIn(money, types)
        generic = next(n['id'] for n in graph['nodes'] if n['name'] == 'T' and n['kind'] == 'type')
        field = next(n['id'] for n in graph['nodes'] if n['qualified_name'] == 'Warehouse.Box<T>.Value')
        self.assertTrue(any(e['source'] == field and e['target'] == generic and e['kind'] == 'uses_type' for e in graph['edges']))

    def test_utf8_byte_spans_preserve_crlf_and_non_ascii(self):
        text = '// 雪 😀 Ω\r\nnamespace Café { public class A { public int Métier(int x) { return x + 1; } } }\r\n'
        graph = self.analyze({'nested/Code.cs': text})
        node = next(n for n in graph['nodes'] if n['name'] == 'Métier')
        self.assertEqual(node['path'], 'nested/Code.cs')
        span = text.encode()[node['start_byte']:node['end_byte']].decode()
        self.assertEqual(span, 'public int Métier(int x) { return x + 1; }')
        self.assertEqual(node['line'], 2)
        self.assertGreater(node['start_byte'], text.index('public int'))

    def test_changed_arguments_change_real_overload_target(self):
        base = 'class A { static int F(int x)=>x; static string F(string x)=>x; static object Run()=>F(ARG); }'
        one = self.analyze({'A.cs': base.replace('ARG', '1')})
        two = self.analyze({'A.cs': base.replace('ARG', '"one"')})
        targets = lambda g: {e['target'] for e in g['edges'] if e['kind'] == 'calls'}
        self.assertNotEqual(targets(one), targets(two))
        self.assertTrue(any('System.Int32)' in value for value in targets(one)))
        self.assertTrue(any('System.String)' in value for value in targets(two)))

    def test_missing_type_and_dynamic_calls_are_explicitly_partial(self):
        graph = self.analyze({'Bad.cs': 'class A { MissingType field; public object Run(dynamic value) { return value.Unknown(1); } }'})
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('CS0246' in d['message'] for d in graph['diagnostics']))
        self.assertTrue(any(u['kind'] == 'calls' and 'Dynamic' in u['reason'] for u in graph['unresolved']))
        self.assertFalse(any(n['name'] == 'Unknown' for n in graph['nodes']))

    def test_ambiguous_calls_do_not_select_a_candidate(self):
        graph = self.analyze({'Ambiguous.cs': 'class A { static void F(string s){} static void F(System.Uri s){} static void Run(){ F(null); } }'})
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('CS0121' in d['message'] for d in graph['diagnostics']))
        targets = {e['target'] for e in graph['edges'] if e['kind'] == 'calls'}
        self.assertFalse(any('.F(' in target for target in targets))
        self.assertTrue(any('candidates' in u['reason'] for u in graph['unresolved']))

    def test_external_contracts_and_actual_input_hashes(self):
        graph = self.analyze({'A.cs': 'class A { static void Run(){ System.Console.WriteLine(1); } }'})
        target = next(n for n in graph['nodes'] if n['name'] == 'WriteLine')
        self.assertTrue(target['external'])
        self.assertIsNone(target['path'])
        self.assertEqual(target['start_byte'], 0)
        self.assertTrue(any(e['target'] == target['id'] and e['kind'] == 'calls' for e in graph['edges']))
        names = {Path(i['path']).name for i in graph['inputs']}
        self.assertTrue({'GraphAnalyzer.dll', 'Microsoft.CodeAnalysis.dll', 'Microsoft.CodeAnalysis.CSharp.dll', 'System.Private.CoreLib.dll'} <= names)
        for item in graph['inputs']:
            self.assertTrue(Path(item['path']).is_absolute())
            self.assertEqual(hashlib.sha256(Path(item['path']).read_bytes()).hexdigest(), item['hash'])
        self.assertFalse(any('System.Linq.' in n['qualified_name'] for n in graph['nodes']))

    def test_explicit_metadata_reference_resolves_real_external_type(self):
        texts = {'A.cs': 'public class A { public Microsoft.CodeAnalysis.ISymbol Symbol; }'}
        without = self.analyze(texts)
        self.assertFalse(without['analysis_complete'])
        metadata = HELPER.parent / 'Microsoft.CodeAnalysis.dll'
        with_reference = self.analyze(texts, references=[str(metadata)])
        self.assertTrue(with_reference['analysis_complete'], with_reference['diagnostics'])
        target = next(n for n in with_reference['nodes'] if n['qualified_name'] == 'Microsoft.CodeAnalysis.ISymbol')
        self.assertTrue(target['external'])
        self.assertTrue(any(e['target'] == target['id'] and e['kind'] == 'uses_type' for e in with_reference['edges']))

    def test_missing_reference_and_selected_file_report_diagnostics(self):
        graph = self.analyze({'A.cs': 'class A {}'}, references=[str(self.root / 'missing.dll')], files=['Missing.cs'])
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('Metadata reference unavailable' in d['message'] for d in graph['diagnostics']))
        self.assertTrue(any('not supplied' in d['message'] for d in graph['diagnostics']))

    def test_defines_are_honored_without_building_target_project(self):
        text = '#if ENABLED\nclass Enabled {}\n#else\nclass Disabled {}\n#endif\n'
        original = self.analyze({'A.cs': text})
        selected = self.analyze({'A.cs': text}, defines=['ENABLED'])
        self.assertIn('Enabled', {n['name'] for n in selected['nodes']})
        self.assertNotIn('Disabled', {n['name'] for n in selected['nodes']})
        self.assertIn('Disabled', {n['name'] for n in original['nodes']})
        fixture = ROOT / 'tests/fixtures/semantic/csharp/Fixture.csproj'
        shutil.copyfile(fixture, self.root / 'Fixture.csproj')
        self.analyze(self.fixture())
        self.assertFalse((self.root / 'msbuild-was-executed.marker').exists())
        self.assertFalse((self.root / 'obj').exists())

    def test_delegate_target_is_not_guessed_and_ids_are_deterministic(self):
        texts = {'A.cs': 'class A { static int Run(System.Func<int,int> handler){ return handler(2); } }'}
        first = self.analyze(texts)
        second = self.analyze(texts)
        self.assertFalse(first['analysis_complete'])
        self.assertEqual(first['nodes'], second['nodes'])
        self.assertEqual(first['edges'], second['edges'])
        self.assertTrue(any('Delegate' in u['reason'] for u in first['unresolved']))
        self.assertTrue(any(e['dispatch'] == 'dynamic' for e in first['edges'] if e['kind'] == 'calls'))

    def test_partial_type_and_method_declarations_deduplicate(self):
        graph = self.analyze({'A.cs': 'partial class A { partial void Work(); void Run(){ Work(); } }',
                              'B.cs': 'partial class A { partial void Work() { System.Console.WriteLine(1); } }'})
        self.assertTrue(graph['analysis_complete'], graph['diagnostics'])
        work = [n for n in graph['nodes'] if n['name'] == 'Work']
        self.assertEqual(len(work), 1)
        self.assertTrue(work[0]['definition'])
        self.assertEqual(work[0]['path'], 'B.cs')
        self.assertEqual(len([n for n in graph['nodes'] if n['kind'] == 'type' and n['name'] == 'A']), 1)

    def test_authoritative_request_source_overrides_disk_and_hash_mismatch_rejects(self):
        (self.root / 'A.cs').write_text('this is not valid C#')
        graph = self.analyze({'A.cs': 'class RequestSource {}'})
        self.assertTrue(graph['analysis_complete'])
        self.assertIn('RequestSource', {n['name'] for n in graph['nodes']})
        request = self.request({'A.cs': 'class A {}'})
        request['files'][0]['hash'] = '0' * 64
        bad = BACKEND.analyze(request)
        self.assertFalse(bad['analysis_complete'])
        self.assertTrue(any('hash differs' in d['message'] for d in bad['diagnostics']))

    def test_helper_publish_replaces_old_tree_without_stale_satellites(self):
        target = self.root / 'roslyn'
        target.mkdir()
        (target / 'old.dll').write_bytes(b'old')
        (target / 'ru').mkdir()
        (target / 'ru' / 'old.resources.dll').write_bytes(b'stale')
        staged = self.root / 'fresh'
        staged.mkdir()
        (staged / 'GraphAnalyzer.dll').write_bytes(b'new')
        self.assertEqual(BUILDER.publish(staged, target), [])
        self.assertEqual({p.name for p in target.iterdir()}, {'GraphAnalyzer.dll'})
        self.assertEqual((target / 'GraphAnalyzer.dll').read_bytes(), b'new')

    def test_helper_publication_failure_restores_previous_complete_helper(self):
        target = self.root / 'roslyn'
        target.mkdir()
        (target / 'GraphAnalyzer.dll').write_bytes(b'previous working helper')
        staged = self.root / 'fresh'
        staged.mkdir()
        (staged / 'GraphAnalyzer.dll').write_bytes(b'new helper')
        replace = os.replace
        calls = []
        def fail_new_install(source, destination):
            calls.append((source, destination))
            if len(calls) == 2:
                raise OSError('injected publication failure')
            return replace(source, destination)
        with patch.object(BUILDER.os, 'replace', side_effect=fail_new_install):
            with self.assertRaises(OSError):
                BUILDER.publish(staged, target)
        self.assertEqual((target / 'GraphAnalyzer.dll').read_bytes(), b'previous working helper')
        self.assertEqual((staged / 'GraphAnalyzer.dll').read_bytes(), b'new helper')

    def test_implicit_source_constructor_has_compiler_id_and_no_fabricated_body(self):
        graph = self.analyze({'A.cs': 'class A { static A Run()=>new A(); }'})
        constructor = next(n for n in graph['nodes'] if n['name'] == '.ctor')
        self.assertEqual(constructor['id'], 'csharp:M:A.#ctor')
        self.assertTrue(constructor['implicit'])
        self.assertFalse(constructor['external'])
        self.assertIsNone(constructor['path'])
        self.assertEqual(constructor['start_byte'], 0)
        self.assertEqual(constructor['end_byte'], 0)


if __name__ == '__main__':
    unittest.main()
