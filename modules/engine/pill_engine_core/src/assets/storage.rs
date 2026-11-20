//! Many-per-type asset storage, addressed by generational handle.
//!
//! # Responsibilities
//!
//! - Defines the [`Asset`] marker trait for types stored many-per-type.
//! - Provides [`Handle<T>`], a typed generational reference to one asset.
//! - Implements [`AssetManager`], the store holding every asset column.
//!
//! # Design
//!
//! A [`Resource`](crate::resource::Resource) holds one value per type, and code
//! fetches it by type through [`Res`](crate::query::Res). A mesh or a texture
//! is one of many values of the same type, so the type alone cannot name one.
//! Assets therefore get their own store instead of a second reading of
//! [`Resource`]. [`AssetManager`] is itself an ordinary resource, so
//! `Res<AssetManager>` and `ResMut<AssetManager>` reach it, and the scheduler
//! already keeps two systems from using it at once when their uses conflict.
//!
//! Storage works like component storage - one type-erased column per type in a
//! `TraitTypeMap` - without archetypes. Archetypes group entities by their set
//! of components, and that grouping means nothing for an asset.
//!
//! Each column is an `ErasedVecStorage` from `trait_type_map`. The operations
//! the column runs on its values are a table of function pointers that this
//! store owns, so loading new code can replace the table (see
//! [`AssetManager::rehome`]). Values and handle indices stay where they are
//! across that swap.
//!
//! A [`Handle<T>`] is generational. Freeing a slot adds one to that slot's
//! generation, so a handle kept across an unload returns `None` instead of
//! quietly naming whatever was loaded into the slot next.

// Standard library
use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;

// External crates
use trait_type_map::{
    ErasedVecFamily, ErasedVecStorage, ErasedVecStorageInfo, ErasedVecStorageOps, TraitAccessible,
    TraitTypeMap,
};

// Current crate
use crate::resource::Resource;

// =============================================================================
// Asset
// =============================================================================

/// Marks data that is stored many-per-type in the [`AssetManager`].
///
/// Implement this for meshes, textures, materials, shaders and the like: any
/// type a world holds several of, where the type alone does not identify one
/// value. Use [`Resource`](crate::resource::Resource) instead for genuine
/// singletons such as a clock or an input snapshot.
///
/// The `Send + Sync` bound has the same reason resources have it: systems run
/// on several threads, and any of them may reach the store.
///
/// # Examples
///
/// ```
/// use pill_engine::asset::Asset;
/// use trait_type_map::impl_trait_accessible;
///
/// struct Mesh {
///     vertices: Vec<[f32; 3]>,
/// }
///
/// impl Asset for Mesh {}
/// impl_trait_accessible!(dyn Asset; Mesh);
/// ```
pub trait Asset: Send + Sync + 'static {
    /// A stable name that identifies this asset type across binaries, or
    /// `None` to be identified by its `TypeId` alone.
    ///
    /// This is the asset version of
    /// [`Component::shared_name`](crate::Component::shared_name).
    ///
    /// Every binary that compiles its own copy of the crate defining the type
    /// (a module and the crates that link it, for example) gets its own
    /// `TypeId` for that type. A shared name gives those copies one column
    /// instead of one each; registration also checks that the copies agree on
    /// the type's size and alignment.
    ///
    /// Write a fixed string rather than building one from the module path, so
    /// moving the type does not change its identity.
    fn shared_name() -> Option<&'static str>
    where
        Self: Sized,
    {
        None
    }

    /// The stable identity that [`Self::shared_name`] hashes to, or `None`.
    ///
    /// This is the same hash a shared component uses. Override it only when
    /// you also override `Self::shared_name()`, and keep the two based on the
    /// same string.
    fn shared_identity() -> Option<u128>
    where
        Self: Sized,
    {
        Self::shared_name().map(crate::component::shared_component_identity)
    }
}

// =============================================================================
// AssetLoader
// =============================================================================

/// Where an asset's bytes come from: a file of the project, or bytes already
/// in memory.
///
/// A relative path names a file below the project's `res` directory. The asset
/// store looks for it in the packs first - a shipping or web build carries a
/// packed copy - and then on disk, so a development run reads the file itself.
/// The loading code is the same on every target.
#[derive(Debug, Clone)]
pub enum AssetLoader {
    /// A file of the project, named relative to its asset directory.
    Path(PathBuf),
    /// Bytes already in memory, such as an asset embedded at build time.
    Bytes(Box<[u8]>),
}

/// Something went wrong while loading an asset.
///
/// The asset's source could not be found, read or decoded, or its metadata file
/// or a standalone document could not be used or written.
#[derive(Debug, thiserror::Error)]
pub enum AssetLoadError {
    /// No pack or directory holds the path.
    #[error("asset path was not found: {path}")]
    PathNotFound { path: PathBuf },
    /// The file was found but could not be read.
    #[error("failed to read asset {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The bytes read are not valid UTF-8 text.
    #[error("asset {label} is not valid UTF-8: {source}")]
    Utf8 {
        label: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
    /// The bytes read do not hold the asset or document they should.
    #[error("failed to decode asset {label}: {detail}")]
    Decode { label: String, detail: String },
    /// The metadata layer cannot read, build or write an asset's metadata
    /// file.
    ///
    /// The file does not parse, names another asset type, was written by a
    /// newer format version, or holds settings that do not fit the type.
    #[error("asset metadata {path} cannot be used: {detail}")]
    Metadata {
        /// The metadata file, relative to the project's asset directory.
        path: PathBuf,
        /// What is wrong with it.
        detail: String,
    },
    /// Writing a file into the project's asset directory failed.
    #[error("failed to write asset file {path}: {source}")]
    Write {
        /// The file that could not be written.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// The result of any asset file operation: reading, decoding or writing a
/// source or a document.
pub type AssetLoadResult<T> = Result<T, AssetLoadError>;

/// Why the store refused to bind a name or a guid to an asset.
///
/// Either the name already addresses a live asset, or the handle no longer
/// addresses one. This is a separate type from [`AssetLoadError`] because
/// nothing failed to load: the store refused a request.
#[derive(Debug, thiserror::Error)]
pub enum AssetBindingError {
    /// The name already addresses a live asset.
    #[error("an asset named `{name}` already exists")]
    NameInUse {
        /// The name that is already taken.
        name: String,
    },
    /// The handle no longer addresses a live asset.
    #[error("the asset handle is stale: its asset was removed")]
    StaleHandle,
}

/// The result of a binding operation, which can be refused by an occupied name
/// or a stale handle.
pub type AssetBindingResult<T> = Result<T, AssetBindingError>;

impl AssetLoader {
    /// Mounts the project's asset directory. Relative [`Self::Path`] values are
    /// looked up below it; see [`crate::asset_store::mount_directory`].
    pub fn set_root(path: impl Into<PathBuf>) {
        crate::asset_store::mount_directory(path);
    }

    /// Returns the asset directory that is currently mounted, if one is set.
    pub fn root() -> Option<PathBuf> {
        crate::asset_store::mounted_directory()
    }

    /// Reads this source into owned bytes.
    ///
    /// A path is looked up the way [`crate::asset_store::read`] does it: in the
    /// packs first, then on disk.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::PathNotFound`] when no pack or directory
    /// holds the path, and [`AssetLoadError::Read`] when the file was found
    /// but could not be read.
    pub fn load(&self) -> AssetLoadResult<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => Ok(bytes.to_vec()),
            Self::Path(path) => crate::asset_store::read(path),
        }
    }

    /// Reads this source as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::load`], and [`AssetLoadError::Utf8`]
    /// when the bytes are not valid UTF-8.
    pub fn load_string(&self) -> AssetLoadResult<String> {
        let label = match self {
            Self::Path(path) => path.display().to_string(),
            Self::Bytes(_) => "embedded bytes".to_owned(),
        };
        String::from_utf8(self.load()?).map_err(|source| AssetLoadError::Utf8 { label, source })
    }
}

// =============================================================================
// Handle
// =============================================================================

/// A typed, generational reference to one asset in the [`AssetManager`].
///
/// A handle is cheap to copy and holds no borrow, so one can live inside a
/// component, inside another asset or inside project state without keeping the
/// store alive.
///
/// A handle is only meaningful to the [`AssetManager`] that issued it. Passing
/// one to a different manager is a programming error. It returns `None`, or -
/// if that manager happens to have a live slot at the same index and
/// generation - it names an unrelated asset. Worlds hold one manager, so this
/// does not happen in ordinary use.
#[repr(C)]
pub struct Handle<T: Asset> {
    /// Slot index within this type's column.
    index: u32,
    /// Generation of the slot at the time this handle was issued.
    ///
    /// Compared on every lookup: a mismatch means the slot was freed and
    /// possibly refilled, so this handle no longer refers to a live asset.
    generation: u32,
    /// Marks `T` without storing one; the handle owns no asset data.
    _marker: PhantomData<fn() -> T>,
}

/// The slot index that [`Handle::INVALID`] carries.
///
/// This is a named constant rather than a value written inline because the
/// field layer saves a reference as a plain index and generation pair. The
/// sentinel for "no asset" has to be the same value everywhere a reference is
/// saved, restored or cleared.
pub const INVALID_ASSET_INDEX: u32 = u32::MAX;

/// The generation that [`Handle::INVALID`] carries.
pub const INVALID_ASSET_GENERATION: u32 = u32::MAX;

impl<T: Asset> Handle<T> {
    /// Sentinel for an optional asset reference that has not been assigned.
    pub const INVALID: Self = Self {
        index: INVALID_ASSET_INDEX,
        generation: INVALID_ASSET_GENERATION,
        _marker: PhantomData,
    };

