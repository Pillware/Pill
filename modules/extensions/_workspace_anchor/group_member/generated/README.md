# Workspace anchor for grouped module wrappers

Not an extension. This file exists so the workspace member glob
`extensions/*/*/generated/*` in `modules/Cargo.toml` always matches something.

The host writes one `host_module_<crate>` wrapper crate per loaded extension
crate into that extension's `generated/` folder - for a grouped extension, the
folder inside the group. Before the host has written any, the glob matches
nothing, and Cargo reads that as a literal path, which makes every cargo
command fail. This README is the guaranteed match; `modules/Cargo.toml`
excludes `extensions/_workspace_anchor`, so it never becomes a member.
