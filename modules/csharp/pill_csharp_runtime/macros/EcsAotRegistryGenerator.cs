// Roslyn source generator for the NativeAOT shipping posture.
//
// The reflection-based system discovery in `ProjectHost` cannot run under
// NativeAOT: `Activator.CreateInstance`, `Expression.Compile` and
// `MethodInfo.Invoke` all require dynamic code or reflection invokers that
// AOT does not provide. This generator instead emits, at compile time, a
// direct registration table for every `[System]` / `[Startup]` method:
// a static runner per system that constructs each query with `new` and calls
// the method directly (no reflection), plus the query descriptors
// needed for scheduler access derivation and the component manifest.
//
// The generated code is compiled into the *project* assembly (the only one
// that can see the gameplay types) and installed into `AotRegistry`
// (csharp_runtime) through a module initializer, so `ProjectHost` reads the
// same data shape it would have discovered reflectively.
//
// It also emits the loader's export forwarders: NativeAOT exports
// `[UnmanagedCallersOnly]` methods only from the root assembly of a library
// publish, which is the project, so every `LoaderInterop` method marked
// `[ShippedExport]` gets a forwarder here. A project carries no file for them.
//
// Only referenced when the project is published with PublishAot=true
// (see project_cs.csproj); ordinary JIT builds never run it.

using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;

namespace PillCSharpRuntimeMacros
{
    /// <summary>Emits the AOT system registry for a project assembly.</summary>
    [Generator(LanguageNames.CSharp)]
    public sealed class EcsAotRegistryGenerator : IIncrementalGenerator
    {
        private const string AotRegistryNamespace = "TracyLive.Loader";
        private const string AotRegistryTypeName = "AotRegistry";
        private const string LoaderInteropTypeName = "TracyLive.Loader.LoaderInterop";
        private const string ShippedExportAttributeName = "TracyLive.Loader.ShippedExportAttribute";
        /// <summary>Parameter budget matching the runtime's managed system contract.</summary>
        private const int MaxSystemParameters = 6;
        // FullyQualifiedFormat renders with the `global::` prefix, so the
        // comparison strings must carry it too.
        private const string CommandsTypeName = "global::TracyLive.Commands";
        private const string IResourceParameterTypeName = "global::TracyLive.IResourceParameter";
        private const string ResTypeName = "global::TracyLive.Res<";
        private const string ResMutTypeName = "global::TracyLive.ResMut<";
        private const string IQueryDescriptorTypeName = "global::TracyLive.IQueryDescriptor";
        /// <summary>Matches the open `Write&lt;T&gt;` query term; conditions may not write.</summary>
        private const string WriteTypeName = "global::TracyLive.Write<";

        /// <summary>Fully-qualified symbol rendering with `global::` prefixes.</summary>
        private static readonly SymbolDisplayFormat FullyQualified =
            SymbolDisplayFormat.FullyQualifiedFormat;

        public void Initialize(IncrementalGeneratorInitializationContext context)
        {
            IncrementalValuesProvider<IMethodSymbol> attributedMethods = context
                .SyntaxProvider.CreateSyntaxProvider(
                    static (node, _) => IsAttributedMethod(node),
                    static (ctx, _) =>
                        (IMethodSymbol?)ctx.SemanticModel.GetDeclaredSymbol(ctx.Node))
                .Where(static method => method is not null)
                .Select(static (method, _) => method!);

            context.RegisterSourceOutput(
                attributedMethods.Collect(),
                static (sourceContext, methods) =>
                    Emit(sourceContext, methods));

            // The loader's exports, re-declared in the project: NativeAOT
            // exports only from the root assembly of a library publish.
            context.RegisterSourceOutput(
                context.CompilationProvider,
                static (sourceContext, compilation) =>
                    EmitLoaderExports(sourceContext, compilation));
        }

