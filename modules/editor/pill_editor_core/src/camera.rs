//! The scene viewports' orbit cameras and the mouse gestures that drive them.
//!
//! # Responsibilities
//!
//! - [`OrbitCamera`]: one scene view's view state. It holds the pivot the
//!   camera orbits, its yaw, pitch and distance, and derives the eye position,
//!   the forward, right and up directions and the rotation from them.
//! - The four gestures a camera answers - [`OrbitCamera::orbit`],
//!   [`OrbitCamera::pan`], [`OrbitCamera::dolly`] and [`OrbitCamera::zoom`].
//!   Orbit stops the pitch just short of the poles, and dolly and zoom keep
//!   the distance inside its range.
//! - [`navigation_binding`]: which gesture a mouse button and the keyboard
//!   modifiers mean under a keymap preset. The default preset is Blender's.
//!
//! # Design
//!
//! The camera is plain arrays and scalars. The executable owns the pointer and
//! the surface, so a camera's numbers are testable with no window. The chords
//! live in this module instead of inside the event handlers, and the
//! preferences file only names the preset it wants. A viewport's camera lives
//! beside its window, so the scene views navigate independently, and only the
//! *active* viewport's camera receives a gesture.

// Current crate
use crate::commands::KeymapPreset;

// =============================================================================
// Constants
// =============================================================================

/// The closest the eye may come to its pivot, in world units. The scene
/// camera's near clip plane is 0.1 away, so this limit keeps the point being
/// orbited in front of it.
pub const MIN_DISTANCE: f32 = 0.25;

/// The farthest the eye may go from its pivot, in world units.
pub const MAX_DISTANCE: f32 = 2_000.0;

/// The pitch stops just short of the poles. At exactly +-90 degrees the
/// camera looks straight down or up, the look direction has no horizontal
/// part left, and turning the yaw would only roll the view around the look
/// axis instead of turning it.
pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.02;

/// How far one pixel of an orbit drag turns the view, in radians.
pub const ORBIT_RADIANS_PER_PIXEL: f32 = 0.008;

/// How fast a dolly drag changes the distance. Every pixel scales the distance
/// by the same ratio, so a dolly feels the same at any distance.
pub const DOLLY_EXPONENT_PER_PIXEL: f32 = 0.006;

/// How much one wheel line moves the eye, as a distance ratio.
pub const ZOOM_RATIO_PER_LINE: f32 = 1.15;

/// The downward pitch a new camera starts with, in radians.
const DEFAULT_PITCH: f32 = 0.30;

/// The distance a new camera keeps from its pivot, in world units.
const DEFAULT_DISTANCE: f32 = 16.0;

// =============================================================================
// Types
// =============================================================================

/// One scene viewport's orbit camera.
///
/// The camera is not a world entity. The executable publishes it to the scene
/// surface as a view camera, so project systems can neither see nor move it.
/// Each scene view keeps its own, which is what makes navigation "the active
/// viewport's" rather than the editor's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    /// The point the camera looks at and orbits around.
    pub pivot: [f32; 3],
    /// Rotation around the world's Y axis, in radians.
    pub yaw: f32,
    /// Downward tilt, in radians; positive looks down at the pivot.
    pub pitch: f32,
    /// Distance from the pivot.
    pub distance: f32,
}

