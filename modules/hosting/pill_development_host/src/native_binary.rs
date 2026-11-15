//! Reading a native library's dependency, import and export names from its
//! bytes, without mapping it into the process.
//!
//! # Responsibilities
//!
//! - Report the shared libraries an image names as dependencies.
//! - Report the symbols it needs from other images and the symbols it exports.
//! - Read ELF itself, and hand a PE image to [`crate::analytics::inspect_pe`]
//!   instead of reading the same tables twice.
//!
//! # Design
//!
//! This is the evidence a load failure is explained from. A module whose
//! engine dylib was built from a different dependency graph imports symbol
//! names the mapped copy does not export. No loader says that: Windows reports
//! `LoadLibrary` error 127, and a Unix loader reports
//! `undefined symbol: <one mangled name>`, naming one symbol and no cause. The
//! import and export tables are therefore read from the file, which works
//! whether or not the image is mapped and on every host platform. Comparing
//! the two tables is what turns the loader's message into a sentence that
//! names both builds.
//!
//! Everything here is best effort, and it fails toward silence. An image that
//! is neither PE nor ELF, a class this reader does not know, or a header that
//! points outside the file answers `None`, and the caller then keeps the
//! loader's own error. A symbol table that cannot be read to its end answers
//! `None` as well rather than a shortened export list: a copy that appears to
//! lack a symbol it does have would be reported as the cause of a failure it
//! did not cause.

// Standard library
use std::path::Path;

// =============================================================================
// Constants
// =============================================================================

/// First four bytes of every ELF image: `0x7F` followed by `E`, `L`, `F`.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// `EI_CLASS`, at byte 4 of the identification block: 32-bit ELF.
const ELF_CLASS_32: u8 = 1;
/// `EI_CLASS`: 64-bit ELF.
const ELF_CLASS_64: u8 = 2;

/// `EI_DATA`, at byte 5: little-endian fields.
const ELF_DATA_LITTLE_ENDIAN: u8 = 1;
/// `EI_DATA`: big-endian fields.
const ELF_DATA_BIG_ENDIAN: u8 = 2;

/// `SHT_DYNAMIC`: the `DT_*` entries, including the dependency names.
const SECTION_TYPE_DYNAMIC: u32 = 6;
/// `SHT_DYNSYM`: the dynamic symbol table.
const SECTION_TYPE_DYNAMIC_SYMBOLS: u32 = 11;

/// Bytes of the fixed ELF header this reader takes: the 64-bit header, which
/// is the longer of the two.
///
/// Only the dependency-only reader ([`needed_libraries`]) uses it, and that
/// reader exists for a host that resolves its engine dylibs through `DT_NEEDED`
/// - a non-Windows one.
#[cfg(not(windows))]
const ELF_HEADER_LENGTH: usize = 64;

/// The largest table [`needed_libraries`] will read in one piece.
///
/// The tables it reads are kilobytes in every real image; the bound only keeps
/// a damaged header from asking for an allocation the size of the file.
#[cfg(not(windows))]
const MAX_TABLE_BYTES: usize = 8 * 1024 * 1024;

/// `DT_NEEDED`: a string-table offset holding one dependency's name.
const DYNAMIC_TAG_NEEDED: i64 = 1;
/// `DT_NULL`: the end of the dynamic table.
const DYNAMIC_TAG_NULL: i64 = 0;

/// `SHN_UNDEF`: the symbol needs a definition from another image.
const SECTION_INDEX_UNDEFINED: u16 = 0;

/// `STB_GLOBAL`: the binding another image resolves against.
const SYMBOL_BINDING_GLOBAL: u8 = 1;
/// `STB_WEAK`: the binding a strong definition may replace.
const SYMBOL_BINDING_WEAK: u8 = 2;

/// `STV_DEFAULT`: the visibility the loader resolves symbols through.
const SYMBOL_VISIBILITY_DEFAULT: u8 = 0;
/// `STV_PROTECTED`: defined here, and still exported.
const SYMBOL_VISIBILITY_PROTECTED: u8 = 3;