        /// <summary>
        /// Emit one `[UnmanagedCallersOnly]` forwarder per loader method marked
        /// `[ShippedExport]`, so the project - the AOT root - exports the loader
        /// ABI the Rust host resolves.
        /// </summary>
        /// <remarks>
        /// Read off the referenced runtime assembly rather than listed here, so
        /// an export added to the loader reaches every project's shipping build
        /// with nothing to copy. The output exists only in the compiler: no
        /// project carries a file for it.
        /// </remarks>
        private static void EmitLoaderExports(SourceProductionContext context, Compilation compilation)
        {
            INamedTypeSymbol? loader = compilation.GetTypeByMetadataName(LoaderInteropTypeName);
            if (loader is null)
                return;
            StringBuilder source = new StringBuilder();
            source.AppendLine("// <auto-generated> by PillCSharpRuntimeMacros.EcsAotRegistryGenerator");
            source.AppendLine("// NativeAOT export forwarders for the loader ABI. Do not edit.</auto-generated>");
            source.AppendLine($"namespace {AotRegistryNamespace}");
            source.AppendLine("{");
            source.AppendLine("    internal static unsafe class GeneratedLoaderExports");
            source.AppendLine("    {");
            int emitted = 0;
            foreach (ISymbol member in loader.GetMembers())
            {
                if (member is not IMethodSymbol method || !method.IsStatic)
                    continue;
                string? entryPoint = null;
                foreach (AttributeData attribute in method.GetAttributes())
                {
                    if (attribute.AttributeClass?.ToDisplayString() == ShippedExportAttributeName &&
                        attribute.ConstructorArguments.Length == 1)
                        entryPoint = attribute.ConstructorArguments[0].Value as string;
                }
                if (entryPoint is null)
                    continue;
                string returnType = method.ReturnType.ToDisplayString(FullyQualified);
                string parameters = string.Join(", ", method.Parameters.Select(parameter =>
                    $"{parameter.Type.ToDisplayString(FullyQualified)} {parameter.Name}"));
                string arguments = string.Join(", ", method.Parameters.Select(parameter => parameter.Name));
                source.AppendLine(
                    "        [global::System.Runtime.InteropServices.UnmanagedCallersOnly(" +
                    $"EntryPoint = \"{entryPoint}\")]");
                source.AppendLine(
                    $"        public static {returnType} {method.Name}({parameters}) => " +
                    $"global::{LoaderInteropTypeName}.{method.Name}({arguments});");
                emitted++;
            }
            source.AppendLine("    }");
            source.AppendLine("}");
            if (emitted > 0)
                context.AddSource("GeneratedLoaderExports.g.cs", SourceText.From(source.ToString(), Encoding.UTF8));
        }

        /// <summary>Fast syntax filter: a static method carrying any attribute.</summary>
        private static bool IsAttributedMethod(SyntaxNode node)
        {
            if (node is not MethodDeclarationSyntax method)
                return false;
            if (method.AttributeLists.Count == 0)
                return false;
            // Static systems/startups are required by the runtime.
            return method.Modifiers.Any(SyntaxKind.StaticKeyword);
        }

