//! Helpers shared by the asset rules.

pub(crate) mod color;
pub(crate) mod rtex;
pub(crate) mod vec_math;

use std::path::Path;

use anyhow::{Context, Result};

/// Writes `bytes` to `<path>.part`, then renames it into place, so an interrupted cook never
/// leaves a truncated output that later runs would treat as up to date.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let partial = path.with_extension("part");
    std::fs::write(&partial, bytes).with_context(|| format!("write {partial:?}"))?;
    std::fs::rename(&partial, path).with_context(|| format!("rename {partial:?} to {path:?}"))
}
