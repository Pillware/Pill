//! The shipping bundle's committed stub: no project, no modules, no renderer.
//!
//! # Responsibilities
//!
//! - Keep `pill_standalone`'s optional path dependency resolvable on a fresh
//!   clone, which is the only reason this crate is committed at all.
//! - Refuse to compile, so nothing can be type-checked or shipped against the
//!   stub in place of a real bundle.
//!
//! # Design
//!
//! `devops/tools/generate_shipping_bundle.py` rewrites this file and the
//! manifest beside it for the project named on its command line: the generated
//! source exports the module table, the renderer choice and the project
//! accessor the `shipping` posture of `pill_standalone` calls, and the
//! manifest gains the dependencies those names need. The stub exports none of
//! them. `--restore-stub` writes these bytes back once a generation is done.
//!
//! A committed generation would be worse than this refusal: it compiles, so
//! `cargo clippy -p pill_standalone --features shipping` and a shipped binary
//! would both type-check against whichever project happened to be generated
//! last. The generator's output is a build artifact of one project, never the
//! tree's state; `devops/tests/test_shipping_smoke.py` asserts the committed
//! files are this stub, and the gate runs that assertion before anything
//! generates a bundle.

compile_error!(
    "this is the project-agnostic shipping bundle stub; run \
     `python devops/tools/generate_shipping_bundle.py <project>` before building the \
     shipping posture"
);
