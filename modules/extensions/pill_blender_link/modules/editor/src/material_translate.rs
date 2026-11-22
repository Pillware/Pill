//! Translates a cooked PBR fragment stage into a Blender node graph.
//!
//! # Responsibilities
//!
//! - Parse and validate a material's cooked fragment WGSL with naga.
//! - Find the `shade_pbr_surface` call in the entry point and the surface the
//!   call is given.
//! - Translate the five surface fields into [`material_graph`] nodes: images,
//!   named parameter values, products, channels and the normal map.
//! - Fall back to the renderer's standard slot names when a fragment builds no
//!   surface, and report the slots that have no standard name.
//! - Refuse a field whose expression is outside the supported subset, with a
//!   reason that names the construct, instead of approximating it.
//! - Resolve a material's texture slots to the absolute files Blender loads
//!   and the colour space each is read in.
//!
//! # Design
//!
//! The graph is produced from the cooked WGSL, not the HLSL: that text is what
//! the renderer compiles, and it is already in memory on the shader asset. The
//! cook suffixes every name it emits (`shade_pbr_surface_0`,
//! `PbrSurface_0`, `albedo_2`), so names are matched by their prefix with that
//! suffix stripped.
//!
//! The supported expressions are deliberately few - a texture sample, a
//! material parameter, a literal, `*`, `+`, `saturate`/`clamp(x, 0, 1)` and
//! `normalize(x * 2 - 1)`. They are exactly what the surface fields need, and
//! every one of them maps to a Blender node whose meaning matches the shader's.
//! Anything else refuses the field rather than produce a node that shades
//! differently.

// Standard library
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

// External crates
use naga::{
    Arena, BinaryOperator, Expression, Function, GlobalVariable, Handle, Literal, MathFunction,
    Module, Statement, SwizzleComponent, TypeInner, VectorSize,
};
use pill_engine::AssetManager;
use pill_master_renderer_data::{
    config, Material, MaterialParameter, Shader, ShaderParameterType, ShaderTextureKind,
    DEFAULT_COLOR_TEXTURE, DEFAULT_NORMAL_TEXTURE,
};

// Current crate
use crate::material_graph::{
    Channel, ImageColorSpace, Input, MaterialGraph, Node, PrincipledInputs, Refusal,
};

// =============================================================================
// Constants
// =============================================================================

/// The fragment entry point the surface is built in.
const ENTRY_POINT_NAME: &str = "fs_main";

/// The shading function a shader has to end in, with its cooked suffix
/// stripped.
const SURFACE_SHADING_FUNCTION: &str = "shade_pbr_surface";

/// The material's parameter block, as the data crate declares the binding.
const MATERIAL_PARAMETERS_BINDING: u32 = 0;

/// The surface field the base colour is read from.
const FIELD_ALBEDO: &str = "albedo";
/// The surface field the perceptual roughness is read from.
const FIELD_ROUGHNESS: &str = "roughness";
/// The surface field the metalness is read from.
const FIELD_METALLIC: &str = "metallic";
/// The surface field the tangent-space normal is read from.
const FIELD_TANGENT_NORMAL: &str = "tangent_normal";
/// The surface field the emitted colour is read from.
const FIELD_EMISSIVE: &str = "emissive";

/// How deep a constant expression is followed before it is given up on.
const MAXIMUM_CONSTANT_DEPTH: u32 = 8;

/// The texture slot the fallback graph takes the base colour from.
const FALLBACK_BASE_COLOR_TEXTURE: &str = "base_color";
/// The texture slot the fallback graph takes roughness and metalness from.
const FALLBACK_PACKING_TEXTURE: &str = "metallic_roughness";
/// The texture slot the fallback graph takes the normal map from.
const FALLBACK_NORMAL_TEXTURE: &str = "normal";
/// The texture slot the fallback graph takes the emitted colour from.
const FALLBACK_EMISSION_TEXTURE: &str = "emissive";
/// The parameter slot the fallback graph takes the base colour from.
const FALLBACK_BASE_COLOR_PARAMETER: &str = "pbr_base";
/// The parameter slot the fallback graph takes roughness from.
const FALLBACK_ROUGHNESS_PARAMETER: &str = "pbr_roughness";
/// The parameter slot the fallback graph takes metalness from.
const FALLBACK_METALLIC_PARAMETER: &str = "pbr_metallic";
/// The parameter slot the fallback graph takes the emitted colour from.
const FALLBACK_EMISSION_PARAMETER: &str = "pbr_emissive";

/// The texture slots the fallback graph can place.
const FALLBACK_TEXTURE_SLOTS: [&str; 4] = [
    FALLBACK_BASE_COLOR_TEXTURE,
    FALLBACK_PACKING_TEXTURE,
    FALLBACK_NORMAL_TEXTURE,
    FALLBACK_EMISSION_TEXTURE,
];

/// The parameter slots the fallback graph can place.
const FALLBACK_PARAMETER_SLOTS: [&str; 4] = [
    FALLBACK_BASE_COLOR_PARAMETER,
    FALLBACK_ROUGHNESS_PARAMETER,
    FALLBACK_METALLIC_PARAMETER,
    FALLBACK_EMISSION_PARAMETER,
];

// =============================================================================
// Types
// =============================================================================

/// A texture slot resolved to the file Blender loads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTexture {
    /// The file, absolute, so Blender needs no project context to load it.
    pub absolute_path: PathBuf,
    /// How Blender must read the texels.
    pub color_space: ImageColorSpace,
}

/// Why the whole fragment could not be translated.
///
/// A field the mapping cannot express is not one of these: it becomes a
/// [`Refusal`] in the graph, so the rest of the material still translates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranslationError {
    /// The fragment text is not WGSL.
    Parse(String),
    /// The fragment is WGSL but does not validate.
    Invalid(String),
    /// The fragment has no `fs_main`.
    MissingEntryPoint,
    /// The fragment does not call `shade_pbr_surface`.
    MissingSurface,
    /// The call is not given a surface the translator can read.
    UnsupportedSurface(String),
}

impl fmt::Display for TranslationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(detail) => write!(formatter, "the fragment stage does not parse: {detail}"),
            Self::Invalid(detail) => {
                write!(formatter, "the fragment stage does not validate: {detail}")
            }
            Self::MissingEntryPoint => {
                write!(formatter, "the fragment stage has no `{ENTRY_POINT_NAME}`")
            }
            Self::MissingSurface => write!(
                formatter,
                "the fragment does not call `{SURFACE_SHADING_FUNCTION}`"
            ),
            Self::UnsupportedSurface(detail) => write!(
                formatter,
                "the shading call is not given a surface the mapping can read: {detail}"
            ),
        }
    }
}

impl std::error::Error for TranslationError {}

// =============================================================================
// Resolving the material's textures
// =============================================================================

