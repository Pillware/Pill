use glam::{IVec2, Mat3, Mat3A, Mat4, Vec2, Vec3, Vec4};

pub type Vector2i = IVec2;

pub type Vector2f = Vec2;
pub type Vector3f = Vec3;
pub type Vector4f = Vec4;

pub type Color = Vec3;

pub type Matrix3f = Mat3;
pub type Matrix3fA = Mat3A;

pub type Matrix4f = Mat4;

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
