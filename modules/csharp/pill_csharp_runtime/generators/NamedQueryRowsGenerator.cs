// Roslyn source generator for named query-row iteration.
//
// A `Query<T1..T8>` row exposes generic accessors (`row.Write<Position>()`),
// because one row type serves every query shape. That is fine for the JIT -
// the type checks fold away - but it reads nothing like the Rust side, where
// a system destructures the query into named locals.
//
// This generator closes the gap: it scans every method that takes a closed
// `Query<...>` parameter, and for each distinct shape emits
//
//   * a `Rows()` extension on the query,
//   * a per-shape iterator and row with one named member per term
//     (`row.PhysicsState`, `row.Position`, `row.MeshRendererComponent`, `row.Entity`),
//   * a `ForEachParallel()` extension pair plus a per-shape body delegate,
//     whose ref/in parameters bind the caller's lambda to the runtime's
//     parallel pass (`QueryEnumerator.ForEachSliceParallel`).
//
// The generated row wraps the runtime's own typed row, so it inherits the
// same semantics: `Write<T>` terms return `ref T` and stamp the change tick,
// `Read<T>` terms return `ref readonly T`, optional terms return the optional
// wrappers, and the JIT sees the same direct references through the wrappers.
// The generated types live in the global namespace so a system needs no using
// directive to reach `query.Rows()`.

using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using System.Text;
using System.Threading;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;

namespace PillIterationGenerators
{
    /// <summary>Emits named query-row wrappers for every closed query shape.</summary>
    [Generator(LanguageNames.CSharp)]
    public sealed class NamedQueryRowsGenerator : IIncrementalGenerator
    {
        /// <summary>Fully-qualified symbol rendering with `global::` prefixes.</summary>
        private static readonly SymbolDisplayFormat FullyQualified =
            SymbolDisplayFormat.FullyQualifiedFormat;

        public void Initialize(IncrementalGeneratorInitializationContext context)
        {
            IncrementalValuesProvider<QueryShape> shapes = context
                .SyntaxProvider.CreateSyntaxProvider(
                    static (node, _) => IsCandidateMethod(node),
                    static (ctx, cancellationToken) => Extract(ctx, cancellationToken))
                .Where(static found => found.Length > 0)
                .SelectMany(static (found, _) => found);

            context.RegisterSourceOutput(
                shapes.Collect(),
                static (sourceContext, collected) => Emit(sourceContext, collected));
        }

        /// <summary>
        /// Fast syntax filter: a method with at least one parameter written as
        /// `Query&lt;...&gt;`, which is the only place a query shape can appear.
        /// </summary>
        private static bool IsCandidateMethod(SyntaxNode node)
        {
            if (node is not MethodDeclarationSyntax method)
                return false;
            foreach (ParameterSyntax parameter in method.ParameterList.Parameters)
                if (parameter.Type is GenericNameSyntax generic &&
                    generic.Identifier.ValueText == "Query")
                    return true;
            return false;
        }

        /// <summary>Collect the closed query shapes one candidate method declares.</summary>
        private static ImmutableArray<QueryShape> Extract(
            GeneratorSyntaxContext context, CancellationToken cancellationToken)
        {
            if (context.Node is not MethodDeclarationSyntax methodSyntax)
                return ImmutableArray<QueryShape>.Empty;
            if (context.SemanticModel.GetDeclaredSymbol(methodSyntax, cancellationToken)
                is not IMethodSymbol method)
                return ImmutableArray<QueryShape>.Empty;

            ImmutableArray<QueryShape>.Builder found = ImmutableArray.CreateBuilder<QueryShape>();
            foreach (IParameterSymbol parameter in method.Parameters)
            {
                QueryShape? shape = DescribeShape(parameter.Type);
                if (shape is not null)
                    found.Add(shape);
            }
            return found.ToImmutable();
        }

