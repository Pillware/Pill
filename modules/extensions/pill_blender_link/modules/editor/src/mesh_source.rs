//! Writing a linked mesh back over its source file.
//!
//! # Responsibilities
//!
//! - Resolve the file a mesh asset was imported from: its asset name is the
//!   path below the project's `res`, and only a file that is really there is
//!   written (a module's read-only resources are refused).
//! - Resolve the two paths a person picks in Blender: the file an object is
//!   linked to (which exists) and the file a create writes (which does not
//!   yet), both named below the project's `res`.
//! - Encode a mesh as a Wavefront OBJ that imports back to the same geometry
//!   under the file's own import settings (the `.meta`'s `flip_v`).
//! - Write it through a temporary file and a rename, so the asset watcher
//!   never reads half a file.
//!
//! # Design
//!
//! The OBJ keeps the mesh's single-indexed vertices: one `v`/`vt`/`vn` line
//! each and faces naming the same index three times (`f 1/1/1 ...`). Floats
//! are written in Rust's shortest round-trip form, so positions and normals
//! read back bit for bit. After the write the asset watcher re-imports the
//! file into the same asset slot, which the link notices as an outside change
//! and takes as the saved state.

// Standard library
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// External crates
use pill_engine::asset_metadata::{AssetImport, MetadataPolicy};
use pill_engine::{AssetManager, Handle};
use pill_master_renderer_data::assets::Mesh;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the asset name of the file a create should write: the picked path
/// relative to the project's `res` (`models/teapot.obj`).
///
/// The file does not exist yet, so the folder it names is what has to be real:
/// both it and the mounted directory are canonicalised, and the file's own name
/// is appended as picked. A `.obj` name is required, and a path that is not
/// below the mounted directory is refused.
///
/// # Errors
///
/// Returns a message when no asset directory is mounted, when the picked path
/// names no file, when its folder cannot be read, when it is outside the
/// project's `res`, or when it is not an `.obj` file.
pub fn created_mesh_name(picked_path: &Path) -> Result<String, String> {
    let Some(asset_directory) = pill_engine::asset_store::mounted_directory() else {
        return Err("no project asset directory is mounted".to_owned());
    };
    let canonical_directory = asset_directory
        .canonicalize()
        .unwrap_or_else(|_| asset_directory.clone());
    let parent = picked_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("{} names no folder", picked_path.display()))?;
    let file_name = picked_path
        .file_name()
        .ok_or_else(|| format!("{} names no file", picked_path.display()))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("{} cannot be read: {error}", parent.display()))?;
    let relative_path = canonical_parent
        .join(file_name)
        .strip_prefix(&canonical_directory)
        .map(|relative_path| relative_path.to_path_buf())
        .map_err(|_| format!("{} is outside the project's res", picked_path.display()))?;
    let asset_name = relative_path.to_string_lossy().replace('\\', "/");
    if !asset_name.to_ascii_lowercase().ends_with(".obj") {
        return Err(format!("{asset_name} is not an .obj file"));
    }
    Ok(asset_name)
}

/// A mesh created from Blender: where its file is and the asset it became.
#[derive(Clone, Debug)]
pub struct CreatedMesh {
    /// The project-relative path the asset was written to, `models/<name>.obj`.
    pub relative_path: PathBuf,
    /// The file on disk.
    pub absolute_path: PathBuf,
    /// The imported asset, ready to draw and to link.
    pub handle: Handle<Mesh>,
}

/// Writes `mesh` to the file `relative_name` names below the project's `res`
/// and imports it as an asset, leaving the metadata file the importer writes
/// beside it.
///
/// The `.meta` is written with the type's default import settings
/// (`flip_v: true`, `calculate_tangents: true`), and the OBJ is encoded with V
/// flipped to match, so the file imports back to the same geometry.
///
/// # Errors
///
/// Returns a message when no asset directory is mounted, when the folder cannot
/// be created, when the file already exists (a create never replaces), or when
/// the import fails. A refused import removes the file again, so a failed
/// create leaves nothing behind.
pub fn create_mesh_asset(
    relative_name: &str,
    mesh: &Mesh,
    assets: &mut AssetManager,
) -> Result<CreatedMesh, String> {
    // Step 1: where the file goes. The caller passes a name
    // `created_mesh_name` produced, so the path rules have already run.
    let Some(asset_directory) = pill_engine::asset_store::mounted_directory() else {
        return Err("no project asset directory is mounted".to_owned());
    };
    let relative_path = PathBuf::from(relative_name);
    let absolute_path = asset_directory.join(&relative_path);
    if let Some(folder) = absolute_path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    }
    // Step 2: the file. `write_new` refuses an existing destination, which is
    // the "that name is taken" refusal.
    let obj_text = encode_obj(mesh, true);
    pill_engine::asset_store::write_new(&absolute_path, obj_text.as_bytes())
        .map_err(|error| format!("cannot write {}: {error:?}", absolute_path.display()))?;
    // Step 3: import it. `CreateIfMissing` writes the `.meta` beside the
    // source, after the source decoded.
    let imported = assets.import::<Mesh>(AssetImport::new(
        relative_path.clone(),
        MetadataPolicy::CreateIfMissing,
    ));
    match imported {
        Ok(outcome) if !outcome.already_loaded => Ok(CreatedMesh {
            relative_path,
            absolute_path,
            handle: outcome.handle,
        }),
        Ok(_) => {
            let _ = std::fs::remove_file(&absolute_path);
            Err(format!("{relative_name} is already loaded"))
        }
        Err(error) => {
            let _ = std::fs::remove_file(&absolute_path);
            Err(format!("cannot import {relative_name}: {error:?}"))
        }
    }
}

