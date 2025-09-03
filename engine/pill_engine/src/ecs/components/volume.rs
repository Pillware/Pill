use pill_core::{ BoundingBox, Vector3f };

#[inline]
fn saturate(value: f32) -> f32 { value.max(0.0).min(1.0) }

pub trait Volume3D {
    fn set_is_global(&mut self, is_global: bool);

    fn is_global(&self) -> bool;

    fn get_bounding_box(&self) -> Option<BoundingBox>;

    fn get_falloff(&self) -> f32 { 0.0 }

    /// Checks if a point is inside the volume.
    fn contains_point(&self, point: Vector3f) -> bool {
        if self.is_global() {
            return true;
        }

        let min_bound = self.get_bounding_box().unwrap().min;
        let max_bound = self.get_bounding_box().unwrap().max;
        (point.x >= min_bound.x && point.x <= max_bound.x) &&
        (point.y >= min_bound.y && point.y <= max_bound.y) &&
        (point.z >= min_bound.z && point.z <= max_bound.z)
    }

    /// Soft containment (inward falloff). 
    /// `falloff` is the thickness (in world units) from the AABB surface toward its interior
    /// at which the weight ramps from 0 (on surface) to 1 (>= falloff deep).
    fn contains_point_falloffed(&self, point: Vector3f) -> f32 {
        if self.is_global() {
            return 1.0;
        }

        let min_bound = self.get_bounding_box().unwrap().min;
        let max_bound = self.get_bounding_box().unwrap().max;

        // Outside? weight = 0
        if point.x < min_bound.x || point.x > max_bound.x ||
           point.y < min_bound.y || point.y > max_bound.y ||
           point.z < min_bound.z || point.z > max_bound.z {
            return 0.0;
        }

        // Inward distance to nearest face (>= 0 if inside)
        let distance_x = (point.x - min_bound.x).min(max_bound.x - point.x);
        let distance_y = (point.y - min_bound.y).min(max_bound.y - point.y);
        let distance_z = (point.z - min_bound.z).min(max_bound.z - point.z);
        let inward_distance = distance_x.min(distance_y.min(distance_z));

        if self.get_falloff() <= 0.0 {
            // No falloff region: anything inside is fully "1"
            return 1.0;
        }

        // Linear ramp (0 at surface → 1 after falloff distance)
        let weight = saturate(inward_distance / self.get_falloff());

        weight * weight * (3.0 - 2.0 * weight)
    }
}