        /// <summary>
        /// Describe one parameter type as a query shape, or return null when it
        /// is not a closed `Query&lt;T1..T8&gt;` over supported terms.
        /// </summary>
        private static QueryShape? DescribeShape(ITypeSymbol type)
        {
            if (type is not INamedTypeSymbol named)
                return null;
            if (!named.IsGenericType || named.Name != "Query")
                return null;
            if (named.ContainingNamespace is null ||
                named.ContainingNamespace.ToDisplayString() != "TracyLive")
                return null;
            int arity = named.TypeArguments.Length;
            if (arity < 1 || arity > 8)
                return null;

            TermInfo[] terms = new TermInfo[arity];
            for (int index = 0; index < arity; index++)
            {
                TermInfo? term = DescribeTerm(named.TypeArguments[index]);
                if (term is null)
                    return null;
                terms[index] = term;
            }
            return new QueryShape(
                named.ToDisplayString(FullyQualified),
                terms,
                named.TypeArguments
                    .Select(argument => argument.ToDisplayString(FullyQualified))
                    .ToImmutableArray(),
                // A wrapper is at most as visible as the components it exposes:
                // an internal gameplay component cannot appear in a public
                // surface, and the compiler enforces that on generated code too.
                terms.All(static term => term.PayloadIsPublic) ? "public" : "internal");
        }

        /// <summary>Whether a type and every type containing it are public.</summary>
        private static bool IsEffectivelyPublic(ITypeSymbol symbol)
        {
            for (ISymbol? current = symbol; current is not null; current = current.ContainingType)
                if (current.DeclaredAccessibility != Accessibility.Public)
                    return false;
            return true;
        }

        /// <summary>Describe one query term, or return null when it is unsupported.</summary>
        private static TermInfo? DescribeTerm(ITypeSymbol type)
        {
            if (type is not INamedTypeSymbol named)
                return null;
            if (named.ContainingNamespace is null ||
                named.ContainingNamespace.ToDisplayString() != "TracyLive")
                return null;
            if (named.IsGenericType)
            {
                if (named.TypeArguments.Length != 1)
                    return null;
                ITypeSymbol payload = named.TypeArguments[0];
                if (payload is not INamedTypeSymbol payloadType)
                    return null;
                // Generic component types would need sanitized member names and
                // never appear in the scripting surface today; skip them rather
                // than emit code that may not compile.
                if (payloadType.IsGenericType)
                    return null;
                // The accessor APIs constrain components to unmanaged types.
                if (!payload.IsUnmanagedType)
                    return null;
                string payloadDisplay = payload.ToDisplayString(FullyQualified);
                string payloadName = payloadType.Name;
                bool payloadIsPublic = IsEffectivelyPublic(payload);
                switch (named.Name)
                {
                    case "Read":
                        return new TermInfo(TermKind.Read, payloadDisplay, payloadName, null, payloadIsPublic);
                    case "Write":
                        return new TermInfo(TermKind.Write, payloadDisplay, payloadName, null, payloadIsPublic);
                    case "OptionalRead":
                        return new TermInfo(TermKind.OptionalRead, payloadDisplay, payloadName, null, payloadIsPublic);
                    case "OptionalWrite":
                        return new TermInfo(TermKind.OptionalWrite, payloadDisplay, payloadName, null, payloadIsPublic);
                    default:
                        return null;
                }
            }
            if (named.Name == "EntityTerm")
                return new TermInfo(TermKind.Entity, null, null, "Entity", payloadIsPublic: true);
            return null;
        }

        /// <summary>Emit one wrapper triple per distinct shape.</summary>
        private static void Emit(SourceProductionContext context, ImmutableArray<QueryShape> shapes)
        {
            if (shapes.IsDefaultOrEmpty)
                return;

            // Distinct shapes only: several systems may share one shape, and two
            // `Rows()` extensions with the same signature would be ambiguous.
            Dictionary<string, QueryShape> distinct = new Dictionary<string, QueryShape>();
            foreach (QueryShape shape in shapes)
                if (!distinct.ContainsKey(shape.Key))
                    distinct.Add(shape.Key, shape);

            List<QueryShape> ordered = distinct.Values.ToList();
            ordered.Sort(static (left, right) => string.CompareOrdinal(left.Key, right.Key));

            // Type names derive from the term names; two shapes sharing a name
            // (say `Read<A>` and `Write<A>` queried separately) get a mode
            // qualifier so both wrappers can exist.
            Dictionary<string, int> nameCounts = new Dictionary<string, int>();
            foreach (QueryShape shape in ordered)
            {
                string candidate = shape.CandidateName;
                nameCounts.TryGetValue(candidate, out int count);
                nameCounts[candidate] = count + 1;
            }

            foreach (QueryShape shape in ordered)
            {
                string name = shape.CandidateName;
                if (nameCounts[name] > 1)
                    name = shape.ModeQualifiedName();
                context.AddSource($"{name}.g.cs", SourceText.From(Render(shape, name), Encoding.UTF8));
            }
        }

