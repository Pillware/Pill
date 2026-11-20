//! The camera values the shaders read, and the GPU resources that carry them.
//!
//! # Responsibilities
//!
//! - Pack the camera's position, view-projection matrix and inverse
//!   view-projection matrix into the layout the shaders' `CameraParams`
//!   constant buffer expects ([`CameraParametersData`]).
//! - Replace each projection input that no perspective matrix can use, and
//!   name it in the report: a field of view outside `(0, 180)`, a near plane
//!   that is not positive, a far plane at or inside the near one. Without
//!   this, a matrix full of NaN values draws a blank frame with nothing to
//!   trace it to.
//! - Own the uniform buffer and the bind group that every shader reading the
//!   camera binds ([`RendererCamera`]).
//!
//! # Design
//!
//! The value and the GPU objects are separate: [`CameraParametersData`] is the
//! `#[repr(C)]` layout the shaders read, and [`RendererCamera`] is the buffer
//! and the bind group that hold it. The matrices are rebuilt from the camera
//! and transform components on every update instead of being tracked, so a
//! moved entity is always followed. The same pass replaces the projection
//! inputs that cannot be used and reports what it replaced. A replacement is
//! reported once per change rather than once per frame, so a value the game
//! never fixes does not fill the log.

// External crates
use pill_core::math::{Matrix4f, Vector3f, Vector4f};
use pill_core::telemetry::telemetry_target;
use wgpu::util::DeviceExt;

// Current crate
use crate::components::{CameraComponent, TransformComponent};

// =============================================================================
// RendererCameraHandle
// =============================================================================

pill_core::define_slot_key!(RendererCameraHandle);

// =============================================================================
// CameraParametersData
// =============================================================================

/// The camera values the shaders read, in the layout of the HLSL
/// `CameraParams` constant buffer.
///
/// The struct here and the HLSL structure are one layout written twice. The
/// HLSL side is the `pill_master_renderer_data` crate's
/// `shader_sources/common_shaders/include/common.hlsl`. A field changed here
/// has to be changed there in the same pass, or every value after it is read
/// from the wrong place.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraParametersData {
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

impl Default for CameraParametersData {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraParametersData {
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
    /// the renderer actually draws into. Returns a description of every
    /// projection input that had to be replaced, or `None` when all of the
    /// component's values were usable.
    pub fn update_data(
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
        let (vertical_fov, near, far, substitution_report) = sanitize_projection(camera_component);
        self.view_projection_matrix =
            CameraParametersData::calculate_projection_matrix(vertical_fov, near, far, aspect)
                * CameraParametersData::calculate_view_matrix(transform_component);

        // Step 3: Invert the combined matrix. A skybox reads the inverse to
        // turn a pixel into the direction the camera looks along.
        self.inverse_view_projection_matrix = self.view_projection_matrix.inverse();
        substitution_report
    }

    /// Builds the view matrix: it looks from the transform's position along
    /// the transform's forward axis.
    ///
    /// The rotation decides where forward points. A degenerate rotation would
    /// spread NaN values through every basis vector of the matrix, so it is
    /// replaced before this function uses it.
    fn calculate_view_matrix(transform_component: &TransformComponent) -> Matrix4f {
        let position = Vector3f::from_array(transform_component.translation);
        // The shared helper normalizes the quaternion, and reads a value that
        // is not finite or is too close to zero as the identity. That keeps
        // NaN values out of the view direction.
        let rotation = glam::Quat::from_array(
            pill_engine::common_components::rotation_or_identity(transform_component.rotation),
        );
        glam::camera::rh::view::look_to_mat4(
            position,
            rotation * Vector3f::NEG_Z,
            rotation * Vector3f::Y,
        )
    }