/// Resolves every texture slot a material draws with to the file Blender loads.
///
/// A slot the material leaves unbound draws the renderer's stand-in texture
/// for its kind, so the stand-in is what is resolved. A slot of a kind with no
/// stand-in - an environment map - is left out, and a field that samples it is
/// refused by the translator.
pub fn resolve_material_textures(
    shader: &Shader,
    material: &Material,
    assets: &AssetManager,
) -> BTreeMap<String, ResolvedTexture> {
    let mut resolved = BTreeMap::new();
    for (slot_name, slot) in &shader.texture_slots {
        let bound_name = material
            .textures
            .get(slot_name)
            .and_then(|binding| assets.name_of(binding.texture))
            .map(str::to_owned);
        let name = match bound_name {
            Some(name) => name,
            None => match slot.kind {
                ShaderTextureKind::Color => DEFAULT_COLOR_TEXTURE.to_owned(),
                ShaderTextureKind::Normal => DEFAULT_NORMAL_TEXTURE.to_owned(),
                _ => continue,
            },
        };
        let Some(absolute_path) = pill_engine::asset_store::locate_file(Path::new(&name)) else {
            continue;
        };
        resolved.insert(
            slot_name.clone(),
            ResolvedTexture {
                absolute_path,
                color_space: color_space_of(slot.kind),
            },
        );
    }
    resolved
}

// =============================================================================
// Translating
// =============================================================================

/// Translates a material's cooked fragment stage into a Blender node graph.
///
/// `parameters` are the material's values by slot name, and `textures` are its
/// slots resolved with [`resolve_material_textures`]. A field outside the
/// supported subset is recorded in [`MaterialGraph::refusals`] and left
/// unconnected; the returned error is reserved for a fragment that cannot be
/// read at all.
///
/// # Errors
///
/// Returns [`TranslationError::Parse`] or [`TranslationError::Invalid`] when
/// the text is not valid WGSL, [`TranslationError::MissingEntryPoint`] when the
/// fragment has no `fs_main`, [`TranslationError::MissingSurface`] when it does
/// not call `shade_pbr_surface` - the caller then builds
/// [`translate_material_fallback`] - and [`TranslationError::UnsupportedSurface`]
/// when the call's surface is not a struct built in the fragment.
pub fn translate_material(
    shader: &Shader,
    parameters: &BTreeMap<String, MaterialParameter>,
    textures: &BTreeMap<String, ResolvedTexture>,
) -> Result<MaterialGraph, TranslationError> {
    // Step 1: Read the text as WGSL and validate it, so a construct the
    // translator does not expect is a parse error rather than a panic.
    let module = naga::front::wgsl::parse_str(&shader.fragment_wgsl)
        .map_err(|error| TranslationError::Parse(error.to_string()))?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator
        .validate(&module)
        .map_err(|error| TranslationError::Invalid(error.to_string()))?;

    // Step 2: Find the entry point and the surface it shades.
    let entry = module
        .entry_points
        .iter()
        .find(|entry| entry.name == ENTRY_POINT_NAME)
        .ok_or(TranslationError::MissingEntryPoint)?;
    let function = &entry.function;
    let call = surface_call(&module, function).ok_or(TranslationError::MissingSurface)?;
    let Statement::Call { arguments, .. } = call else {
        return Err(TranslationError::MissingSurface);
    };
    let surface_argument = *arguments.first().ok_or(TranslationError::MissingSurface)?;
    let fields = surface_field_expressions(&module, function, surface_argument)?;

    // Step 3: Translate each field into nodes. The five are independent, so
    // one refused field leaves the other four connected.
    let mut translator = Translator::new(&module, function, shader, parameters, textures);
    let base_color = translator.field(FIELD_ALBEDO, &fields);
    let roughness = translator.field(FIELD_ROUGHNESS, &fields);
    let metallic = translator.field(FIELD_METALLIC, &fields);
    let normal = translator.field(FIELD_TANGENT_NORMAL, &fields);
    let emission = translator.field(FIELD_EMISSIVE, &fields);
    let mut graph = translator.into_graph();
    graph.principled = PrincipledInputs {
        base_color,
        roughness,
        metallic,
        normal,
        emission,
    };
    Ok(graph)
}

// =============================================================================
// Falling back to slot names
// =============================================================================

/// Builds the graph a material gets when its shader builds no surface: the
/// five Principled inputs are fed from the slots whose names are the renderer's
/// standard ones, and every declared slot outside that list is recorded in
/// [`MaterialGraph::refusals`].
///
/// A texture called `base_color` is the base colour and `metallic_roughness` is
/// the packing map whose green and blue channels are roughness and metalness,
/// each multiplied by its own `pbr_*` parameter. A slot with a project's own
/// name is left unconnected rather than guessed at from its type or position:
/// a wrong guess would silently misrepresent the material.
pub fn translate_material_fallback(
    shader: &Shader,
    parameters: &BTreeMap<String, MaterialParameter>,
    textures: &BTreeMap<String, ResolvedTexture>,
) -> MaterialGraph {
    let mut graph = MaterialGraph::default();

    // Step 1: the base colour - the base-colour image, tinted by its parameter.
    let base_color_image = fallback_image(&mut graph, textures, FALLBACK_BASE_COLOR_TEXTURE);
    let base_color_parameter =
        fallback_parameter(&mut graph, parameters, FALLBACK_BASE_COLOR_PARAMETER);
    let base_color = combine(&mut graph, base_color_image, base_color_parameter);
    graph.principled.base_color = base_color;

    // Step 2: roughness and metalness - one channel of the packing map each,
    // times its own parameter.
    let packing_image = fallback_image(&mut graph, textures, FALLBACK_PACKING_TEXTURE);
    let roughness = fallback_channel(
        &mut graph,
        packing_image,
        Channel::Green,
        parameters,
        FALLBACK_ROUGHNESS_PARAMETER,
    );
    graph.principled.roughness = roughness;
    let metallic = fallback_channel(
        &mut graph,
        packing_image,
        Channel::Blue,
        parameters,
        FALLBACK_METALLIC_PARAMETER,
    );
    graph.principled.metallic = metallic;

    // Step 3: the normal map, through a Normal Map node so Blender reads it as
    // a tangent-space normal rather than a colour.
    if let Some(normal_image) = fallback_image(&mut graph, textures, FALLBACK_NORMAL_TEXTURE) {
        let normal = graph.push(Node::NormalMap {
            input: normal_image,
        });
        graph.principled.normal = normal;
    }

    // Step 4: the emitted colour - the emissive image, tinted by its parameter.
    let emission_image = fallback_image(&mut graph, textures, FALLBACK_EMISSION_TEXTURE);
    let emission_parameter =
        fallback_parameter(&mut graph, parameters, FALLBACK_EMISSION_PARAMETER);
    let emission = combine(&mut graph, emission_image, emission_parameter);
    graph.principled.emission = emission;

    // Step 5: every declared slot the fallback had no standard name for, so a
    // missing input in Blender is never a mystery.
    for slot_name in shader.texture_slots.keys() {
        if !FALLBACK_TEXTURE_SLOTS.contains(&slot_name.as_str()) {
            graph.refusals.push(Refusal {
                field: slot_name.clone(),
                reason: "no standard Principled input takes this texture slot".to_owned(),
            });
        }
    }
    for parameter_name in shader.parameter_slots.keys() {
        if !FALLBACK_PARAMETER_SLOTS.contains(&parameter_name.as_str()) {
            graph.refusals.push(Refusal {
                field: parameter_name.clone(),
                reason: "no standard Principled input takes this parameter".to_owned(),
            });
        }
    }
    graph
}

