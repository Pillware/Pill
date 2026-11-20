// Managed assembly metadata for the scheduler-aware C# runtime.
//
// Responsibilities:
// - Exposes internal discovery and access-model types to the executable tests.
//
// Design:
// - Production consumers use only the public query API; the friend assembly is
//   deliberately limited to csharp_runtime_tests.

using System.Runtime.CompilerServices;

[assembly: InternalsVisibleTo("csharp_runtime_tests")]