/// Size of one 64-bit symbol table entry.
const SYMBOL_ENTRY_SIZE_64: usize = 24;
/// Size of one 32-bit symbol table entry.
const SYMBOL_ENTRY_SIZE_32: usize = 16;
/// Size of one 64-bit dynamic table entry.
const DYNAMIC_ENTRY_SIZE_64: usize = 16;
/// Size of one 32-bit dynamic table entry.
const DYNAMIC_ENTRY_SIZE_32: usize = 8;
/// Size of one 64-bit section header.
const SECTION_ENTRY_SIZE_64: usize = 64;
/// Size of one 32-bit section header.
const SECTION_ENTRY_SIZE_32: usize = 40;

// =============================================================================
// Types
// =============================================================================

/// One dependency library and the symbols the image attributes to it.
pub(crate) struct LibraryImport {
    /// The file name the loader looks the dependency up by.
    pub(crate) library: String,
    /// Named imports attributed to this library. A format that resolves
    /// symbols by search order instead of recording their owner leaves this
    /// empty.
    pub(crate) symbols: Vec<String>,
}

/// What a native image needs from, and offers to, the other images in the
/// process.
pub(crate) struct BinaryInspection {
    /// Every dependency library, by the name the loader looks up.
    pub(crate) libraries: Vec<String>,
    /// The dependencies whose symbols the image attributes, in table order.
    pub(crate) imports: Vec<LibraryImport>,
    /// Every symbol the image needs from another image.
    pub(crate) required_symbols: Vec<String>,
    /// Every symbol the image exports.
    pub(crate) exported_symbols: Vec<String>,
}

/// The parts of an ELF image this reader keeps.
struct ElfImage {
    /// `DT_NEEDED` names, in table order.
    libraries: Vec<String>,
    /// Undefined dynamic symbols, which another image has to provide.
    required_symbols: Vec<String>,
    /// Defined dynamic symbols another image can resolve against.
    exported_symbols: Vec<String>,
}

/// One entry of an ELF section table, reduced to the fields this reader uses.
#[derive(Clone, Copy)]
struct SectionHeader {
    /// `sh_type`, which is how the dynamic and symbol tables are found.
    section_type: u32,
    /// `sh_offset`, the section's position in the file.
    offset: usize,
    /// `sh_size`, the section's length in bytes.
    size: usize,
    /// `sh_link`, the index of the section this one reads strings from.
    link: usize,
    /// `sh_entsize`, the entry size the section declares, `0` when it has none.
    entry_size: usize,
}

/// Bounds-checked field reads over an image in its own byte order.
///
/// Every read answers `None` instead of panicking when the image is shorter
/// than the field it would read, so a damaged table ends the parse rather than
/// the host.
struct ByteReader<'a> {
    /// The whole file, as read from disk.
    bytes: &'a [u8],
    /// Whether fields are little-endian (`EI_DATA` says so).
    little_endian: bool,
}

impl ByteReader<'_> {
    /// One byte at `offset`.
    fn u8(&self, offset: usize) -> Option<u8> {
        self.bytes.get(offset).copied()
    }

    /// A two-byte field at `offset`.
    fn u16(&self, offset: usize) -> Option<u16> {
        let field: [u8; 2] = self
            .bytes
            .get(offset..offset.checked_add(2)?)?
            .try_into()
            .ok()?;
        Some(if self.little_endian {
            u16::from_le_bytes(field)
        } else {
            u16::from_be_bytes(field)
        })
    }

    /// A four-byte unsigned field at `offset`.
    fn u32(&self, offset: usize) -> Option<u32> {
        let field: [u8; 4] = self
            .bytes
            .get(offset..offset.checked_add(4)?)?
            .try_into()
            .ok()?;
        Some(if self.little_endian {
            u32::from_le_bytes(field)
        } else {
            u32::from_be_bytes(field)
        })
    }

    /// A four-byte signed field at `offset`, which is how `d_tag` is spelled.
    fn i32(&self, offset: usize) -> Option<i32> {
        let field: [u8; 4] = self
            .bytes
            .get(offset..offset.checked_add(4)?)?
            .try_into()
            .ok()?;
        Some(if self.little_endian {
            i32::from_le_bytes(field)
        } else {
            i32::from_be_bytes(field)
        })
    }

    /// An eight-byte unsigned field at `offset`.
    fn u64(&self, offset: usize) -> Option<u64> {
        let field: [u8; 8] = self
            .bytes
            .get(offset..offset.checked_add(8)?)?
            .try_into()
            .ok()?;
        Some(if self.little_endian {
            u64::from_le_bytes(field)
        } else {
            u64::from_be_bytes(field)
        })
    }

    /// An eight-byte signed field at `offset`.
    fn i64(&self, offset: usize) -> Option<i64> {
        let field: [u8; 8] = self
            .bytes
            .get(offset..offset.checked_add(8)?)?
            .try_into()
            .ok()?;
        Some(if self.little_endian {
            i64::from_le_bytes(field)
        } else {
            i64::from_be_bytes(field)
        })
    }
}

