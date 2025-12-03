# Pill Project Coding Standards  

This document defines coding standards for the Pill project, ensuring consistency, readability, maintainability, and idiomatic language practices.  

## 1. General Principles  
1. Prioritize safety, clarity, and performance.  
2. Prefer idiomatic Rust patterns and avoid unnecessary abstractions.  
3. Favor expressiveness without sacrificing simplicity.  
4. Always import types, functions and macros explicitly at the top of the file (e.g. `use anyhow::Result;`).  

## 2. Naming Conventions  
1. Types (structs, enums, traits): PascalCase.  
2. Functions, variables, modules, resources (texture, sound, model files), shader/material parameters: snake_case.  
3. Constants and statics: SCREAMING_SNAKE_CASE.  
4. Generics: short uppercase letters (`T`, `E`, `R`), descriptive when needed (Request, Response).  
5. Use fully descriptive, expressive names rather than abbreviations — clarity is more important than brevity.  
6. Avoid abbreviations such as `env`, `dt`, `tex`, `ctx`, `fmt`, `rot`, `pos`, or similar. The only allowed exceptions are the Rust keywords like `mut`, `ref`, `ptr`, `len`, `idx`, and `dyn`.
7. Using long names is encouraged as they make intent clearer.  

## 3. Project Structure  
1. Organize modules using clear directory hierarchies.  
2. Keep files small and cohesive.  
3. Name crates and modules using snake_case.  
4. Place universal, shared, or cross-cutting utilities in the pill_core module to keep functionality centralized and avoid duplication.  
5. Each folder must contain a mod.rs file to clearly define the module's public API and re-export relevant items.  

## 4. Error handling  
1. Use `Result<T, E>` for recoverable errors.  
2. In binary crates, prefer `anyhow::Result` and attach context using `.context("...")?` for clearer diagnostics.  
3. Use the `?` operator extensively for propagating errors.  
4. Avoid panics except for unrecoverable logic errors.  

## 5. Code Style  
1. Follow rustfmt code formatter for formatting.  
2. Follow clippy code linter recommendations unless deviating intentionally; document deviations.  
3. Break long expressions using intermediate variables for readability.  
4. Keep function bodies short and single-responsibility.  

## 6. Documentation  
1. Important game-developer–facing areas (materials, audio, ECS basics API, etc.) must each have a dedicated page in the Pill Guide.  
2. Engine subsystems and internal architecture must be documented in the Engine Internals section of the Pill Guide  
3. Every public function, type, and module must have a `///` doc comment (so it appears correctly in cargo doc) that well explains the purpose of the function  
4. Public APIs functions should include short, runnable examples to clarify usage.  
5. Documentation must be updated whenever related code changes to avoid drift.  

## 7. Testing  
1. Write unit tests for all critical logic, especially pure functions and data transformations.  
2. Write unit tests directly under the code they validate, in the same module.  
3. Avoid complex test setups; extract helpers if needed.  
4. Avoid testing implementation details. Focus on observable behavior and invariants.  
5. Document test purpose clearly with a `///` doc comment so failures provide meaningful insight.  

## 8. Unsafe Code Guidelines  
1. Avoid unsafe unless necessary for performance or FFI.  
2. Encapsulate unsafe blocks behind safe abstractions.  
3. Document why unsafe is required and the invariants it relies on.  

## 9. Logging & Observability  
1. Use the contextual logging macros (`info!(ctx => ...)`, etc.) with a required `LogContext`, unless intentionally using "default".  
2. Configure logging per context using strings like ecs = debug and follow consistent log-level semantics.  
3. Keep log messages clear and descriptive, avoiding abbreviations.  
4. Use the project's Timer utility when measuring execution time for functions, subsystems, or critical paths.  
5. Structure timing output with nested contexts (`begin_context` / `end_context`) to maintain readable, hierarchical performance data.  

## 10. Code Review Expectations  
1. Ensure PRs are small and focused.
2. Submitted code should not trigger any warnings.
2. Request feedback early for architectural changes.  
3. Review for correctness, clarity, and idiomatic usage.

## 11. Version Control  
1. Use descriptive commit messages.  
2. Try to keep commits atomic.  
3. Avoid committing generated files.  