        /// <summary>Emit the registry source into the compilation.</summary>
        private static void Emit(SourceProductionContext context, ImmutableArray<IMethodSymbol> methods)
        {
            List<(IMethodSymbol Method, AttributeData Declaration)> systems = new();
            List<IMethodSymbol> startups = new();
            foreach (IMethodSymbol method in methods)
            {
                foreach (AttributeData attribute in method.GetAttributes())
                {
                    string name = attribute.AttributeClass?.Name ?? "";
                    if (name == "SystemAttribute")
                    {
                        systems.Add((method, attribute));
                        break;
                    }
                    if (name == "StartupAttribute")
                    {
                        startups.Add(method);
                        break;
                    }
                }
            }

            // Deterministic order, matching the reflection path: by declaring
            // type full name, then method name. The declaration's `Order`
            // decides the schedule, never this discovery order.
            Comparison<IMethodSymbol> order = static (left, right) =>
            {
                int byType = string.CompareOrdinal(
                    left.ContainingType.ToDisplayString(FullyQualified),
                    right.ContainingType.ToDisplayString(FullyQualified));
                return byType != 0 ? byType : string.CompareOrdinal(left.Name, right.Name);
            };
            systems.Sort((left, right) => order(left.Method, right.Method));
            startups.Sort(order);

            StringBuilder source = new StringBuilder();
            source.AppendLine("// <auto-generated> by PillCSharpRuntimeMacros.EcsAotRegistryGenerator");
            source.AppendLine("// Direct, reflection-free registrations of every [System]/[Startup]");
            source.AppendLine("// method, for the NativeAOT shipping posture. Do not edit.</auto-generated>");
            source.AppendLine($"namespace {AotRegistryNamespace}");
            source.AppendLine("{");
            source.AppendLine("    internal static class GeneratedAotRegistry");
            source.AppendLine("    {");

            List<string> systemEntries = new();
            List<string> startupEntries = new();

            for (int index = 0; index < systems.Count; index++)
            {
                (IMethodSymbol method, AttributeData declaration) = systems[index];
                string receiver = method.ContainingType.ToDisplayString(FullyQualified);
                string? entry = EmitSystem(
                    source, method, declaration, receiver, index, systemEntries, out string? refusal);
                if (entry is null)
                {
                    if (refusal is not null)
                    {
                        context.ReportDiagnostic(Diagnostic.Create(
                            new DiagnosticDescriptor(
                                "PCS0003",
                                "A [System] condition cannot be called",
                                "System {0} declares a RunIf condition the AOT registry cannot call: {1}",
                                "PillCSharpRuntimeMacros",
                                DiagnosticSeverity.Error,
                                isEnabledByDefault: true),
                            method.Locations.FirstOrDefault(),
                            method.ToDisplayString(FullyQualified),
                            refusal));
                        continue;
                    }
                    context.ReportDiagnostic(Diagnostic.Create(
                        new DiagnosticDescriptor(
                            "PCS0001",
                            "Unsupported SystemAttribute signature",
                            "System {0} has an unsupported signature for the AOT registry " +
                            "(expected up to six parameters: query parameters plus at most one Commands)",
                            "PillCSharpRuntimeMacros",
                            DiagnosticSeverity.Error,
                            isEnabledByDefault: true),
                        method.Locations.FirstOrDefault(),
                        method.ToDisplayString(FullyQualified)));
                }
            }

            for (int index = 0; index < startups.Count; index++)
            {
                IMethodSymbol method = startups[index];
                string receiver = method.ContainingType.ToDisplayString(FullyQualified);
                string? refusal = EmitStartup(source, method, receiver, index, startupEntries);
                if (refusal is not null)
                    context.ReportDiagnostic(Diagnostic.Create(
                        new DiagnosticDescriptor(
                            "PCS0002",
                            "Unsupported StartupAttribute signature",
                            "Startup {0} has an unsupported signature for the AOT registry: {1}",
                            "PillCSharpRuntimeMacros",
                            DiagnosticSeverity.Error,
                            isEnabledByDefault: true),
                        method.Locations.FirstOrDefault(),
                        method.ToDisplayString(FullyQualified),
                        refusal));
            }

            // The registry arrays ProjectHost's AOT branch reads at startup.
            source.AppendLine(
                "        internal static readonly global::TracyLive.Loader.AotSystemRegistration[] Systems =");
            source.AppendLine("            new global::TracyLive.Loader.AotSystemRegistration[]");
            source.AppendLine("            {");
            foreach (string entry in systemEntries)
                source.AppendLine(entry + ",");
            source.AppendLine("            };");
            source.AppendLine();
            source.AppendLine(
                "        internal static readonly global::TracyLive.Loader.AotStartupRegistration[] Startups =");
            source.AppendLine("            new global::TracyLive.Loader.AotStartupRegistration[]");
            source.AppendLine("            {");
            foreach (string entry in startupEntries)
                source.AppendLine(entry + ",");
            source.AppendLine("            };");

            // Install through a module initializer so the host never reflects.
            // The registration arrays are read by ProjectHost's AOT branch.
            source.AppendLine("    }");
            source.AppendLine();
            source.AppendLine("    internal static class AotModule");
            source.AppendLine("    {");
            source.AppendLine("        [global::System.Runtime.CompilerServices.ModuleInitializer]");
            source.AppendLine("        internal static void Install()");
            source.AppendLine("        {");
            source.AppendLine("            global::TracyLive.Loader.AotRegistry.Install(");
            source.AppendLine("                GeneratedAotRegistry.Systems,");
            source.AppendLine("                GeneratedAotRegistry.Startups,");
            // The project assembly (this compilation's root) is what backs the
            // component manifest; the merged AOT image keeps csharp_runtime's
            // types in their own assembly, so the manifest must not enumerate
            // the runtime assembly instead.
            source.AppendLine("                typeof(GeneratedAotRegistry).Assembly);");
            source.AppendLine("        }");
            source.AppendLine("    }");
            source.AppendLine("}");
            context.AddSource("GeneratedAotRegistry.g.cs", SourceText.From(source.ToString(), Encoding.UTF8));
        }

