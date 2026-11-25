//! Material assets connecting shaders, textures, and uniform parameters.
//!
//! # Responsibilities
//!
//! - Carry what one drawn surface needs: the shader it draws with and the
//!   values that shader reads, both as uniform parameters and as bound
//!   textures.
//! - Be plain data. A material names its shader by handle and its values by
//!   the slot names the shader declares, so a game describes an appearance
//!   without reaching into the renderer.
//! - Load a material from a `.material` file in `res`
//!   ([`StandaloneAsset`] with [`MaterialDocument`]), which names its shader
//!   and textures by guid so the file still resolves in the next run.
//!
//! # Design
//!
//! The fields copy [`RenderPass`](crate::RenderPass) by design: a shader
//! handle, a map of parameter values and a map of textures. Both maps are keyed
//! by the slot names the shader declares and packed by the same rules, so the
//! renderer drives a material and a pass through one code path. A material's
//! `rendering_order` decides where it sorts inside the pass that draws it.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_engine::{
    asset, pill_mirror_impl, pill_mirror_method, pill_mirror_object, AssetLoadResult, AssetManager,
    AssetReference, Handle, StandaloneAsset,
};
// The parameter vocabulary is part of the frame contract, shared with passes.
use pill_renderer_api::frame::MaterialParameter;
use serde::{Deserialize, Serialize};

// Current crate
use super::{Shader, Texture};

// =============================================================================
// MaterialTexture
// =============================================================================

/// A texture handle bound to one of a material's slots.
///
/// It holds the asset handle, not a GPU texture. The renderer looks the handle
/// up when it builds the material, so one texture asset can fill any number of
/// slots without being copied.
#[derive(Clone, Debug)]
pub struct MaterialTexture {
    /// The texture asset this binding points at.
    pub texture: Handle<Texture>,
}

// =============================================================================
// Material
// =============================================================================

/// How a surface looks: the shader a draw runs and the values that shader
/// reads.
///
/// A material is an asset, so the renderer rebuilds its bind groups whenever the
/// asset's content version changes. A game never has to edit renderer state in
/// the middle of a frame. [`MaterialBuilder`] is the supported way to make one.
///
/// Its maps are `BTreeMap`s, never `HashMap`s. An empty `HashMap` allocates
/// nothing: it points at a static inside the binary that created it. A material
/// lives in the world and outlives that binary - a project or module that made
/// it reloads and its old image is eventually unloaded - so reading such a map
/// afterwards would fault. A `BTreeMap` keeps no such pointer.
#[asset(standalone)]
#[pill_mirror_object(
    asset,
    standalone,
    shared = "pill_rendering::Material",
    former = "pill_master_renderer::Material"
)]
#[derive(Clone, Debug)]
pub struct Material {
    /// Label used in logs and error messages.
    pub name: String,
    /// Shader the material draws with. [`Handle::INVALID`] selects the
    /// renderer's default shader, as does a handle that names no loaded
    /// shader.
    pub shader: Handle<Shader>,
    /// Textures bound to the shader's slots, by slot name.
    ///
    /// It is a map, like [`RenderPass::textures`](crate::RenderPass): a slot
    /// named twice has one entry, where a list would leave the binder to take
    /// its first match. A slot the shader declares and the material leaves
    /// unbound gets the renderer's default texture for that slot's type.
    pub textures: BTreeMap<String, MaterialTexture>,
    /// Uniform parameters, packed one 16-byte slot each, in the order the
    /// shader declares them. A declared slot the material leaves unset packs
    /// as zero.
    pub parameters: BTreeMap<String, MaterialParameter>,
    /// Sort key for materials inside the pass that draws them.
    ///
    /// The render queue stores this byte inverted in its sort key and sorts the
    /// key ascending, so a larger value is drawn earlier. The default `u8::MAX`
    /// therefore draws before any material given an explicit order. Passes are
    /// the reverse: a lower `order` in
    /// [`RenderPass::settings`](crate::RenderPass) runs first.
    pub rendering_order: u8,
}

// =============================================================================
// MaterialBuilder
// =============================================================================

/// Builds a [`Material`] one field at a time.
///
/// A fresh builder holds the renderer's built-in shader, no textures, no
/// parameters and the highest rendering order. A call therefore only has to
/// name the fields that should differ from those defaults.
#[pill_mirror_object]
pub struct MaterialBuilder {
    material: Material,
}

