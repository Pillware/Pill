// Vectors
pub type Vector3i = cgmath::Vector3<i32>;
pub type Vector2i = cgmath::Vector2<i32>;

pub type Vector3f = cgmath::Vector3<f32>;
pub type Vector2f = cgmath::Vector2<f32>;

pub type Color = cgmath::Vector3<f32>;
pub type Matrix3f = cgmath::Matrix3<f32>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
    Right,
    Left,
    Up,
    Down,
    WorldForward,
    WorldBackward,
    WorldRight,
    WorldLeft,
    WorldUp,
    WorldDown,
}

pub trait Vector3fExt {
    const X: Self;
    const Y: Self;
    const Z: Self;
}

impl Vector3fExt for cgmath::Vector3<f32> {
    const X: Self = Vector3f::new(1.0, 0.0, 0.0);
    const Y: Self = Vector3f::new(0.0, 1.0, 0.0);
    const Z: Self = Vector3f::new(0.0, 0.0, 1.0);
}