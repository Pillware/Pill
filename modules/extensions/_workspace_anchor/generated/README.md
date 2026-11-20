# Workspace anchor for generated extension wrappers

Not an extension. This file exists so the workspace member glob
`extensions/*/generated/*` in `modules/Cargo.toml` always matches something.

The host writes one `host_module_<crate>` wrapper crate per loaded extension
crate into `extensions/<extension>/generated/`. On a fresh clone none exist
yet, and Cargo reads a member glob that matches nothing as a literal path,
which makes every cargo command fail. This README is the guaranteed match;
`modules/Cargo.toml` excludes `extensions/_workspace_anchor`, so it never
becomes a member.