/// Returns the file to write a mesh asset back to: `asset_name` (the path
/// below `res`, as the asset store names an imported asset) under the
/// project's mounted asset directory.
///
/// # Errors
///
/// Returns a message when no asset directory is mounted, when the name is not
/// an `.obj` path, or when no such file exists under the project's `res`
/// (an extension's built-in resource, or a mesh built in code).
pub fn source_file_path(asset_name: &str) -> Result<PathBuf, String> {
    if !asset_name.to_ascii_lowercase().ends_with(".obj") {
        return Err(format!("{asset_name} is not an .obj file"));
    }
    // A name that climbs out of `res`, or is absolute, is not a project file.
    if Path::new(asset_name).is_absolute()
        || asset_name
            .split(['/', '\\'])
            .any(|path_part| path_part == "..")
    {
        return Err(format!("{asset_name} is outside the project's res"));
    }
    let Some(asset_directory) = pill_engine::asset_store::mounted_directory() else {
        return Err("no project asset directory is mounted".to_owned());
    };
    let source_path = asset_directory.join(asset_name);
    if !source_path.is_file() {
        return Err(format!(
            "{asset_name} is not a file under the project's res, so it cannot be overwritten"
        ));
    }
    Ok(source_path)
}

/// Returns the asset name (`models/teapot.obj`, the path below `res`) of a
/// file the person picked, so a Blender object can be pointed at an asset the
/// project already holds.
///
/// # Errors
///
/// Returns a message when no asset directory is mounted, when the file cannot
/// be read, when it is outside the project's `res`, or when it is not an `.obj`
/// file.
pub fn asset_name_from_path(picked_path: &Path) -> Result<String, String> {
    let Some(asset_directory) = pill_engine::asset_store::mounted_directory() else {
        return Err("no project asset directory is mounted".to_owned());
    };
    // Both sides are canonicalised: a person picks a path from a dialog, which
    // may hold `.` steps, a different case on Windows, or a link.
    let canonical_directory = asset_directory
        .canonicalize()
        .unwrap_or_else(|_| asset_directory.clone());
    let canonical_file = picked_path
        .canonicalize()
        .map_err(|error| format!("{} cannot be read: {error}", picked_path.display()))?;
    let relative_path = canonical_file
        .strip_prefix(&canonical_directory)
        .map_err(|_| format!("{} is outside the project's res", picked_path.display()))?;
    let asset_name = relative_path.to_string_lossy().replace('\\', "/");
    if !asset_name.to_ascii_lowercase().ends_with(".obj") {
        return Err(format!("{asset_name} is not an .obj file"));
    }
    Ok(asset_name)
}

/// The loaded mesh an asset name names, importing its file when the project
/// has not loaded it yet.
///
/// The import writes the `.meta` beside a file that has none
/// (`CreateIfMissing`), the same way opening any project asset would.
///
/// # Errors
///
/// Returns the importer's message when the file cannot be read or is not a
/// mesh.
pub fn load_mesh_asset(
    asset_name: &str,
    assets: &mut AssetManager,
) -> Result<Handle<Mesh>, String> {
    if let Some(handle) = assets.handle_by_name::<Mesh>(asset_name) {
        return Ok(handle);
    }
    let imported = assets.import::<Mesh>(AssetImport::new(
        PathBuf::from(asset_name),
        MetadataPolicy::CreateIfMissing,
    ));
    match imported {
        Ok(outcome) => Ok(outcome.handle),
        Err(error) => Err(format!("cannot import {asset_name}: {error:?}")),
    }
}

