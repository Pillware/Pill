//! The GPU objects behind the asset store, kept up to date with it one asset
//! type at a time.
//!
//! # Responsibilities
//!
//! - Hold one GPU object per live asset - shader, texture, mesh, material - and
//!   the content version it was built from.
//! - Bring those objects up to date when the store's revision moves, and only
//!   the ones that moved: an edited texture is uploaded again while its
//!   neighbours keep the objects they already have, and a removed asset frees
//!   its slot.
//! - Own the resource epoch, the counter that increases whenever a shader or
//!   texture object changes. The chain reads it to learn that its bind groups
//!   are out of date.
//! - Resolve a frame's instances to the GPU handles the draw path needs, and
//!   hold the default shader and material that an asset naming none falls back
//!   to.
//!
//! # Design
//!
//! This mirrors the store rather than caching it: `AssetManager` looks assets
//! up by handle, while this keys its GPU objects by asset key. A key alone does
//! not say whether the asset behind it is still live, so every sync walks the
//! store's column for its own asset type.
//!
//! A failure is per asset and never fatal: a texture that will not upload, a
//! shader that will not compile or a material whose pipeline will not build is
//! named in the log and skipped. Its version is left unrecorded, so the next
//! store revision tries it again.

// Standard library
use std::collections::{HashMap, HashSet};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{AssetManager, Handle};

// Current crate
use crate::{
    assets::{
        asset_key, slots_by_name, Material, MaterialParameter, Mesh, Shader, ShaderParameterSlot,
        ShaderParameterType, ShaderTextureKind, ShaderTextureSlot, Texture,
    },
    chain::ChainContext,
    error::Result,
    frame::RenderFrame,
    render_queue::{compose_render_queue_key, RenderQueueItem},
    renderer::State,
    resources::{
        RendererMaterial, RendererMaterialHandle, RendererMesh, RendererMeshHandle, RendererShader,
        RendererShaderHandle, RendererTexture, RendererTextureHandle, UniformGroups, Vertex,
    },
    Instance,
};

// =============================================================================
// GpuColumn
// =============================================================================

/// One asset type's GPU objects, keyed by asset key.
///
/// An entry is the handle of the object on the GPU plus the content version it
/// was built from, which leaves the sync four cases per key: not built yet,
/// built from the current version, built from an older version, and built but
/// marked stale. The last one keeps the handle and forgets the version, so the
/// next sync rebuilds the object even though the asset's own content did not
/// change - which is what a material asks for when its shader or texture moved.
pub(crate) struct GpuColumn<H> {
    entries: HashMap<u64, (H, Option<u64>)>,
}

impl<H: Copy> GpuColumn<H> {
    /// Creates a column that holds no objects yet.
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// The handle of the object behind `key`, when one is there.
    pub(crate) fn handle(&self, key: u64) -> Option<H> {
        self.entries.get(&key).map(|(handle, _)| *handle)
    }

    /// Returns whether `key`'s object was built from content version
    /// `version`.
    fn is_current(&self, key: u64, version: u64) -> bool {
        matches!(self.entries.get(&key), Some((_, Some(built))) if *built == version)
    }

    /// Records `handle` as `key`'s object, built from `version`.
    fn insert(&mut self, key: u64, handle: H, version: u64) {
        self.entries.insert(key, (handle, Some(version)));
    }

    /// Removes `key`'s entry and hands back its handle, for the rebuild that
    /// replaces or drops the object.
    fn remove(&mut self, key: u64) -> Option<H> {
        self.entries.remove(&key).map(|(handle, _)| handle)
    }

    /// Keeps `key`'s handle but forgets the version, so the next sync rebuilds
    /// it from the assets it depends on as they are now.
    fn invalidate(&mut self, key: u64) {
        if let Some((_, version)) = self.entries.get_mut(&key) {
            *version = None;
        }
    }

