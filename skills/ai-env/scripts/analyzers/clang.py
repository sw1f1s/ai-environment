#!/usr/bin/env python3
"""Compiler-resolved C/C++ graph, using only Python stdlib and libclang C API.

API declarations follow clang-c/Index.h (https://clang.llvm.org/doxygen/).
No compiler command, build script, response file or project plugin is executed.
"""
from __future__ import annotations

import ctypes as C
import glob
import hashlib
import json
import os
from pathlib import Path
import shlex
import sys


class CXString(C.Structure):
    _fields_ = [("data", C.c_void_p), ("flags", C.c_uint)]


class Cursor(C.Structure):
    _fields_ = [("kind", C.c_uint), ("xdata", C.c_int), ("data", C.c_void_p * 3)]


class Type(C.Structure):
    _fields_ = [("kind", C.c_uint), ("data", C.c_void_p * 2)]


class Location(C.Structure):
    _fields_ = [("data", C.c_void_p * 2), ("value", C.c_uint)]


class Range(C.Structure):
    _fields_ = [("data", C.c_void_p * 2), ("start", C.c_uint), ("end", C.c_uint)]


class Unsaved(C.Structure):
    _fields_ = [("filename", C.c_char_p), ("contents", C.c_char_p), ("length", C.c_ulong)]


VISITOR = C.CFUNCTYPE(C.c_uint, Cursor, Cursor, C.c_void_p)
INCLUSION = C.CFUNCTYPE(None, C.c_void_p, C.POINTER(Location), C.c_uint, C.c_void_p)
TU_SUFFIXES = {"c": {".c"}, "cpp": {".cpp", ".cc", ".cxx", ".c++", ".C"}}
HEADERS = {".h", ".hh", ".hpp", ".hxx", ".h++", ".inc"}
FUNCTIONS = {"FunctionDecl", "CXXMethod", "Constructor", "Destructor", "ConversionFunction",
             "CXXConstructor", "CXXDestructor", "CXXConversion", "FunctionTemplate"}
DECLARATIONS = {**{k: "function" for k in FUNCTIONS}, **{k: "type" for k in (
    "StructDecl", "UnionDecl", "ClassDecl", "EnumDecl", "ClassTemplate", "ClassTemplatePartialSpecialization")},
    "TypedefDecl": "alias", "TypeAliasDecl": "alias", "TypeAliasTemplateDecl": "alias",
    "NamespaceAlias": "alias", "Namespace": "namespace", "FieldDecl": "field",
    "VarDecl": "variable", "ParmDecl": "variable", "EnumConstantDecl": "variable"}


