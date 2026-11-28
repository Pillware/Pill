//! Answering a screen point with a world-space ray, and testing it against a
//! mesh.
//!
//! # Responsibilities
//!
//! - Build the ray that leaves the camera through a point on a surface, from
//!   the camera's parameters ([`ray_through_screen`]).
//! - Report where a ray first meets a mesh, placed by an instance's transform
//!   ([`ray_hits_mesh`]).
//!
//! # Design
//!
//! A renderer with a GPU answers a pick by drawing the scene into an id buffer
//! and reading the pixel back. A renderer without one has to do the same work
//! on the processor, which is what this module is for: it needs no device, only
//! the camera matrices and the mesh data the world already holds.
//!
//! The ray is unprojected through the inverse view-projection, so it points
//! exactly where the drawn pixel points, whatever projection the camera uses.
//! The test brings the ray into the mesh's own space instead of transforming
//! every triangle into the world, which keeps it one matrix inverse per
//! instance rather than one transform per vertex.
//!
//! Triangles are tested two-sided: clicking a face from behind it selects it,
//! exactly as clicking it from the front does.

// External crates
use glam::{Vec3, Vec4};
use pill_core::math::Matrix4f;
use pill_rendering::assets::Mesh;
use pill_rendering::components::TransformComponent;

// =============================================================================
// Types
// =============================================================================

/// A ray in world space: where it starts, and the direction it travels in.
///
/// `direction` is unit length, so a hit distance is a distance in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    /// The point the ray starts at.
    pub origin: Vec3,
    /// The direction the ray travels in, normalized.
    pub direction: Vec3,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the ray from the camera through `screen`, a point in surface pixels
/// whose origin is the surface's top-left corner.
///
/// `viewport` is the rectangle the camera draws into; `parameters` are that
/// camera's values for the frame being picked, whose inverse view-projection
/// the unprojection uses.
pub fn ray_through_screen(
    parameters: &crate::camera::CameraParameters,
    screen: (f32, f32),
    viewport: pill_renderer_api::components::RenderViewport,
) -> Option<Ray> {
    // Step 1: The point in clip space. The viewport's own rectangle maps onto
    // the full -1..1 range, and y points up in clip space while it points down
    // in pixels.
    let width = viewport.width as f32;
    let height = viewport.height as f32;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let normalized_x = ((screen.0 - viewport.x as f32) / width) * 2.0 - 1.0;
    let normalized_y = 1.0 - ((screen.1 - viewport.y as f32) / height) * 2.0;

    // Step 2: Two points on the same screen position, one on the near plane
    // and one on the far plane, take the ray's direction.
    let unproject = |depth: f32| {
        let clip = Vec4::new(normalized_x, normalized_y, depth, 1.0);
        let world = parameters.inverse_view_projection_matrix * clip;
        world / world.w
    };
    let near = unproject(0.0);
    let far = unproject(1.0);
    let direction = (far - near).truncate();
    if direction.length_squared() < f32::EPSILON {
        return None;
    }
    Some(Ray {
        origin: near.truncate(),
        direction: direction.normalize(),
    })
}

