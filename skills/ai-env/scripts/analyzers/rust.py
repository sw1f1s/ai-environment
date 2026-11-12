#!/usr/bin/env python3
"""Rust graphs from rust-analyzer semantic definitions and its actual syntax tree.

Only a supplied source snapshot is loaded, in an isolated rust-project.json.
Cargo, build.rs, rustc wrappers, proc-macros, check and target programs never run.
"""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from urllib.parse import unquote, urlparse
from rust_lsp import Lsp


SYMBOL_KINDS={2:'namespace',3:'namespace',5:'type',6:'function',8:'field',10:'type',11:'type',12:'function',13:'variable',14:'variable',23:'type',26:'alias'}


def walk_symbols(items, ancestors=()):
    for item in items or []:
        yield item, ancestors
        yield from walk_symbols(item.get('children',[]),ancestors+(item,))


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def offset(text, pos):
    lines = text.splitlines(keepends=True)
    line, col = pos['line'], pos['character']
    if line == len(lines) and col == 0:
        return len(text.encode())
    if line >= len(lines):
        raise ValueError('LSP position outside source')
    raw = lines[line].encode('utf-16-le')[:col*2]
    return len(''.join(lines[:line]).encode()) + len(raw.decode('utf-16-le').encode())


def position(text, byte):
    prefix = text.encode()[:byte].decode()
    return {'line':prefix.count('\n'), 'character':len(prefix.rsplit('\n',1)[-1].encode('utf-16-le'))//2}


def walk(node, ancestors=()):
    yield node, ancestors
    for child in node.get('children', []):
        yield from walk(child, ancestors+(node,))


def uri_path(uri):
    parsed = urlparse(uri)
    if parsed.scheme != 'file':
        return None
    return Path(unquote(parsed.path))


def runtime():
    profile_path=Path(__file__).resolve().parent.parent/'bin/runtimes.json'
    profile=json.loads(profile_path.read_text()) if profile_path.is_file() else {}
    explicit=os.environ.get('AI_ENV_RUST_ANALYZER')
    candidates=[explicit] if explicit else [shutil.which('rust-analyzer'),profile.get('rust_analyzer')]
    env=os.environ.copy();executable=None;errors=[]
    for candidate in dict.fromkeys(p for p in candidates if p):
        # Keep rustup proxy basename: resolving its symlink invokes rustup itself.
        candidate=os.path.abspath(candidate)
        try:
            result=subprocess.run([candidate,'--version'],capture_output=True,text=True,timeout=10,env=env)
            if result.returncode==0 and result.stdout.startswith('rust-analyzer '):
                executable=candidate;break
            errors.append((result.stderr or result.stdout)[:1000])
        except (OSError,subprocess.TimeoutExpired) as exc:
            errors.append(str(exc))
    if not executable:
        raise ValueError('rust-analyzer unavailable; install rust-analyzer and rust-src, or set AI_ENV_RUST_ANALYZER. '+'; '.join(errors))
    sysroot = os.environ.get('AI_ENV_RUST_SYSROOT') or profile.get('rust_sysroot')
    if not sysroot:
        tool = shutil.which('rustc')
        if tool:
            found = subprocess.run([tool, '--print', 'sysroot'], capture_output=True,text=True,timeout=10,env=env)
            if found.returncode == 0:
                sysroot = found.stdout.strip()
    if not sysroot or not (Path(sysroot)/'lib/rustlib/src/rust/library/core/src/lib.rs').is_file():
        raise ValueError('rust-src unavailable; set AI_ENV_RUST_SYSROOT to a Rust sysroot with rust-src installed')
    return executable, sysroot, env, result.stdout.strip()


class Analyzer:
    def __init__(self, request):
        if request.get('language') != 'rust' or request.get('schema_version') != 1:
            raise ValueError('Rust adapter requires schema_version 1 and language rust')
        self.root = Path(request['root']).resolve()
        self.options = request.get('options') or {}
        self.files = {}
        self.configs = {}
        for entry in request['files']:
            path = entry['path']
            if entry.get('hash') is not None and entry['hash'] != hashlib.sha256(entry['text'].encode()).hexdigest():
                raise ValueError('Supplied source hash does not match text: '+path)
            p = Path(path)
            if p.is_absolute() or '..' in p.parts or p.as_posix() != path or '\\' in path or any(ord(c)<32 for c in path):
                raise ValueError('Unsafe source path')
            if p.suffix == '.rs':
                self.files[path] = entry['text']
            elif p.name in ('Cargo.toml', 'rust-project.json', 'Cargo.lock'):
                self.configs[path] = entry['text']
        if not self.files:
            raise ValueError('No Rust source files in snapshot')
        if self.options.get('files') and any(p not in self.files for p in self.options['files']):
            raise ValueError('Selected Rust source is absent from snapshot')
        self.nodes = {}; self.edges = {}; self.locations = {}; self.symbols = {}; self.external_symbols={}
        self.diagnostics=[];self.unresolved=[];self.inputs={};self.limitations=[
            'Static Rust source analysis only; Cargo/build scripts, proc-macros, generators and target programs are disabled.',
            'Call targets are compile-time declarations; trait-object runtime implementations and function-pointer targets are not inferred.',
            'Macro expansions are not indexed; macro invocations are reported unresolved. rust-analyzer diagnostics are not a rustc build proof.',
            'Uses the selected installed analyzer/sysroot; Cargo target, feature/default-feature and toolchain override evaluation is not performed.']

    def input(self, path):
        path=Path(path)
        if path.is_file():
            self.inputs[str(path.absolute())]=digest(path)

    def issue(self,path,message,severity='warning',line=0):
        self.diagnostics.append(dict(severity=severity,path=path,line=line,message=message))

    def unresolved_at(self,path,source,node,reason,kind='calls'):
        start=node['start'][0]; end=node['end'][0]
        self.unresolved.append(dict(source=source,kind=kind,expression=self.files[path].encode()[start:end].decode(),
                                    path=path,line=node['start'][1]+1,reason=reason))

    def crate_config(self, isolated, sysroot):
        crates=[]; names={}; pending=[]
        manifests={p:tomllib.loads(t) for p,t in self.configs.items() if Path(p).name=='Cargo.toml'}
        explicit=self.options.get('project')
        if explicit:
            path=Path(explicit)
            try: rel=path.resolve().relative_to(self.root).as_posix()
            except ValueError: raise ValueError('Rust project configuration must be inside the indexed snapshot')
            if rel not in self.configs:
                raise ValueError('Rust project configuration is absent from supplied snapshot')
            self.input(path)
            if path.name=='rust-project.json':
                cfg=json.loads(self.configs[rel]); base=Path(rel).parent
                for crate in cfg.get('crates',[]):
                    original=Path(crate['root_module'])
                    logical=(original.relative_to(self.root) if original.is_absolute() else base/original).as_posix()
                    if logical not in self.files: raise ValueError('rust-project root_module is absent from source snapshot')
                    crates.append(dict(root_module=str(isolated/logical),edition=crate.get('edition','2021'),
                                       deps=crate.get('deps',[]),cfg=crate.get('cfg',[]),display_name=crate.get('display_name')))
                if not crates: raise ValueError('rust-project.json has no crates')
                return dict(sysroot=sysroot,sysroot_src=str(Path(sysroot)/"lib/rustlib/src/rust/library"),crates=crates)
            if path.name!='Cargo.toml': raise ValueError('Rust --project must be Cargo.toml or rust-project.json')
        for manifest,data in sorted(manifests.items()):
            self.input(self.root/manifest)
            package=data.get('package')
            if not package: continue
            base=Path(manifest).parent
            candidates=[]
            lib=data.get('lib',{})
            libpath=(base/lib.get('path','src/lib.rs')).as_posix()
            if libpath in self.files: candidates.append((libpath,lib.get('name',package['name'].replace('-','_'))))
            for binary in data.get('bin',[]):
                binpath=(base/binary.get('path','src/main.rs')).as_posix()
                if binpath in self.files: candidates.append((binpath,binary.get('name',package['name'].replace('-','_'))))
            main=(base/'src/main.rs').as_posix()
            if main in self.files and all(p!=main for p,n in candidates): candidates.append((main,package['name'].replace('-','_')+'_bin'))
            for source,name in candidates:
                index=len(crates); names[(str(base),name)]=index
                edition=package.get('edition','2015')
                if isinstance(edition,dict):
                    inherited=[(Path(m).parent,d.get('workspace',{}).get('package',{}).get('edition')) for m,d in manifests.items() if base.is_relative_to(Path(m).parent)]
                    inherited=[(b,e) for b,e in inherited if e]
                    edition=max(inherited,key=lambda x:len(x[0].parts))[1] if inherited else '2015'
                    if not inherited:self.issue(manifest,'Workspace edition could not be inherited; using Cargo default edition 2015')
                if str(edition) not in ('2015','2018','2021','2024'):raise ValueError('Unsupported Rust edition: '+str(edition))
                crates.append(dict(root_module=str(isolated/source),edition=str(edition),deps=[],cfg=[],display_name=name))
                pending.append((index,base,data.get('dependencies',{}),libpath))
            if package.get('build') or (base/'build.rs').as_posix() in self.files:
                self.issue(manifest,'Build script intentionally disabled; generated declarations/configuration are unavailable')
            if data.get('features'):
                self.issue(manifest,'Cargo feature evaluation is not performed; cfg(feature) requires explicit rust-project.json cfg')
            if lib.get('proc-macro'):
                self.issue(manifest,'Proc-macro execution is disabled')
        for index,base,deps,libpath in pending:
            for name,spec in deps.items():
                target=None
                if isinstance(spec,dict) and 'path' in spec:
                    logical=Path(os.path.normpath(str(base/spec['path'])))
                    for (directory,crate_name),i in names.items():
                        if directory==str(logical) and i!=index:
                            target=i;break
                if target is None:
                    self.issue(None,'External/workspace dependency %s is unavailable in static snapshot crate graph'%name)
                else: crates[index]['deps'].append(dict(crate=target,name=name.replace('-','_')))
            # Binary can reference its package library without an explicit dependency.
            libindex=next((i for i,c in enumerate(crates) if c['root_module']==str(isolated/libpath)),None)
            if libindex is not None and libindex!=index:
                crates[index]['deps'].append(dict(crate=libindex,name=crates[libindex]['display_name']))
        if not crates:
            roots=[p for p in self.files if Path(p).name in ('lib.rs','main.rs')]
            if not roots: roots=[sorted(self.files)[0]]
            for path in roots:
                crates.append(dict(root_module=str(isolated/path),edition='2021',deps=[],cfg=[],display_name=Path(path).stem))
            self.issue(None,'No Cargo package configuration; inferred standalone crate roots, edition 2021')
        return dict(sysroot=sysroot,sysroot_src=str(Path(sysroot)/"lib/rustlib/src/rust/library"),crates=crates)

    def add_symbols(self,path,items,parent=None,qualifiers=()):
        for item in items or []:
            kind=SYMBOL_KINDS.get(item['kind'])
            if not kind: # impl blocks are structural containers, not fabricated types.
                self.add_symbols(path,item.get('children'),parent,qualifiers+(item['name'],));continue
            text=self.files[path];start=offset(text,item['range']['start']);end=offset(text,item['range']['end'])
            selected=offset(text,item['selectionRange']['start'])
            signature=(item['name']+': '+item['detail']) if item.get('detail') else item['name']
            qualified='::'.join(qualifiers+(item['name'],))
            identifier='rust:'+path+'::'+qualified+':'+signature
            n=dict(id=identifier,name=item['name'],qualified_name=qualified,kind=kind,signature=signature,definition=True,
                   external=False,implicit=False,path=path,line=item['selectionRange']['start']['line']+1,
                   column=selected-len(''.join(text.splitlines(keepends=True)[:item['selectionRange']['start']['line']]).encode())+1,
                   end_line=item['range']['end']['line']+1,start_byte=start,end_byte=end)
            self.nodes[identifier]=n;self.locations[(path,selected)]=identifier
            self.symbols[identifier]=item
            if parent:self.edge(parent,identifier,'contains',path,item['selectionRange']['start'])
            self.add_symbols(path,item.get('children'),identifier,qualifiers+(item['name'],))

    def containing(self,path,byte):
        candidates=[n for n in self.nodes.values() if n['path']==path and n['start_byte']<=byte<n['end_byte']]
        return min(candidates,key=lambda n:n['end_byte']-n['start_byte'])['id'] if candidates else None

    def edge(self,source,target,kind,path,pos,dispatch='static'):
        if not source or not target:return
        b=offset(self.files[path],pos)
        prefix=self.files[path].encode()[:b].decode()
        col=len(prefix.rsplit('\n',1)[-1].encode())+1
        e=dict(source=source,target=target,kind=kind,path=path,line=pos['line']+1,column=col,dispatch=dispatch)
        self.edges[tuple(e.values())]=e

    def resolve(self,path,pos):
        key=(path,pos['line'],pos['character'])
        if key in self.resolved:return self.resolved[key]
        result=self.client.request('textDocument/definition',dict(textDocument={'uri':(self.isolated/path).as_uri()},position=pos))
        if isinstance(result,dict):result=[result]
        if result:self.definition_presence.add(key)
        targets=[]
        for location in result or []:
            uri=location.get('targetUri',location.get('uri')); rng=location.get('targetSelectionRange',location.get('range'))
            file=uri_path(uri)
            if file is None:continue
            try:relative=file.relative_to(self.isolated).as_posix()
            except ValueError:relative=None
            if relative in self.files:
                byte=offset(self.files[relative],rng['start'])
                target=self.locations.get((relative,byte))
                if not target:
                    # Parameters and local bindings are not exported as project API nodes.
                    continue
            else:
                if not file.is_file():continue
                # Dependencies outside this snapshot are only admitted from the explicit
                # Rust sysroot; absolute #[path]/include! cannot silently use disk code.
                if not file.is_relative_to(Path(self.sysroot)):
                    self.issue(path,'Resolved source outside supplied snapshot and sysroot is excluded: '+str(file))
                    continue
                self.input(file)
                content=file.read_text();byte=offset(content,rng['start']);end=offset(content,rng['end'])
                name=content.encode()[byte:end].decode()
                identifier='rust:external:'+str(file)+':'+str(byte)+':'+name
                if uri not in self.external_symbols:
                    external=self.client.request('textDocument/documentSymbol',dict(textDocument={'uri':uri}))
                    self.external_symbols[uri]=[s for s,a in walk_symbols(external)]
                decl=next((s for s in self.external_symbols[uri] if s['selectionRange']['start']==rng['start']),None)
                external_kind=SYMBOL_KINDS.get(decl['kind']) if decl else None
                if not external_kind:
                    continue
                signature=(name+': '+decl['detail']) if decl.get('detail') else name
                self.nodes.setdefault(identifier,dict(id=identifier,name=name,qualified_name=name,kind=external_kind,signature=signature,
                    definition=True,external=True,implicit=False,path=None,line=0,column=0,end_line=0,start_byte=0,end_byte=0))
                target=identifier
            if target not in targets:targets.append(target)
        self.resolved[key]=targets
        return targets

    def analyze_tree(self,path,tree):
        for node,ancestors in walk(tree):
            kind=node.get('kind');children=node.get('children',[])
            if kind in ('CALL_EXPR','METHOD_CALL_EXPR'):
                if kind=='METHOD_CALL_EXPR':
                    refs=[c for c in children if c.get('kind')=='NAME_REF']
                else:
                    callee=next((c for c in children if c.get('type')=='Node'),None)
                    refs=[n for n,a in walk(callee or {}) if n.get('kind')=='NAME_REF' and not any(p.get('kind') in ('GENERIC_ARG_LIST','TYPE_ARG','PATH_TYPE') for p in a)]
                targetnode=refs[-1] if refs else node
                pos=position(self.files[path],targetnode['start'][0]);source=self.containing(path,node['start'][0])
                targets=self.resolve(path,pos)
                callable_targets=[t for t in targets if self.nodes[t]['kind'] in ('function','type')]
                if len(callable_targets)==1:
                    target=callable_targets[0]
                    # A method declaration inside a trait is a compile-time virtual target.
                    trait=any(n['kind']=='type' and self.symbols.get(n['id'],{}).get('kind')==11 and n['path']==self.nodes[target]['path'] and n['start_byte']<=self.nodes[target]['start_byte']<n['end_byte'] for n in self.nodes.values())
                    self.edge(source,target,'calls',path,pos,'virtual' if trait else 'static')
                    if trait:self.unresolved_at(path,source,node,'Trait dispatch runtime implementation is not statically determined')
                else:self.unresolved_at(path,source,node,'No unique callable declaration resolved by rust-analyzer')
            elif kind=='MACRO_CALL':
                self.unresolved_at(path,self.containing(path,node['start'][0]),node,'Macro expansions intentionally disabled','references')
            elif kind=='NAME_REF':
                # All classification comes from rust-analyzer AST, not token spelling.
                pos=position(self.files[path],node['start'][0]);source=self.containing(path,node['start'][0])
                targets=self.resolve(path,pos)
                in_type=any(a.get('kind') in ('PATH_TYPE','TYPE_BOUND','TYPE_BOUND_LIST') for a in ancestors)
                expression=self.files[path].encode()[node['start'][0]:node['end'][0]].decode()
                if not targets and (path,pos['line'],pos['character']) not in self.definition_presence and expression not in {'u8','u16','u32','u64','u128','usize','i8','i16','i32','i64','i128','isize','f16','f32','f64','f128','bool','char','str','Self','self','crate','super'}:
                    self.unresolved_at(path,source,node,'No declaration resolved by rust-analyzer','uses_type' if in_type else 'references')
                for target in targets:
                    self.edge(source,target,'uses_type' if in_type else 'references',path,pos)
                    if source and self.nodes[source]['kind']=='alias' and in_type:
                        self.edge(source,target,'aliases',path,pos)
            elif kind=='IMPL':
                types=[c for c in children if c.get('kind')=='PATH_TYPE']
                if len(types)>=2:
                    resolved=[]
                    for t in types[:2]:
                        refs=[n for n,a in walk(t) if n.get('kind')=='NAME_REF']
                        resolved.append(self.resolve(path,position(self.files[path],refs[-1]['start'][0])) if refs else [])
                    if all(len(t)==1 for t in resolved):self.edge(resolved[1][0],resolved[0][0],'inherits',path,position(self.files[path],types[0]['start'][0]))

    def run(self):
        executable,sysroot,env,version=runtime()
        self.sysroot=sysroot
        self.input(Path(executable).resolve())
        self.input(Path(__file__).resolve().parent.parent/'bin/runtimes.json')
        # rust-analyzer loads the sysroot crate graph independently; cover its whole
        # static source universe so edits to indirectly read standard types stale cache.
        for path in sorted((Path(sysroot)/'lib/rustlib/src/rust/library').rglob('*')):
            if path.is_file() and (path.suffix=='.rs' or path.name=='Cargo.toml'):
                self.input(path)
        # The proxy's actual toolchain compiler and analyzer affect semantics as well.
        for path in (Path(sysroot)/'bin/rust-analyzer',Path(sysroot)/'bin/rustc',Path(sysroot)/'lib/rustlib/src/rust/library/core/src/lib.rs'):
            self.input(path)
        with tempfile.TemporaryDirectory(prefix='ai-env-rust-') as directory:
            self.isolated=Path(directory).resolve()
            for path,text in self.files.items():
                p=self.isolated/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(text.encode())
            config=self.crate_config(self.isolated,sysroot)
            self.client=Lsp([executable],str(self.isolated),env)
            try:
                init=self.client.request('initialize',dict(processId=None,rootUri=self.isolated.as_uri(),
                    capabilities={'experimental':{'serverStatusNotification':True},'textDocument':{'documentSymbol':{'hierarchicalDocumentSymbolSupport':True}}},
                    initializationOptions={'linkedProjects':[config],'cargo':{'buildScripts':{'enable':False}},'procMacro':{'enable':False},
                    'checkOnSave':False,'check':{'enable':False},'diagnostics':{'enable':True},'files':{'watcher':'client'}}))
                self.client.notify('initialized',{})
                for path,text in self.files.items():
                    self.client.notify('textDocument/didOpen',dict(textDocument=dict(uri=(self.isolated/path).as_uri(),languageId='rust',version=1,text=text)))
                deadline=time.monotonic()+35
                while not any(n.get('method')=='experimental/serverStatus' and n['params'].get('quiescent') for n in self.client.notifications):
                    if time.monotonic()>deadline:raise ValueError('rust-analyzer workspace did not become ready')
                    self.client.request('rust-analyzer/analyzerStatus',{})
                    time.sleep(.02)
                self.resolved={};self.definition_presence=set()
                for path in sorted(self.files):
                    items=self.client.request('textDocument/documentSymbol',dict(textDocument={'uri':(self.isolated/path).as_uri()}))
                    module=Path(path).with_suffix('').parts
                    if module and module[0]=='src':module=module[1:]
                    if module and module[-1] in ('lib','main','mod'):module=module[:-1]
                    self.add_symbols(path,items,qualifiers=tuple(module))
                for path in sorted(self.files):
                    raw=self.client.request('rust-analyzer/viewSyntaxTree',dict(textDocument={'uri':(self.isolated/path).as_uri()}))
                    tree=json.loads(raw) if isinstance(raw,str) else raw
                    # rust-analyzer normalizes CRLF before parsing; its syntax-tree
                    # offsets address that normalized UTF8 buffer, while LSP ranges
                    # address the original document. Translate back to snapshot bytes.
                    if '\r\n' in self.files[path]:
                        normalized=self.files[path].replace('\r\n','\n')
                        for node,ancestors in walk(tree):
                            for key in ('start','end'):
                                node[key][0]=offset(self.files[path],position(normalized,node[key][0]))
                    for ast,ancestors in walk(tree):
                        if ast.get('kind')=='FN':
                            name=next((c for c in ast.get('children',[]) if c.get('kind')=='NAME'),None)
                            identifier=self.locations.get((path,name['start'][0])) if name else None
                            if identifier:
                                self.nodes[identifier]['definition']=any(c.get('kind')=='BLOCK_EXPR' for c in ast.get('children',[]))
                    if self.options.get('files') and path not in self.options['files']:
                        continue
                    self.analyze_tree(path,tree)
                    diagnostic=self.client.request('textDocument/diagnostic',dict(textDocument={'uri':(self.isolated/path).as_uri()}))
                    for item in (diagnostic or {}).get('items',[]):
                        self.issue(path,item['message'],{1:'error',2:'warning',3:'note',4:'note'}.get(item.get('severity'),'warning'),item['range']['start']['line']+1)
            finally:self.client.close()
        return dict(schema_version=1,language='rust',analyzer=dict(name='rust-analyzer LSP',version=version,
            capabilities=['symbols','calls','references','uses_type','inherits','aliases']),nodes=list(self.nodes.values()),edges=list(self.edges.values()),
            diagnostics=self.diagnostics,unresolved=self.unresolved,inputs=[dict(path=p,hash=h) for p,h in sorted(self.inputs.items())],
            analysis_complete=not self.unresolved and not any(d['severity'] in ('warning','error') for d in self.diagnostics),limitations=self.limitations)


def main():
    try:
        value=Analyzer(json.load(sys.stdin)).run()
    except Exception as exc:
        print('Rust semantic analyzer: '+str(exc),file=sys.stderr)
        return 1
    json.dump(value,sys.stdout,ensure_ascii=False)
    return 0

if __name__=='__main__':raise SystemExit(main())
