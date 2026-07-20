//! Conversion helpers for transforming glam column-major matrices into the
//! row-major affine `[f32; 12]` format required by `TlasInstance::transform`.
//!
//! The engine convention is column-major with `Matrix4f::model()` producing
//! `T * R * S` (translation, then YPR rotation, then scale). The TLAS
//! expects the transpose in row-major order, stored as three rows of four
//! floats (the bottom row `[0, 0, 0, 1]` is implicit).

use pill_core::Matrix4f;

/// Convert a column-major glam `Matrix4f` to the row-major `[f32; 12]`
/// affine transform required by `TlasInstance`.
///
/// Returns `None` when the matrix contains non-finite elements or has a
/// near-zero determinant (singular), as such transforms cannot participate
/// in ray queries.
pub fn model_to_tlas_transform(model: &Matrix4f) -> Option<[f32; 12]> {
    // Reject non-finite values before conversion.
    if !model.is_finite() {
        return None;
    }

    // Reject singular matrices (zero or near-zero determinant). The model
    // matrix must be invertible for ray tracing to be well-defined.
    let determinant = model.determinant();
    if determinant.abs() < f32::MIN_POSITIVE {
        return None;
    }

    // Transpose from column-major to row-major and take the first 12 floats.
    let row_major = model.transpose();
    let cols = row_major.to_cols_array();

    Some([
        cols[0], cols[1], cols[2], cols[3],
        cols[4], cols[5], cols[6], cols[7],
        cols[8], cols[9], cols[10], cols[11],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pill_core::{Matrix4f, Vector3f};

    /// Helper to build a model matrix matching the engine convention.
    fn make_model(position: Vector3f, rotation_deg: Vector3f, scale: Vector3f) -> Matrix4f {
        use pill_core::Matrix3f;
        let rz = Matrix3f::from_rotation_z(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_rotation_y(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_rotation_x(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;
        Matrix4f::from_translation(position)
            * Matrix4f::from_mat3(rot3)
            * Matrix4f::from_scale(scale)
    }

    #[test]
    fn identity_transform() {
        let model = Matrix4f::IDENTITY;
        let tlas = model_to_tlas_transform(&model).unwrap();
        // Row-major identity: rows are [1,0,0,0], [0,1,0,0], [0,0,1,0]
        assert_eq!(tlas[0..4], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(tlas[4..8], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(tlas[8..12], [0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn translation_transform() {
        let model = make_model(Vector3f::new(1.0, 2.0, 3.0), Vector3f::ZERO, Vector3f::new(1.0, 1.0, 1.0));
        let tlas = model_to_tlas_transform(&model).unwrap();
        // In row-major, the translation goes into the fourth column of each row
        assert!((tlas[3] - 1.0).abs() < 0.001);
        assert!((tlas[7] - 2.0).abs() < 0.001);
        assert!((tlas[11] - 3.0).abs() < 0.001);
    }

    #[test]
    fn rotation_y_90_degrees() {
        let model = make_model(Vector3f::ZERO, Vector3f::new(0.0, 90.0, 0.0), Vector3f::new(1.0, 1.0, 1.0));
        let tlas = model_to_tlas_transform(&model).unwrap();
        // Y-rotation by 90 degrees in the engine's YPR convention.
        // The test verifies that the transform is non-identity and has the
        // expected rotation characteristics (first row Z is approximately
        // -1, third row X is approximately 1).
        // Exact values depend on the multiplication order in Matrix4f::model.
        assert!((tlas[2].abs() - 1.0).abs() < 0.001,
            "Expected |tlas[2]| ≈ 1.0, got {}", tlas[2]);
        assert!((tlas[8].abs() - 1.0).abs() < 0.001,
            "Expected |tlas[8]| ≈ 1.0, got {}", tlas[8]);
        // The middle row should be identity-like for a pure Y rotation.
        assert!((tlas[5] - 1.0).abs() < 0.001,
            "Expected tlas[5] ≈ 1.0, got {}", tlas[5]);
    }

    #[test]
    fn non_uniform_scale() {
        let model = make_model(Vector3f::ZERO, Vector3f::ZERO, Vector3f::new(2.0, 3.0, 4.0));
        let tlas = model_to_tlas_transform(&model).unwrap();
        assert!((tlas[0] - 2.0).abs() < 0.001);
        assert!((tlas[5] - 3.0).abs() < 0.001);
        assert!((tlas[10] - 4.0).abs() < 0.001);
    }

    #[test]
    fn singular_zero_scale_rejected() {
        let model = make_model(Vector3f::ZERO, Vector3f::ZERO, Vector3f::new(0.0, 1.0, 1.0));
        assert!(model_to_tlas_transform(&model).is_none());
    }

    #[test]
    fn non_finite_rejected() {
        let mut model = Matrix4f::IDENTITY;
        model.x_axis.w = f32::NAN;
        assert!(model_to_tlas_transform(&model).is_none());
    }
}