impl OrbitCamera {
    /// The direction the camera looks along.
    pub fn forward(&self) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        [-cos_pitch * sin_yaw, -sin_pitch, -cos_pitch * cos_yaw]
    }

    /// The camera's right direction. It is always horizontal, and the gizmo
    /// builds its screen-space rays from it.
    pub fn right(&self) -> [f32; 3] {
        let forward = self.forward();
        let horizontal = (forward[0] * forward[0] + forward[2] * forward[2]).sqrt();
        if horizontal <= f32::EPSILON {
            return [1.0, 0.0, 0.0];
        }
        [-forward[2] / horizontal, 0.0, forward[0] / horizontal]
    }

    /// The camera's up direction. It is perpendicular to the look and right
    /// directions, so it tilts with the pitch.
    pub fn up(&self) -> [f32; 3] {
        [
            self.right()[1] * self.forward()[2] - self.right()[2] * self.forward()[1],
            self.right()[2] * self.forward()[0] - self.right()[0] * self.forward()[2],
            self.right()[0] * self.forward()[1] - self.right()[1] * self.forward()[0],
        ]
    }

    /// The world position of the eye: the pivot moved back along the look
    /// direction by `distance`.
    pub fn eye(&self) -> [f32; 3] {
        let forward = self.forward();
        [
            self.pivot[0] - forward[0] * self.distance,
            self.pivot[1] - forward[1] * self.distance,
            self.pivot[2] - forward[2] * self.distance,
        ]
    }

    /// The camera's orientation as an `[x, y, z, w]` quaternion built from
    /// yaw and pitch, ready for a transform's rotation field.
    pub fn rotation(&self) -> [f32; 4] {
        let half_yaw = self.yaw * 0.5;
        let half_pitch = self.pitch * 0.5;
        let yaw_quaternion = [0.0, half_yaw.sin(), 0.0, half_yaw.cos()];
        let pitch_quaternion = [(-half_pitch).sin(), 0.0, 0.0, half_pitch.cos()];
        quaternion_product(yaw_quaternion, pitch_quaternion)
    }

    /// Turns the camera around the pivot by a drag's pixel delta. Dragging
    /// right turns the view right, and dragging down raises the eye so it
    /// looks more steeply down at the pivot.
    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        self.yaw -= delta_x * ORBIT_RADIANS_PER_PIXEL;
        self.pitch = (self.pitch + delta_y * ORBIT_RADIANS_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Moves the pivot so the world follows the drag, which means the pivot
    /// moves the opposite way. `world_per_pixel` is the world height of one
    /// screen pixel at the pivot's depth; the caller computes it from the
    /// distance and the field of view.
    pub fn pan(&mut self, delta_x: f32, delta_y: f32, world_per_pixel: f32) {
        let right = self.right();
        let up = self.up();
        for index in 0..3 {
            self.pivot[index] += (up[index] * delta_y - right[index] * delta_x) * world_per_pixel;
        }
    }

    /// Moves the eye along the view by a drag's pixel delta. Dragging up moves
    /// closer and dragging down moves farther, at a rate that feels the same
    /// at every distance.
    pub fn dolly(&mut self, delta_y: f32) {
        let factor = 2.0f32.powf(delta_y * DOLLY_EXPONENT_PER_PIXEL);
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Zooms by a wheel's line count. A positive count (wheel up) moves the
    /// eye closer and a negative one moves it farther.
    pub fn zoom(&mut self, lines: f32) {
        let factor = ZOOM_RATIO_PER_LINE.powf(-lines);
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            pivot: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            distance: DEFAULT_DISTANCE,
        }
    }
}

/// The mouse buttons a navigation gesture can bind to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraButton {
    /// The left button.
    Left,
    /// The middle button.
    Middle,
    /// The right button.
    Right,
}

/// The keyboard modifiers held during a press, as plain booleans. The event
/// handler converts winit's modifier state into this, because the navigation
/// rules compare booleans.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift held.
    pub shift: bool,
    /// Control held.
    pub control: bool,
    /// Alt held.
    pub alt: bool,
}

/// One camera navigation gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraNavigation {
    /// Turn around the pivot.
    Orbit,
    /// Slide the pivot in the view's plane.
    Pan,
    /// Move the eye along the view.
    Dolly,
}

// =============================================================================
// Free Functions
// =============================================================================

/// The gesture a press means under `preset`, or `None` when no camera gesture
/// claims the press.
///
/// Blender and Godot orbit with a plain middle-button drag, pan with
/// Shift+middle and dolly with Ctrl+middle. Unity and Unreal orbit with Alt
/// and the left button, pan with Alt and the middle button, and dolly with Alt
/// and the right button.
pub fn navigation_binding(
    preset: KeymapPreset,
    button: CameraButton,
    modifiers: Modifiers,
) -> Option<CameraNavigation> {
    match preset {
        KeymapPreset::Unity | KeymapPreset::Unreal => match (button, modifiers.alt) {
            (CameraButton::Left, true) => Some(CameraNavigation::Orbit),
            (CameraButton::Middle, true) => Some(CameraNavigation::Pan),
            (CameraButton::Right, true) => Some(CameraNavigation::Dolly),
            _ => None,
        },
        _ => middle_button_table(button, modifiers),
    }
}

/// The middle-button navigation Blender and Godot share: a plain drag orbits,
/// Shift+drag pans and Ctrl+drag dollies.
fn middle_button_table(button: CameraButton, modifiers: Modifiers) -> Option<CameraNavigation> {
    if button != CameraButton::Middle || modifiers.alt {
        return None;
    }
    match (modifiers.shift, modifiers.control) {
        (false, false) => Some(CameraNavigation::Orbit),
        (true, false) => Some(CameraNavigation::Pan),
        (false, true) => Some(CameraNavigation::Dolly),
        // Both held: no chord, so a stray press does nothing.
        (true, true) => None,
    }
}