/// Returns how far along `ray` it first meets `mesh`, placed by `transform`, or
/// `None` when it misses.
///
/// A mesh with no triangles or no vertices is never hit. The distance is in
/// world units along [`Ray::direction`], so a caller picks the closest of
/// several hits by taking the smallest value.
pub fn ray_hits_mesh(ray: &Ray, mesh: &Mesh, transform: &TransformComponent) -> Option<f32> {
    if mesh.indices.len() < 3 || mesh.vertices.is_empty() {
        return None;
    }
    // Step 1: Bring the ray into the mesh's own space. A scale of zero on any
    // axis has no inverse, and nothing of that instance is visible anyway.
    let model = glam::Affine3A::from_scale_rotation_translation(
        Vec3::from_array(transform.scale),
        glam::Quat::from_array(pill_engine::common_components::rotation_or_identity(
            transform.rotation,
        )),
        Vec3::from_array(transform.translation),
    );
    let inverse = model.inverse();
    let origin = inverse.transform_point3(ray.origin);
    // A direction is a vector: it takes the linear part only, and the scaling
    // it picks up changes the unit of the distance, which is why the distance
    // is measured in world space again below.
    let direction = inverse.transform_vector3(ray.direction);
    if direction.length_squared() < f32::EPSILON {
        return None;
    }
    let scale_correction = 1.0 / direction.length();
    let direction = direction.normalize();

    // Step 2: Test every triangle, keeping the closest hit.
    let mut closest: Option<f32> = None;
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let corners = [
            Vec3::from_array(mesh.vertices[triangle[0] as usize].position),
            Vec3::from_array(mesh.vertices[triangle[1] as usize].position),
            Vec3::from_array(mesh.vertices[triangle[2] as usize].position),
        ];
        let Some(distance) = ray_triangle_distance(origin, direction, corners) else {
            continue;
        };
        let distance = distance * scale_correction;
        if closest.is_none_or(|closest| distance < closest) {
            closest = Some(distance);
        }
    }
    closest
}

/// How far along `direction` a triangle is hit, by the Möller-Trumbore test.
///
/// Returns `None` for a miss, for a triangle behind the origin, and for a
/// triangle seen edge-on, whose hit would be a line rather than a surface.
fn ray_triangle_distance(origin: Vec3, direction: Vec3, corners: [Vec3; 3]) -> Option<f32> {
    let edge1 = corners[1] - corners[0];
    let edge2 = corners[2] - corners[0];
    let across = direction.cross(edge2);
    let determinant = edge1.dot(across);
    // The determinant is the triangle's area projected onto the ray's
    // direction: near zero means the ray runs along the triangle's plane.
    if determinant.abs() < 1e-8 {
        return None;
    }
    let inverse_determinant = 1.0 / determinant;
    let from_corner = origin - corners[0];
    let u = from_corner.dot(across) * inverse_determinant;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let along = from_corner.cross(edge1);
    let v = direction.dot(along) * inverse_determinant;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = edge2.dot(along) * inverse_determinant;
    (distance > 0.0).then_some(distance)
}

