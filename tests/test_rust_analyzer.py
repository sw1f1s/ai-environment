"""Real rust-analyzer tests: isolated source snapshot, semantic identity and safety."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
ADAPTER=ROOT/'skills/ai-env/scripts/analyzers/rust.py'
FIXTURE=ROOT/'tests/fixtures/popular/rust'
SYSROOT=ROOT/'.tools/rustup/toolchains/stable-aarch64-apple-darwin'


def environment():
    env=os.environ.copy()
    if (SYSROOT/'bin/rust-analyzer').is_file():
        env.setdefault('AI_ENV_RUST_ANALYZER',str(SYSROOT/'bin/rust-analyzer'))
        env.setdefault('AI_ENV_RUST_SYSROOT',str(SYSROOT))
    return env


def request(root, files=None, options=None):
    if files is None:
        files=[dict(path=p.relative_to(FIXTURE).as_posix(),text=p.read_text()) for p in sorted(FIXTURE.rglob('*')) if p.is_file()]
    return dict(schema_version=1,language='rust',root=str(root),files=files,options=options or {})


def analyze(value, env=None):
    result=subprocess.run(['python3',str(ADAPTER)],input=json.dumps(value),capture_output=True,text=True,
                          env=env or environment(),timeout=55)
    if result.returncode:raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


class RustAnalyzerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp=tempfile.TemporaryDirectory(prefix='ai-env-rust-tests-')
        cls.root=Path(cls.temp.name)
        cls.graph=analyze(request(cls.root))
        cls.nodes={n['id']:n for n in cls.graph['nodes']}

    @classmethod
    def tearDownClass(cls):cls.temp.cleanup()

    def one(self,name,kind=None):
        matches=[n for n in self.nodes.values() if n['qualified_name']==name and (kind is None or n['kind']==kind)]
        self.assertEqual(len(matches),1,name)
        return matches[0]

    def has(self,source,target,kind):
        return any(e['source']==source['id'] and e['target']==target['id'] and e['kind']==kind for e in self.graph['edges'])

    def test_crossmodule_calls_distinguish_same_named_decoy(self):
        run=self.one('run');refund=self.one('ledger::refund');decoy=self.one('decoy::refund')
        self.assertTrue(self.has(run,refund,'calls'))
        self.assertFalse(self.has(run,decoy,'calls'))
        self.assertTrue(self.has(run,self.one('ledger::identity'),'calls'))
        self.assertTrue(self.graph['analysis_complete'],self.graph['diagnostics'])

    def test_receiver_resolves_impl_and_trait_relation(self):
        price=self.one('types::impl Pricing for Shop::price')
        self.assertTrue(self.has(self.one('run'),price,'calls'))
        self.assertTrue(self.has(self.one('types::Shop'),self.one('types::Pricing'),'inherits'))

    def test_alias_field_and_generic_type_dependencies(self):
        money=self.one('types::Money')
        self.assertTrue(self.has(self.one('types::Price'),money,'aliases'))
        self.assertTrue(self.has(self.one('types::Basket::values'),money,'uses_type'))
        self.assertTrue(self.has(self.one('ledger::refund'),money,'uses_type'))
        self.assertTrue(any(n['external'] and n['name']=='Option' and n['kind']=='type' for n in self.nodes.values()))

    def test_compiler_node_spans_are_utf8_bytes(self):
        run=self.one('run');text=(FIXTURE/'src/lib.rs').read_text()
        body=text.encode()[run['start_byte']:run['end_byte']].decode()
        self.assertEqual(body,'pub fn run(shop: Shop) -> i64 { identity(refund(shop.price())) }')
        self.assertGreater(run['start_byte'],text.index('pub fn run'))
        self.assertEqual(run['line'],7)

    def test_snapshot_overrides_disk_and_keeps_crlf(self):
        files=request(self.root)['files']
        for f in files:
            if f['path']=='src/lib.rs':f['text']=f['text'].replace('\n','\r\n')
        (self.root/'src').mkdir(exist_ok=True)
        (self.root/'src/lib.rs').write_text('compile_error!("STALE DISK MUST NOT BE ANALYZED");')
        graph=analyze(request(self.root,files))
        node=next(n for n in graph['nodes'] if n['qualified_name']=='run')
        text=next(f['text'] for f in files if f['path']=='src/lib.rs')
        self.assertEqual(text.encode()[node['start_byte']:node['end_byte']].decode(),'pub fn run(shop: Shop) -> i64 { identity(refund(shop.price())) }')
        self.assertTrue(graph['analysis_complete'],graph['diagnostics'])

    def test_dynamic_pointer_macro_missing_calls_remain_partial(self):
        files=[dict(path='Cargo.toml',text='[package]\nname="partial"\nversion="0.1.0"\nedition="2021"\n'),
               dict(path='src/lib.rs',text='pub fn run(callback: fn(i32)->i32) -> i32 { println!("hi"); missing(callback(1)) }')]
        graph=analyze(request(self.root,files))
        self.assertFalse(graph['analysis_complete'])
        expressions=[u['expression'] for u in graph['unresolved']]
        self.assertIn('callback(1)',expressions)
        self.assertIn('missing(callback(1))',expressions)
        self.assertTrue(any('println!' in s for s in expressions))
        self.assertTrue(any('No unique callable' in u['reason'] for u in graph['unresolved']))

    def test_build_script_never_executed(self):
        marker=self.root/'EXECUTED'
        files=request(self.root)['files']
        files.append(dict(path='build.rs',text='fn main(){std::fs::write('+json.dumps(str(marker))+',"bad").unwrap();}'))
        graph=analyze(request(self.root,files))
        self.assertFalse(marker.exists())
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any('Build script' in d['message'] for d in graph['diagnostics']))

    def test_runtime_and_stdlib_inputs_are_actual_hashes(self):
        self.assertTrue(any(p['path'].endswith('/bin/rust-analyzer') for p in self.graph['inputs']))
        self.assertTrue(any(p['path'].endswith('/core/src/option.rs') for p in self.graph['inputs']))
        import hashlib
        for item in self.graph['inputs'][:3]:self.assertEqual(item['hash'],hashlib.sha256(Path(item['path']).read_bytes()).hexdigest())
        self.assertFalse(any('ai-env-rust-' in p['path'] for p in self.graph['inputs']))

    def test_missing_runtime_and_unsafe_paths_fail(self):
        env=environment();env['AI_ENV_RUST_ANALYZER']='/missing/rust-analyzer'
        with self.assertRaisesRegex(RuntimeError,'Rust semantic analyzer'):analyze(request(self.root),env)
        with self.assertRaisesRegex(RuntimeError,'Unsafe source path'):analyze(request(self.root,[dict(path='../escape.rs',text='fn main(){}')]))

    def test_identity_stable_when_body_changes(self):
        files=request(self.root)['files']
        for f in files:
            if f['path']=='src/ledger.rs':f['text']=f['text'].replace('value.amount','value.amount + 1')
        graph=analyze(request(self.root,files))
        self.assertEqual(self.one('ledger::refund')['id'],next(n['id'] for n in graph['nodes'] if n['qualified_name']=='ledger::refund'))


    def test_unknown_type_is_not_silently_complete(self):
        files=[dict(path='Cargo.toml',text='[package]\nname="unknown"\nversion="0.1.0"\nedition="2021"\n'),
               dict(path='src/lib.rs',text='pub struct Holder { pub value: UnavailableType }')]
        graph=analyze(request(self.root,files))
        self.assertFalse(graph['analysis_complete'])
        self.assertTrue(any(u['expression']=='UnavailableType' and u['kind']=='uses_type' for u in graph['unresolved']))

    def test_trait_objects_are_compile_time_virtual_targets(self):
        files=request(self.root)['files']
        next(f for f in files if f['path']=='src/lib.rs')['text']+='\npub fn dynamic(value: &dyn Pricing) -> i64 { value.price().amount }\n'
        graph=analyze(request(self.root,files));nodes={n['id']:n for n in graph['nodes']}
        edge=next(e for e in graph['edges'] if e['kind']=='calls' and nodes[e['source']]['qualified_name']=='dynamic')
        self.assertEqual(nodes[edge['target']]['qualified_name'],'types::Pricing::price')
        self.assertEqual(edge['dispatch'],'virtual')
        self.assertFalse(graph['analysis_complete'])
        self.assertFalse(nodes[edge['target']]['definition'])

    def test_static_local_path_dependency_uses_supplied_sources(self):
        files=[dict(path='Cargo.toml',text='[package]\nname="root"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nhelper={path="helper"}\n'),
               dict(path='src/lib.rs',text='pub fn run()->i32 { helper::credit() }'),
               dict(path='helper/Cargo.toml',text='[package]\nname="helper"\nversion="0.1.0"\nedition="2021"\n'),
               dict(path='helper/src/lib.rs',text='pub fn credit()->i32 { 7 }')]
        graph=analyze(request(self.root,files));nodes={n['id']:n for n in graph['nodes']}
        self.assertTrue(any(e['kind']=='calls' and nodes[e['target']]['name']=='credit' and nodes[e['target']]['path']=='helper/src/lib.rs' for e in graph['edges']))
        self.assertTrue(graph['analysis_complete'],graph['diagnostics'])

    def test_mismatching_supplied_hash_is_rejected(self):
        files=request(self.root)['files'];files[0]['hash']='0'*64
        with self.assertRaisesRegex(RuntimeError,'hash does not match'):analyze(request(self.root,files))

    def test_explicit_project_requires_snapshot_and_rejects_outside(self):
        with self.assertRaisesRegex(RuntimeError,'inside the indexed snapshot'):
            analyze(request(self.root,options={'project':'/outside/Cargo.toml'}))
        graph=analyze(request(self.root,options={'project':str(self.root/'Cargo.toml')}))
        self.assertTrue(graph['analysis_complete'],graph['diagnostics'])


    def test_standard_library_function_is_not_mislabeled_as_type(self):
        files=request(self.root)['files']
        next(f for f in files if f['path']=='src/lib.rs')['text']+='\npub fn bytes()->usize { std::mem::size_of::<types::Money>() }\n'
        graph=analyze(request(self.root,files));nodes={n['id']:n for n in graph['nodes']}
        edge=next(e for e in graph['edges'] if e['kind']=='calls' and nodes[e['source']]['qualified_name']=='bytes')
        self.assertEqual(nodes[edge['target']]['kind'],'function')
        self.assertEqual(nodes[edge['target']]['name'],'size_of')
        self.assertTrue(nodes[edge['target']]['external'])


    def test_file_selection_excludes_unselected_relations(self):
        files=request(self.root)['files']
        files.append(dict(path='ignored.rs',text='pub fn ignored(){ absent_function(); }'))
        graph=analyze(request(self.root,files,options={'files':['src/lib.rs']}))
        self.assertTrue(graph['analysis_complete'],graph['unresolved'])
        self.assertTrue(any(n['name']=='ignored' for n in graph['nodes']))
        self.assertFalse(any(e['kind']!='contains' and e['path']=='ignored.rs' for e in graph['edges']))
        self.assertFalse(any(u['path']=='ignored.rs' for u in graph['unresolved']))


    def test_recursive_call_preserves_self_edge(self):
        files=[dict(path='Cargo.toml',text='[package]\nname="recursive"\nversion="0.1.0"\nedition="2021"\n'),
               dict(path='src/lib.rs',text='pub fn countdown(n: u32) -> u32 { if n == 0 { 0 } else { countdown(n - 1) } }')]
        graph=analyze(request(self.root,files))
        node=next(n for n in graph['nodes'] if n['name']=='countdown')
        self.assertTrue(any(e['source']==node['id'] and e['target']==node['id'] and e['kind']=='calls' for e in graph['edges']))
        self.assertTrue(graph['analysis_complete'],graph['unresolved'])

if __name__=='__main__':unittest.main()
