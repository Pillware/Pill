use glam::{IVec2, Vec2, Vec3, Mat3};

pub type Vector2i = IVec;

pub type Vector3f = Vec3;
pub type Vector2f = Vec2;

pub type Color = Vec3;
pub type Matrix3f = Mat3;

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