// =============================================================================
// Inspection
// =============================================================================

/// Reads what `path` needs and exports, in whichever native format it is.
///
/// Returns `None` for a file this reader does not recognise, which includes
/// every non-image file and every image whose headers cannot be read.
pub(crate) fn inspect(path: &Path) -> Option<BinaryInspection> {
    // Step 1: A PE image is already read by the analytics collector, so reuse
    // that reader instead of duplicating its import and export parsing.
    if let Some(pe_inspection) = crate::analytics::inspect_pe(path) {
        let imports = pe_inspection
            .imports
            .iter()
            .map(|import| LibraryImport {
                library: import.dll.clone(),
                symbols: import.symbols.clone(),
            })
            .collect::<Vec<_>>();
        let required_symbols = imports
            .iter()
            .flat_map(|import| import.symbols.iter().cloned())
            .collect();
        return Some(BinaryInspection {
            libraries: pe_inspection.import_dlls,
            imports,
            required_symbols,
            exported_symbols: pe_inspection.exports,
        });
    }

    // Step 2: Everything else this host loads is ELF.
    let image_bytes = std::fs::read(path).ok()?;
    let image = parse_elf(&image_bytes)?;
    Some(BinaryInspection {
        libraries: image.libraries,
        // ELF resolves an undefined symbol by searching the dependency list in
        // order, so an entry does not record which library defines it.
        imports: Vec::new(),
        required_symbols: image.required_symbols,
        exported_symbols: image.exported_symbols,
    })
}

