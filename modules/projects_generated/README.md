# Generated project members

The host writes one `host_project_<package>` crate here for the project it
runs (`PROJECT_PATH`). Projects live outside the cargo workspace, so the
generated member that builds them as part of it lives here instead. Everything
in this folder except this README is generated and gitignored; never edit or
remove a member by hand - start the host and let it regenerate it.

This README is also the guaranteed match of the workspace member glob
`projects_generated/*`: Cargo reads a glob that matches nothing as a literal
path and fails, and a fresh clone has no generated member yet.
`modules/Cargo.toml` excludes this file, so it never becomes a member.
