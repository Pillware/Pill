# pill_blender_link

Edit a mesh asset in Blender and see every change in the Pill editor as you
make it. The geometry streams over a local socket straight into the mesh asset
in memory; it reaches the mesh's source file only when you save from the
editor. The other direction works too: a mesh modelled in Blender becomes a
Pill asset from the Properties editor's **Pill** block.

An editor-only extension. The folder holds only an editor crate,
`pill_blender_link_editor` in `modules/editor`, so the egui editor always
loads it, and no game build (standalone, shipping or web) ever does. The
Blender side lives beside it: the add-on in `blender_addon/` and its checks
in `tests/`.

## Using it

1. Select an entity with a `MeshRendererComponent` in the editor.
2. Click the Blender logo at the right end of that component's Inspector
   header.
3. The mesh opens as a new object, selected and framed, in the Blender you
   already have open. Only when no Blender is running does the editor start
   one (see *Which Blender* below).
4. Edit the object in Object mode or Edit mode, with or without modifiers.
   Every change is sent to the editor at up to 30 updates a second, and the
   renderer redraws the mesh on the next frame.
5. Save when you want to keep the change: the floppy button next to the
   Blender logo, or **Save and quit** when the editor asks on exit.

Clicking the Blender button again for a mesh that is already open selects its
object in Blender.

## Creating a mesh in Pill from Blender

A mesh modelled in Blender can become a Pill asset, and then streams like any
other link - the flow the other direction needs no entity to start from.

1. Select a mesh object in Blender, open the **Object** tab of the Properties
   editor and expand **Pill**. It says *Not linked to any Pill mesh*.
2. Click **Create new Pill mesh**. The file dialog opens in the connected
   project's `res` folder and offers `.obj` files only; it comes up with the
   object's own name.
3. Choose a folder and a name, and confirm.

The add-on sends the object's evaluated geometry - what the viewport shows,
modifiers applied, before the object's own transform - with the picked file.
The editor writes the geometry there with a `.meta` beside it, spawns an entity
for it at the origin, and answers with the new link id. The object then carries
`pill_link_id` and `pill_link_session`, and every edit after that streams into
the asset like an object opened from the editor.

- **The picked file is the asset.** The path must be below the project's `res`
  and end in `.obj`; the dialog starts there, and the editor checks it again,
  refusing anything else with the reason.
- **A taken file is refused.** A create never replaces a file: a second
  create for the same file answers *an asset with that name is already loaded*
  and spawns nothing.
- **One object, one asset.** An object that is already linked refuses a second
  create in Blender itself, before anything is sent.
- **No material.** The new entity carries the mesh and nothing else, so it
  draws nothing until you assign a material to it in the editor. The Blender
  object's material is not sent back: materials travel editor to Blender only.
- **At the origin.** The object's transform is neither baked into the mesh nor
  copied onto the entity; move the entity in the editor.
- **Saving bakes.** The geometry is written as triangles with the modifiers
  applied, so the first save bakes the modifier stack. Until then Blender keeps
  the modifiers and every update is still their result.

## Connecting an object to an existing asset

