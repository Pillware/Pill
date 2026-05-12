//! Minimal declarative asset pipeline.
//!
//! A `Pipeline` is a list of `Rule`s applied to a `root` directory. Each rule
//! declares an input glob, a per-input output path, and an action that turns
//! one input into one output. The runner globs, mtime-skips, and invokes the
//! action — nothing more.
//!
//! Today this is a single rule (HLSL → WGSL via slangc). The interface admits
//! chained rules (output of one rule is input to another) — when that lands,
//! the runner will topo-sort the rule list. Single-rule case needs no sort.
//!
//! Two callers: `pill_engine/build.rs` (cargo build-script side effect) and
//! `PillLauncher -a assets` (explicit). Both share this single runner.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

pub mod rules;

pub use rules::{default_rules, HlslToWgsl, ObjToRmesh, PngToRtex};

pub trait Rule {
    /// Glob (relative to the pipeline root) selecting source inputs.
    fn input_glob(&self) -> &'static str;

    /// Map an input path to the output path it produces.
    fn output_for(&self, input: &Path) -> PathBuf;

    /// Run the tool that turns `input` → `output`. Called only when stale.
    fn build(&self, input: &Path, output: &Path) -> Result<()>;

    /// Human-readable rule name for logs/errors.
    fn name(&self) -> &'static str;
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
}

impl Pipeline {
    pub fn run(&self) -> Result<Stats> {
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
                let input =
                    entry.with_context(|| format!("glob entry error for rule {}", rule.name()))?;
                stats.discovered.push(input.clone());

                let output = rule.output_for(&input);

                if is_up_to_date(&input, &output) {
                    stats.skipped.push(output);
                    continue;
                }

                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create output dir {parent:?}"))?;
                }

                rule.build(&input, &output).with_context(|| {
                    format!("rule {} failed: {input:?} -> {output:?}", rule.name())
                })?;
                stats.rebuilt.push(output);
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
