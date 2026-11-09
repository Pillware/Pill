/// `[mul, add]` with view-space distance = mul / (add - depth) for the [0, 1] depth of glam's
/// `perspective_rh` (used by `camera_view_projection`), so shaders can linearize the depth buffer.
pub(crate) fn depth_unpack(near: f32, far: f32) -> [f32; 2] {
    [near * far / (far - near), far / (far - near)]
}

#[cfg(test)]
mod tests {
    use super::depth_unpack;
    use pill_core::{Matrix4f, Vector4f};

    #[test]
    fn depth_unpack_inverts_perspective_rh() {
        let (near, far) = (0.1, 100.0);
        let projection = Matrix4f::perspective_rh(1.0, 1.5, near, far);
        let [mul, add] = depth_unpack(near, far);
        for distance in [0.1_f32, 1.0, 7.5, 99.0] {
            let clip = projection * Vector4f::new(0.0, 0.0, -distance, 1.0);
            let depth = clip.z / clip.w;
            assert!((mul / (add - depth) - distance).abs() < distance * 1e-3);
        }
    }
}
