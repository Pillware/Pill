//! Hardware Ray Tracing module for the Pill renderer.
//!
//! Implements inline ray queries via `wgpu::Features::EXPERIMENTAL_RAY_QUERY`.
//! All `wgpu::Blas`, `wgpu::Tlas`, bind groups, and experimental opt-in code
//! stays inside this module. `pill_engine` sees only backend-neutral
//! capability and frame types.
//!
//! The module is gated behind both the `hardware_ray_tracing` Cargo feature
//! and `not(target_arch = "wasm32")`.

pub mod blas;
pub mod capability;
pub mod instance_table;
pub mod pipeline;
pub mod scene;
pub mod tlas;
pub mod transform;

#[cfg(all(test, feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
mod gpu_tests;