/// Reads whether the file's import flips the V texture coordinate: the
/// `.meta` sidecar's `settings.flip_v`, which defaults to `true` like the
/// importer's.
pub fn import_flips_v(source_path: &Path) -> bool {
    let mut metadata_path = source_path.as_os_str().to_owned();
    metadata_path.push(".meta");
    std::fs::read(PathBuf::from(metadata_path))
        .ok()
        .and_then(|metadata_bytes| {
            serde_json::from_slice::<serde_json::Value>(&metadata_bytes).ok()
        })
        .and_then(|metadata| metadata.pointer("/settings/flip_v")?.as_bool())
        .unwrap_or(true)
}

/// Encodes a mesh as OBJ text whose import, with `flip_v` as the file's
/// import settings say, gives the mesh back.
pub fn encode_obj(mesh: &Mesh, flip_v: bool) -> String {
    let mut obj_text = String::with_capacity(mesh.vertices.len() * 96 + mesh.indices.len() * 8);
    let _ = writeln!(obj_text, "# Written by the Pill Engine Blender link");
    let _ = writeln!(
        obj_text,
        "o {}",
        mesh.name.replace(char::is_whitespace, "_")
    );
    for vertex in &mesh.vertices {
        let [x, y, z] = vertex.position;
        let _ = writeln!(obj_text, "v {x} {y} {z}");
    }
    for vertex in &mesh.vertices {
        let [u, v] = vertex.texture_coordinates;
        // The import flips V when its settings say so; write the value that
        // flips back to the mesh's own.
        let file_v = if flip_v { 1.0 - v } else { v };
        let _ = writeln!(obj_text, "vt {u} {file_v}");
    }
    for vertex in &mesh.vertices {
        let [x, y, z] = vertex.normal;
        let _ = writeln!(obj_text, "vn {x} {y} {z}");
    }
    for triangle in mesh.indices.chunks_exact(3) {
        // OBJ counts from one.
        let [a, b, c] = [triangle[0] + 1, triangle[1] + 1, triangle[2] + 1];
        let _ = writeln!(obj_text, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}");
    }
    obj_text
}

/// Writes `mesh` over its source file and returns the file's path.
///
/// # Errors
///
/// Returns a message when the source cannot be resolved (see
/// [`source_file_path`]) or the file cannot be written; a failed write leaves
/// the old file in place.
pub fn save_mesh_to_source(mesh: &Mesh, asset_name: &str) -> Result<PathBuf, String> {
    let source_path = source_file_path(asset_name)?;
    let obj_text = encode_obj(mesh, import_flips_v(&source_path));
    // Step 1: the whole file goes to a sibling first.
    let mut temporary_path = source_path.as_os_str().to_owned();
    temporary_path.push(".pill_link_tmp");
    let temporary_path = PathBuf::from(temporary_path);
    std::fs::write(&temporary_path, obj_text)
        .map_err(|error| format!("cannot write {}: {error}", temporary_path.display()))?;
    // Step 2: the rename replaces the source in one step.
    if let Err(error) = std::fs::rename(&temporary_path, &source_path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(format!("cannot replace {}: {error}", source_path.display()));
    }
    Ok(source_path)
}

// =============================================================================
// Tests
// =============================================================================

