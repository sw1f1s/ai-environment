#!/usr/bin/env python3
"""Static Python graph: Jedi resolution over an isolated authoritative snapshot.

AST supplies declaration extents, never semantic call targets. No target module is
imported or executed. Jedi's Interpreter API and unsafe extensions are not used.
"""
from __future__ import annotations

import ast
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile

VENDOR = Path(__file__).resolve().parent.parent / "vendor"


def bootstrap():
    if not (VENDOR / "jedi/__init__.py").is_file():
        raise ValueError("Python semantic analyzer unavailable: packaged Jedi/Parso missing; rebuild AI Env package")
    sys.path.insert(0, str(VENDOR))
    import jedi
    # Every request gets a distinct immutable snapshot. Keep Parso's per-path
    # cache enabled: Jedi 0.20 requires these cache records during resolution.
    jedi.settings.fast_parser = True
    jedi.settings.dynamic_params = False
    jedi.settings.dynamic_array_additions = False
    jedi.settings._cropped_file_size = 2 * 1024 * 1024 + 1
    return jedi


class Analyzer:
    def __init__(self, request, jedi, directory):
        self.jedi, self.request, self.directory = jedi, request, Path(directory).resolve()
        jedi.settings.cache_directory = str(self.directory / ".parser-cache")
        self.files, self.trees, self.scripts = {}, {}, {}
        self.nodes, self.edges, self.diagnostics, self.unresolved, self.inputs = {}, [], [], [], {}
        self.declarations, self.locations, self.parents = {}, {}, {}
        self.resolutions, self.import_names = {}, {}
        self.selected = set(request.get("options", {}).get("files") or [])
        for item in request["files"]:
            path = item["path"]
            rel = Path(path)
            if rel.is_absolute() or ".." in rel.parts or not path or "\\" in path or rel.as_posix() != path:
                raise ValueError("Noncanonical snapshot path")
            if hashlib.sha256(item["text"].encode()).hexdigest() != item["hash"]:
                raise ValueError("Snapshot source hash mismatch: " + path)
            if rel.suffix not in (".py", ".pyi"):
                continue
            self.files[path] = item["text"]
            destination = self.directory / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(item["text"].encode())
        self.project = jedi.Project(str(self.directory), sys_path=[str(self.directory), str(self.directory / "src")],
                                    smart_sys_path=False, load_unsafe_extensions=False)
        for path, text in self.files.items():
            try:
                self.trees[path] = ast.parse(text, filename=path, type_comments=True)
            except SyntaxError as error:
                self.diagnostics.append(dict(severity="error", path=path, line=error.lineno or 0, message=error.msg))
                continue
            self.scripts[path] = jedi.Script(code=text, path=str(self.directory / path), project=self.project)
        for path in self.selected - self.files.keys():
            self.diagnostics.append(dict(severity="error", path=path, line=0, message="Selected Python file not supplied"))

    def external_input(self, path):
        path = Path(path).resolve()
        if str(path) not in self.inputs and path.is_file() and not path.is_relative_to(self.directory):
            self.inputs[str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()

    def span(self, path, node):
        lines = self.files[path].splitlines(keepends=True)
        start = sum(len(line.encode()) for line in lines[:node.lineno - 1]) + node.col_offset
        end = sum(len(line.encode()) for line in lines[:node.end_lineno - 1]) + node.end_col_offset
        return start, end

    def source(self, path, node):
        start, end = self.span(path, node)
        return self.files[path].encode()[start:end].decode()

    def position(self, path, node, end=False):
        line = node.end_lineno if end else node.lineno
        byte = node.end_col_offset if end else node.col_offset
        text = self.files[path].splitlines(keepends=True)[line - 1]
        return line, len(text.encode()[:byte].decode())

    def declare(self, path, tree, parent=None, qualified=None):
        module = str(Path(path).with_suffix("")).replace("/", ".")
        if module.endswith(".__init__"):
            module = module[:-9]
        qualified = qualified or module
        for node in ast.iter_child_nodes(tree):
            if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
                kind = "type" if isinstance(node, ast.ClassDef) else "function"
                name, child = node.name, qualified + "." + node.name
                signature = ("class " + name + ("(" + ", ".join(ast.unparse(b) for b in node.bases) + ")" if node.bases else "")) if kind == "type" else (("async " if isinstance(node, ast.AsyncFunctionDef) else "") + "def " + name + "(" + ast.unparse(node.args) + ")" + (" -> " + ast.unparse(node.returns) if node.returns else ""))
                if node.decorator_list:
                    signature = " ".join("@" + ast.unparse(d) for d in node.decorator_list) + " " + signature
                identity = "python:" + path + ":" + child + ":" + kind
                # Repeated binding definitions cannot be merged; identify their static sites.
                if identity in self.nodes:
                    identity += ":site:" + str(node.lineno)
                self.add_declaration(path, node, name, child, kind, signature, identity, parent)
                self.declare(path, node, identity, child)
            elif isinstance(node, getattr(ast, "TypeAlias", ())):
                name = node.name.id
                identity = "python:" + path + ":" + qualified + "." + name + ":alias"
                self.add_declaration(path, node, name, qualified + "." + name, "alias", ast.unparse(node), identity, parent)
            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                for alias in node.names:
                    name = alias.asname or alias.name.split(".")[0]
                    identity = "python:" + path + ":" + qualified + "." + name + ":alias"
                    if identity in self.nodes:
                        identity += ":site:" + str(node.lineno)
                    self.add_declaration(path, node, name, qualified + "." + name, "alias", self.source(path, node), identity, parent, alias)
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                for target in targets:
                    if isinstance(target, ast.Name):
                        name = target.id
                        kind = "field" if parent and self.nodes[parent]["kind"] == "type" else "variable"
                        identity = "python:" + path + ":" + qualified + "." + name + ":" + kind
                        if identity not in self.nodes:
                            signature = name + (": " + ast.unparse(node.annotation) if isinstance(node, ast.AnnAssign) else "")
                            self.add_declaration(path, node, name, qualified + "." + name, kind, signature, identity, parent, target)
                    elif isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name) and target.value.id in ("self", "cls") and parent and self.nodes[parent]["kind"] == "function":
                        class_name = qualified.rsplit(".", 1)[0]
                        class_id = next((n["id"] for n in self.nodes.values() if n["path"] == path and n["kind"] == "type" and n["qualified_name"] == class_name), None)
                        if class_id:
                            name = target.attr
                            identity = "python:" + path + ":" + class_name + "." + name + ":field"
                            if identity not in self.nodes:
                                signature = name + (": " + ast.unparse(node.annotation) if isinstance(node, ast.AnnAssign) else "")
                                self.add_declaration(path, node, name, class_name + "." + name, "field", signature, identity, class_id, target)
                self.declare(path, node, parent, qualified)
            else:
                self.declare(path, node, parent, qualified)

    def add_declaration(self, path, ast_node, name, qualified, kind, signature, identity, parent, name_node=None):
        start, end = self.span(path, ast_node)
        line, column = ast_node.lineno, ast_node.col_offset + 1
        decorators = getattr(ast_node, "decorator_list", [])
        if decorators:
            first = decorators[0]
            start = self.span(path, first)[0] - 1  # include the ASCII '@'
            line, column = first.lineno, first.col_offset
        self.nodes[identity] = dict(id=identity, name=name, qualified_name=qualified, kind=kind, signature=signature,
                                    definition=True, external=False, implicit=False, path=path, line=line,
                                    column=column, end_line=ast_node.end_lineno, start_byte=start, end_byte=end)
        self.declarations.setdefault((path, id(ast_node)), []).append(identity)
        if parent:
            self.edge(parent, identity, "contains", path, ast_node)
        self.parents[(path, id(ast_node))] = parent
        # Match Jedi's declaration names by line/name, avoiding dependency on prefix lengths.
        self.locations.setdefault((path, ast_node.lineno, name), []).append(identity)
        if name_node and getattr(name_node, "lineno", None) != ast_node.lineno:
            self.locations.setdefault((path, name_node.lineno, name), []).append(identity)

    def edge(self, source, target, kind, path, node, dispatch="static"):
        if source and target:
            self.edges.append(dict(source=source, target=target, kind=kind, path=path, line=node.lineno,
                                   column=node.col_offset + 1, dispatch=dispatch))

    def unresolved_at(self, source, kind, path, node, reason):
        self.unresolved.append(dict(source=source, kind=kind, expression=self.source(path, node)[:1000], path=path,
                                    line=node.lineno, reason=reason))

    def target(self, definition):
        path = definition.module_path
        if path is not None:
            path = Path(path)
            if path.is_relative_to(self.directory):
                rel = path.relative_to(self.directory).as_posix()
                if definition.type == "module":
                    identity = "python:" + rel + ":module"
                    return identity if identity in self.nodes else None
                found = self.locations.get((rel, definition.line, definition.name), [])
                if len(found) == 1:
                    return found[0]
                # Project parameters/local inferred instances have no declaration card.
                return None
            self.external_input(path)
        if definition.type not in ("class", "function", "module", "instance"):
            return None
        qualified = definition.full_name or definition.name
        if not qualified:
            return None
        kind = {"class": "type", "instance": "type", "function": "function", "module": "namespace"}[definition.type]
        identity = "python:external:" + qualified + ":" + kind
        if identity not in self.nodes:
            signatures = definition.get_signatures() if definition.type == "function" else []
            signature = " | ".join(s.to_string() for s in signatures) or definition.description
            self.nodes[identity] = dict(id=identity, name=definition.name, qualified_name=qualified, kind=kind,
                                       signature=signature, definition=False, external=True, implicit=False,
                                       path=None, line=0, column=0, end_line=0, start_byte=0, end_byte=0)
        return identity

    def resolve(self, path, expression, infer=True):
        line, column = self.position(path, expression, True)
        key = path, line, column, infer
        if key in self.resolutions:
            return self.resolutions[key]
        try:
            method = self.scripts[path].infer if infer else self.scripts[path].goto
            names = method(line, column) if infer else method(line, column, follow_imports=True)
            targets = [self.target(name) for name in names]
            # Dropping an unmapped candidate would turn ambiguity into a false
            # unique target. Unknown candidates therefore invalidate the set.
            result = sorted(set(targets)) if targets and all(targets) else []
        except (ValueError, AttributeError, RecursionError) as error:
            self.diagnostics.append(dict(severity="warning", path=path, line=line, message="Jedi resolution unavailable: " + str(error)))
            result = []
        self.resolutions[key] = result
        return result

    def annotations(self, path, annotation, source, kind="uses_type"):
        if not annotation:
            return
        if isinstance(annotation, ast.Constant) and isinstance(annotation.value, str):
            # Exact locations inside quoted annotations aren't AST name positions.
            self.unresolved_at(source, kind, path, annotation, "Quoted forward annotation is not resolved by this adapter")
            return
        expressions = [n for n in ast.walk(annotation) if isinstance(n, (ast.Name, ast.Attribute))]
        for expression in expressions:
            targets = self.resolve(path, expression)
            targets = [t for t in targets if self.nodes[t]["kind"] in ("type", "alias")]
            if len(targets) == 1:
                self.edge(source, targets[0], kind, path, expression)
            elif isinstance(expression, ast.Name) or len(targets) > 1:
                self.unresolved_at(source, kind, path, expression, "Type is unknown or has multiple static targets")

    def visit(self, path, node, owner=None):
        enclosing_owner = owner
        declarations = self.declarations.get((path, id(node)), [])
        if declarations:
            if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
                owner = declarations[0]
            elif isinstance(node, (ast.Assign, ast.AnnAssign, ast.Import, ast.ImportFrom)):
                # Declaration facts belong to the variable/alias, calls in its
                # initializer remain attributed to enclosing callable where possible.
                for identity in declarations:
                    if isinstance(node, ast.AnnAssign):
                        self.annotations(path, node.annotation, identity)
                    if isinstance(node, ast.Assign) and isinstance(node.value, (ast.Name, ast.Attribute)):
                        targets = self.resolve(path, node.value)
                        if len(targets) == 1 and self.nodes[targets[0]]["kind"] in ("type", "function", "alias"):
                            self.nodes[identity]["kind"] = "alias"
                            self.nodes[identity]["signature"] = self.nodes[identity]["name"] + " = " + ast.unparse(node.value)
                            self.edge(identity, targets[0], "aliases", path, node.value)
                    if isinstance(node, (ast.Import, ast.ImportFrom)):
                        alias = next((a for a in node.names if (a.asname or a.name.split('.')[0]) == self.nodes[identity]['name']), None)
                        if alias:
                            try:
                                if path not in self.import_names:
                                    self.import_names[path] = self.scripts[path].get_names(all_scopes=True, definitions=True, references=False)
                                names = self.import_names[path]
                                matches = [n for n in names if n.line == node.lineno and n.name == self.nodes[identity]['name']]
                                targets = sorted({t for n in matches for d in n.goto(follow_imports=True) if (t := self.target(d)) and t != identity})
                            except (AttributeError, ValueError):
                                targets = []
                            if len(targets) == 1:
                                self.edge(identity, targets[0], "aliases", path, node)
                            else:
                                self.unresolved_at(identity, "aliases", path, node, "Import target unavailable or ambiguous in supplied snapshot")
                if not owner:
                    owner = declarations[0]
            elif isinstance(node, getattr(ast, "TypeAlias", ())):
                for identity in declarations:
                    self.annotations(path, node.value, identity, "aliases")
        if isinstance(node, ast.ClassDef):
            for base in node.bases:
                self.annotations(path, base, owner, "inherits")
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for argument in (*node.args.posonlyargs, *node.args.args, *node.args.kwonlyargs, *([node.args.vararg] if node.args.vararg else []), *([node.args.kwarg] if node.args.kwarg else [])):
                self.annotations(path, argument.annotation, owner)
            self.annotations(path, node.returns, owner)
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                self.unresolved_at(owner, "calls", path, decorator, "Decorator may replace the declared object; runtime transformation is not modeled")
        if isinstance(node, ast.Call):
            targets = self.resolve(path, node.func)
            callable_targets = [t for t in targets if self.nodes[t]["kind"] in ("type", "function")]
            if len(callable_targets) == 1:
                self.edge(owner, callable_targets[0], "calls", path, node.func, "dynamic")
            else:
                self.unresolved_at(owner, "calls", path, node, "Call has no unique statically inferred target; dynamic dispatch is not guessed")
        elif isinstance(node, (ast.Name, ast.Attribute)) and isinstance(node.ctx, ast.Load) and owner:
            targets = self.resolve(path, node, infer=False)
            if len(targets) == 1:
                self.edge(owner, targets[0], "references", path, node)
        for child in ast.iter_child_nodes(node):
            child_owner = owner
            if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) and child not in node.body:
                # Defaults, decorator factories and class bases are evaluated in
                # the enclosing scope, rather than on invocation of the body.
                child_owner = enclosing_owner
            self.visit(path, child, child_owner)

    def run(self):
        for path, tree in self.trees.items():
            module = str(Path(path).with_suffix("")).replace("/", ".")
            if module.endswith(".__init__"):
                module = module[:-9]
            identity = "python:" + path + ":module"
            self.nodes[identity] = dict(id=identity, name=module.rsplit(".", 1)[-1], qualified_name=module,
                                        kind="namespace", signature="module " + module, definition=True,
                                        external=False, implicit=False, path=path, line=1, column=1,
                                        end_line=max(1, len(self.files[path].splitlines())), start_byte=0,
                                        end_byte=len(self.files[path].encode()))
            self.declare(path, tree, identity, module)
        for path, tree in self.trees.items():
            if not self.selected or path in self.selected:
                self.visit(path, tree, "python:" + path + ":module")
        self.external_input(sys.executable)
        # Only modules actually imported by this adapter; vendor backend hash also
        # covers the complete installed dependency tree.
        for name, module in list(sys.modules.items()):
            if name.startswith(("jedi", "parso")) and getattr(module, "__file__", None):
                self.external_input(module.__file__)
        edges = {tuple(sorted(e.items())): e for e in self.edges}
        return dict(schema_version=1, language="python", analyzer=dict(name="jedi+ast", version=self.jedi.__version__,
                    capabilities=["symbols", "calls", "references", "uses_type", "inherits", "aliases"]),
                    nodes=sorted(self.nodes.values(), key=lambda n: n["id"]), edges=sorted(edges.values(), key=lambda e: (e["path"], e["line"], e["source"], e["kind"], e["target"])),
                    diagnostics=self.diagnostics, unresolved=self.unresolved,
                    inputs=[dict(path=p, hash=h) for p, h in sorted(self.inputs.items())], analysis_complete=False,
                    limitations=["Static Python inference cannot prove runtime monkey-patching, reflection, dynamic imports or dispatch; resolved calls are candidates, not an exhaustive runtime call graph.",
                                 "Resolution uses supplied Python snapshot modules, bundled static stubs and the installed interpreter's standard library; target project code, custom plugins and extensions are never executed.",
                                 "pyproject.toml packaging, dependency installation, custom import hooks, quoted forward annotations and dynamic source roots are not interpreted."])


def analyze(request):
    if request.get("schema_version") != 1 or request.get("language") != "python":
        raise ValueError("Expected schema 1 Python analyzer request")
    jedi = bootstrap()
    with tempfile.TemporaryDirectory(prefix="ai-env-python-snapshot-") as directory:
        analyzer = Analyzer(request, jedi, directory)
        # Jedi may acquire stdlib source through its isolated interpreter helper.
        # Observe all parsed files, including KnownContentFileIO instances whose
        # bytes originate in that helper, so cache freshness covers those reads.
        from jedi.inference import InferenceState
        original = InferenceState.parse_and_get_code
        def tracked_parse(state, *args, **kwargs):
            file_io = kwargs.get("file_io")
            path = getattr(file_io, "path", None) or kwargs.get("path")
            if path:
                analyzer.external_input(path)
            return original(state, *args, **kwargs)
        InferenceState.parse_and_get_code = tracked_parse
        try:
            return analyzer.run()
        finally:
            InferenceState.parse_and_get_code = original


def main():
    try:
        result = analyze(json.load(sys.stdin))
        json.dump(result, sys.stdout, ensure_ascii=False)
    except Exception as error:
        print("Python semantic analysis failed: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