    /// Returns every key the column holds.
    fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.entries.keys().copied()
    }

    /// Forgets every object's version and keeps the handles, so the next sync
    /// rebuilds all of them.
    fn invalidate_all(&mut self) {
        for (_, version) in self.entries.values_mut() {
            *version = None;
        }
    }
}

// =============================================================================
// RenderingResourcesManager
// =============================================================================

/// The GPU objects behind the asset store, and what they were built from.
///
/// The four columns below hold, per asset type, the content version each GPU
/// object was built from. A key that is not recorded, or whose version moved,
/// is rebuilt; a key whose version matches keeps its object.
pub(crate) struct RenderingResourcesManager {
    /// The store revision these caches are up to date with, or `None` after an
    /// explicit invalidation. The sync runs when it does not match the store's
    /// revision.
    synced_revision: Option<u64>,
    /// Shader objects by asset key.
    shaders: GpuColumn<RendererShaderHandle>,
    /// Material objects by asset key.
    materials: GpuColumn<RendererMaterialHandle>,
    /// Texture objects by asset key.
    textures: GpuColumn<RendererTextureHandle>,
    /// Mesh objects by asset key.
    meshes: GpuColumn<RendererMeshHandle>,
    /// The shader an instance draws with when its material names none, names
    /// one that is not loaded, or is not in the resource storage.
    default_shader: RendererShaderHandle,
    /// The material an instance draws with when its material never built.
    default_material: RendererMaterialHandle,
    /// Generation counter for the shader and texture objects, increased by one
    /// in each sync that created, recreated or dropped one. Pass bind groups
    /// reference those objects by handle, so such a change is what puts them
    /// out of date.
    resource_epoch: u64,
    /// Scratch: the keys the store still holds for the column being walked.
    ///
    /// One set serves all four columns, because the walks run one after
    /// another and each clears it first.
    live_asset_keys: HashSet<u64>,
    /// Scratch: the keys whose object this sync rebuilt or removed. Read once,
    /// by `invalidate_dependent_materials`, and meaningless outside the sync
    /// that filled it.
    changed_asset_keys: HashSet<u64>,
}

impl RenderingResourcesManager {
    /// Creates a manager holding no GPU objects, plus the two objects the
    /// fallbacks need.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`](crate::RendererError) when the built-in lit
    /// shader or the material built from it will not build. Both defaults are
    /// built here and every later fallback depends on them, so this is the one
    /// creation failure that stops the renderer from starting.
    pub(crate) fn new(state: &mut State) -> Result<Self> {
        let (default_shader, default_material) = install_default_material(state)?;
        Ok(Self {
            synced_revision: None,
            shaders: GpuColumn::new(),
            materials: GpuColumn::new(),
            textures: GpuColumn::new(),
            meshes: GpuColumn::new(),
            default_shader,
            default_material,
            resource_epoch: 0,
            live_asset_keys: HashSet::new(),
            changed_asset_keys: HashSet::new(),
        })
    }

    /// Brings the GPU objects up to date with the asset store, one asset at a
    /// time.
    ///
    /// The store's revision decides whether this pass runs at all. Each
    /// asset's content version then decides whether its object is rebuilt, so
    /// one edited texture is uploaded again while its neighbours keep their
    /// GPU objects and a removed asset frees its slot. A creation failure is
    /// logged and skipped instead of ending the pass, so one bad asset neither
    /// blocks the rest of the revision nor is retried every frame.
    ///
    /// The scratch sets are fields rather than locals, because this runs on
    /// every frame in which the store moved and a game that edits assets moves
    /// it often.
    pub(crate) fn sync(&mut self, assets: &AssetManager, state: &mut State) {
        if self.synced_revision == Some(assets.revision()) {
            return;
        }
        self.synced_revision = Some(assets.revision());

        // Step 1: Textures and shaders first, because materials bind them by
        // handle. Only these two move the epoch: they are the objects a pass's
        // bind groups keep.
        self.changed_asset_keys.clear();
        let mut epoch_moved = self.sync_textures(assets, state);
        epoch_moved |= self.sync_shaders(assets, state);
        // Step 2: Meshes. The draw path resolves a mesh handle per frame, so a
        // rebuilt mesh needs no epoch bump.
        self.sync_meshes(assets, state);
        // Step 3: Rebuild the materials built against an object that just
        // changed. Their own content version alone would not ask for that, so
        // their version record is dropped first; then the materials whose own
        // content moved.
        if !self.changed_asset_keys.is_empty() {
            self.invalidate_dependent_materials(assets);
        }
        self.sync_materials(assets, state);
        // Step 4: Increase the epoch once, if a shader or texture object moved.
        if epoch_moved {
            self.resource_epoch = self.resource_epoch.wrapping_add(1);
        }
    }

