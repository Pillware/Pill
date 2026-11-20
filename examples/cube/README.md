# cube

The smallest drawn scene: one cube, one camera, one light, spinning.

`cube_rust` and `cube_csharp` are the same scene. What they demonstrate is
where the cube comes from - its mesh and its material are **files under `res`**,
not code:

```
res/models/cube.obj        the mesh, with a committed .meta holding its guid
res/materials/cube.material  a material naming the PBR shader by guid
res/editor/editor.theme    the editor's project theme layer
```

That is what makes the project editor-ready: the Assets tab lists those files,
the Inspector edits the material's parameters live, and an edit to either file
reloads the scene. A game build reads the very same files, so nothing has to
change to ship it.

## Run it

```powershell
cd modules
$env:PROJECT_PATH = "../examples/cube/cube_rust"
cargo run --package pill_standalone --features rendering
```

Point `PROJECT_PATH` at `cube_csharp` for the managed twin.

## What to try

- Edit `res/materials/cube.material` while the host runs: the cube's colour and
  roughness change without a restart.
- Open the project in the editor and drag the cube; the transform is the same
  component the spin system writes, so the spin wins on the next frame.
- Delete `res/models/cube.obj.meta` and restart: the importer writes a new one,
  which is why the committed file exists - it is what keeps the guid stable for
  the scene and the material that reference it.

## Related

`../cube_no_editor/` loads this same `cube.obj` with no `.meta` beside it, and
builds the material in code.
