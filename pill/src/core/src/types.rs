pub type FScalar = f32; // Floating point position type
pub type IScalar = i32; // Position type, for blocks
pub type UScalar = i32; // Position type, for blocks

// 2D Vectors
pub type Vector2f = na::Vector2<FScalar>;
pub type Vector2i = na::Vector2<IScalar>;
pub type Vector2u = na::Vector2<UScalar>;

// 3D Vectors
pub type Vector3f = na::Vector3<FScalar>;
pub type Vector3i = na::Vector3<IScalar>;
pub type Vector3u = na::Vector3<UScalar>;

// Quaternion
pub type UnitQuaternion = na::UnitQuaternion<FScalar>;

// ### Matrices ###
// Translation
pub type Translation2f = na::Translation2<FScalar>;
pub type Translation3f = na::Translation3<FScalar>;
pub type Translation2i = na::Translation2<IScalar>;
pub type Translation3i = na::Translation3<IScalar>;

// Rotation
pub type Rotation2 = na::Rotation2<FScalar>;
pub type Rotation3 = na::Rotation3<FScalar>;

// Translation + rotation
pub type Isometry2 = na::Isometry2<FScalar>;
pub type Isometry3 = na::Isometry3<FScalar>;

// Translation + rotation + uniform scale
pub type Similarity2 = na::Similarity2<FScalar>;
pub type Similarity3 = na::Similarity3<FScalar>;

// Translation + rotation + scale
pub type Affine2 = na::Affine2<FScalar>;
pub type Affine3 = na::Affine3<FScalar>;

// Translation + rotation + scale + projection
pub type Projection2 = na::Projective2<FScalar>;
pub type Projection3 = na::Projective3<FScalar>;

// Any transformation
pub type Transform2 = na::Transform2<FScalar>;
pub type Transform3 = na::Transform3<FScalar>;