        /// <summary>Classify one system parameter as query or Commands.</summary>
        private static bool IsCommands(ITypeSymbol type) =>
            type.ToDisplayString(FullyQualified) == CommandsTypeName;

        /// <summary>
        /// Classify one parameter as a resource declaration, returning its
        /// access mode: 0 for <c>Res&lt;T&gt;</c>, 1 for <c>ResMut&lt;T&gt;</c>.
        /// </summary>
        /// <remarks>
        /// Matched on the open generic's display prefix rather than on a symbol
        /// comparison, because the generator has no reference to the runtime's
        /// closed types. The interface check is what keeps that prefix from
        /// matching an unrelated type someone happens to name the same way.
        /// </remarks>
        private static byte? ResourceMode(ITypeSymbol type)
        {
            bool declaresContract = false;
            foreach (INamedTypeSymbol implemented in type.AllInterfaces)
            {
                if (implemented.ToDisplayString(FullyQualified) == IResourceParameterTypeName)
                {
                    declaresContract = true;
                    break;
                }
            }
            if (!declaresContract)
                return null;
            string name = type.ToDisplayString(FullyQualified);
            if (name.StartsWith(ResMutTypeName, StringComparison.Ordinal))
                return 1;
            if (name.StartsWith(ResTypeName, StringComparison.Ordinal))
                return 0;
            return null;
        }

        /// <summary>Whether a type is assignable to the IQueryDescriptor contract.</summary>
        private static bool IsQueryType(ITypeSymbol type)
        {
            if (type.TypeKind == TypeKind.Interface &&
                type.ToDisplayString(FullyQualified) == IQueryDescriptorTypeName)
                return true;
            foreach (INamedTypeSymbol implemented in type.AllInterfaces)
            {
                if (implemented.ToDisplayString(FullyQualified) == IQueryDescriptorTypeName)
                    return true;
            }
            return false;
        }

