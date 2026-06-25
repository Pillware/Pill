// Benchmark implementations.
//
// - performance_benchmark: build+run iterations, collect frame-time JSON, print stats.
// - size_benchmark: build + analyze artifact sizes (native folder / WASM binary).

pub mod performance_benchmark;
pub mod size_benchmark;
