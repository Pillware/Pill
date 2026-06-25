// Benchmark actions: performance and artifact-size analysis.
//
// These are thin re-exports from the checks/ module — the actual logic lives there.

pub(crate) use crate::actions::benchmarks::performance_benchmark::Benchmark;
pub(crate) use crate::actions::benchmarks::size_benchmark::SizeBenchmark;
