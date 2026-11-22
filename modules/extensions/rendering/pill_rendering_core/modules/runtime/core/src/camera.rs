//! Turning a camera and a transform into the matrices a shader reads.
//!
//! # Responsibilities
//!
//! - Pack a camera's position, view-projection matrix and inverse
//!   view-projection matrix into the layout the shaders' `CameraParams`
//!   constant buffer expects ([`CameraParameters`]).
//! - Replace each projection input that no perspective matrix can use, and
//!   name it in the report: a field of view outside `(0, 180)`, a near plane
//!   that is not positive, a far plane at or inside the near one ([`sanitize`]).
//!   Without this, a matrix full of NaN values draws a blank frame with nothing
//!   to trace it to.
//!
//! # Design
//!
//! The matrices are rebuilt from the camera and transform components on every
//! update instead of being tracked, so a moved entity is always followed. The
//! same pass replaces the projection inputs that cannot be used and reports
//! what it replaced; a renderer logs that once per change rather than once per
//! frame.
//!
//! The struct is `#[repr(C)]` because its bytes go to a shader unchanged. The
//! HLSL side is `CameraParams` in the renderer's
//! `res/shaders/common_shaders/include/common.hlsl`: a field changed here has
//! to be changed there in the same pass, or every value after it is read from
//! the wrong place.

// External crates
use pill_core::math::{Matrix4f, Vector3f, Vector4f};
use pill_rendering::components::{CameraComponent, TransformComponent};

// =============================================================================
// Camera Parameters
// =============================================================================

/// The camera values a shader reads, in the layout of the `CameraParams`
/// constant buffer.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraParameters {
    /// Camera position in world space.
    ///
    /// The shader declares three components, and the fourth is kept at zero:
    /// it is the padding that puts the matrix after it at the offset both
    /// sides expect.
    pub position: Vector4f,
    /// The view and projection matrices combined, applied to world-space
    /// positions by the vertex shader.
    pub view_projection_matrix: Matrix4f,
    /// The inverse of [`Self::view_projection_matrix`]. It takes a clip-space
    /// point back to world space, which is how a skybox turns a pixel into the
    /// direction the camera looks along.
    pub inverse_view_projection_matrix: Matrix4f,
}

impl Default for CameraParameters {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraParameters {
    /// Creates the empty value: the camera at the origin, with identity
    /// matrices, which the first update replaces.
    ///
    /// `Default` calls this, so both ways of building the parameters agree on
    /// what "empty" means.
    pub fn new() -> Self {
        Self {
            position: Vector4f::ZERO,
            view_projection_matrix: Matrix4f::IDENTITY,
            inverse_view_projection_matrix: Matrix4f::IDENTITY,
        }
    }

