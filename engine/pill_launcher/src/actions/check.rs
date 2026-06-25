// Check actions: code validation and WASM smoke-testing.
//
// These are thin re-exports from the checks/ module — the actual logic lives there.

pub(crate) use crate::actions::checks::check_code::{do_check_code, CheckCode};
pub(crate) use crate::actions::checks::check_wasm::{do_check_wasm, CheckWasm};