    /// Builds a handle from a raw index and generation.
    ///
    /// This trusts its arguments the way [`Deserialize`](serde::Deserialize)
    /// does: the caller promises the pair came from a handle this asset manager
    /// issued. It exists for boundaries that carry only the two integers
    /// instead of a live handle value, such as the C# mirror.
    #[inline]
    pub fn from_raw(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _marker: PhantomData,
        }
    }

    /// Returns the slot index this handle addresses.
    #[inline]
    pub fn index(self) -> u32 {
        self.index
    }

    /// Returns the generation this handle was issued against.
    #[inline]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl<T: Asset> Default for Handle<T> {
    fn default() -> Self {
        Self::INVALID
    }
}

impl<T: Asset> serde::Serialize for Handle<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.index, self.generation).serialize(serializer)
    }
}

impl<'de, T: Asset> serde::Deserialize<'de> for Handle<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (index, generation) = <(u32, u32)>::deserialize(deserializer)?;
        Ok(Self {
            index,
            generation,
            _marker: PhantomData,
        })
    }
}

// The derives are written out by hand because `#[derive]` would add a `T: Clone`
// style bound on the asset type, which a handle never needs - it stores no `T`.
impl<T: Asset> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Asset> Copy for Handle<T> {}

impl<T: Asset> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<T: Asset> Eq for Handle<T> {}

impl<T: Asset> std::hash::Hash for Handle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T: Asset> std::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Handle<{}>({}, gen {})",
            std::any::type_name::<T>(),
            self.index,
            self.generation
        )
    }
}

// =============================================================================
// AssetColumn
// =============================================================================

/// One kind of key - names or guids - in an asset column, mapped both ways.
///
/// It maps each key to its slot and each slot back to its key. Freeing a slot
/// has to remove that slot's key without searching the whole map, so both
/// directions are kept. Keeping them in one type stops a caller from updating
/// one direction and forgetting the other; if that happened, a freed name could
/// still point at whatever asset took the slot.
#[derive(Debug)]
struct KeyIndex<K> {
    slots_by_key: HashMap<K, u32>,
    keys_by_slot: HashMap<u32, K>,
}

// Written by hand rather than derived: the derive would require `K: Default`,
// which no key type here has - an empty map needs nothing from its key.
impl<K> Default for KeyIndex<K> {
    fn default() -> Self {
        Self {
            slots_by_key: HashMap::new(),
            keys_by_slot: HashMap::new(),
        }
    }
}

impl<K> KeyIndex<K>
where
    K: std::hash::Hash + Eq + Clone,
{
    /// Points `key` at `slot`, removing whatever the two held before: the key's
    /// old slot and the slot's old key.
    ///
    /// Every binding site calls this, so neither direction can keep a stale
    /// entry. Callers still refuse a key that another live asset holds where
    /// that is an error rather than a move.
    fn bind(&mut self, key: K, slot: u32) {
        if let Some(previous) = self.keys_by_slot.insert(slot, key.clone()) {
            self.slots_by_key.remove(&previous);
        }
        if let Some(previous) = self.slots_by_key.insert(key, slot) {
            self.keys_by_slot.remove(&previous);
        }
    }

    /// Removes and returns the key that `slot` holds, if any.
    fn release(&mut self, slot: u32) -> Option<K> {
        let key = self.keys_by_slot.remove(&slot)?;
        self.slots_by_key.remove(&key);
        Some(key)
    }

    /// Returns the slot that `key` is bound to, if any.
    fn slot_of<Q>(&self, key: &Q) -> Option<u32>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.slots_by_key.get(key).copied()
    }

    /// Returns the key that `slot` holds, if any.
    fn key_of(&self, slot: u32) -> Option<&K> {
        self.keys_by_slot.get(&slot)
    }
}

/// Bookkeeping for the slots of one asset column.
///
/// The column in the [`TraitTypeMap`] holds the values. This holds everything
/// else needed to hand out and check handles for those values. The two are
/// separate because the map stores columns with their type erased and cannot
/// carry metadata of its own.
#[derive(Default)]
struct AssetColumn {
    /// Size and alignment of the column's element type, as the first
    /// registration declared them. A later registration of the same shared type
    /// must match both.
    element_layout: (usize, usize),
    /// Generation of each slot, one entry per slot that has ever existed.
    ///
    /// Starts at 0 for a fresh slot and increments on every free, so a handle
    /// issued before the free never matches after it.
    generations: Vec<u32>,
    /// Content version of each slot, one entry per slot that has ever existed.
    ///
    /// Adds one whenever the stored value is borrowed mutably. A consumer that
    /// mirrors assets - the renderer's GPU uploads - can then rebuild just the
    /// asset that changed instead of all of them. A freed slot keeps its last
    /// entry, and the handle that refills the slot has a new generation, so
    /// versions are only compared through one handle.
    content_versions: Vec<u64>,
    /// Slots freed by `remove`, refilled before the column grows.
    free_slots: Vec<u32>,
    /// Row the live slot at this index occupies in the packed column.
    ///
    /// Rows are packed, so removing one moves the last row into the hole. This
    /// map and `row_slots` keep the *slot* index stable through that move, and
    /// the slot index is what a handle addresses. An entry for a free slot holds
    /// whatever the last occupant left.
    slot_rows: Vec<u32>,
    /// The slot that owns each row; the reverse of `slot_rows`.
    row_slots: Vec<u32>,
    /// Name lookup for assets that were added with a name, and its reverse.
    ///
    /// It holds only the slot index. A lookup reads the current generation from
    /// `generations`, and `remove` releases the name, so a dead name never
    /// reaches the asset that refilled the slot.
    name_index: KeyIndex<String>,
    /// Guid lookup, and its reverse.
    ///
    /// This is a map of its own rather than a hash of the name computed at
    /// lookup time. An asset may have a guid and no name - a cooked asset
    /// identified by an id - and a name must never answer a guid lookup, or the
    /// other way round.
    guid_index: KeyIndex<AssetGuid>,
    /// The import settings each slot was decoded with, stored as JSON.
    ///
    /// Only an asset decoded without a metadata file has an entry. Without it,
    /// a later `ensure_metadata` would write the type's default settings over
    /// the ones the asset was actually decoded with. JSON is used because the
    /// column is type-erased and `ImportSettings` only has to be
    /// `Serialize + DeserializeOwned` (the trait also requires
    /// `Default + Clone`). Entries are keyed by slot, so freeing the slot drops
    /// its entry.
    import_settings: HashMap<u32, serde_json::Value>,
}

// =============================================================================
// AssetGuid
// =============================================================================

/// A stable 128-bit identity for one asset.
///
/// A name is what a person writes and what a scene file carries. A guid is what
/// still identifies the asset after it is renamed. [`AssetManager`] keeps an
/// index for each, and both point at the same slot. An asset may have a name, a
/// guid, both or neither.
///
/// [`AssetGuid::from_name`] builds a guid from a string with the same FNV-1a
/// hash that a shared component identity uses. A cooking step and the runtime
/// therefore compute the same value from the same fixed name without exchanging
/// anything. A guid that a pipeline, a file or a database already assigned is
/// passed to [`AssetGuid::new`] instead, and it does not have to relate to any
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetGuid(u128);

impl AssetGuid {
    /// Wraps a guid that a cooking pipeline or a content database already
    /// assigned.
    pub const fn new(raw_bits: u128) -> Self {
        Self(raw_bits)
    }

