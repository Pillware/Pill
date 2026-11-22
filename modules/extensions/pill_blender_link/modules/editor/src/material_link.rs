//! Tracks the material one link shows, and decides what Blender needs next.
//!
//! # Responsibilities
//!
//! - Remember the entity a link draws for, the material and shader its graph
//!   was last built from, and the versions of everything sent.
//! - Decide, for one frame, whether Blender needs the whole node graph, only
//!   the parameter values, only the images, nothing, or a report that the
//!   material cannot be read.
//! - Report a problem once instead of on every frame while it lasts.
//!
//! # Design
//!
//! The decision is a plain function over the link and the asset store, so the
//! rules that choose between a full send, a values send and an images send are
//! tested without a socket or a Blender. The socket work lives in
//! [`BlenderLinkServer`](crate::link_server::BlenderLinkServer), which only
//! queues what this module decided, and the logging lives with it.
//!
//! A full send carries the graph [`translate_material`] builds. A values send
//! carries the material's parameter map alone, which the Blender add-on
//! applies to nodes it already has, so an edit that only moves a slider does
//! not rebuild the tree. Both are keyed by what changed since the last send,
//! so an untouched material sends nothing.

// Standard library
use std::collections::BTreeMap;
use std::fmt;

// External crates
use pill_engine::{AssetManager, Entity, Handle};
use pill_master_renderer_data::{
    config, Material, MaterialParameter, MeshRendererComponent, Shader, Texture,
};
use serde::Serialize;

// Current crate
use crate::material_graph::{ImageColorSpace, MaterialGraph};
use crate::material_translate::{
    resolve_material_textures, translate_material, translate_material_fallback, TranslationError,
};

// =============================================================================
// Types
// =============================================================================

/// The material-side state of one link.
///
/// It lives beside the mesh links and survives the frame, so a change is sent
/// once and an unchanged material sends nothing.
#[derive(Clone, Debug)]
pub struct MaterialLink {
    /// The link this state belongs to.
    link_id: u64,
    /// The entity whose material Blender is shown.
    entity: Entity,
    /// What the last full send was built from; `None` means Blender holds no
    /// graph for this link yet.
    sent_structure: Option<MaterialStructure>,
    /// The parameter values the last send carried.
    sent_parameters: BTreeMap<String, MaterialParameter>,
    /// The content version of every texture slot at the last send.
    sent_texture_versions: BTreeMap<String, Option<u64>>,
    /// The problem reported last, so it is logged once while it lasts.
    reported_problem: Option<MaterialProblem>,
}

/// What a full send was built from.
///
/// A change in any field makes the next plan send the whole graph again: the
/// node tree depends on the shader's fragment, on which textures the material
/// binds, and on the shader's own content.
#[derive(Clone, Debug, PartialEq)]
struct MaterialStructure {
    /// The material asset the graph came from.
    material: Handle<Material>,
    /// The shader the material draws with, after the built-in fallback.
    shader: Handle<Shader>,
    /// The shader's content version when the graph was built.
    shader_version: Option<u64>,
    /// The texture handle bound to each of the shader's slots.
    texture_bindings: BTreeMap<String, Option<Handle<Texture>>>,
}

/// Why a link's material cannot be sent right now.
///
/// A problem is reported once, so a warning does not repeat every frame while
/// it lasts. A shader that builds no surface is not a problem: its slots are
/// placed by name, and what could not be placed arrives as a refusal.
#[derive(Clone, Debug, PartialEq)]
pub enum MaterialProblem {
    /// The entity no longer carries a mesh renderer.
    MissingComponent,
    /// The entity has no material assigned, so there is nothing to translate.
    NoMaterial,
    /// The material asset is not loaded.
    MissingMaterial,
    /// The shader asset, or the built-in one, is not loaded.
    MissingShader,
    /// The fragment stage cannot be read as WGSL.
    UnreadableFragment(String),
}

impl fmt::Display for MaterialProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingComponent => {
                write!(formatter, "the entity no longer draws a mesh")
            }
            Self::NoMaterial => write!(formatter, "no material is assigned to the entity"),
            Self::MissingMaterial => write!(formatter, "the material asset is not loaded"),
            Self::MissingShader => write!(formatter, "the shader asset is not loaded"),
            Self::UnreadableFragment(detail) => {
                write!(formatter, "the fragment is not readable: {detail}")
            }
        }
    }
}

