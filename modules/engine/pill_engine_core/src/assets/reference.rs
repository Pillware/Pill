//! A saved reference to an asset: [`AssetReference`].
//!
//! # Responsibilities
//!
//! - Store an asset by its guid in data written to disk. A set guid is
//!   written as `{"guid": "<32 hex digits>"}`; an unset one as `null`.
//! - Turn that guid back into the live [`Handle`] when the data is loaded.
//!   A guid that no loaded asset has becomes [`Handle::INVALID`], and it is
//!   reported once.
//!
//! # Design
//!
//! A [`Handle`] is a slot index and a generation. That is exact and cheap
//! while the game runs, but it means nothing in the next run, where the same
//! asset may sit in a different slot. Hot-reload persistence keeps a handle
//! as its `(index, generation)` pair, so that in-run form has to stay as it
//! is. Anything written for a later run - a scene, a material file - stores
//! an `AssetReference` instead and turns it back into a handle once loaded.
//!
//! An unresolved reference means a missing asset, not a broken program. It
//! resolves to [`Handle::INVALID`], and the renderer's caches answer that
//! with the default material, shader or texture; an instance whose mesh
//! never uploaded is not drawn at all. Each guid is reported once, not once
//! per frame.

// Standard library
use std::marker::PhantomData;

// External crates
use pill_core::telemetry::telemetry_target;
use trait_type_map::TraitAccessible;

// Current crate
use crate::asset::{Asset, AssetGuid, AssetManager, Handle};

// =============================================================================
// Asset Reference
// =============================================================================

/// A saved reference to an asset of type `T`, stored as the asset's guid.
///
/// A set reference is written as `{"guid": "<32 hex digits>"}` and an unset
/// one as `null`. The module docs say when to use this instead of a
/// [`Handle`].
pub struct AssetReference<T: Asset> {
    guid: Option<AssetGuid>,
    /// Marks `T` without storing one; a reference holds only its guid.
    _asset_type: PhantomData<fn() -> T>,
}

impl<T: Asset> AssetReference<T> {
    /// Creates a reference to the asset with `guid`.
    pub const fn new(guid: AssetGuid) -> Self {
        Self {
            guid: Some(guid),
            _asset_type: PhantomData,
        }
    }

    /// Creates a reference to no asset.
    ///
    /// It resolves to [`Handle::INVALID`] and reports nothing: an empty slot
    /// is a choice, not a missing asset.
    pub const fn unset() -> Self {
        Self {
            guid: None,
            _asset_type: PhantomData,
        }
    }

    /// Returns the referenced guid, or `None` when unset.
    pub fn guid(&self) -> Option<AssetGuid> {
        self.guid
    }

    /// Returns whether the reference names an asset.
    pub fn is_set(&self) -> bool {
        self.guid.is_some()
    }

    /// Creates a reference to the asset that `handle` points at, stored by
    /// its guid.
    ///
    /// Returns `None` when the handle is stale or the asset has no guid.
    pub fn from_handle(assets: &AssetManager, handle: Handle<T>) -> Option<Self>
    where
        T: TraitAccessible<dyn Asset>,
    {
        assets.guid_of(handle).map(Self::new)
    }

    /// Returns the live handle of the referenced asset, or `None` when the
    /// reference is unset or no asset has its guid. Reports nothing.
    pub fn try_resolve(&self, assets: &AssetManager) -> Option<Handle<T>>
    where
        T: TraitAccessible<dyn Asset>,
    {
        assets.handle_by_guid::<T>(self.guid?)
    }

    /// Returns the live handle of the referenced asset, or
    /// [`Handle::INVALID`].
    ///
    /// When a set reference resolves to nothing, its guid is logged as a
    /// warning the first time it is seen in this process. An unset reference
    /// is not reported.
    pub fn resolve(&self, assets: &AssetManager) -> Handle<T>
    where
        T: TraitAccessible<dyn Asset>,
    {
        if let Some(handle) = self.try_resolve(assets) {
            return handle;
        }
        if let Some(guid) = self.guid {
            if crate::saved_references::first_report_of(guid) {
                pill_core::warn!(
                    target: telemetry_target::RESOURCES,
                    guid = %guid,
                    asset_type = std::any::type_name::<T>(),
                    "asset reference resolves to no loaded asset; using the invalid handle"
                );
            }
        }
        Handle::INVALID
    }
}

// These impls are written by hand: a derive would require `T` to implement
// each trait too, even though a reference stores no `T`.

impl<T: Asset> Clone for AssetReference<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Asset> Copy for AssetReference<T> {}

