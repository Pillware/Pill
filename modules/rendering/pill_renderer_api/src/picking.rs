//! Selection outlines and picking: which entities a renderer outlines, and
//! which entity or gizmo handle is under a pixel.
//!
//! # Responsibilities
//!
//! - Name the entities to outline ([`SelectionOutlines`], a world resource).
//! - Ask what is under a pixel of a surface ([`PickRequests`]) and receive the
//!   answer frames later ([`PickResults`]), as plain data.
//!
//! # Design
//!
//! A pick reads the pick buffer back from the GPU, and waiting for that answer
//! never blocks a frame. A request made now is answered once the GPU has
//! finished with it, a frame or more later. Each request carries an id, so its
//! answer can be matched however late it arrives. The pick buffer also draws
//! the gizmo handles over the scene, with the same lines and triangles that
//! [`pill_engine::DebugDraw::gizmo_line`] and
//! [`pill_engine::DebugDraw::gizmo_triangle`] draw, so a pick finds a handle
//! that lies in front of an entity.

// External crates
use pill_engine::{Entity, Resource};

// Current crate
use crate::SurfaceId;

// =============================================================================
// Constants
// =============================================================================

/// What one pixel of the pick buffer holds: `0` for nothing, `1 + index` for
/// the instance at `index` of the frame's instance list, and a gizmo handle
/// with the top bit set.
pub const PICK_HANDLE_BIT: u32 = 0x8000_0000;

// =============================================================================
// SelectionOutlines
// =============================================================================

/// The entities a renderer outlines, in one colour.
#[derive(Debug, Clone)]
pub struct SelectionOutlines {
    /// The entities to outline. An entity that draws nothing shows no outline.
    pub entities: Vec<Entity>,
    /// The outline colour, linear RGBA.
    pub color: [f32; 4],
}

impl Default for SelectionOutlines {
    fn default() -> Self {
        Self {
            entities: Vec::new(),
            color: [1.0, 0.55, 0.1, 1.0],
        }
    }
}

impl Resource for SelectionOutlines {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::picking::SelectionOutlines")
    }
}

// =============================================================================
// PickRequest
// =============================================================================

/// One request to find what is under a pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickRequest {
    /// The id its answer carries.
    pub id: u64,
    /// The surface the pixel belongs to.
    pub surface: SurfaceId,
    /// Physical pixels from the surface's left edge.
    pub x: u32,
    /// Physical pixels from the surface's top edge.
    pub y: u32,
}

// =============================================================================
// PickHit
// =============================================================================

/// What a pick found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickHit {
    /// Nothing is drawn there, the pixel is outside the surface, or the
    /// renderer could not read the pick buffer back.
    Nothing,
    /// The entity whose mesh is nearest at that pixel.
    Entity(Entity),
    /// The gizmo handle drawn there.
    Handle(u32),
}

// =============================================================================
// PickResult
// =============================================================================

/// The answer to one [`PickRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickResult {
    /// The request it answers.
    pub request: PickRequest,
    /// What is drawn there.
    pub hit: PickHit,
}

// =============================================================================
// PickRequests
// =============================================================================

/// The picks asked for and not yet handed to a renderer.
///
/// It is a world resource. A frontend or an editor tool adds requests, the
/// next frame carries them to the renderer, and the answers arrive in
/// [`PickResults`].
#[derive(Debug, Clone, Default)]
pub struct PickRequests {
    /// Requests waiting for the next frame.
    pub pending: Vec<PickRequest>,
    /// The id the next request gets.
    next_id: u64,
}

impl PickRequests {
    /// Asks what is under pixel (`x`, `y`) of `surface`, and returns the id
    /// the answer will carry.
    pub fn request(&mut self, surface: SurfaceId, x: u32, y: u32) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.pending.push(PickRequest { id, surface, x, y });
        id
    }
}

impl Resource for PickRequests {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::picking::PickRequests")
    }
}

// =============================================================================
// PickResults
// =============================================================================

/// The answers renderers delivered and nobody took yet.
#[derive(Debug, Clone, Default)]
pub struct PickResults {
    /// Answers in the order the renderers delivered them.
    pub results: Vec<PickResult>,
}

impl PickResults {
    /// Takes every answer delivered so far.
    pub fn take(&mut self) -> Vec<PickResult> {
        std::mem::take(&mut self.results)
    }
}

impl Resource for PickResults {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::picking::PickResults")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Decodes one pick-buffer value against the instance entities of the frame
/// that drew it.
///
/// `instance_entities[index]` is the entity of the instance the buffer numbers
/// `index + 1`. A number past the end of the list answers [`PickHit::Nothing`].
pub fn decode_pick_value(value: u32, instance_entities: &[Entity]) -> PickHit {
    if value == 0 {
        PickHit::Nothing
    } else if value & PICK_HANDLE_BIT != 0 {
        PickHit::Handle(value & !PICK_HANDLE_BIT)
    } else {
        instance_entities
            .get(value as usize - 1)
            .map_or(PickHit::Nothing, |entity| PickHit::Entity(*entity))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Request ids count up, so an answer can be matched to its request.
    #[test]
    fn request_ids_are_unique() {
        let mut requests = PickRequests::default();
        let first = requests.request(SurfaceId::PRIMARY, 1, 2);
        let second = requests.request(SurfaceId(1), 3, 4);
        assert_ne!(first, second);
        assert_eq!(requests.pending.len(), 2);
    }

    /// Zero is nothing, the top bit marks a handle, and any other value is an
    /// instance position.
    #[test]
    fn pick_values_decode() {
        let mut world = pill_engine::World::new();
        let entity = world.reserve_entity();
        assert_eq!(decode_pick_value(0, &[entity]), PickHit::Nothing);
        assert_eq!(decode_pick_value(1, &[entity]), PickHit::Entity(entity));
        assert_eq!(decode_pick_value(2, &[entity]), PickHit::Nothing);
        assert_eq!(
            decode_pick_value(PICK_HANDLE_BIT | 42, &[entity]),
            PickHit::Handle(42)
        );
    }
}