    /// Forgets which versions the objects were built from, so the next sync
    /// rebuilds all of them.
    ///
    /// The objects themselves stay in place until that rebuild replaces them:
    /// the sync removes a key's old object when it creates the new one, so
    /// clearing the records is enough to get the whole store onto the GPU
    /// again.
    pub(crate) fn invalidate(&mut self) {
        self.synced_revision = None;
        self.textures.invalidate_all();
        self.shaders.invalidate_all();
        self.meshes.invalidate_all();
        self.materials.invalidate_all();
    }

    /// Builds one queue item per drawable instance, sorted so that draws
    /// sharing a key are recorded as one instanced draw.
    ///
    /// An instance whose mesh never uploaded is skipped. One whose material
    /// never built draws with the default material, and a material the
    /// resource storage does not hold draws with the default shader. A partly
    /// broken revision therefore still draws something, which is what the two
    /// defaults are for.
    pub(crate) fn build_queue(&self, frame: &RenderFrame, state: &State) -> Vec<RenderQueueItem> {
        let resolve_zone = pill_core::profile_scope!("build queue: resolve instances");
        let mut queue = Vec::with_capacity(frame.instances.len());
        // The mesh and material keys resolved last, with the handles they
        // resolved to. Instances that share both are usually many and next to
        // each other (one archetype's worth), so keeping one pair skips nearly
        // every lookup - and those lookups take most of this function's time
        // once thousands of instances share one mesh and material. Nothing
        // changes the caches during the call, so a remembered answer is the
        // one a fresh lookup would give.
        let mut last_resolved_handles = None;
        for (index, instance) in frame.instances.iter().enumerate() {
            let resolved_handles = match last_resolved_handles {
                Some((mesh_key, material_key, resolved_handles))
                    if mesh_key == instance.mesh && material_key == instance.material =>
                {
                    resolved_handles
                }
                _ => {
                    // The renderer's own caches key by the raw value, while the
                    // frame names it as a typed key.
                    let resolved_handles =
                        self.resolve_draw_handles(instance.mesh.0, instance.material.0, state);
                    last_resolved_handles =
                        Some((instance.mesh, instance.material, resolved_handles));
                    resolved_handles
                }
            };
            let Some((shader_handle, material_handle, mesh_handle)) = resolved_handles else {
                continue;
            };
            queue.push(RenderQueueItem {
                key: compose_render_queue_key(
                    instance.rendering_order,
                    shader_handle,
                    material_handle,
                    mesh_handle,
                ),
                frame_instance_index: index as u32,
                shader: shader_handle,
                material: material_handle,
                mesh: mesh_handle,
            });
        }
        drop(resolve_zone);
        let _sort_zone = pill_core::profile_scope!("build queue: sort");
        queue.sort_unstable();
        queue
    }

