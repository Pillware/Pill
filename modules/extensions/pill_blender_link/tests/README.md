# Blender link tests

The end-to-end checks for the link's two directions - editing a Pill mesh in
Blender, and creating a Pill mesh from a Blender object. Each lives beside the
extension it tests, because each needs a tool the ordinary suites do not: a
Blender install, or a running editor with a client attached.

| File                             | What it proves                                                                                                                                                                                                                                                                                                                    | Needs                                                |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| `blender_material_probe.py`      | The add-on's material half, inside Blender: the node tree a graph builds, the values-only and images-only paths, the rebuild rule, the 3.x `Emission` fallback, and the fallback graph's shape. One line per check.                                                                                                                | A Blender 4.x install; no editor, no project.        |
| `blender_link_create_probe.py`   | The add-on's create half, inside Blender: the `create_mesh` frame with the picked file's path and its evaluated geometry (a level-2 Subdivision cube sends 384 vertices), the local refusal of a linked object and of an empty pick, the link properties and unsent baseline an ok answer sets, and a refusal that sets nothing. | A Blender 4.x install; no editor, no project.        |
| `blender_link_panel_probe.py`    | The Pill panels' layout and the two pickers, drawn against a recording fake layout: the sidebar's Connection block (status, port, one button), the Object tab's Pill block (*Not linked to any Pill mesh* with the link and create buttons, the disconnect button once linked, every row disabled without a connection), both pickers' `.obj` filter and the frames a pick sends, plus the mesh a connection pushes. | A Blender 4.x install; no editor, no project.        |
| `blender_link_material_smoke.py` | The editor's half, without Blender: a fake client greets the editor, reads its frames, and the scripted `command pill_blender_link.open_in_blender` asks for a mesh entity's material. Asserts `open_mesh` before `material`, all five Principled inputs, image and parameter nodes, and no refusals.                                                        | `cargo build -p pill_editor`, and the scene project. |
| `blender_link_create_smoke.py`   | The editor's half of a create, without Blender: a fake client sends `create_mesh` for the file the greeting's `res` folder names, and the script asserts the `create_result` fields, the three log tokens, exactly one new entity with a mesh renderer, the asset in the listing with its metadata, the refusal of the same file twice, and the refusal of a destination outside `res`. | `cargo build -p pill_editor`, and the scene project. |
| `blender_link_instances_probe.py` | The Geometry Nodes export, inside Blender: a 3x3 grid of instances of a plane exports 18 triangles into the host's own space (its transform is not baked), the merged geometry sits exactly where a Realize Instances node puts it, the object's realize switch empties the export when turned off, and editing the instanced source moves what is exported. | A Blender 4.x install; no editor, no project.        |
| `blender_link_test_support.py`   | Not a check: the frame codec, the assertion helper, the fake Blender both smokes use, and the `dump entities` row reader.                                                                                                                                                                                                        | Nothing on its own.                                  |
| `run_blender_link_tests.py`      | All seven checks, in order, with the exit codes summed.                                                                                                                                                                                                                                                                         | All of the above.                                    |

## Running them

```powershell
cd <repository>
python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py
python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py --probe-only
python modules/extensions/pill_blender_link/tests/run_blender_link_tests.py --smoke-only
```

`run_blender_link_tests.py` looks for Blender in `PILL_BLENDER`, then the
standard install folders, then `blender` on `PATH`. A probe can also be run by
hand, which is what a failing check is easiest to read from:

```powershell
& "C:\Program Files\Blender Foundation\Blender 4.5\blender.exe" -b `
    --factory-startup --python-exit-code 1 `
    --python modules/extensions/pill_blender_link/tests/blender_link_create_probe.py
```

A smoke script writes the editor's own log, its command file and its app data
under `local/editor-validation/egui-<timestamp>/`; that log is where the
editor-side tokens to look for live (`[blender_link] material sent`,
`[blender_link] created mesh asset`).

