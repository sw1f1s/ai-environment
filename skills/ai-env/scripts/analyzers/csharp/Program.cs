using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Operations;
using Microsoft.CodeAnalysis.Text;

record Node(string id, string name, string qualified_name, string kind, string signature,
    bool definition, bool external, bool @implicit, string? path, int line, int column, int end_line, int start_byte, int end_byte);
record Edge(string source, string target, string kind, string path, int line, int column, string dispatch);
record Note(string severity, string? path, int line, string message);
record Unknown(string? source, string kind, string expression, string path, int line, string reason);
record Input(string path, string hash);
record FileData(string Path, string Text, int[] Bytes);

sealed class Analyzer
{
    readonly Dictionary<string, Node> nodes = new(StringComparer.Ordinal);
    readonly HashSet<Edge> edges = new();
    readonly List<Note> diagnostics = new();
    readonly HashSet<Unknown> unresolved = new();
    readonly SortedDictionary<string, Input> inputs = new(StringComparer.Ordinal);
    readonly Dictionary<SyntaxTree, FileData> files = new();
    CSharpCompilation compilation = null!;
    bool partial;
    static readonly SymbolDisplayFormat Signature = new(
        typeQualificationStyle: SymbolDisplayTypeQualificationStyle.NameAndContainingTypesAndNamespaces,
        genericsOptions: SymbolDisplayGenericsOptions.IncludeTypeParameters,
        memberOptions: SymbolDisplayMemberOptions.IncludeContainingType | SymbolDisplayMemberOptions.IncludeParameters | SymbolDisplayMemberOptions.IncludeType | SymbolDisplayMemberOptions.IncludeModifiers | SymbolDisplayMemberOptions.IncludeAccessibility | SymbolDisplayMemberOptions.IncludeConstantValue | SymbolDisplayMemberOptions.IncludeExplicitInterface,
        parameterOptions: SymbolDisplayParameterOptions.IncludeType | SymbolDisplayParameterOptions.IncludeName | SymbolDisplayParameterOptions.IncludeParamsRefOut,
        miscellaneousOptions: SymbolDisplayMiscellaneousOptions.UseSpecialTypes | SymbolDisplayMiscellaneousOptions.EscapeKeywordIdentifiers);
    static readonly SymbolDisplayFormat Qualified = new(
        typeQualificationStyle: SymbolDisplayTypeQualificationStyle.NameAndContainingTypesAndNamespaces,
        genericsOptions: SymbolDisplayGenericsOptions.IncludeTypeParameters,
        memberOptions: SymbolDisplayMemberOptions.IncludeContainingType,
        miscellaneousOptions: SymbolDisplayMiscellaneousOptions.EscapeKeywordIdentifiers);
    static string Hash(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant();
    static string[] Strings(JsonElement options, string name) => options.TryGetProperty(name, out var field) && field.ValueKind == JsonValueKind.Array
        ? field.EnumerateArray().Select(item => item.GetString() ?? throw new ArgumentException($"{name} must contain strings")).ToArray() : Array.Empty<string>();
    static ISymbol Canonical(ISymbol symbol)
    {
        if (symbol is IMethodSymbol method)
        {
            method = method.ReducedFrom ?? method;
            method = method.PartialImplementationPart ?? method;
            return method.OriginalDefinition;
        }
        return symbol.OriginalDefinition;
    }
    void Diagnostic(string message, string? path = null, int line = 0, string severity = "error")
    {
        diagnostics.Add(new Note(severity, path, line, message));
        if (severity == "error") partial = true;
    }
    void InputFile(string path)
    {
        path = System.IO.Path.GetFullPath(path);
        if (!inputs.ContainsKey(path)) inputs[path] = new Input(path, Hash(File.ReadAllBytes(path)));
    }
    static int[] ByteMap(string text)
    {
        int[] map = new int[text.Length + 1]; int count = 0;
        for (int i = 0; i < text.Length;)
        {
            int width = char.IsSurrogatePair(text, i) ? 2 : 1;
            map[i] = count;
            if (width == 2) map[i + 1] = count;
            count += Encoding.UTF8.GetByteCount(text.AsSpan(i, width)); i += width; map[i] = count;
        }
        return map;
    }
    static bool HasBody(SyntaxNode node) => node is BaseTypeDeclarationSyntax or NamespaceDeclarationSyntax or FileScopedNamespaceDeclarationSyntax
        || node is BaseMethodDeclarationSyntax method && (method.Body != null || method.ExpressionBody != null)
        || node is LocalFunctionStatementSyntax local && (local.Body != null || local.ExpressionBody != null)
        || node is PropertyDeclarationSyntax property && (property.ExpressionBody != null || property.AccessorList != null)
        || node is VariableDeclaratorSyntax or ParameterSyntax or UsingDirectiveSyntax or DelegateDeclarationSyntax or AccessorDeclarationSyntax;
    (SyntaxNode? Syntax, FileData? File) Declaration(ISymbol symbol)
    {
        return symbol.DeclaringSyntaxReferences.Select(reference => reference.GetSyntax())
            .Where(syntax => files.ContainsKey(syntax.SyntaxTree))
            .OrderByDescending(HasBody).ThenBy(syntax => files[syntax.SyntaxTree].Path, StringComparer.Ordinal).ThenBy(syntax => syntax.SpanStart)
            .Select(syntax => (syntax, files[syntax.SyntaxTree])).Cast<(SyntaxNode?, FileData?)>().FirstOrDefault();
    }
    string? Ensure(ISymbol? raw)
    {
        if (raw == null || raw is ITypeSymbol { TypeKind: TypeKind.Error } || raw is IDynamicTypeSymbol || raw is ILabelSymbol || raw is IDiscardSymbol) return null;
        var symbol = Canonical(raw);
        if (symbol is INamespaceSymbol { IsGlobalNamespace: true }) return null;
        var (syntax, file) = Declaration(symbol);
        string? doc = DocumentationCommentId.CreateDeclarationId(symbol);
        if (doc == null && syntax == null) return null;
        string id = "csharp:" + (doc ?? $"local:{file!.Path}:{file.Bytes[syntax!.SpanStart]}:{symbol.Kind}:{symbol.MetadataName}");
        string kind = symbol switch { IMethodSymbol => "function", INamedTypeSymbol or IArrayTypeSymbol or IPointerTypeSymbol or ITypeParameterSymbol => "type", IAliasSymbol => "alias", INamespaceSymbol => "namespace", IFieldSymbol or IPropertySymbol or IEventSymbol => "field", _ => "variable" };
        bool external = syntax == null && !symbol.Locations.Any(location => location.IsInSource && location.SourceTree != null && files.ContainsKey(location.SourceTree));
        var location = syntax?.GetLocation().GetLineSpan();
        var node = new Node(id, symbol.Name, symbol.ToDisplayString(Qualified), kind, symbol is IAliasSymbol aliasContract ? $"using {aliasContract.Name} = {aliasContract.Target.ToDisplayString(Signature)}" : symbol.ToDisplayString(Signature),
            syntax != null && HasBody(syntax), external, symbol.IsImplicitlyDeclared && syntax == null, file?.Path,
            location.HasValue ? location.Value.StartLinePosition.Line + 1 : 0,
            location.HasValue ? location.Value.StartLinePosition.Character + 1 : 0,
            location.HasValue ? location.Value.EndLinePosition.Line + 1 : 0,
            syntax == null ? 0 : file!.Bytes[syntax.Span.Start], syntax == null ? 0 : file!.Bytes[syntax.Span.End]);
        if (!nodes.TryGetValue(id, out var previous) || node.definition && !previous.definition || !node.external && previous.external) nodes[id] = node;
        return id;
    }
    string? Owner(SemanticModel model, SyntaxNode syntax)
    {
        var symbol = model.GetEnclosingSymbol(syntax.SpanStart);
        while (symbol is IParameterSymbol or ILocalSymbol) symbol = symbol.ContainingSymbol;
        return Ensure(symbol);
    }
    void Link(string? source, ISymbol? target, string kind, SyntaxNode syntax, string dispatch = "static")
    {
        var id = Ensure(target); if (source == null || id == null) return;
        var position = syntax.GetLocation().GetLineSpan().StartLinePosition;
        edges.Add(new Edge(source, id, kind, files[syntax.SyntaxTree].Path, position.Line + 1, position.Character + 1, dispatch));
    }
    void Unknown(string? source, string kind, SyntaxNode syntax, string reason)
    {
        string expression = syntax.ToString();
        if (expression.Length > 180) expression = expression[..180] + "…";
        unresolved.Add(new Unknown(source, kind, expression, files[syntax.SyntaxTree].Path, syntax.GetLocation().GetLineSpan().StartLinePosition.Line + 1, reason));
        partial = true;
    }
    void Type(string? source, ITypeSymbol? type, SyntaxNode syntax, string kind = "uses_type", HashSet<ITypeSymbol>? seen = null)
    {
        if (type == null) return;
        if (type.TypeKind == TypeKind.Error) { Unknown(source, kind, syntax, "Compiler could not resolve this type"); return; }
        if (type is IDynamicTypeSymbol) return;
        seen ??= new HashSet<ITypeSymbol>(SymbolEqualityComparer.Default); if (!seen.Add(type)) return;
        if (type is IArrayTypeSymbol array) Type(source, array.ElementType, syntax, kind, seen);
        else if (type is IPointerTypeSymbol pointer) Type(source, pointer.PointedAtType, syntax, kind, seen);
        else if (type is IFunctionPointerTypeSymbol functionPointer)
        {
            Type(source, functionPointer.Signature.ReturnType, syntax, kind, seen);
            foreach (var parameter in functionPointer.Signature.Parameters) Type(source, parameter.Type, syntax, kind, seen);
        }
        else
        {
            Link(source, type, kind, syntax);
            if (type is INamedTypeSymbol named) foreach (var argument in named.TypeArguments) Type(source, argument, syntax, kind, seen);
        }
    }
    void Declared(SemanticModel model, SyntaxNode syntax)
    {
        ISymbol? symbol;
        try { symbol = model.GetDeclaredSymbol(syntax); } catch (ArgumentException) { return; }
        if (symbol == null) return;
        var id = Ensure(symbol);
        if (id == null) return;
        var parent = symbol.ContainingSymbol;
        Link(Ensure(parent), symbol, "contains", syntax);
        string? typeOwner = symbol is IParameterSymbol or ILocalSymbol ? Ensure(parent) : id;
        switch (symbol)
        {
            case IMethodSymbol method:
                Type(id, method.ReturnType, syntax);
                foreach (var parameter in method.Parameters) Type(id, parameter.Type, syntax);
                foreach (var parameter in method.TypeParameters) foreach (var constraint in parameter.ConstraintTypes) Type(id, constraint, syntax);
                break;
            case INamedTypeSymbol type:
                Type(id, type.BaseType, syntax, "inherits");
                foreach (var implemented in type.Interfaces) Type(id, implemented, syntax, "inherits");
                break;
            case ITypeParameterSymbol parameter:
                foreach (var constraint in parameter.ConstraintTypes) Type(id, constraint, syntax); break;
            case IFieldSymbol field: Type(id, field.Type, syntax); break;
            case IPropertySymbol property:
                Type(id, property.Type, syntax); foreach (var parameter in property.Parameters) Type(id, parameter.Type, syntax); break;
            case IEventSymbol eventSymbol: Type(id, eventSymbol.Type, syntax); break;
            case ILocalSymbol local: Type(typeOwner, local.Type, syntax); break;
            case IParameterSymbol parameter: Type(typeOwner, parameter.Type, syntax); break;
            case IAliasSymbol alias: Link(id, alias.Target, "aliases", syntax); if (alias.Target is ITypeSymbol target) Type(id, target, syntax, "aliases"); break;
        }
    }
    void Call(SemanticModel model, SyntaxNode syntax)
    {
        var source = Owner(model, syntax);
        var operation = model.GetOperation(syntax);
        if (operation is IDynamicInvocationOperation || operation is IDynamicObjectCreationOperation)
        { Unknown(source, "calls", syntax, "Dynamic runtime target is not statically resolvable"); return; }
        var info = model.GetSymbolInfo(syntax);
        var method = info.Symbol as IMethodSymbol;
        if (method == null)
        {
            string candidates = info.CandidateSymbols.Length == 0 ? "No compiler-resolved call target" : $"Ambiguous/unbound compiler candidates ({info.CandidateReason}); no target selected";
            Unknown(source, "calls", syntax, candidates); return;
        }
        string dispatch = method.IsVirtual || method.IsAbstract || method.IsOverride || method.ContainingType.TypeKind == TypeKind.Interface ? "virtual" : "static";
        if (method.MethodKind == MethodKind.DelegateInvoke)
        { dispatch = "dynamic"; Unknown(source, "calls", syntax, "Delegate Invoke contract is known; runtime delegate target is unknown"); }
        Link(source, method, "calls", syntax, dispatch);
    }
    void References(SemanticModel model, SyntaxNode syntax)
    {
        if (syntax is InvocationExpressionSyntax or ObjectCreationExpressionSyntax or ImplicitObjectCreationExpressionSyntax or ConstructorInitializerSyntax) Call(model, syntax);
        if (syntax is IdentifierNameSyntax or GenericNameSyntax or PredefinedTypeSyntax)
        {
            var source = Owner(model, syntax);
            var alias = syntax is IdentifierNameSyntax name ? model.GetAliasInfo(name) : null;
            if (alias != null) Link(source, alias, "references", syntax);
            var info = model.GetSymbolInfo(syntax);
            if (info.Symbol != null)
            {
                Link(source, info.Symbol, info.Symbol is ITypeSymbol ? "uses_type" : "references", syntax);
                if (info.Symbol is ITypeSymbol type) Type(source, type, syntax);
            }
        }
        IMethodSymbol? overloaded = model.GetOperation(syntax) switch
        {
            IBinaryOperation operation => operation.OperatorMethod,
            IUnaryOperation operation => operation.OperatorMethod,
            IConversionOperation operation => operation.OperatorMethod,
            _ => null
        };
        if (overloaded != null) Link(Owner(model, syntax), overloaded, "calls", syntax);
    }
    public object Run(JsonElement request)
    {
        if (request.GetProperty("schema_version").GetInt32() != 1 || request.GetProperty("language").GetString() != "csharp") throw new ArgumentException("Expected graph schema 1 and language csharp");
        string root = System.IO.Path.GetFullPath(request.GetProperty("root").GetString()!);
        var options = request.TryGetProperty("options", out var opts) ? opts : JsonDocument.Parse("{}").RootElement;
        if (Strings(options, "flags").Length != 0 || options.TryGetProperty("compile_commands", out var database) && database.ValueKind != JsonValueKind.Null)
            Diagnostic("C# raw compilation does not accept Clang flags/compile_commands; provide explicit defines and metadata references");
        CSharpParseOptions parse;
        try { parse = new CSharpParseOptions(LanguageVersion.Latest, preprocessorSymbols: Strings(options, "defines")); }
        catch (ArgumentException error) { Diagnostic("Invalid preprocessor define: " + error.Message); parse = new CSharpParseOptions(LanguageVersion.Latest); }
        foreach (var input in request.GetProperty("files").EnumerateArray())
        {
            string path = input.GetProperty("path").GetString()!; if (!path.EndsWith(".cs", StringComparison.OrdinalIgnoreCase)) continue;
            if (System.IO.Path.IsPathRooted(path) || path.Split('/').Any(part => part is ".." or "." or "")) throw new ArgumentException("Expected normalized relative source path");
            string text = input.GetProperty("text").GetString()!;
            if (Hash(Encoding.UTF8.GetBytes(text)) != input.GetProperty("hash").GetString()) throw new ArgumentException("Source hash differs from authoritative request: " + path);
            var tree = CSharpSyntaxTree.ParseText(SourceText.From(text, Encoding.UTF8), parse, path);
            if (files.Values.Any(file => file.Path == path)) throw new ArgumentException("Duplicate input path: " + path);
            files.Add(tree, new FileData(path, text, ByteMap(text)));
        }
        if (files.Count == 0) Diagnostic("No supplied C# sources");
        var references = new List<MetadataReference>();
        var metadata = new SortedSet<string>(StringComparer.Ordinal);
        string[] tpa = ((string?)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES") ?? "").Split(System.IO.Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);
        foreach (var path in tpa) if (!System.IO.Path.GetFileName(path).StartsWith("Microsoft.CodeAnalysis", StringComparison.Ordinal) && System.IO.Path.GetFullPath(path) != Assembly.GetExecutingAssembly().Location) metadata.Add(path);
        if (metadata.Count == 0) Diagnostic(".NET trusted platform metadata is unavailable");
        foreach (var reference in Strings(options, "references")) metadata.Add(System.IO.Path.IsPathRooted(reference) ? reference : System.IO.Path.Combine(root, reference));
        foreach (var path in metadata)
        {
            try { references.Add(MetadataReference.CreateFromFile(path)); InputFile(path); }
            catch (Exception error) when (error is IOException or BadImageFormatException or UnauthorizedAccessException or ArgumentException) { Diagnostic($"Metadata reference unavailable: {path}: {error.Message}"); }
        }
        compilation = CSharpCompilation.Create("AIEnvStaticAnalysis", files.Keys, references,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true));
        foreach (var diagnostic in compilation.GetDiagnostics())
        {
            var span = diagnostic.Location.IsInSource ? diagnostic.Location.GetLineSpan() : default;
            Diagnostic(diagnostic.Id + ": " + diagnostic.GetMessage(System.Globalization.CultureInfo.InvariantCulture), diagnostic.Location.IsInSource ? span.Path : null,
                diagnostic.Location.IsInSource ? span.StartLinePosition.Line + 1 : 0,
                diagnostic.Severity == DiagnosticSeverity.Error ? "error" : diagnostic.Severity == DiagnosticSeverity.Warning ? "warning" : "note");
        }
        var selected = Strings(options, "files").ToHashSet(StringComparer.Ordinal);
        foreach (var path in selected) if (!files.Values.Any(file => file.Path == path)) Diagnostic("Selected C# file was not supplied: " + path, path);
        var active = files.Keys.Where(tree => selected.Count == 0 || selected.Contains(files[tree].Path)).ToArray();
        foreach (var tree in active)
        {
            var model = compilation.GetSemanticModel(tree);
            foreach (var syntax in tree.GetRoot().DescendantNodesAndSelf()) Declared(model, syntax);
        }
        foreach (var tree in active)
        {
            var model = compilation.GetSemanticModel(tree);
            foreach (var syntax in tree.GetRoot().DescendantNodesAndSelf()) References(model, syntax);
        }
        foreach (var assembly in new[] { Assembly.GetExecutingAssembly(), typeof(CSharpCompilation).Assembly, typeof(Compilation).Assembly })
        {
            InputFile(assembly.Location);
            foreach (var extension in new[] { ".deps.json", ".runtimeconfig.json" }) { string path = System.IO.Path.ChangeExtension(assembly.Location, extension); if (File.Exists(path)) InputFile(path); }
        }
        return Response();
    }
    object Response() => new {
        schema_version = 1, language = "csharp",
        analyzer = new { name = "roslyn", version = typeof(CSharpCompilation).Assembly.GetName().Version!.ToString(), capabilities = new[] { "symbols", "calls", "references", "uses_type", "inherits", "aliases" } },
        nodes = nodes.Values.OrderBy(node => node.id, StringComparer.Ordinal).ToArray(),
        edges = edges.OrderBy(edge => edge.source, StringComparer.Ordinal).ThenBy(edge => edge.target, StringComparer.Ordinal).ThenBy(edge => edge.kind, StringComparer.Ordinal).ThenBy(edge => edge.path, StringComparer.Ordinal).ThenBy(edge => edge.line).ThenBy(edge => edge.column).ThenBy(edge => edge.dispatch).ToArray(),
        diagnostics = diagnostics.OrderBy(note => note.path, StringComparer.Ordinal).ThenBy(note => note.line).ThenBy(note => note.message, StringComparer.Ordinal).ToArray(),
        unresolved = unresolved.OrderBy(item => item.path, StringComparer.Ordinal).ThenBy(item => item.line).ThenBy(item => item.expression, StringComparer.Ordinal).ThenBy(item => item.reason, StringComparer.Ordinal).ToArray(),
        inputs = inputs.Values.ToArray(), analysis_complete = !partial,
        limitations = new[] {
            "Static C# compilation of supplied sources with framework TPA metadata and explicit references/defines; csproj/MSBuild, generators and target-project analyzers are not executed or inferred.",
            "Virtual/interface calls identify the compile-time contract, not all runtime implementations; dynamic and delegate runtime targets remain unresolved.",
            "Property/event access records contract references; implicit accessor calls and reflection runtime targets are not expanded.",
            "Only selected source trees contribute relation walks when options.files is supplied; other supplied C# trees resolve referenced contracts. Partial symbols retain one representative declaration span."
        }
    };
    public object Failure(Exception error) { Diagnostic(error.GetType().Name + ": " + error.Message); return Response(); }
}

static class Program
{
    public static int Main()
    {
        var analyzer = new Analyzer();
        try
        {
            using var request = JsonDocument.Parse(Console.In.ReadToEnd());
            Console.WriteLine(JsonSerializer.Serialize(analyzer.Run(request.RootElement)));
            return 0;
        }
        catch (Exception error)
        {
            Console.WriteLine(JsonSerializer.Serialize(analyzer.Failure(error)));
            return 0;
        }
    }
}
