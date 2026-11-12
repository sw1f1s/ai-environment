"""Real TypeScript Compiler API QA for TS and checked JavaScript snapshots."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / 'skills/ai-env/scripts/analyzers/typescript.py'
SPEC = importlib.util.spec_from_file_location('aienv_typescript', ADAPTER)
BACKEND = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BACKEND)


class TypeScriptAnalyzerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='aienv-typescript-test-')
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def request(self, sources, language='typescript', **options):
        return {'schema_version': 1, 'language': language, 'root': str(self.root),
                'files': [{'path': name, 'text': text, 'hash': hashlib.sha256(text.encode()).hexdigest()} for name, text in sorted(sources.items())], 'options': options}

    def analyze(self, sources, language='typescript', **options):
        result = BACKEND.analyze(self.request(sources, language, **options))
        ids = {n['id'] for n in result['nodes']}
        self.assertEqual(len(ids), len(result['nodes']))
        self.assertEqual(result['analyzer']['name'], 'typescript-compiler')
        for edge in result['edges']:
            self.assertIn(edge['source'], ids)
            self.assertIn(edge['target'], ids)
        return result

    def fixture(self, language):
        base = ROOT / 'tests/fixtures/popular' / language
        return {p.relative_to(base).as_posix(): p.read_text() for p in base.rglob('*') if p.is_file()}

    def test_ts_real_overloads_alias_generics_inheritance_receivers(self):
        graph = self.analyze(self.fixture('typescript'))
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        refunds = [n for n in graph['nodes'] if n['name'] == 'refund' and n['kind'] == 'function']
        self.assertEqual(len(refunds), 3)
        called = {e['target'] for e in graph['edges'] if e['kind'] == 'calls'}
        self.assertEqual(len(called & {n['id'] for n in refunds}), 2)
        self.assertNotIn(next(n['id'] for n in graph['nodes'] if n['name'] == 'decoy'), called)
        price = next(n['id'] for n in graph['nodes'] if n['qualified_name'] == 'Pricer.price')
        self.assertTrue(any(e['target'] == price and e['kind'] == 'calls' and e['dispatch'] == 'virtual' for e in graph['edges']))
        money = next(n['id'] for n in graph['nodes'] if n['name'] == 'Money' and n['kind'] == 'type')
        amount = next(n['id'] for n in graph['nodes'] if n['name'] == 'Amount' and n['path'] == 'src/types.ts')
        self.assertTrue(any(e['source'] == amount and e['target'] == money and e['kind'] == 'aliases' for e in graph['edges']))
        saved = next(n['id'] for n in graph['nodes'] if n['qualified_name'] == 'Receipt.saved')
        box = next(n['id'] for n in graph['nodes'] if n['qualified_name'] == 'Box')
        self.assertTrue({money, box} <= {e['target'] for e in graph['edges'] if e['source'] == saved and e['kind'] == 'uses_type'})
        derived = next(n['id'] for n in graph['nodes'] if n['name'] == 'Discount')
        base = next(n['id'] for n in graph['nodes'] if n['name'] == 'Base')
        self.assertTrue(any(e['source'] == derived and e['target'] == base and e['kind'] == 'inherits' for e in graph['edges']))

    def test_js_real_jsdoc_and_cross_module_target(self):
        graph = self.analyze(self.fixture('javascript'), language='javascript')
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        refund = next(n for n in graph['nodes'] if n['name'] == 'refund' and n['kind'] == 'function')
        self.assertIn('number', refund['signature'])
        self.assertTrue(any(e['target'] == refund['id'] and e['kind'] == 'calls' for e in graph['edges']))
        self.assertTrue(any('JSDoc' in item for item in graph['limitations']))

    def test_overload_implementation_impact_reaches_each_selected_contract_caller(self):
        graph = self.analyze({'code.ts': '''export function refund(value: number): number;
export function refund(value: string): string;
export function refund(value: number | string): number | string { return value; }
export function numbers() { return refund(1); }
export function strings() { return refund("one"); }
export function unrelated() { return 42; }
'''})
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        refunds = [n for n in graph['nodes'] if n['name'] == 'refund']
        self.assertEqual(len(refunds), 3)
        implementation = next(n for n in refunds if n['definition'])
        contracts = {n['id'] for n in refunds if not n['definition']}
        self.assertEqual(contracts, {e['source'] for e in graph['edges'] if e['kind'] == 'references' and e['target'] == implementation['id']})
        self.assertEqual(contracts, {e['target'] for e in graph['edges'] if e['kind'] == 'calls'})
        impacted = {implementation['id']}
        for _ in range(2):
            impacted |= {e['source'] for e in graph['edges'] if e['target'] in impacted}
        names = {n['name'] for n in graph['nodes'] if n['id'] in impacted}
        self.assertTrue({'numbers', 'strings'} <= names)
        self.assertNotIn('unrelated', names)

    def test_method_and_constructor_overload_contracts_link_to_their_implementation(self):
        graph = self.analyze({'code.ts': '''export class Shop {
constructor(value: number);
constructor(value: string);
constructor(value: number | string) {}
refund(value: number): number;
refund(value: string): string;
refund(value: number | string): number | string { return value; }
}
export function run() { return new Shop(1).refund("one"); }
'''})
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        for name in ('constructor', 'refund'):
            declarations = [n for n in graph['nodes'] if n['name'] == name]
            implementation = next(n for n in declarations if n['definition'])
            contracts = {n['id'] for n in declarations if not n['definition']}
            self.assertEqual(len(contracts), 2)
            self.assertEqual(contracts, {e['source'] for e in graph['edges'] if e['target'] == implementation['id'] and e['kind'] == 'references'})
            self.assertTrue(any(e['target'] in contracts and e['kind'] == 'calls' for e in graph['edges']))

    def test_utf8_crlf_exact_body_and_stable_body_change_id(self):
        text = '// 雪 😀\r\nexport function café(value: number): number { return value + 1; }\r\n'
        graph = self.analyze({'code.ts': text})
        node = next(n for n in graph['nodes'] if n['name'] == 'café')
        self.assertEqual(text.encode()[node['start_byte']:node['end_byte']].decode(), 'export function café(value: number): number { return value + 1; }')
        self.assertEqual(node['line'], 2)
        other = self.analyze({'code.ts': text.replace('value + 1', 'value + 29')})
        self.assertEqual(node['id'], next(n['id'] for n in other['nodes'] if n['name'] == 'café'))

    def test_overlay_and_import_resolution_do_not_read_stale_source(self):
        (self.root / 'math.ts').write_text('export function target(x: string): string { throw 1; }')
        texts = {'math.ts': 'export function target(x: number): number { return x; }',
                 'app.ts': "import {target} from './math';\nexport function run(): number { return target(1); }"}
        graph = self.analyze(texts)
        self.assertTrue(graph['analysis_complete'], graph['diagnostics'])
        node = next(n for n in graph['nodes'] if n['kind'] == 'function' and n['name'] == 'target')
        self.assertIn('number', node['signature'])
        self.assertTrue(any(e['target'] == node['id'] and e['kind'] == 'calls' for e in graph['edges']))
        self.assertNotIn(str(self.root / 'math.ts'), {i['path'] for i in graph['inputs']})

    def test_missing_snapshot_source_not_silently_loaded_from_disk(self):
        (self.root / 'hidden.ts').write_text('export function hidden() { return 1; }')
        graph = self.analyze({'app.ts': "import {hidden} from './hidden';\nexport function run(){ return hidden(); }"})
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('TS2307' in d['message'] for d in graph['diagnostics']))
        self.assertFalse(any(n['kind'] == 'function' and n['name'] == 'hidden' for n in graph['nodes']))

    def test_dynamic_callback_and_missing_types_are_partial(self):
        graph = self.analyze({'bad.ts': 'export function run(value: any, callback: (n: number)=>number): number {\n value.missing();\n return callback(1);\n}\nexport let missing: UnknownType;\n'})
        self.assertFalse(graph['analysis_complete'])
        self.assertEqual(len(graph['unresolved']), 2)
        self.assertTrue(any('TS2304' in d['message'] for d in graph['diagnostics']))
        self.assertFalse(any(n['name'] == 'missing' and n['kind'] == 'function' for n in graph['nodes']))

    def test_static_config_extends_paths_no_plugin_or_target_execution(self):
        marker = self.root / 'EXECUTED'
        (self.root / 'evil.cjs').write_text("require('fs').writeFileSync(" + json.dumps(str(marker)) + ", 'yes');")
        files = {'base.json': {'compilerOptions': {'target': 'ES2022', 'module': 'commonjs', 'baseUrl': '.', 'paths': {'@math': ['lib/math']}, 'plugins': [{'name': './evil.cjs'}]}},
                 'tsconfig.json': {'extends': './base.json', 'include': ['src/**/*.ts']},
                 'lib/math.ts': 'export function target(n: number): number { return n; }',
                 'src/app.ts': "import {target} from '@math';\nexport function run() { return target(1); }"}
        files = {name: json.dumps(text) if isinstance(text, dict) else text for name, text in files.items()}
        graph = self.analyze(files, project=str(self.root / 'tsconfig.json'))
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        self.assertFalse(marker.exists())
        self.assertTrue(any('plugins' in note for note in graph['limitations']))
        target = next(n for n in graph['nodes'] if n['kind'] == 'function' and n['name'] == 'target')
        self.assertTrue(any(e['target'] == target['id'] and e['kind'] == 'calls' for e in graph['edges']))

    def test_input_provenance_real_runtime_compiler_libs(self):
        graph = self.analyze({'code.ts': 'export function run(value: string): number { return value.length; }'})
        names = {Path(i['path']).name for i in graph['inputs']}
        self.assertIn('typescript.js', names)
        self.assertIn('typescript.cjs', names)
        self.assertTrue(any(n.startswith('lib.es') and n.endswith('.d.ts') for n in names))
        for item in graph['inputs']:
            self.assertTrue(Path(item['path']).is_absolute())
            self.assertEqual(hashlib.sha256(Path(item['path']).read_bytes()).hexdigest(), item['hash'])

    def test_jsdoc_named_type_dependencies_and_implicit_constructors(self):
        graph = self.analyze({'code.js': '/** @typedef {{cents: number}} Money */\n/** @param {Money} value @returns {number} */\nexport function cost(value) { return value.cents; }\nexport class Empty {}\nexport function create() { return new Empty(); }'}, language='javascript')
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        money = next(n for n in graph['nodes'] if n['name'] == 'Money')
        cost = next(n for n in graph['nodes'] if n['name'] == 'cost')
        self.assertTrue(any(e['source'] == cost['id'] and e['target'] == money['id'] and e['kind'] == 'uses_type' for e in graph['edges']))
        constructor = next(n for n in graph['nodes'] if n['name'] == 'constructor')
        self.assertTrue(constructor['implicit'])
        self.assertIsNone(constructor['path'])
        self.assertEqual(constructor['start_byte'], 0)
        self.assertTrue(any(e['target'] == constructor['id'] and e['kind'] == 'calls' for e in graph['edges']))

    def test_unrelated_same_line_diagnostic_keeps_known_call_and_node_options_ignored(self):
        with patch.dict(os.environ, {'NODE_OPTIONS': '--require /no/such/target/plugin.cjs'}):
            graph = self.analyze({'code.ts': 'export function target(n: number): number { return n; }\nexport function run() { const unknown: Missing = 1; return target(1); }'})
        self.assertFalse(graph['analysis_complete'])
        target = next(n for n in graph['nodes'] if n['name'] == 'target')
        self.assertTrue(any(e['target'] == target['id'] and e['kind'] == 'calls' for e in graph['edges']))

    def test_union_call_targets_not_guessed_and_top_level_calls_have_implicit_owner(self):
        graph = self.analyze({'code.ts': 'export function first(n: number): number { return n; }\nexport function second(n: number): number { return n+1; }\nfirst(1);\nexport function run(choice: typeof first | typeof second) { return choice(2); }'})
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('Union receiver' in u['reason'] for u in graph['unresolved']))
        module = next(n for n in graph['nodes'] if n['name'] == '<module>')
        first = next(n for n in graph['nodes'] if n['name'] == 'first')
        self.assertTrue(any(e['source'] == module['id'] and e['target'] == first['id'] and e['kind'] == 'calls' for e in graph['edges']))
        run = next(n for n in graph['nodes'] if n['name'] == 'run')
        self.assertFalse(any(e['source'] == run['id'] and e['kind'] == 'calls' for e in graph['edges']))

    def test_recursive_calls_and_recursive_type_dependencies_are_retained(self):
        graph = self.analyze({'code.ts': 'export function recurse(n: number): number { return n ? recurse(n-1) : 0; }\nexport interface Branch { child?: Branch }\nexport type Tree = { children: Tree[] };\nexport type Nest = Nest[];\nexport type Plain = { value: number };'})
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        recurse = next(n for n in graph['nodes'] if n['name'] == 'recurse')
        self.assertTrue(any(e['source'] == recurse['id'] and e['target'] == recurse['id'] and e['kind'] == 'calls' for e in graph['edges']))
        branch = next(n for n in graph['nodes'] if n['name'] == 'Branch')
        child = next(n for n in graph['nodes'] if n['qualified_name'] == 'Branch.child')
        self.assertTrue(any(e['source'] == child['id'] and e['target'] == branch['id'] and e['kind'] == 'uses_type' for e in graph['edges']))
        tree = next(n for n in graph['nodes'] if n['name'] == 'Tree')
        children = next(n for n in graph['nodes'] if n['qualified_name'] == 'Tree.children')
        self.assertTrue(any(e['source'] == children['id'] and e['target'] == tree['id'] and e['kind'] == 'uses_type' for e in graph['edges']))
        nest = next(n for n in graph['nodes'] if n['name'] == 'Nest')
        self.assertTrue(any(e['source'] == nest['id'] and e['target'] == nest['id'] and e['kind'] == 'uses_type' for e in graph['edges']))
        plain = next(n for n in graph['nodes'] if n['name'] == 'Plain')
        self.assertFalse(any(e['source'] == plain['id'] and e['target'] == plain['id'] and e['kind'] == 'uses_type' for e in graph['edges']))

    def test_wide_union_type_traversal_limit_is_explicitly_partial(self):
        classes = '\n'.join(f'export class C{i} {{ marker{i} = 0; }}' for i in range(110))
        union = ' | '.join(f'C{i}' for i in range(110))
        graph = self.analyze({'wide.ts': classes + '\nexport let values: ' + union + ';'})
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('100 distinct types' in d['message'] and d['severity'] == 'warning' for d in graph['diagnostics']))
        self.assertTrue(any('truncated' in note for note in graph['limitations']))
        values = next(n for n in graph['nodes'] if n['name'] == 'values')
        dependencies = {e['target'] for e in graph['edges'] if e['source'] == values['id'] and e['kind'] == 'uses_type'}
        self.assertGreater(len(dependencies), 1)
        self.assertLess(len(dependencies), 110)

    def test_long_callable_contract_preserves_parameters_after_4096_characters(self):
        parameters = ', '.join(f'value{i}: number' for i in range(350))
        graph = self.analyze({'wide.ts': f'export function wide({parameters}): number {{ return value349; }}'})
        self.assertTrue(graph['analysis_complete'], (graph['diagnostics'], graph['unresolved']))
        function = next(n for n in graph['nodes'] if n['name'] == 'wide')
        self.assertGreater(len(function['signature']), 4096)
        self.assertTrue(function['signature'].endswith('value349: number): number'), function['signature'][-120:])
        for index in range(350):
            self.assertIn(f'value{index}: number', function['signature'])

    def test_selected_missing_config_and_bad_runtime_are_actionable(self):
        with self.assertRaisesRegex(RuntimeError, 'configuration unavailable'):
            self.analyze({'code.ts': 'export const x=1;'}, project=str(self.root / 'missing.json'))
        with self.assertRaisesRegex(RuntimeError, 'Selected source not supplied'):
            self.analyze({'code.ts': 'export const x=1;'}, files=['missing.ts'])
        with patch.dict(os.environ, {'AI_ENV_NODE': str(self.root / 'missing-node')}):
            with self.assertRaises(OSError):
                self.analyze({'code.ts': 'export const x=1;'})


if __name__ == '__main__':
    unittest.main()