class Clang:
    def __init__(self, filename=None):
        candidates = [filename] if filename else [
            "/Library/Developer/CommandLineTools/usr/lib/libclang.dylib",
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/libclang.dylib",
            "/usr/local/lib/libclang.so", "/usr/lib/libclang.so",
            *sorted(glob.glob("/usr/lib/llvm-*/lib/libclang.so*"), reverse=True),
            *sorted(glob.glob("/usr/lib/*-linux-gnu/libclang-*.so*"), reverse=True)]
        self.path = next((Path(p).resolve() for p in candidates if p and Path(p).is_file()), None)
        if self.path is None:
            raise ValueError("libclang unavailable; install Clang development libraries or configure --libclang PATH")
        self.lib = C.CDLL(str(self.path))
        specs = {
            "getClangVersion": (CXString, []), "getCString": (C.c_char_p, [CXString]), "disposeString": (None, [CXString]),
            "createIndex": (C.c_void_p, [C.c_int, C.c_int]), "disposeIndex": (None, [C.c_void_p]),
            "parseTranslationUnit2": (C.c_int, [C.c_void_p, C.c_char_p, C.POINTER(C.c_char_p), C.c_int, C.POINTER(Unsaved), C.c_uint, C.c_uint, C.POINTER(C.c_void_p)]),
            "disposeTranslationUnit": (None, [C.c_void_p]), "getTranslationUnitCursor": (Cursor, [C.c_void_p]),
            "visitChildren": (C.c_uint, [Cursor, VISITOR, C.c_void_p]),
            "getCursorKindSpelling": (CXString, [C.c_uint]), "getCursorSpelling": (CXString, [Cursor]),
            "getCursorDisplayName": (CXString, [Cursor]), "getCursorUSR": (CXString, [Cursor]),
            "getCanonicalCursor": (Cursor, [Cursor]), "getCursorReferenced": (Cursor, [Cursor]),
            "getSpecializedCursorTemplate": (Cursor, [Cursor]),
            "getCursorSemanticParent": (Cursor, [Cursor]), "isCursorDefinition": (C.c_uint, [Cursor]),
            "getCursorLocation": (Location, [Cursor]), "getCursorExtent": (Range, [Cursor]),
            "getRangeStart": (Location, [Range]), "getRangeEnd": (Location, [Range]),
            "getExpansionLocation": (None, [Location, C.POINTER(C.c_void_p), C.POINTER(C.c_uint), C.POINTER(C.c_uint), C.POINTER(C.c_uint)]),
            "getFileName": (CXString, [C.c_void_p]), "getCursorType": (Type, [Cursor]),
            "getFileContents": (C.c_void_p, [C.c_void_p, C.c_void_p, C.POINTER(C.c_size_t)]),
            "getCursorResultType": (Type, [Cursor]), "getTypeSpelling": (CXString, [Type]),
            "getTypeDeclaration": (Cursor, [Type]), "getCanonicalType": (Type, [Type]),
            "getPointeeType": (Type, [Type]), "getArrayElementType": (Type, [Type]),
            "getResultType": (Type, [Type]), "getNumArgTypes": (C.c_int, [Type]), "getArgType": (Type, [Type, C.c_uint]),
            "Type_getNumTemplateArguments": (C.c_int, [Type]), "Type_getTemplateArgumentAsType": (Type, [Type, C.c_uint]),
            "getTypedefDeclUnderlyingType": (Type, [Cursor]), "CXXMethod_isVirtual": (C.c_uint, [Cursor]),
            "getNumDiagnostics": (C.c_uint, [C.c_void_p]), "getDiagnostic": (C.c_void_p, [C.c_void_p, C.c_uint]),
            "getDiagnosticSeverity": (C.c_uint, [C.c_void_p]), "getDiagnosticSpelling": (CXString, [C.c_void_p]),
            "getDiagnosticLocation": (Location, [C.c_void_p]), "disposeDiagnostic": (None, [C.c_void_p]),
            "getInclusions": (None, [C.c_void_p, INCLUSION, C.c_void_p]),
        }
        for name, (restype, argtypes) in specs.items():
            function = getattr(self.lib, "clang_" + name)
            function.restype, function.argtypes = restype, argtypes
            setattr(self, name, function)
        self.version = self.string(self.getClangVersion())
        self._kinds = {}

    def string(self, value):
        try:
            return (self.getCString(value) or b"").decode("utf-8", errors="replace")
        finally:
            self.disposeString(value)

    def kind(self, cursor):
        if cursor.kind not in self._kinds:
            self._kinds[cursor.kind] = self.string(self.getCursorKindSpelling(cursor.kind))
        return self._kinds[cursor.kind]

    def spelling(self, cursor):
        return self.string(self.getCursorSpelling(cursor))

    def children(self, cursor):
        output = []
        @VISITOR
        def visit(child, parent, data):
            output.append(Cursor.from_buffer_copy(child))
            return 1  # CXChildVisit_Continue; explicit iteration avoids ctypes recursion.
        self.visitChildren(cursor, visit, None)
        return output

    def location(self, location):
        file, line, column, offset = C.c_void_p(), C.c_uint(), C.c_uint(), C.c_uint()
        self.getExpansionLocation(location, C.byref(file), C.byref(line), C.byref(column), C.byref(offset))
        filename = self.string(self.getFileName(file)) if file else ""
        return (str(Path(filename).resolve()) if filename else None, line.value, column.value, offset.value)