/// Reads an ELF image's dependencies and dynamic symbols.
///
/// Returns `None` when the image is not ELF, when its class or byte order is
/// not one this reader knows, or when its section table lies outside the file.
fn parse_elf(image_bytes: &[u8]) -> Option<ElfImage> {
    // Step 1: Identification, class and byte order. The shortest header this
    // reader reads a field from is the 32-bit one, so a file below that size
    // cannot be an image.
    if image_bytes.len() < 0x34 {
        return None;
    }
    let (is_64_bit, little_endian) = elf_identity(image_bytes)?;
    let reader = ByteReader {
        bytes: image_bytes,
        little_endian,
    };

    // Step 2: Locate the section table. `e_shnum == 0` means the real count sits
    // in section 0's `sh_size`, which only an image with more than 65,279
    // sections needs; such an image is refused rather than read wrongly.
    let (section_table_offset, declared_entry_size, section_count) =
        section_table_location(&reader, is_64_bit)?;
    let minimum_entry_size = if is_64_bit {
        SECTION_ENTRY_SIZE_64
    } else {
        SECTION_ENTRY_SIZE_32
    };
    if section_count == 0 || declared_entry_size < minimum_entry_size {
        return None;
    }
    let section_table_end =
        section_table_offset.checked_add(declared_entry_size.checked_mul(section_count)?)?;
    if section_table_end > image_bytes.len() {
        return None;
    }

    // Step 3: One section header by index.
    let section_at = |index: usize| -> Option<SectionHeader> {
        section_header_at(
            &reader,
            section_table_offset,
            declared_entry_size,
            index,
            is_64_bit,
        )
    };

    // Step 4: Find the two sections that hold the answer. A shared library has
    // one of each; a repeated type is left to the first, because a second one
    // would mean an image this reader does not understand.
    let mut dynamic_section: Option<SectionHeader> = None;
    let mut symbol_section: Option<SectionHeader> = None;
    for index in 1..section_count {
        let Some(section) = section_at(index) else {
            break;
        };
        match section.section_type {
            SECTION_TYPE_DYNAMIC if dynamic_section.is_none() => dynamic_section = Some(section),
            SECTION_TYPE_DYNAMIC_SYMBOLS if symbol_section.is_none() => {
                symbol_section = Some(section);
            }
            _ => {}
        }
        if dynamic_section.is_some() && symbol_section.is_some() {
            break;
        }
    }

    // Step 5: Dependency names, one `DT_NEEDED` string-table offset each.
    let mut libraries = Vec::new();
    if let Some(dynamic_section) = dynamic_section {
        let string_table = section_at(dynamic_section.link);
        let entry_size = resolved_entry_size(
            dynamic_section.entry_size,
            if is_64_bit {
                DYNAMIC_ENTRY_SIZE_64
            } else {
                DYNAMIC_ENTRY_SIZE_32
            },
        );
        let entry_count = dynamic_section.size / entry_size;
        for index in 0..entry_count {
            let Some(entry_offset) = dynamic_section
                .offset
                .checked_add(index.checked_mul(entry_size)?)
            else {
                break;
            };
            let Some((tag, value)) = dynamic_entry(&reader, is_64_bit, entry_offset) else {
                break;
            };
            if tag == DYNAMIC_TAG_NULL {
                break;
            }
            if tag != DYNAMIC_TAG_NEEDED {
                continue;
            }
            let Some(string_table) = string_table else {
                break;
            };
            let Some(name_offset) = usize::try_from(value).ok() else {
                continue;
            };
            if let Some(name) = string_at(&reader, string_table, name_offset) {
                libraries.push(name);
            }
        }
    }

    // Step 6: Dynamic symbols. One table holds both answers: an entry whose
    // section index is `SHN_UNDEF` is an import, any other externally visible
    // entry is an export.
    let mut required_symbols = Vec::new();
    let mut exported_symbols = Vec::new();
    if let Some(symbol_section) = symbol_section {
        let string_table = section_at(symbol_section.link);
        let entry_size = resolved_entry_size(
            symbol_section.entry_size,
            if is_64_bit {
                SYMBOL_ENTRY_SIZE_64
            } else {
                SYMBOL_ENTRY_SIZE_32
            },
        );
        // A table whose length is not a whole number of entries cannot be read
        // to its end, so it is refused like a damaged one.
        if symbol_section.size % entry_size != 0 {
            return None;
        }
        let entry_count = symbol_section.size / entry_size;
        // Entry 0 is the reserved null symbol, which names nothing.
        for index in 1..entry_count {
            let Some(entry_offset) = symbol_section
                .offset
                .checked_add(index.checked_mul(entry_size)?)
            else {
                break;
            };
            let (name_offset, information, other, section_index) = if is_64_bit {
                (
                    reader.u32(entry_offset),
                    reader.u8(entry_offset.checked_add(4)?),
                    reader.u8(entry_offset.checked_add(5)?),
                    reader.u16(entry_offset.checked_add(6)?),
                )
            } else {
                (
                    reader.u32(entry_offset),
                    reader.u8(entry_offset.checked_add(12)?),
                    reader.u8(entry_offset.checked_add(13)?),
                    reader.u16(entry_offset.checked_add(14)?),
                )
            };
            // A shortened export list would make this image look like it lacks
            // a symbol it does have, and the diagnosis would then blame a copy
            // that is not at fault. Every unreadable entry therefore refuses
            // the whole image instead of being skipped.
            let (Some(name_offset), Some(information), Some(other), Some(section_index)) =
                (name_offset, information, other, section_index)
            else {
                return None;
            };
            if name_offset == 0 {
                continue;
            }
            let string_table = string_table?;
            let Ok(name_offset) = usize::try_from(name_offset) else {
                return None;
            };
            let name = string_at(&reader, string_table, name_offset)?;
            if section_index == SECTION_INDEX_UNDEFINED {
                required_symbols.push(name);
                continue;
            }
            let binding = information >> 4;
            let visibility = other & 0x3;
            let is_exported = matches!(binding, SYMBOL_BINDING_GLOBAL | SYMBOL_BINDING_WEAK)
                && matches!(
                    visibility,
                    SYMBOL_VISIBILITY_DEFAULT | SYMBOL_VISIBILITY_PROTECTED
                );
            if is_exported {
                exported_symbols.push(name);
            }
        }
    }

    Some(ElfImage {
        libraries,
        required_symbols,
        exported_symbols,
    })
}