/// The model matrix a transform places a mesh with.
///
/// [`ray_hits_mesh`] needs the inverse of this matrix; a caller that keeps it
/// for a whole scene can transform once and test many rays.
pub fn model_matrix(transform: &TransformComponent) -> Matrix4f {
    Matrix4f::from(glam::Affine3A::from_scale_rotation_translation(
        Vec3::from_array(transform.scale),
        glam::Quat::from_array(pill_engine::common_components::rotation_or_identity(
            transform.rotation,
        )),
        Vec3::from_array(transform.translation),
    ))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_renderer_api::components::RenderViewport;

    /// A 2 by 2 square in the XY plane, facing `+Z`.
    fn square() -> Mesh {
        let vertex = |position| pill_rendering::assets::MeshVertex {
            position,
            texture_coordinates: [0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        };
        Mesh::from_data(
            "square",
            vec![
                vertex([-1.0, -1.0, 0.0]),
                vertex([1.0, -1.0, 0.0]),
                vertex([1.0, 1.0, 0.0]),
                vertex([-1.0, 1.0, 0.0]),
            ],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    fn identity() -> TransformComponent {
        TransformComponent::default()
    }

    #[test]
    fn a_ray_through_the_middle_of_the_square_hits_it() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };

        let distance = ray_hits_mesh(&ray, &square(), &identity()).expect("the ray hits");

        assert!((distance - 5.0).abs() < 1e-4, "{distance}");
    }

    #[test]
    fn a_ray_beside_the_square_misses_it() {
        let ray = Ray {
            origin: Vec3::new(4.0, 0.0, 5.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };

        assert_eq!(ray_hits_mesh(&ray, &square(), &identity()), None);
    }

    /// A triangle seen exactly edge-on has no surface to hit.
    #[test]
    fn a_ray_along_the_squares_plane_misses_it() {
        let ray = Ray {
            origin: Vec3::new(0.0, -5.0, 0.0),
            direction: Vec3::new(0.0, 1.0, 0.0),
        };

        assert_eq!(ray_hits_mesh(&ray, &square(), &identity()), None);
    }

    /// The transform moves the mesh out of the way: the same ray that hit the
    /// mesh at the origin now misses it.
    #[test]
    fn the_transform_places_the_mesh_the_ray_is_tested_against() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        let moved_aside = TransformComponent {
            translation: [10.0, 0.0, 0.0],
            ..identity()
        };

        assert_eq!(ray_hits_mesh(&ray, &square(), &moved_aside), None);

        let moved_closer = TransformComponent {
            translation: [0.0, 0.0, 3.0],
            ..identity()
        };
        let distance =
            ray_hits_mesh(&ray, &square(), &moved_closer).expect("the ray hits the moved mesh");
        assert!((distance - 2.0).abs() < 1e-4, "{distance}");
    }

    /// A scaled instance is hit where it is drawn, and the distance is in world
    /// units even though the ray was tested in the mesh's own space.
    #[test]
    fn a_scaled_instance_is_hit_where_it_is_drawn() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 5.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        let wide = TransformComponent {
            scale: [4.0, 4.0, 1.0],
            ..identity()
        };

        let distance = ray_hits_mesh(&ray, &square(), &wide).expect("the ray hits the wide square");
        assert!((distance - 5.0).abs() < 1e-4, "{distance}");

        // A point outside the unscaled square but inside the wide one.
        let beside = Ray {
            origin: Vec3::new(3.0, 0.0, 5.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        assert!(ray_hits_mesh(&beside, &square(), &identity()).is_none());
        assert!(ray_hits_mesh(&beside, &square(), &wide).is_some());
    }

    /// A click in the middle of the viewport sends the ray down the camera's
    /// forward axis.
    #[test]
    fn the_ray_through_the_middle_of_the_viewport_points_ahead() {
        let mut parameters = crate::camera::CameraParameters::new();
        let camera = pill_rendering::components::CameraComponent::default();
        let transform = TransformComponent {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        parameters.update(&camera, &transform, 1.0);
        let viewport = RenderViewport::new(0, 0, 100, 100);

        let ray = ray_through_screen(&parameters, (50.0, 50.0), viewport).expect("a ray exists");

        assert!(ray.direction.z < -0.99, "{:?}", ray.direction);
        assert!(ray.origin.distance(Vec3::ZERO) < 0.2, "{:?}", ray.origin);
    }

    /// A click on the right half sends the ray to the camera's right.
    #[test]
    fn a_click_off_centre_turns_the_ray_towards_that_side() {
        let mut parameters = crate::camera::CameraParameters::new();
        let transform = TransformComponent {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        parameters.update(
            &pill_rendering::components::CameraComponent::default(),
            &transform,
            1.0,
        );
        let viewport = RenderViewport::new(0, 0, 100, 100);

        let right = ray_through_screen(&parameters, (90.0, 50.0), viewport).expect("a ray exists");
        let up = ray_through_screen(&parameters, (50.0, 10.0), viewport).expect("a ray exists");

        assert!(right.direction.x > 0.1, "{:?}", right.direction);
        assert!(up.direction.y > 0.1, "{:?}", up.direction);
    }

    /// A viewport with no area has no ray to give.
    #[test]
    fn a_viewport_with_no_area_has_no_ray() {
        let parameters = crate::camera::CameraParameters::new();
        let viewport = RenderViewport::new(0, 0, 0, 0);

        assert!(ray_through_screen(&parameters, (0.0, 0.0), viewport).is_none());
    }
}
