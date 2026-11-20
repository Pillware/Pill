//! The resources the editor lets a person inspect and edit.
//!
//! # Responsibilities
//!
//! - List the world resources that are *editable*: the engine keeps no
//!   resource reflection, so a resource joins this list by having a curated
//!   field table beside it (the gizmo settings' is
//!   [`crate::gizmo::RESOURCE_FIELDS`]).
//! - Expose, per resource, its label and the field table the Inspector reads
//!   and the write path goes through.
//!
//! # Design
//!
//! Each resource's field table is the single source of the row order, the
//! labels and the field names a write goes through, so the tab cannot offer a
//! field the write path would refuse. Adding a resource is one entry here
//! plus its own read/write table, the same shape the gizmo's table has.

// Current crate
use crate::gizmo;

/// One editable world resource.
#[derive(Clone, Copy, Debug)]
pub struct ResourceEntry {
    /// The stable name a command addresses the resource by - its type path.
    pub name: &'static str,
    /// The label the Inspector's section shows.
    pub label: &'static str,
    /// The editable fields, in row order. This is the resource's own table,
    /// the same one its write path goes through.
    pub fields: &'static [gizmo::ResourceField],
}

/// The resources the Inspector can show, in list order.
pub const EDITABLE_RESOURCES: &[ResourceEntry] = &[ResourceEntry {
    name: gizmo::RESOURCE_NAME,
    label: "Gizmo Settings",
    fields: gizmo::RESOURCE_FIELDS,
}];

/// The entry for the resource called `name`, when the list holds one.
pub fn entry(name: &str) -> Option<&'static ResourceEntry> {
    EDITABLE_RESOURCES.iter().find(|entry| entry.name == name)
}