#[pill_mirror_impl]
impl Material {
    /// Start building a material under the given name.
    ///
    /// The name labels the material in logs and error messages, which is why
    /// the builder asks for it up front.
    #[pill_mirror_method]
    pub fn builder(name: impl Into<String>) -> MaterialBuilder {
        MaterialBuilder {
            material: Self {
                name: name.into(),
                shader: Handle::INVALID,
                textures: BTreeMap::new(),
                parameters: BTreeMap::new(),
                rendering_order: u8::MAX,
            },
        }
    }
}

#[pill_mirror_impl]
impl MaterialBuilder {
    /// Set the shader the material draws with. [`Handle::INVALID`] keeps the
    /// renderer's built-in shader.
    #[pill_mirror_method]
    pub fn shader(mut self, shader: &Handle<Shader>) -> Self {
        self.material.shader = *shader;
        self
    }

    /// Bind a texture to one of the shader's declared slots.
    ///
    /// Binding the same slot twice replaces the earlier binding: the map holds
    /// one texture per slot, so the last call wins.
    #[pill_mirror_method]
    pub fn texture(mut self, slot: impl Into<String>, texture: &Handle<Texture>) -> Self {
        self.material
            .textures
            .insert(slot.into(), MaterialTexture { texture: *texture });
        self
    }

    /// Set one scalar uniform parameter, under the slot the shader declares.
    #[pill_mirror_method]
    pub fn scalar_parameter(mut self, slot: impl Into<String>, value: f32) -> Self {
        self.material
            .parameters
            .insert(slot.into(), MaterialParameter::Scalar(value));
        self
    }

    /// Set one boolean uniform parameter, packed as a `u32` of 0 or 1.
    #[pill_mirror_method]
    pub fn bool_parameter(mut self, slot: impl Into<String>, value: bool) -> Self {
        self.material
            .parameters
            .insert(slot.into(), MaterialParameter::Bool(value));
        self
    }

    /// Set one colour uniform parameter, packed as three `f32`s.
    #[pill_mirror_method]
    pub fn color_parameter(mut self, slot: impl Into<String>, value: [f32; 3]) -> Self {
        self.material
            .parameters
            .insert(slot.into(), MaterialParameter::Color(value));
        self
    }

    /// Choose when the material draws relative to the others in the same pass.
    ///
    /// See [`Material::rendering_order`]: a larger value is drawn earlier.
    #[pill_mirror_method]
    pub fn rendering_order(mut self, value: u8) -> Self {
        self.material.rendering_order = value;
        self
    }

    /// Finish building and return the material.
    #[pill_mirror_method]
    pub fn build(self) -> Material {
        self.material
    }
}

// =============================================================================
// MaterialDocument
// =============================================================================

/// Path of the default material this crate ships, relative to its `res/`.
///
/// It is what a scene draws with when nothing names another material: a
/// renderer's chain points its lit pass at it. Its shader is unset, so whichever
/// renderer draws it substitutes the shader it would use for a material that
/// names none.
pub const DEFAULT_MATERIAL_PATH: &str = "materials/pill.pbr.material";

/// The parameters a newly created material starts with, by slot name.
///
/// A plain, lit surface: a light grey base, roughness halfway, no metalness and
/// no light of its own. Those are the values a person who just created a
/// material expects to see shading from, and to be able to pull a colour away
/// from; `neutral_material_parameters` is the other kind of default, the
/// multipliers the shipped default material uses so that a bound map passes
/// through unchanged, which with no maps bound draws as a mirror that gives off
/// white light.
///
/// The names are the material slot vocabulary the shipped shaders use; a
/// renderer whose shader declares other slots ignores them.
pub fn new_material_parameters() -> BTreeMap<String, MaterialParameter> {
    BTreeMap::from([
        (
            "pbr_base".to_owned(),
            MaterialParameter::Color([0.8, 0.8, 0.8]),
        ),
        ("pbr_roughness".to_owned(), MaterialParameter::Scalar(0.5)),
        ("pbr_metallic".to_owned(), MaterialParameter::Scalar(0.0)),
        (
            "pbr_emissive".to_owned(),
            MaterialParameter::Color([0.0, 0.0, 0.0]),
        ),
    ])
}