/// One image Blender reloads, as the images frame describes it.
///
/// It carries the file and the colour space rather than a slot name alone, so
/// Blender can also load an image node the person deleted, or one whose file
/// was missing when the graph was built.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaterialImage {
    /// The texture slot it fills.
    pub slot: String,
    /// The file Blender loads.
    pub absolute_path: String,
    /// How Blender must read the texels.
    pub color_space: ImageColorSpace,
}

/// What one frame's plan says to send.
#[derive(Clone, Debug)]
pub enum MaterialAction {
    /// The whole node graph, with the material's name.
    Full {
        /// The material's asset name, which Blender shows.
        name: String,
        /// The node graph to build.
        graph: MaterialGraph,
        /// Whether the graph came from the standard slot names instead of a
        /// surface the shader builds.
        fallback: bool,
    },
    /// Only the parameter values, applied to nodes Blender already has.
    Values {
        /// Every parameter the material sets, by slot name.
        parameters: BTreeMap<String, MaterialParameter>,
    },
    /// Reload these images, leaving the rest of the node tree alone.
    Images {
        /// The textures whose content moved.
        images: Vec<MaterialImage>,
    },
    /// Nothing changed since the last send.
    None,
    /// The material cannot be sent, and the reason was not reported yet.
    Problem(MaterialProblem),
}

// =============================================================================
// Planner
// =============================================================================

impl MaterialLink {
    /// Starts tracking a link that has not sent a material yet.
    pub fn new(link_id: u64, entity: Entity) -> Self {
        Self {
            link_id,
            entity,
            sent_structure: None,
            sent_parameters: BTreeMap::new(),
            sent_texture_versions: BTreeMap::new(),
            reported_problem: None,
        }
    }

    /// The link this state belongs to.
    pub fn link_id(&self) -> u64 {
        self.link_id
    }

    /// The entity whose material Blender is shown.
    pub fn entity(&self) -> Entity {
        self.entity
    }

    /// Shows a different entity's material on the same link.
    ///
    /// The next plan sends the whole graph, because the node tree Blender
    /// holds belongs to the previous entity's material.
    pub fn change_entity(&mut self, entity: Entity) {
        self.entity = entity;
        self.sent_structure = None;
    }

    /// Makes the next plan send the whole graph, keeping the reported problem.
    ///
    /// A Blender that just connected holds no nodes yet.
    pub fn forget_sent_graph(&mut self) {
        self.sent_structure = None;
    }

    /// Reports `problem` unless it is the one already reported.
    pub fn report_once(&mut self, problem: MaterialProblem) -> MaterialAction {
        if self.reported_problem.as_ref() == Some(&problem) {
            return MaterialAction::None;
        }
        self.reported_problem = Some(problem.clone());
        MaterialAction::Problem(problem)
    }

    /// Takes the version of every texture slot the material binds.
    pub fn texture_versions(
        texture_bindings: &BTreeMap<String, Option<Handle<Texture>>>,
        assets: &AssetManager,
    ) -> BTreeMap<String, Option<u64>> {
        texture_bindings
            .iter()
            .map(|(slot, texture)| {
                (
                    slot.clone(),
                    texture.and_then(|handle| assets.content_version(handle)),
                )
            })
            .collect()
    }
}