/// The Hamilton product of two `[x, y, z, w]` quaternions: applying the
/// result turns a vector by `right` first and then by `left`.
fn quaternion_product(left: [f32; 4], right: [f32; 4]) -> [f32; 4] {
    let [left_x, left_y, left_z, left_w] = left;
    let [right_x, right_y, right_z, right_w] = right;
    [
        left_w * right_x + left_x * right_w + left_y * right_z - left_z * right_y,
        left_w * right_y - left_x * right_z + left_y * right_w + left_z * right_x,
        left_w * right_z + left_x * right_y - left_y * right_x + left_z * right_w,
        left_w * right_w - left_x * right_x - left_y * right_y - left_z * right_z,
    ]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn close(left: f32, right: f32) -> bool {
        (left - right).abs() < 1e-5
    }

    #[test]
    fn the_default_camera_looks_down_at_its_pivot() {
        let camera = OrbitCamera::default();
        let forward = camera.forward();
        assert!(forward[1] < 0.0, "the default looks down: {forward:?}");
        assert!(close(
            camera.eye()[1] - camera.pivot[1],
            DEFAULT_PITCH.sin() * camera.distance
        ));
        // The eye sits exactly `distance` away from the pivot, in the
        // direction opposite to `forward`.
        let offset = [
            camera.eye()[0] - camera.pivot[0],
            camera.eye()[1] - camera.pivot[1],
            camera.eye()[2] - camera.pivot[2],
        ];
        let length = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
        assert!(close(length, camera.distance), "eye distance {length}");
    }

    #[test]
    fn orbiting_turns_the_view_and_stops_at_the_poles() {
        let mut camera = OrbitCamera::default();
        let before = camera.forward();
        camera.orbit(40.0, 0.0);
        assert!(!close(camera.forward()[0], before[0]), "yaw moved the view");
        // A horizontal drag turns the view, and its direction is part of the
        // contract: a drag to the right turns the view to the right. From a
        // camera looking down -Z, that swings the forward vector toward +X.
        assert!(camera.yaw < 0.0, "a rightward drag turned the view left");
        assert!(
            camera.forward()[0] > 0.0,
            "the view turned toward -X on a rightward drag"
        );
        // A drag far past the pole clamps the pitch instead of flipping it
        // over.
        camera.orbit(0.0, 100_000.0);
        assert!(close(camera.pitch, MAX_PITCH));
        camera.orbit(0.0, -1_000_000.0);
        assert!(close(camera.pitch, -MAX_PITCH));
    }

    #[test]
    fn panning_moves_the_pivot_in_the_view_plane() {
        let mut camera = OrbitCamera {
            yaw: 0.0,
            pitch: 0.0,
            ..OrbitCamera::default()
        };
        // At pitch 0 the view looks along -Z, so right is +X and up is +Y.
        camera.pan(10.0, 0.0, 0.5);
        assert!(
            close(camera.pivot[0], -5.0),
            "drag right moves left: {:?}",
            camera.pivot
        );
        camera.pan(0.0, 10.0, 0.5);
        assert!(
            close(camera.pivot[1], 5.0),
            "drag down moves up: {:?}",
            camera.pivot
        );
        assert!(close(camera.pivot[2], 0.0), "pan stays in the view plane");
    }

    #[test]
    fn dolly_and_zoom_clamp_their_distances() {
        let mut camera = OrbitCamera::default();
        camera.dolly(100.0);
        assert!(camera.distance > 16.0, "dragging down backs off");
        camera.dolly(1_000_000.0);
        assert!(close(camera.distance, MAX_DISTANCE));
        camera.dolly(-1_000_000.0);
        assert!(close(camera.distance, MIN_DISTANCE));
        camera.zoom(1.0);
        assert!(
            camera.distance < MIN_DISTANCE * 1.5,
            "wheel up comes closer"
        );
        camera.zoom(-100.0);
        assert!(close(camera.distance, MAX_DISTANCE));
    }

    #[test]
    fn blender_navigation_is_the_default_table() {
        let modifiers = Modifiers::default();
        assert_eq!(
            navigation_binding(KeymapPreset::Blender, CameraButton::Middle, modifiers),
            Some(CameraNavigation::Orbit)
        );
        assert_eq!(
            navigation_binding(
                KeymapPreset::Blender,
                CameraButton::Middle,
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                }
            ),
            Some(CameraNavigation::Pan)
        );
        assert_eq!(
            navigation_binding(
                KeymapPreset::Blender,
                CameraButton::Middle,
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                }
            ),
            Some(CameraNavigation::Dolly)
        );
        // Unity and Unreal navigate on Alt chords, so Blender's table treats a
        // middle-button drag with Alt held as no gesture at all.
        assert_eq!(
            navigation_binding(
                KeymapPreset::Blender,
                CameraButton::Middle,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                }
            ),
            None
        );
        // In Blender's table the left button belongs to selection, whatever
        // the modifiers.
        assert_eq!(
            navigation_binding(KeymapPreset::Blender, CameraButton::Left, modifiers),
            None
        );
        // Godot uses the same middle-button table as Blender.
        assert_eq!(
            navigation_binding(KeymapPreset::Godot, CameraButton::Middle, modifiers),
            Some(CameraNavigation::Orbit)
        );
    }

    #[test]
    fn unity_navigation_uses_alt_and_the_left_button() {
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(
            navigation_binding(KeymapPreset::Unity, CameraButton::Left, alt),
            Some(CameraNavigation::Orbit)
        );
        assert_eq!(
            navigation_binding(KeymapPreset::Unity, CameraButton::Middle, alt),
            Some(CameraNavigation::Pan)
        );
        assert_eq!(
            navigation_binding(KeymapPreset::Unity, CameraButton::Right, alt),
            Some(CameraNavigation::Dolly)
        );
        assert_eq!(
            navigation_binding(
                KeymapPreset::Unity,
                CameraButton::Middle,
                Modifiers::default()
            ),
            None
        );
    }
}
