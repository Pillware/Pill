# Workspace anchor for grouped extension crates

Not an extension. This file exists so the workspace member glob
`extensions/*/*/modules/*` in `modules/Cargo.toml` always matches something.

A group folder - a folder under `extensions/` with no `modules/` folder of its
own - holds extension folders, each with its crates under `modules/`. A fresh
clone, or a group whose extensions have not arrived yet, has none of them, and
Cargo reads a member glob that matches nothing as a literal path, which makes
every cargo command fail. This README is the guaranteed match;
`modules/Cargo.toml` excludes `extensions/_workspace_anchor`, so it never
becomes a member.
