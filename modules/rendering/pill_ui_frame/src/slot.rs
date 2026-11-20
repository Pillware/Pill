//! The world resource a UI producer publishes a frame into and a renderer
//! takes it from.
//!
//! # Responsibilities
//!
//! - [`UiFrameSlot`]: the world resource holding the newest [`UiFrame`] a UI
//!   producer built. It offers `publish` and `take` for the frame, `is_empty`,
//!   and `clear` to drop a frame but keep its buffers.
//! - `request_texture_resend` and `texture_resend_generation`: the pair a
//!   painter uses to ask for every texture again, and a producer to tell
//!   whether that request is new.
//!
//! # Design
//!
//! A UI producer and a renderer meet here rather than naming each other. The
//! producer (a project's egui pass) writes what it drew; the renderer takes
//! the frame and draws it. Neither side depends on the other's crate, which
//! lets a project link the game-UI library while the renderer module draws
//! without linking it too. On Windows a single link unit cannot hold two
//! copies of the same third-party graph in different formats.
//!
//! The slot lives beside the data it holds rather than in the engine core, so
//! the core names no UI types. Both sides link this crate, so they share one
//! slot type, and its [`Resource::shared_name`] is pinned. The resource id
//! comes from that name rather than from a per-artifact type id, so the
//! project's DLL and the renderer module reach one resource. A changed name
//! would register a second one.

// External crates
use pill_engine_core::resource::Resource;

// Current crate
use crate::frame::UiFrame;

// =============================================================================
// UiFrameSlot
// =============================================================================

/// Holds the frame the last UI pass published until the renderer takes it.
#[derive(Debug, Default)]
pub struct UiFrameSlot {
    /// The frame waiting for the renderer; `None` until the first publish.
    frame: Option<UiFrame>,
    /// The generation of the newest resend request a painter made.
    ///
    /// A rebuilt renderer module starts with an empty texture map. The
    /// producer, a shared library that outlives it, already carried every
    /// image once, so nothing new is waiting to be sent. A painter that starts
    /// empty asks for everything again; the producer compares this number with
    /// the one it last served and re-carries every texture it holds when the
    /// two differ.
    texture_resend_generation: u64,
}

impl UiFrameSlot {
    /// An empty slot.
    pub fn new() -> UiFrameSlot {
        UiFrameSlot::default()
    }

    /// Whether nothing is waiting to be drained.
    pub fn is_empty(&self) -> bool {
        self.frame.as_ref().is_none_or(UiFrame::is_empty)
    }

    /// Hand `frame` to the renderer, replacing anything not yet drained.
    ///
    /// The newest frame is the one worth drawing. When a producer runs twice
    /// between two renderer passes, the second frame replaces the first, so a
    /// renderer never works through a queue of older frames.
    pub fn publish(&mut self, frame: UiFrame) {
        self.frame = Some(frame);
    }

    /// Drain the slot, leaving it empty.
    pub fn take(&mut self) -> UiFrame {
        self.frame.take().unwrap_or_default()
    }

    /// Forget what is waiting, keeping its allocations.
    pub fn clear(&mut self) {
        if let Some(frame) = self.frame.as_mut() {
            frame.clear();
        }
    }

    /// Ask the producer to carry every texture again.
    ///
    /// A painter calls this when its texture map starts empty, which happens
    /// after a renderer module is rebuilt. The producer carries a texture only
    /// when its revision changed, so without this request a rebuilt painter
    /// would wait for each image to change again. The full rule, covering the
    /// revision set and this request, is in [`crate::texture`].
    pub fn request_texture_resend(&mut self) {
        self.texture_resend_generation = self.texture_resend_generation.wrapping_add(1);
    }

    /// The generation of the newest resend request a painter made.
    ///
    /// A producer re-carries its textures when this differs from the value it
    /// last served.
    pub fn texture_resend_generation(&self) -> u64 {
        self.texture_resend_generation
    }
}

impl Resource for UiFrameSlot {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine_core::resource::PlayState {
        pill_engine_core::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_ui_frame::UiFrameSlot")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::{Color, DrawList, Rect, TextureId};
    use crate::frame::UiDraw;
    use crate::texture::{UiTexture, UiTextureFilter};

    /// A frame carrying one draw list, so emptiness is observable.
    fn drawn_frame() -> UiFrame {
        let mut frame = UiFrame::default();
        let mut list = DrawList::new();
        list.push_quad(
            TextureId(1),
            Rect::from_min_size(
                pill_core::math::Vector2f::ZERO,
                pill_core::math::Vector2f::new(2.0, 2.0),
            ),
            pill_core::math::Vector2f::ZERO,
            pill_core::math::Vector2f::new(1.0, 1.0),
            Color::WHITE,
        );
        frame.draws.push(UiDraw {
            surface: 0,
            scale_factor: 1.0,
            list,
        });
        frame
    }

    #[test]
    fn a_fresh_slot_is_empty() {
        let mut slot = UiFrameSlot::new();
        assert!(slot.is_empty());
        assert!(slot.take().is_empty());
    }

    #[test]
    fn a_published_frame_is_what_the_next_take_returns() {
        let mut slot = UiFrameSlot::new();
        slot.publish(drawn_frame());
        assert!(!slot.is_empty());
        let frame = slot.take();
        assert_eq!(frame.draws.len(), 1);
        assert!(slot.is_empty(), "the take drained the slot");
    }

    #[test]
    fn the_newest_frame_replaces_the_one_not_yet_drained() {
        let mut slot = UiFrameSlot::new();
        slot.publish(drawn_frame());
        slot.publish(UiFrame::default());
        assert!(
            slot.take().is_empty(),
            "the second publish replaced the first frame"
        );
    }

    /// A painter that starts with no textures asks for them again, and the
    /// producer can see the request as a number it compares.
    #[test]
    fn a_resend_request_bumps_the_generation_the_producer_watches() {
        let mut slot = UiFrameSlot::new();
        assert_eq!(slot.texture_resend_generation(), 0, "nothing asked yet");

        slot.request_texture_resend();
        let asked = slot.texture_resend_generation();
        assert_ne!(asked, 0, "the request is visible");

        // Publishing and draining are the frame's own business and leave the
        // request alone.
        slot.publish(drawn_frame());
        let _ = slot.take();
        assert_eq!(slot.texture_resend_generation(), asked);
    }

    /// A frame's textures wait for the renderer even when the frame draws into
    /// no surface: they belong to the update, not to a surface. The renderer
    /// applies them before it decides what to draw, so a frame that drew
    /// nothing still hands them over.
    #[test]
    fn a_frame_keeps_its_textures_until_a_renderer_takes_it() {
        let mut slot = UiFrameSlot::new();
        let mut frame = UiFrame::default();
        frame.textures.textures.push(UiTexture {
            id: TextureId(1),
            width: 1,
            height: 1,
            revision: 1,
            pixels: std::sync::Arc::from(vec![0u8, 0, 0, 255].into_boxed_slice()),
            filter: UiTextureFilter::Nearest,
        });
        slot.publish(frame);

        // A frame with no draws is still not an empty slot while it carries
        // textures. The renderer has something to apply.
        assert!(!slot.is_empty());
        let taken = slot.take();
        assert_eq!(taken.textures.textures.len(), 1);
        assert_eq!(taken.textures.textures[0].revision, 1);
        assert!(
            slot.take().textures.textures.is_empty(),
            "the take drained them"
        );
    }
}