/// Appends the image node of a texture slot, when a file resolved for it.
pub fn fallback_image(
    graph: &mut MaterialGraph,
    textures: &BTreeMap<String, ResolvedTexture>,
    slot_name: &str,
) -> Option<Input> {
    let texture = textures.get(slot_name)?;
    Some(graph.push(Node::Image {
        slot: slot_name.to_owned(),
        absolute_path: texture.absolute_path.to_string_lossy().replace('\\', "/"),
        color_space: texture.color_space,
    }))
}

/// Appends the node of a material parameter, when the material sets it.
pub fn fallback_parameter(
    graph: &mut MaterialGraph,
    parameters: &BTreeMap<String, MaterialParameter>,
    parameter_name: &str,
) -> Option<Input> {
    let value = match parameters.get(parameter_name)? {
        MaterialParameter::Color(color) => Node::Rgb {
            name: parameter_name.to_owned(),
            value: *color,
        },
        MaterialParameter::Scalar(value) => Node::Value {
            name: parameter_name.to_owned(),
            value: *value,
        },
        MaterialParameter::Bool(value) => Node::Value {
            name: parameter_name.to_owned(),
            value: f32::from(u8::from(*value)),
        },
    };
    Some(graph.push(value))
}

/// Feeds one Principled input from the two sources a fallback can find for it.
///
/// Both present means a product - a texture tinted by a parameter - and only
/// one present means that one. Neither leaves the input unconnected, so
/// Blender's own default applies.
pub fn combine(
    graph: &mut MaterialGraph,
    texture: Option<Input>,
    parameter: Option<Input>,
) -> Input {
    match (texture, parameter) {
        (Some(texture), Some(parameter)) => graph.push(Node::Multiply {
            first: texture,
            second: parameter,
        }),
        (Some(input), None) | (None, Some(input)) => input,
        (None, None) => Input::Unconnected,
    }
}

/// Feeds roughness or metalness from one channel of the packing map, times the
/// parameter that scales it.
pub fn fallback_channel(
    graph: &mut MaterialGraph,
    packing_image: Option<Input>,
    channel: Channel,
    parameters: &BTreeMap<String, MaterialParameter>,
    parameter_name: &str,
) -> Input {
    let parameter = fallback_parameter(graph, parameters, parameter_name);
    let Some(packing_image) = packing_image else {
        return parameter.unwrap_or(Input::Unconnected);
    };
    let component = graph.push(Node::Component {
        input: packing_image,
        channel,
    });
    match parameter {
        Some(parameter) => graph.push(Node::Multiply {
            first: component,
            second: parameter,
        }),
        None => component,
    }
}

// =============================================================================
// Translator
// =============================================================================

/// Translates one fragment's surface expressions, collecting the graph.
struct Translator<'a> {
    /// The parsed module the expressions belong to.
    module: &'a Module,
    /// The entry point's function.
    function: &'a Function,
    /// The shader asset, for the slots it declares.
    shader: &'a Shader,
    /// The material's parameter values by slot name.
    parameters: &'a BTreeMap<String, MaterialParameter>,
    /// The material's texture slots resolved to files.
    textures: &'a BTreeMap<String, ResolvedTexture>,
    /// The material's parameter block, when the shader declares one.
    material_global: Option<Handle<GlobalVariable>>,
    /// Texture slot names by the binding they are declared at.
    texture_slots_by_binding: HashMap<u32, String>,
    /// The value each local variable is first stored from.
    local_values: HashMap<Handle<naga::LocalVariable>, Handle<Expression>>,
    /// The node a parameter slot already produced, by slot name.
    parameter_nodes: HashMap<String, Input>,
    /// The node a texture slot already produced, by slot name.
    texture_nodes: HashMap<String, Input>,
    /// The graph being built.
    graph: MaterialGraph,
}

impl<'a> Translator<'a> {
    /// Starts a translator for one fragment stage.
    fn new(
        module: &'a Module,
        function: &'a Function,
        shader: &'a Shader,
        parameters: &'a BTreeMap<String, MaterialParameter>,
        textures: &'a BTreeMap<String, ResolvedTexture>,
    ) -> Self {
        let material_global = module
            .global_variables
            .iter()
            .find(|(_, global)| {
                global.binding.is_some_and(|binding| {
                    binding.group == config::MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX
                        && binding.binding == MATERIAL_PARAMETERS_BINDING
                })
            })
            .map(|(handle, _)| handle);
        let texture_slots_by_binding = shader
            .texture_slots
            .values()
            .map(|slot| (slot.texture_binding, slot.name.clone()))
            .collect();
        let mut local_values = HashMap::new();
        for statement in function.body.iter() {
            if let Statement::Store { pointer, value } = statement {
                if let Expression::LocalVariable(local) = function.expressions[*pointer] {
                    local_values.insert(local, *value);
                }
            }
        }
        Self {
            module,
            function,
            shader,
            parameters,
            textures,
            material_global,
            texture_slots_by_binding,
            local_values,
            parameter_nodes: HashMap::new(),
            texture_nodes: HashMap::new(),
            graph: MaterialGraph::default(),
        }
    }

    /// Returns the graph built so far.
    fn into_graph(self) -> MaterialGraph {
        self.graph
    }

    /// Translates one surface field, recording a refusal when it cannot.
    fn field(&mut self, field: &str, fields: &HashMap<String, Handle<Expression>>) -> Input {
        let Some(handle) = fields.get(field) else {
            self.graph.refusals.push(Refusal {
                field: field.to_owned(),
                reason: "the surface does not set this field".to_owned(),
            });
            return Input::Unconnected;
        };
        match self.input(*handle) {
            Ok(input) => input,
            Err(reason) => {
                self.graph.refusals.push(Refusal {
                    field: field.to_owned(),
                    reason,
                });
                Input::Unconnected
            }
        }
    }

    /// Translates one value expression into a node.
    fn input(&mut self, handle: Handle<Expression>) -> Result<Input, String> {
        let function = self.function;
        match &function.expressions[handle] {
            Expression::Load { pointer } => self.pointer(*pointer),
            Expression::LocalVariable(local) => {
                let Some(value) = self.local_values.get(local).copied() else {
                    return Err(
                        "the surface reads a local variable before it is written".to_owned()
                    );
                };
                self.input(value)
            }
            Expression::GlobalVariable(global) => self.material_member(*global, None),
            Expression::AccessIndex { base, index } => self.access_index(*base, *index),
            Expression::Access { base, index } => {
                let index = constant_u32(&function.expressions, *index).ok_or_else(|| {
                    "the surface indexes a value with something other than a constant".to_owned()
                })?;
                self.access_index(*base, index)
            }
            Expression::Swizzle {
                size,
                vector,
                pattern,
            } => self.swizzle(*vector, *size, pattern),
            Expression::ImageSample { image, .. } => self.image_sample(*image),
            Expression::Binary { op, left, right } => self.binary(*op, *left, *right),
            Expression::Math {
                fun,
                arg,
                arg1,
                arg2,
                ..
            } => self.math(*fun, *arg, *arg1, *arg2),
            Expression::Literal(literal) => self.literal(literal),
            Expression::Compose { ty, components } => self.compose(*ty, components),
            Expression::Splat { size, value } => self.splat(*size, *value),
            Expression::Constant(constant) => self.constant(*constant),
            other => Err(format!(
                "the surface uses {}, which the Blender mapping does not translate",
                expression_kind(other)
            )),
        }
    }

