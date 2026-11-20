# Generated editor module wrappers

The host writes one `host_module_<crate>` wrapper crate here for each of the
editor's own modules it loads: a crate directly under `editor/` whose manifest
tags an editor frontend (`editor_frontend` under `[package.metadata.pill]`),
such as `pill_editor_shell`. The wrapper's `cdylib` is the DLL the editor
loads; the module stays a plain library inside it. Everything in this folder
except this README is generated and gitignored; never edit or remove a wrapper
by hand - start the editor and let the host regenerate it.

This README is also the guaranteed match of the workspace member glob
`editor/generated/*`: Cargo reads a glob that matches nothing as a literal
path and fails, and a fresh clone has no generated wrapper yet.
`modules/Cargo.toml` excludes this file, so it never becomes a member.
