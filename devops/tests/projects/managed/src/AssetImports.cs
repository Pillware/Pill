// Imports this project's source assets through their .meta files.
//
// Responsibilities
// - Import `res/textures/checker.png` and `res/models/cube.obj` through the
//   engine's `AssetManager`, which gives each a guid kept in a `.meta` file
//   beside it (written on the first run, read on every later one).
// - Log each asset's guid, so a run can be compared with the next one.
//
// Design
// The managed counterpart of a Rust project's `assets.import(...)` calls,
// written the same way: the startup takes `ResMut<AssetManager>` and imports
// the renderer's own `Texture` and `Mesh` types, which the renderer data crate
// mirrors to C# (`generated/pill_rendering_Components.g.cs`). The
// decoding and the `.meta` handling happen in that crate's Rust code; this
// file names paths and a policy only. A startup runs once for the life of the
// host, so it needs no guard against importing twice - and an import of a
// loaded path would return the loaded asset anyway.

// The renderer's asset types, generated from their Rust declarations.
using pill_rendering;

namespace TracyLive;

/// <summary>Imports the project's texture and mesh at startup.</summary>
public static class AssetImportStartup
{
    /// <summary>The texture, relative to <c>res</c>.</summary>
    internal const string CheckerTexture = "textures/checker.png";

    /// <summary>The mesh, relative to <c>res</c>.</summary>
    internal const string CubeMesh = "models/cube.obj";

    /// <summary>Imports both assets and logs their guids.</summary>
    [Startup]
    public static void Start(ResMut<AssetManager> assets)
    {
        ImportedAsset<Texture> texture = assets.Import<Texture>(CheckerTexture, MetadataPolicy.CreateIfMissing);
        ImportedAsset<Mesh> mesh = assets.Import<Mesh>(CubeMesh, MetadataPolicy.CreateIfMissing);
        Log.Info(
            $"[managed] imported {CheckerTexture} guid={texture.Guid} already_loaded={texture.AlreadyLoaded}; " +
            $"{CubeMesh} guid={mesh.Guid} already_loaded={mesh.AlreadyLoaded}");
    }
}