The create smoke writes `res/models/blender_create_smoke.obj` and its `.meta`
into the scene project and deletes both on the way out, so a killed run can
leave them behind: `git status` on `devops/tests/projects/scene` is the check.

## Not in the gate yet

`devops/tests/run_all.py` discovers `test_*.py` suites under `devops/tests/`,
so it does not find these. They are deliberately excluded from the gate while
they depend on a Blender install and on a built editor; wiring them in means a
devops wrapper that skips when Blender is absent, which does not exist yet.

## What the material probe covers

- The tree a full send builds: image times parameter for base colour, the
  packing map's green and blue channels for roughness and metallic, the normal
  through a `Normal Map` node, emission, and the BSDF feeding the output.
- Colour spaces: the colour map reads as sRGB, the normal map as `Non-Color`.
- A values-only send moves the parameter nodes and keeps every node and image.
- An images-only send reloads a repainted file and keeps the tree.
- A full send rebuilds the tree, so a node added in Blender is discarded.
- A graph shaped like the fallback (standard slot names with no surface): the
  channel goes straight onto Roughness without a clamp, and an input the graph
  leaves unconnected keeps Blender's default.
- The 3.x `Emission` input name is found when `Emission Color` is absent.

## What the create probe covers

- The frame: a `create_mesh` whose `path` is the file the picker named, whose
  `session` is the link's, and whose body is one 32-byte vertex record per
  vertex plus one index per triangle corner.
- The geometry is the evaluated one: a level-2 Subdivision cube sends 384
  vertices, not the cage's 24.
- The pending record the answer is matched by, cleared when the answer arrives.
- An ok answer sets `pill_link_id` and `pill_link_session`, remembers the
  baseline with `sent=False`, queues nothing, and sends `material_request` for
  the new link.
- A refusal sets no properties, remembers no baseline and records its reason
  for the Pill block; a linked object, an empty pick and a disconnected link
  are refused before anything is sent.
- The sidebar's connection labels: `Connected to: <editor title>` while the
  socket is up (and a placeholder before the greeting names the editor),
  `Not connected` plus its reason once it closes, and the three-second check
  recording the label it shows.

## What the panel probe covers

- The viewport sidebar is one block, *Connection*, in the *Pill* category, and
  carries no second block.
- Connection carries the status line (`Not connected` / `Connected to: Pill`),
  the reason under it once a link closes, the port field, and exactly one
  button that reads Connect or Disconnect with the matching operator.
- The Object tab's Pill block: a `PROPERTIES`/`object` panel that shows for a
  mesh object only. With no connection every row of it is disabled; with one,
  an unlinked object shows *Linked Pill mesh: None*, *Not linked to any Pill
  mesh* and the two buttons `pill.link_pick` and `pill.link_create`, and a
  linked object shows the asset's path in *Linked Pill mesh:* and only the
  disconnect button.
- The *Realize geometry nodes instances* switch is drawn only for an object
  with a Geometry Nodes modifier, and disappears with the modifier.
- Both pickers offer `.obj` files only, and the create picker asks before a
  name is taken.
- A pick sends `connect_asset` with the picked file's path and the object's
  name; an empty pick is refused with its reason and sends nothing.
- Connecting sends the object's mesh at once: the object is queued and the
  next streaming step records one `mesh_update` that names the new link and
  carries the object's vertices.
- A refusal leaves the object unlinked, keeps the labels and adds the editor's
  reason as an error line.

## What the instances probe covers

- A Geometry Nodes object is recognised by its modifier, and its realize
  switch is on by default.
- A 3x3 grid of instances of a plane exports 18 triangles (what the viewport
  draws), and the merged geometry sits exactly where a Realize Instances node
  puts it - the host object's own transform is not baked into the mesh.
- Turning the switch off exports nothing for a tree whose output is instances
  only, which is what the link did before the merge.
- Editing the instanced source mesh moves what is exported.
