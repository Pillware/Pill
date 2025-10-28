// Vectors
pub type Vector3i = glam::IVec3;
pub type Vector2i = glam::IVec2;

pub type Vector3f = glam::Vec3;
pub type Vector2f = glam::Vec2;

pub type Color = glam::Vec3;
pub type Matrix3f = glam::Mat3;

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

impl Vector3fExt for glam::Vec3 {
    const X: Self = Vector3f::new(1.0, 0.0, 0.0);
    const Y: Self = Vector3f::new(0.0, 1.0, 0.0);
    const Z: Self = Vector3f::new(0.0, 0.0, 1.0);
}
