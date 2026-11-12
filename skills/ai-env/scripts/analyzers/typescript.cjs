'use strict';
// Compiler API only: no project imports, plugins, transforms, or emit.
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const ts = require('../vendor/typescript/lib/typescript.js');
const digest = value => crypto.createHash('sha256').update(value).digest('hex');

function analyze(req) {
  if (!['typescript', 'javascript'].includes(req.language)) throw Error('Unsupported TypeScript helper language');
  const root = path.resolve(req.root), lang = req.language, spec = req.options || {};
  const overlays = new Map(), inputs = new Map(), diagnostics = [], unresolved = [], limitations = [];
  const absolute = name => path.resolve(root, name);
  const relative = name => path.relative(root, name).split(path.sep).join('/');
  const inside = name => name === root || name.startsWith(root + path.sep);
  const compilerRoot = path.resolve(__dirname, '../vendor/typescript') + path.sep;
  const mayReadDependency = name => name.startsWith(compilerRoot) || name.includes(path.sep + 'node_modules' + path.sep);
  const isSource = name => /\.(?:[cm]?tsx?|[cm]?jsx?)$/.test(name);
  for (const f of req.files || []) {
    const full = absolute(f.path);
    if (!inside(full)) throw Error('Source path escapes project');
    if (digest(Buffer.from(f.text)) !== f.hash) throw Error('Supplied source hash mismatch: ' + f.path);
    overlays.set(full, f.text);
  }
  function readFile(name) {
    name = absolute(name);
    if (overlays.has(name)) return overlays.get(name);
    // Project implementation source must come from the authoritative snapshot.
    if (inside(name) && isSource(name) && !mayReadDependency(name)) return undefined;
    try {
      const data = fs.readFileSync(name);
      inputs.set(name, digest(data));
      return data.toString('utf8');
    } catch (_) { return undefined; }
  }
  function exists(name) {
    name = absolute(name);
    if (overlays.has(name)) return true;
    if (inside(name) && isSource(name) && !mayReadDependency(name)) return false;
    try { return fs.statSync(name).isFile(); } catch (_) { return false; }
  }
  const dirs = new Set([root]);
  for (const name of overlays.keys()) {
    let dir = path.dirname(name);
    while (inside(dir)) { dirs.add(dir); if (dir === root) break; dir = path.dirname(dir); }
  }
  const directoryExists = name => dirs.has(absolute(name)) || ts.sys.directoryExists(absolute(name));
  const sourceNames = [...overlays.keys()].filter(isSource);
  let options = {target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
    moduleResolution: ts.ModuleResolutionKind.Node10, strict: true, allowJs: true,
    checkJs: true, noImplicitAny: lang === 'typescript', skipLibCheck: true, noEmit: true};
  let roots = sourceNames;
  const config = spec.project ? absolute(spec.project) : ['tsconfig.json', 'jsconfig.json'].map(absolute).find(exists);
  const errorRanges = [];
  function diagnostic(value) {
    const sf = value.file, full = sf ? absolute(sf.fileName) : null;
    const point = sf && value.start !== undefined ? sf.getLineAndCharacterOfPosition(value.start) : null;
    if (value.category === ts.DiagnosticCategory.Error && sf && value.start !== undefined) errorRanges.push({file: full, start: value.start, end: value.start + (value.length || 1)});
    diagnostics.push({severity: value.category === ts.DiagnosticCategory.Error ? 'error' : value.category === ts.DiagnosticCategory.Warning ? 'warning' : 'info',
      path: full && overlays.has(full) ? relative(full) : null, line: point ? point.line + 1 : 0,
      message: 'TS' + value.code + ': ' + ts.flattenDiagnosticMessageText(value.messageText, '\n')});
  }
  if (config) {
    const text = readFile(config);
    if (text === undefined) throw Error('Project configuration unavailable: ' + config);
    const parsed = ts.parseConfigFileTextToJson(config, text);
    if (parsed.error) diagnostic(parsed.error);
    const host = {useCaseSensitiveFileNames: true, readFile, fileExists: exists, directoryExists,
      readDirectory: (base, extensions, excludes, includes) => ts.matchFiles(base, extensions, excludes, includes, true, root, undefined,
        directory => { const prefix = absolute(directory) + path.sep; return {
          files: [...overlays.keys()].filter(n => path.dirname(n) === absolute(directory)).map(n => path.basename(n)),
          directories: [...dirs].filter(n => path.dirname(n) === absolute(directory) && n !== absolute(directory)).map(n => path.basename(n))}; }, p => absolute(p))};
    const result = ts.parseJsonConfigFileContent(parsed.config || {}, host, path.dirname(config), undefined, config);
    result.errors.forEach(diagnostic);
    options = {...options, ...result.options, noEmit: true, allowJs: true, checkJs: true};
    roots = result.fileNames.filter(n => overlays.has(absolute(n)) && isSource(n));
    if (result.projectReferences?.length) limitations.push('Project references are not built or followed; supply their source snapshots or declaration dependencies.');
    if (options.plugins?.length) limitations.push('Language-service plugins are ignored and never loaded.');
    delete options.plugins;
  }
  if (spec.flags?.length || spec.references?.length || spec.defines?.length) throw Error('Use static tsconfig/jsconfig compilerOptions; flags/references/defines are unsupported for TypeScript');
  if (spec.files?.length) {
    roots = spec.files.map(absolute);
    for (const name of roots) if (!overlays.has(name) || !isSource(name)) throw Error('Selected source not supplied: ' + relative(name));
  }
  if (!roots.length) throw Error('No TypeScript/JavaScript source roots in supplied snapshot/configuration');
  const host = ts.createCompilerHost(options, true);
  host.readFile = readFile; host.fileExists = exists; host.directoryExists = directoryExists;
  host.getCurrentDirectory = () => root;
  host.getSourceFile = (name, target) => { const text = readFile(name); return text === undefined ? undefined : ts.createSourceFile(name, text, target, true); };
  host.writeFile = () => { throw Error('Analysis must not emit files'); };
  host.realpath = name => absolute(name);
  const program = ts.createProgram(roots, options, host), checker = program.getTypeChecker();
  ts.getPreEmitDiagnostics(program).forEach(diagnostic);
  let typeBudgetExceeded = false;
  const nodes = new Map(), edges = new Map(), declNodes = new Map(), byIdDecl = new Map();
  const fmt = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope;
  const projectFile = sf => overlays.has(absolute(sf.fileName)) && isSource(sf.fileName);
  function declarationKind(d) {
    if (ts.isFunctionDeclaration(d) || ts.isMethodDeclaration(d) || ts.isMethodSignature(d) || ts.isConstructorDeclaration(d) || ts.isGetAccessorDeclaration(d) || ts.isSetAccessorDeclaration(d)) return 'function';
    if (ts.isVariableDeclaration(d) && d.initializer && (ts.isArrowFunction(d.initializer) || ts.isFunctionExpression(d.initializer))) return 'function';
    if (ts.isClassDeclaration(d) || ts.isInterfaceDeclaration(d) || ts.isEnumDeclaration(d) || ts.isTypeParameterDeclaration(d)) return 'type';
    if (ts.isJSDocTypedefTag(d) || ts.isJSDocCallbackTag(d)) return 'alias';
    if (ts.isTypeAliasDeclaration(d) || ts.isImportSpecifier(d) || ts.isNamespaceImport(d) || ts.isImportEqualsDeclaration(d) || (ts.isImportClause(d) && d.name)) return 'alias';
    if (ts.isPropertyDeclaration(d) || ts.isPropertySignature(d) || ts.isEnumMember(d) || ts.isPropertyAssignment(d)) return 'field';
    if (ts.isVariableDeclaration(d)) return 'variable';
    if (ts.isModuleDeclaration(d)) return 'namespace';
    return null;
  }
  function normal(d) {
    if (d && (ts.isArrowFunction(d) || ts.isFunctionExpression(d)) && ts.isVariableDeclaration(d.parent)) return d.parent;
    return d;
  }
  function qualified(d) {
    const names = []; let n = d;
    while (n && !ts.isSourceFile(n)) { if (n.name && (ts.isIdentifier(n.name) || ts.isStringLiteral(n.name) || ts.isNumericLiteral(n.name))) names.unshift(n.name.text); else if (ts.isConstructorDeclaration(n)) names.unshift('constructor'); n = n.parent; }
    return names.join('.') || '<anonymous>';
  }
  function signature(d, symbol) {
    const callable = d.initializer && (ts.isArrowFunction(d.initializer) || ts.isFunctionExpression(d.initializer)) ? d.initializer : d;
    if (ts.isFunctionLike(callable)) {
      const sig = checker.getSignatureFromDeclaration(callable);
      return qualified(d) + (sig ? checker.signatureToString(sig, callable, fmt) : '(?)');
    }
    if (ts.isJSDocTypedefTag(d) || ts.isJSDocCallbackTag(d)) return '@typedef ' + qualified(d) + ' = ' + (d.typeExpression?.getText() || '?');
    if (ts.isTypeAliasDeclaration(d)) return 'type ' + d.name.text + (d.typeParameters ? '<' + d.typeParameters.map(t => t.getText()).join(', ') + '>' : '') + ' = ' + d.type.getText();
    if (ts.isClassDeclaration(d) || ts.isInterfaceDeclaration(d) || ts.isEnumDeclaration(d)) return (ts.isInterfaceDeclaration(d) ? 'interface ' : ts.isEnumDeclaration(d) ? 'enum ' : 'class ') + qualified(d) + (d.typeParameters ? '<' + d.typeParameters.map(t => t.getText()).join(', ') + '>' : '') + (d.heritageClauses ? ' ' + d.heritageClauses.map(h => h.getText()).join(' ') : '');
    if (symbol) return qualified(d) + ': ' + checker.typeToString(checker.getTypeOfSymbolAtLocation(symbol, d), d, fmt);
    return d.getText();
  }
  function nodeFor(d) {
    d = normal(d); if (!d || !declarationKind(d)) return null;
    if (declNodes.has(d)) return declNodes.get(d);
    const sf = d.getSourceFile(), local = projectFile(sf), kind = declarationKind(d);
    const symbol = d.name ? checker.getSymbolAtLocation(d.name) : null;
    const q = qualified(d), sig = signature(d, symbol), start = d.getStart(sf), end = d.end;
    const key = (local ? relative(sf.fileName) : absolute(sf.fileName)) + ':' + q + ':' + kind + ':' + sig;
    let id = lang + ':' + digest(key);
    // Repeated declarations with identical contracts merge; distinct local shadowing keeps its scope location.
    if (nodes.has(id) && byIdDecl.get(id) !== d && byIdDecl.get(id).parent !== d.parent) id += ':' + sf.getLineAndCharacterOfPosition(start).line;
    const point = sf.getLineAndCharacterOfPosition(start), endPoint = sf.getLineAndCharacterOfPosition(end);
    const result = {id, name: d.name?.getText(sf) || (ts.isConstructorDeclaration(d) ? 'constructor' : '<anonymous>'), qualified_name: q, kind,
      signature: sig, definition: Boolean(d.body || kind !== 'function'), external: !local, implicit: false,
      path: local ? relative(sf.fileName) : null, line: local ? point.line + 1 : 0, column: local ? point.character + 1 : 0,
      end_line: local ? endPoint.line + 1 : 0, start_byte: local ? Buffer.byteLength(sf.text.slice(0, start)) : 0,
      end_byte: local ? Buffer.byteLength(sf.text.slice(0, end)) : 0};
    if (!nodes.has(id) || result.definition) { nodes.set(id, result); byIdDecl.set(id, d); }
    declNodes.set(d, result); return result;
  }
  function owner(n) {
    for (let p = n; p; p = p.parent) {
      if (ts.isSourceFile(p)) {
        if (!projectFile(p)) return null;
        const name = relative(p.fileName), id = lang + ':' + digest('module:' + name);
        if (!nodes.has(id)) nodes.set(id, {id, name: '<module>', qualified_name: name + '.<module>', kind: 'namespace',
          signature: 'module ' + name, definition: true, external: false, implicit: true, path: null,
          line: 0, column: 0, end_line: 0, start_byte: 0, end_byte: 0});
        return nodes.get(id);
      }
      const result = nodeFor(p); if (result) return result;
    }
    return null;
  }
  function edge(source, target, kind, loc, dispatch = 'static', allowSelf = false) {
    if (!source || !target || (source.id === target.id && kind !== 'calls' && kind !== 'references' && !allowSelf)) return;
    const sf = loc.getSourceFile(); if (!projectFile(sf)) return;
    const point = sf.getLineAndCharacterOfPosition(loc.getStart(sf));
    const e = {source: source.id, target: target.id, kind, path: relative(sf.fileName), line: point.line + 1, column: point.character + 1, dispatch};
    edges.set(JSON.stringify(e), e);
  }
  function resolveSymbol(symbol) {
    if (!symbol) return null;
    if (symbol.flags & ts.SymbolFlags.Alias) { try { return checker.getAliasedSymbol(symbol); } catch (_) { return null; } }
    return symbol;
  }
  function symbolTarget(symbol) {
    symbol = resolveSymbol(symbol);
    const declarations = symbol?.declarations || [];
    return declarations.map(normal).map(nodeFor).find(Boolean) || null;
  }
  function typeEdges(source, type, loc, seen = new Set()) {
    if (!type || seen.has(type)) return;
    if (seen.size >= 100) {
      if (!typeBudgetExceeded) {
        const sf = loc.getSourceFile(), point = sf.getLineAndCharacterOfPosition(loc.getStart());
        diagnostics.push({severity: 'warning', path: relative(sf.fileName), line: point.line + 1,
          message: 'Type dependency traversal exceeded 100 distinct types at one declaration; type edges are truncated.'});
        limitations.push('Type dependency traversal was truncated at 100 distinct types per declaration; omitted relations are unknown.');
      }
      typeBudgetExceeded = true;
      return;
    }
    seen.add(type);
    edge(source, symbolTarget(type.aliasSymbol || type.symbol), 'uses_type', loc);
    if (type.aliasTypeArguments) for (const t of type.aliasTypeArguments) typeEdges(source, t, loc, seen);
    if (type.typeArguments) for (const t of type.typeArguments) typeEdges(source, t, loc, seen);
    if (type.types) for (const t of type.types) typeEdges(source, t, loc, seen);
  }
  const projectSources = program.getSourceFiles().filter(projectFile);
  function collect(n) { nodeFor(n); for (const doc of n.jsDoc || []) for (const tag of doc.tags || []) nodeFor(tag); ts.forEachChild(n, collect); }
  projectSources.forEach(collect);
  function walk(n) {
    const current = nodeFor(n), source = current || owner(n.parent);
    for (const doc of n.jsDoc || []) for (const tag of doc.tags || []) { const alias = nodeFor(tag); if (alias && tag.typeExpression?.type) typeEdges(alias, checker.getTypeFromTypeNode(tag.typeExpression.type), tag); }
    if (current) {
      edge(owner(n.parent), current, 'contains', n);
      // Calls retain the selected overload signature. Its implementation is a
      // separate compiler declaration, linked so implementation impact includes
      // callers through every declared overload without inventing call edges.
      if (ts.isFunctionLike(n) && !n.body) {
        const symbol = n.name ? checker.getSymbolAtLocation(n.name) : null;
        const declarations = ts.isConstructorDeclaration(n) ? n.parent.members.filter(ts.isConstructorDeclaration) : symbol?.declarations;
        const implementation = declarations?.find(d => ts.isFunctionLike(d) && d.body);
        if (implementation) edge(current, nodeFor(implementation), 'references', n);
      }
      if (n.type) typeEdges(current, checker.getTypeFromTypeNode(n.type), n.type);
      if (n.typeParameters) for (const p of n.typeParameters) if (p.constraint) typeEdges(current, checker.getTypeFromTypeNode(p.constraint), p);
      if (ts.isVariableDeclaration(n) || ts.isPropertyDeclaration(n) || ts.isPropertySignature(n)) typeEdges(current, checker.getTypeAtLocation(n), n);
      const callable = n.initializer && (ts.isArrowFunction(n.initializer) || ts.isFunctionExpression(n.initializer)) ? n.initializer : n;
      if (ts.isFunctionLike(callable)) {
        for (const p of callable.parameters) typeEdges(current, checker.getTypeAtLocation(p), p);
        const sig = checker.getSignatureFromDeclaration(callable); if (sig) typeEdges(current, checker.getReturnTypeOfSignature(sig), callable);
      }
      if (n.heritageClauses) for (const h of n.heritageClauses) for (const t of h.types) { const type = checker.getTypeAtLocation(t); edge(current, symbolTarget(type.symbol), 'inherits', t, 'virtual'); typeEdges(current, type, t); }
      if (current.kind === 'alias') {
        let sym = n.name ? checker.getSymbolAtLocation(n.name) : null;
        if (ts.isTypeAliasDeclaration(n)) { const t = checker.getTypeFromTypeNode(n.type); edge(current, symbolTarget(t.symbol), 'aliases', n.type); typeEdges(current, t, n.type); }
        else edge(current, symbolTarget(sym), 'aliases', n);
      }
    }
    if (ts.isCallExpression(n) || ts.isNewExpression(n)) {
      const origin = owner(n.parent), exprType = checker.getTypeAtLocation(n.expression), sig = checker.getResolvedSignature(n);
      let declaration = normal(sig?.declaration), target = nodeFor(declaration);
      const implicitClass = ts.isNewExpression(n) && (!declaration || ts.isClassDeclaration(declaration)) ? (ts.isClassDeclaration(declaration || {}) ? declaration : exprType.symbol?.declarations?.find(ts.isClassDeclaration)) : null;
      if (implicitClass && sig) {
        const classNode = nodeFor(implicitClass), id = lang + ':' + digest(classNode.id + ':implicit-constructor:' + checker.signatureToString(sig));
        target = {id, name: 'constructor', qualified_name: classNode.qualified_name + '.constructor', kind: 'function',
          signature: classNode.qualified_name + '.new' + checker.signatureToString(sig), definition: true,
          external: classNode.external, implicit: true, path: null, line: 0, column: 0, end_line: 0, start_byte: 0, end_byte: 0};
        nodes.set(id, target);
      }
      const unionTargets = exprType.isUnion() ? new Set(exprType.types.flatMap(t => checker.getSignaturesOfType(t, ts.isNewExpression(n) ? ts.SignatureKind.Construct : ts.SignatureKind.Call).map(s => s.declaration)).filter(Boolean)).size : 0;
      const errorHere = errorRanges.some(d => d.file === absolute(n.getSourceFile().fileName) && d.start < n.end && d.end > n.getStart());
      if (!origin || !target || unionTargets > 1 || exprType.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown) || errorHere) {
        unresolved.push({source: origin?.id || null, kind: 'calls', expression: n.expression.getText().slice(0, 512),
          path: relative(n.getSourceFile().fileName), line: n.getSourceFile().getLineAndCharacterOfPosition(n.getStart()).line + 1,
          reason: errorHere ? 'Compiler diagnostic intersects this call; no candidate selected.' : unionTargets > 1 ? 'Union receiver has multiple possible callable declarations.' : 'Dynamic/callback call has no unique concrete declaration.'});
      } else edge(origin, target, 'calls', n, declaration && (ts.isMethodDeclaration(declaration) || ts.isMethodSignature(declaration)) ? 'virtual' : 'static');
    }
    if (ts.isTypeReferenceNode(n) && source) {
      const target = symbolTarget(checker.getSymbolAtLocation(n.typeName));
      if (target?.id === source.id) edge(source, target, 'uses_type', n, 'static', true);
    }
    if (ts.isIdentifier(n) && n !== n.parent.name) edge(source, symbolTarget(checker.getSymbolAtLocation(n)), 'references', n);
    ts.forEachChild(n, walk);
  }
  projectSources.forEach(walk);
  if (edges.size && [...edges.values()].some(e => e.dispatch === 'virtual')) limitations.push('Method dispatch targets the compiler-selected declaration; runtime overrides are not exhaustively enumerated.');
  if (lang === 'javascript') limitations.push('JavaScript uses TypeScript static/JSDoc inference; dynamic runtime mutation cannot be proven complete.');
  if (unresolved.length) limitations.push('Unresolved calls are explicit; omitted runtime targets must not be treated as absent.');
  for (const file of [__filename, require.resolve('../vendor/typescript/lib/typescript.js')]) inputs.set(file, digest(fs.readFileSync(file)));
  return {schema_version: 1, language: lang, analyzer: {name: 'typescript-compiler', version: ts.version,
    capabilities: ['symbols', 'calls', 'references', 'uses_type', 'inherits', 'aliases']},
    nodes: [...nodes.values()].sort((a,b) => a.id.localeCompare(b.id)), edges: [...edges.values()], diagnostics, unresolved,
    inputs: [...inputs].map(([name, hash]) => ({path: name, hash})),
    analysis_complete: !typeBudgetExceeded && !diagnostics.some(d => d.severity === 'error') && !unresolved.length && !limitations.some(x => x.startsWith('Project references')), limitations};
}

try { process.stdout.write(JSON.stringify(analyze(JSON.parse(fs.readFileSync(0, 'utf8')))) + '\n'); }
catch (error) { process.stderr.write(String(error.stack || error) + '\n'); process.exitCode = 1; }