    /// Reads a value through a pointer expression.
    fn pointer(&mut self, pointer: Handle<Expression>) -> Result<Input, String> {
        let function = self.function;
        let Some((root, indices)) = access_chain(function, pointer) else {
            return Err("the surface reads a pointer the Blender mapping cannot follow".to_owned());
        };
        match &function.expressions[root] {
            Expression::GlobalVariable(global) => {
                let Some((member_index, component_indices)) = indices.split_first() else {
                    return self.material_member(*global, None);
                };
                let mut input = self.material_member(*global, Some(*member_index))?;
                for component_index in component_indices {
                    input = self.component(input, *component_index)?;
                }
                Ok(input)
            }
            Expression::LocalVariable(local) => {
                let Some(value) = self.local_values.get(local).copied() else {
                    return Err(
                        "the surface reads a local variable before it is written".to_owned()
                    );
                };
                let mut input = self.input(value)?;
                for component_index in &indices {
                    input = self.component(input, *component_index)?;
                }
                Ok(input)
            }
            other => Err(format!(
                "the surface reads through {}, which the Blender mapping does not translate",
                expression_kind(other)
            )),
        }
    }

    /// Reads one member of a value: a material parameter, or a component of a
    /// vector.
    fn access_index(&mut self, base: Handle<Expression>, index: u32) -> Result<Input, String> {
        if let Expression::GlobalVariable(global) = &self.function.expressions[base] {
            return self.material_member(*global, Some(index));
        }
        let input = self.input(base)?;
        self.component(input, index)
    }

    /// Produces the node for one member of the material's parameter block.
    fn material_member(
        &mut self,
        global: Handle<GlobalVariable>,
        member: Option<u32>,
    ) -> Result<Input, String> {
        if self.material_global != Some(global) {
            return Err(
                "the surface reads a uniform that is not the material's parameters".to_owned(),
            );
        }
        let Some(member_index) = member else {
            return Err("the surface reads the material's parameters as a whole".to_owned());
        };
        let global_type = self.module.global_variables[global].ty;
        let TypeInner::Struct { members, .. } = &self.module.types[global_type].inner else {
            return Err("the material's parameters are not a struct".to_owned());
        };
        let Some(member) = members.get(member_index as usize) else {
            return Err(format!(
                "the material's parameter block has no member {member_index}"
            ));
        };
        let Some(member_name) = &member.name else {
            return Err(format!(
                "the material's parameter block member {member_index} has no name"
            ));
        };
        let name = strip_cooked_suffix(member_name).to_owned();
        if let Some(input) = self.parameter_nodes.get(&name) {
            return Ok(*input);
        }
        let Some(slot) = self.shader.parameter_slots.get(&name) else {
            return Err(format!("the shader declares no parameter named `{name}`"));
        };
        let node = match slot.parameter_type {
            ShaderParameterType::Color => {
                let value = match self.parameters.get(&name) {
                    Some(MaterialParameter::Color(color)) => *color,
                    _ => [0.0, 0.0, 0.0],
                };
                Node::Rgb {
                    name: name.clone(),
                    value,
                }
            }
            ShaderParameterType::Scalar | ShaderParameterType::Bool => {
                let value = match self.parameters.get(&name) {
                    Some(MaterialParameter::Scalar(value)) => *value,
                    Some(MaterialParameter::Bool(value)) if *value => 1.0,
                    Some(MaterialParameter::Bool(_)) => 0.0,
                    _ => 0.0,
                };
                Node::Value {
                    name: name.clone(),
                    value,
                }
            }
        };
        let input = self.graph.push(node);
        self.parameter_nodes.insert(name, input);
        Ok(input)
    }

    /// Takes one channel of a value.
    fn component(&mut self, input: Input, index: u32) -> Result<Input, String> {
        let channel = match index {
            0 => Channel::Red,
            1 => Channel::Green,
            2 => Channel::Blue,
            other => return Err(format!("component {other} has no Blender channel")),
        };
        // A scalar parameter read at `.x` is the value itself; a Blender Value
        // node has no channels to separate.
        if index == 0 && matches!(self.graph.node_of(input), Some(Node::Value { .. })) {
            return Ok(input);
        }
        Ok(self.graph.push(Node::Component { input, channel }))
    }

    /// Translates a multi-component swizzle. Only the identity maps to a
    /// Principled input; anything else would silently drop or reorder channels.
    fn swizzle(
        &mut self,
        vector: Handle<Expression>,
        size: VectorSize,
        pattern: &[SwizzleComponent; 4],
    ) -> Result<Input, String> {
        let identity = match size {
            VectorSize::Bi => pattern[..2] == [SwizzleComponent::X, SwizzleComponent::Y],
            VectorSize::Tri => {
                pattern[..3]
                    == [
                        SwizzleComponent::X,
                        SwizzleComponent::Y,
                        SwizzleComponent::Z,
                    ]
            }
            VectorSize::Quad => {
                pattern[..4]
                    == [
                        SwizzleComponent::X,
                        SwizzleComponent::Y,
                        SwizzleComponent::Z,
                        SwizzleComponent::W,
                    ]
            }
        };
        if !identity {
            return Err(format!(
                "the surface swizzles a vector as {pattern:?}, which the Blender mapping does not translate"
            ));
        }
        self.input(vector)
    }

    /// Produces the image node for a texture the surface samples.
    fn image_sample(&mut self, image: Handle<Expression>) -> Result<Input, String> {
        let image = peel_load(self.function, image);
        let function = self.function;
        let Expression::GlobalVariable(global) = &function.expressions[image] else {
            return Err("the surface samples something that is not a bound texture".to_owned());
        };
        let Some(binding) = &self.module.global_variables[*global].binding else {
            return Err("the sampled texture has no binding".to_owned());
        };
        if binding.group != config::MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX {
            return Err(format!(
                "the surface samples a texture bound outside the material's texture group, at binding {}",
                binding.binding
            ));
        }
        let Some(slot) = self.texture_slots_by_binding.get(&binding.binding) else {
            return Err(format!(
                "the shader declares no texture slot at binding {}",
                binding.binding
            ));
        };
        if let Some(input) = self.texture_nodes.get(slot) {
            return Ok(*input);
        }
        let Some(resolved) = self.textures.get(slot) else {
            return Err(format!("no texture is resolved for slot `{slot}`"));
        };
        let input = self.graph.push(Node::Image {
            slot: slot.clone(),
            absolute_path: resolved.absolute_path.to_string_lossy().replace('\\', "/"),
            color_space: resolved.color_space,
        });
        self.texture_nodes.insert(slot.clone(), input);
        Ok(input)
    }

