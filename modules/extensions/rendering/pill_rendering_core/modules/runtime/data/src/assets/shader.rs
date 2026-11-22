//! Shader assets and their material binding declarations.
//!
//! # Responsibilities
//!
//! - Carry a shader's two WGSL stages plus the uniform and texture slots its
//!   source declares, under the names the source gives them.
//! - Keep the slots in the order the source declares them. Where a parameter
//!   slot sits in that order decides where its value lands in the uniform
//!   buffer, so reordering the slots would leave the packed buffer disagreeing
//!   with the shader's own declarations.
//! - Offer both ways in: [`ShaderBuilder::with_vertex_source`] and
//!   [`ShaderBuilder::with_fragment_source`] name files to read when the shader
//!   is built, and [`ShaderBuilder::with_wgsl`] takes both stages already in
//!   memory.
//!
//! # Design
//!
//! [`ShaderBuilder`] exists because reading the sources is the one step that
//! can fail: the builder collects the parts in any order, and
//! [`ShaderBuilder::build`] reads the two stages, so a half-built shader is
//! never alive. The slots are where the shader meets the material and pass side
//! of the renderer: that side walks them by name, in declaration order, to size
//! the uniform buffer and place each value.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use indexmap::IndexMap;
use pill_core::telemetry::telemetry_target;
use pill_engine::{
    asset, pill_mirror_impl, pill_mirror_method, pill_mirror_object, AssetLoadError,
    AssetLoadResult, AssetLoader, ImportedAsset, PillMirror,
};
use serde::{Deserialize, Serialize};

// Current crate
use super::TextureType;

// =============================================================================
// ShaderParameterType
// =============================================================================

/// The kind of value a uniform parameter carries.
///
/// The renderer packs a slot's bytes according to this kind, not according to
/// the kind of value it was given. A value whose kind does not match its slot
/// is ignored, and that slot packs as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PillMirror)]
#[repr(u8)]
pub enum ShaderParameterType {
    /// A single float.
    Scalar,
    /// A flag, packed as the integer `1` or `0`.
    Bool,
    /// An RGB color.
    Color,
}

// =============================================================================
// ShaderParameterSlot
// =============================================================================

/// One uniform a shader declares, under the name its source gives it.
///
/// A slot is how a value finds the uniform meant for it. A material or a pass
/// carries its parameters by name, and the shader's slots say which names it
/// accepts and how each matching value packs.
///
/// This is also the type a `.hlsl.meta` declares its uniforms with: one type
/// means one serialized shape, so a new slot attribute is added only here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[pill_mirror_object]
pub struct ShaderParameterSlot {
    /// The name the source binds the uniform under.
    pub name: String,
    /// The kind of value the uniform carries, which decides how it packs.
    #[serde(rename = "type")]
    pub parameter_type: ShaderParameterType,
}

#[pill_mirror_impl]
impl ShaderParameterSlot {
    /// A slot for `name`, carrying `parameter_type`.
    #[pill_mirror_method]
    pub fn new(name: impl Into<String>, parameter_type: ShaderParameterType) -> Self {
        Self {
            name: name.into(),
            parameter_type,
        }
    }
}

// =============================================================================
// ShaderTextureKind
// =============================================================================

/// What a shader texture slot reads: a texture asset's texels, or a depth
/// buffer.
///
/// A depth buffer is not an asset - no file decodes to one - so it is not a
/// [`TextureType`], which is the texel layout a texture asset carries. A slot
/// names both kinds because a shader can bind either, and the kind decides the
/// binding wgpu accepts: a depth texture cannot be bound where a filterable
/// colour texture is expected.
///
/// Serialized by variant name, so a `.hlsl.meta` writes `"type": "Color"` or
/// `"type": "Depth"`, and renaming a variant changes that file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PillMirror)]
#[repr(u8)]
pub enum ShaderTextureKind {
    /// A colour image, uploaded as sRGB, so a sample arrives as linear light.
    Color,
    /// A normal map, whose bytes are directions.
    Normal,
    /// A high-dynamic-range equirectangular panorama.
    Equirect,
    /// A cubemap projected from a panorama.
    Cubemap,
    /// A depth buffer, which only a pass's own input can supply.
    Depth,
}