    /// Returns the shader, material and mesh handles one instance draws with,
    /// or `None` when its mesh never uploaded.
    ///
    /// A material that never built falls back to the default material, and a
    /// material the resource storage does not hold to the default shader.
    fn resolve_draw_handles(
        &self,
        mesh_key: u64,
        material_key: u64,
        state: &State,
    ) -> Option<(
        RendererShaderHandle,
        RendererMaterialHandle,
        RendererMeshHandle,
    )> {
        let mesh_handle = self.meshes.handle(mesh_key)?;
        let material_handle = self
            .materials
            .handle(material_key)
            .unwrap_or(self.default_material);
        let shader_handle = state
            .renderer_resource_storage
            .materials
            .get(material_handle)
            .map(|material| material.shader_handle)
            .unwrap_or(self.default_shader);
        Some((shader_handle, material_handle, mesh_handle))
    }

    /// Returns the shader column, which the chain reads to find the pipeline
    /// behind a pass's shader.
    pub(crate) fn shader_handles(&self) -> &GpuColumn<RendererShaderHandle> {
        &self.shaders
    }

    /// Returns what the chain needs from these caches to build its GPU
    /// objects.
    ///
    /// The chain reads the shader and texture columns by key, together with
    /// the epoch that moves when one of them is rebuilt. Passing them together
    /// keeps the borrow split in one place instead of at every call site. The
    /// surface arrives as the pieces the chain reads - its format, its size,
    /// its depth buffer and its offscreen targets - rather than as the whole
    /// target, so the chain those pieces belong to can be rebuilt at the same
    /// time.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn chain_context<'a>(
        &'a self,
        state: &'a mut State,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
        depth_texture: &'a RendererTexture,
        offscreen: &'a mut HashMap<String, RendererTexture>,
    ) -> ChainContext<'a> {
        ChainContext {
            state,
            surface_format,
            size,
            depth_texture,
            offscreen,
            shader_handles: &self.shaders,
            texture_handles: &self.textures,
            resource_epoch: self.resource_epoch,
        }
    }

    /// Forgets the version of every material that reads an asset this sync
    /// rebuilt.
    ///
    /// A material's bind groups are built from the layouts and views its
    /// shader and textures had at the time. When one of those is rebuilt or
    /// removed, the bind groups are out of date, yet the material's own
    /// content version does not change. Forgetting the version makes
    /// `sync_materials` rebuild the material in the same pass, against the new
    /// object.
    fn invalidate_dependent_materials(&mut self, assets: &AssetManager) {
        for (handle, material) in assets.iter_handles::<Material>() {
            let shader_changed = self
                .changed_asset_keys
                .contains(&asset_key(material.shader));
            let texture_changed = material.textures.values().any(|texture| {
                self.changed_asset_keys
                    .contains(&asset_key(texture.texture))
            });
            if shader_changed || texture_changed {
                self.materials.invalidate(asset_key(handle));
            }
        }
    }

    /// Drops and rebuilds the textures whose asset changed.
    ///
    /// Returns whether any texture object was created, recreated or dropped.
    /// Keys whose object moved are added to `changed_asset_keys`, so the
    /// materials built against them can be rebuilt too.
    fn sync_textures(&mut self, assets: &AssetManager, state: &mut State) -> bool {
        let mut epoch_moved = false;
        // The keys the store still holds, collected while the rebuild loop
        // walks it. These caches key by asset key and `AssetManager` looks
        // assets up by handle, so walking the store is the only way to tell
        // whether the asset behind a cached key is still live.
        self.live_asset_keys.clear();

        for (handle, texture) in assets.iter_handles::<Texture>() {
            let key = asset_key(handle);
            self.live_asset_keys.insert(key);
            let version = assets.content_version(handle).unwrap_or(0);
            if self.textures.is_current(key, version) {
                continue;
            }
            // Recorded before the rebuild, because a failed creation has still
            // dropped the texture the old materials were built against.
            self.changed_asset_keys.insert(key);
            if let Some(old_handle) = self.textures.remove(key) {
                state.renderer_resource_storage.textures.remove(old_handle);
            }
            match RendererTexture::new_texture(
                &state.device,
                &state.queue,
                Some(&texture.name),
                &texture.rgba,
                texture.width,
                texture.height,
                texture.texture_type,
            ) {
                Ok(texture_object) => {
                    let gpu_handle = state
                        .renderer_resource_storage
                        .textures
                        .insert(texture_object);
                    self.textures.insert(key, gpu_handle, version);
                    epoch_moved = true;
                }
                // The key stays unrecorded, so a later revision tries again,
                // and the warning names the asset that is not on the GPU.
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "texture `{}` is not uploaded: {error}",
                    texture.name
                ),
            }
        }

        // A key the store no longer holds belongs to a removed asset, so its
        // object goes with it.
        let removed_keys: Vec<u64> = self
            .textures
            .keys()
            .filter(|key| !self.live_asset_keys.contains(key))
            .collect();
        for key in removed_keys {
            if let Some(gpu_handle) = self.textures.remove(key) {
                state.renderer_resource_storage.textures.remove(gpu_handle);
                self.changed_asset_keys.insert(key);
                epoch_moved = true;
            }
        }
        epoch_moved
    }

    /// Drops and rebuilds the shaders whose asset changed.
    ///
    /// Returns whether any shader object was created, recreated or dropped.
    /// Keys whose object moved are added to `changed_asset_keys`, so the
    /// materials built against them can be rebuilt too.
    fn sync_shaders(&mut self, assets: &AssetManager, state: &mut State) -> bool {
        let mut epoch_moved = false;
        // See `sync_textures`: walking the store is how this learns which
        // cached keys are still live.
        self.live_asset_keys.clear();

        for (handle, shader) in assets.iter_handles::<Shader>() {
            let key = asset_key(handle);
            self.live_asset_keys.insert(key);
            let version = assets.content_version(handle).unwrap_or(0);
            if self.shaders.is_current(key, version) {
                continue;
            }
            self.changed_asset_keys.insert(key);
            // An object already in the cache makes this a rebuild rather than
            // the asset's first compile, which is what the log line below
            // reports.
            let previous_handle = self.shaders.remove(key);
            let shader_recompiled = previous_handle.is_some();
            if let Some(old_handle) = previous_handle {
                state.renderer_resource_storage.shaders.remove(old_handle);
            }
            let compiled_shader = RendererShader::new(
                &shader.name,
                &state.device,
                state.surface_format,
                Some(state.depth_format),
                &[
                    RendererMesh::data_layout_descriptor(),
                    Instance::data_layout_descriptor(),
                ],
                &shader.vertex_wgsl,
                &shader.fragment_wgsl,
                &shader.parameter_slots,
                &shader.texture_slots,
                &state
                    .renderer_resource_storage
                    .engine_parameters
                    .bind_group_layout,
                &state.camera_bind_group_layout,
                UniformGroups {
                    engine: shader.pass_engine_parameters,
                    camera: shader.pass_camera_parameters,
                },
            );
            match compiled_shader {
                Ok(shader_object) => {
                    let gpu_handle = state
                        .renderer_resource_storage
                        .shaders
                        .insert(shader_object);
                    self.shaders.insert(key, gpu_handle, version);
                    if shader_recompiled {
                        pill_core::info!(
                            target: telemetry_target::RENDERING,
                            shader = shader.name.as_str(),
                            version,
                            "shader recompiled"
                        );
                    }
                    epoch_moved = true;
                }
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "shader `{}` is not compiled: {error}",
                    shader.name
                ),
            }
        }

        let removed_keys: Vec<u64> = self
            .shaders
            .keys()
            .filter(|key| !self.live_asset_keys.contains(key))
            .collect();
        for key in removed_keys {
            if let Some(gpu_handle) = self.shaders.remove(key) {
                state.renderer_resource_storage.shaders.remove(gpu_handle);
                self.changed_asset_keys.insert(key);
                epoch_moved = true;
            }
        }
        epoch_moved
    }

    /// Drops and rebuilds the meshes whose asset changed.
    ///
    /// Meshes need no epoch increase. The draw path resolves a mesh handle per
    /// frame, so a rebuilt mesh is picked up without any pipeline going out of
    /// date.
    fn sync_meshes(&mut self, assets: &AssetManager, state: &mut State) {
        self.live_asset_keys.clear();

        for (handle, mesh) in assets.iter_handles::<Mesh>() {
            let key = asset_key(handle);
            self.live_asset_keys.insert(key);
            let version = assets.content_version(handle).unwrap_or(0);
            if self.meshes.is_current(key, version) {
                continue;
            }
            if let Some(old_handle) = self.meshes.remove(key) {
                state.renderer_resource_storage.meshes.remove(old_handle);
            }
            match RendererMesh::new(&state.device, &mesh.name, mesh) {
                Ok(mesh_object) => {
                    let gpu_handle = state.renderer_resource_storage.meshes.insert(mesh_object);
                    self.meshes.insert(key, gpu_handle, version);
                }
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "mesh `{}` is not on the GPU: {error}",
                    mesh.name
                ),
            }
        }

        let removed_keys: Vec<u64> = self
            .meshes
            .keys()
            .filter(|key| !self.live_asset_keys.contains(key))
            .collect();
        for key in removed_keys {
            if let Some(gpu_handle) = self.meshes.remove(key) {
                state.renderer_resource_storage.meshes.remove(gpu_handle);
            }
        }
    }

    /// Drops and rebuilds the materials whose asset changed.
    fn sync_materials(&mut self, assets: &AssetManager, state: &mut State) {
        self.live_asset_keys.clear();

        for (handle, material) in assets.iter_handles::<Material>() {
            let key = asset_key(handle);
            self.live_asset_keys.insert(key);
            let version = assets.content_version(handle).unwrap_or(0);
            if self.materials.is_current(key, version) {
                continue;
            }
            if let Some(old_handle) = self.materials.remove(key) {
                state.renderer_resource_storage.materials.remove(old_handle);
            }
            let shader_key = asset_key(material.shader);
            let shader_handle = if material.shader == Handle::INVALID {
                // A material with no shader is documented to draw with the
                // renderer's built-in one, so this is a choice, not a fault.
                self.default_shader
            } else if let Some(shader_handle) = self.shaders.handle(shader_key) {
                shader_handle
            } else {
                pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "material `{}` names a shader that is not loaded; drawing with the default shader",
                    material.name
                );
                self.default_shader
            };
            let textures = material
                .textures
                .iter()
                .filter_map(|(slot, texture)| {
                    self.textures
                        .handle(asset_key(texture.texture))
                        .map(|gpu_handle| (slot.clone(), gpu_handle))
                })
                .collect::<Vec<_>>();
            match RendererMaterial::new(
                &state.device,
                &state.queue,
                &state.renderer_resource_storage,
                &material.name,
                shader_handle,
                &textures,
                &material.parameters,
            ) {
                Ok(material_object) => {
                    let gpu_handle = state
                        .renderer_resource_storage
                        .materials
                        .insert(material_object);
                    self.materials.insert(key, gpu_handle, version);
                }
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "material `{}` has no pipeline: {error}",
                    material.name
                ),
            }
        }

        let removed_keys: Vec<u64> = self
            .materials
            .keys()
            .filter(|key| !self.live_asset_keys.contains(key))
            .collect();
        for key in removed_keys {
            if let Some(gpu_handle) = self.materials.remove(key) {
                state.renderer_resource_storage.materials.remove(gpu_handle);
            }
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the built-in lit shader and a material from it, and returns both.
///
/// The material is what an instance draws with when its own material never
/// built. The shader is what a material draws with when it names none or names
/// one that is not loaded. The two are built together and kept as one pair.
///
/// The WGSL is the shipped pipeline's own text rather than a copy: the vertex
/// stage comes from `shader_sources/common_shaders/default_vertex.wgsl` and the
/// fragment from `shader_sources/simple_pipeline/shaders/default_lit_fragment.wgsl`.
/// `simple_pipeline` names the same shader with the same declared slots, so a
/// material written for this fallback draws there without being rebuilt.
///
/// # Errors
///
/// Returns a [`RendererError`](crate::RendererError) when wgpu refuses the
/// built-in shader's pipeline or the material's bind groups. Both point at this
/// fallback rather than at a project's own asset.
fn install_default_material(
    state: &mut State,
) -> Result<(RendererShaderHandle, RendererMaterialHandle)> {
    // Step 1: Build the shader from the two embedded stages and store it.
    let parameter_slots = slots_by_name(
        "pill_default_lit",
        "parameter",
        [
            ShaderParameterSlot::new("tint", ShaderParameterType::Color),
            ShaderParameterSlot::new("specularity", ShaderParameterType::Scalar),
        ],
    );
    let texture_slots = slots_by_name(
        "pill_default_lit",
        "texture",
        [
            ShaderTextureSlot::new("color", ShaderTextureKind::Color, (0, 1)),
            ShaderTextureSlot::new("normal", ShaderTextureKind::Normal, (2, 3)),
        ],
    );
    let shader = RendererShader::new(
        "pill_default_lit",
        &state.device,
        state.surface_format,
        Some(state.depth_format),
        &[
            RendererMesh::data_layout_descriptor(),
            Instance::data_layout_descriptor(),
        ],
        crate::config::DEFAULT_VERTEX.embedded_source,
        crate::config::simple_pipeline::DEFAULT_LIT_FRAGMENT.embedded_source,
        &parameter_slots,
        &texture_slots,
        &state
            .renderer_resource_storage
            .engine_parameters
            .bind_group_layout,
        &state.camera_bind_group_layout,
        UniformGroups {
            engine: true,
            camera: true,
        },
    )?;
    let shader_handle = state.renderer_resource_storage.shaders.insert(shader);
    // Step 2: Build a material from the shader, with one value for each
    // declared parameter slot, and store it.
    let parameters = std::collections::BTreeMap::from([
        ("tint".to_owned(), MaterialParameter::Color([1.0; 3])),
        ("specularity".to_owned(), MaterialParameter::Scalar(0.5)),
    ]);
    let material = RendererMaterial::new(
        &state.device,
        &state.queue,
        &state.renderer_resource_storage,
        "pill_default_lit",
        shader_handle,
        &[],
        &parameters,
    )?;
    let material_handle = state.renderer_resource_storage.materials.insert(material);
    Ok((shader_handle, material_handle))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A material whose shader or texture moved keeps its handle but loses its
    /// version, so the next sync rebuilds it against the new object.
    #[test]
    fn forgetting_a_version_keeps_the_handle_and_asks_for_a_rebuild() {
        let mut column: GpuColumn<u32> = GpuColumn::new();
        column.insert(7, 42, 3);
        assert!(column.is_current(7, 3));

        column.invalidate(7);

        assert_eq!(column.handle(7), Some(42), "the handle stays");
        assert!(!column.is_current(7, 3), "and the next sync rebuilds it");
    }

    /// A column hands back the object it drops, which is what lets the sync
    /// remove it from the resource storage.
    #[test]
    fn removing_an_entry_hands_back_its_handle() {
        let mut column: GpuColumn<u32> = GpuColumn::new();
        column.insert(1, 10, 0);

        assert_eq!(column.remove(1), Some(10));
        assert_eq!(column.handle(1), None);
        assert_eq!(column.remove(1), None, "removing twice is harmless");
    }

    /// Versions are compared per key, so one asset moving leaves its
    /// neighbours' objects alone.
    #[test]
    fn a_moved_version_only_touches_its_own_key() {
        let mut column: GpuColumn<u32> = GpuColumn::new();
        column.insert(1, 10, 0);
        column.insert(2, 20, 0);

        column.insert(1, 11, 1);

        assert!(column.is_current(1, 1));
        assert!(!column.is_current(1, 0));
        assert!(column.is_current(2, 0), "its neighbour is untouched");
        assert_eq!(column.handle(2), Some(20));
    }
}