    /// Translates a binary operation.
    fn binary(
        &mut self,
        op: BinaryOperator,
        left: Handle<Expression>,
        right: Handle<Expression>,
    ) -> Result<Input, String> {
        match op {
            BinaryOperator::Multiply => {
                let first = self.input(left)?;
                let second = self.input(right)?;
                Ok(self.graph.push(Node::Multiply { first, second }))
            }
            BinaryOperator::Add => {
                let first = self.input(left)?;
                let second = self.input(right)?;
                Ok(self.graph.push(Node::Add { first, second }))
            }
            other => Err(format!(
                "the surface uses the `{other:?}` operator, which the Blender mapping does not translate"
            )),
        }
    }

    /// Translates a math function.
    fn math(
        &mut self,
        fun: MathFunction,
        arg: Handle<Expression>,
        arg1: Option<Handle<Expression>>,
        arg2: Option<Handle<Expression>>,
    ) -> Result<Input, String> {
        match fun {
            MathFunction::Saturate => {
                let input = self.input(arg)?;
                Ok(self.graph.push(Node::Saturate { input }))
            }
            MathFunction::Clamp => {
                let lower = arg1.and_then(|handle| self.constant_scalar(handle));
                let upper = arg2.and_then(|handle| self.constant_scalar(handle));
                if lower != Some(0.0) || upper != Some(1.0) {
                    return Err(
                        "the surface clamps to a range other than 0..=1, which the Blender mapping does not translate".to_owned()
                    );
                }
                let input = self.input(arg)?;
                Ok(self.graph.push(Node::Saturate { input }))
            }
            MathFunction::Normalize => {
                let input = self.normal_pattern(arg)?;
                Ok(self.graph.push(Node::NormalMap { input }))
            }
            other => Err(format!(
                "the surface uses the `{other:?}` function, which the Blender mapping does not translate"
            )),
        }
    }

    /// Recognizes `normalize(sample * 2 - 1)` and returns the sampled colour.
    fn normal_pattern(&mut self, arg: Handle<Expression>) -> Result<Input, String> {
        let function = self.function;
        let Expression::Binary {
            op: BinaryOperator::Subtract,
            left,
            right,
        } = &function.expressions[arg]
        else {
            return Err(
                "the surface builds a normal from something other than a tangent-space vector"
                    .to_owned(),
            );
        };
        if self.constant_vector3(*right) != Some([1.0, 1.0, 1.0]) {
            return Err("the surface builds a normal with an offset other than 1".to_owned());
        }
        let Expression::Binary {
            op: BinaryOperator::Multiply,
            left: sample,
            right: scale,
        } = &function.expressions[*left]
        else {
            return Err("the surface builds a normal without scaling a sampled vector".to_owned());
        };
        if self.constant_vector3(*scale) != Some([2.0, 2.0, 2.0]) {
            return Err("the surface builds a normal with a scale other than 2".to_owned());
        }
        self.input(*sample)
    }

    /// Produces a constant scalar node.
    fn literal(&mut self, literal: &Literal) -> Result<Input, String> {
        match literal {
            Literal::F32(value) => Ok(self.graph.push(Node::Value {
                name: String::new(),
                value: *value,
            })),
            Literal::AbstractFloat(value) => Ok(self.graph.push(Node::Value {
                name: String::new(),
                value: *value as f32,
            })),
            other => Err(format!(
                "the surface uses a {other:?} constant, which the Blender mapping does not translate"
            )),
        }
    }

    /// Produces a constant colour node from three scalar components.
    fn compose(
        &mut self,
        ty: naga::Handle<naga::Type>,
        components: &[Handle<Expression>],
    ) -> Result<Input, String> {
        if !matches!(
            &self.module.types[ty].inner,
            TypeInner::Vector {
                size: VectorSize::Tri,
                ..
            }
        ) {
            return Err("the surface builds a constant of a type other than a colour".to_owned());
        }
        if components.len() != 3 {
            return Err(
                "the surface builds a colour from the wrong number of components".to_owned(),
            );
        }
        let mut value = [0.0f32; 3];
        for (slot, component) in value.iter_mut().zip(components) {
            let Some(scalar) = self.constant_scalar(*component) else {
                return Err(
                    "the surface builds a colour from a component that is not a constant"
                        .to_owned(),
                );
            };
            *slot = scalar;
        }
        Ok(self.graph.push(Node::Rgb {
            name: String::new(),
            value,
        }))
    }

    /// Produces a constant colour node from one value repeated three times.
    fn splat(&mut self, size: VectorSize, value: Handle<Expression>) -> Result<Input, String> {
        if size != VectorSize::Tri {
            return Err("the surface builds a constant vector that is not a colour".to_owned());
        }
        let Some(scalar) = self.constant_scalar(value) else {
            return Err(
                "the surface builds a colour from something that is not a constant".to_owned(),
            );
        };
        Ok(self.graph.push(Node::Rgb {
            name: String::new(),
            value: [scalar; 3],
        }))
    }

    /// Produces a constant node for a named constant.
    fn constant(&mut self, constant: Handle<naga::Constant>) -> Result<Input, String> {
        let init = self.module.constants[constant].init;
        let module = self.module;
        match &module.global_expressions[init] {
            Expression::Literal(literal) => {
                let literal = *literal;
                self.literal(&literal)
            }
            Expression::Splat { size, value } => self.splat(*size, *value),
            Expression::Compose { ty, components } => self.compose(*ty, components),
            other => Err(format!(
                "the surface uses {}, which the Blender mapping does not translate",
                expression_kind(other)
            )),
        }
    }

    /// Reads a scalar constant out of the function's expressions.
    fn constant_scalar(&self, handle: Handle<Expression>) -> Option<f32> {
        self.scalar_in(&self.function.expressions, handle, 0)
    }

    /// Reads a scalar constant out of `arena`.
    fn scalar_in(
        &self,
        arena: &Arena<Expression>,
        handle: Handle<Expression>,
        depth: u32,
    ) -> Option<f32> {
        if depth > MAXIMUM_CONSTANT_DEPTH {
            return None;
        }
        match &arena[handle] {
            Expression::Literal(Literal::F32(value)) => Some(*value),
            Expression::Literal(Literal::AbstractFloat(value)) => Some(*value as f32),
            Expression::Splat { value, .. } => self.scalar_in(arena, *value, depth + 1),
            Expression::Constant(constant) => {
                let init = self.module.constants[*constant].init;
                self.scalar_in(&self.module.global_expressions, init, depth + 1)
            }
            _ => None,
        }
    }

    /// Reads a colour constant, or a scalar broadcast to three components.
    fn constant_vector3(&self, handle: Handle<Expression>) -> Option<[f32; 3]> {
        self.vector3_in(&self.function.expressions, handle, 0)
    }