    /// Rebuilds the uniform values from the camera and transform components.
    ///
    /// `aspect` is the drawing viewport's width divided by its height. It is
    /// passed in rather than read here, so the projection follows the viewport
    /// a renderer actually draws into. Returns a description of every
    /// projection input that had to be replaced, or `None` when all of the
    /// component's values were usable.
    pub fn update(
        &mut self,
        camera_component: &CameraComponent,
        transform_component: &TransformComponent,
        aspect: f32,
    ) -> Option<String> {
        // Step 1: Copy the camera's world position out of the transform.
        self.position = Vector4f::new(
            transform_component.translation[0],
            transform_component.translation[1],
            transform_component.translation[2],
            0.0,
        );

        // Step 2: Build both matrices, replacing any projection input the
        // matrix cannot use. An impossible field of view or near/far pair
        // would otherwise become an infinity or a NaN in the matrix, which
        // draws nothing and leaves no error to look at.
        let (vertical_fov, near, far, substitution_report) = sanitize(camera_component);
        self.view_projection_matrix =
            projection_matrix(vertical_fov, near, far, aspect) * view_matrix(transform_component);

        // Step 3: Invert the combined matrix. A skybox reads the inverse to
        // turn a pixel into the direction the camera looks along.
        self.inverse_view_projection_matrix = self.view_projection_matrix.inverse();
        substitution_report
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the view matrix: it looks from the transform's position along the
/// transform's forward axis.
///
/// The rotation decides where forward points. A degenerate rotation would
/// spread NaN values through every basis vector of the matrix, so it is
/// replaced before this function uses it.
pub fn view_matrix(transform_component: &TransformComponent) -> Matrix4f {
    let position = Vector3f::from_array(transform_component.translation);
    // The shared helper normalizes the quaternion, and reads a value that is
    // not finite or is too close to zero as the identity. That keeps NaN
    // values out of the view direction.
    let rotation = glam::Quat::from_array(pill_engine::common_components::rotation_or_identity(
        transform_component.rotation,
    ));
    glam::camera::rh::view::look_to_mat4(
        position,
        rotation * Vector3f::NEG_Z,
        rotation * Vector3f::Y,
    )
}

/// Builds the perspective matrix for wgpu's 0-to-1 depth range.
///
/// The DirectX convention uses that range, which is why glam is asked for it
/// rather than for the OpenGL one. `vertical_fov` is in degrees, as the
/// component stores it. The inputs are expected to come from [`sanitize`], so
/// `perspective` never divides by a zero angle or a non-positive plane
/// distance.
pub fn projection_matrix(vertical_fov: f32, near: f32, far: f32, aspect: f32) -> Matrix4f {
    glam::camera::rh::proj::directx::perspective(vertical_fov.to_radians(), aspect, near, far)
}

/// Cleans up the camera's projection inputs and names every value it replaced.
///
/// `perspective` divides by `tan(fov / 2)` and by `far - near`, so a field of
/// view of zero, a near plane that is not positive, or a far plane at or
/// inside the near one produces infinities and NaN values: a frame that draws
/// nothing, with no error to show for it.
///
/// An unusable field of view or near plane is replaced with the component's
/// default. An unusable far plane is replaced with the near plane plus the
/// default far distance, because a world may legitimately place its near plane
/// past the default far plane. The returned text names each replacement, so
/// the caller can report what it did.
pub fn sanitize(camera_component: &CameraComponent) -> (f32, f32, f32, Option<String>) {
    let mut reasons = Vec::new();
    let default_camera_component = CameraComponent::default();

    // Step 1: The field of view has to be a finite angle inside (0, 180). A
    // zero angle would make the matrix divide by zero.
    let vertical_fov = if camera_component.vertical_fov.is_finite()
        && camera_component.vertical_fov > 0.0
        && camera_component.vertical_fov < 180.0
    {
        camera_component.vertical_fov
    } else {
        reasons.push(format!(
            "vertical_fov {} is outside (0, 180)",
            camera_component.vertical_fov
        ));
        default_camera_component.vertical_fov
    };

    // Step 2: The near plane has to be a finite positive distance, because the
    // matrix is built around it.
    let near = if camera_component.near.is_finite() && camera_component.near > 0.0 {
        camera_component.near
    } else {
        reasons.push(format!("near {} is not positive", camera_component.near));
        default_camera_component.near
    };

    // Step 3: The far plane has to be finite and beyond the near plane, which
    // may itself have just been replaced.
    let far = if camera_component.far.is_finite() && camera_component.far > near {
        camera_component.far
    } else {
        reasons.push(format!(
            "far {} is not beyond near {near}",
            camera_component.far
        ));
        near + default_camera_component.far
    };

    (
        vertical_fov,
        near,
        far,
        (!reasons.is_empty()).then(|| reasons.join("; ")),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_projection_values_come_back_unchanged() {
        let camera = CameraComponent {
            vertical_fov: 45.0,
            near: 0.5,
            far: 250.0,
            ..CameraComponent::default()
        };

        let (vertical_fov, near, far, report) = sanitize(&camera);

        assert_eq!((vertical_fov, near, far), (45.0, 0.5, 250.0));
        assert!(report.is_none());
    }

    #[test]
    fn impossible_projection_values_are_replaced_and_named() {
        let camera = CameraComponent {
            vertical_fov: 0.0,
            near: -1.0,
            far: f32::NAN,
            ..CameraComponent::default()
        };

        let (vertical_fov, near, far, report) = sanitize(&camera);

        assert_eq!(vertical_fov, 60.0);
        assert_eq!(near, 0.1);
        assert!(far > near && far.is_finite());
        let report = report.expect("every replaced value is named");
        assert!(report.contains("vertical_fov"));
        assert!(report.contains("near"));
        assert!(report.contains("far"));
    }

    #[test]
    fn a_far_plane_inside_the_near_plane_is_pushed_past_it() {
        let camera = CameraComponent {
            near: 10.0,
            far: 5.0,
            ..CameraComponent::default()
        };

        let (_, near, far, report) = sanitize(&camera);

        assert_eq!(near, 10.0);
        assert!(far > near);
        assert!(report.is_some_and(|reason| reason.contains("far")));
    }

    /// The camera looks down the transform's forward axis from its position,
    /// and the combined matrix carries that view: a point straight ahead lands
    /// in the middle of the view and in front of the camera.
    #[test]
    fn the_view_projection_takes_a_point_ahead_to_the_middle_of_the_view() {
        let mut parameters = CameraParameters::new();
        let camera = CameraComponent::default();
        let transform = TransformComponent {
            translation: [0.0, 0.0, 0.0],
            // Default forward is `-Z`.
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };

        assert!(
            parameters.update(&camera, &transform, 1.0).is_none(),
            "the default camera's values are usable"
        );

        let ahead = parameters.view_projection_matrix * glam::Vec4::new(0.0, 0.0, -10.0, 1.0);
        assert!(ahead.w > 0.0, "the point is in front of the camera");
        assert!(
            (ahead.x / ahead.w).abs() < 1e-5 && (ahead.y / ahead.w).abs() < 1e-5,
            "and in the middle of the view: {ahead:?}"
        );
    }

    /// Moving the camera moves what it sees, and the stored position follows
    /// the transform.
    #[test]
    fn the_camera_position_follows_the_transform() {
        let mut parameters = CameraParameters::new();
        let transform = TransformComponent {
            translation: [3.0, -2.0, 7.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };

        parameters.update(&CameraComponent::default(), &transform, 1.5);

        assert_eq!(parameters.position, Vector4f::new(3.0, -2.0, 7.0, 0.0));
    }

    /// The inverse matrix takes a clip-space point back to where it came from,
    /// which is what a skybox reads it for.
    #[test]
    fn the_inverse_matrix_undoes_the_view_projection() {
        let mut parameters = CameraParameters::new();
        let transform = TransformComponent {
            translation: [1.0, 2.0, 3.0],
            rotation: glam::Quat::from_rotation_y(0.8).to_array(),
            scale: [1.0; 3],
        };
        parameters.update(&CameraComponent::default(), &transform, 16.0 / 9.0);

        let world = glam::Vec4::new(-4.0, 1.5, 6.0, 1.0);
        let clip = parameters.view_projection_matrix * world;
        let restored = parameters.inverse_view_projection_matrix * clip;

        assert!(
            (restored / restored.w - world).abs().max_element() < 1e-4,
            "{restored:?}"
        );
    }
}
