//! The key a draw queue sorts by, and the slot identity it packs.
//!
//! # Responsibilities
//!
//! - Name one GPU slot by its index and the version it was issued with
//!   ([`SlotIdentity`]).
//! - Pack a draw's rendering order and the slot identity of the three objects
//!   it binds into the one `u64` a queue sorts by
//!   ([`compose_draw_key`]).
//!
//! # Design
//!
//! Sorting by the key puts draws that use the same rendering order, shader,
//! material and mesh next to each other, so a renderer records each run of
//! equal keys as one instanced draw.
//!
//! The key exists only for that comparison. It stores each index and version
//! in one byte, so a value above 255 keeps only its low byte, and nothing reads
//! a key back as a handle. A draw carries its real handles beside the key,
//! which is why an asset whose slot version has passed 255 still draws with its
//! own objects.

// =============================================================================
// Slot Identity
// =============================================================================

/// One object's place in a renderer's per-type slot table: its index, and the
/// version the table issued that index with.
///
/// A slot is reused with a version that moves by one, so the pair names one
/// exact object for as long as it is alive. The index and the version travel
/// together into [`compose_draw_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotIdentity {
    /// The slot's index in its table.
    pub index: u32,
    /// The version that slot was issued with.
    pub version: u32,
}

impl SlotIdentity {
    /// Names slot `index` at `version`.
    pub fn new(index: u32, version: u32) -> Self {
        Self { index, version }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Packs the rendering order of a draw and the objects it binds into one `u64`
/// that sorts in the order a draw queue needs. The most significant byte comes
/// first:
///
/// - bits 56-63: the material's rendering order, stored inverted
///   (`u8::MAX - order`), so a higher order sorts earlier;
/// - bits 48-55: the shader's slot index;
/// - bits 40-47: the slot version the shader was issued with;
/// - bits 32-39: the material's slot index;
/// - bits 24-31: the slot version the material was issued with;
/// - bits 16-23: the mesh's slot index;
/// - bits 8-15: the slot version the mesh was issued with;
/// - bits 0-7: unused and always zero.
///
/// Because each slot index sits next to its version, a key names one exact
/// state of each slot.
///
/// The key is only ever compared. Every index and version is stored in a single
/// byte, so a value above 255 keeps only its low byte and can match a lower
/// value. A key never stands in for the handles a draw binds: those travel
/// beside it.
pub fn compose_draw_key(
    order: u8,
    shader: SlotIdentity,
    material: SlotIdentity,
    mesh: SlotIdentity,
) -> u64 {
    (u64::from(u8::MAX - order) << 56)
        | (u64::from(shader.index as u8) << 48)
        | (u64::from(shader.version as u8) << 40)
        | (u64::from(material.index as u8) << 32)
        | (u64::from(material.version as u8) << 24)
        | (u64::from(mesh.index as u8) << 16)
        | (u64::from(mesh.version as u8) << 8)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_larger_order_sorts_earlier_in_the_composed_key() {
        let shader = SlotIdentity::new(3, 4);
        let material = SlotIdentity::new(5, 6);
        let mesh = SlotIdentity::new(7, 8);

        let order_200 = compose_draw_key(200, shader, material, mesh);
        let order_10 = compose_draw_key(10, shader, material, mesh);

        assert!(
            order_200 < order_10,
            "the queue sorts ascending on this key, so the larger order is drawn first"
        );
    }

    /// Each object's identity has its own byte, in the order the key lays out,
    /// so two draws that differ in any one of them never compare equal.
    #[test]
    fn each_object_occupies_its_own_byte_pair() {
        let shader = SlotIdentity::new(3, 4);
        let material = SlotIdentity::new(5, 6);
        let mesh = SlotIdentity::new(7, 8);

        let key = compose_draw_key(0, shader, material, mesh);

        assert_eq!(key >> 56, u64::from(u8::MAX), "no rendering order");
        assert_eq!((key >> 48) & 0xFF, 3, "the shader's index");
        assert_eq!((key >> 40) & 0xFF, 4, "the shader's version");
        assert_eq!((key >> 32) & 0xFF, 5, "the material's index");
        assert_eq!((key >> 24) & 0xFF, 6, "the material's version");
        assert_eq!((key >> 16) & 0xFF, 7, "the mesh's index");
        assert_eq!((key >> 8) & 0xFF, 8, "the mesh's version");
        assert_eq!(key & 0xFF, 0, "the lowest byte is unused");
    }

    /// A slot version above 255 keeps only its low byte, which is why a draw
    /// carries its real slots beside the key.
    #[test]
    fn a_version_above_255_keeps_only_its_low_byte() {
        let at_256 = compose_draw_key(
            0,
            SlotIdentity::new(3, 4),
            SlotIdentity::new(5, 256),
            SlotIdentity::new(7, 8),
        );
        let at_0 = compose_draw_key(
            0,
            SlotIdentity::new(3, 4),
            SlotIdentity::new(5, 0),
            SlotIdentity::new(7, 8),
        );

        assert_eq!(at_256, at_0);
    }
}
