use glam::{IVector2f, Vector2f, Vec3, Mat3};

pub type Vector2i = IVector2f;

pub type Vector3f = Vec3;
pub type Vector2f = Vector2f;

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