impl ShaderTextureKind {
    /// The asset texel layout this slot reads, or `None` for a depth buffer,
    /// which no asset carries.
    pub fn texture_type(self) -> Option<TextureType> {
        match self {
            Self::Color => Some(TextureType::Color),
            Self::Normal => Some(TextureType::Normal),
            Self::Equirect => Some(TextureType::Equirect),
            Self::Cubemap => Some(TextureType::Cubemap),
            Self::Depth => None,
        }
    }
}

impl From<TextureType> for ShaderTextureKind {
    fn from(texture_type: TextureType) -> Self {
        match texture_type {
            TextureType::Color => Self::Color,
            TextureType::Normal => Self::Normal,
            TextureType::Equirect => Self::Equirect,
            TextureType::Cubemap => Self::Cubemap,
        }
    }
}

// =============================================================================
// ShaderTextureSlot
// =============================================================================

/// One texture a shader declares, under the name its source gives it, at the
/// texture and sampler bindings the shader declares.
///
/// This is also the type a `.hlsl.meta` declares its textures with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[pill_mirror_object]
pub struct ShaderTextureSlot {
    /// The name the source binds the texture under.
    pub name: String,
    /// The kind of texture the slot binds, which decides the binding wgpu
    /// accepts.
    #[serde(rename = "type")]
    pub kind: ShaderTextureKind,
    /// The binding the shader declares the texture at.
    pub texture_binding: u32,
    /// The binding the shader declares the sampler at.
    pub sampler_binding: u32,
}

#[pill_mirror_impl]
impl ShaderTextureSlot {
    /// A slot for `name`, reading `kind`, at the texture and sampler bindings
    /// the shader uses.
    #[pill_mirror_method]
    pub fn new(name: impl Into<String>, kind: ShaderTextureKind, bindings: (u32, u32)) -> Self {
        Self {
            name: name.into(),
            kind,
            texture_binding: bindings.0,
            sampler_binding: bindings.1,
        }
    }
}

// =============================================================================
// SlotName
// =============================================================================

/// A slot that carries the name its source binds it under.
///
/// Both slot kinds are keyed by this name, so [`slots_by_name`] accepts either
/// of them.
pub trait SlotName {
    /// The name the source binds the slot under.
    fn slot_name(&self) -> &str;
}

impl SlotName for ShaderParameterSlot {
    fn slot_name(&self) -> &str {
        &self.name
    }
}

impl SlotName for ShaderTextureSlot {
    fn slot_name(&self) -> &str {
        &self.name
    }
}

// =============================================================================
// Shader
// =============================================================================

/// A shader asset: its two WGSL stages plus the slots the source declares.
///
/// The slots travel with the sources because both describe the same shader:
/// the sources go to the pipeline, and the slots say how that pipeline's
/// uniform buffer and texture bindings must be laid out.
#[asset(imported)]
#[pill_mirror_object(
    asset,
    shared = "pill_rendering::Shader",
    former = "pill_master_renderer::Shader"
)]
#[derive(Clone, Debug)]
pub struct Shader {
    /// Name used in logs and error messages.
    pub name: String,
    /// The vertex stage, as WGSL text.
    pub vertex_wgsl: String,
    /// The fragment stage, as WGSL text.
    pub fragment_wgsl: String,
    /// The uniforms the shader declares, by name, in declaration order.
    pub parameter_slots: IndexMap<String, ShaderParameterSlot>,
    /// The textures the shader declares, by name, each with its bindings.
    pub texture_slots: IndexMap<String, ShaderTextureSlot>,
    /// Whether the shader reads the engine's parameters, at set 0.
    pub pass_engine_parameters: bool,
    /// Whether the shader reads the camera's parameters, at set 1.
    pub pass_camera_parameters: bool,
}

#[pill_mirror_impl]
impl Shader {
    /// Starts a shader with its name and returns the builder that collects the
    /// rest.
    ///
    /// [`RenderPass::new`](crate::RenderPass::new) starts a pass the same way:
    /// the name comes first, then the parts, and [`ShaderBuilder::build`] reads
    /// the sources and hands the asset back.
    // `clippy::new_ret_no_self`: the chain returns the builder, not the shader,
    // because reading the sources is the one step that can fail.
    #[allow(clippy::new_ret_no_self)]
    #[pill_mirror_method]
    pub fn new(name: impl Into<String>) -> ShaderBuilder {
        ShaderBuilder {
            name: name.into(),
            vertex_source: None,
            fragment_source: None,
            parameter_slots: Vec::new(),
            texture_slots: Vec::new(),
            pass_engine_parameters: false,
            pass_camera_parameters: false,
        }
    }
}

