//! Development shader reload, data half: writes a re-cooked WGSL file into the
//! shader assets built from it.
//!
//! # Responsibilities
//!
//! - Map one re-cooked WGSL file back to the shader assets built from it, using
//!   the table each pipeline publishes ([`all_shader_sources`]).
//! - Write the new text into those assets. That raises their content version,
//!   so the GPU module rebuilds only those shaders on its next sync.
//! - Offer that as `pill_render_data_shader_changed`, a named C-ABI export the
//!   development host calls.
//!
//! # Design
//!
//! Watching and cooking belong to the host: a worker thread there cooks the
//! crate's HLSL sources with `pill_assets::cook_shader_tree`, the routine this
//! crate's build script also runs, and passes each re-cooked file to the main
//! thread. That thread calls this export on whichever data module generation
//! is current at that moment. The data crate uses neither `notify` nor the
//! cooker at run time, so reloading it cannot leave a watcher thread running
//! code from a retired image.
//!
//! A shader edit is an asset edit. The renderer's pipelines keep their shaders
//! as `Shader` assets, and the GPU module rebuilds any asset whose content
//! version went up. Nothing here touches a device, a pipeline or a bind group.
//! An asset is rewritten only when the new text differs from what it holds, so
//! a save that changes nothing costs no GPU work.
//!
//! This module is compiled into native debug builds and the tests. A shipping
//! build has no sources to watch and no host to deliver edits.

// Standard library
use std::slice;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{AssetManager, World};

// Current crate
use crate::assets::Shader;
use crate::config::{all_shader_sources, ShaderSourceRecord};

// =============================================================================
// Constants
// =============================================================================

/// What [`pill_render_data_shader_changed`] returns when it cannot reach the
/// shader assets at all.
///
/// That is a null world, a world without an `AssetManager`, a null buffer with
/// a non-zero length, or a path or text that is not UTF-8.
pub const SHADER_CHANGE_REFUSED: u32 = u32::MAX;

// =============================================================================
// Free Functions
// =============================================================================

/// Replaces one stage's WGSL in every recorded shader asset built from
/// `relative_path` and returns the names of the assets whose text changed.
///
/// `relative_path` is relative to the crate's `shader_sources/` and uses `/`
/// separators, which is how the records spell it. A record whose asset is not
/// in the store is skipped, which happens for a pipeline this project never
/// installed (the opt-in simple pipeline, for example).
pub fn apply_shader_source<'a>(
    assets: &mut AssetManager,
    relative_path: &str,
    wgsl: &str,
    shader_source_records: impl IntoIterator<Item = &'a ShaderSourceRecord>,
) -> Vec<&'static str> {
    let mut changed_shader_asset_names = Vec::new();
    for record in shader_source_records {
        let feeds_vertex = record.vertex_path == relative_path;
        let feeds_fragment = record.fragment_path == relative_path;
        if !feeds_vertex && !feeds_fragment {
            continue;
        }
        let Some(stored_shader) = assets.get_by_name::<Shader>(record.shader_asset_name) else {
            continue;
        };
        let unchanged = (!feeds_vertex || stored_shader.vertex_wgsl == wgsl)
            && (!feeds_fragment || stored_shader.fragment_wgsl == wgsl);
        if unchanged {
            continue;
        }
        // Borrowing the asset mutably is what raises its content version, so
        // the borrow is taken only for a shader whose text changed.
        if let Some(shader) = assets.get_by_name_mut::<Shader>(record.shader_asset_name) {
            if feeds_vertex {
                shader.vertex_wgsl = wgsl.to_owned();
            }
            if feeds_fragment {
                shader.fragment_wgsl = wgsl.to_owned();
            }
            changed_shader_asset_names.push(record.shader_asset_name);
        }
    }
    changed_shader_asset_names
}

/// Writes one re-cooked WGSL file into the shader assets built from it, in
/// `world`'s `AssetManager`.
///
/// `path` is the cooked file's path relative to the crate's `shader_sources/`,
/// with `/` separators (for example
/// `pbr_pipeline/shaders/pbr_fragment.wgsl`), and `wgsl` is its text. The
/// return value is the number of shader assets that changed, or
/// [`SHADER_CHANGE_REFUSED`].
///
/// # Safety
///
/// `world` must be null or point at a live `World` that nothing else uses for
/// the duration of the call. `path` and `wgsl` must each point at their
/// declared number of readable bytes, unless that length is zero.
#[no_mangle]
pub unsafe extern "C" fn pill_render_data_shader_changed(
    world: *mut World,
    path: *const u8,
    path_len: usize,
    wgsl: *const u8,
    wgsl_len: usize,
) -> u32 {
    // Step 1: Read both buffers as UTF-8 text.
    // SAFETY: the caller guarantees that each buffer is readable for the length
    // it passes, which is what `read_text` requires.
    let (Some(path), Some(wgsl)) = (unsafe { read_text(path, path_len) }, unsafe {
        read_text(wgsl, wgsl_len)
    }) else {
        return SHADER_CHANGE_REFUSED;
    };
    // Step 2: Find the asset store the world holds.
    // SAFETY: the caller guarantees `world` is null or a live world nothing
    // else uses.
    let Some(world) = (unsafe { world.as_mut() }) else {
        return SHADER_CHANGE_REFUSED;
    };
    let Some(assets) = world.get_resource_mut::<AssetManager>() else {
        return SHADER_CHANGE_REFUSED;
    };
    // Step 3: Write the text into the assets built from it.
    let changed_shader_asset_names = apply_shader_source(assets, path, wgsl, all_shader_sources());
    pill_core::info!(
        target: telemetry_target::HOT_RELOAD,
        path,
        shaders = %changed_shader_asset_names.join(", "),
        "shader source applied"
    );
    changed_shader_asset_names.len() as u32
}