/// Runs `body` with a scratch folder mounted as the project's asset directory,
/// and leaves nothing mounted afterwards.
///
/// The mounted directory is process-global, so every test that needs one goes
/// through here: one lock and one counter keep two tests off each other's
/// folders.
#[cfg(test)]
pub(crate) fn in_scratch_asset_directory<Body>(body: Body)
where
    Body: FnOnce(&Path),
{
    static MOUNT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SCRATCH_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let _mounted = MOUNT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sequence = SCRATCH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "pill-blender-scratch-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("a scratch folder");
    pill_engine::asset_store::mount_directory(&root);
    body(&root);
    pill_engine::asset_store::mount_directory("");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The written file imports back to the same geometry, whichever way the
    /// import settings flip V.
    #[test]
    fn an_encoded_mesh_imports_back_to_the_same_geometry() {
        let mesh = Mesh::triangle();
        let flipped = Mesh::from_obj_bytes("t", encode_obj(&mesh, true).as_bytes()).unwrap();
        let top_down =
            Mesh::from_obj_bytes_top_down("t", encode_obj(&mesh, false).as_bytes()).unwrap();
        for imported in [flipped, top_down] {
            assert_eq!(imported.indices, mesh.indices);
            for (read_back, original) in imported.vertices.iter().zip(&mesh.vertices) {
                assert_eq!(read_back.position, original.position);
                assert_eq!(read_back.normal, original.normal);
                for axis in 0..2 {
                    let difference =
                        read_back.texture_coordinates[axis] - original.texture_coordinates[axis];
                    assert!(difference.abs() < 1.0e-6, "{difference}");
                }
            }
        }
    }

    #[test]
    fn a_name_that_climbs_out_of_res_is_refused() {
        assert!(source_file_path("../outside.obj").is_err());
    }

    #[test]
    fn a_missing_meta_means_the_import_flips_v() {
        assert!(import_flips_v(Path::new("does/not/exist.obj")));
    }

    /// A create writes the mesh and the metadata file beside it, imports the
    /// asset, and refuses a second create for the same name without touching
    /// the first file.
    #[test]
    fn a_create_writes_the_mesh_and_its_metadata() {
        in_scratch_asset_directory(|root| {
            let mut assets = AssetManager::new();
            let mesh = Mesh::triangle();

            let created = create_mesh_asset("models/teapot.obj", &mesh, &mut assets)
                .expect("the create writes and imports");
            assert_eq!(created.relative_path, PathBuf::from("models/teapot.obj"));
            assert!(created.absolute_path.is_file(), "the mesh file exists");
            assert!(
                root.join("models/teapot.obj.meta").is_file(),
                "the metadata file exists"
            );
            let imported = assets
                .get::<Mesh>(created.handle)
                .expect("the asset is live");
            assert_eq!(imported.vertices.len(), mesh.vertices.len());
            assert_eq!(imported.indices, mesh.indices);

            let refused = create_mesh_asset("models/teapot.obj", &mesh, &mut assets);
            assert!(refused.is_err(), "a second create is refused");
            assert!(
                root.join("models/teapot.obj").is_file(),
                "the first file is kept"
            );
        });
    }

    /// With no asset directory mounted there is nowhere to write, so the
    /// create refuses instead of inventing a path.
    #[test]
    fn a_create_without_a_mounted_directory_is_refused() {
        in_scratch_asset_directory(|_root| {
            pill_engine::asset_store::mount_directory("");
            let mut assets = AssetManager::new();
            let refused = create_mesh_asset("models/teapot.obj", &Mesh::triangle(), &mut assets);
            assert!(refused.is_err(), "no mounted directory means no create");
        });
    }

    /// A file the person picked resolves to the asset name the store uses, a
    /// file outside the project's `res` is refused, and the resolved asset
    /// imports on demand.
    #[test]
    fn a_picked_file_resolves_to_its_asset_name() {
        in_scratch_asset_directory(|root| {
            std::fs::create_dir_all(root.join("models")).expect("a models folder");
            let mesh_file = root.join("models/teapot.obj");
            std::fs::write(&mesh_file, encode_obj(&Mesh::triangle(), true)).expect("the mesh file");

            let resolved = asset_name_from_path(&mesh_file).expect("a file under res resolves");
            assert_eq!(resolved, "models/teapot.obj");

            let outside = std::env::temp_dir()
                .join(format!("pill-blender-outside-{}.obj", std::process::id()));
            std::fs::write(&outside, "").expect("a file outside res");
            assert!(
                asset_name_from_path(&outside).is_err(),
                "a file outside the project's res is refused"
            );
            let _ = std::fs::remove_file(&outside);

            let mut assets = AssetManager::new();
            let handle =
                load_mesh_asset("models/teapot.obj", &mut assets).expect("the file imports");
            assert!(
                assets.get::<Mesh>(handle).is_some(),
                "the resolved asset is live"
            );
        });
    }

    /// A file that does not exist yet resolves the same way as one that does,
    /// so the create picker can name a new file; a name outside `res` and a
    /// name that is not an `.obj` are refused.
    #[test]
    fn a_picked_destination_resolves_to_its_asset_name() {
        in_scratch_asset_directory(|root| {
            std::fs::create_dir_all(root.join("models/props")).expect("a models folder");

            let new_file = root.join("models/props/lamp.obj");
            let resolved = created_mesh_name(&new_file).expect("a new file under res resolves");
            assert_eq!(resolved, "models/props/lamp.obj");

            let outside = std::env::temp_dir().join("pill-blender-outside-destination.obj");
            assert!(
                created_mesh_name(&outside).is_err(),
                "a destination outside the project's res is refused"
            );

            let not_obj = root.join("models/props/lamp.fbx");
            assert!(
                created_mesh_name(&not_obj).is_err(),
                "a destination that is not an .obj is refused"
            );

            let missing_folder = root.join("missing/lamp.obj");
            assert!(
                created_mesh_name(&missing_folder).is_err(),
                "a folder that does not exist is refused"
            );
        });
    }
}