// =============================================================================
// ShaderBuilder
// =============================================================================

/// A [`Shader`] under construction, started by [`Shader::new`].
///
/// The parts arrive in any order and [`Self::build`] reads the two sources, so
/// a half-built shader is never alive.
#[derive(Clone, Debug)]
#[pill_mirror_object]
pub struct ShaderBuilder {
    /// The name the finished shader will carry.
    name: String,
    /// The vertex stage's source, read when the shader is built.
    vertex_source: Option<AssetLoader>,
    /// The fragment stage's source, read when the shader is built.
    fragment_source: Option<AssetLoader>,
    /// The uniform slots collected so far, in the order they were declared.
    parameter_slots: Vec<ShaderParameterSlot>,
    /// The texture slots collected so far, in the order they were declared.
    texture_slots: Vec<ShaderTextureSlot>,
    /// Whether the finished shader will read the engine's parameters.
    pass_engine_parameters: bool,
    /// Whether the finished shader will read the camera's parameters.
    pass_camera_parameters: bool,
}

#[pill_mirror_impl]
impl ShaderBuilder {
    /// The WGSL vertex stage, read from this source when the shader is built.
    #[pill_mirror_method]
    pub fn with_vertex_source(mut self, source: AssetLoader) -> Self {
        self.vertex_source = Some(source);
        self
    }

    /// The WGSL fragment stage, read from this source when the shader is built.
    #[pill_mirror_method]
    pub fn with_fragment_source(mut self, source: AssetLoader) -> Self {
        self.fragment_source = Some(source);
        self
    }

    /// Sets both WGSL stages from text already in hand.
    ///
    /// For stages that are not files: `include_str!` gives a `&'static str`,
    /// and a C# project can pass the text it holds, because the builder is
    /// mirrored to C#. Nothing is resolved or read, so [`Self::build`] already
    /// has both stages, where [`Self::with_vertex_source`] holds a path that
    /// may turn out not to exist.
    #[pill_mirror_method]
    pub fn with_wgsl(mut self, vertex: impl Into<String>, fragment: impl Into<String>) -> Self {
        self.vertex_source = Some(AssetLoader::Bytes(vertex.into().into_bytes().into()));
        self.fragment_source = Some(AssetLoader::Bytes(fragment.into().into_bytes().into()));
        self
    }

    /// The uniform slots the fragment stage declares, each carrying the name it
    /// binds under. Declaration order is the order their values pack in.
    #[pill_mirror_method]
    pub fn with_parameter_slots(
        mut self,
        parameter_slots: impl IntoIterator<Item = ShaderParameterSlot>,
    ) -> Self {
        self.parameter_slots = parameter_slots.into_iter().collect();
        self
    }

    /// The texture slots the fragment stage declares, each carrying the name it
    /// binds under and the bindings the shader declares it at.
    #[pill_mirror_method]
    pub fn with_texture_slots(
        mut self,
        texture_slots: impl IntoIterator<Item = ShaderTextureSlot>,
    ) -> Self {
        self.texture_slots = texture_slots.into_iter().collect();
        self
    }

    /// Whether the shader reads the engine's parameters, at set 0.
    #[pill_mirror_method]
    pub fn with_engine_parameters(mut self, pass_engine_parameters: bool) -> Self {
        self.pass_engine_parameters = pass_engine_parameters;
        self
    }

    /// Whether the shader reads the camera's parameters, at set 1.
    #[pill_mirror_method]
    pub fn with_camera_parameters(mut self, pass_camera_parameters: bool) -> Self {
        self.pass_camera_parameters = pass_camera_parameters;
        self
    }