/// Borrows `length` bytes at `pointer` as UTF-8 text.
///
/// Returns `None` when those bytes are not valid UTF-8, or when `pointer` is
/// null and `length` is not zero. A zero length always yields empty text.
///
/// # Safety
///
/// `pointer` must point at `length` readable bytes, unless `length` is zero.
/// The returned text borrows those bytes, so whoever holds it must keep them
/// alive and unmodified for as long as it is used.
unsafe fn read_text<'a>(pointer: *const u8, length: usize) -> Option<&'a str> {
    if length == 0 {
        return Some("");
    }
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees `length` readable bytes at `pointer`.
    let bytes = unsafe { slice::from_raw_parts(pointer, length) };
    std::str::from_utf8(bytes).ok()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A record for one vertex and one fragment file, stored as `test.shader`.
    const RECORD: ShaderSourceRecord = ShaderSourceRecord {
        shader_asset_name: "test.shader",
        vertex_path: "test/test_vertex.wgsl",
        fragment_path: "test/test_fragment.wgsl",
    };

    /// The vertex text the test shader is built from.
    const RECORD_VERTEX: &str = "vertex one";
    /// The fragment text the test shader is built from.
    const RECORD_FRAGMENT: &str = "fragment one";

    /// A store holding `test.shader`, built from the two texts above.
    fn store_with_shader() -> (AssetManager, pill_engine::Handle<Shader>) {
        let mut assets = AssetManager::new();
        let shader = Shader::new("test")
            .with_wgsl(RECORD_VERTEX, RECORD_FRAGMENT)
            .build()
            .expect("both stages set");
        let handle = assets
            .add_named(RECORD.shader_asset_name, shader)
            .expect("a free name");
        (assets, handle)
    }

    #[test]
    fn an_edited_stage_replaces_its_text_and_moves_the_version() {
        let (mut assets, handle) = store_with_shader();
        let before = assets.content_version(handle);

        let changed_shader_asset_names = apply_shader_source(
            &mut assets,
            "test/test_fragment.wgsl",
            "fragment two",
            [&RECORD],
        );

        assert_eq!(changed_shader_asset_names, ["test.shader"]);
        let shader = assets.get(handle).expect("still live");
        assert_eq!(shader.fragment_wgsl, "fragment two");
        assert_eq!(shader.vertex_wgsl, RECORD_VERTEX);
        assert_ne!(assets.content_version(handle), before);
    }

    #[test]
    fn unchanged_text_leaves_the_version_alone() {
        let (mut assets, handle) = store_with_shader();
        let before = assets.content_version(handle);

        let changed_shader_asset_names = apply_shader_source(
            &mut assets,
            "test/test_vertex.wgsl",
            RECORD_VERTEX,
            [&RECORD],
        );

        assert!(changed_shader_asset_names.is_empty());
        assert_eq!(assets.content_version(handle), before);
    }

    #[test]
    fn a_path_no_record_names_changes_nothing() {
        let (mut assets, handle) = store_with_shader();
        let before = assets.content_version(handle);

        let changed_shader_asset_names =
            apply_shader_source(&mut assets, "other/other.wgsl", "text", [&RECORD]);

        assert!(changed_shader_asset_names.is_empty());
        assert_eq!(assets.content_version(handle), before);
    }

    #[test]
    fn a_shader_the_store_does_not_hold_is_skipped() {
        let mut assets = AssetManager::new();

        assert!(
            apply_shader_source(&mut assets, "test/test_vertex.wgsl", "text", [&RECORD]).is_empty()
        );
    }

    #[test]
    fn the_export_writes_through_a_world() {
        // One of the crate's own records, so the export's table finds it.
        let record = all_shader_sources()
            .next()
            .expect("the crate records shaders");
        let mut assets = AssetManager::new();
        let shader = Shader::new("recorded")
            .with_wgsl("vertex one", "fragment one")
            .build()
            .expect("both stages set");
        let handle = assets
            .add_named(record.shader_asset_name, shader)
            .expect("a free name");
        let mut world = World::new();
        world.insert_resource(assets);
        let path = record.fragment_path;
        let text = "edited fragment";

        // SAFETY: a live, unshared world and two valid UTF-8 buffers.
        let changed = unsafe {
            pill_render_data_shader_changed(
                &mut world,
                path.as_ptr(),
                path.len(),
                text.as_ptr(),
                text.len(),
            )
        };

        assert_eq!(changed, 1);
        let assets = world.get_resource::<AssetManager>().expect("inserted");
        let shader = assets.get(handle).expect("still live");
        assert_eq!(shader.fragment_wgsl, text);
        assert_eq!(shader.vertex_wgsl, "vertex one");
    }

    #[test]
    fn the_export_refuses_a_null_world_and_non_utf8_text() {
        let path = "a.wgsl";
        let invalid = [0xff_u8, 0xfe];

        // SAFETY: a null world is part of the contract; the buffers are valid.
        let null_world = unsafe {
            pill_render_data_shader_changed(
                std::ptr::null_mut(),
                path.as_ptr(),
                path.len(),
                path.as_ptr(),
                path.len(),
            )
        };
        let mut world = World::new();
        // SAFETY: a live world; `invalid` is two readable bytes.
        let bad_text = unsafe {
            pill_render_data_shader_changed(
                &mut world,
                path.as_ptr(),
                path.len(),
                invalid.as_ptr(),
                invalid.len(),
            )
        };

        assert_eq!(null_world, SHADER_CHANGE_REFUSED);
        assert_eq!(bad_text, SHADER_CHANGE_REFUSED);
    }
}
