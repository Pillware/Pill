//! The Blender node graph a material's surface translates into.
//!
//! # Responsibilities
//!
//! - Describe the nodes the link asks Blender to build, as plain data: a
//!   Principled BSDF's five inputs and the nodes that feed them.
//! - Name the nodes that carry a material parameter or a texture slot, so a
//!   later update can address a value or an image by name.
//!
//! # Design
//!
//! The graph is an arena: a node refers to an earlier node by its index in
//! [`MaterialGraph::nodes`], and the serializer keeps that order, so the
//! Blender side can build the list in one pass. Nothing here knows about
//! Blender: the add-on maps a [`Node`] to `bpy` node types, and the editor
//! never imports a Blender Python API.
//!
//! Only the five surface fields' expressions are represented. A construct
//! outside the supported subset is not approximated; the translator records a
//! [`Refusal`] for that field instead, so a wrong node never silently changes
//! how the material looks.

// External crates
use serde::{Deserialize, Serialize};

// =============================================================================
// Types
// =============================================================================

/// One input of a node, or of the Principled BSDF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    /// The output of the node at this index in [`MaterialGraph::nodes`].
    Node(usize),
    /// Nothing is connected, so Blender's default for the socket applies.
    #[default]
    Unconnected,
}

/// One channel of a colour input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    /// The first channel.
    Red,
    /// The second channel.
    Green,
    /// The third channel.
    Blue,
}

/// How Blender must read a texture's texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageColorSpace {
    /// Colour data, read as sRGB so sampling produces linear light.
    Srgb,
    /// Data that is not a colour, read exactly as stored: normal maps and
    /// masks.
    NonColor,
}

/// One node of the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// A scalar value. `name` is the material parameter it carries, or empty
    /// for a constant the translator produced.
    Value {
        /// Parameter slot name, or empty for a constant.
        name: String,
        /// The value.
        value: f32,
    },
    /// An RGB colour. `name` is the parameter slot name, or empty.
    Rgb {
        /// Parameter slot name, or empty for a constant.
        name: String,
        /// The colour, linear.
        value: [f32; 3],
    },
    /// An image loaded from an absolute path.
    Image {
        /// Texture slot name.
        slot: String,
        /// The file to load.
        absolute_path: String,
        /// How to read its texels.
        color_space: ImageColorSpace,
    },
    /// One channel of a colour input.
    Component {
        /// The colour.
        input: Input,
        /// The channel to take.
        channel: Channel,
    },
    /// The product of two inputs.
    Multiply {
        /// First factor.
        first: Input,
        /// Second factor.
        second: Input,
    },
    /// The sum of two inputs.
    Add {
        /// First term.
        first: Input,
        /// Second term.
        second: Input,
    },
    /// The input clamped to `0..=1`.
    Saturate {
        /// The value to clamp.
        input: Input,
    },
    /// A tangent-space normal, ready for a Normal Map node.
    NormalMap {
        /// The normal map's colour input.
        input: Input,
    },
}

/// The Principled BSDF inputs the graph feeds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrincipledInputs {
    /// Albedo.
    pub base_color: Input,
    /// Perceptual roughness.
    pub roughness: Input,
    /// Metalness.
    pub metallic: Input,
    /// Tangent-space normal.
    pub normal: Input,
    /// Emitted colour.
    pub emission: Input,
}

/// One input the translator could not fill as nodes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The surface field, or - for the slot-name fallback - the slot that has
    /// no standard name to be placed under.
    pub field: String,
    /// Why it was refused.
    pub reason: String,
}

/// A material's Blender node graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialGraph {
    /// What the Principled BSDF's five inputs are fed from.
    pub principled: PrincipledInputs,
    /// The nodes, in an order where a node's inputs come before it.
    pub nodes: Vec<Node>,
    /// The surface fields that could not be translated.
    pub refusals: Vec<Refusal>,
}

impl MaterialGraph {
    /// Appends `node` and returns an [`Input`] naming it.
    pub fn push(&mut self, node: Node) -> Input {
        self.nodes.push(node);
        Input::Node(self.nodes.len() - 1)
    }

    /// Returns the node an [`Input`] names, when it names one.
    pub fn node_of(&self, input: Input) -> Option<&Node> {
        match input {
            Input::Node(index) => self.nodes.get(index),
            Input::Unconnected => None,
        }
    }
}
