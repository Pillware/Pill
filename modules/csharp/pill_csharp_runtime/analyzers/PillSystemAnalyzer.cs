// Compile-time rules for the hot-reloadable gameplay project.
//
// Responsibilities:
// - Reject system declarations the loader cannot register, or would register
//   into a shape that never runs.
// - Reject awaits inside a system, whose continuation would resume after the
//   scheduled scope is gone.
// - Reject the patterns that keep a collectible AssemblyLoadContext alive, so
//   a hot reload does not silently leak the previous assembly.
// - Reject component layouts whose declared size cannot match the runtime's.
//
// Design:
// - The analyzer runs against `project_cs`, the assembly a developer edits and
//   the host hot-reloads. Every rule here describes something the runtime
//   either refuses later (at the next host start, or at the next reload) or
//   cannot detect at all; reporting it at the keystroke is the difference
//   between a build error and a silent misbehaviour.
// - Symbol and syntax actions only: no semantic model walking beyond what a
//   single declaration needs, so the analyzer stays cheap enough to run on
//   every keystroke.

using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Diagnostics;

namespace PillScriptAnalyzers
{
    /// <summary>Reports the gameplay scripting rules at build time.</summary>
    [DiagnosticAnalyzer(LanguageNames.CSharp)]
    public sealed class PillSystemAnalyzer : DiagnosticAnalyzer
    {
        private const string SystemAttributeName = "TracyLive.SystemAttribute";
        private const string StartupAttributeName = "TracyLive.StartupAttribute";

        /// <summary>The managed Commands type; a body may not reference it.</summary>
        private const string CommandsType = "TracyLive.Commands";

        /// <summary>The engine facade; a body may not reference it.</summary>
        private const string EngineFacadeType = "TracyLive.Engine";

        /// <summary>Matches the loader's own parameter budget.</summary>
        private const int MaxSystemParameters = 6;

        /// <summary>Types whose construction starts work outside any frame.</summary>
        private static readonly ImmutableHashSet<string> BackgroundWorkTypes =
            ImmutableHashSet.Create(
                "System.Threading.Thread",
                "System.Threading.Timer",
                "System.Timers.Timer",
                "System.Threading.RegisteredWaitHandle");

        /// <summary>Calls that start work outside any frame, as `Type.Member`.</summary>
        private static readonly ImmutableHashSet<string> BackgroundWorkCalls =
            ImmutableHashSet.Create(
                "System.Threading.Tasks.Task.Run",
                "System.Threading.Tasks.Task.Delay",
                "System.Threading.ThreadPool.QueueUserWorkItem",
                "System.Threading.ThreadPool.UnsafeQueueUserWorkItem",
                "System.Threading.Tasks.TaskFactory.StartNew");

        public override ImmutableArray<DiagnosticDescriptor> SupportedDiagnostics =>
            ImmutableArray.Create(
                PillDiagnostics.MustBeStatic,
                PillDiagnostics.MustReturnVoid,
                PillDiagnostics.ParameterBudget,
                PillDiagnostics.WritableCondition,
                PillDiagnostics.UnknownCondition,
                PillDiagnostics.NoAsyncSystem,
                PillDiagnostics.NoEngineCallsInParallelBody,
                PillDiagnostics.MutableStaticState,
                PillDiagnostics.StaticEventBlocksUnload,
                PillDiagnostics.BackgroundWorkBlocksUnload,
                PillDiagnostics.GcHandleBlocksUnload,
                PillDiagnostics.FinalizerDelaysUnload,
                PillDiagnostics.UnsupportedLayoutKind,
                PillDiagnostics.PackIsIgnored,
                PillDiagnostics.UnsupportedFieldType);

        public override void Initialize(AnalysisContext context)
        {
            context.ConfigureGeneratedCodeAnalysis(GeneratedCodeAnalysisFlags.None);
            context.EnableConcurrentExecution();
            context.RegisterSymbolAction(AnalyzeMethod, SymbolKind.Method);
            context.RegisterSymbolAction(AnalyzeNamedType, SymbolKind.NamedType);
            context.RegisterSymbolAction(AnalyzeField, SymbolKind.Field);
            context.RegisterSymbolAction(AnalyzeEvent, SymbolKind.Event);
            context.RegisterSymbolAction(AnalyzeProperty, SymbolKind.Property);
            context.RegisterOperationAction(
                AnalyzeObjectCreation, OperationKind.ObjectCreation);
            context.RegisterOperationAction(
                AnalyzeInvocation, OperationKind.Invocation);
            context.RegisterSyntaxNodeAction(
                AnalyzeParallelBody, SyntaxKind.InvocationExpression);
        }