        /// <summary>Render the three generated declarations for one shape.</summary>
        private static string Render(QueryShape shape, string name)
        {
            // The runtime row pads every shape to eight slots with None.
            List<string> padded = new List<string>(shape.TermArgs);
            while (padded.Count < 8)
                padded.Add("global::TracyLive.None");
            string paddedArgs = string.Join(", ", padded);

            StringBuilder source = new StringBuilder();
            source.AppendLine("// <auto-generated> Named query-row accessors. Do not edit.</auto-generated>");
            source.AppendLine("//");
            source.AppendLine($"// Query shape: {shape.QueryDisplay}");
            source.AppendLine("// Emitted into the global namespace so every system can call");
            source.AppendLine("// `query.Rows()` without a using directive.");
            source.AppendLine("#nullable enable");
            source.AppendLine("using System.Diagnostics.CodeAnalysis;");
            source.AppendLine("using System.Runtime.CompilerServices;");
            source.AppendLine();

            // The parallel entry point's shape: a body delegate whose ref/in
            // parameters replace the ref-struct row, because a worker lambda
            // cannot capture a ref struct. Slots are term positions.
            List<string> bodyParameters = new List<string>();
            List<string> bodyArguments = new List<string>();
            for (int index = 0; index < shape.Terms.Length; index++)
            {
                TermInfo term = shape.Terms[index];
                switch (term.Kind)
                {
                    case TermKind.Write:
                        bodyParameters.Add($"ref {term.PayloadDisplay} {term.MemberName}");
                        bodyArguments.Add($"ref {term.MemberName}");
                        break;
                    case TermKind.Read:
                        bodyParameters.Add($"in {term.PayloadDisplay} {term.MemberName}");
                        bodyArguments.Add($"in {term.MemberName}");
                        break;
                    case TermKind.OptionalWrite:
                        bodyParameters.Add($"global::TracyLive.OptionalWriteRef<{term.PayloadDisplay}> {term.MemberName}");
                        bodyArguments.Add(term.MemberName);
                        break;
                    case TermKind.OptionalRead:
                        bodyParameters.Add($"global::TracyLive.OptionalReadRef<{term.PayloadDisplay}> {term.MemberName}");
                        bodyArguments.Add(term.MemberName);
                        break;
                    case TermKind.Entity:
                        bodyParameters.Add($"global::TracyLive.Entity {term.MemberName}");
                        bodyArguments.Add(term.MemberName);
                        break;
                }
            }
            string bodyParameterList = string.Join(", ", bodyParameters);
            string bodyArgumentList = string.Join(", ", bodyArguments);

            source.AppendLine($"/// <summary>Body of a parallel pass over `{shape.QueryDisplay}`: one call per row.</summary>");
            source.AppendLine("/// <remarks>Runs on worker threads - touch only the references handed in, call no engine APIs or Commands, start no nested pass; row order is undefined.</remarks>");
            source.AppendLine($"{shape.Accessibility} delegate void {name}ParallelBody({bodyParameterList});");
            source.AppendLine();

            source.AppendLine($"/// <summary>Entry point for `{shape.QueryDisplay}`: iterate with named per-term accessors.</summary>");
            source.AppendLine($"{shape.Accessibility} static class {name}Extensions");
            source.AppendLine("{");
            source.AppendLine("    /// <summary>Iterate the query with named, strongly typed accessors.</summary>");
            source.AppendLine($"    {shape.Accessibility} static {name}Rows Rows(this {shape.QueryDisplay} query) => new(query.GetEnumerator());");
            source.AppendLine();
            source.AppendLine("    /// <summary>Run a parallel pass over every matched row on the host's shared thread pool.</summary>");
            source.AppendLine("    /// <remarks>");
            source.AppendLine("    /// The body runs on worker threads: it may touch only the references it is handed,");
            source.AppendLine("    /// must not call engine APIs or Commands, must not start another parallel pass, and");
            source.AppendLine("    /// row order is undefined. The call returns after every row finished; the first");
            source.AppendLine("    /// exception a row throws is rethrown on the calling thread after the join.");
            source.AppendLine("    /// </remarks>");
            source.AppendLine($"    {shape.Accessibility} static void ForEachParallel(this {shape.QueryDisplay} query, {name}ParallelBody body) =>");
            source.AppendLine("        ForEachParallel(query, global::TracyLive.ParallelRows.DefaultRowsPerSlice, body);");
            source.AppendLine();
            source.AppendLine("    /// <summary>Run a parallel pass with a caller-chosen rows-per-slice size (clamped to the runtime minimum).</summary>");
            source.AppendLine($"    {shape.Accessibility} static void ForEachParallel(this {shape.QueryDisplay} query, int rowsPerSlice, {name}ParallelBody body)");
            source.AppendLine("    {");
            source.AppendLine("        // The runner captures only `body`: nothing here may keep an enumerator or");
            source.AppendLine("        // query reference beyond the frame-thread resolution that made the segments.");
            source.AppendLine("        ((global::TracyLive.QueryBase)query).GetEnumerator().ForEachSliceParallel(");
            source.AppendLine("            (segment, start, count) =>");
            source.AppendLine("            {");
            source.AppendLine("                for (int row = start; row < start + count; row++)");
            source.AppendLine("                {");
            for (int index = 0; index < shape.Terms.Length; index++)
            {
                TermInfo term = shape.Terms[index];
                switch (term.Kind)
                {
                    case TermKind.Write:
                        // Stamp like the sequential row does when its writable
                        // accessor is fetched, then hand out the reference.
                        source.AppendLine($"                    segment.MarkChanged({index}, row);");
                        source.AppendLine($"                    ref {term.PayloadDisplay} {term.MemberName} = ref segment.Get<{term.PayloadDisplay}>({index}, row);");
                        break;
                    case TermKind.Read:
                        source.AppendLine($"                    ref readonly {term.PayloadDisplay} {term.MemberName} = ref segment.Get<{term.PayloadDisplay}>({index}, row);");
                        break;
                    case TermKind.OptionalWrite:
                        source.AppendLine($"                    global::TracyLive.OptionalWriteRef<{term.PayloadDisplay}> {term.MemberName} = segment.GetOptionalWrite<{term.PayloadDisplay}>({index}, row);");
                        break;
                    case TermKind.OptionalRead:
                        source.AppendLine($"                    global::TracyLive.OptionalReadRef<{term.PayloadDisplay}> {term.MemberName} = segment.GetOptionalRead<{term.PayloadDisplay}>({index}, row);");
                        break;
                    case TermKind.Entity:
                        source.AppendLine($"                    global::TracyLive.Entity {term.MemberName} = segment.GetEntity({index}, row);");
                        break;
                }
            }
            source.AppendLine($"                    body({bodyArgumentList});");
            source.AppendLine("                }");
            source.AppendLine("            }, rowsPerSlice);");
            source.AppendLine("    }");
            source.AppendLine("}");
            source.AppendLine();

            source.AppendLine($"/// <summary>Iterator over `{shape.QueryDisplay}`; yields {name}Row.</summary>");
            source.AppendLine($"{shape.Accessibility} ref struct {name}Rows");
            source.AppendLine("{");
            source.AppendLine($"    private global::TracyLive.QueryEnumerator<{paddedArgs}> _inner;");
            source.AppendLine();
            source.AppendLine($"    public {name}Rows(global::TracyLive.QueryEnumerator<{paddedArgs}> inner) => _inner = inner;");
            source.AppendLine();
            source.AppendLine("    /// <summary>The struct is its own enumerator, as foreach expects.</summary>");
            source.AppendLine("    [MethodImpl(MethodImplOptions.AggressiveInlining)]");
            source.AppendLine("    public " + name + "Rows GetEnumerator() => this;");
            source.AppendLine();
            source.AppendLine("    /// <summary>Advance to the next joined row.</summary>");
            source.AppendLine("    [MethodImpl(MethodImplOptions.AggressiveInlining)]");
            source.AppendLine("    public bool MoveNext() => _inner.MoveNext();");
            source.AppendLine();
            source.AppendLine("    /// <summary>Named accessors for the current row.</summary>");
            // The row value carries references into this enumerator's columns,
            // so the property takes the same [UnscopedRef] opt-in the runtime's
            // typed Current uses: the foreach shape keeps the enumerator alive
            // for every use of the row it hands out. AggressiveInlining keeps
            // the wrapper layer from pushing the accessor chain past the JIT's
            // inline budget, which costs ~3 ns per row when it happens.
            source.AppendLine("    [UnscopedRef]");
            source.AppendLine($"    public {name}Row Current => new(_inner.Current);");
            source.AppendLine("}");
            source.AppendLine();

            source.AppendLine($"/// <summary>Named accessors for one row of `{shape.QueryDisplay}`.</summary>");
            source.AppendLine($"{shape.Accessibility} ref struct {name}Row");
            source.AppendLine("{");
            source.AppendLine($"    private readonly global::TracyLive.QueryRow<{paddedArgs}> _row;");
            source.AppendLine();
            source.AppendLine($"    public {name}Row(global::TracyLive.QueryRow<{paddedArgs}> row) => _row = row;");

            foreach (TermInfo term in shape.Terms)
            {
                source.AppendLine();
                switch (term.Kind)
                {
                    case TermKind.Entity:
                        source.AppendLine($"    /// <summary>The row's entity (declared `EntityTerm`).</summary>");
                        source.AppendLine($"    public global::TracyLive.Entity {term.MemberName} => _row.Entity;");
                        break;
                    case TermKind.Write:
                        source.AppendLine($"    /// <summary>Writable `{term.PayloadName}` (declared `Write<{term.PayloadName}>`); writes stamp the change tick.</summary>");
                        source.AppendLine("    [UnscopedRef]");
                        source.AppendLine($"    public ref {term.PayloadDisplay} {term.MemberName}");
                        source.AppendLine("    {");
                        source.AppendLine("        [MethodImpl(MethodImplOptions.AggressiveInlining)]");
                        source.AppendLine($"        get => ref _row.Write<{term.PayloadDisplay}>();");
                        source.AppendLine("    }");
                        break;
                    case TermKind.Read:
                        source.AppendLine($"    /// <summary>Read-only `{term.PayloadName}` (declared `Read<{term.PayloadName}>`).</summary>");
                        source.AppendLine("    [UnscopedRef]");
                        source.AppendLine($"    public ref readonly {term.PayloadDisplay} {term.MemberName}");
                        source.AppendLine("    {");
                        source.AppendLine("        [MethodImpl(MethodImplOptions.AggressiveInlining)]");
                        source.AppendLine($"        get => ref _row.Read<{term.PayloadDisplay}>();");
                        source.AppendLine("    }");
                        break;
                    case TermKind.OptionalWrite:
                        source.AppendLine($"    /// <summary>Optional writable `{term.PayloadName}`; check `HasValue` before use.</summary>");
                        source.AppendLine($"    public global::TracyLive.OptionalWriteRef<{term.PayloadDisplay}> {term.MemberName}");
                        source.AppendLine("    {");
                        source.AppendLine("        [MethodImpl(MethodImplOptions.AggressiveInlining)]");
                        source.AppendLine($"        get => _row.OptionalWrite<{term.PayloadDisplay}>();");
                        source.AppendLine("    }");
                        break;
                    case TermKind.OptionalRead:
                        source.AppendLine($"    /// <summary>Optional read-only `{term.PayloadName}`; check `HasValue` before use.</summary>");
                        source.AppendLine($"    public global::TracyLive.OptionalReadRef<{term.PayloadDisplay}> {term.MemberName}");
                        source.AppendLine("    {");
                        source.AppendLine("        [MethodImpl(MethodImplOptions.AggressiveInlining)]");
                        source.AppendLine($"        get => _row.OptionalRead<{term.PayloadDisplay}>();");
                        source.AppendLine("    }");
                        break;
                }
            }

            source.AppendLine("}");
            return source.ToString();
        }
    }