        /// <summary>Emit one system's query holders, runner, and registry entry.</summary>
        private static string? EmitSystem(
            StringBuilder source,
            IMethodSymbol method,
            AttributeData declaration,
            string receiver,
            int index,
            List<string> entries,
            out string? refusal)
        {
            refusal = null;
            // Classify parameters: queries and at most one Commands, within
            // the same parameter budget the runtime enforces.
            if (method.Parameters.Length == 0 || method.Parameters.Length > MaxSystemParameters)
                return null;
            bool usesCommands = false;
            foreach (IParameterSymbol parameter in method.Parameters)
            {
                if (IsCommands(parameter.Type))
                {
                    if (usesCommands)
                        return null; // second Commands parameter: unsupported
                    usesCommands = true;
                    continue;
                }
                if (ResourceMode(parameter.Type) is not null)
                    continue;
                if (!IsQueryType(parameter.Type))
                    return null; // non-query, non-Commands parameter: unsupported
            }

            // One static query holder per query parameter, so the runner can
            // hand each of them to the method in declared order.
            string[] queryFields = new string[method.Parameters.Length];
            for (int parameterIndex = 0; parameterIndex < method.Parameters.Length; parameterIndex++)
            {
                IParameterSymbol parameter = method.Parameters[parameterIndex];
                if (IsCommands(parameter.Type) || ResourceMode(parameter.Type) is not null)
                    continue;
                string field = $"s_query_{index}_{parameterIndex}";
                string queryType = parameter.Type.ToDisplayString(FullyQualified);
                source.AppendLine($"        private static readonly {queryType} {field} = new {queryType}();");
                queryFields[parameterIndex] = field;
            }

            string runMethod = $"RunSystem_{index}";
            // Build the invocation argument list in declared parameter order,
            // alongside the descriptor and name arrays the runtime consumes.
            List<string> arguments = new();
            List<string> descriptors = new();
            List<string> queryNames = new();
            List<string> resourceAccesses = new();
            for (int parameterIndex = 0; parameterIndex < method.Parameters.Length; parameterIndex++)
            {
                IParameterSymbol parameter = method.Parameters[parameterIndex];
                if (IsCommands(parameter.Type))
                {
                    arguments.Add("default");
                    continue;
                }
                // Res<T> and ResMut<T> carry no state - every access re-asks
                // the host - so the default value is the whole parameter, and
                // only the declaration it implies has to be recorded.
                if (ResourceMode(parameter.Type) is byte mode)
                {
                    arguments.Add("default");
                    string resource = ((INamedTypeSymbol)parameter.Type)
                        .TypeArguments[0].ToDisplayString(FullyQualified);
                    resourceAccesses.Add(
                        $"global::TracyLive.Loader.ResourceAccessRegistration" +
                        $".Of<{resource}>((byte){mode})");
                    continue;
                }
                arguments.Add(queryFields[parameterIndex]);
                descriptors.Add($"{queryFields[parameterIndex]}.Descriptor");
                queryNames.Add($"\"{parameter.Name}\"");
            }
            // Conditions are resolved at generation time, so the emitted runner
            // is `conditions && body` with no reflection at run time. Each
            // condition is held to the read-only half of the parameter
            // contract; one the registry cannot call refuses the system rather
            // than emitting code that would not compile.
            List<string> conditionGuards = new();
            ITypeSymbol conditionOwner = ConditionOwner(declaration, method);
            int conditionIndex = 0;
            foreach (string conditionName in ConditionNames(declaration))
            {
                IMethodSymbol? condition = conditionOwner
                    .GetMembers(conditionName)
                    .OfType<IMethodSymbol>()
                    .FirstOrDefault(member =>
                        member.IsStatic && member.MethodKind == MethodKind.Ordinary);
                if (condition is null)
                {
                    refusal = $"'{conditionName}' is not a static method on " +
                        conditionOwner.ToDisplayString();
                    return null;
                }
                if (condition.ReturnType.SpecialType != SpecialType.System_Boolean)
                {
                    refusal = $"'{conditionName}' returns {condition.ReturnType.ToDisplayString()}, not bool";
                    return null;
                }
                if (condition.Parameters.Length > MaxSystemParameters)
                {
                    refusal = $"'{conditionName}' declares {condition.Parameters.Length} parameters, " +
                        $"over the {MaxSystemParameters}-parameter limit";
                    return null;
                }
                List<string> conditionArguments = new();
                for (int parameterIndex = 0; parameterIndex < condition.Parameters.Length; parameterIndex++)
                {
                    IParameterSymbol parameter = condition.Parameters[parameterIndex];
                    if (IsCommands(parameter.Type))
                    {
                        refusal = $"'{conditionName}' takes Commands; conditions are read-only";
                        return null;
                    }
                    if (ResourceMode(parameter.Type) is byte mode)
                    {
                        if (mode != 0)
                        {
                            refusal = $"'{conditionName}' takes a writable resource; conditions are read-only";
                            return null;
                        }
                        conditionArguments.Add("default");
                        continue;
                    }
                    if (!IsQueryType(parameter.Type))
                    {
                        refusal = $"'{conditionName}' parameter '{parameter.Name}' is not a query";
                        return null;
                    }
                    if (QueryWrites(parameter.Type))
                    {
                        refusal = $"'{conditionName}' query writes a component; conditions are read-only";
                        return null;
                    }
                    string conditionField = $"s_condition_{index}_{conditionIndex}_{parameterIndex}";
                    string conditionQueryType = parameter.Type.ToDisplayString(FullyQualified);
                    source.AppendLine(
                        $"        private static readonly {conditionQueryType} {conditionField} = " +
                        $"new {conditionQueryType}();");
                    conditionArguments.Add(conditionField);
                }
                string conditionMethod = $"Condition_{index}_{conditionIndex}";
                source.AppendLine(
                    $"        private static bool {conditionMethod}() => " +
                    $"{condition.ContainingType.ToDisplayString(FullyQualified)}.{condition.Name}" +
                    $"({string.Join(", ", conditionArguments)});");
                conditionGuards.Add($"            if (!{conditionMethod}())");
                conditionGuards.Add("                return;");
                conditionIndex++;
            }
            string args = string.Join(", ", arguments);
            source.AppendLine($"        private static void {runMethod}()");
            source.AppendLine("        {");
            foreach (string guard in conditionGuards)
                source.AppendLine(guard);
            source.AppendLine($"            {receiver}.{method.Name}({args});");
            source.AppendLine("        }");

            // The declaration's values, read the same way the reflection path
            // reads them: a null `Name` keeps the reflected identity, and a
            // missing argument keeps the attribute's own default.
            string? declaredName = StringArgument(declaration, "Name");
            string name = declaredName is { Length: > 0 }
                ? declaredName
                : $"{ReflectedTypeName(method)}.{method.Name}";
            int declaredOrder = IntArgument(declaration, "Order") ?? 0;
            int declaredPhase = IntArgument(declaration, "RunPhase") ?? (int)RunPhaseProject;
            bool declaredEditMode = BoolArgument(declaration, "RunInEditMode") ?? false;
            string conditions = ConditionEntries(declaration, method);
            string nameLiteral = SymbolDisplay.FormatLiteral(name, quote: true);
            entries.Add(
                $"            new global::TracyLive.Loader.AotSystemRegistration(" +
                $"{nameLiteral}, new global::TracyLive.QueryDescriptor?[] {{ {string.Join(", ", descriptors)} }}, " +
                $"new string[] {{ {string.Join(", ", queryNames)} }}, " +
                $"{(usesCommands ? "true" : "false")}, " +
                $"new global::TracyLive.Loader.ResourceAccessRegistration[] " +
                $"{{ {string.Join(", ", resourceAccesses)} }}, " +
                $"{declaredOrder}, (global::TracyLive.RunPhase){declaredPhase}, " +
                $"{(declaredEditMode ? "true" : "false")}, " +
                $"{conditions}, " +
                $"{runMethod})");
            return name;
        }

