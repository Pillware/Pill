//! Asset conversion + declarative build pipeline.
//!
//! Two layers:
//!  - Pure conversion/bake ([`convert`], [`ibl`]) — no filesystem, wasm-safe. Shared by the
//!    build-time cooker and by games loading assets at runtime.
//!  - A build-time `Pipeline` of `Rule`s (feature `build`, the default) applied to a `root`
//!    directory: each rule declares an input glob, a per-input output path, and an action that
//!    turns one input into one output. The runner globs, mtime-skips, and invokes the action.
//!    Callers: `pill_engine/build.rs` and `PillLauncher -a assets`.
//!
//! Games depend on this crate with `default-features = false` so the filesystem pipeline and its
//! deps are never compiled into a wasm binary.

// Pure conversion/bake — shared by the build cooker (this crate's `Rule`s) and the engine's
// runtime glTF loader. No filesystem, no glTF dep; wasm-safe so `pill_engine`'s `gltf_loading`
// feature can compile these into a game binary.
pub mod convert;
pub mod ibl;
// Asset pack (PPAK): reader + decoders are wasm-safe; the writer needs feature `encode`.
pub mod pak_codec;
pub mod ppak;

// --- Build-time filesystem pipeline (feature `build`, the default) ---

#[cfg(feature = "build")]
mod pipeline {
    use std::collections::{HashMap, HashSet};
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    use anyhow::{Context, Result};

    use crate::rules::default_rules;

    /// Per-root sidecar of each output's last cook time, so the DAG report shows times for
    /// outputs that were up to date this run (and across the separate engine/game cooks).
    /// Build tooling only: never packed or shipped.
    pub const COOK_TIMES_FILE: &str = ".pill_assets_cache";

    pub trait Rule {
        /// Glob (relative to the pipeline root) selecting source inputs.
        fn input_glob(&self) -> &'static str;

        /// Map an input path to the output path it produces.
        fn output_for(&self, input: &Path) -> PathBuf;

        /// Run the tool that turns `input` → `output`. Called only when stale.
        fn build(&self, input: &Path, output: &Path) -> Result<()>;

        /// Human-readable rule name for logs/errors.
        fn name(&self) -> &'static str;

        /// Side outputs `build` writes next to `output_for`; recorded in `Stats::edges`, which
        /// is how they reach the DAG report and the asset pack.
        fn extra_outputs(&self, _input: &Path) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    pub struct Pipeline {
        pub root: PathBuf,
        pub rules: Vec<Box<dyn Rule>>,
    }

    #[derive(Default, Debug)]
    pub struct Stats {
        /// Every input matched by any rule. Build scripts emit cargo:rerun-if-changed for these.
        pub discovered: Vec<PathBuf>,
        /// Outputs we actually wrote this run.
        pub rebuilt: Vec<PathBuf>,
        /// Outputs that were already up-to-date (skipped).
        pub skipped: Vec<PathBuf>,
        /// (input, output, rule name) per produced file of every discovered input.
        pub edges: Vec<(PathBuf, PathBuf, &'static str)>,
        /// How long each output rebuilt this run took to cook.
        pub cook_times: Vec<(PathBuf, Duration)>,
    }

    impl Pipeline {
        /// A pipeline over `root` using the built-in rule set.
        pub fn new(root: PathBuf) -> Self {
            Self {
                root,
                rules: default_rules(),
            }
        }

        pub fn run(&self) -> Result<Stats> {
            let stats = self.walk(true)?;
            // Best-effort: only the DAG report reads it.
            let _ = persist_cook_times(&self.root, &stats);
            Ok(stats)
        }

        /// `discovered` + `edges` without building or writing anything.
        pub fn plan(&self) -> Result<Stats> {
            self.walk(false)
        }

        fn walk(&self, execute: bool) -> Result<Stats> {
            let mut stats = Stats::default();

            for rule in &self.rules {
                let pattern = self.root.join(rule.input_glob());
                let pattern_str = pattern
                    .to_str()
                    .with_context(|| format!("non-UTF8 path in pipeline root: {pattern:?}"))?;

                let matches = glob::glob(pattern_str).with_context(|| {
                    format!("invalid glob {pattern_str:?} for rule {}", rule.name())
                })?;

                for entry in matches {
                    let input = entry
                        .with_context(|| format!("glob entry error for rule {}", rule.name()))?;
                    stats.discovered.push(input.clone());

                    let output = rule.output_for(&input);
                    stats
                        .edges
                        .push((input.clone(), output.clone(), rule.name()));
                    // After the build: some rules list only the side outputs present on disk.
                    let push_extra_edges = |stats: &mut Stats| {
                        for extra_output in rule.extra_outputs(&input) {
                            stats.edges.push((input.clone(), extra_output, rule.name()));
                        }
                    };

                    if !execute || is_up_to_date(&input, &output) {
                        push_extra_edges(&mut stats);
                        if execute {
                            stats.skipped.push(output);
                        }
                        continue;
                    }

                    if let Some(parent) = output.parent() {
                        std::fs::create_dir_all(parent)
                            .with_context(|| format!("create output dir {parent:?}"))?;
                    }

                    let started = Instant::now();
                    rule.build(&input, &output).with_context(|| {
                        format!("rule {} failed: {input:?} -> {output:?}", rule.name())
                    })?;
                    stats.cook_times.push((output.clone(), started.elapsed()));
                    stats.rebuilt.push(output);
                    push_extra_edges(&mut stats);
                }
            }

            Ok(stats)
        }
    }