A mesh that is already a project asset can be picked from Blender's side:
select the object, open the **Object** tab of the Properties editor, expand the
**Pill** block, set **Mesh file** with the browse button (the file under the
project's `res`), and click **Connect to Pill mesh**.

The object becomes a link for the rest of the session: edits flow into the
asset exactly like an object opened from the editor. Connecting sends the
object's mesh straight away, so the asset holds the object's geometry as soon
as the button turns into **Disconnect from Pill mesh**; the file on disk
changes at the next save. The editor keeps the name rules - the file must be
under the project's `res` and end in `.obj`, and anything else is refused in
the block with the reason, which is the only text the block adds - and it
imports the file on first use (writing the `.meta` if it has none).
**Disconnect from Pill mesh** clears the object's link so it stops streaming;
the editor keeps the link for the rest of its run.

The panel is not a tab of its own: Blender's Properties tabs are a fixed set
that add-ons cannot extend, so the block lives inside the Object tab.

## Which Blender

- **A linked Blender is used.** The first time the button is used in an editor
  run, the editor installs the link as a Blender add-on, in a background
  Blender, and enables it in your Blender preferences. From then on every
  Blender you open connects to the editor by itself, and the mesh opens there.
  The installed add-on is a small loader that runs `pill_blender_link.py` from
this checkout, so updating the script needs no reinstall. A Blender that is
already open keeps the copy it loaded: the edit shows up at the next start, or
when the add-on is disabled and re-enabled in Preferences. Each enable prints
`[pill_blender_link] v<version> loaded from <path>` in Blender's console, which
is the quickest way to see which copy a running Blender has.
- **A Blender that is open but not linked is not replaced.** If Blender is
  running and has not connected (it was opened before the add-on was enabled),
  the editor starts no second one: it waits, and after a few seconds a toast
  says to enable *Pill Engine Live Link* in Edit > Preferences > Add-ons (it is
  already installed) or to restart Blender. The mesh opens as soon as Blender
  connects.
- **Only with no Blender running** does the editor start one, with the link
  script and the editor's port.
- **One Blender at a time.** When several Blenders with the add-on are open,
  the first to connect keeps the link; the others are answered *busy*, show
  "another Blender is linked" in their Pill tab, and take over when it closes.
- Set `PILL_BLENDER_INSTALL_ADDON=0` to keep the editor from touching your
  Blender preferences; install `blender_addon/pill_blender_link.py` yourself
  then. To remove it, disable or remove *Pill Engine Live Link* in Blender's
  add-ons list.

## The buttons

The Blender logo carries a dot in its lower-right corner for the link's state,
and its hover text says the same in words, with the triangle count and when
the last update arrived:

| Dot    | Meaning                                                      |
| ------ | ------------------------------------------------------------ |
| none   | The mesh was never opened in Blender.                        |
| amber  | Waiting for Blender to start and connect.                    |
| accent | Linked to a connected Blender.                               |
| green  | Updates are streaming in (an update within the last second). |
| grey   | Blender is not connected; a click starts it again.           |

A second, upper-right amber dot on both buttons means **unsaved**: the asset
holds geometry its file does not. The save button appears once the mesh is
linked and can be clicked while there is something to save; its hover text
names the file, or says why the mesh cannot be saved (a mesh built in code, or
an extension's read-only resource).

## Unsaved work

- **Saving** writes the mesh over its `.obj` under the project's `res`, in a
  form that imports back to the same geometry under the file's own `.meta`
  settings (V flip included). The write goes to a temporary file and is renamed
  over the source, so the asset watcher never reads half a file. The watcher's
  re-import is then taken as the saved state.
- **An unsaved mesh is marked** with the dot on both buttons and a dot on its
  tile in the Assets tab. Undoing back to the saved geometry clears it.
- **Quitting asks.** File > Quit and the main window's close button open a
  prompt listing the unsaved meshes: **Save and quit** (Enter), **Quit without
  saving**, or **Cancel** (Escape). If a save fails, the editor stays open and
  says why. A scripted `quit` and the test deadline quit without asking.
- **Anything that rewrites the file wins.** When the file is re-imported for
  another reason (another program wrote it), the asset takes the file's
  geometry and that becomes the saved state.

## Undo

Each run of updates from Blender - everything until Blender has been quiet for
0.4 s, so one drag is one step - becomes one step of the editor's undo
history, named "Blender edit &lt;file&gt;". Undo and Redo in the editor put the
mesh back and replace the Blender object's mesh too, so the two stay the same.
The replaced object keeps its materials and its mode (Edit mode included), but
it is rebuilt from the asset's triangles, so quads and n-gons made in Blender
become triangles again. Undo in Blender works as before: the undone geometry
streams back as a new editor step.

## Materials

The linked object also shows the clicked entity's material, as a Blender node
tree built from what the PBR shader actually computes - not from a guess about
which slot means what.

- **What syncs.** The five inputs of the shader's surface become the Principled
  BSDF's **Base Color**, **Roughness**, **Metallic**, **Normal** and **Emission
  Color**. The editor reads the cooked fragment, so a texture times a parameter
  arrives as an image node into a multiply with an RGB node, a packed
  roughness/metallic map arrives through one colour splitter, and a
  tangent-space normal goes through a **Normal Map** node. Emission is sent as
  a colour with strength 1.0.
- **Parameters are nodes.** Every material parameter the surface reads is a
  named **Value** or **RGB** node (`pbr_base`, `pbr_roughness`, ...), and every
  texture is an image node named after its slot (`base_color`, `normal`, ...).
  Node edits you make in Blender survive a parameter edit, because only the
  values are re-sent.
- **Textures** load from the project's `res` by absolute path, with the colour
  space their slot declares: colour maps read as **sRGB**, normal maps as
  **Non-Color**.
- **Three messages keep it current.** `material` carries the whole graph and
  rebuilds the tree; `material_values` only sets the parameter nodes; and
  `material_images` only reloads the image nodes whose texture content moved
  (a repainted PNG shows up in Blender without touching the rest of the tree).
- **A full send rebuilds the node tree, and that discards node edits made in
  Blender.** It happens when the mesh is opened in a Blender that has no tree
  for it (including after you delete the object there), when the entity's
  material changes, when the shader is re-cooked, or when a texture slot binds
  a different texture.
- **What cannot be expressed stays unconnected.** A surface field built from
  something the editor does not translate (a custom function, a texture
  projection, a mix) leaves its Principled input at the default, and the
  editor's log names the field and the reason:
  `[blender_link] material field not translated`.
- **A shader that builds no surface still gets a material.** The slot names
  the renderer standardised are placed by name alone - `base_color`,
  `metallic_roughness`, `normal` and `emissive` as textures, each `pbr_*`
  parameter tinting the texture it belongs to - so a simple custom shader
  shows its parameters on the Principled inputs. Any other slot is left
  unconnected and named in the editor's log; nothing is guessed from a slot's
  type, order or position.
- **Normals follow the shader, not a setting.** Pill's tangent frame counts V
  upward (the OpenGL / +Y green convention) and the fragment does
  `sample * 2 - 1`, which is what Blender's Normal Map node expects, so the
  green channel is never flipped. The link's V flip is about geometry UVs
  only.
- **The graph comes from the shader the renderer draws.** The editor reads the
  material's shader as it was cooked, not the `.hlsl` source, so a module
  header such as `pbr_surface.hlsl` reaches a project's already-cooked shaders
  at that project's next cook - a host restart, or a re-cook of that shader -
  and not the moment the header changed.
- **The material fills slot 0** of the object; any other material slots you add
  stay as they are.
- **Blender 4.0 or newer** for the material side. The script also finds the
  older **Emission** input name, but the 4.x Principled layout is what is
  tested.

## What to know

- **Every user of the mesh changes.** The edit goes into the asset, so every
  entity that draws that mesh shows it.
- **Shading.** Faces whose corner normals all match the face are flat, and an
  edge where the asset's normals split is marked sharp, so Blender's own
  normals reproduce the asset's shading and stay correct while you edit.
  Blender's split normals (modifiers applied) are what stream back.
- **Coordinates.** Pill is Y up and Blender is Z up; the script converts both
  ways (Blender's OBJ convention: forward -Z, up Y). Texture V is flipped to
  Blender's bottom-up convention and back.
- **Instances are one mesh.** A Geometry Nodes scatter is merged into the
  asset's single mesh while the Pill block's *Realize geometry nodes
  instances* switch is on (the default); it keeps one material and stops
  being instances in the editor.
- **The mesh opens as triangles**, because that is what the asset stores.
  Blender's *Tris to Quads* tidies it if you prefer to model in quads.

## Settings

| Environment variable         | Meaning                                                                                                                                                   |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PILL_BLENDER_PATH`          | The Blender executable to start. Otherwise the newest `Program Files\Blender Foundation\Blender *` install, the macOS app bundle, or `blender` on `PATH`. |
| `PILL_BLENDER_LINK_PORT`     | The local port the editor listens on (default `47390`, bound to `127.0.0.1` only). A Blender the editor starts inherits it; the add-on reads it too.      |
| `PILL_BLENDER_INSTALL_ADDON` | `0`, `false`, `no` or `off` stops the editor from installing and enabling the add-on in Blender's preferences.                                            |

## Installing the add-on by hand

The editor installs the add-on itself (above). To do it by hand instead,
install `blender_addon/pill_blender_link.py` (Edit > Preferences > Add-ons >
Install from disk), or run `blender -b --python-exit-code 1 --python
blender_addon/install_addon.py -- <path of pill_blender_link.py>`. The add-on
connects to the editor on its own and retries every two seconds while the
editor is not running.

The 3D viewport's sidebar gets a **Pill** tab with one block, **Connection**:
its status line, *Connected to: Pill* or *Not connected* (re-read every three
seconds), the port, and one button that reads **Connect** or **Disconnect**
depending on that status. Everything that changes an object's link lives in the
Properties editor instead (below).

### The Object tab's Pill block

The Properties editor's **Object** tab carries a **Pill** block, which is where
an object is linked, created as an asset, or unlinked. It is disabled as a
whole while Blender is not connected to the editor, since nothing in it can
reach the editor, and it holds:

- **Linked Pill mesh:** the asset's path below the project's `res`
  (`Linked Pill mesh: models/teapot.obj`), or `Linked Pill mesh: None` while
  the object is not a link;
- *Realize geometry nodes instances*, only when the object carries a Geometry
  Nodes modifier - see below - on by default;
- *Not linked to any Pill mesh*, with **Link to existing Pill mesh** (a file
  dialog over the project's `res`, `.obj` files only) and **Create new Pill
  mesh** (the same dialog, writing the new file there);
- **Disconnect from Pill mesh** once the object is a link;
- the reason, when a request was refused.

### Geometry Nodes

A Geometry Nodes tree that *instances* geometry - an Instance on Points node
scattering an object, a Collection Info node - does not put that geometry in
the object's mesh: Blender keeps the instances beside it, which is why the
link exports nothing for them by default. The **Realize geometry nodes
instances** switch in the Pill block decides what happens, and it is on by
default:

- **On** - the instances are merged into the one mesh the link sends, each
  moved by its own placement, so a scatter reaches Pill as the geometry the
  viewport shows. One read per source mesh serves every instance of it.
- **Off** - only the object's own evaluated mesh is sent, so a tree that
  outputs instances alone sends nothing. This is the old behaviour.

The merge is exactly what a **Realize Instances** node in the tree would
produce, so either way works; the switch spares the person editing the tree.
What it cannot do: an asset has one material, so per-instance materials
(GN *Set Material*, or instances of different objects with different
materials) are not carried - the scatter takes the object's material. A
scatter also becomes one mesh in the editor, so its instances are no longer
shared there.

When the add-on is enabled, a Blender the editor starts leaves the link to the
add-on.

## Testing it

The scripted channel can press the buttons: `command
pill_blender_link.open_in_blender` (or `pill_blender_link.save_to_source`)
runs the command on the selected entity. The quit prompt answers to `key
enter` and `key escape`, and the command palette (`palette quit`, `palette
run`) reaches File > Quit.

Two end-to-end checks live in `tests/` beside this file, because each needs a
tool the ordinary suites do not: `blender_material_probe.py` runs inside
Blender and asserts the node tree the add-on builds, and
`blender_link_material_smoke.py` plays Blender's half of the protocol against a
running editor and asserts the frames it sends. Both directions have their
pair: `blender_link_create_probe.py` drives the create and connect requests in
Blender, and `blender_link_create_smoke.py` creates an asset from a fake
Blender and reads the entity and the listing back. `python
modules/extensions/pill_blender_link/tests/run_blender_link_tests.py` runs all
four; `tests/README.md` says what each one covers.

## How it works

- `pill_editor_core::placements` holds the general hook: a module registers two
  *commands* with an `InspectorComponentHeader` placement (a draw function
  bound to a component name and to the module's liveness, like a module panel).
  The shell draws each on the matching component header from the
  `CommandState` the module published for that entity in `CommandStates`, or
  through the draw function the module registered for that command and place,
  and a click travels as a `CommandInvocation` through `CommandInbox`, where
  the module takes it. A module raises a toast with `PanelCommand::Notify`.
- `pill_editor_core::extension_edits` makes an extension's own state undoable:
  the extension pushes a step built by `extension_edit_entry` (the geometry
  before and after a run of updates, as opaque payloads); Undo and Redo replay
  it as `EditorCommand::ExtensionEdit`, which the drain hands back through
  `ExtensionEditInbox` on the extension's channel.
- `pill_editor_core::unsaved_work::UnsavedWork` is where modules report work
  held in memory; the executable holds a person's quit while it is not empty,
  the shell draws the prompt, and "Save and quit" is a numbered request each
  module answers once.
- `BlenderLinkServer` is a non-blocking socket server polled once per frame by
  the `blender_link` system (`post_project`, running in edit mode, so it
  keeps working while the editor edits or the engine is paused). It is a
  `Keep` resource, so Play and Stop leave the connection alone. It starts no
  thread, so a module reload cannot leave code running in an unmapped image,
  and as a shared resource it keeps Blender connected across that reload. Each linked mesh's
  state (`MeshLink`: saved and current geometry fingerprints, the open edit
  run, the content version it last wrote) lives in it.
- Each frame is an 8-byte prefix (header and body lengths), a JSON header and a
  binary body. Geometry is four little-endian arrays: positions, texture
  coordinates, normals (all `f32`) and triangle indices (`u32`). The editor
  checks every length, index and value before the asset is touched, then
  rebuilds tangents with `Mesh::calculate_tangents`.
- Messages: editor to Blender `welcome`, `busy`, `open_mesh`, `replace_mesh`,
  `resync`, `material`, `material_values`, `material_images`; Blender to editor
  `hello`, `mesh_update`, `mesh_positions`, `create_mesh`, `connect_asset`,
  `material_request`. The `welcome` names the editor's window title and the
  project's mounted `res` folder, which is where the two pickers start and the
  only place a create may write. A refused connection gets `busy`, then the
  editor shuts its sending side and drains the socket before closing, so the
  answer is not lost to a connection reset.
- The material graph crosses the link as plain JSON (`MaterialGraph` in
  `material_graph.rs`): an arena of nodes, each naming its inputs by index, and
  the Principled inputs. The add-on maps a node to `bpy` types, so the editor
  never imports Blender and the Blender side never guesses what a field means.
  An object opened fresh asks for the graph with `material_request`, which is
  how a deleted-and-reopened object gets its tree back.
- Whether a Blender is running is asked of the process list (`tasklist` on
  Windows, `pgrep` elsewhere) only when the button is used with no Blender
  connected; the add-on installer's own background Blender is not counted. Blender remembers the geometry it last sent or received per
  object and never sends the same state twice, so opening or replacing a mesh
  (or clicking a selection in Edit mode) streams nothing back.
- Only the newest full update of each mesh in a frame, and the newest
  positions update after it, are applied; an update identical to the asset is
  dropped, and while edits are arriving the editor draws at full rate instead
  of its idle polling rate.

## Streaming a drag

Blender reads the evaluated mesh with NumPy (`foreach_get` for every
attribute, `np.unique` to merge corners), so an export costs a few
milliseconds per ten thousand vertices instead of a Python loop over every
corner. On a 41k-vertex mesh one full export takes about 130 ms (the loop it
replaced took about 970 ms) and a positions-only update about 35 ms.

- **Full update (`mesh_update`).** The whole mesh, with a `topology_id`. Blender
  keeps, per object, how its corners map onto the vertices it sent.
- **Positions only (`mesh_positions`).** When the triangulation, the corner to
  vertex mapping and the UVs are exactly as in the last full update the editor
  received, and the corners merged into one vertex still agree on the normal,
  Blender sends only positions and normals (24 bytes a vertex instead of about
  56), naming that `topology_id`. A quad that flips its diagonal mid-drag, a
  bent flat face, a UV change or an edit to the topology sends a full update
  instead.
- **Resync.** The editor applies a positions update only to the vertex order
  it was written for, at the asset version the link itself wrote. After an
  undo, a re-import or a missed full update it writes nothing and sends
  `resync`; Blender answers with a full update even if nothing changed. The
  editor compares geometry independently of vertex order, so that answer -
  the same surface in Blender's order - is taken without an undo step or an
  unsaved mark.
- **The GPU.** A mesh that keeps its vertex and index counts is written into
  its existing GPU buffers (`RendererMesh::update_in_place`) instead of two new
  buffers being created every frame of a drag.