        /// <summary>The `Project` phase's number, the attribute's default.</summary>
        private const int RunPhaseProject = 1;

        /// <summary>
        /// The declaring type's name as reflection reports it, so a system's
        /// AOT registration carries the same identity (no `global::` prefix)
        /// the reflection path builds from `Type.FullName`.
        /// </summary>
        private static string ReflectedTypeName(IMethodSymbol method)
        {
            string qualified = method.ContainingType.ToDisplayString(FullyQualified);
            return qualified.StartsWith("global::", StringComparison.Ordinal)
                ? qualified.Substring("global::".Length)
                : qualified;
        }

        /// <summary>One named attribute argument, or null when it was not written.</summary>
        private static TypedConstant? NamedArgument(AttributeData attribute, string name)
        {
            foreach (KeyValuePair<string, TypedConstant> argument in attribute.NamedArguments)
            {
                if (argument.Key == name)
                    return argument.Value;
            }
            return null;
        }

        /// <summary>One named string argument, or null.</summary>
        private static string? StringArgument(AttributeData attribute, string name) =>
            NamedArgument(attribute, name) is { Value: string text } ? text : null;

        /// <summary>One named integer argument (an enum arrives as its number), or null.</summary>
        private static int? IntArgument(AttributeData attribute, string name) =>
            NamedArgument(attribute, name) is { Value: int number } ? number : null;

        /// <summary>One named boolean argument, or null.</summary>
        private static bool? BoolArgument(AttributeData attribute, string name) =>
            NamedArgument(attribute, name) is { Value: bool flag } ? flag : null;

        /// <summary>The declared `RunIf` condition names, in evaluation order.</summary>
        private static IEnumerable<string> ConditionNames(AttributeData declaration)
        {
            if (NamedArgument(declaration, "RunIf") is not { Kind: TypedConstantKind.Array } array ||
                array.IsNull)
                yield break;
            foreach (TypedConstant argument in array.Values)
            {
                if (argument.Value is string name)
                    yield return name;
            }
        }

