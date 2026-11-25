//! What a renderer tells an editor about the content it draws.
//!
//! # Responsibilities
//!
//! - Carry the shader a newly created file should name ([`DefaultShader`]).
//! - Install that value before any artifact loads ([`ensure_editor_resources`]),
//!   so a renderer that sets it never has to insert a resource of its own.
//!
//! # Design
//!
//! An editor writes files; it does not own a rendering chain, so it cannot work
//! out which shader a material or a pass should start with. The renderer that
//! owns the chain knows, and says so through this value. A frontend that never
//! creates a file ignores it, and a renderer that cannot name a shader leaves it
//! empty, which writes the file the way it was written before this value
//! existed: no shader, so the chain substitutes its own.
//!
//! The value is a resource rather than a function because it crosses a module
//! boundary: a renderer sets it, the editor reads it, and neither links the
//! other. The host inserts it before any artifact loads, because a renderer may
//! register no data of its own - a failed load clears only its systems, which is
//! sound only while it owns nothing.

// External crates
use pill_engine::{AssetGuid, Resource, World};

// =============================================================================
// DefaultShader
// =============================================================================

/// The shader a newly created material or pass should name, when the renderer
/// can name one.
///
/// `None` means the renderer has no answer - it draws its chain through a
/// shader an editor cannot name, or it loaded no shader at all - and a created
/// file then names none, which is what makes a material draw through whatever
/// shader the chain would substitute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefaultShader {
    /// The guid of the shader to write into a file that is created without one.
    pub guid: Option<AssetGuid>,
}

impl DefaultShader {
    /// Names `guid` as the shader a created file should start with.
    pub fn with(guid: AssetGuid) -> Self {
        Self { guid: Some(guid) }
    }

    /// The shader a created file should name, or `None` when the renderer named
    /// none.
    pub fn guid(&self) -> Option<AssetGuid> {
        self.guid
    }
}

impl Resource for DefaultShader {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::DefaultShader")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Inserts [`DefaultShader`] unless the world already has it, so a reload keeps
/// the value the renderer set before the swap.
pub fn ensure_editor_resources(world: &mut World) {
    if !world.has_resource::<DefaultShader>() {
        world.insert_resource(DefaultShader::default());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The value keeps the name both sides of the module boundary see, so a
    /// renderer sets the same resource an editor reads.
    #[test]
    fn the_world_shares_the_default_shader_by_one_name() {
        assert_eq!(
            DefaultShader::shared_name(),
            Some("pill_renderer_api::DefaultShader")
        );
    }

    /// A world that has the value keeps it when the host installs again, which
    /// is what makes a renderer's answer survive a reload.
    #[test]
    fn installing_twice_keeps_the_answer_the_renderer_gave() {
        let mut world = World::new();
        ensure_editor_resources(&mut world);
        let guid = AssetGuid::from_name("an editor's shader");
        world.get_resource_mut::<DefaultShader>().unwrap().guid = Some(guid);

        ensure_editor_resources(&mut world);

        assert_eq!(
            world.get_resource::<DefaultShader>().unwrap().guid(),
            Some(guid),
            "a second install must not clear what the renderer said"
        );
    }
}