    fn is_up_to_date(input: &Path, output: &Path) -> bool {
        let in_mtime = mtime(input);
        let out_mtime = mtime(output);
        match (in_mtime, out_mtime) {
            (Some(i), Some(o)) => o >= i,
            _ => false,
        }
    }

    fn mtime(p: &Path) -> Option<SystemTime> {
        p.metadata().ok()?.modified().ok()
    }

    /// Merge this run's cook times into `<root>/.pill_assets_cache`, keeping last-known times
    /// of up-to-date outputs and pruning outputs no longer produced.
    /// Lines are `<millis>\t<output path relative to root>`.
    fn persist_cook_times(root: &Path, stats: &Stats) -> Result<()> {
        let mut times = read_cook_times(root);
        for (output, duration) in &stats.cook_times {
            times.insert(output.clone(), *duration);
        }
        let current_outputs: HashSet<&PathBuf> =
            stats.edges.iter().map(|(_, output, _)| output).collect();
        times.retain(|output, _| current_outputs.contains(output));

        let mut entries: Vec<(&PathBuf, &Duration)> = times.iter().collect();
        entries.sort_by_key(|(path, _)| *path);
        let mut serialized = String::new();
        for (output, duration) in entries {
            let relative = output.strip_prefix(root).unwrap_or(output);
            let _ = writeln!(
                serialized,
                "{}\t{}",
                duration.as_millis(),
                relative.display()
            );
        }
        crate::util::write_atomic(&root.join(COOK_TIMES_FILE), serialized.as_bytes())
    }

    /// Last cook time per absolute output path under `root`; empty when there is no cache.
    pub fn read_cook_times(root: &Path) -> HashMap<PathBuf, Duration> {
        let mut times = HashMap::new();
        let Ok(text) = std::fs::read_to_string(root.join(COOK_TIMES_FILE)) else {
            return times;
        };
        for line in text.lines() {
            let Some((millis, relative)) = line.split_once('\t') else {
                continue;
            };
            if let Ok(millis) = millis.parse::<u64>() {
                times.insert(root.join(relative), Duration::from_millis(millis));
            }
        }
        times
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Writes `<stem>.out` plus a `<stem>.side` sidecar; lists the sidecar only once it exists.
        struct WithSidecar;

        impl Rule for WithSidecar {
            fn input_glob(&self) -> &'static str {
                "*.src"
            }

            fn output_for(&self, input: &Path) -> PathBuf {
                input.with_extension("out")
            }

            fn build(&self, input: &Path, output: &Path) -> Result<()> {
                std::fs::write(input.with_extension("side"), b"side")?;
                std::fs::write(output, b"out")?;
                Ok(())
            }

            fn name(&self) -> &'static str {
                "with_sidecar"
            }

            fn extra_outputs(&self, input: &Path) -> Vec<PathBuf> {
                let side = input.with_extension("side");
                side.exists().then_some(side).into_iter().collect()
            }
        }

        fn outputs(stats: &Stats) -> Vec<PathBuf> {
            stats
                .edges
                .iter()
                .map(|(_, output, _)| output.clone())
                .collect()
        }

        #[test]
        fn fresh_cook_records_side_outputs_written_by_build() {
            let root =
                std::env::temp_dir().join(format!("pill_pipeline_side_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("a.src"), b"src").unwrap();
            let pipeline = Pipeline {
                root: root.clone(),
                rules: vec![Box::new(WithSidecar)],
            };
            let expected = vec![root.join("a.out"), root.join("a.side")];

            let cooked = pipeline.run().unwrap();
            assert_eq!(cooked.rebuilt, vec![root.join("a.out")]);
            assert_eq!(outputs(&cooked), expected);
            assert_eq!(outputs(&pipeline.plan().unwrap()), expected);
            assert_eq!(outputs(&pipeline.run().unwrap()), expected);
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

#[cfg(feature = "build")]
pub mod dag;
#[cfg(feature = "build")]
pub mod rules;
#[cfg(feature = "build")]
pub mod usage;
mod util;

#[cfg(feature = "build")]
pub use dag::{asset_type, write_dag_svg, DagArtifacts, DagSection};
#[cfg(feature = "build")]
pub use pipeline::{read_cook_times, Pipeline, Rule, Stats, COOK_TIMES_FILE};
#[cfg(feature = "build")]
pub use ppak::pack_list;
pub use ppak::Ppak;
#[cfg(feature = "encode")]
pub use ppak::{pack_sources, write_pack_for_build};

#[cfg(feature = "build")]
pub use rules::{
    default_rules, BrdfLut, EquirectToIBL, GlbToCookedMesh, HdrToEquirect, HlslToWgsl,
    ObjToCookedMesh, PngToCookedTex, ProceduralEquirect,
};
#[cfg(feature = "build")]
pub use usage::{scan_usage, AssetRef, Scope, Usage, UsageScan};