        /// <summary>The type the conditions are declared on: `RunIfType` or the system's own.</summary>
        private static ITypeSymbol ConditionOwner(AttributeData declaration, IMethodSymbol method) =>
            NamedArgument(declaration, "RunIfType") is { Value: ITypeSymbol declared }
                ? declared
                : method.ContainingType;

        /// <summary>Whether a query type declares any write term, nested ones included.</summary>
        private static bool QueryWrites(ITypeSymbol type)
        {
            if (type is not INamedTypeSymbol named)
                return false;
            if (named.ToDisplayString(FullyQualified).StartsWith(WriteTypeName, StringComparison.Ordinal))
                return true;
            foreach (ITypeSymbol argument in named.TypeArguments)
            {
                if (QueryWrites(argument))
                    return true;
            }
            return false;
        }

        /// <summary>
        /// Emit every declared `RunIf` condition, each with the type it is
        /// declared on: the system's own type, or the `RunIfType` the
        /// declaration names.
        /// </summary>
        private static string ConditionEntries(AttributeData declaration, IMethodSymbol method)
        {
            if (NamedArgument(declaration, "RunIf") is not { Kind: TypedConstantKind.Array } array ||
                array.IsNull)
                return "new global::TracyLive.Loader.ManagedCondition[0]";
            string owner = NamedArgument(declaration, "RunIfType") is { Value: ITypeSymbol declared }
                ? declared.ToDisplayString(FullyQualified)
                : method.ContainingType.ToDisplayString(FullyQualified);
            List<string> entries = new();
            foreach (TypedConstant argument in array.Values)
            {
                if (argument.Value is not string condition)
                    continue;
                entries.Add(
                    "new global::TracyLive.Loader.ManagedCondition(" +
                    $"{SymbolDisplay.FormatLiteral(condition, quote: true)}, typeof({owner}))");
            }
            return "new global::TracyLive.Loader.ManagedCondition[] { " +
                string.Join(", ", entries) + " }";
        }

        /// <summary>
        /// Emit one startup's runner and registry entry, or say why the
        /// signature cannot be called directly.
        /// </summary>
        /// <remarks>
        /// The contract is the one `ProjectHost.CreateStartup` enforces on the
        /// reflection path: a void method, at most `MaxSystemParameters`
        /// parameters, one `Commands` at most, and every other parameter a
        /// `Res&lt;T&gt;`/`ResMut&lt;T&gt;`. A signature the runtime would
        /// refuse must be refused here instead, at generation time, so an AOT
        /// build never ships a registry that cannot run.
        /// </remarks>
        private static string? EmitStartup(
            StringBuilder source,
            IMethodSymbol method,
            string receiver,
            int index,
            List<string> entries)
        {
            string? refusal = StartupRefusal(method);
            if (refusal is not null)
                return refusal;
            string runMethod = $"RunStartup_{index}";
            // Every startup parameter - Commands, Res<T>, ResMut<T> - is
            // stateless, so each is passed as its default, as the reflection
            // path does.
            string arguments = string.Join(", ", method.Parameters.Select(_ => "default"));
            source.AppendLine(
                $"        private static void {runMethod}() => {receiver}.{method.Name}({arguments});");
            string name = $"{ReflectedTypeName(method)}.{method.Name}";
            entries.Add(
                $"            new global::TracyLive.Loader.AotStartupRegistration(" +
                $"\"{name}\", {runMethod})");
            return null;
        }

        /// <summary>
        /// Why the AOT registry cannot call this startup directly, or null.
        /// </summary>
        private static string? StartupRefusal(IMethodSymbol method)
        {
            if (!method.ReturnsVoid)
                return "a startup must return void";
            if (method.Parameters.Length > MaxSystemParameters)
                return $"a startup takes at most {MaxSystemParameters} parameters";
            bool usesCommands = false;
            foreach (IParameterSymbol parameter in method.Parameters)
            {
                if (IsCommands(parameter.Type))
                {
                    if (usesCommands)
                        return "Commands is declared more than once";
                    usesCommands = true;
                    continue;
                }
                if (ResourceMode(parameter.Type) is null)
                    return $"{parameter.Name} is not a Commands, Res<T> or ResMut<T> parameter";
            }
            return null;
        }
    }
}
