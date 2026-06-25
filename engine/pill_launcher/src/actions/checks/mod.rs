// Checks sub-module: validation and benchmarking actions.
//
// - check_code: fast compile-check of engine crates (no game code).
// - check_wasm: build WASM, smoke-test dev server, check size budget.
// - benchmarks: performance and artifact-size benchmarks.

pub mod check_code;
pub mod check_wasm;