def file_hash(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def relative_source(path):
    p = Path(path)
    if not path or p.is_absolute() or any(part in (".", "..") for part in p.parts) or "\\" in path or any(ord(c) < 32 for c in path):
        raise ValueError("Source paths must be normal project-relative paths")
    if p.as_posix() != path:
        raise ValueError("Source path must be canonical")
    return path


def include_roots(flags, directory):
    """Logical search roots keep symlink identity in dependency paths."""
    roots = []
    pairs = {"-I", "-isystem", "-iquote", "-idirafter", "-isysroot", "--sysroot", "-F", "-iframework", "-resource-dir"}
    joined = ("--sysroot=", "-resource-dir=", "-isystem", "-iquote", "-idirafter", "-isysroot", "-I", "-F")
    i = 0
    while i < len(flags):
        value = None
        if flags[i] in pairs and i + 1 < len(flags):
            i += 1
            value = flags[i]
        else:
            for prefix in joined:
                if flags[i].startswith(prefix) and len(flags[i]) > len(prefix):
                    value = flags[i][len(prefix):]
                    break
        if value:
            path = Path(os.path.abspath(Path(directory) / value))
            if path.is_dir():
                roots.append((path, path.resolve()))
        i += 1
    return roots


def safe_flags(arguments, directory, source, from_database=False):
    """Allow parsing flags; discard build outputs and reject code/config loaders."""
    arguments = list(arguments)
    if from_database:
        if not arguments or arguments[0].startswith("-"):
            raise ValueError("Compilation command must start with a compiler executable")
        arguments = arguments[1:]
    output = []
    pairs = {"-I", "-D", "-U", "-F", "-isystem", "-iquote", "-idirafter", "-isysroot", "--sysroot", "-include", "-imacros", "-iframework", "-target", "--target", "-arch", "-resource-dir", "-x", "-stdlib"}
    discard_pairs = {"-o", "--output", "-MF", "-MT", "-MQ", "-MJ", "-dependency-file", "-serialize-diagnostics", "-L", "-l"}
    discard = {"-c", "-S", "-E", "-emit-llvm", "-fsyntax-only", "-M", "-MM", "-MD", "-MMD", "-MP", "-MG", "-shared", "-pie", "-rdynamic"}
    standalone = {"-nostdinc", "-nostdinc++", "-nobuiltininc", "-nostdlibinc", "-pthread", "-fno-exceptions", "-fexceptions", "-frtti", "-fno-rtti", "-ffreestanding", "-fno-builtin", "-fms-extensions", "-fms-compatibility", "-fdelayed-template-parsing", "-fno-delayed-template-parsing", "-fblocks", "-fno-blocks", "-ansi", "-pedantic", "-pedantic-errors", "-w", "-fPIC", "-fpic", "-fPIE", "-fpie", "-fno-common", "-fwrapv", "-fno-strict-aliasing", "-fno-omit-frame-pointer"}
    i = 0
    while i < len(arguments):
        flag = arguments[i]
        if not isinstance(flag, str) or "\0" in flag:
            raise ValueError("Compiler arguments must be strings without NUL")
        unsafe = ("-Xclang", "-Xpreprocessor", "-Xarch", "-mllvm", "-load", "-plugin", "-add-plugin", "-fplugin", "-fpass-plugin", "-ivfsoverlay", "-vfsoverlay", "-include-pch", "-include-pth", "-fmodule", "-fcxx-module", "--config")
        if flag.startswith("@") or any(flag.startswith(prefix) for prefix in unsafe):
            raise ValueError("Unsafe/unsupported compiler code-loading, response-file or precompiled-module flag: " + flag)
        if flag in pairs or flag in discard_pairs:
            i += 1
            if i >= len(arguments) or not isinstance(arguments[i], str) or arguments[i].startswith(("@", "-")) or "\0" in arguments[i]:
                raise ValueError("Missing/unsafe compiler option argument: " + flag)
            if flag in pairs:
                output += [flag, arguments[i]]
        elif flag in discard or flag.startswith(("-Wl,", "-Wa,", "-MF", "-MT", "-MQ", "-MJ", "--output=")):
            pass
        elif flag.startswith("-o") and flag != "-objc":
            pass
        elif flag in standalone or flag.startswith(("-I", "-D", "-U", "-F", "-std=", "-stdlib=", "--sysroot=", "--target=", "-isystem", "-iquote", "-idirafter", "-isysroot", "-resource-dir=", "-fvisibility=", "-fms-compatibility-version=")):
            output.append(flag)
        elif flag.startswith("-W") or (flag.startswith("-O") and flag[2:] in {"0", "1", "2", "3", "s", "z", "g", "fast"}) or flag in {"-g", "-g0", "-g1", "-g2", "-g3", "-gline-tables-only"}:
            output.append(flag)
        elif flag.startswith("-m") and flag not in {"-m", "-mllvm"}:
            output.append(flag)
        elif not flag.startswith("-") and str((Path(directory) / flag).resolve()) == str(Path(source).resolve()):
            pass
        else:
            raise ValueError("Unsupported compiler argument (configure explicit parse flags): " + flag)
        i += 1
    return output


class Graph:
    def __init__(self, request, clang):
        if request.get("schema_version") != 1 or request.get("language") not in TU_SUFFIXES:
            raise ValueError("Expected schema_version 1 and language c or cpp")
        self.language, self.clang = request["language"], clang
        self.root = Path(request["root"]).resolve(strict=True)
        self.options = request.get("options") or {}
        if not isinstance(self.options, dict):
            raise ValueError("options must be an object")
        self.sources = {}
        for item in request["files"]:
            relative = relative_source(item["path"])
            absolute = str(self.root / relative)
            contents = item["text"].encode("utf-8")
            if absolute in self.sources or hashlib.sha256(contents).hexdigest() != item["hash"]:
                raise ValueError("Duplicate or mismatched source hash: " + relative)
            self.sources[absolute] = (relative, contents)
        self.nodes, self.edges, self.inputs = {}, {}, {}
        self.diagnostics, self.unresolved, self.limitations = [], {}, [
            "Virtual calls represent the compiler's static target, not every possible runtime override.",
            "Indirect/dependent calls without one compiler-resolved function target are reported unresolved.",
            "This is static translation-unit analysis; it does not infer a build configuration or run project build scripts."]
        self.complete = True
        self.input(str(clang.path))

    def input(self, path):
        path = os.path.abspath(path)
        if path in self.sources:
            return
        try:
            self.inputs[path] = {"path": path, "hash": file_hash(path)}
        except OSError as error:
            self.diagnostic("error", "Unable to hash compiler input " + path + ": " + str(error))

    def diagnostic(self, severity, message, path=None, line=0):
        self.diagnostics.append({"severity": severity, "path": path, "line": line, "message": message})
        if severity == "error":
            self.complete = False

    def position(self, cursor):
        return self.clang.location(self.clang.getCursorLocation(cursor))

    def identifier(self, cursor):
        canonical = self.clang.getCanonicalCursor(cursor)
        usr = self.clang.string(self.clang.getCursorUSR(canonical))
        if not usr:
            path, _, _, offset = self.position(canonical)
            location = self.sources[path][0] if path in self.sources else (path or "builtin")
            usr = "local:" + location + ":" + str(offset) + ":" + self.clang.kind(canonical) + ":" + self.clang.spelling(canonical)
        return self.language + ":" + usr

    def qualified(self, cursor):
        parts = [self.clang.spelling(cursor) or "(anonymous)"]
        seen = set()
        parent = self.clang.getCursorSemanticParent(cursor)
        for _ in range(64):
            if self.clang.kind(parent) in {"TranslationUnit", "NoDeclFound", "InvalidFile"} or not parent.kind:
                break
            identity = self.identifier(parent)
            if identity in seen:
                break
            seen.add(identity)
            name = self.clang.spelling(parent)
            if name:
                parts.append(name)
            parent = self.clang.getCursorSemanticParent(parent)
        return "::".join(reversed(parts))

    def node(self, cursor):
        kind = self.clang.kind(cursor)
        if kind not in DECLARATIONS:
            return None
        identifier = self.identifier(cursor)
        path, line, column, _ = self.position(cursor)
        project = path in self.sources
        extent = self.clang.getCursorExtent(cursor)
        start_file, _, _, start = self.clang.location(self.clang.getRangeStart(extent))
        end_file, end_line, _, end = self.clang.location(self.clang.getRangeEnd(extent))
        if not project or start_file != path or end_file != path or not 0 <= start <= end <= len(self.sources[path][1]):
            start, end, end_line = 0, 0, 0
        spelling = self.clang.spelling(cursor)
        display = self.clang.string(self.clang.getCursorDisplayName(cursor)) or spelling
        ty = self.clang.string(self.clang.getTypeSpelling(self.clang.getCursorType(cursor)))
        qualified = self.qualified(cursor)
        signature = qualified + (" " + ty if ty else "")
        if display != spelling:
            signature += " [" + display + "]"
        candidate = {"id": identifier, "name": spelling or "(anonymous)", "qualified_name": qualified,
                     "kind": DECLARATIONS[kind], "signature": signature,
                     "definition": bool(self.clang.isCursorDefinition(cursor)), "external": not project,
                     "path": self.sources[path][0] if project else None,
                     "line": line if project else 0, "column": column if project else 0,
                     "end_line": end_line if project else 0, "start_byte": start, "end_byte": end}
        previous = self.nodes.get(identifier)
        rank = lambda n: (not n["external"], n["definition"], n["end_byte"] - n["start_byte"])
        if previous is None or rank(candidate) > rank(previous):
            self.nodes[identifier] = candidate
        if len(self.nodes) > 50_000:
            raise ValueError("Compiler graph exceeds 50000 nodes; select fewer translation units")
        # Specializations keep their concrete contract, while depending on the
        # compiler-selected template declaration. Reverse impact can therefore
        # reach specialization callers when the primary template changes.
        template = self.clang.getSpecializedCursorTemplate(cursor)
        if project and self.clang.kind(template) in DECLARATIONS and self.identifier(template) != identifier:
            target = self.node(template)
            self.edge(identifier, target, "references", cursor)
        return identifier

    def edge(self, source, target, kind, cursor, dispatch="static"):
        if not source or not target or source == target and kind in {"contains", "uses_type", "aliases"}:
            return
        path, line, column, _ = self.position(cursor)
        value = {"source": source, "target": target, "kind": kind,
                 "path": self.sources[path][0] if path in self.sources else None,
                 "line": line, "column": column, "dispatch": dispatch}
        key = (source, target, kind, value["path"] or "", line, column, dispatch)
        self.edges[key] = value
        if len(self.edges) > 200_000:
            raise ValueError("Compiler graph exceeds 200000 edges; select fewer translation units")

    def unknown(self, owner, cursor, reason, kind="calls"):
        path, line, _, offset = self.position(cursor)
        extent = self.clang.getCursorExtent(cursor)
        end_file, _, _, end = self.clang.location(self.clang.getRangeEnd(extent))
        expression = self.clang.spelling(cursor)
        if path in self.sources and end_file == path:
            expression = self.sources[path][1][offset:min(end, offset + 240)].decode("utf-8", errors="replace")
        value = {"source": owner, "kind": kind, "expression": expression[:160],
                 "path": self.sources[path][0] if path in self.sources else None, "line": line, "reason": reason}
        self.unresolved[(owner or "", value["path"] or "", line, offset, reason)] = value
        self.complete = False

    def types(self, owner, ty, cursor, relation="uses_type"):
        pending, seen = [ty], set()
        while pending:
            ty = pending.pop()
            if not ty.kind:
                continue
            spelling = self.clang.string(self.clang.getTypeSpelling(ty))
            key = (ty.kind, spelling)
            if key in seen:
                continue
            seen.add(key)
            target = self.node(self.clang.getTypeDeclaration(ty))
            if target:
                self.edge(owner, target, relation, cursor)
            pending += [self.clang.getPointeeType(ty), self.clang.getArrayElementType(ty), self.clang.getResultType(ty)]
            canonical = self.clang.getCanonicalType(ty)
            if (canonical.kind, self.clang.string(self.clang.getTypeSpelling(canonical))) != key:
                pending.append(canonical)
            count = self.clang.getNumArgTypes(ty)
            pending += [self.clang.getArgType(ty, i) for i in range(max(0, count))]
            count = self.clang.Type_getNumTemplateArguments(ty)
            pending += [self.clang.Type_getTemplateArgumentAsType(ty, i) for i in range(max(0, count))]

    def walk(self, tu):
        pending = [(self.clang.getTranslationUnitCursor(tu), None, None)]
        while pending:
            cursor, owner, function = pending.pop()
            kind = self.clang.kind(cursor)
            path, _, _, _ = self.position(cursor)
            if kind != "TranslationUnit" and path not in self.sources:
                continue  # External declarations are materialized only when referenced.
            current = self.node(cursor)
            if current:
                parent = self.node(self.clang.getCursorSemanticParent(cursor))
                self.edge(parent, current, "contains", cursor)
                self.types(current, self.clang.getCursorType(cursor), cursor)
                if owner and owner != current and kind in {"ParmDecl", "VarDecl", "FieldDecl"}:
                    self.types(owner, self.clang.getCursorType(cursor), cursor)
                if kind in FUNCTIONS:
                    self.types(current, self.clang.getCursorResultType(cursor), cursor)
                    function = current
                if kind in {"TypedefDecl", "TypeAliasDecl"}:
                    self.types(current, self.clang.getTypedefDeclUnderlyingType(cursor), cursor, "aliases")
                owner = current
            if kind == "CallExpr":
                referenced = self.clang.getCursorReferenced(cursor)
                target = self.node(referenced)
                if self.clang.kind(referenced) in FUNCTIONS and target:
                    dispatch = "virtual" if self.clang.kind(referenced) == "CXXMethod" and self.clang.CXXMethod_isVirtual(referenced) else "static"
                    self.edge(function or owner, target, "calls", cursor, dispatch)
                else:
                    self.unknown(function or owner, cursor, "No unique compiler-resolved function target (indirect pointer, dependent expression or invalid source)")
            elif kind in {"TypeRef", "TemplateRef", "C++ base class specifier", "CXXBaseSpecifier", "NamespaceRef", "DeclRefExpr", "MemberRefExpr", "OverloadedDeclRef"}:
                referenced = self.clang.getCursorReferenced(cursor)
                target = self.node(referenced)
                if target:
                    relation = "inherits" if kind in {"C++ base class specifier", "CXXBaseSpecifier"} else "uses_type" if kind in {"TypeRef", "TemplateRef"} else "references"
                    if kind == "NamespaceRef" and owner and self.nodes[owner]["kind"] == "alias":
                        relation = "aliases"
                    self.edge(function or owner, target, relation, cursor)
                elif kind == "OverloadedDeclRef":
                    self.unknown(function or owner, cursor, "Overloaded reference has no unique compiler-selected declaration", "references")
            pending.extend((child, owner, function) for child in reversed(self.clang.children(cursor)))

    def compile_database(self):
        filename = self.options.get("compile_commands")
        if not filename:
            return {}
        filename = Path(filename)
        if not filename.is_absolute():
            filename = self.root / filename
        self.input(str(filename))
        records = json.loads(filename.read_bytes())
        if not isinstance(records, list):
            raise ValueError("compile_commands must contain a JSON array")
        output = {}
        for record in records:
            directory = Path(record["directory"])
            if not directory.is_absolute():
                directory = filename.parent / directory
            directory = directory.resolve(strict=True)
            source = str((directory / record["file"]).resolve())
            arguments = record.get("arguments")
            if arguments is None:
                arguments = shlex.split(record["command"], posix=True)
            if not isinstance(arguments, list) or not all(isinstance(a, str) for a in arguments):
                raise ValueError("Compilation database arguments must be strings")
            if source in output:
                self.diagnostic("warning", "Multiple compilation commands; using the first for " + source)
                self.complete = False
            else:
                output[source] = (directory, arguments)
        return output

    def analyze(self):
        database = self.compile_database()
        selected = self.options.get("files") or [relative for relative, _ in self.sources.values() if Path(relative).suffix in TU_SUFFIXES[self.language]]
        if not isinstance(selected, list) or not all(isinstance(s, str) for s in selected):
            raise ValueError("options.files must be a list of relative translation-unit paths")
        if not selected:
            self.diagnostic("error", "No translation units selected. Headers need an explicit --file translation unit and its compile flags.")
        options_flags = self.options.get("flags") or []
        if not isinstance(options_flags, list) or not all(isinstance(a, str) for a in options_flags):
            raise ValueError("options.flags must be a string array")
        if not database and not options_flags:
            self.diagnostic("warning", "No compile database or explicit flags: default language configuration may differ from the real build")
            self.complete = False
        definitions = self.options.get("defines") or []
        if not isinstance(definitions, list) or not all(isinstance(d, str) and d and "\0" not in d and "\n" not in d for d in definitions):
            raise ValueError("defines must be nonempty single-line strings")
        encoded = [(p.encode(), content) for p, (_, content) in self.sources.items()]
        overlays = (Unsaved * len(encoded))(*(Unsaved(p, content, len(content)) for p, content in encoded))
        index = self.clang.createIndex(0, 0)
        try:
            for relative in sorted(set(selected)):
                relative_source(relative)
                absolute = str(self.root / relative)
                if absolute not in self.sources:
                    self.diagnostic("error", "Selected translation unit is absent from indexed inputs: " + relative, relative)
                    continue
                if Path(relative).suffix not in TU_SUFFIXES[self.language] | HEADERS:
                    self.diagnostic("error", "Unsupported translation-unit suffix: " + relative, relative)
                    continue
                if Path(relative).suffix in HEADERS:
                    self.diagnostic("warning", "Header parsed as an explicit standalone translation unit; enclosing build macro context is not inferred", relative)
                    self.complete = False
                directory, raw = database.get(absolute, (self.root, []))
                if database and absolute not in database:
                    self.diagnostic("warning", "Selected translation unit has no compilation database entry: " + relative, relative)
                    self.complete = False
                try:
                    flags = safe_flags(raw, directory, absolute, bool(raw)) + safe_flags(options_flags, directory, absolute)
                except ValueError as error:
                    self.diagnostic("error", str(error), relative)
                    continue
                flags = ["-x", "c" if self.language == "c" else "c++", *flags, *("-D" + d for d in definitions)]
                if sys.platform == "darwin" and not any(f.startswith(("-isysroot", "--sysroot")) for f in flags):
                    # libclang is not the xcrun driver: explicitly select the
                    # installed SDK next to this toolchain, unless configured.
                    candidates = [self.clang.path.parents[2] / "SDKs/MacOSX.sdk",
                                  Path("/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk")]
                    sdk = next((p for p in candidates if p.is_dir()), None)
                    if sdk:
                        flags += ["-isysroot", str(sdk)]
                        limitation = "Unconfigured macOS sysroot defaults to the installed SDK: " + str(sdk)
                        if limitation not in self.limitations:
                            self.limitations.append(limitation)
                # Disable implicit module construction; no target project artifacts.
                flags += ["-fno-modules", "-fno-implicit-modules", "-fno-implicit-module-maps"]
                roots = include_roots(flags, directory)
                argv = (C.c_char_p * len(flags))(*(f.encode() for f in flags))
                tu = C.c_void_p()
                previous = Path.cwd()
                try:
                    os.chdir(directory)
                    status = self.clang.parseTranslationUnit2(index, absolute.encode(), argv, len(flags), overlays, len(overlays), 0x200, C.byref(tu))
                    if status or not tu:
                        self.diagnostic("error", "libclang could not parse translation unit (error " + str(status) + ")", relative)
                        continue
                    for i in range(self.clang.getNumDiagnostics(tu)):
                        diagnostic = self.clang.getDiagnostic(tu, i)
                        try:
                            severity = self.clang.getDiagnosticSeverity(diagnostic)
                            path, line, _, _ = self.clang.location(self.clang.getDiagnosticLocation(diagnostic))
                            if severity:
                                self.diagnostic("error" if severity >= 3 else "warning" if severity == 2 else "note", self.clang.string(self.clang.getDiagnosticSpelling(diagnostic)), self.sources[path][0] if path in self.sources else None, line)
                        finally:
                            self.clang.disposeDiagnostic(diagnostic)
                    included = {}
                    @INCLUSION
                    def inclusion(file, stack, count, data):
                        name = self.clang.string(self.clang.getFileName(file))
                        if name:
                            included[os.path.abspath(name)] = file
                    self.clang.getInclusions(tu, inclusion, None)
                    for path in sorted(included):
                        actual = str(Path(path).resolve())
                        if actual not in self.sources:
                            size = C.c_size_t()
                            contents = self.clang.getFileContents(tu, included[path], C.byref(size))
                            if contents or size.value == 0 and Path(path).stat().st_size == 0:
                                parsed_hash = hashlib.sha256(C.string_at(contents, size.value)).hexdigest()
                                paths = {path, actual}
                                for logical_root, actual_root in roots:
                                    if Path(actual).is_relative_to(actual_root):
                                        alias = logical_root / Path(actual).relative_to(actual_root)
                                        if alias.is_file() and alias.resolve() == Path(actual):
                                            paths.add(str(alias))
                                for dependency in sorted(paths):
                                    if dependency in self.inputs and self.inputs[dependency]["hash"] != parsed_hash:
                                        self.diagnostic("error", "Compiler input changed between translation units: " + dependency)
                                    else:
                                        self.inputs[dependency] = {"path": dependency, "hash": parsed_hash}
                                    try:
                                        if file_hash(dependency) != parsed_hash:
                                            self.diagnostic("error", "Compiler input changed during analysis: " + dependency)
                                    except OSError:
                                        self.diagnostic("error", "Compiler input disappeared during analysis: " + dependency)
                            else:
                                self.diagnostic("error", "Compiler input buffer unavailable for provenance: " + path)
                            if Path(actual).is_relative_to(self.root):
                                self.diagnostic("warning", "Compiler used a project header absent from indexed input; only external contracts are exposed: " + str(Path(actual).relative_to(self.root)), relative)
                                self.complete = False
                    self.walk(tu)
                finally:
                    os.chdir(previous)
                    if tu:
                        self.clang.disposeTranslationUnit(tu)
        finally:
            self.clang.disposeIndex(index)
        return self.response()

    def response(self):
        diagnostics = {json.dumps(d, sort_keys=True): d for d in self.diagnostics}
        return {"schema_version": 1, "language": self.language,
                "analyzer": {"name": "libclang", "version": self.clang.version,
                             "capabilities": ["symbols", "calls", "references", "uses_type", "inherits", "aliases"]},
                "nodes": sorted(self.nodes.values(), key=lambda n: n["id"]),
                "edges": [value for _, value in sorted(self.edges.items())],
                "diagnostics": [value for _, value in sorted(diagnostics.items())],
                "unresolved": [value for _, value in sorted(self.unresolved.items())],
                "inputs": [value for _, value in sorted(self.inputs.items())],
                "analysis_complete": self.complete, "limitations": self.limitations}


def analyze(request):
    clang = Clang((request.get("options") or {}).get("libclang"))
    return Graph(request, clang).analyze()


def main():
    request = {}
    try:
        raw = sys.stdin.buffer.read(32 * 1024 * 1024 + 1)
        if len(raw) > 32 * 1024 * 1024:
            raise ValueError("Analyzer request exceeds 32 MiB; select a smaller source set")
        request = json.loads(raw)
        response = analyze(request)
        output = json.dumps(response, ensure_ascii=False, separators=(",", ":"))
        if len(output.encode()) > 8 * 1024 * 1024:
            raise ValueError("Compiler graph exceeds 8 MiB; select fewer translation units")
    except Exception as error:
        response = {"schema_version": 1, "language": request.get("language", "cpp") if isinstance(request, dict) else "cpp",
                    "analyzer": {"name": "libclang", "version": "unavailable", "capabilities": []},
                    "nodes": [], "edges": [], "diagnostics": [{"severity": "error", "path": None, "line": 0, "message": str(error)}],
                    "unresolved": [], "inputs": [], "analysis_complete": False,
                    "limitations": ["Analysis failed explicitly; no lexical graph was substituted."]}
        output = json.dumps(response, ensure_ascii=False, separators=(",", ":"))
    print(output)


if __name__ == "__main__":
    main()