    /// <summary>What one query term exposes on the generated row.</summary>
    internal enum TermKind
    {
        Read,
        Write,
        OptionalRead,
        OptionalWrite,
        Entity,
    }

    /// <summary>One term of a query shape.</summary>
    internal sealed class TermInfo
    {
        public TermInfo(
            TermKind kind,
            string? payloadDisplay,
            string? payloadName,
            string? memberName,
            bool payloadIsPublic)
        {
            Kind = kind;
            PayloadDisplay = payloadDisplay;
            PayloadName = payloadName;
            MemberName = memberName ?? payloadName ?? "Value";
            PayloadIsPublic = payloadIsPublic;
        }

        public TermKind Kind { get; }

        /// <summary>Fully-qualified payload type, `null` for `EntityTerm`.</summary>
        public string? PayloadDisplay { get; }

        /// <summary>Simple payload type name, `null` for `EntityTerm`.</summary>
        public string? PayloadName { get; }

        /// <summary>Member name on the generated row.</summary>
        public string MemberName { get; set; }

        /// <summary>Whether the payload type may appear in a public surface.</summary>
        public bool PayloadIsPublic { get; }
    }

    /// <summary>
    /// One distinct closed query shape: the terms, the declared query type,
    /// and the names derived from them. Equality is by <see cref="Key"/> so
    /// the incremental pipeline can cache per shape.
    /// </summary>
    internal sealed class QueryShape : IEquatable<QueryShape>
    {
        public QueryShape(
            string queryDisplay,
            TermInfo[] terms,
            ImmutableArray<string> termArgs,
            string accessibility)
        {
            // Two terms can want the same member name: a query that repeats a
            // component (invalid at run time, but the compiler still sees it),
            // or two same-named types from different namespaces. Later
            // occurrences get a numeric suffix so the generated row compiles;
            // the type name follows the member names, so it stays unique too.
            Dictionary<string, int> seen = new Dictionary<string, int>();
            foreach (TermInfo term in terms)
            {
                seen.TryGetValue(term.MemberName, out int occurrences);
                seen[term.MemberName] = occurrences + 1;
                if (occurrences > 0)
                    term.MemberName = term.MemberName + (occurrences + 1);
            }

            QueryDisplay = queryDisplay;
            Terms = terms;
            TermArgs = termArgs;
            Accessibility = accessibility;
            Key = queryDisplay;
            CandidateName = string.Concat(terms.Select(term => term.MemberName)) + "Query";
        }