/// Decides what this frame owes Blender for one link's material, and records
/// the state it decided from.
///
/// The first call of a link sends the whole graph. After that, a changed
/// material, shader or texture binding rebuilds the graph, a changed parameter
/// map sends the values alone, and a moved texture version reloads the images.
/// When several of those change in one frame the earlier send wins and the
/// later one follows on the next frame, because each send carries the state it
/// describes.
pub fn plan_material_update(
    link: &mut MaterialLink,
    assets: &AssetManager,
    component: Option<&MeshRendererComponent>,
) -> MaterialAction {
    // Step 1: The entity has to still draw the material.
    let Some(component) = component else {
        return link.report_once(MaterialProblem::MissingComponent);
    };

    // Step 2: The material asset has to be loaded - and there has to be one.
    // An entity with no material assigned draws nothing yet, which is a state
    // the person can fix, so it is reported as itself rather than as a missing
    // asset.
    if component.material == Handle::INVALID {
        return link.report_once(MaterialProblem::NoMaterial);
    }
    let Some(material) = assets.get::<Material>(component.material) else {
        return link.report_once(MaterialProblem::MissingMaterial);
    };

    // Step 3: The shader, with the renderer's built-in one for an empty handle.
    let shader_handle = if material.shader == Handle::INVALID {
        match assets.handle_by_name::<Shader>(config::pbr_pipeline::SHADER_PATH) {
            Some(handle) => handle,
            None => return link.report_once(MaterialProblem::MissingShader),
        }
    } else {
        material.shader
    };
    let Some(shader) = assets.get::<Shader>(shader_handle) else {
        return link.report_once(MaterialProblem::MissingShader);
    };

    // Step 4: What a full graph would be built from this frame.
    let texture_bindings: BTreeMap<String, Option<Handle<Texture>>> = shader
        .texture_slots
        .keys()
        .map(|slot| {
            let bound = material.textures.get(slot).map(|binding| binding.texture);
            (slot.clone(), bound)
        })
        .collect();
    let structure = MaterialStructure {
        material: component.material,
        shader: shader_handle,
        shader_version: assets.content_version(shader_handle),
        texture_bindings,
    };
    let texture_versions = MaterialLink::texture_versions(&structure.texture_bindings, assets);

    // Step 5: A new or changed structure rebuilds the graph.
    if link.sent_structure.as_ref() != Some(&structure) {
        let textures = resolve_material_textures(shader, material, assets);
        // A shader that builds a surface describes its inputs exactly; one
        // that does not still gets the standard slots, placed by name.
        let (graph, fallback) = match translate_material(shader, &material.parameters, &textures) {
            Ok(graph) => (graph, false),
            Err(TranslationError::MissingSurface) => (
                translate_material_fallback(shader, &material.parameters, &textures),
                true,
            ),
            Err(error) => {
                return link.report_once(MaterialProblem::UnreadableFragment(error.to_string()));
            }
        };
        let name = assets
            .name_of(component.material)
            .map(str::to_owned)
            .unwrap_or_else(|| material.name.clone());
        link.sent_structure = Some(structure);
        link.sent_parameters = material.parameters.clone();
        link.sent_texture_versions = texture_versions;
        link.reported_problem = None;
        return MaterialAction::Full {
            name,
            graph,
            fallback,
        };
    }

    // Step 6: A changed parameter map only needs the values.
    if link.sent_parameters != material.parameters {
        link.sent_parameters = material.parameters.clone();
        link.reported_problem = None;
        return MaterialAction::Values {
            parameters: material.parameters.clone(),
        };
    }

    // Step 7: Moved texture content reloads just those images.
    let mut changed_slots = Vec::new();
    for (slot, version) in &texture_versions {
        if link.sent_texture_versions.get(slot) != Some(version) {
            changed_slots.push(slot.clone());
        }
    }
    if !changed_slots.is_empty() {
        let textures = resolve_material_textures(shader, material, assets);
        // A slot whose file cannot be resolved still travels, without a path:
        // Blender then reloads the image it already holds.
        let images: Vec<MaterialImage> = changed_slots
            .into_iter()
            .map(|slot| {
                let texture = textures.get(&slot);
                MaterialImage {
                    absolute_path: texture
                        .map(|texture| texture.absolute_path.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default(),
                    color_space: texture
                        .map(|texture| texture.color_space)
                        .unwrap_or(ImageColorSpace::Srgb),
                    slot,
                }
            })
            .collect();
        link.sent_texture_versions = texture_versions;
        link.reported_problem = None;
        return MaterialAction::Images { images };
    }

    // Step 8: Nothing changed, and any problem reported earlier is over.
    link.reported_problem = None;
    MaterialAction::None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Standard library
    use super::*;

    // External crates
    use pill_master_renderer_data::{
        config::pbr_pipeline::PBR_FRAGMENT, ShaderParameterSlot, ShaderParameterType,
        ShaderTextureKind, ShaderTextureSlot, TextureType,
    };

    /// The shipped PBR fragment with the slots its metadata declares.
    fn pbr_shader() -> Shader {
        Shader::new("pbr")
            .with_wgsl("", PBR_FRAGMENT.embedded_source)
            .with_parameter_slots([
                ShaderParameterSlot {
                    name: "pbr_base".to_owned(),
                    parameter_type: ShaderParameterType::Color,
                },
                ShaderParameterSlot {
                    name: "pbr_roughness".to_owned(),
                    parameter_type: ShaderParameterType::Scalar,
                },
                ShaderParameterSlot {
                    name: "pbr_metallic".to_owned(),
                    parameter_type: ShaderParameterType::Scalar,
                },
                ShaderParameterSlot {
                    name: "pbr_emissive".to_owned(),
                    parameter_type: ShaderParameterType::Color,
                },
            ])
            .with_texture_slots([
                ShaderTextureSlot {
                    name: "base_color".to_owned(),
                    kind: ShaderTextureKind::Color,
                    texture_binding: 0,
                    sampler_binding: 1,
                },
                ShaderTextureSlot {
                    name: "normal".to_owned(),
                    kind: ShaderTextureKind::Normal,
                    texture_binding: 2,
                    sampler_binding: 3,
                },
                ShaderTextureSlot {
                    name: "metallic_roughness".to_owned(),
                    kind: ShaderTextureKind::Color,
                    texture_binding: 4,
                    sampler_binding: 5,
                },
                ShaderTextureSlot {
                    name: "emissive".to_owned(),
                    kind: ShaderTextureKind::Color,
                    texture_binding: 6,
                    sampler_binding: 7,
                },
            ])
            .with_engine_parameters(true)
            .with_camera_parameters(true)
            .build()
            .expect("the shipped fragment is text")
    }

    /// A one-texel colour texture.
    fn texture(name: &str) -> Texture {
        Texture::from_rgba(name, TextureType::Color, vec![255; 4], 1, 1)
            .expect("one RGBA texel fits a 1x1 colour texture")
    }

    /// A store holding the shader, a texture and a material that draws with
    /// them, plus the handles the tests need.
    struct Fixture {
        assets: AssetManager,
        shader: Handle<Shader>,
        texture: Handle<Texture>,
        material: Handle<Material>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut assets = AssetManager::new();
            let shader = assets
                .add_named("shaders/pbr.hlsl", pbr_shader())
                .expect("the name is free");
            let texture = assets
                .add_named("textures/probe.png", texture("textures/probe.png"))
                .expect("the name is free");
            let material = assets
                .add_named(
                    "materials/probe.material",
                    Material::builder("probe")
                        .shader(&shader)
                        .texture("base_color", &texture)
                        .texture("metallic_roughness", &texture)
                        .texture("emissive", &texture)
                        .color_parameter("pbr_base", [1.0, 1.0, 1.0])
                        .scalar_parameter("pbr_roughness", 0.5)
                        .scalar_parameter("pbr_metallic", 0.0)
                        .color_parameter("pbr_emissive", [0.0, 0.0, 0.0])
                        .build(),
                )
                .expect("the name is free");
            Self {
                assets,
                shader,
                texture,
                material,
            }
        }

        /// The component a linked entity carries.
        fn component(&self) -> MeshRendererComponent {
            MeshRendererComponent {
                mesh: Handle::INVALID,
                material: self.material,
            }
        }

        /// Plans one frame for `link`, with the entity still drawing.
        fn plan(&self, link: &mut MaterialLink) -> MaterialAction {
            let component = self.component();
            plan_material_update(link, &self.assets, Some(&component))
        }
    }

    /// The entity a link shows when nothing changed it.
    fn linked_entity() -> Entity {
        Entity::from_raw(1, 1)
    }

    /// The problem an action reports, if it reports one.
    fn problem_of(action: MaterialAction) -> Option<MaterialProblem> {
        match action {
            MaterialAction::Problem(problem) => Some(problem),
            _ => None,
        }
    }

    /// The plan of a link that has never sent anything is the whole graph.
    #[test]
    fn the_first_plan_of_a_link_sends_the_whole_graph() {
        let fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        let action = fixture.plan(&mut link);
        match action {
            MaterialAction::Full { name, .. } => {
                assert_eq!(name, "materials/probe.material");
            }
            other => panic!("the first plan sends the graph, got {other:?}"),
        }
    }

    /// A moved slider sends the values alone.
    #[test]
    fn a_parameter_change_sends_only_the_values() {
        let mut fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));

        let material = fixture
            .assets
            .get_mut::<Material>(fixture.material)
            .expect("the material is loaded");
        material
            .parameters
            .insert("pbr_roughness".to_owned(), MaterialParameter::Scalar(0.25));

        match fixture.plan(&mut link) {
            MaterialAction::Values { parameters } => {
                assert_eq!(
                    parameters.get("pbr_roughness"),
                    Some(&MaterialParameter::Scalar(0.25))
                );
            }
            other => panic!("a parameter edit sends the values, got {other:?}"),
        }
    }

    /// A new shader version rebuilds the graph, because the fragment may have
    /// changed shape.
    #[test]
    fn a_shader_content_change_sends_the_graph_again() {
        let mut fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));

        let _ = fixture
            .assets
            .get_mut::<Shader>(fixture.shader)
            .expect("the shader is loaded");

        assert!(
            matches!(fixture.plan(&mut link), MaterialAction::Full { .. }),
            "a shader version move sends the graph again"
        );
    }

    /// An entity with no material assigned is reported as that, once, instead
    /// of as a missing asset.
    #[test]
    fn a_component_without_a_material_reports_it_once() {
        let fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        let component = MeshRendererComponent {
            mesh: Handle::INVALID,
            material: Handle::INVALID,
        };
        match plan_material_update(&mut link, &fixture.assets, Some(&component)) {
            MaterialAction::Problem(MaterialProblem::NoMaterial) => {}
            action => panic!("expected a missing-material report, got {action:?}"),
        }
        assert!(
            matches!(
                plan_material_update(&mut link, &fixture.assets, Some(&component)),
                MaterialAction::None
            ),
            "the same problem is not reported twice"
        );
    }

    /// New texture content reloads only the slots that moved.
    #[test]
    fn a_texture_content_change_sends_only_the_images() {
        let mut fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));

        let _ = fixture
            .assets
            .get_mut::<Texture>(fixture.texture)
            .expect("the texture is loaded");

        match fixture.plan(&mut link) {
            MaterialAction::Images { images } => {
                let slots: Vec<&str> = images.iter().map(|image| image.slot.as_str()).collect();
                assert_eq!(slots, vec!["base_color", "emissive", "metallic_roughness"]);
            }
            other => panic!("a texture edit sends the images, got {other:?}"),
        }
    }

    /// An untouched material sends nothing, frame after frame.
    #[test]
    fn an_unchanged_material_sends_nothing() {
        let fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));
        assert!(matches!(fixture.plan(&mut link), MaterialAction::None));
        assert!(matches!(fixture.plan(&mut link), MaterialAction::None));
    }

    /// A link whose entity lost its renderer reports that once.
    #[test]
    fn a_link_whose_entity_lost_its_renderer_reports_once() {
        let fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert_eq!(
            problem_of(plan_material_update(&mut link, &fixture.assets, None)),
            Some(MaterialProblem::MissingComponent)
        );
        assert!(matches!(
            plan_material_update(&mut link, &fixture.assets, None),
            MaterialAction::None
        ));
    }

    /// A fragment that never shades a surface still sends a graph: the slots
    /// whose names are the renderer's standard ones, placed by the fallback.
    #[test]
    fn a_fragment_without_a_surface_sends_the_fallback_graph() {
        let mut fixture = Fixture::new();
        let shader = Shader::new("plain")
            .with_wgsl(
                "",
                "@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }\n",
            )
            .with_engine_parameters(true)
            .with_camera_parameters(true)
            .build()
            .expect("the fragment is text");
        let shader = fixture
            .assets
            .add_named("shaders/plain.hlsl", shader)
            .expect("the name is free");
        let material = fixture
            .assets
            .add_named(
                "materials/plain.material",
                Material::builder("plain").shader(&shader).build(),
            )
            .expect("the name is free");
        let mut link = MaterialLink::new(7, linked_entity());
        let component = MeshRendererComponent {
            mesh: Handle::INVALID,
            material,
        };
        match plan_material_update(&mut link, &fixture.assets, Some(&component)) {
            MaterialAction::Full { fallback, .. } => {
                assert!(
                    fallback,
                    "a shader with no surface is answered by the named slots"
                );
            }
            other => panic!("a shader with no surface sends the fallback, got {other:?}"),
        }
        assert!(matches!(
            plan_material_update(&mut link, &fixture.assets, Some(&component)),
            MaterialAction::None
        ));
    }

    /// A fragment that cannot be read at all is reported once, and the link
    /// keeps quiet until it changes.
    #[test]
    fn a_fragment_that_cannot_be_read_reports_once() {
        let mut fixture = Fixture::new();
        let shader = Shader::new("broken")
            .with_wgsl("", "this is not wgsl")
            .with_engine_parameters(true)
            .with_camera_parameters(true)
            .build()
            .expect("the shader only stores its text");
        let shader = fixture
            .assets
            .add_named("shaders/broken.hlsl", shader)
            .expect("the name is free");
        let material = fixture
            .assets
            .add_named(
                "materials/broken.material",
                Material::builder("broken").shader(&shader).build(),
            )
            .expect("the name is free");
        let mut link = MaterialLink::new(7, linked_entity());
        let component = MeshRendererComponent {
            mesh: Handle::INVALID,
            material,
        };
        let first = problem_of(plan_material_update(
            &mut link,
            &fixture.assets,
            Some(&component),
        ));
        assert!(
            matches!(first, Some(MaterialProblem::UnreadableFragment(_))),
            "an unreadable fragment is reported, got {first:?}"
        );
        assert!(matches!(
            plan_material_update(&mut link, &fixture.assets, Some(&component)),
            MaterialAction::None
        ));
    }

    /// A click on another entity rebuilds the graph, even though the link's
    /// last send recorded a material.
    #[test]
    fn another_entity_on_the_same_link_sends_the_graph_again() {
        let fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));

        link.change_entity(Entity::from_raw(2, 1));
        assert!(
            matches!(fixture.plan(&mut link), MaterialAction::Full { .. }),
            "the new entity's material is built from scratch"
        );
    }

    /// A Blender that just connected gets the graph again.
    #[test]
    fn a_new_connection_gets_the_graph_again() {
        let fixture = Fixture::new();
        let mut link = MaterialLink::new(7, linked_entity());
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));
        assert!(matches!(fixture.plan(&mut link), MaterialAction::None));

        link.forget_sent_graph();
        assert!(matches!(
            fixture.plan(&mut link),
            MaterialAction::Full { .. }
        ));
    }

    /// The images payload names the fields the Blender add-on reads, in the
    /// spelling it compares against.
    #[test]
    fn an_image_travels_with_the_fields_blender_reads() {
        let image = MaterialImage {
            slot: "base_color".to_owned(),
            absolute_path: "C:/project/res/textures/base.png".to_owned(),
            color_space: ImageColorSpace::NonColor,
        };
        let encoded = serde_json::to_value(&image).expect("an image is plain data");
        assert_eq!(encoded["slot"], "base_color");
        assert_eq!(encoded["absolute_path"], "C:/project/res/textures/base.png");
        assert_eq!(encoded["color_space"], "NonColor");
    }

    /// A parameter value travels in the form the add-on applies: one named
    /// variant per type.
    #[test]
    fn a_parameter_travels_in_the_form_blender_applies() {
        let scalar =
            serde_json::to_value(MaterialParameter::Scalar(0.25)).expect("a scalar is plain data");
        assert_eq!(scalar["Scalar"], 0.25);
        let colour = serde_json::to_value(MaterialParameter::Color([1.0, 0.0, 0.0]))
            .expect("a colour is plain data");
        assert_eq!(colour["Color"][0], 1.0);
    }
}