    /// Reads a colour constant out of `arena`.
    fn vector3_in(
        &self,
        arena: &Arena<Expression>,
        handle: Handle<Expression>,
        depth: u32,
    ) -> Option<[f32; 3]> {
        if depth > MAXIMUM_CONSTANT_DEPTH {
            return None;
        }
        match &arena[handle] {
            Expression::Literal(Literal::F32(value)) => Some([*value; 3]),
            Expression::Literal(Literal::AbstractFloat(value)) => Some([*value as f32; 3]),
            Expression::Splat { size, value } => {
                if *size != VectorSize::Tri {
                    return None;
                }
                let scalar = self.scalar_in(arena, *value, depth + 1)?;
                Some([scalar; 3])
            }
            Expression::Compose { components, .. } => {
                if components.len() != 3 {
                    return None;
                }
                let mut value = [0.0f32; 3];
                for (slot, component) in value.iter_mut().zip(components) {
                    *slot = self.scalar_in(arena, *component, depth + 1)?;
                }
                Some(value)
            }
            Expression::Constant(constant) => {
                let init = self.module.constants[*constant].init;
                self.vector3_in(&self.module.global_expressions, init, depth + 1)
            }
            _ => None,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns how Blender must read a texture slot's texels.
pub fn color_space_of(kind: ShaderTextureKind) -> ImageColorSpace {
    match kind {
        ShaderTextureKind::Color => ImageColorSpace::Srgb,
        _ => ImageColorSpace::NonColor,
    }
}

/// Finds the call to the surface shading function in the entry point.
pub fn surface_call<'a>(module: &'a Module, function: &'a Function) -> Option<&'a Statement> {
    function.body.iter().find(|statement| match statement {
        Statement::Call {
            function: callee, ..
        } => {
            let name = module.functions[*callee]
                .name
                .as_deref()
                .unwrap_or_default();
            strip_cooked_suffix(name) == SURFACE_SHADING_FUNCTION
        }
        _ => false,
    })
}

/// Maps every surface field the entry point sets to the expression it is set
/// from.
pub fn surface_field_expressions(
    module: &Module,
    function: &Function,
    argument: Handle<Expression>,
) -> Result<HashMap<String, Handle<Expression>>, TranslationError> {
    // Step 1: The call reads the surface by value, so follow the load to the
    // place it was built in.
    let place = peel_load(function, argument);
    let mut fields = HashMap::new();
    match &function.expressions[place] {
        Expression::Compose { ty, components } => {
            let members = struct_members(module, *ty)?;
            for (member, component) in members.iter().zip(components) {
                if let Some(name) = &member.name {
                    fields.insert(strip_cooked_suffix(name).to_owned(), *component);
                }
            }
        }
        Expression::LocalVariable(local) => {
            let value_type = function.local_variables[*local].ty;
            let members = struct_members(module, value_type)?;
            for statement in function.body.iter() {
                let Statement::Store { pointer, value } = statement else {
                    continue;
                };
                let Some((store_local, member_index)) = member_pointer(function, *pointer) else {
                    continue;
                };
                if store_local != *local {
                    continue;
                }
                if let Some(member) = members.get(member_index as usize) {
                    if let Some(name) = &member.name {
                        fields.insert(strip_cooked_suffix(name).to_owned(), *value);
                    }
                }
            }
        }
        _ => {
            return Err(TranslationError::UnsupportedSurface(
                "the shading call is not given a surface built in the fragment".to_owned(),
            ));
        }
    }
    Ok(fields)
}

/// Returns a struct's members.
pub fn struct_members(
    module: &Module,
    ty: naga::Handle<naga::Type>,
) -> Result<&[naga::StructMember], TranslationError> {
    match &module.types[ty].inner {
        TypeInner::Struct { members, .. } => Ok(members.as_slice()),
        _ => Err(TranslationError::UnsupportedSurface(
            "the shade function does not take a struct".to_owned(),
        )),
    }
}

/// Reads a store's pointer as a local variable and a member index.
pub fn member_pointer(
    function: &Function,
    pointer: Handle<Expression>,
) -> Option<(Handle<naga::LocalVariable>, u32)> {
    let (root, indices) = access_chain(function, pointer)?;
    let Expression::LocalVariable(local) = &function.expressions[root] else {
        return None;
    };
    let [member_index] = indices.as_slice() else {
        return None;
    };
    Some((*local, *member_index))
}

/// Returns the place a pointer starts at and the constant indices applied to
/// it, outermost last.
///
/// A material parameter read is a chain like `material.pbr_roughness.x`:
/// two indices over the parameter block's global, the member and then the
/// component.
pub fn access_chain(
    function: &Function,
    pointer: Handle<Expression>,
) -> Option<(Handle<Expression>, Vec<u32>)> {
    let mut indices = Vec::new();
    let mut current = pointer;
    loop {
        match &function.expressions[current] {
            Expression::AccessIndex { base, index } => {
                indices.push(*index);
                current = *base;
            }
            Expression::Access { base, index } => {
                indices.push(constant_u32(&function.expressions, *index)?);
                current = *base;
            }
            _ => break,
        }
    }
    indices.reverse();
    Some((current, indices))
}

/// Reads an unsigned integer constant out of `arena`.
pub fn constant_u32(arena: &Arena<Expression>, handle: Handle<Expression>) -> Option<u32> {
    match &arena[handle] {
        Expression::Literal(Literal::U32(value)) => Some(*value),
        Expression::Literal(Literal::AbstractInt(value)) => u32::try_from(*value).ok(),
        _ => None,
    }
}

/// Follows value loads to the place they read.
pub fn peel_load(function: &Function, mut handle: Handle<Expression>) -> Handle<Expression> {
    while let Expression::Load { pointer } = &function.expressions[handle] {
        handle = *pointer;
    }
    handle
}

/// Strips the suffix the shader cook adds to every name it emits.
pub fn strip_cooked_suffix(name: &str) -> &str {
    let Some((head, tail)) = name.rsplit_once('_') else {
        return name;
    };
    if !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit()) {
        head
    } else {
        name
    }
}

/// Names an expression kind for a refusal.
pub fn expression_kind(expression: &Expression) -> &'static str {
    match expression {
        Expression::Literal(_) => "a literal",
        Expression::Constant(_) => "a named constant",
        Expression::ZeroValue(_) => "a zero value",
        Expression::Compose { .. } => "a composite",
        Expression::Access { .. } | Expression::AccessIndex { .. } => "an access",
        Expression::Splat { .. } => "a splat",
        Expression::Swizzle { .. } => "a swizzle",
        Expression::FunctionArgument(_) => "a function argument",
        Expression::GlobalVariable(_) => "a global variable",
        Expression::LocalVariable(_) => "a local variable",
        Expression::Load { .. } => "a load",
        Expression::ImageSample { .. } => "an image sample",
        Expression::ImageLoad { .. } => "an image load",
        Expression::Binary { .. } => "a binary operation",
        Expression::Unary { .. } => "a unary operation",
        Expression::Math { .. } => "a math function",
        Expression::As { .. } => "a cast",
        Expression::CallResult(_) => "a function call",
        _ => "a construct",
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_master_renderer_data::config::pbr_pipeline::{neutral_parameters, PBR_FRAGMENT};
    use pill_master_renderer_data::{ShaderParameterSlot, ShaderTextureSlot};

    /// A shader with the shipped PBR shader's slots, over `fragment`.
    fn pbr_slots_shader(fragment: &str) -> Shader {
        Shader::new("pbr")
            .with_wgsl("", fragment)
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

    /// The shipped PBR shader, with the slots its metadata declares.
    fn pbr_shader() -> Shader {
        pbr_slots_shader(PBR_FRAGMENT.embedded_source)
    }

    /// The four slots resolved to made-up files.
    fn pbr_textures() -> BTreeMap<String, ResolvedTexture> {
        let mut textures = BTreeMap::new();
        for (slot, color_space) in [
            ("base_color", ImageColorSpace::Srgb),
            ("normal", ImageColorSpace::NonColor),
            ("metallic_roughness", ImageColorSpace::Srgb),
            ("emissive", ImageColorSpace::Srgb),
        ] {
            textures.insert(
                slot.to_owned(),
                ResolvedTexture {
                    absolute_path: PathBuf::from(format!("C:/project/res/textures/{slot}.png")),
                    color_space,
                },
            );
        }
        textures
    }

    /// The node an input names.
    fn node(graph: &MaterialGraph, input: Input) -> &Node {
        graph.node_of(input).expect("the input names a node")
    }

    /// Asserts the input is the image node of `slot`.
    fn assert_image(graph: &MaterialGraph, input: Input, slot: &str) {
        match node(graph, input) {
            Node::Image {
                slot: actual_slot,
                absolute_path,
                ..
            } => {
                assert_eq!(actual_slot, slot);
                assert!(
                    absolute_path.ends_with(&format!("{slot}.png")),
                    "the image node names the resolved file, got `{absolute_path}`"
                );
            }
            other => panic!("expected the `{slot}` image, got {other:?}"),
        }
    }

    /// Asserts the input is a parameter node with `name`.
    fn assert_parameter(graph: &MaterialGraph, input: Input, name: &str) {
        let actual_name = match node(graph, input) {
            Node::Value { name, .. } | Node::Rgb { name, .. } => name,
            other => panic!("expected the `{name}` parameter, got {other:?}"),
        };
        assert_eq!(actual_name, name);
    }

    /// The shipped PBR fragment translates into the five Principled inputs the
    /// material's surface describes.
    #[test]
    fn the_shipped_pbr_fragment_translates_into_the_five_principled_inputs() {
        let shader = pbr_shader();
        let textures = pbr_textures();
        let graph = translate_material(&shader, &neutral_parameters(), &textures)
            .expect("the shipped fragment is in the shape the translator reads");
        assert!(
            graph.refusals.is_empty(),
            "the shipped fragment refuses no field: {:?}",
            graph.refusals
        );

        // Base colour: the base-colour image times the `pbr_base` parameter.
        let Node::Multiply { first, second } = node(&graph, graph.principled.base_color) else {
            panic!(
                "base colour is a product, got {:?}",
                node(&graph, graph.principled.base_color)
            );
        };
        assert_image(&graph, *first, "base_color");
        assert_parameter(&graph, *second, "pbr_base");

        // Roughness: the green channel of the packing map, clamped, times the
        // `pbr_roughness` parameter.
        let Node::Saturate { input } = node(&graph, graph.principled.roughness) else {
            panic!(
                "roughness is clamped, got {:?}",
                node(&graph, graph.principled.roughness)
            );
        };
        let Node::Multiply { first, second } = node(&graph, *input) else {
            panic!("roughness is a product, got {:?}", node(&graph, *input));
        };
        let Node::Component {
            input: sample,
            channel,
        } = node(&graph, *first)
        else {
            panic!(
                "roughness reads one channel, got {:?}",
                node(&graph, *first)
            );
        };
        assert_eq!(*channel, Channel::Green);
        assert_image(&graph, *sample, "metallic_roughness");
        assert_parameter(&graph, *second, "pbr_roughness");

        // Metallic: the blue channel instead.
        let Node::Saturate { input } = node(&graph, graph.principled.metallic) else {
            panic!(
                "metallic is clamped, got {:?}",
                node(&graph, graph.principled.metallic)
            );
        };
        let Node::Multiply { first, second } = node(&graph, *input) else {
            panic!("metallic is a product, got {:?}", node(&graph, *input));
        };
        let Node::Component {
            input: sample,
            channel,
        } = node(&graph, *first)
        else {
            panic!("metallic reads one channel, got {:?}", node(&graph, *first));
        };
        assert_eq!(*channel, Channel::Blue);
        assert_image(&graph, *sample, "metallic_roughness");
        assert_parameter(&graph, *second, "pbr_metallic");

        // Normal: the normal map's image through a Normal Map node.
        let Node::NormalMap { input } = node(&graph, graph.principled.normal) else {
            panic!(
                "the normal is a normal map, got {:?}",
                node(&graph, graph.principled.normal)
            );
        };
        assert_image(&graph, *input, "normal");

        // Emission: the emissive image times the `pbr_emissive` parameter.
        let Node::Multiply { first, second } = node(&graph, graph.principled.emission) else {
            panic!(
                "emission is a product, got {:?}",
                node(&graph, graph.principled.emission)
            );
        };
        assert_image(&graph, *first, "emissive");
        assert_parameter(&graph, *second, "pbr_emissive");
    }

    /// A surface field built from a function outside the subset refuses that
    /// field and leaves the other four connected.
    #[test]
    fn an_unsupported_expression_refuses_only_its_field() {
        let shader = Shader::new("refusing")
            .with_wgsl("", REFUSING_FRAGMENT)
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
            ])
            .build()
            .expect("the fixture is text");

        let graph = translate_material(&shader, &neutral_parameters(), &pbr_textures())
            .expect("the fixture builds a surface");

        assert_eq!(
            graph.refusals.len(),
            1,
            "one field is refused: {:?}",
            graph.refusals
        );
        assert_eq!(graph.refusals[0].field, FIELD_ROUGHNESS);
        assert!(
            graph.refusals[0].reason.contains("Abs"),
            "the refusal names the construct, got `{}`",
            graph.refusals[0].reason
        );
        assert_eq!(graph.principled.roughness, Input::Unconnected);
        assert_ne!(graph.principled.base_color, Input::Unconnected);
        assert_ne!(graph.principled.metallic, Input::Unconnected);
        assert_ne!(graph.principled.normal, Input::Unconnected);
        assert_ne!(graph.principled.emission, Input::Unconnected);
    }

    /// A fragment that never calls the shading function is reported as a
    /// missing surface, which the fallback mapping answers.
    #[test]
    fn a_fragment_without_the_shading_call_reports_a_missing_surface() {
        let shader = Shader::new("plain")
            .with_wgsl(
                "",
                "@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }\n",
            )
            .build()
            .expect("the fixture is text");

        let translated = translate_material(&shader, &BTreeMap::new(), &BTreeMap::new());

        assert_eq!(translated, Err(TranslationError::MissingSurface));
    }

    /// A fragment with no surface still gets the five inputs, filled from the
    /// slots whose names the renderer standardised.
    #[test]
    fn a_fragment_without_the_shading_call_yields_the_named_inputs() {
        let shader = pbr_slots_shader(PLAIN_FRAGMENT);
        let textures = pbr_textures();
        let parameters = neutral_parameters();
        assert_eq!(
            translate_material(&shader, &parameters, &textures),
            Err(TranslationError::MissingSurface)
        );

        let graph = translate_material_fallback(&shader, &parameters, &textures);
        assert!(
            graph.refusals.is_empty(),
            "every shipped slot has a standard name: {:?}",
            graph.refusals
        );

        // Base colour: the base-colour image times the `pbr_base` parameter.
        let Node::Multiply { first, second } = node(&graph, graph.principled.base_color) else {
            panic!(
                "base colour is a product, got {:?}",
                node(&graph, graph.principled.base_color)
            );
        };
        assert_image(&graph, *first, "base_color");
        assert_parameter(&graph, *second, "pbr_base");

        // Roughness: the packing map's green channel times its parameter.
        let Node::Multiply { first, second } = node(&graph, graph.principled.roughness) else {
            panic!(
                "roughness is a product, got {:?}",
                node(&graph, graph.principled.roughness)
            );
        };
        let Node::Component {
            input: sample,
            channel,
        } = node(&graph, *first)
        else {
            panic!(
                "roughness reads one channel, got {:?}",
                node(&graph, *first)
            );
        };
        assert_eq!(*channel, Channel::Green);
        assert_image(&graph, *sample, "metallic_roughness");
        assert_parameter(&graph, *second, "pbr_roughness");

        // Metallic: the blue channel instead.
        let Node::Multiply { first, second } = node(&graph, graph.principled.metallic) else {
            panic!(
                "metallic is a product, got {:?}",
                node(&graph, graph.principled.metallic)
            );
        };
        let Node::Component {
            input: sample,
            channel,
        } = node(&graph, *first)
        else {
            panic!("metallic reads one channel, got {:?}", node(&graph, *first));
        };
        assert_eq!(*channel, Channel::Blue);
        assert_image(&graph, *sample, "metallic_roughness");
        assert_parameter(&graph, *second, "pbr_metallic");

        // Normal: the normal map's image through a Normal Map node.
        let Node::NormalMap { input } = node(&graph, graph.principled.normal) else {
            panic!(
                "the normal is a normal map, got {:?}",
                node(&graph, graph.principled.normal)
            );
        };
        assert_image(&graph, *input, "normal");

        // Emission: the emissive image times the `pbr_emissive` parameter.
        let Node::Multiply { first, second } = node(&graph, graph.principled.emission) else {
            panic!(
                "emission is a product, got {:?}",
                node(&graph, graph.principled.emission)
            );
        };
        assert_image(&graph, *first, "emissive");
        assert_parameter(&graph, *second, "pbr_emissive");
    }

    /// A slot with a project's own name is reported instead of guessed at, and
    /// the inputs it would have filled fall back to what is left.
    #[test]
    fn a_fragment_with_custom_slots_reports_the_slots_it_cannot_place() {
        let shader = Shader::new("custom")
            .with_wgsl("", PLAIN_FRAGMENT)
            .with_parameter_slots([
                ShaderParameterSlot {
                    name: "pbr_base".to_owned(),
                    parameter_type: ShaderParameterType::Color,
                },
                ShaderParameterSlot {
                    name: "detail_strength".to_owned(),
                    parameter_type: ShaderParameterType::Scalar,
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
                    name: "detail_map".to_owned(),
                    kind: ShaderTextureKind::Color,
                    texture_binding: 2,
                    sampler_binding: 3,
                },
            ])
            .build()
            .expect("the fixture is text");
        let mut parameters = neutral_parameters();
        parameters.insert("detail_strength".to_owned(), MaterialParameter::Scalar(0.5));
        let mut textures = BTreeMap::new();
        for slot in ["base_color", "detail_map"] {
            textures.insert(
                slot.to_owned(),
                ResolvedTexture {
                    absolute_path: PathBuf::from(format!("C:/project/res/textures/{slot}.png")),
                    color_space: ImageColorSpace::Srgb,
                },
            );
        }

        let graph = translate_material_fallback(&shader, &parameters, &textures);

        let refused: Vec<&str> = graph
            .refusals
            .iter()
            .map(|refusal| refusal.field.as_str())
            .collect();
        assert_eq!(
            refused,
            vec!["detail_map", "detail_strength"],
            "both custom slots are reported: {:?}",
            graph.refusals
        );
        // The standard slots that do exist are still placed, and an input with
        // no packing map keeps the parameter alone.
        assert_ne!(graph.principled.base_color, Input::Unconnected);
        assert_parameter(&graph, graph.principled.roughness, "pbr_roughness");
        assert_parameter(&graph, graph.principled.metallic, "pbr_metallic");
        assert_eq!(graph.principled.normal, Input::Unconnected);
    }

    /// The cook's numeric suffix comes off a name and nothing else does.
    #[test]
    fn the_cooked_suffix_is_stripped_from_names() {
        assert_eq!(
            strip_cooked_suffix("shade_pbr_surface_0"),
            "shade_pbr_surface"
        );
        assert_eq!(strip_cooked_suffix("albedo_2"), "albedo");
        assert_eq!(strip_cooked_suffix("pbr_base"), "pbr_base");
        assert_eq!(strip_cooked_suffix("offset_"), "offset_");
        assert_eq!(strip_cooked_suffix("a_1_b"), "a_1_b");
    }

    /// A valid fragment that builds no surface, so the fallback answers it.
    const PLAIN_FRAGMENT: &str = r#"
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}
"#;

