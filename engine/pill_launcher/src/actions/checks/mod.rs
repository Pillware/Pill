// Checks sub-module: code validation and WASM smoke-testing.
//
// - check_code: fast compile-check of engine crates (no project code).
// - check_wasm: build WASM, smoke-test dev server, check size budget.

pub mod check_code;
pub mod check_wasm;