/// The libraries `path` names as dependencies, without reading its symbols.
///
/// [`inspect`] reads the whole file and every dynamic symbol, which a diagnosis
/// can afford and a load path cannot: the editor's own binary is hundreds of
/// megabytes, and so is its shared core. Here only the fixed header, the section
/// table, the dynamic table and the few bytes each dependency name occupies are
/// read - kilobytes - which is everything a `DT_NEEDED` entry needs.
///
/// The PE side reuses the analytics reader, which reads the image; it is only
/// reached on a Windows host, where the caller that needs this fast path does
/// not run. The function itself is compiled where that caller is.
#[cfg(not(windows))]
pub(crate) fn needed_libraries(path: &Path) -> Option<Vec<String>> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    /// Longest dependency name read from a string table.
    ///
    /// No loader accepts a longer one - the format's own limit is 255 bytes -
    /// and a short read keeps this from depending on how large the surrounding
    /// string table is: a debug build's `.dynstr` can pass eight megabytes.
    const MAX_DEPENDENCY_NAME: usize = 1024;

    /// Reads `length` bytes at `offset`.
    fn read_at(file: &mut std::fs::File, offset: u64, length: usize) -> Option<Vec<u8>> {
        let mut buffer = vec![0u8; length];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut buffer).ok()?;
        Some(buffer)
    }

    // Step 1: Identify the file from its first bytes.
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if magic[0..2] == *b"MZ" {
        return crate::analytics::inspect_pe(path).map(|inspection| inspection.import_dlls);
    }
    if magic != ELF_MAGIC {
        return None;
    }

    // Step 2: The fixed header, then the section table it points at.
    let header = read_at(&mut file, 0, ELF_HEADER_LENGTH)?;
    let (is_64_bit, little_endian) = elf_identity(&header)?;
    let header_reader = ByteReader {
        bytes: &header,
        little_endian,
    };
    let (section_table_offset, section_entry_size, section_count) =
        section_table_location(&header_reader, is_64_bit)?;
    let table_length = section_entry_size.checked_mul(section_count)?;
    if table_length > MAX_TABLE_BYTES {
        return None;
    }
    let section_table = read_at(&mut file, section_table_offset as u64, table_length)?;
    let table_reader = ByteReader {
        bytes: &section_table,
        little_endian,
    };
    let section_at =
        |index: usize| section_header_at(&table_reader, 0, section_entry_size, index, is_64_bit);

    // Step 3: The dynamic table, whose `DT_NEEDED` entries name the dependencies
    // from a string table this reader never loads whole.
    let mut dynamic_section: Option<SectionHeader> = None;
    for index in 1..section_count {
        let Some(section) = section_at(index) else {
            break;
        };
        if section.section_type == SECTION_TYPE_DYNAMIC {
            dynamic_section = Some(section);
            break;
        }
    }
    let dynamic_section = dynamic_section?;
    let string_section = section_at(dynamic_section.link)?;
    if dynamic_section.size > MAX_TABLE_BYTES {
        return None;
    }
    let dynamic_table = read_at(
        &mut file,
        dynamic_section.offset as u64,
        dynamic_section.size,
    )?;
    let dynamic_reader = ByteReader {
        bytes: &dynamic_table,
        little_endian,
    };
    let entry_size = resolved_entry_size(
        dynamic_section.entry_size,
        if is_64_bit {
            DYNAMIC_ENTRY_SIZE_64
        } else {
            DYNAMIC_ENTRY_SIZE_32
        },
    );
    if entry_size == 0 {
        return None;
    }
    let mut libraries = Vec::new();
    for index in 0..dynamic_section.size / entry_size {
        let Some(entry_offset) = index.checked_mul(entry_size) else {
            break;
        };
        let Some((tag, value)) = dynamic_entry(&dynamic_reader, is_64_bit, entry_offset) else {
            break;
        };
        if tag == DYNAMIC_TAG_NULL {
            break;
        }
        if tag != DYNAMIC_TAG_NEEDED {
            continue;
        }
        let Ok(name_offset) = usize::try_from(value) else {
            continue;
        };
        // One string, bounded by the table it lives in and by the name limit.
        let Some(name_start) = string_section.offset.checked_add(name_offset) else {
            continue;
        };
        let Some(remaining) = string_section.size.checked_sub(name_offset) else {
            continue;
        };
        let Some(bytes) = read_at(
            &mut file,
            name_start as u64,
            remaining.min(MAX_DEPENDENCY_NAME),
        ) else {
            continue;
        };
        let length = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        libraries.push(String::from_utf8_lossy(&bytes[..length]).into_owned());
    }
    Some(libraries)
}