    /// Derives a guid from a name with the shared-identity hash.
    ///
    /// This is the same function components use for their stable names, so one
    /// fixed string gives one identity everywhere: in a build step, in the host,
    /// and in a module that was compiled separately from either.
    pub const fn from_name(name: &str) -> Self {
        Self(crate::component::shared_component_identity(name))
    }

    /// Returns the raw 128-bit value.
    pub const fn value(self) -> u128 {
        self.0
    }

    /// Builds a new guid from the operating system's random source.
    ///
    /// This is what an asset's metadata file gets when it is first written. The
    /// guid is deliberately unrelated to the asset's path: a guid derived from
    /// the path would be handed to whatever file took that path later, and a
    /// stale reference would then point at the wrong asset instead of at
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns the `getrandom` error when the platform has no usable random
    /// source.
    pub fn random() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)?;
        Ok(Self(u128::from_le_bytes(bytes)))
    }

    /// Parses exactly 32 hexadecimal digits, the form
    /// [`Display`](std::fmt::Display) writes.
    ///
    /// Returns `None` for any other text; either letter case is accepted.
    pub fn parse(text: &str) -> Option<Self> {
        // `from_str_radix` on its own would also accept a sign and a shorter
        // string, so the length and the digits are checked first.
        if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(text, 16).ok().map(Self)
    }
}

impl std::fmt::Display for AssetGuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

// Serialized as the 32-digit hex string rather than a number: a 128-bit integer
// does not survive JSON number handling in most other tools, C# included.
impl serde::Serialize for AssetGuid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for AssetGuid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{text}` is not an asset guid (expected 32 hexadecimal digits)"
            ))
        })
    }
}

// =============================================================================
// AssetManager
// =============================================================================

/// The function that refreshes one asset type's column, given its storage key;
/// see [`refresh_column_ops`].
type AssetOpsRefresher = fn(&mut TraitTypeMap<dyn Asset, ErasedVecFamily>, TypeId);

/// Stores many assets per type, each addressed by a [`Handle`].
///
/// Inserted into a world like any other resource, and reached through
/// `Res<AssetManager>` / `ResMut<AssetManager>`:
///
/// ```
/// use pill_engine::asset::{Asset, AssetManager};
/// use trait_type_map::impl_trait_accessible;
///
/// #[derive(Debug, PartialEq)]
/// struct Mesh(&'static str);
/// impl Asset for Mesh {}
/// impl_trait_accessible!(dyn Asset; Mesh);
///
/// let mut assets = AssetManager::new();
/// let rock = assets.add(Mesh("rock"));
/// let tree = assets.add_named("tree", Mesh("tree")).expect("a fresh name");
///
/// assert_eq!(assets.get(rock), Some(&Mesh("rock")));
/// assert_eq!(assets.handle_by_name::<Mesh>("tree"), Some(tree));
/// ```
#[derive(Default)]
pub struct AssetManager {
    /// One erased column per asset type, holding the values themselves.
    columns: TraitTypeMap<dyn Asset, ErasedVecFamily>,
    /// Slot bookkeeping for each column, keyed by the same storage key.
    ///
    /// A storage key is a type's own `TypeId`, except for a shared asset type
    /// (see [`Asset::shared_name`]), whose key is the `TypeId` of the first
    /// binary that registered it; see [`Self::shared_keys`].
    metadata: HashMap<TypeId, AssetColumn>,
    /// The storage key of each shared asset type, by its shared identity.
    ///
    /// Written by the first registration and never replaced, so every binary's
    /// copy of the type reaches the same column. A `TypeId` is only a value
    /// here: it stays a valid key after the binary it came from is unloaded.
    shared_keys: HashMap<u128, TypeId>,
    /// The function that rebuilds each registered type's table, as recorded by
    /// the last registration of that type; see [`Self::rehome`].
    ops_refreshers: HashMap<TypeId, AssetOpsRefresher>,
    /// The name each asset type is registered under, looked up by its Rust type
    /// path.
    ///
    /// The name is the type's shared name, or its Rust type name when it has
    /// none. A caller that knows only the path - the C# mirror, which carries
    /// `std::any::type_name` - uses this to find the name that asset references
    /// use. The strings are owned copies.
    registered_names: HashMap<String, String>,
    /// The change counter that renderer upload caches compare against.
    revision: u64,
}

/// The shared name that the store's C# mirror registers the manager under
/// (`[NativeResource("pill_engine::AssetManager")]`).
///
/// The name is shared instead of being keyed by `TypeId`, so both languages end
/// up with the same identity: the managed mirror hashes the same string, and
/// every artifact that links the engine agrees on the identity without comparing
/// `TypeId`s.
pub const ASSET_MANAGER_SHARED_NAME: &str = "pill_engine::AssetManager";

impl Resource for AssetManager {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some(ASSET_MANAGER_SHARED_NAME)
    }
}

impl AssetManager {
    /// Returns the registered name of the asset type whose Rust type path is
    /// `type_path` (`std::any::type_name`), if a type with that path registered.
    /// This is the name a `Handle<T>` field's asset type is recorded as.
    pub fn registered_asset_type(&self, type_path: &str) -> Option<&str> {
        self.registered_names.get(type_path).map(String::as_str)
    }

    /// Returns the storage key that `T`'s column is kept under, or `None` when
    /// `T` is a shared type that no binary has registered yet.
    #[inline]
    fn key_of<T: Asset>(&self) -> Option<TypeId> {
        match T::shared_identity() {
            Some(identity) => self.shared_keys.get(&identity).copied(),
            None => Some(TypeId::of::<T>()),
        }
    }

    /// `T`'s column bookkeeping, or `None` when `T` has no column.
    #[inline]
    fn metadata_of<T: Asset>(&self) -> Option<&AssetColumn> {
        self.metadata.get(&self.key_of::<T>()?)
    }

    /// `T`'s column bookkeeping, mutably, or `None` when `T` has no column.
    #[inline]
    fn metadata_of_mut<T: Asset>(&mut self) -> Option<&mut AssetColumn> {
        let key = self.key_of::<T>()?;
        self.metadata.get_mut(&key)
    }

    /// `T`'s column.
    ///
    /// # Panics
    ///
    /// Panics when `T` has no column: every caller checks first.
    #[inline]
    fn column<T: Asset>(&self) -> &ErasedVecStorage<dyn Asset> {
        let key = self.key_of::<T>().expect("the asset type is registered");
        self.columns
            .get_trait_storage(key)
            .expect("the asset type's column exists")
    }

    /// `T`'s column, mutably.
    ///
    /// # Panics
    ///
    /// Panics when `T` has no column: every caller checks first.
    #[inline]
    fn column_mut<T: Asset>(&mut self) -> &mut ErasedVecStorage<dyn Asset> {
        let key = self.key_of::<T>().expect("the asset type is registered");
        self.columns
            .get_trait_storage_mut(key)
            .expect("the asset type's column exists")
    }

    /// Creates an empty manager with no asset types.
    ///
    /// Types register themselves on first use, so nothing has to be declared in
    /// advance: a project that loads no meshes carries no mesh column. Declaring
    /// a type with [`Self::register`] is for types owned by a part of the code
    /// that reloads; see that method for why it matters there.
    pub fn new() -> Self {
        Self::default()
    }