        // ------------------------------------------------------------------
        // System declaration shape and frame scope
        // ------------------------------------------------------------------

        /// <summary>Check one method's ECS attributes, shape and async-ness.</summary>
        private static void AnalyzeMethod(SymbolAnalysisContext context)
        {
            var method = (IMethodSymbol)context.Symbol;

            // A finalizer is the one method shape checked regardless of
            // attributes, because it delays the unload of the whole assembly.
            if (method.MethodKind == MethodKind.Destructor)
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.FinalizerDelaysUnload,
                    method.Locations.FirstOrDefault(),
                    method.ContainingType?.Name ?? method.Name));
                return;
            }

            string? attributeName = EcsAttributeName(method);
            if (attributeName is null)
                return;

            // A [System]'s RunIf conditions are part of its declaration: a name
            // that resolves to nothing, or a method that could write or does
            // not return bool, is refused at the keystroke. Discovery makes the
            // same refusals at load time.
            if (attributeName == SystemAttributeName)
            {
                foreach (AttributeData declaration in method.GetAttributes())
                {
                    if (declaration.AttributeClass?.ToDisplayString() == SystemAttributeName)
                    {
                        AnalyzeConditions(context, method, declaration);
                        break;
                    }
                }
            }

            if (!method.IsStatic)
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.MustBeStatic,
                    method.Locations.FirstOrDefault(),
                    method.Name,
                    ShortAttributeName(attributeName)));
            }

            if (!method.ReturnsVoid)
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.MustReturnVoid,
                    method.Locations.FirstOrDefault(),
                    method.Name,
                    method.ReturnType.ToDisplayString()));
            }

            // `async void` reports void, so the loader accepts it and the
            // continuation escapes the frame. The modifier is the only signal.
            if (method.IsAsync)
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.NoAsyncSystem,
                    method.Locations.FirstOrDefault(),
                    method.Name));
            }

            // Only systems have a parameter budget the scheduler enforces; a
            // startup's Commands and Res/ResMut parameters are checked at load.
            if (attributeName == SystemAttributeName &&
                (method.Parameters.Length == 0 || method.Parameters.Length > MaxSystemParameters))
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.ParameterBudget,
                    method.Locations.FirstOrDefault(),
                    method.Name,
                    method.Parameters.Length,
                    MaxSystemParameters));
            }
        }

        /// <summary>The ECS attribute on a method, or null when it has none.</summary>
        private static string? EcsAttributeName(IMethodSymbol method)
        {
            foreach (AttributeData attribute in method.GetAttributes())
            {
                string? name = attribute.AttributeClass?.ToDisplayString();
                if (name == SystemAttributeName || name == StartupAttributeName)
                    return name;
            }
            return null;
        }

        /// <summary>`TracyLive.SystemAttribute` renders as `System`.</summary>
        private static string ShortAttributeName(string fullName)
        {
            int lastDot = fullName.LastIndexOf('.');
            string name = lastDot < 0 ? fullName : fullName.Substring(lastDot + 1);
            return name.EndsWith("Attribute")
                ? name.Substring(0, name.Length - "Attribute".Length)
                : name;
        }

        // ------------------------------------------------------------------
        // Parallel row bodies
        // ------------------------------------------------------------------

        /// <summary>Reject engine and Commands use inside a parallel row body.</summary>
        /// <remarks>
        /// A `ForEachParallel` body runs on pool worker threads, where workers
        /// carry no scheduled scope and every engine entry point refuses the
        /// call - typically as a per-frame error storm from inside the pass.
        /// The rule is a best effort: it inspects lambda arguments of calls
        /// named `ForEachParallel` and flags the first identifier whose symbol
        /// is the enclosing system's `Commands` parameter or the `Engine`
        /// facade. Arbitrary aliasing into the body is out of scope - the
        /// runtime refusal remains the enforcement of record.
        /// </remarks>
        private static void AnalyzeParallelBody(SyntaxNodeAnalysisContext context)
        {
            var invocation = (InvocationExpressionSyntax)context.Node;
            if (MethodNameOf(invocation.Expression) != "ForEachParallel")
                return;

            foreach (ArgumentSyntax argument in invocation.ArgumentList.Arguments)
            {
                if (argument.Expression is not AnonymousFunctionExpressionSyntax lambda)
                    continue;
                SyntaxNode? body = lambda.Body;
                if (body is null)
                    continue;

                foreach (SyntaxNode node in body.DescendantNodesAndSelf())
                {
                    if (node is not IdentifierNameSyntax identifier)
                        continue;
                    ISymbol? symbol = context.SemanticModel
                        .GetSymbolInfo(identifier, context.CancellationToken)
                        .Symbol;
                    bool touchesEngine =
                        (symbol is IParameterSymbol parameter &&
                         parameter.Type.ToDisplayString() == CommandsType) ||
                        (symbol is INamedTypeSymbol type &&
                         type.ToDisplayString() == EngineFacadeType);
                    if (!touchesEngine)
                        continue;
                    context.ReportDiagnostic(Diagnostic.Create(
                        PillDiagnostics.NoEngineCallsInParallelBody,
                        identifier.GetLocation(),
                        identifier.Identifier.ValueText));
                    // One finding per body: the first names the pattern, and
                    // the edit that fixes it usually removes the rest.
                    return;
                }
            }
        }

        /// <summary>The simple name a call expression names, or null.</summary>
        private static string? MethodNameOf(ExpressionSyntax expression)
        {
            switch (expression)
            {
                case IdentifierNameSyntax identifier:
                    return identifier.Identifier.ValueText;
                case GenericNameSyntax generic:
                    return generic.Identifier.ValueText;
                case MemberAccessExpressionSyntax member:
                    return MethodNameOf(member.Name);
                default:
                    return null;
            }
        }

        /// <summary>Check one [System] declaration's RunIf conditions.</summary>
        private static void AnalyzeConditions(
            SymbolAnalysisContext context, IMethodSymbol method, AttributeData declaration)
        {
            TypedConstant? runIf = null;
            TypedConstant? runIfType = null;
            foreach (KeyValuePair<string, TypedConstant> argument in declaration.NamedArguments)
            {
                if (argument.Key == "RunIf")
                    runIf = argument.Value;
                else if (argument.Key == "RunIfType")
                    runIfType = argument.Value;
            }
            if (runIf is not { Kind: TypedConstantKind.Array } array || array.IsNull)
                return;
            ITypeSymbol owner = runIfType?.Value is ITypeSymbol declared
                ? declared
                : method.ContainingType;
            foreach (TypedConstant entry in array.Values)
            {
                if (entry.Value is not string name)
                    continue;
                IMethodSymbol? condition = owner.GetMembers(name)
                    .OfType<IMethodSymbol>()
                    .FirstOrDefault(candidate =>
                        candidate.IsStatic && candidate.MethodKind == MethodKind.Ordinary);
                if (condition is null)
                {
                    context.ReportDiagnostic(Diagnostic.Create(
                        PillDiagnostics.UnknownCondition,
                        method.Locations.FirstOrDefault(),
                        name,
                        owner.Name));
                    continue;
                }
                string? reason = ConditionRefusal(condition);
                if (reason is not null)
                {
                    context.ReportDiagnostic(Diagnostic.Create(
                        PillDiagnostics.WritableCondition,
                        method.Locations.FirstOrDefault(),
                        name,
                        reason));
                }
            }
        }

        /// <summary>Why one condition cannot be a read-only gate, or null.</summary>
        private static string? ConditionRefusal(IMethodSymbol condition)
        {
            if (condition.ReturnType.SpecialType != SpecialType.System_Boolean)
                return $"returns {condition.ReturnType.ToDisplayString()} instead of bool";
            if (condition.Parameters.Length > MaxSystemParameters)
                return $"declares {condition.Parameters.Length} parameters, over the " +
                    $"{MaxSystemParameters}-parameter limit";
            foreach (IParameterSymbol parameter in condition.Parameters)
            {
                if (parameter.Type.ToDisplayString() == CommandsType)
                    return "takes Commands";
                if (parameter.Type.ToDisplayString().StartsWith(
                        "TracyLive.ResMut<", StringComparison.Ordinal))
                    return "takes a ResMut<T>";
                if (QueryDeclaresWrite(parameter.Type))
                    return "takes a query that writes a component";
            }
            return null;
        }

        /// <summary>Whether a query type declares any write term, nested ones included.</summary>
        private static bool QueryDeclaresWrite(ITypeSymbol type)
        {
            if (type is not INamedTypeSymbol named)
                return false;
            if (named.ToDisplayString().StartsWith("TracyLive.Write<", StringComparison.Ordinal))
                return true;
            foreach (ITypeSymbol argument in named.TypeArguments)
            {
                if (QueryDeclaresWrite(argument))
                    return true;
            }
            return false;
        }

        // ------------------------------------------------------------------
        // Static state and unloadability
        // ------------------------------------------------------------------

        /// <summary>Flag mutable static fields in types that declare systems.</summary>
        private static void AnalyzeField(SymbolAnalysisContext context)
        {
            var field = (IFieldSymbol)context.Symbol;
            if (!field.IsStatic || field.IsConst || field.IsImplicitlyDeclared)
                return;
            // `readonly` of a mutable reference type is mutable in every way
            // that matters, which is the case Burst's own readonly rule misses,
            // so a readonly List or array is reported too.
            if (field.IsReadOnly && !IsMutableThroughReadOnly(field.Type))
                return;
            if (!DeclaresSystems(field.ContainingType))
                return;
            context.ReportDiagnostic(Diagnostic.Create(
                PillDiagnostics.MutableStaticState,
                field.Locations.FirstOrDefault(),
                field.Name));
        }

        /// <summary>Flag mutable static auto-properties in system-declaring types.</summary>
        private static void AnalyzeProperty(SymbolAnalysisContext context)
        {
            var property = (IPropertySymbol)context.Symbol;
            if (!property.IsStatic || property.SetMethod is null)
                return;
            if (!DeclaresSystems(property.ContainingType))
                return;
            context.ReportDiagnostic(Diagnostic.Create(
                PillDiagnostics.MutableStaticState,
                property.Locations.FirstOrDefault(),
                property.Name));
        }

        /// <summary>A static event roots the collectible load context.</summary>
        private static void AnalyzeEvent(SymbolAnalysisContext context)
        {
            var declared = (IEventSymbol)context.Symbol;
            if (!declared.IsStatic)
                return;
            context.ReportDiagnostic(Diagnostic.Create(
                PillDiagnostics.StaticEventBlocksUnload,
                declared.Locations.FirstOrDefault(),
                declared.Name));
        }

        /// <summary>Reject constructing a thread or timer from gameplay code.</summary>
        private static void AnalyzeObjectCreation(OperationAnalysisContext context)
        {
            var creation = (Microsoft.CodeAnalysis.Operations.IObjectCreationOperation)context.Operation;
            string? type = creation.Type?.ToDisplayString();
            if (type is null || !BackgroundWorkTypes.Contains(type))
                return;
            context.ReportDiagnostic(Diagnostic.Create(
                PillDiagnostics.BackgroundWorkBlocksUnload, creation.Syntax.GetLocation(), type));
        }

        /// <summary>Reject calls that queue work or pin a managed object.</summary>
        private static void AnalyzeInvocation(OperationAnalysisContext context)
        {
            var invocation = (Microsoft.CodeAnalysis.Operations.IInvocationOperation)context.Operation;
            IMethodSymbol target = invocation.TargetMethod;
            string? containingType = target.ContainingType?.ToDisplayString();
            if (containingType is null)
                return;

            if (containingType == "System.Runtime.InteropServices.GCHandle" && target.Name == "Alloc")
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.GcHandleBlocksUnload, invocation.Syntax.GetLocation()));
                return;
            }

            string qualified = containingType + "." + target.Name;
            if (BackgroundWorkCalls.Contains(qualified))
            {
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.BackgroundWorkBlocksUnload,
                    invocation.Syntax.GetLocation(),
                    qualified));
            }
        }

        /// <summary>Whether a type declares at least one attributed ECS method.</summary>
        private static bool DeclaresSystems(INamedTypeSymbol? type)
        {
            if (type is null)
                return false;
            foreach (ISymbol member in type.GetMembers())
            {
                if (member is IMethodSymbol method && EcsAttributeName(method) is not null)
                    return true;
            }
            return false;
        }

        /// <summary>Whether a readonly field of this type is still mutable.</summary>
        private static bool IsMutableThroughReadOnly(ITypeSymbol type)
        {
            // A readonly reference to an array or a collection binds the
            // reference, not the contents.
            if (type.TypeKind == TypeKind.Array)
                return true;
            return type.IsReferenceType && type.SpecialType != SpecialType.System_String;
        }

        // ------------------------------------------------------------------
        // Component layout
        // ------------------------------------------------------------------

        /// <summary>Check the layout attributes and fields of a component struct.</summary>
        private static void AnalyzeNamedType(SymbolAnalysisContext context)
        {
            var type = (INamedTypeSymbol)context.Symbol;
            if (type.TypeKind != TypeKind.Struct || type.IsGenericType || type.IsImplicitlyDeclared)
                return;
            if (!IsComponentCandidate(type))
                return;

            AttributeData? layout = type.GetAttributes().FirstOrDefault(attribute =>
                attribute.AttributeClass?.ToDisplayString()
                    == "System.Runtime.InteropServices.StructLayoutAttribute");

            if (layout is not null)
            {
                string? kind = LayoutKindName(layout);
                bool hasDeclaredSize = NamedArgument(layout, "Size") is int size && size > 0;
                if (kind == "Auto" || (kind == "Explicit" && !hasDeclaredSize))
                {
                    context.ReportDiagnostic(Diagnostic.Create(
                        PillDiagnostics.UnsupportedLayoutKind,
                        type.Locations.FirstOrDefault(),
                        type.Name,
                        kind == "Auto" ? "LayoutKind.Auto" : "LayoutKind.Explicit without a Size"));
                }
                if (NamedArgument(layout, "Pack") is int pack && pack > 0)
                {
                    context.ReportDiagnostic(Diagnostic.Create(
                        PillDiagnostics.PackIsIgnored,
                        type.Locations.FirstOrDefault(),
                        type.Name));
                }
            }

            foreach (IFieldSymbol field in type.GetMembers().OfType<IFieldSymbol>())
            {
                if (field.IsStatic || field.IsConst || field.IsImplicitlyDeclared)
                    continue;
                if (IsSupportedFieldType(field.Type))
                    continue;
                context.ReportDiagnostic(Diagnostic.Create(
                    PillDiagnostics.UnsupportedFieldType,
                    field.Locations.FirstOrDefault(),
                    field.Name,
                    field.Type.ToDisplayString()));
            }
        }

        /// <summary>
        /// Whether a struct looks like a component the host would register.
        /// </summary>
        /// <remarks>
        /// Every struct with instance fields in the project assembly is a
        /// candidate, so a layout rule can reach a struct used only through
        /// `Commands.With` or `Add`. The manifest builder applies the same
        /// over-approximation and skips a type it cannot describe; this
        /// analyzer reports `PILL0403` as an error, so a non-component struct
        /// with a non-blittable field is refused here. Narrowing the trigger
        /// to query terms would lose the command-only case.
        /// </remarks>
        private static bool IsComponentCandidate(INamedTypeSymbol type)
        {
            foreach (AttributeData attribute in type.GetAttributes())
            {
                if (attribute.AttributeClass?.ToDisplayString() == "TracyLive.EcsSharedComponentAttribute")
                    return true;
            }
            // A struct declared in the gameplay assembly with instance fields is
            // a candidate: the manifest builder makes the same over-approximation
            // and validates rather than guesses.
            return type.GetMembers().OfType<IFieldSymbol>()
                .Any(field => !field.IsStatic && !field.IsConst && !field.IsImplicitlyDeclared);
        }

        /// <summary>The `LayoutKind` name a StructLayout attribute names.</summary>
        private static string? LayoutKindName(AttributeData layout)
        {
            if (layout.ConstructorArguments.Length == 0)
                return null;
            TypedConstant kind = layout.ConstructorArguments[0];
            return kind.Value is int value
                ? value switch { 0 => "Sequential", 2 => "Explicit", 3 => "Auto", _ => null }
                : null;
        }

        /// <summary>Read one named argument of an attribute as an int.</summary>
        private static int? NamedArgument(AttributeData attribute, string name)
        {
            foreach (KeyValuePair<string, TypedConstant> argument in attribute.NamedArguments)
            {
                if (argument.Key == name && argument.Value.Value is int value)
                    return value;
            }
            return null;
        }

        /// <summary>Whether a component field may have this type.</summary>
        private static bool IsSupportedFieldType(ITypeSymbol type)
        {
            if (type.TypeKind == TypeKind.Enum)
                return true;
            switch (type.SpecialType)
            {
                // `bool` and `char` are excluded deliberately: their marshalled
                // sizes differ from their runtime sizes, so a manifest built
                // from one disagrees with a row write using the other.
                case SpecialType.System_Boolean:
                case SpecialType.System_Char:
                    return false;
                case SpecialType.System_Byte:
                case SpecialType.System_SByte:
                case SpecialType.System_Int16:
                case SpecialType.System_UInt16:
                case SpecialType.System_Int32:
                case SpecialType.System_UInt32:
                case SpecialType.System_Int64:
                case SpecialType.System_UInt64:
                case SpecialType.System_Single:
                case SpecialType.System_Double:
                case SpecialType.System_IntPtr:
                case SpecialType.System_UIntPtr:
                    return true;
            }
            // A nested value type is checked on its own when the analyzer
            // reaches its declaration, so accepting it here is not a hole.
            return type.IsValueType && !type.IsReferenceType;
        }
    }
}
