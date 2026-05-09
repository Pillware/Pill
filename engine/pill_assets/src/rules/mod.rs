use crate::Rule;

pub mod hlsl_to_wgsl;

pub use hlsl_to_wgsl::HlslToWgsl;

/// Built-in rule set used by both the cargo build-script and the launcher.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(HlslToWgsl)]
}
