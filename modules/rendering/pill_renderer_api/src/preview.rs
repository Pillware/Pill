//! Asset previews: what an editor asks a renderer to draw
//! off-screen, and the pixels it gets back.
//!
//! # Responsibilities
//!
//! - Name the asset one preview draws ([`PreviewAsset`]) and the request that
//!   carries it ([`PreviewRequest`], [`PreviewRequests`]).
//! - Carry the pixels back ([`PreviewImage`], [`PreviewResults`]).
//!
//! # Design
//!
//! A preview is asked for through the world, the same way a pick is: the
//! editor writes a request into [`PreviewRequests`], the renderer draws one
//! request per frame into its own small target and reads the pixels back over
//! the following frames, and the frontend moves them into
//! [`PreviewResults`]. Nothing is written to disk, and the editor needs no GPU
//! device of its own.

// External crates
use pill_engine::{Resource, World};

// =============================================================================
// PreviewAsset
// =============================================================================

/// The asset one preview draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewAsset {
    /// A 2D texture file, drawn as a quad that keeps its aspect.
    ///
    /// The file is named instead of looked up as an asset. A project imports
    /// its textures under names of its own and may not have loaded one at
    /// all, so a guid cannot lead to the pixels of a tile. Decoding the file
    /// also means a texture that was imported again shows its new pixels.
    Texture {
        /// The file to decode, as the frontend resolves it (absolute).
        path: String,
    },
    /// A material, drawn on a preview sphere.
    Material {
        /// The asset's guid, in hex.
        guid: String,
    },
    /// A mesh, drawn with the renderer's own default shading.
    ///
    /// Named by its file for the same reason a texture is: the project may
    /// have loaded it under a name of its own, or not at all.
    Mesh {
        /// The file to decode, as the frontend resolves it (absolute).
        path: String,
    },
}

// =============================================================================
// PreviewRequest
// =============================================================================

/// One preview asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRequest {
    /// The id that ties the answer to this request.
    pub id: u64,
    /// The asset to draw.
    pub asset: PreviewAsset,
}

// =============================================================================
// PreviewRequests
// =============================================================================

/// The previews waiting for the renderer.
#[derive(Default)]
pub struct PreviewRequests {
    /// Every request asked for since the last frame.
    pub pending: Vec<PreviewRequest>,
    /// The next id [`Self::request`] hands out.
    next_request_id: u64,
}

impl PreviewRequests {
    /// Queues one preview of `asset` and returns the id its answer carries.
    pub fn request(&mut self, asset: PreviewAsset) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.pending.push(PreviewRequest { id, asset });
        id
    }
}

impl Resource for PreviewRequests {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::preview::PreviewRequests")
    }
}

// =============================================================================
// PreviewImage
// =============================================================================

/// One preview that came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewImage {
    /// The id of the request this answers.
    pub id: u64,
    /// The image's width in texels.
    pub width: u32,
    /// The image's height in texels.
    pub height: u32,
    /// Tightly packed RGBA8 rows, top row first.
    pub rgba: Vec<u8>,
}

impl PreviewImage {
    /// The answer to a request that was taken but could not be drawn: the id,
    /// and no pixels.
    ///
    /// Every taken request is answered. A caller that waits for an answer
    /// which never comes - an undecodable file, a frame that drew nothing, a
    /// renderer that was still busy - waits forever, and an editor that asks
    /// one question at a time stops asking. A refusal says "no preview for
    /// this", so the caller forgets the request and takes the next one.
    pub fn refused(id: u64) -> Self {
        Self {
            id,
            width: 0,
            height: 0,
            rgba: Vec::new(),
        }
    }

    /// Whether this answer carries no pixels. Only a refused preview has none.
    pub fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }
}

// =============================================================================
// PreviewResults
// =============================================================================

/// The previews the renderer finished.
#[derive(Default)]
pub struct PreviewResults {
    /// Every answer handed over since the last [`Self::take`].
    pub results: Vec<PreviewImage>,
}

impl PreviewResults {
    /// Takes every answer waiting and leaves the list empty.
    pub fn take(&mut self) -> Vec<PreviewImage> {
        std::mem::take(&mut self.results)
    }
}

impl Resource for PreviewResults {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::preview::PreviewResults")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Inserts [`PreviewRequests`] and [`PreviewResults`] unless the world already
/// has them, so a reload keeps the requests and answers already waiting.
pub fn ensure_preview_resources(world: &mut World) {
    if !world.has_resource::<PreviewRequests>() {
        world.insert_resource(PreviewRequests::default());
    }
    if !world.has_resource::<PreviewResults>() {
        world.insert_resource(PreviewResults::default());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused answer names its request and carries no pixels, so the caller
    /// can drop the request instead of waiting for a preview that will never
    /// come.
    #[test]
    fn a_refused_preview_names_its_request_and_carries_no_pixels() {
        let refused = PreviewImage::refused(7);
        assert_eq!(refused.id, 7);
        assert!(refused.is_empty());
        assert!(!PreviewImage {
            id: 8,
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        }
        .is_empty());
    }
}
