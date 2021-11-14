#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod renderer;

pub use renderer::{
    Renderer,
    RendererError,
    PillRenderer,
};