    /// The change counter that renderer upload caches compare against.
    ///
    /// It moves on every operation that may change stored asset data: adding an
    /// asset, borrowing one mutably, and removing one.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the content version of the slot that `handle` names, or `None`
    /// when the handle is stale.
    ///
    /// The version adds one whenever the value is borrowed mutably, so a
    /// consumer that mirrors assets can rebuild the one asset that changed
    /// instead of all of them. A version is only meaningful through one handle:
    /// a refilled slot keeps the counter its previous occupant left, so versions
    /// from two different handles cannot be compared.
    pub fn content_version<T>(&self, handle: Handle<T>) -> Option<u64>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }
        self.metadata_of::<T>()
            .and_then(|metadata| metadata.content_versions.get(handle.index as usize))
            .copied()
    }

    /// Stores `asset` and returns a handle to it.
    ///
    /// The type is registered first. That is also when its column, its metadata
    /// and its function table are created or refreshed, and when the layout of a
    /// shared column is checked. A slot freed by [`Self::remove`] is reused when
    /// one is free, so a project that loads and unloads repeatedly does not grow
    /// the column without bound.
    pub fn add<T>(&mut self, asset: T) -> Handle<T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        // Step 1: Create the type's column and metadata if this is its first
        // asset, or refresh them if it is not.
        self.register::<T>();

        let key = self.key_of::<T>().expect("register created the type");
        let storage = self
            .columns
            .get_trait_storage_mut(key)
            .expect("the column is created alongside its metadata");
        let metadata = self
            .metadata
            .get_mut(&key)
            .expect("column metadata is created alongside the column");

        // Step 2: Reuse a freed slot when one exists; otherwise add one and
        // give it generation 0.
        let index = match metadata.free_slots.pop() {
            Some(index) => index,
            None => {
                let index = metadata.slot_rows.len() as u32;
                metadata.slot_rows.push(0);
                metadata.generations.push(0);
                metadata.content_versions.push(0);
                index
            }
        };
        // Step 3: Put the value in a new row and point the slot at it. A
        // refilled slot never keeps the row its previous occupant used.
        let row = storage.len() as u32;
        storage.push(asset);
        metadata.slot_rows[index as usize] = row;
        metadata.row_slots.push(index);

        self.revision = self.revision.wrapping_add(1);

        Handle {
            index,
            generation: metadata.generations[index as usize],
            _marker: PhantomData,
        }
    }

    /// Stores `asset` under `name` and returns a handle to it.
    ///
    /// The name is a lookup key for code that finds assets by string, such as a
    /// scene file naming a mesh. A name that already addresses a live asset is
    /// refused rather than rebound, so loading the same key twice is reported
    /// instead of quietly adding a copy that nothing looks up. Remove the first
    /// asset with [`Self::remove`] when a replacement is intended.
    ///
    /// # Errors
    ///
    /// Returns [`AssetBindingError::NameInUse`] when `name` already addresses a
    /// live asset. Nothing is stored and no handle is issued.
    pub fn add_named<T>(
        &mut self,
        name: impl Into<String>,
        asset: T,
    ) -> AssetBindingResult<Handle<T>>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let name = name.into();
        // Checked before `add`, so a refused name cannot leave an unreachable
        // asset behind in the column.
        if self.handle_by_name::<T>(&name).is_some() {
            return Err(AssetBindingError::NameInUse { name });
        }

        let handle = self.add(asset);
        let metadata = self
            .metadata_of_mut::<T>()
            .expect("add created the column metadata");
        metadata.name_index.bind(name, handle.index);
        Ok(handle)
    }

    /// Renames the live asset `handle`, keeping its handle, guid and value.
    ///
    /// This is what following a moved source file needs: everything that holds
    /// the handle or the guid keeps working, and only lookups by the old name
    /// stop resolving. Renaming an asset to the name it already has does
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns [`AssetBindingError::NameInUse`] when another live asset has
    /// `new_name`, and [`AssetBindingError::StaleHandle`] when `handle` is
    /// stale. Nothing changes on an error.
    pub fn rename<T>(
        &mut self,
        handle: Handle<T>,
        new_name: impl Into<String>,
    ) -> AssetBindingResult<()>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let new_name = new_name.into();
        if !self.is_live(handle) {
            return Err(AssetBindingError::StaleHandle);
        }
        match self.handle_by_name::<T>(&new_name) {
            Some(existing) if existing == handle => return Ok(()),
            Some(_) => return Err(AssetBindingError::NameInUse { name: new_name }),
            None => {}
        }
        let metadata = self
            .metadata_of_mut::<T>()
            .expect("a live handle has column metadata");
        metadata.name_index.bind(new_name, handle.index);
        Ok(())
    }

    /// Stores `asset` under `guid` and returns a handle to it.
    ///
    /// Unlike [`Self::add_named`], a guid that is already in use is rebound: it
    /// points at the new asset and leaves the old one reachable by its handle.
    /// Nothing is unloaded.
    pub fn add_with_guid<T>(&mut self, guid: AssetGuid, asset: T) -> Handle<T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let handle = self.add(asset);
        self.bind_guid::<T>(guid, handle.index);
        handle
    }

    /// Stores `asset` under both a name and a guid.
    ///
    /// This is the usual shape for a cooked asset: the name is what a scene file
    /// writes, and the guid is what survives the name changing.
    ///
    /// # Errors
    ///
    /// Returns [`AssetBindingError::NameInUse`] on the same terms as
    /// [`Self::add_named`].
    pub fn add_named_with_guid<T>(
        &mut self,
        name: impl Into<String>,
        guid: AssetGuid,
        asset: T,
    ) -> AssetBindingResult<Handle<T>>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let handle = self.add_named(name, asset)?;
        self.bind_guid::<T>(guid, handle.index);
        Ok(handle)
    }

    /// Points `guid` at the asset that already lives in `slot_index`.
    ///
    /// Both guid-assigning entry points call this.
    fn bind_guid<T>(&mut self, guid: AssetGuid, slot_index: u32)
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let metadata = self
            .metadata_of_mut::<T>()
            .expect("add created the column metadata");
        metadata.guid_index.bind(guid, slot_index);
    }

    /// Borrows the asset that `handle` refers to, or returns `None` when the
    /// handle is stale.
    pub fn get<T>(&self, handle: Handle<T>) -> Option<&T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }
        let row = self.slot_row::<T>(handle.index)?;
        Some(self.column::<T>().get::<T>(row as usize))
    }

    /// Mutably borrows the asset that `handle` refers to, or returns `None` when
    /// the handle is stale.
    pub fn get_mut<T>(&mut self, handle: Handle<T>) -> Option<&mut T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        // One counter per slot, not one per column. It tells a mirrored
        // consumer which single asset changed, so borrowing one value does not
        // look like a change to all of them.
        let version = &mut self
            .metadata_of_mut::<T>()
            .expect("a live handle implies existing metadata")
            .content_versions[handle.index as usize];
        *version = version.wrapping_add(1);
        let row = self.slot_row::<T>(handle.index)?;
        Some(self.column_mut::<T>().get_mut::<T>(row as usize))
    }

    /// Returns a live handle for `name`, or `None` when nothing holds it.
    pub fn handle_by_name<T>(&self, name: &str) -> Option<Handle<T>>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let metadata = self.metadata_of::<T>()?;
        let index = metadata.name_index.slot_of(name)?;
        Some(Handle {
            index,
            generation: *metadata.generations.get(index as usize)?,
            _marker: PhantomData,
        })
    }

    /// Borrows the asset stored under `name`, or returns `None` when nothing
    /// holds it.
    pub fn get_by_name<T>(&self, name: &str) -> Option<&T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        self.get(self.handle_by_name::<T>(name)?)
    }

    /// Returns a live handle for `guid`, or `None` when nothing holds it.
    pub fn handle_by_guid<T>(&self, guid: AssetGuid) -> Option<Handle<T>>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let metadata = self.metadata_of::<T>()?;
        let index = metadata.guid_index.slot_of(&guid)?;
        Some(Handle {
            index,
            generation: *metadata.generations.get(index as usize)?,
            _marker: PhantomData,
        })
    }

    /// Borrows the asset stored under `guid`, or returns `None` when nothing
    /// holds it.
    pub fn get_by_guid<T>(&self, guid: AssetGuid) -> Option<&T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        self.get(self.handle_by_guid::<T>(guid)?)
    }

    /// Mutably borrows the asset stored under `name`, or returns `None` when
    /// nothing holds it.
    pub fn get_by_name_mut<T>(&mut self, name: &str) -> Option<&mut T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        self.get_mut(self.handle_by_name::<T>(name)?)
    }

    /// Mutably borrows the asset stored under `guid`, or returns `None` when
    /// nothing holds it.
    pub fn get_by_guid_mut<T>(&mut self, guid: AssetGuid) -> Option<&mut T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        self.get_mut(self.handle_by_guid::<T>(guid)?)
    }

    /// Returns the name that `handle` was stored under, or `None` when it has
    /// none.
    pub fn name_of<T>(&self, handle: Handle<T>) -> Option<&str>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }
        self.metadata_of::<T>()?
            .name_index
            .key_of(handle.index)
            .map(String::as_str)
    }

    /// Returns the guid that `handle` was stored under, or `None` when it has
    /// none.
    pub fn guid_of<T>(&self, handle: Handle<T>) -> Option<AssetGuid>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }
        self.metadata_of::<T>()?
            .guid_index
            .key_of(handle.index)
            .copied()
    }

    /// Records the import settings that `handle` was decoded with, as the JSON
    /// that a metadata file carries; see `asset_metadata`.
    ///
    /// The metadata layer calls this when it decoded an asset with settings of
    /// its own because no metadata file supplied any. A later `ensure_metadata`
    /// then writes those settings instead of the type's defaults.
    pub(crate) fn set_import_settings<T>(&mut self, handle: Handle<T>, settings: serde_json::Value)
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return;
        }
        if let Some(metadata) = self.metadata_of_mut::<T>() {
            metadata.import_settings.insert(handle.index, settings);
        }
    }

    /// Returns the import settings that `handle` was decoded with, when they
    /// were recorded; see [`Self::set_import_settings`].
    pub(crate) fn import_settings<T>(&self, handle: Handle<T>) -> Option<&serde_json::Value>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }
        self.metadata_of::<T>()?.import_settings.get(&handle.index)
    }

    /// Removes the asset that `handle` refers to and returns it.
    ///
    /// The slot is freed and its generation increases, so every outstanding
    /// handle to it - including this one - now resolves to `None`. Returns
    /// `None` when the handle was already stale, so removing twice does nothing
    /// instead of failing.
    pub fn remove<T>(&mut self, handle: Handle<T>) -> Option<T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if !self.is_live(handle) {
            return None;
        }

        // Step 1: Find the row the slot occupies and take the value out of the
        // packed column.
        let storage_key = self
            .key_of::<T>()
            .expect("a live handle implies a registered type");
        let row = self
            .metadata
            .get(&storage_key)
            .expect("a live handle implies existing metadata")
            .slot_rows[handle.index as usize];

        let storage = self
            .columns
            .get_trait_storage_mut(storage_key)
            .expect("a live handle implies an existing column");
        let asset = storage.swap_remove::<T>(row as usize);
        // Step 2: A packed column moves its last row into the hole. The slot
        // that owned that row takes over the vacated slot index below, so
        // handles follow the slot, not the row.
        let row_moved = (row as usize) < storage.len();

        let metadata = self
            .metadata
            .get_mut(&storage_key)
            .expect("a live handle implies existing metadata");

        // The popped entry is the slot of the row that moved in, or the removed
        // slot itself when the last row went out; neither case needs a fix-up.
        let last_row_slot = metadata.row_slots.pop().expect("a live slot owns a row");
        if row_moved {
            metadata.slot_rows[last_row_slot as usize] = row;
            metadata.row_slots[row as usize] = last_row_slot;
        }

        // Step 3: Increase the generation so this handle, and every copy of it,
        // stops resolving. It saturates instead of wrapping: at u32::MAX the
        // slot is retired rather than allowed to validate old handles again.
        let generation = &mut metadata.generations[handle.index as usize];
        *generation = generation.saturating_add(1);
        if *generation < u32::MAX {
            metadata.free_slots.push(handle.index);
        }

        // Step 4: Drop the name, guid and import settings that belonged to the
        // slot.
        metadata.name_index.release(handle.index);
        metadata.guid_index.release(handle.index);
        metadata.import_settings.remove(&handle.index);

        self.revision = self.revision.wrapping_add(1);

        Some(asset)
    }

    /// Returns whether `handle` still refers to a live asset.
    pub fn contains<T>(&self, handle: Handle<T>) -> bool
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        self.is_live(handle) && self.slot_row::<T>(handle.index).is_some()
    }

    /// Returns the number of live assets of type `T`.
    pub fn len<T>(&self) -> usize
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        if self.metadata_of::<T>().is_none() {
            return 0;
        }
        self.column::<T>().len()
    }

    /// Returns whether the store holds no asset of type `T`.
    pub fn is_empty<T>(&self) -> bool
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        self.len::<T>() == 0
    }

    /// Iterates over every live asset of type `T`.
    ///
    /// The order is row order: insertion order until the first removal, and
    /// arbitrary after that. A caller that needs a stable order has to impose
    /// one.
    pub fn iter<T>(&self) -> impl Iterator<Item = &T>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        // An unregistered type yields nothing; the column does not have to
        // exist.
        self.metadata_of::<T>()
            .is_some()
            .then(|| self.column::<T>().iter::<T>())
            .into_iter()
            .flatten()
    }

    /// Iterates over every live asset of type `T`, together with its handle.
    pub fn iter_handles<T>(&self) -> impl Iterator<Item = (Handle<T>, &T)>
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        let metadata = self.metadata_of::<T>();
        metadata
            .map(|metadata| {
                self.column::<T>()
                    .iter::<T>()
                    .enumerate()
                    .map(move |(row, asset)| {
                        // Rows are packed. The handle names the slot that the
                        // row lives in, because the slot index is what survives
                        // a swap.
                        let slot = metadata.row_slots[row];
                        (
                            Handle {
                                index: slot,
                                generation: metadata.generations[slot as usize],
                                _marker: PhantomData,
                            },
                            asset,
                        )
                    })
            })
            .into_iter()
            .flatten()
    }

    /// Returns whether `handle`'s generation still matches its slot's.
    ///
    /// Every accessor checks handles here, so they all agree on what "stale"
    /// means.
    fn is_live<T: Asset>(&self, handle: Handle<T>) -> bool {
        self.metadata_of::<T>()
            .and_then(|metadata| metadata.generations.get(handle.index as usize))
            .is_some_and(|generation| *generation == handle.generation)
    }

    /// Returns the row that the live slot at `index` occupies, or `None` when
    /// nothing lives there.
    ///
    /// The check through `row_slots` is what proves the slot is occupied. A
    /// freed slot's row entry belongs to another slot or to no row, so even a
    /// handle that guessed a freed slot's generation cannot reach another
    /// asset's row.
    fn slot_row<T: Asset>(&self, index: u32) -> Option<u32> {
        let metadata = self.metadata_of::<T>()?;
        let row = *metadata.slot_rows.get(index as usize)?;
        (metadata.row_slots.get(row as usize) == Some(&index)).then_some(row)
    }

    /// Declares `T` as an asset type and points its column at the code that is
    /// running now.
    ///
    /// Using a type creates its column anyway, but an explicit declaration also
    /// matters across a reload. The column's function table is code from the
    /// copy that filled it, so the copy that owns an asset type should declare
    /// the type when it registers, even if it adds no asset this run. Declaring
    /// twice is harmless: the second call only refreshes the table.
    ///
    /// A shared asset type ([`Asset::shared_name`]) is keyed by the `TypeId` of
    /// the first binary that registered it.
    ///
    /// # Panics
    ///
    /// Panics when a later registrant of a shared type has a copy of another
    /// size or alignment, before that copy can read a row.
    pub fn register<T>(&mut self)
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        // Step 1: Record the name this type is registered under.
        let type_path = std::any::type_name::<T>();
        self.registered_names.insert(
            type_path.to_owned(),
            T::shared_name().unwrap_or(type_path).to_owned(),
        );
        // Step 2: Find the storage key, creating it for a shared type.
        let key = match T::shared_identity() {
            Some(identity) => *self
                .shared_keys
                .entry(identity)
                .or_insert_with(TypeId::of::<T>),
            None => TypeId::of::<T>(),
        };
        // Step 3: Refresh an existing column, or create one.
        if let Some(metadata) = self.metadata.get(&key) {
            // Checked here, where a mismatch is a registration problem with one
            // clear cause, rather than at the first read of a row.
            assert!(
                metadata.element_layout == (std::mem::size_of::<T>(), std::mem::align_of::<T>()),
                "asset type {} does not match the layout of the shared column it is registered to",
                std::any::type_name::<T>()
            );
            self.ops_refreshers.insert(key, refresh_column_ops::<T>);
            if let Some(column) = self.columns.get_trait_storage_mut(key) {
                column.refresh_ops(ErasedVecStorageOps::of::<T>());
            }
            return;
        }
        self.ops_refreshers.insert(key, refresh_column_ops::<T>);
        if T::shared_identity().is_some() {
            self.columns
                .insert_erased(ErasedVecStorage::new(ErasedVecStorageInfo::of_shared::<T>()));
        } else {
            self.columns.register_type_storage::<T>();
        }
        self.metadata.insert(
            key,
            AssetColumn {
                element_layout: (std::mem::size_of::<T>(), std::mem::align_of::<T>()),
                ..AssetColumn::default()
            },
        );
    }

    /// Points every registered asset column at its newest function table.
    ///
    /// The reload transaction calls this next to the component and resource
    /// re-homing passes, after the new copy of the code registered and before
    /// the old copy can be unloaded. A column keeps working only while its table
    /// points at code that is still loaded. A type that the new copy neither
    /// declared nor used keeps the table it already has, because there is
    /// nothing newer to point it at.
    pub fn rehome(&mut self) {
        for (key, refresher) in &self.ops_refreshers {
            refresher(&mut self.columns, *key);
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Rebuilds one asset type's function table from the code that is running now.
///
/// [`AssetManager::register`] records this function's address for each type. A
/// freshly loaded copy of the code calls it to point that type's column at its
/// own functions. Without that, the column would still call into the copy that
/// filled it and stop working once that copy is unloaded.
fn refresh_column_ops<T>(
    columns: &mut TraitTypeMap<dyn Asset, ErasedVecFamily>,
    storage_key: TypeId,
) where
    T: Asset + TraitAccessible<dyn Asset>,
{
    if let Some(column) = columns.get_trait_storage_mut(storage_key) {
        column.refresh_ops(ErasedVecStorageOps::of::<T>());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use trait_type_map::impl_trait_accessible;

    /// A rename keeps the handle and the guid, moves the name, and refuses a
    /// name that another asset holds or a stale handle.
    #[test]
    fn rename_keeps_the_handle_and_guid_and_refuses_a_taken_name() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::new(7);
        let moved = assets
            .add_named_with_guid("old.png", guid, Mesh("moved"))
            .unwrap();
        let other = assets.add_named("taken.png", Mesh("other")).unwrap();

        assets.rename(moved, "new.png").unwrap();
        assert_eq!(assets.handle_by_name::<Mesh>("new.png"), Some(moved));
        assert_eq!(assets.handle_by_name::<Mesh>("old.png"), None);
        assert_eq!(assets.handle_by_guid::<Mesh>(guid), Some(moved));
        assert_eq!(assets.name_of(moved), Some("new.png"));
        assert_eq!(assets.get(moved), Some(&Mesh("moved")));

        // Renaming to its own name is a no-op; a taken name is refused.
        assets.rename(moved, "new.png").unwrap();
        assert!(matches!(
            assets.rename(moved, "taken.png"),
            Err(AssetBindingError::NameInUse { .. })
        ));
        assert_eq!(assets.handle_by_name::<Mesh>("taken.png"), Some(other));

        assets.remove(moved);
        assert!(matches!(
            assets.rename(moved, "again.png"),
            Err(AssetBindingError::StaleHandle)
        ));
    }

    #[derive(Debug, PartialEq)]
    struct Mesh(&'static str);
    impl Asset for Mesh {}
    impl_trait_accessible!(dyn Asset; Mesh);

    #[derive(Debug, PartialEq)]
    struct Texture(u32);
    impl Asset for Texture {}
    impl_trait_accessible!(dyn Asset; Texture);

    /// The shared name the copies below register under, standing in for one
    /// asset type compiled into two binaries (two distinct `TypeId`s).
    const SHARED_MESH: &str = "asset_tests::SharedMesh";

    /// One binary's copy of the shared asset type.
    #[derive(Debug, PartialEq)]
    struct WriterMesh {
        vertices: Vec<u32>,
    }
    impl Asset for WriterMesh {
        fn shared_name() -> Option<&'static str> {
            Some(SHARED_MESH)
        }
    }
    impl_trait_accessible!(dyn Asset; WriterMesh);

    /// Another binary's copy of the same type.
    #[derive(Debug, PartialEq)]
    struct ReaderMesh {
        vertices: Vec<u32>,
    }
    impl Asset for ReaderMesh {
        fn shared_name() -> Option<&'static str> {
            Some(SHARED_MESH)
        }
    }
    impl_trait_accessible!(dyn Asset; ReaderMesh);

    /// A type claiming the same shared name with a different layout.
    #[derive(Debug, PartialEq)]
    struct MismatchedMesh(u8);
    impl Asset for MismatchedMesh {
        fn shared_name() -> Option<&'static str> {
            Some(SHARED_MESH)
        }
    }
    impl_trait_accessible!(dyn Asset; MismatchedMesh);

    /// Two copies of one shared asset type reach one column.
    ///
    /// What one copy stores, the other finds by handle, by name and by
    /// iteration, and it can add to and remove from the same column.
    #[test]
    fn copies_of_a_shared_asset_type_share_one_column() {
        let mut assets = AssetManager::new();
        let written = assets
            .add_named(
                "cube",
                WriterMesh {
                    vertices: vec![1, 2, 3],
                },
            )
            .expect("a fresh name");

        let as_reader = Handle::<ReaderMesh>::from_raw(written.index(), written.generation());
        assert_eq!(
            assets.get(as_reader).map(|mesh| mesh.vertices.clone()),
            Some(vec![1, 2, 3])
        );
        assert_eq!(assets.handle_by_name::<ReaderMesh>("cube"), Some(as_reader));
        assert_eq!(assets.len::<ReaderMesh>(), 1);

        let added = assets.add(ReaderMesh { vertices: vec![4] });
        assert_eq!(assets.len::<WriterMesh>(), 2);
        assert_eq!(
            assets
                .iter::<WriterMesh>()
                .map(|mesh| mesh.vertices.len())
                .sum::<usize>(),
            4
        );

        assert_eq!(
            assets.remove(added).map(|mesh| mesh.vertices),
            Some(vec![4])
        );
        assert_eq!(assets.len::<WriterMesh>(), 1);
    }

    /// A reader that looks before any copy registered sees nothing, rather
    /// than a column of its own.
    #[test]
    fn a_shared_type_nobody_registered_has_no_assets() {
        let assets = AssetManager::new();
        assert_eq!(assets.len::<ReaderMesh>(), 0);
        assert_eq!(assets.handle_by_name::<ReaderMesh>("cube"), None);
    }

    /// Registering a second copy re-points the column's function table and
    /// keeps its rows, as a reloaded module's registration does.
    #[test]
    fn a_later_copy_registers_onto_the_existing_column() {
        let mut assets = AssetManager::new();
        assets.add(WriterMesh { vertices: vec![7] });
        assets.register::<ReaderMesh>();
        assets.rehome();
        assert_eq!(assets.len::<ReaderMesh>(), 1);
    }

    /// A copy whose layout differs is refused when it registers.
    #[test]
    #[should_panic(expected = "does not match the layout of the shared column")]
    fn a_shared_type_with_a_different_layout_is_refused() {
        let mut assets = AssetManager::new();
        assets.add(WriterMesh { vertices: vec![1] });
        assets.register::<MismatchedMesh>();
    }

    /// Unshared types keep their own columns even when they look alike.
    #[test]
    fn unshared_types_keep_separate_columns() {
        let mut assets = AssetManager::new();
        assets.add(Mesh("rock"));
        assert_eq!(assets.len::<Mesh>(), 1);
        assert_eq!(assets.len::<Texture>(), 0);
    }

    /// A guid resolves to the asset stored under it.
    ///
    /// The name and the guid are looked up separately: a name lookup does not
    /// answer a guid, and the other way round.
    #[test]
    fn resolves_an_asset_by_guid() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::from_name("textures/grass");
        let grass = assets.add_with_guid(guid, Texture(7));

        assert_eq!(assets.get_by_guid::<Texture>(guid), Some(&Texture(7)));
        assert_eq!(assets.handle_by_guid::<Texture>(guid), Some(grass));
        // Stored with a guid alone, so no name resolves to it.
        assert_eq!(assets.get_by_name::<Texture>("textures/grass"), None);
    }

    /// The usual cooked-asset shape: both keys address the same slot.
    #[test]
    fn name_and_guid_address_one_asset() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::new(0xdead_beef);
        let handle = assets
            .add_named_with_guid("grass", guid, Texture(1))
            .expect("a fresh name");

        assert_eq!(assets.handle_by_name::<Texture>("grass"), Some(handle));
        assert_eq!(assets.handle_by_guid::<Texture>(guid), Some(handle));
        assert_eq!(assets.name_of(handle), Some("grass"));
        assert_eq!(assets.guid_of(handle), Some(guid));
    }

    /// Freeing a slot must drop its guid entry. Otherwise the next asset to
    /// take that slot would answer the dead guid - the wrong-asset failure that
    /// the generational handle prevents, coming back through the index.
    #[test]
    fn removing_an_asset_releases_its_guid() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::from_name("textures/grass");
        let grass = assets.add_with_guid(guid, Texture(7));

        assets.remove(grass);
        assert_eq!(assets.get_by_guid::<Texture>(guid), None);

        // The freed slot is reused; the stale guid must not reach the new value.
        let stone = assets.add(Texture(9));
        assert_eq!(stone.index(), grass.index());
        assert_eq!(assets.get_by_guid::<Texture>(guid), None);
        assert_eq!(assets.guid_of(stone), None);
    }

    /// Rebinding points the guid at the new asset and leaves the old one
    /// reachable by its handle.
    ///
    /// Guids rebind where names are refused. A guid is the identity that a cook
    /// step pins an asset to, not a lookup key that a second load can collide
    /// on.
    #[test]
    fn rebinding_a_guid_moves_it_to_the_new_asset() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::new(42);
        let first = assets.add_with_guid(guid, Texture(1));
        let second = assets.add_with_guid(guid, Texture(2));

        assert_eq!(assets.get_by_guid::<Texture>(guid), Some(&Texture(2)));
        assert_eq!(assets.get(first), Some(&Texture(1)));
        // The displaced asset keeps its slot but no longer claims the guid.
        assert_eq!(assets.guid_of(first), None);
        assert_eq!(assets.guid_of(second), Some(guid));
    }

    /// Guids are per-type, like every other column key: one value does not
    /// collide across two asset types.
    #[test]
    fn guids_do_not_collide_across_types() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::new(1);
        assets.add_with_guid(guid, Texture(5));
        assets.add_with_guid(guid, Mesh("rock"));

        assert_eq!(assets.get_by_guid::<Texture>(guid), Some(&Texture(5)));
        assert_eq!(assets.get_by_guid::<Mesh>(guid), Some(&Mesh("rock")));
    }

    /// The name hash is a fixed function of the bytes, which is what lets a
    /// cooking step and the runtime derive one identity from one name.
    #[test]
    fn guid_from_name_is_stable_and_distinguishing() {
        assert_eq!(
            AssetGuid::from_name("textures/grass"),
            AssetGuid::from_name("textures/grass")
        );
        assert_ne!(
            AssetGuid::from_name("textures/grass"),
            AssetGuid::from_name("textures/stone")
        );
    }

    /// A stale handle answers nothing, on the guid accessors as on the rest.
    #[test]
    fn a_stale_handle_has_no_name_or_guid() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::new(3);
        let handle = assets
            .add_named_with_guid("grass", guid, Texture(1))
            .expect("a fresh name");
        assets.remove(handle);

        assert_eq!(assets.name_of(handle), None);
        assert_eq!(assets.guid_of(handle), None);
    }

    /// Mutable lookup by either key reaches the same value.
    #[test]
    fn mutable_lookup_by_name_and_guid() {
        let mut assets = AssetManager::new();
        let guid = AssetGuid::new(11);
        let handle = assets
            .add_named_with_guid("grass", guid, Texture(1))
            .expect("a fresh name");

        assets.get_by_name_mut::<Texture>("grass").unwrap().0 = 2;
        assert_eq!(assets.get(handle), Some(&Texture(2)));

        assets.get_by_guid_mut::<Texture>(guid).unwrap().0 = 3;
        assert_eq!(assets.get(handle), Some(&Texture(3)));
    }

    /// The point of the module: one type, many live values.
    #[test]
    fn stores_many_assets_of_one_type() {
        let mut assets = AssetManager::new();
        let rock = assets.add(Mesh("rock"));
        let tree = assets.add(Mesh("tree"));

        assert_ne!(rock, tree);
        assert_eq!(assets.get(rock), Some(&Mesh("rock")));
        assert_eq!(assets.get(tree), Some(&Mesh("tree")));
        assert_eq!(assets.len::<Mesh>(), 2);
    }

    /// Columns are per type and do not interfere.
    #[test]
    fn asset_types_are_stored_independently() {
        let mut assets = AssetManager::new();
        let mesh = assets.add(Mesh("rock"));
        let texture = assets.add(Texture(7));

        assert_eq!(assets.get(mesh), Some(&Mesh("rock")));
        assert_eq!(assets.get(texture), Some(&Texture(7)));
        assert_eq!(assets.len::<Mesh>(), 1);
        assert_eq!(assets.len::<Texture>(), 1);
    }

    /// An unused type reads as empty rather than panicking on a missing column.
    #[test]
    fn unregistered_types_read_as_empty() {
        let assets = AssetManager::new();

        assert_eq!(assets.len::<Mesh>(), 0);
        assert!(assets.is_empty::<Mesh>());
        assert_eq!(assets.iter::<Mesh>().count(), 0);
        assert_eq!(assets.handle_by_name::<Mesh>("rock"), None);
        assert_eq!(assets.get_by_name::<Mesh>("rock"), None);
    }

    /// Assets are mutable in place through their handle.
    #[test]
    fn assets_can_be_mutated_through_a_handle() {
        let mut assets = AssetManager::new();
        let handle = assets.add(Texture(1));

        *assets.get_mut(handle).expect("live handle") = Texture(2);

        assert_eq!(assets.get(handle), Some(&Texture(2)));
    }

    /// Render upload caches can cheaply observe every data-changing operation.
    #[test]
    fn revision_changes_when_asset_data_can_change() {
        let mut assets = AssetManager::new();
        let initial = assets.revision();
        let handle = assets.add(Texture(1));
        let after_add = assets.revision();
        assert_ne!(after_add, initial);

        assets.get_mut(handle).expect("live handle").0 = 2;
        let after_mutation = assets.revision();
        assert_ne!(after_mutation, after_add);

        assets.remove(handle);
        assert_ne!(assets.revision(), after_mutation);
    }

    /// The per-slot content counter lets a GPU uploader rebuild the one asset
    /// that was edited instead of all of them. Only a mutable borrow moves it.
    #[test]
    fn content_version_moves_only_on_a_mutable_borrow() {
        let mut assets = AssetManager::new();
        let handle = assets.add(Texture(1));

        let created = assets.content_version(handle).expect("live handle");
        assert_eq!(
            assets.content_version(handle),
            Some(created),
            "reading leaves the version alone"
        );

        assets.get_mut(handle).expect("live handle").0 = 2;
        assert_eq!(
            assets.content_version(handle),
            Some(created + 1),
            "editing moves it"
        );

        assets.remove(handle);
        assert_eq!(
            assets.content_version(handle),
            None,
            "a stale handle answers nothing"
        );
    }

    /// Removing yields the asset and invalidates the handle.
    #[test]
    fn removing_returns_the_asset_and_invalidates_the_handle() {
        let mut assets = AssetManager::new();
        let handle = assets.add(Mesh("rock"));

        assert_eq!(assets.remove(handle), Some(Mesh("rock")));
        assert_eq!(assets.get(handle), None);
        assert!(!assets.contains(handle));
        // Removing twice is a no-op, not a panic.
        assert_eq!(assets.remove(handle), None);
    }

    /// The reason handles carry a generation: a refilled slot must not
    /// resurrect an old handle as a pointer to the new asset.
    #[test]
    fn a_stale_handle_does_not_alias_the_asset_that_reuses_its_slot() {
        let mut assets = AssetManager::new();
        let rock = assets.add(Mesh("rock"));
        assets.remove(rock);

        let tree = assets.add(Mesh("tree"));

        // The slot is reused, which is exactly the dangerous case.
        assert_eq!(rock.index(), tree.index());
        assert_ne!(rock.generation(), tree.generation());

        assert_eq!(assets.get(rock), None, "the stale handle must not resolve");
        assert_eq!(assets.get(tree), Some(&Mesh("tree")));
    }

    /// Freed slots are refilled rather than leaked.
    #[test]
    fn freed_slots_are_reused_before_the_column_grows() {
        let mut assets = AssetManager::new();
        let first = assets.add(Mesh("a"));
        let second = assets.add(Mesh("b"));
        assets.remove(first);

        let third = assets.add(Mesh("c"));

        assert_eq!(third.index(), first.index());
        assert_eq!(assets.len::<Mesh>(), 2);
        assert_eq!(assets.get(second), Some(&Mesh("b")));
    }

    /// Names resolve to handles and follow removal.
    #[test]
    fn names_resolve_to_live_handles() {
        let mut assets = AssetManager::new();
        let handle = assets
            .add_named("rock", Mesh("rock"))
            .expect("a fresh name");

        assert_eq!(assets.handle_by_name::<Mesh>("rock"), Some(handle));
        assert_eq!(assets.get_by_name::<Mesh>("rock"), Some(&Mesh("rock")));

        assets.remove(handle);
        assert_eq!(assets.handle_by_name::<Mesh>("rock"), None);
        assert_eq!(assets.get_by_name::<Mesh>("rock"), None);
    }

    /// A name that is already bound is refused, and the asset holding it is
    /// left exactly as it was - the collision is reported, not ignored.
    #[test]
    fn a_taken_name_is_refused() {
        let mut assets = AssetManager::new();
        let original = assets
            .add_named("mesh", Mesh("first"))
            .expect("a fresh name");

        let refused = assets.add_named("mesh", Mesh("second"));

        assert!(matches!(
            refused,
            Err(AssetBindingError::NameInUse { ref name }) if name == "mesh"
        ));
        assert_eq!(assets.handle_by_name::<Mesh>("mesh"), Some(original));
        assert_eq!(assets.get(original), Some(&Mesh("first")));
        // The refused asset never entered the column, so nothing leaked either.
        assert_eq!(assets.len::<Mesh>(), 1);
    }

    /// Freeing a slot frees the name with it, so a later load can take it.
    #[test]
    fn a_removed_name_can_be_bound_again() {
        let mut assets = AssetManager::new();
        let first = assets
            .add_named("mesh", Mesh("first"))
            .expect("a fresh name");
        assets.remove(first);

        let second = assets
            .add_named("mesh", Mesh("second"))
            .expect("removal freed the name");

        assert_eq!(assets.handle_by_name::<Mesh>("mesh"), Some(second));
        assert_eq!(assets.get_by_name::<Mesh>("mesh"), Some(&Mesh("second")));
    }

    /// Iteration visits live assets only.
    #[test]
    fn iteration_skips_removed_slots() {
        let mut assets = AssetManager::new();
        let first = assets.add(Mesh("a"));
        assets.add(Mesh("b"));
        assets.add(Mesh("c"));
        assets.remove(first);

        let mut names: Vec<&str> = assets.iter::<Mesh>().map(|mesh| mesh.0).collect();
        names.sort_unstable();
        assert_eq!(names, ["b", "c"]);
    }

    /// Handle iteration returns handles that actually resolve.
    #[test]
    fn iter_handles_returns_resolvable_handles() {
        let mut assets = AssetManager::new();
        let first = assets.add(Mesh("a"));
        assets.add(Mesh("b"));
        assets.remove(first);
        assets.add(Mesh("c"));

        let pairs: Vec<(Handle<Mesh>, &Mesh)> = assets.iter_handles::<Mesh>().collect();
        assert_eq!(pairs.len(), 2);
        for (handle, asset) in pairs {
            assert_eq!(assets.get(handle), Some(asset));
        }
    }

    /// The packed column swaps rows on removal; a handle follows its slot, not
    /// the row its value was moved out of.
    #[test]
    fn removing_the_middle_asset_keeps_the_others_addressable() {
        let mut assets = AssetManager::new();
        let a = assets.add(Mesh("a"));
        let b = assets.add(Mesh("b"));
        let c = assets.add(Mesh("c"));

        assert_eq!(assets.remove(b), Some(Mesh("b")));
        assert_eq!(assets.get(a), Some(&Mesh("a")));
        assert_eq!(assets.get(c), Some(&Mesh("c")));

        let d = assets.add(Mesh("d"));
        assert_eq!(d.index(), b.index(), "the freed slot is refilled");
        assert_eq!(assets.get(a), Some(&Mesh("a")));
        assert_eq!(assets.get(c), Some(&Mesh("c")));
        assert_eq!(assets.get(d), Some(&Mesh("d")));
        assert_eq!(assets.len::<Mesh>(), 3);
    }

    /// Declaring a type re-points its column without disturbing its contents.
    #[test]
    fn declaring_a_type_twice_keeps_its_assets() {
        let mut assets = AssetManager::new();
        let handle = assets
            .add_named("rock", Mesh("rock"))
            .expect("a fresh name");

        assets.register::<Mesh>();

        assert_eq!(assets.get(handle), Some(&Mesh("rock")));
        assert_eq!(assets.handle_by_name::<Mesh>("rock"), Some(handle));
        assert_eq!(assets.len::<Mesh>(), 1);
    }

    /// The re-home pass swaps every column's table; values, handles and names
    /// must survive it untouched.
    #[test]
    fn rehoming_leaves_assets_and_handles_intact() {
        let mut assets = AssetManager::new();
        let shared = assets.add(Mesh("shared"));
        let doomed = assets.add(Mesh("doomed"));
        assets.remove(doomed);
        let named = assets
            .add_named("tree", Mesh("tree"))
            .expect("a fresh name");

        assets.rehome();

        assert_eq!(assets.get(shared), Some(&Mesh("shared")));
        assert_eq!(assets.get(named), Some(&Mesh("tree")));
        assert_eq!(assets.handle_by_name::<Mesh>("tree"), Some(named));
        let mut names: Vec<&str> = assets.iter::<Mesh>().map(|mesh| mesh.0).collect();
        names.sort_unstable();
        assert_eq!(names, ["shared", "tree"]);
    }

    /// The column still drops its values through its function table after the
    /// table has been swapped - the property the whole design exists to keep.
    #[test]
    fn dropping_the_manager_drops_its_assets() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        impl Asset for DropProbe {}
        impl_trait_accessible!(dyn Asset; DropProbe);

        let drops = Arc::new(AtomicUsize::new(0));
        {
            let mut assets = AssetManager::new();
            assets.add(DropProbe(Arc::clone(&drops)));
            assets.rehome();
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    /// Handles are inert values: copyable, comparable, and hashable, so they
    /// can sit inside components and map keys.
    #[test]
    fn handles_are_copyable_and_hashable() {
        use std::collections::HashSet;

        let mut assets = AssetManager::new();
        let handle = assets.add(Mesh("rock"));
        let copy = handle;

        assert_eq!(handle, copy);
        let mut set = HashSet::new();
        set.insert(handle);
        assert!(set.contains(&copy));
    }

    /// The manager is a resource, so it reaches systems through the `Res` /
    /// `ResMut` parameters like any other.
    #[test]
    fn manager_round_trips_through_the_world_as_a_resource() {
        use crate::world::World;

        let mut world = World::new();
        let mut assets = AssetManager::new();
        let handle = assets.add(Mesh("rock"));
        world.insert_resource(assets);

        let stored = world
            .get_resource::<AssetManager>()
            .expect("manager was inserted");
        assert_eq!(stored.get(handle), Some(&Mesh("rock")));
    }

    /// The world-side API the reload transaction uses: declaring a type
    /// creates the manager on first use, and the re-home pass runs through it.
    #[cfg(feature = "development")]
    #[test]
    fn world_registers_and_rehomes_asset_types() {
        use crate::world::World;

        let mut world = World::new();
        world.register_asset::<Mesh>();
        let handle = world
            .get_resource_mut::<AssetManager>()
            .expect("register_asset creates the manager")
            .add(Mesh("rock"));

        world.rehome_assets();

        assert_eq!(
            world
                .get_resource::<AssetManager>()
                .and_then(|assets| assets.get(handle)),
            Some(&Mesh("rock"))
        );
    }

    /// A loader over embedded bytes hands them back unchanged.
    #[test]
    fn asset_loader_returns_embedded_bytes() {
        let loader = AssetLoader::Bytes(vec![1, 2, 3].into_boxed_slice());
        assert_eq!(loader.load().unwrap(), [1, 2, 3]);
    }

    /// A path is looked up below the mounted asset directory.
    #[test]
    fn asset_loader_resolves_paths_below_its_root() {
        let _mounted = crate::asset_store::mounted_directory_test_lock();
        let root = std::env::temp_dir().join(format!(
            "pill-asset-loader-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("sample.wgsl"), b"shader").unwrap();
        AssetLoader::set_root(&root);

        let bytes = AssetLoader::Path("sample.wgsl".into()).load().unwrap();

        assert_eq!(bytes, b"shader");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A guid is written as the 32-digit hex string `Display` gives, and reads
    /// back to the same value.
    #[test]
    fn a_guid_serializes_as_32_hex_digits() {
        let guid = AssetGuid::new(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        let json = serde_json::to_string(&guid).unwrap();
        assert_eq!(json, "\"0123456789abcdef0011223344556677\"");
        assert_eq!(serde_json::from_str::<AssetGuid>(&json).unwrap(), guid);
        // Leading zeros are kept, so every guid has the same width.
        assert_eq!(
            serde_json::to_string(&AssetGuid::new(1)).unwrap(),
            "\"00000000000000000000000000000001\""
        );
    }

    /// Anything but exactly 32 hex digits is refused rather than read as some
    /// other guid.
    #[test]
    fn malformed_guid_text_is_refused() {
        for text in [
            "",
            "abc",
            "+0123456789abcdef0011223344556677",
            "0123456789abcdef001122334455667g",
        ] {
            assert_eq!(AssetGuid::parse(text), None, "{text:?}");
        }
        assert!(serde_json::from_str::<AssetGuid>("\"xyz\"").is_err());
        assert!(serde_json::from_str::<AssetGuid>("42").is_err());
        assert_eq!(
            AssetGuid::parse("0123456789ABCDEF0011223344556677"),
            Some(AssetGuid::new(0x0123_4567_89ab_cdef_0011_2233_4455_6677))
        );
    }

    /// Random guids differ from each other and survive a round trip.
    #[test]
    fn random_guids_are_distinct_and_round_trip() {
        let first = AssetGuid::random().expect("a random source");
        let second = AssetGuid::random().expect("a random source");
        assert_ne!(first, second);
        let json = serde_json::to_string(&first).unwrap();
        assert_eq!(serde_json::from_str::<AssetGuid>(&json).unwrap(), first);
    }
}
