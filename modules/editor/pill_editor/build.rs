//! Stage the toolchain's standard-library dylib and embed the editor's icon.
//!
//! # Responsibilities
//!
//! - Copy the toolchain's `std` dylib beside the executable.
//! - Include the shared staging code from `modules/frontends/build_support`;
//!   this script only wires it up.
//! - Write the Windows icon resource from `res/pill_editor_black.ico` and link it
//!   into the executable.
//!
//! # Design
//!
//! The engine workspace links with `-C prefer-dynamic`, so this executable
//! imports `std-<hash>.dll` when the process starts, and Windows does not
//! search the toolchain directory that holds it. Without this script the
//! editor starts only when another executable's build has already copied the
//! dylib into the shared target directory.
//!
//! The icon resource is written here rather than by an icon crate: none is in
//! the workspace's lockfile or the registry cache, and a build dependency that
//! cannot be fetched offline is worse than the lines below that write the same
//! bytes `rc.exe` writes. Windows reads an executable's own icon from this
//! resource for the taskbar button and Explorer; the window's icon is set
//! separately, when the window is created.

// Standard library
use std::fs;
use std::path::PathBuf;

// The staging code shared by every executable's build script.
#[path = "../../frontends/build_support/stage_std_dylib.rs"]
mod stage_std_dylib;

// The icon resource format `rc.exe` writes: a 32-byte header per resource,
// then the resource's data, each padded to a four-byte boundary.
const RESOURCE_HEADER_SIZE: u32 = 0x20;
const RESOURCE_MEMORY_FLAGS: u16 = 0x1010;
const RESOURCE_LANGUAGE_EN_US: u16 = 0x0409;
const RESOURCE_TYPE_ICON: u16 = 3;
const RESOURCE_TYPE_GROUP_ICON: u16 = 14;
// The name the icon group is filed under. The shell and `ExtractIconEx` take an
// executable's first icon group whatever it is called.
const ICON_GROUP_NAME: u16 = 1;
// The white mark on the black disc; `res` also holds the inverted `_white` set.
const ICON_ASSET: &str = "res/pill_editor_black.ico";

fn main() {
    // Copy `std-*.dll` next to the built executable (no-op off Windows).
    stage_std_dylib::stage_std_dylib();
    embed_icon();
}

/// One image inside the `.ico` asset, as its directory describes it.
struct IconEntry {
    /// The directory's width byte, zero for 256 pixels.
    width: u8,
    /// The directory's height byte, zero for 256 pixels.
    height: u8,
    /// The image itself: a bitmap with its mask, or a whole PNG.
    data: Vec<u8>,
}

/// Links the editor's icon into the executable, on Windows only.
fn embed_icon() {
    println!("cargo:rerun-if-changed={ICON_ASSET}");
    if !cfg!(target_os = "windows") {
        return;
    }
    let manifest_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let icon = fs::read(manifest_directory.join(ICON_ASSET))
        .unwrap_or_else(|_| panic!("the editor's icon asset is missing: {ICON_ASSET}"));
    let resource = resource_file(&parse_ico(&icon));
    let mut resource_path = PathBuf::from(
        std::env::var("OUT_DIR").expect("a build script always runs with OUT_DIR set"),
    );
    resource_path.push("pill_editor_icon.res");
    fs::write(&resource_path, resource).expect("the icon resource could not be written");
    println!(
        "cargo:rustc-link-arg-bin=pill_editor={}",
        resource_path.display()
    );
}

/// The images an `.ico` file holds, in directory order.
fn parse_ico(data: &[u8]) -> Vec<IconEntry> {
    assert!(data.len() >= 6, "the icon asset is too short to be an ICO");
    let count = usize::from(u16::from_le_bytes([data[4], data[5]]));
    assert!(count > 0, "the icon asset holds no images");
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let directory = &data[6 + index * 16..6 + index * 16 + 16];
        let size =
            u32::from_le_bytes([directory[8], directory[9], directory[10], directory[11]]) as usize;
        let start = u32::from_le_bytes([directory[12], directory[13], directory[14], directory[15]])
            as usize;
        entries.push(IconEntry {
            width: directory[0],
            height: directory[1],
            data: data[start..start + size].to_vec(),
        });
    }
    entries
}