    /// A surface built and shaded as the cook writes it, with one field
    /// (`abs`) outside the supported subset.
    const REFUSING_FRAGMENT: &str = r#"
struct PbrSurface_0 {
    albedo_2 : vec3<f32>,
    roughness_4 : f32,
    metallic_1 : f32,
    tangent_normal_0 : vec3<f32>,
    emissive_0 : vec3<f32>,
};

struct MaterialParams_0 {
    pbr_base_0 : vec4<f32>,
    pbr_roughness_0 : vec4<f32>,
    pbr_metallic_0 : vec4<f32>,
    pbr_emissive_0 : vec4<f32>,
};

@group(2) @binding(0) var<uniform> material_0 : MaterialParams_0;
@group(3) @binding(0) var base_color_texture_0 : texture_2d<f32>;
@group(3) @binding(1) var base_color_sampler_0 : sampler;
@group(3) @binding(2) var normal_texture_0 : texture_2d<f32>;
@group(3) @binding(3) var normal_sampler_0 : sampler;

fn shade_pbr_surface_0(surface_0 : PbrSurface_0) -> vec3<f32>
{
    return surface_0.albedo_2;
}

@fragment
fn fs_main() -> @location(0) vec4<f32>
{
    var base_sample_0 : vec4<f32> = textureSample(base_color_texture_0, base_color_sampler_0, vec2<f32>(0.0, 0.0));
    var normal_sample_0 : vec3<f32> = textureSample(normal_texture_0, normal_sampler_0, vec2<f32>(0.0, 0.0)).xyz;
    var surface_1 : PbrSurface_0;
    surface_1.albedo_2 = base_sample_0.xyz * material_0.pbr_base_0.xyz;
    surface_1.roughness_4 = abs(base_sample_0.x - 0.5);
    surface_1.metallic_1 = saturate(0.5);
    surface_1.tangent_normal_0 = normalize(normal_sample_0 * vec3<f32>(2.0) - vec3<f32>(1.0));
    surface_1.emissive_0 = vec3<f32>(0.0);
    return vec4<f32>(shade_pbr_surface_0(surface_1), 1.0);
}
"#;
}
