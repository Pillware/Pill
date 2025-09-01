use cgmath::{Vector3, Point3};

use crate::Vector3f;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub min: Vector3f,
    pub max: Vector3f,
}

impl BoundingBox {
    /// Create a new AABB from min and max corners.
    pub fn new(min: Vector3f, max: Vector3f) -> Self {
        Self { min, max }
    }

    /// Create a degenerate AABB around a single point.
    pub fn from_point(point: Vector3f) -> Self {
        Self { min: point, max: point }
    }

    /// Expand this AABB to include a point.
    pub fn grow_to_include(&mut self, point: Vector3f) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.min.z = self.min.z.min(point.z);

        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
        self.max.z = self.max.z.max(point.z);
    }

    /// Merge two AABBs into one that contains both.
    pub fn merged(&self, other: &Self) -> Self {
        Self {
            min: Vector3f::new(
                self.min.x.min(other.min.x),
                self.min.y.min(other.min.y),
                self.min.z.min(other.min.z),
            ),
            max: Vector3f::new(
                self.max.x.max(other.max.x),
                self.max.y.max(other.max.y),
                self.max.z.max(other.max.z),
            ),
        }
    }

    /// Get the center point of the AABB.
    pub fn center(&self) -> Vector3f {
        Vector3f::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
            (self.min.z + self.max.z) * 0.5,
        )
    }

    /// Get the extents (half-size) of the AABB.
    pub fn extents(&self) -> Vector3f {
        Vector3f::new(
            (self.max.x - self.min.x) * 0.5,
            (self.max.y - self.min.y) * 0.5,
            (self.max.z - self.min.z) * 0.5,
        )
    }

    /// Check if a point lies inside (inclusive).
    pub fn contains_point(&self, point: Vector3f) -> bool {
        point.x >= self.min.x && point.x <= self.max.x &&
        point.y >= self.min.y && point.y <= self.max.y &&
        point.z >= self.min.z && point.z <= self.max.z
    }

    /// Check if two AABBs overlap.
    pub fn intersects(&self, other: &Self) -> bool {
        !(self.max.x < other.min.x || self.min.x > other.max.x ||
          self.max.y < other.min.y || self.min.y > other.max.y ||
          self.max.z < other.min.z || self.min.z > other.max.z)
    }
}