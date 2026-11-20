//! Lightweight entity handles with generation-based invalidation.
//!
//! # Responsibilities
//!
//! - Defines the [`Entity`] handle type used throughout the ECS.
//! - Defines [`EntityId`], the stable identity the world keeps for an
//!   [`Entity`], for references that outlive the handle (a saved reference, a
//!   network message).
//! - Provides generation-counter logic to prevent dangling-handle bugs.
//! - Keeps the handle compact (16 bytes) for cheap copy and storage.
//!
//! # Design
//!
//! Entities are not objects: they are 64-bit slot ids paired with a 32-bit
//! generation counter. When an entity is destroyed, its id goes back on a
//! free list, and the next handle for that slot carries the next generation.
//! A slot that has reached the maximum generation is retired instead of
//! reused. Any old handle still holds the previous generation, so it fails
//! validation. That prevents use-after-free bugs without reference counting
//! or garbage collection.
//!
//! A handle is therefore cheap but meaningless outside its run, because the
//! slot it names is reused. [`EntityId`] is the opposite trade: it is random
//! and not derived from a slot, so data written for a later run can name one
//! entity without depending on the order the world happened to spawn them
//! in.

// Standard library
use std::fmt;

// =============================================================================
// Entity
// =============================================================================

/// A handle that names one entity in a [`World`].
///
/// The handle is 16 bytes, [`Copy`], and cheap to pass by value. Its
/// generation counter tells a reused slot apart from the entity that used it
/// before:
///
/// ```
/// # use pill_engine::*;
/// # #[derive(Debug, Clone)] struct Health(f32);
/// # impl Component for Health {}
/// # #[derive(Debug, Clone)] struct Damage(f32);
/// # impl Component for Damage {}
/// # let mut world = World::new();
/// # world.register_component::<Health>();
/// # world.register_component::<Damage>();
/// let enemy = world.create_entity().with(Health(100.0)).build().unwrap();
/// world.destroy_entity(enemy);
///
/// let bullet = world.create_entity().with(Damage(10.0)).build().unwrap();
///
/// // The destroyed handle is reported as invalid:
/// assert!(!world.is_entity_valid(enemy));
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    /// The numeric id of this entity's slot.
    ///
    /// Ids are reused when entities are destroyed, so the id space does not
    /// grow without end. The same id can name different entities over time;
    /// the `generation` field tells them apart.
    pub(crate) id: u64,

    /// The generation of the entity now in this slot.
    ///
    /// The world adds one each time the slot is reused. This is what lets a
    /// stale handle - one whose entity was destroyed and whose slot went to a
    /// new entity - be detected. A handle is valid only if both its id and its
    /// generation match the entity now in that slot.
    pub(crate) generation: u32,
}

// =============================================================================
// Entity - Inherent Implementations
// =============================================================================

impl Entity {
    /// A handle that names no entity.
    ///
    /// What a reference field holds before it is set and after it is cleared.
    /// Both halves are saturated, which no slot allocation can produce: slot
    /// ids count up from zero and stay far below `u64::MAX`.
    pub const NULL: Self = Self {
        id: u64::MAX,
        generation: u32::MAX,
    };

    /// Whether this is the [null handle](Self::NULL).
    #[inline]
    pub fn is_null(self) -> bool {
        self == Self::NULL
    }

    // -------------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------------

    /// Creates an entity handle for use in test fixtures only.
    #[cfg(test)]
    pub(crate) fn new_for_test(id: u64, generation: u32) -> Self {
        Self { id, generation }
    }

    /// Rebuilds a handle from its id and generation.
    ///
    /// Callers that stored the pair and check it against the world themselves
    /// use this - the editor's state recovery and saved references, for
    /// example. Until a live entity carries both numbers again,
    /// [`World::is_entity_valid`] answers `false`.
    #[inline]
    pub const fn from_raw(id: u64, generation: u32) -> Self {
        Self { id, generation }
    }

    // -------------------------------------------------------------------------
    // Property accessors
    // -------------------------------------------------------------------------

    /// Returns the numeric id of this entity's slot.
    ///
    /// The id alone is not unique over time, because the slot is reused. Pair
    /// it with [`generation`](Self::generation) to name one entity.
    #[inline]
    pub fn id(self) -> u64 {
        self.id
    }