    /// Reads both sources and produces the shader.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::Decode`] when one of the sources was never
    /// set. Reading one of them can also fail: the path does not resolve, the
    /// read fails, or the bytes are not UTF-8 text.
    #[pill_mirror_method]
    pub fn build(self) -> AssetLoadResult<Shader> {
        // Step 1: Refuse a builder that never named a source. That is a mistake
        // in the chain, not a missing file, so it is refused rather than read
        // from a default path.
        let vertex_source = self.vertex_source.ok_or_else(|| AssetLoadError::Decode {
            label: format!("shader `{}`", self.name),
            detail: "it was built without a vertex source".to_owned(),
        })?;
        let fragment_source = self.fragment_source.ok_or_else(|| AssetLoadError::Decode {
            label: format!("shader `{}`", self.name),
            detail: "it was built without a fragment source".to_owned(),
        })?;

        // Step 2: Key the slots by name, so a repeated name is reported before
        // the stages are read.
        let name = self.name;
        let parameter_slots = slots_by_name(&name, "parameter", self.parameter_slots);
        let texture_slots = slots_by_name(&name, "texture", self.texture_slots);
        Ok(Shader {
            name,
            vertex_wgsl: vertex_source.load_string()?,
            fragment_wgsl: fragment_source.load_string()?,
            parameter_slots,
            texture_slots,
            pass_engine_parameters: self.pass_engine_parameters,
            pass_camera_parameters: self.pass_camera_parameters,
        })
    }
}

// =============================================================================
// ShaderImportSettings
// =============================================================================

/// How to build a shader from its `.hlsl` source: the settings its `.hlsl.meta`
/// holds.
///
/// Every field is part of the metadata file format, so renaming one breaks
/// existing files. `#[serde(default)]` lets a file written before a field
/// existed still load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaderImportSettings {
    /// The vertex stage, by path relative to the same root as the source.
    ///
    /// One `.hlsl` file holds one stage and a shader needs two, so the meta
    /// says which vertex stage this source pairs with.
    pub vertex: String,
    /// The uniforms the shader declares, in the order their values pack.
    pub parameters: Vec<ShaderParameterSlot>,
    /// The textures the shader declares, each with its texture and sampler
    /// bindings.
    pub textures: Vec<ShaderTextureSlot>,
    /// Whether the shader reads the engine's parameters, at set 0.
    pub engine_parameters: bool,
    /// Whether the shader reads the camera's parameters, at set 1.
    pub camera_parameters: bool,
}

// The metadata type name defaults to the shared name above, so every binary's
// copy of `Shader` reads the same files.
impl ImportedAsset for Shader {
    type ImportSettings = ShaderImportSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["hlsl"];