impl<T: Asset> PartialEq for AssetReference<T> {
    fn eq(&self, other: &Self) -> bool {
        self.guid == other.guid
    }
}

impl<T: Asset> Eq for AssetReference<T> {}

impl<T: Asset> Default for AssetReference<T> {
    fn default() -> Self {
        Self::unset()
    }
}

impl<T: Asset> std::fmt::Debug for AssetReference<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.guid {
            Some(guid) => write!(formatter, "AssetReference({guid})"),
            None => formatter.write_str("AssetReference(unset)"),
        }
    }
}

impl<T: Asset> serde::Serialize for AssetReference<T> {
    /// Writes the saved form: `{"guid": "<32 hex digits>"}`, or `null` when
    /// unset.
    ///
    /// A reference holds no path, so no hint is written. A writer that has
    /// the asset manager can add one through
    /// [`SavedAssetReference::with_path`](crate::SavedAssetReference::with_path).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.guid {
            Some(guid) => {
                crate::saved_references::SavedAssetReference::new(guid).serialize(serializer)
            }
            None => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Asset> serde::Deserialize<'de> for AssetReference<T> {
    /// Reads both stored spellings: the object form this build writes and the
    /// bare 32-hex guid that an older material or render pass file holds.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            guid: Option::<crate::saved_references::SavedAssetReference>::deserialize(
                deserializer,
            )?
            .map(|reference| reference.guid),
            _asset_type: PhantomData,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use trait_type_map::impl_trait_accessible;

    #[derive(Debug, PartialEq)]
    struct Picture(&'static str);
    impl Asset for Picture {}
    impl_trait_accessible!(dyn Asset; Picture);

    const GUID: &str = "000000000000000000000000000000ab";

    #[test]
    fn a_reference_serializes_in_the_saved_form_and_back() {
        let reference = AssetReference::<Picture>::new(AssetGuid::parse(GUID).unwrap());

        let json = serde_json::to_string(&reference).unwrap();
        assert_eq!(json, format!(r#"{{"guid":"{GUID}"}}"#));
        let read: AssetReference<Picture> = serde_json::from_str(&json).unwrap();
        assert_eq!(read, reference);
    }

    /// The bare hex a `.material` file holds loads as the same reference.
    #[test]
    fn a_reference_reads_the_bare_guid_older_files_hold() {
        let read: AssetReference<Picture> = serde_json::from_str(&format!("\"{GUID}\"")).unwrap();
        assert_eq!(read, AssetReference::new(AssetGuid::parse(GUID).unwrap()));

        let object: AssetReference<Picture> =
            serde_json::from_str(&format!(r#"{{"guid":"{GUID}","path":"a.png"}}"#)).unwrap();
        assert_eq!(object, read, "the path hint does not change the identity");
    }

    #[test]
    fn an_unset_reference_serializes_as_null() {
        let json = serde_json::to_string(&AssetReference::<Picture>::unset()).unwrap();
        assert_eq!(json, "null");
        let read: AssetReference<Picture> = serde_json::from_str("null").unwrap();
        assert!(!read.is_set());
    }

    #[test]
    fn a_malformed_guid_is_a_deserialization_error() {
        assert!(serde_json::from_str::<AssetReference<Picture>>("\"not a guid\"").is_err());
    }

    #[test]
    fn a_reference_resolves_to_the_live_handle_of_its_guid() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::parse(GUID).unwrap();
        let handle = assets
            .add_named_with_guid("a.png", guid, Picture("a"))
            .unwrap();

        let reference = AssetReference::<Picture>::from_handle(&assets, handle).unwrap();
        assert_eq!(reference.guid(), Some(guid));
        assert_eq!(reference.resolve(&assets), handle);
        assert_eq!(assets.get(reference.resolve(&assets)), Some(&Picture("a")));
    }

    #[test]
    fn a_missing_or_unset_reference_resolves_to_the_invalid_handle() {
        let assets = AssetManager::new();
        let missing = AssetReference::<Picture>::new(
            AssetGuid::parse("0000000000000000000000000000beef").unwrap(),
        );

        assert_eq!(missing.resolve(&assets), Handle::INVALID);
        assert_eq!(
            AssetReference::<Picture>::unset().resolve(&assets),
            Handle::INVALID
        );
        assert_eq!(missing.try_resolve(&assets), None);
    }

    #[test]
    fn a_missing_guid_is_reported_once() {
        let guid = AssetGuid::parse("0000000000000000000000000000cafe").unwrap();
        assert!(crate::saved_references::first_report_of(guid));
        assert!(!crate::saved_references::first_report_of(guid));
        assert!(!crate::saved_references::first_report_of(guid));
    }
}
