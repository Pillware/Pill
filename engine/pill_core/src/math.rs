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