    /// Returns the generation this handle was issued with.
    #[inline]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

// =============================================================================
// Entity - Trait Implementations
// =============================================================================

impl Default for Entity {
    /// The [null handle](Self::NULL): a reference to nothing.
    ///
    /// Slot 0 is a real slot the world hands out, so a defaulted field must not
    /// mean "entity 0" - a component whose reference field was never set would
    /// otherwise point at whoever owns that slot.
    fn default() -> Self {
        Self::NULL
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}v{}", self.id, self.generation)
    }
}

// =============================================================================
// EntityId
// =============================================================================

/// Stable 128-bit identity of one entity, for references that outlive a run.
///
/// An [`Entity`] names a slot and carries a generation: it is cheap, and it
/// means nothing to a world that did not issue it, because the slot is reused.
/// An `EntityId` is what data meant for a later run stores instead - a scene
/// file, a spawn table, a saved selection - so a reference finds its entity
/// again after the world is rebuilt and the slots are handed out in a
/// different order.
///
/// The id is random, not derived from the slot. A slot-derived id would name
/// whoever took that slot next, so a stale reference would address the wrong
/// entity instead of nothing. The world assigns one when it creates the
/// entity, unless the creator supplies one, and indexes the id in both
/// directions.
///
/// # Examples
///
/// ```
/// # use pill_engine::*;
/// # let mut world = World::new();
/// let entity = world.create_entity().build().unwrap();
/// let id = world.entity_id(entity).expect("the entity has one");
/// assert_eq!(world.entity_by_id(id), Some(entity));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(u128);

impl EntityId {
    /// Wraps an id a file or a spawn table already recorded.
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Derives an id from a name, with the same hash the engine uses for shared
    /// component identities.
    ///
    /// This is what the world falls back to when it cannot mint a random id -
    /// after four id collisions, or when the platform has no random source.
    /// The result is unique per name and can be derived again from that name.
    pub const fn from_name(name: &str) -> Self {
        Self(crate::component::shared_component_identity(name))
    }

    /// Returns the id as its raw 128-bit number.
    pub const fn value(self) -> u128 {
        self.0
    }

    /// Creates a new id from the operating system's random source.
    ///
    /// # Errors
    ///
    /// Returns the `getrandom` error when the platform has no usable random
    /// source. The world then falls back to counter-derived ids and logs the
    /// failure.
    pub fn random() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)?;
        Ok(Self(u128::from_le_bytes(bytes)))
    }

    /// Parses a 32-digit hexadecimal id, in either letter case, or returns
    /// `None` for any other text. This is the form
    /// [`Display`](std::fmt::Display) writes.
    pub fn parse(text: &str) -> Option<Self> {
        // Exactly 32 hex digits: `from_str_radix` alone would also accept a
        // leading `+` and a shorter string.
        if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(text, 16).ok().map(Self)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

// Serialized as the 32-digit hex string rather than a number: a 128-bit integer
// does not survive JSON number handling in most other tools, C# included.
impl serde::Serialize for EntityId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for EntityId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{text}` is not an entity id (expected 32 hexadecimal digits)"
            ))
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `Entity` is 16 bytes with 8-byte alignment.
    #[test]
    fn entity_size_and_alignment() {
        assert_eq!(
            std::mem::size_of::<Entity>(),
            16,
            "Entity size changed - check field order"
        );
        assert_eq!(std::mem::align_of::<Entity>(), 8);
    }

    /// An id round-trips through the hex form the saved data uses, and refuses
    /// anything that is not exactly 32 hex digits.
    #[test]
    fn an_entity_id_round_trips_through_hex() {
        let id = EntityId::new(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);
        assert_eq!(
            id.to_string(),
            "0123456789abcdeffedcba9876543210",
            "an id prints as 32 hex digits"
        );
        assert_eq!(EntityId::parse(&id.to_string()), Some(id));
        assert_eq!(EntityId::parse("0123"), None, "short text is not an id");
        assert_eq!(EntityId::parse(&"z".repeat(32)), None, "not hex");
    }

    /// Random ids carry nothing about the entity, so two calls do not return
    /// the same id.
    #[test]
    fn random_ids_differ() {
        let first = EntityId::random().expect("the platform has a random source");
        let second = EntityId::random().expect("the platform has a random source");
        assert_ne!(first, second);
    }

    /// Ids serialize as the hex string, not as a JSON number, so a 128-bit
    /// value survives a reader with 64-bit numbers.
    #[test]
    fn an_entity_id_serializes_as_hex_text() {
        let id = EntityId::new(7);
        let text = serde_json::to_string(&id).expect("an id serializes");
        assert_eq!(text, "\"00000000000000000000000000000007\"");
        assert_eq!(serde_json::from_str::<EntityId>(&text).ok(), Some(id));
    }
}
