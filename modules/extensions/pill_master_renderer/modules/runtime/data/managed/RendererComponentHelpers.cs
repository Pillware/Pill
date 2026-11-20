// Hand-written conveniences over the generated renderer component mirrors.
//
// Responsibilities
// - Give gameplay code readable accessors for the generated structs in
//   `generated/pill_master_renderer_data_Components.g.cs`: vectors and
//   quaternions instead of numbered array elements.
// - Keep the "no mesh, no material" and placement shorthands the gameplay
//   projects rely on.
//
// Design
// The mirrors themselves are generated from the Rust registration, so their
// layout can never drift from the native components. These are `partial`
// halves of those same structs, and they declare no instance fields - only
// properties and static members - so they cannot change a mirror's layout.
// Everything here reads and writes the generated fields, which is what the host
// binds. The asset handles are already typed in the generated mirror
// (`Handle<Mesh>`, `Handle<Material>`), because the renderer mirrors its asset
// types with `#[pill_mirror_object]`.

using System;
using System.Numerics;
using System.Runtime.InteropServices;

namespace pill_master_renderer.component
{
    /// <summary>Conveniences over the generated <c>MeshRendererComponent</c> mirror.</summary>
    public partial struct MeshRendererComponent
    {
        /// <summary>
        /// "No mesh, no material". <c>default(MeshRendererComponent)</c> is all
        /// zeroes, and (index 0, generation 0) is the first asset ever added to
        /// its column - a live handle, not an absent one. A component that has
        /// not been given real handles yet should start from this.
        /// </summary>
        public static MeshRendererComponent None => new()
        {
            Mesh = global::TracyLive.Handle<global::pill_master_renderer_data.Mesh>.Invalid,
            Material = global::TracyLive.Handle<global::pill_master_renderer_data.Material>.Invalid,
        };

        /// <summary>A renderer drawing <paramref name="mesh"/> with <paramref name="material"/>.</summary>
        /// <remarks>
        /// The handles are the ones <c>assets.AddNamed(...)</c> returned for a
        /// mesh and a material. The parameter types are spelled in full because
        /// inside this struct <c>Mesh</c> and <c>Material</c> name its fields.
        /// </remarks>
        public static MeshRendererComponent From(
            global::TracyLive.Handle<global::pill_master_renderer_data.Mesh> mesh,
            global::TracyLive.Handle<global::pill_master_renderer_data.Material> material) => new()
        {
            Mesh = mesh,
            Material = material,
        };
    }

    /// <summary>Conveniences over the generated <c>TransformComponent</c> mirror.</summary>
    public partial struct TransformComponent
    {
        /// <summary>Position in world space.</summary>
        public Vector3 Translation
        {
            readonly get => new(Translation0, Translation1, Translation2);
            set => (Translation0, Translation1, Translation2) = (value.X, value.Y, value.Z);
        }

        /// <summary>Orientation as a unit quaternion (x, y, z, w).</summary>
        public Quaternion Rotation
        {
            readonly get => new(Rotation0, Rotation1, Rotation2, Rotation3);
            set => (Rotation0, Rotation1, Rotation2, Rotation3) = (value.X, value.Y, value.Z, value.W);
        }

        /// <summary>Per-axis scale.</summary>
        public Vector3 Scale
        {
            readonly get => new(Scale0, Scale1, Scale2);
            set => (Scale0, Scale1, Scale2) = (value.X, value.Y, value.Z);
        }

        /// <summary>No translation, no rotation, unit scale.</summary>
        public static TransformComponent Identity => At(0, 0, 0, 1);

        /// <summary>At (<paramref name="x"/>, <paramref name="y"/>, <paramref name="z"/>), unrotated, scaled uniformly.</summary>
        public static TransformComponent At(float x, float y, float z, float scale)
        {
            var transform = default(TransformComponent);
            transform.Translation = new Vector3(x, y, z);
            transform.Rotation = Quaternion.Identity;
            transform.Scale = new Vector3(scale);
            return transform;
        }
    }

    /// <summary>Conveniences over the generated <c>DirectionalLightComponent</c> mirror.</summary>
    public partial struct DirectionalLightComponent
    {
        /// <summary>Light colour, linear RGB.</summary>
        public Vector3 Color
        {
            readonly get => new(Color0, Color1, Color2);
            set => (Color0, Color1, Color2) = (value.X, value.Y, value.Z);
        }

        /// <summary>A light of <paramref name="color"/> at <paramref name="intensity"/>.</summary>
        public static DirectionalLightComponent From(Vector3 color, float intensity)
        {
            var light = default(DirectionalLightComponent);
            light.Color = color;
            light.Intensity = intensity;
            return light;
        }
    }

    /// <summary>Conveniences over the generated <c>CameraComponent</c> mirror.</summary>
    public partial struct CameraComponent
    {
        /// <summary>An enabled perspective camera.</summary>
        public static CameraComponent Perspective(float verticalFov, float near, float far, int priority = 0) => new()
        {
            Enabled = 1,
            Priority = priority,
            VerticalFov = verticalFov,
            Near = near,
            Far = far,
        };
    }
}