/// The whole `.res` file: every image as its own resource, then the group that
/// names them in order.
fn resource_file(entries: &[IconEntry]) -> Vec<u8> {
    let mut resource = Vec::new();
    // `rc.exe` opens the file with an empty entry, and the linker expects one:
    // a header with no data and no flags, its type and name ordinals zero.
    resource.extend_from_slice(&0_u32.to_le_bytes());
    resource.extend_from_slice(&RESOURCE_HEADER_SIZE.to_le_bytes());
    resource.extend_from_slice(&0xFFFF_u16.to_le_bytes());
    resource.extend_from_slice(&0_u16.to_le_bytes());
    resource.extend_from_slice(&0xFFFF_u16.to_le_bytes());
    resource.extend_from_slice(&0_u16.to_le_bytes());
    resource.extend_from_slice(&[0; 16]);
    for (index, entry) in entries.iter().enumerate() {
        let name = (index + 1) as u16;
        write_resource_header(
            &mut resource,
            entry.data.len() as u32,
            RESOURCE_TYPE_ICON,
            name,
        );
        resource.extend_from_slice(&entry.data);
        pad_to_four_bytes(&mut resource);
    }
    let group = group_icon(entries);
    write_resource_header(
        &mut resource,
        group.len() as u32,
        RESOURCE_TYPE_GROUP_ICON,
        ICON_GROUP_NAME,
    );
    resource.extend_from_slice(&group);
    pad_to_four_bytes(&mut resource);
    resource
}

/// The group icon's payload: which images make up the one icon and in what
/// order, in the shape an `.ico` file's directory holds them.
fn group_icon(entries: &[IconEntry]) -> Vec<u8> {
    let mut group = Vec::new();
    group.extend_from_slice(&0_u16.to_le_bytes());
    // A group of icons; a cursor group would say 2.
    group.extend_from_slice(&1_u16.to_le_bytes());
    group.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (index, entry) in entries.iter().enumerate() {
        group.push(entry.width);
        group.push(entry.height);
        // A true-colour icon has no palette, and nothing else is reserved.
        group.push(0);
        group.push(0);
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.extend_from_slice(&32_u16.to_le_bytes());
        group.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        group.extend_from_slice(&((index + 1) as u16).to_le_bytes());
    }
    group
}

/// One resource header: the data's size, then what it is filed under.
fn write_resource_header(resource: &mut Vec<u8>, data_size: u32, kind: u16, name: u16) {
    resource.extend_from_slice(&data_size.to_le_bytes());
    resource.extend_from_slice(&RESOURCE_HEADER_SIZE.to_le_bytes());
    // An ordinal type and an ordinal name: the 0xFFFF marker, then the number.
    resource.extend_from_slice(&0xFFFF_u16.to_le_bytes());
    resource.extend_from_slice(&kind.to_le_bytes());
    resource.extend_from_slice(&0xFFFF_u16.to_le_bytes());
    resource.extend_from_slice(&name.to_le_bytes());
    resource.extend_from_slice(&0_u32.to_le_bytes());
    resource.extend_from_slice(&RESOURCE_MEMORY_FLAGS.to_le_bytes());
    resource.extend_from_slice(&RESOURCE_LANGUAGE_EN_US.to_le_bytes());
    resource.extend_from_slice(&0_u32.to_le_bytes());
    resource.extend_from_slice(&0_u32.to_le_bytes());
}

/// Pads to the four-byte boundary every resource and every header starts on.
fn pad_to_four_bytes(resource: &mut Vec<u8>) {
    while !resource.len().is_multiple_of(4) {
        resource.push(0);
    }
}