/// `EI_CLASS` and `EI_DATA`: whether the image is 64-bit, and its byte order.
fn elf_identity(header: &[u8]) -> Option<(bool, bool)> {
    if header.len() < 6 || header[0..4] != ELF_MAGIC {
        return None;
    }
    let is_64_bit = match header[4] {
        ELF_CLASS_64 => true,
        ELF_CLASS_32 => false,
        _ => return None,
    };
    let little_endian = match header[5] {
        ELF_DATA_LITTLE_ENDIAN => true,
        ELF_DATA_BIG_ENDIAN => false,
        _ => return None,
    };
    Some((is_64_bit, little_endian))
}

/// `e_shoff`, `e_shentsize` and `e_shnum`, read from an image header.
fn section_table_location(
    reader: &ByteReader<'_>,
    is_64_bit: bool,
) -> Option<(usize, usize, usize)> {
    if is_64_bit {
        Some((
            usize::try_from(reader.u64(0x28)?).ok()?,
            usize::from(reader.u16(0x3a)?),
            usize::from(reader.u16(0x3c)?),
        ))
    } else {
        Some((
            usize::try_from(reader.u32(0x20)?).ok()?,
            usize::from(reader.u16(0x2e)?),
            usize::from(reader.u16(0x30)?),
        ))
    }
}

/// One section header at `index`, through `reader` whose byte 0 is the section
/// table's first entry.
fn section_header_at(
    reader: &ByteReader<'_>,
    table_offset: usize,
    entry_size: usize,
    index: usize,
    is_64_bit: bool,
) -> Option<SectionHeader> {
    let minimum_entry_size = if is_64_bit {
        SECTION_ENTRY_SIZE_64
    } else {
        SECTION_ENTRY_SIZE_32
    };
    let header_offset = table_offset.checked_add(index.checked_mul(entry_size)?)?;
    if header_offset.checked_add(minimum_entry_size)? > reader.bytes.len() {
        return None;
    }
    let (section_type, offset, size, link, entry_size) = if is_64_bit {
        (
            reader.u32(header_offset.checked_add(4)?)?,
            usize::try_from(reader.u64(header_offset.checked_add(0x18)?)?).ok()?,
            usize::try_from(reader.u64(header_offset.checked_add(0x20)?)?).ok()?,
            usize::try_from(reader.u32(header_offset.checked_add(0x28)?)?).ok()?,
            usize::try_from(reader.u64(header_offset.checked_add(0x38)?)?).ok()?,
        )
    } else {
        (
            reader.u32(header_offset.checked_add(4)?)?,
            usize::try_from(reader.u32(header_offset.checked_add(0x10)?)?).ok()?,
            usize::try_from(reader.u32(header_offset.checked_add(0x14)?)?).ok()?,
            usize::try_from(reader.u32(header_offset.checked_add(0x18)?)?).ok()?,
            usize::try_from(reader.u32(header_offset.checked_add(0x24)?)?).ok()?,
        )
    };
    Some(SectionHeader {
        section_type,
        offset,
        size,
        link,
        entry_size,
    })
}

/// One `(tag, value)` dynamic-table entry at `entry_offset`.
fn dynamic_entry(
    reader: &ByteReader<'_>,
    is_64_bit: bool,
    entry_offset: usize,
) -> Option<(i64, u64)> {
    if is_64_bit {
        Some((
            reader.i64(entry_offset)?,
            reader.u64(entry_offset.checked_add(8)?)?,
        ))
    } else {
        Some((
            reader.i32(entry_offset).map(i64::from)?,
            reader.u32(entry_offset.checked_add(4)?).map(u64::from)?,
        ))
    }
}

/// The entry size a section actually uses.
///
/// A section is allowed to declare `sh_entsize == 0`, which means "one entry";
/// for the tables read here the class's own entry size is the only sane
/// answer, and dividing by zero would otherwise end the parse.
fn resolved_entry_size(declared_entry_size: usize, class_entry_size: usize) -> usize {
    if declared_entry_size == 0 {
        class_entry_size
    } else {
        declared_entry_size
    }
}

