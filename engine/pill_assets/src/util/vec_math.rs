//! Small fixed-size vector helpers on plain arrays.

pub(crate) fn add3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

pub(crate) fn sub3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

pub(crate) fn sub2(left: [f32; 2], right: [f32; 2]) -> [f32; 2] {
    [left[0] - right[0], left[1] - right[1]]
}

pub(crate) fn scale3(vector: [f32; 3], scalar: f32) -> [f32; 3] {
    [vector[0] * scalar, vector[1] * scalar, vector[2] * scalar]
}

pub(crate) fn dot3(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

pub(crate) fn cross3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

/// Normalize to unit length; returns the input unchanged if near-zero.
pub(crate) fn normalize3(vector: [f32; 3]) -> [f32; 3] {
    let length = dot3(vector, vector).sqrt();
    if length < 1e-10 {
        vector
    } else {
        scale3(vector, 1.0 / length)
    }
}

pub(crate) fn lerp3(start: [f32; 3], end: [f32; 3], factor: f32) -> [f32; 3] {
    add3(start, scale3(sub3(end, start), factor))
}

/// Orthonormal (tangent, bitangent) for `normal`, for when no UV-derived frame exists.
pub(crate) fn tangent_frame(normal: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if normal[1].abs() < 0.9 {
        [0.0f32, 1.0, 0.0]
    } else {
        [1.0f32, 0.0, 0.0]
    };
    let tangent = normalize3(cross3(up, normal));
    let bitangent = cross3(normal, tangent);
    (tangent, bitangent)
}