        public string QueryDisplay { get; }

        public TermInfo[] Terms { get; }

        /// <summary>Fully-qualified declared term arguments, arity 1..8.</summary>
        public ImmutableArray<string> TermArgs { get; }

        /// <summary>`public` or `internal`, matching the exposed components.</summary>
        public string Accessibility { get; }

        public string Key { get; }

        /// <summary>Base type name, e.g. `PhysicsStatePositionMeshRendererComponentQuery`.</summary>
        public string CandidateName { get; }

        /// <summary>
        /// Name for shapes whose base name is shared with another shape: the
        /// term names gain a read/write qualifier so both can exist.
        /// </summary>
        public string ModeQualifiedName()
        {
            StringBuilder name = new StringBuilder();
            foreach (TermInfo term in Terms)
            {
                switch (term.Kind)
                {
                    case TermKind.Read:
                        name.Append(term.MemberName).Append("Read");
                        break;
                    case TermKind.Write:
                        name.Append(term.MemberName).Append("Write");
                        break;
                    case TermKind.OptionalRead:
                        name.Append(term.MemberName).Append("OptionalRead");
                        break;
                    case TermKind.OptionalWrite:
                        name.Append(term.MemberName).Append("OptionalWrite");
                        break;
                    default:
                        name.Append(term.MemberName);
                        break;
                }
            }
            return name.Append("Query").ToString();
        }

        public bool Equals(QueryShape? other) =>
            other is not null && string.Equals(Key, other.Key, StringComparison.Ordinal);

        public override bool Equals(object? obj) => Equals(obj as QueryShape);

        public override int GetHashCode() => Key.GetHashCode();
    }
}