/// The NUL-terminated string at `string_offset` inside `string_table`.
///
/// The search stops at the end of the table, so a table whose last string is
/// unterminated still yields that string instead of running into the section
/// that follows it.
fn string_at(
    reader: &ByteReader<'_>,
    string_table: SectionHeader,
    string_offset: usize,
) -> Option<String> {
    if string_offset >= string_table.size {
        return None;
    }
    let start = string_table.offset.checked_add(string_offset)?;
    let end = string_table
        .offset
        .checked_add(string_table.size)?
        .min(reader.bytes.len());
    let table_slice = reader.bytes.get(start..end)?;
    let length = table_slice
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(table_slice.len());
    Some(String::from_utf8_lossy(&table_slice[..length]).into_owned())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::{inspect, parse_elf};
    use crate::scratch::ScratchDirectory;

    /// Offsets inside the synthetic image's string table.
    const DEPENDENCY_NAME_OFFSET: u32 = 1;
    /// The exported symbol's name.
    const EXPORTED_NAME_OFFSET: u32 = 11;
    /// The imported symbol's name.
    const IMPORTED_NAME_OFFSET: u32 = 27;
    /// Where the string table starts.
    const STRING_TABLE_OFFSET: u32 = 0x40;
    /// Its length, terminator included.
    const STRING_TABLE_SIZE: u32 = 43;
    /// Where the dynamic symbol table starts.
    const SYMBOL_TABLE_OFFSET: u32 = 0x70;
    /// Where the dynamic table starts.
    const DYNAMIC_TABLE_OFFSET: u32 = 0xb8;
    /// Where the section table starts.
    const SECTION_TABLE_OFFSET: u32 = 0xd8;

    /// Writes a little-endian 16-bit field at `offset`.
    fn write_u16(image: &mut [u8], offset: usize, value: u16) {
        image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a little-endian 32-bit field at `offset`.
    fn write_u32(image: &mut [u8], offset: usize, value: u32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes a little-endian 64-bit field at `offset`.
    fn write_u64(image: &mut [u8], offset: usize, value: u64) {
        image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Writes one 64-bit section header at `index`, type and body.
    ///
    /// `sh_name` is given the offset of the first real string rather than zero:
    /// a reader that mistook the name field for the type would then see `1`
    /// instead of a section type and find nothing, which is what makes this
    /// fixture able to catch a wrong field offset.
    fn write_section_header(
        image: &mut [u8],
        index: usize,
        section_type: u32,
        offset: u32,
        size: u32,
        link: u32,
        entry_size: u64,
    ) {
        let header = SECTION_TABLE_OFFSET as usize + index * 64;
        write_u32(image, header, DEPENDENCY_NAME_OFFSET);
        write_u32(image, header + 4, section_type);
        write_u64(image, header + 0x18, u64::from(offset));
        write_u64(image, header + 0x20, u64::from(size));
        write_u32(image, header + 0x28, link);
        write_u64(image, header + 0x38, entry_size);
    }

    /// A minimal 64-bit little-endian shared object: one dependency
    /// (`libc.so.6`), one exported symbol and one imported symbol.
    ///
    /// Written by hand so the reader is tested against bytes rather than
    /// against whatever the toolchain on this machine happens to produce.
    fn synthetic_elf_image() -> Vec<u8> {
        let mut image = vec![0u8; SECTION_TABLE_OFFSET as usize + 4 * 64];
        // Identification: magic, 64-bit, little-endian, EV_CURRENT.
        image[0..4].copy_from_slice(b"\x7fELF");
        image[4] = 2;
        image[5] = 1;
        image[6] = 1;
        // Header: ET_DYN, x86-64, EV_CURRENT, section table location and size.
        write_u16(&mut image, 0x10, 3);
        write_u16(&mut image, 0x12, 62);
        write_u32(&mut image, 0x14, 1);
        write_u64(&mut image, 0x28, u64::from(SECTION_TABLE_OFFSET));
        write_u16(&mut image, 0x34, 64);
        write_u16(&mut image, 0x3a, 64);
        write_u16(&mut image, 0x3c, 4);
        // String table: "", "libc.so.6", "exported_symbol", "imported_symbol".
        let strings: &[u8] = b"\0libc.so.6\0exported_symbol\0imported_symbol\0";
        let string_start = STRING_TABLE_OFFSET as usize;
        image[string_start..string_start + strings.len()].copy_from_slice(strings);
        // Symbol 1: exported (global function, defined in section 1).
        // Symbol 2: imported (global function, `SHN_UNDEF`).
        let symbol_start = SYMBOL_TABLE_OFFSET as usize;
        write_u32(&mut image, symbol_start + 24, EXPORTED_NAME_OFFSET);
        image[symbol_start + 28] = 0x12;
        write_u16(&mut image, symbol_start + 30, 1);
        write_u32(&mut image, symbol_start + 48, IMPORTED_NAME_OFFSET);
        image[symbol_start + 52] = 0x12;
        write_u16(&mut image, symbol_start + 54, 0);
        // Dynamic entries: DT_NEEDED -> "libc.so.6", then DT_NULL.
        let dynamic_start = DYNAMIC_TABLE_OFFSET as usize;
        write_u64(&mut image, dynamic_start, 1);
        write_u64(
            &mut image,
            dynamic_start + 8,
            u64::from(DEPENDENCY_NAME_OFFSET),
        );
        // Section 1: the string table. Section 2: the symbol table, which
        // reads strings from section 1. Section 3: the dynamic table, likewise.
        write_section_header(
            &mut image,
            1,
            3,
            STRING_TABLE_OFFSET,
            STRING_TABLE_SIZE,
            0,
            0,
        );
        write_section_header(&mut image, 2, 11, SYMBOL_TABLE_OFFSET, 3 * 24, 1, 24);
        write_section_header(&mut image, 3, 6, DYNAMIC_TABLE_OFFSET, 2 * 16, 1, 16);
        image
    }

    /// The reader reports the dependency, the import and the export the image
    /// declares, and attributes no import to a library on its own.
    #[test]
    fn an_elf_image_reports_its_dependencies_imports_and_exports() {
        let image = synthetic_elf_image();
        let parsed = parse_elf(&image).expect("the synthetic image parses");

        assert_eq!(parsed.libraries, vec!["libc.so.6"]);
        assert_eq!(parsed.required_symbols, vec!["imported_symbol"]);
        assert_eq!(parsed.exported_symbols, vec!["exported_symbol"]);
    }

    /// A file that is not a native image is refused instead of half-read.
    #[test]
    fn a_file_that_is_not_a_native_image_is_refused() {
        let directory = ScratchDirectory::new("native_binary_text");
        let path = directory.write("notes.txt", b"not an image");

        assert!(parse_elf(&std::fs::read(&path).expect("the file was written")).is_none());
        assert!(inspect(&path).is_none());
    }

    /// A truncated image ends the parse instead of panicking on a short read.
    #[test]
    fn a_truncated_image_is_refused() {
        let image = synthetic_elf_image();
        let truncated = &image[..0x30];

        assert!(parse_elf(truncated).is_none());
    }

    /// The file entry point reads the synthetic image back through the same
    /// path a load failure uses.
    #[test]
    fn inspection_reads_an_image_from_disk() {
        let directory = ScratchDirectory::new("native_binary_elf");
        let path = directory.write("libsample.so", synthetic_elf_image());

        let inspection = inspect(&path).expect("the synthetic image parses");

        assert_eq!(inspection.libraries, vec!["libc.so.6"]);
        assert_eq!(inspection.required_symbols, vec!["imported_symbol"]);
        assert_eq!(inspection.exported_symbols, vec!["exported_symbol"]);
        assert!(inspection.imports.is_empty());
    }

    /// The dependency-only reader answers from the same image.
    ///
    /// It reads the header, the section table and the dynamic table, never the
    /// symbol table: a load path runs this on files hundreds of megabytes long.
    /// Only a non-Windows host has that load path, so the reader and its test
    /// exist there.
    #[cfg(not(windows))]
    #[test]
    fn dependencies_are_read_without_the_symbol_table() {
        let directory = ScratchDirectory::new("native_binary_needed");
        let path = directory.write("libsample.so", synthetic_elf_image());

        assert_eq!(
            super::needed_libraries(&path),
            Some(vec!["libc.so.6".to_string()])
        );
    }
}