    /// Builds the perspective matrix for wgpu's 0-to-1 depth range.
    ///
    /// The DirectX convention uses that range, which is why glam is asked for
    /// it rather than for the OpenGL one. `vertical_fov` is in degrees, as the
    /// component stores it. The inputs arrive already cleaned up, so
    /// `perspective` never divides by a zero angle or a non-positive plane
    /// distance.
    fn calculate_projection_matrix(
        vertical_fov: f32,
        near: f32,
        far: f32,
        aspect: f32,
    ) -> Matrix4f {
        glam::camera::rh::proj::directx::perspective(vertical_fov.to_radians(), aspect, near, far)
    }
}

// =============================================================================
// RendererCamera
// =============================================================================

/// The GPU-side resources of one camera: its uniform values, the buffer those
/// values are written to, and the bind group that exposes the buffer to
/// shaders.
///
/// It is built once and then only updated in place. [`RendererCamera::update`]
/// rewrites the buffer's contents on every frame, so moving the camera costs
/// one buffer write and no bind group rebuild.
#[derive(Debug)]
pub struct RendererCamera {
    /// The values for the current frame. [`RendererCamera::update`] rebuilds
    /// them before it writes them to the buffer.
    pub parameters_data: CameraParametersData,
    /// The uniform buffer that holds `parameters_data`.
    pub parameters_uniform_buffer: wgpu::Buffer,
    /// The layout the bind group was built from, kept beside it.
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// The bind group every shader that reads the camera binds: at group 1 in
    /// the chain's pipelines and the mesh drawer, at group 0 in the overlay's.
    pub bind_group: wgpu::BindGroup,
    /// Whether the last update already found unusable projection values and
    /// warned about them. The flag keeps that warning to once per change
    /// instead of once per frame, and clears when the values become usable.
    projection_reported: bool,
}

impl RendererCamera {
    /// Creates a camera on `device`, with its bind group built from
    /// `camera_bind_group_layout`.
    ///
    /// `camera_bind_group_layout` is the layout the renderer creates at
    /// startup and hands to the camera and to every shader that reads the
    /// camera. Sharing it is what makes the bind group compatible with those
    /// pipelines.
    pub fn new(device: &wgpu::Device, camera_bind_group_layout: wgpu::BindGroupLayout) -> Self {
        let parameters_data = CameraParametersData::new();

        let parameters_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("camera_parameters_buffer"),
                contents: bytemuck::cast_slice(&[parameters_data]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0, // (set = 1, binding = 0)
                resource: parameters_uniform_buffer.as_entire_binding(),
            }],
            label: Some("camera_parameters_bind_group"),
        });

        Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout: camera_bind_group_layout,
            bind_group,
            projection_reported: false,
        }
    }

    /// Uploads the camera's values for this frame to the uniform buffer.
    ///
    /// Called once for each frame the surface draws, with the drawing
    /// viewport's aspect ratio. An unusable projection value is reported when
    /// it first appears, not on every frame it persists, so a broken camera
    /// produces one warning instead of a stream of them.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        camera_component: &CameraComponent,
        transform_component: &TransformComponent,
        aspect: f32,
    ) {
        // Step 1: Rebuild the values for this frame.
        let substitution_report =
            self.parameters_data
                .update_data(camera_component, transform_component, aspect);
        // Step 2: Warn once for each change, not once per frame.
        match substitution_report {
            Some(reason) if !self.projection_reported => {
                pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "camera projection has unusable values ({reason}); the renderer substitutes defaults"
                );
                self.projection_reported = true;
            }
            None => self.projection_reported = false,
            Some(_) => {}
        }
        // Step 3: Upload the values. This write is ordered before every
        // command buffer submitted after it, so this frame's passes read them.
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.parameters_data]),
        );
    }
}

// =============================================================================
// Free Functions
// =============================================================================

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
fn sanitize_projection(camera_component: &CameraComponent) -> (f32, f32, f32, Option<String>) {
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

    // Step 2: The near plane has to be a finite positive distance, because
    // the matrix is built around it.
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

        let (vertical_fov, near, far, report) = sanitize_projection(&camera);

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

        let (vertical_fov, near, far, report) = sanitize_projection(&camera);

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

        let (_, near, far, report) = sanitize_projection(&camera);

        assert_eq!(near, 10.0);
        assert!(far > near);
        assert!(report.is_some_and(|reason| reason.contains("far")));
    }
}
