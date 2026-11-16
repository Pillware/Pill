//! The extension ABI revision shared by the host and every module.
//!
//! # Responsibilities
//!
//! - Define the extension ABI revision the host and every module read.

/// Revision number of the extension C-ABI export contract.
///
/// The host and every extension compile against this one constant. The
/// `#[pill_module]` macro generates the `pill_module_abi_version` export from
/// it, so a module and the host built from one workspace always agree on the
/// revision.
///
/// A module built against a different revision is rejected at load time with
/// a clear error instead of misbehaving silently. A module that exports no
/// revision is rejected too.
pub const MODULE_ABI_VERSION: u32 = 1;