    fn import(
        name: &str,
        _source_bytes: &[u8],
        settings: &ShaderImportSettings,
    ) -> AssetLoadResult<Self> {
        // Step 1: Refuse a meta that names no vertex stage - a shader is two
        // stages, and the meta is what supplies this one.
        if settings.vertex.is_empty() {
            return Err(AssetLoadError::Decode {
                label: name.to_owned(),
                detail: "its `.hlsl.meta` names no vertex stage".to_owned(),
            });
        }
        // Step 2: Build from the WGSL cooked beside this source and the vertex
        // stage the meta names. The HLSL bytes themselves are not read.
        let fragment_path = cooked_wgsl_beside(name)?;
        Shader::new(name.to_owned())
            .with_vertex_source(AssetLoader::Path(settings.vertex.clone().into()))
            .with_fragment_source(AssetLoader::Path(fragment_path))
            .with_parameter_slots(settings.parameters.iter().cloned())
            .with_texture_slots(settings.textures.iter().cloned())
            .with_engine_parameters(settings.engine_parameters)
            .with_camera_parameters(settings.camera_parameters)
            .build()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Key a list of slots by the names they carry, keeping the list's order.
///
/// The shader stores its slots this way: a name says which slot, and the order
/// stays the order the source declares, which is the order a parameter's value
/// packs into the uniform buffer.
///
/// A name declared twice keeps its first declaration and logs a warning naming
/// the later one. Keeping both would give the uniform buffer more slots than
/// the shader declares.
///
/// `declared` names the slot kind in that warning - "parameter" or "texture".
pub fn slots_by_name<S: SlotName>(
    shader_name: &str,
    declared: &'static str,
    slots: impl IntoIterator<Item = S>,
) -> IndexMap<String, S> {
    let mut keyed = IndexMap::new();
    for slot in slots {
        if keyed.contains_key(slot.slot_name()) {
            pill_core::warn!(
                target: telemetry_target::RENDERING,
                "shader `{shader_name}` declares {declared} slot `{}` twice; the later declaration is ignored",
                slot.slot_name()
            );
            continue;
        }
        keyed.insert(slot.slot_name().to_owned(), slot);
    }
    keyed
}

/// The WGSL file this crate's build script cooked beside a shader source.
///
/// # Errors
///
/// Returns [`AssetLoadError::Decode`] when the source names no file, so there
/// is nothing to cook beside.
fn cooked_wgsl_beside(source_path: &str) -> AssetLoadResult<PathBuf> {
    let path = Path::new(source_path);
    let cooked_path = path.with_extension("wgsl");
    if cooked_path.file_name().is_none() {
        return Err(AssetLoadError::Decode {
            label: source_path.to_owned(),
            detail: "it names no file to cook".to_owned(),
        });
    }
    Ok(cooked_path)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.hlsl.meta` declares its slots as the slot types themselves, by
    /// variant name, and a depth texture slot loads the same way.
    #[test]
    fn a_meta_declares_its_slots_as_the_slot_types() {
        let settings: ShaderImportSettings = serde_json::from_str(
            r#"{
                "vertex": "shaders/common_shaders/default_vertex.hlsl",
                "parameters": [
                    { "name": "tint", "type": "Color" },
                    { "name": "exposure", "type": "Scalar" }
                ],
                "textures": [
                    { "name": "color", "type": "Color", "texture_binding": 0, "sampler_binding": 1 },
                    { "name": "normal", "type": "Normal", "texture_binding": 2, "sampler_binding": 3 },
                    { "name": "depth", "type": "Depth", "texture_binding": 4, "sampler_binding": 5 }
                ]
            }"#,
        )
        .expect("the meta shape loads");

        assert_eq!(
            settings.parameters[1].parameter_type,
            ShaderParameterType::Scalar
        );
        assert_eq!(
            settings.textures[0].kind.texture_type(),
            Some(TextureType::Color)
        );
        assert_eq!(settings.textures[2].kind, ShaderTextureKind::Depth);
        assert_eq!(
            settings.textures[2].kind.texture_type(),
            None,
            "a depth buffer is no asset texel layout"
        );
    }

    /// A slot serializes with the name a meta reads it by: the kind's name in
    /// `type`, so a file this build writes loads in the next build.
    #[test]
    fn a_texture_slot_serializes_as_the_name_it_is_read_by() {
        let slot = ShaderTextureSlot::new("sky", ShaderTextureKind::Cubemap, (0, 1));
        let json = serde_json::to_value(&slot).unwrap();

        assert_eq!(json["type"], serde_json::json!("Cubemap"));
        assert_eq!(json["texture_binding"], serde_json::json!(0));
        assert_eq!(json["sampler_binding"], serde_json::json!(1));
        assert_eq!(
            serde_json::from_value::<ShaderTextureSlot>(json).unwrap(),
            slot
        );
    }

    /// Every texel layout an asset can carry maps to a slot kind, and back
    /// again.
    #[test]
    fn every_texture_type_names_its_slot_kind() {
        for texture_type in [
            TextureType::Color,
            TextureType::Normal,
            TextureType::Equirect,
            TextureType::Cubemap,
        ] {
            assert_eq!(
                ShaderTextureKind::from(texture_type).texture_type(),
                Some(texture_type)
            );
        }
    }

    /// Keying a slot list keeps the source's order, and a repeated name keeps
    /// its first declaration, so the packed layout agrees with the shader.
    #[test]
    fn slots_keep_their_order_and_the_first_of_a_repeated_name() {
        let keyed = slots_by_name(
            "test",
            "parameter",
            [
                ShaderParameterSlot::new("tint", ShaderParameterType::Color),
                ShaderParameterSlot::new("roughness", ShaderParameterType::Scalar),
                ShaderParameterSlot::new("tint", ShaderParameterType::Scalar),
            ],
        );

        let names: Vec<&str> = keyed.keys().map(String::as_str).collect();
        assert_eq!(names, ["tint", "roughness"]);
        assert_eq!(keyed["tint"].parameter_type, ShaderParameterType::Color);
    }
}
