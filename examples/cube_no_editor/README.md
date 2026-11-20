# cube_no_editor

The same cube as `../cube/`, with as little on disk as a project can get away
with: one `res/models/cube.obj`, deliberately without a `.meta` beside it, and
no scene file or editor theme. The material is a builder call in both projects.

Both projects import the model with `MetadataPolicy.ReadIfPresent`: it decodes
under the importer's defaults and its guid lives only in memory, so the `res`
tree is exactly what the repository ships and no run writes a file back.

This is the shape of a project that is only ever a game: nothing it draws has
to be registered in an asset database with a stable guid, and it runs the same
from a headless host and a windowed one.

| Project                 | Mesh                                                                 | Material                                         |
| ----------------------- | -------------------------------------------------------------------- | ------------------------------------------------ |
| `cube_no_editor_rust`   | `res/models/cube.obj`, imported with `MetadataPolicy::ReadIfPresent` | `Material::builder` over the simple lit pipeline |
| `cube_no_editor_csharp` | the same file, imported with `MetadataPolicy.ReadIfPresent`          | `Material.Builder` over the PBR shader           |

## Run it

```powershell
cd modules
$env:PROJECT_PATH = "../examples/cube_no_editor/cube_no_editor_rust"
cargo run --package pill_standalone --features rendering
```

## Related

`../cube/` authors the same scene as files, with a committed `cube.obj.meta` so
the model has a guid that survives a rebuild; comparing the two is the point.