/// The parameters a newly created material starts with, by slot name.
///
/// Each factor multiplies the map bound to its slot, so these values leave a
/// bound map unchanged, and a slot left unset would pack as zero instead. The
/// names are the material slot vocabulary the shipped shaders use; a renderer
/// whose shader declares other slots ignores them.
pub fn neutral_material_parameters() -> BTreeMap<String, MaterialParameter> {
    BTreeMap::from([
        (
            "pbr_base".to_owned(),
            MaterialParameter::Color([1.0, 1.0, 1.0]),
        ),
        ("pbr_roughness".to_owned(), MaterialParameter::Scalar(1.0)),
        ("pbr_metallic".to_owned(), MaterialParameter::Scalar(1.0)),
        (
            "pbr_emissive".to_owned(),
            MaterialParameter::Color([1.0, 1.0, 1.0]),
        ),
    ])
}

/// A material as written in its `.material` file.
///
/// It is the asset itself, stored under the standard asset header (format
/// version, asset type, guid). Its shader and textures are guids
/// ([`AssetReference`]), resolved to handles when the file loads: a handle means
/// nothing in the next run, a guid does. A reference that names no loaded asset
/// resolves to [`Handle::INVALID`], which draws with the renderer's default
/// shader or the slot's default texture.
///
/// A newly created file starts from the default document: no shader of its own,
/// so the renderer substitutes the one it draws an unqualified material with, at
/// the neutral parameters below, with no maps bound. The editor fills that
/// shader in when the renderer names one, so the material a person just made
/// says which shader it draws through instead of leaving it to a substitution
/// they cannot see. Naming a shader is also how a material joins a chain that
/// draws only its own: a project with a custom chain points its materials at
/// that chain's shader. Maps are `BTreeMap`s for the reason [`Material`] gives.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialDocument {
    /// The shader the material draws with. Unset means the renderer's
    /// default shader.
    pub shader: AssetReference<Shader>,
    /// Textures bound to the shader's slots, by slot name.
    pub textures: BTreeMap<String, AssetReference<Texture>>,
    /// Uniform parameters, by slot name.
    pub parameters: BTreeMap<String, MaterialParameter>,
    /// See [`Material::rendering_order`].
    pub rendering_order: u8,
}

impl Default for MaterialDocument {
    /// No shader and the plain surface a new material starts from; see the
    /// type's docs.
    fn default() -> Self {
        Self {
            shader: AssetReference::unset(),
            textures: BTreeMap::new(),
            parameters: new_material_parameters(),
            rendering_order: u8::MAX,
        }
    }
}

impl StandaloneAsset for Material {
    type Document = MaterialDocument;
    const FILE_EXTENSION: &'static str = "material";

    fn from_document(
        name: &str,
        document: MaterialDocument,
        assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        let textures = document
            .textures
            .into_iter()
            .map(|(slot_name, texture_reference)| {
                let texture = texture_reference.resolve(assets);
                (slot_name, MaterialTexture { texture })
            })
            .collect();
        Ok(Self {
            name: name.to_owned(),
            shader: document.shader.resolve(assets),
            textures,
            parameters: document.parameters,
            rendering_order: document.rendering_order,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A newly created `.material` file draws as it is and looks like a plain
    /// surface: it names no shader, so a renderer substitutes its own default,
    /// and it starts at [`new_material_parameters`] - grey, halfway rough,
    /// non-metal, no light of its own - with nothing bound. The file this crate
    /// ships as the default material is the other kind of default: the neutral
    /// multipliers, which is what makes it renderer-neutral.
    #[test]
    fn the_default_document_is_a_plain_surface() {
        let mut assets = AssetManager::new();
        crate::declare_crate_roots();

        let material =
            Material::from_document("materials/a.material", MaterialDocument::default(), &assets)
                .unwrap();

        assert_eq!(material.shader, Handle::INVALID);
        assert!(material.textures.is_empty());
        assert_eq!(material.parameters, new_material_parameters());
        assert_ne!(
            material.parameters,
            neutral_material_parameters(),
            "a new material is a surface to look at, not the identity multipliers"
        );
        assert_eq!(material.rendering_order, u8::MAX);
        assert_eq!(
            serde_json::to_value(MaterialDocument::default()).unwrap()["shader"],
            serde_json::Value::Null,
            "an unset reference saves as no guid"
        );

        let shipped = assets
            .import_standalone::<Material>(std::path::Path::new(DEFAULT_MATERIAL_PATH))
            .expect("the crate ships its default material");
        let shipped_material = assets
            .get(shipped.handle)
            .expect("the material it just imported");
        assert_eq!(shipped_material.shader, material.shader);
        assert_eq!(
            shipped_material.parameters,
            neutral_material_parameters(),
            "the shipped file keeps the identity multipliers"
        );
        assert!(shipped_material.textures.is_empty());
    }
}
