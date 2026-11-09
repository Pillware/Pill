use std::borrow::Cow;
use std::collections::HashMap;
#[cfg(feature = "build")]
use std::path::{Path, PathBuf};

#[cfg(feature = "build")]
use anyhow::{Context, Result};

use crate::pak_codec::{self, Codec};
#[cfg(feature = "build")]
use crate::Stats;

const MAGIC: &[u8; 4] = b"PPAK";
const VERSION: u32 = 4;

/// PPAK v4: flat archive embedded in the release binary; each entry stored with a
/// `pak_codec::Codec` (WebP textures, quantized meshes, brotli).
///
/// Layout:
///   [4]  magic "PPAK"
///   [4]  version u32le = 4
///   [4]  entry_count u32le
///   per entry:
///     [4]         name_len u32le
///     [name_len]  name UTF-8 (path relative to res/, forward-slash separated)
///     [8]         offset u64le       (byte offset from start of file)
///     [8]         stored_size u64le  (bytes in the data section)
///     [8]         raw_size u64le     (size `get` returns)
///     [1]         codec u8
///   data section: concatenated stored entry bytes at the declared offsets
pub struct Ppak<'a> {
    data: &'a [u8],
    entries: Vec<PpakEntry>,
    index: HashMap<String, usize>,
}

pub struct PpakEntry {
    pub name: String,
    pub stored_size: u64,
    pub raw_size: u64,
    pub codec: Codec,
    offset: u64,
}

impl<'a> Ppak<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        let u32_at = |pos: usize| -> Option<u32> {
            Some(u32::from_le_bytes(
                bytes.get(pos..pos + 4)?.try_into().ok()?,
            ))
        };
        let u64_at = |pos: usize| -> Option<u64> {
            Some(u64::from_le_bytes(
                bytes.get(pos..pos + 8)?.try_into().ok()?,
            ))
        };
        if bytes.get(0..4)? != MAGIC || u32_at(4)? != VERSION {
            return None;
        }
        let count = u32_at(8)? as usize;

        let mut pos = 12;
        let mut entries = Vec::with_capacity(count);
        let mut index = HashMap::with_capacity(count);
        for i in 0..count {
            let name_len = u32_at(pos)? as usize;
            pos += 4;
            let name = std::str::from_utf8(bytes.get(pos..pos + name_len)?)
                .ok()?
                .to_string();
            pos += name_len;
            let offset = u64_at(pos)?;
            let stored_size = u64_at(pos + 8)?;
            let raw_size = u64_at(pos + 16)?;
            let codec = Codec::from_u8(*bytes.get(pos + 24)?)?;
            pos += 25;
            index.insert(name.clone(), i);
            entries.push(PpakEntry {
                name,
                stored_size,
                raw_size,
                codec,
                offset,
            });
        }

        Some(Self {
            data: bytes,
            entries,
            index,
        })
    }

    /// Entry bytes: borrowed when stored raw, decoded otherwise (see `pak_codec`).
    pub fn get(&self, name: &str) -> Option<Cow<'a, [u8]>> {
        let entry = &self.entries[*self.index.get(name)?];
        let start = entry.offset as usize;
        let stored = self.data.get(start..start + entry.stored_size as usize)?;
        if entry.codec == Codec::Raw {
            return Some(Cow::Borrowed(stored));
        }
        pak_codec::decode(entry.codec, stored, entry.raw_size as usize).map(Cow::Owned)
    }

    pub fn entries(&self) -> &[PpakEntry] {
        &self.entries
    }
}

/// Shaders reach the binary via `include_str!`, so they are not packed.
#[cfg(feature = "build")]
fn is_shader_source(output: &Path) -> bool {
    matches!(
        output.extension().and_then(|ext| ext.to_str()),
        Some("wgsl") | Some("hlsl")
    )
}

/// The `(name, path)` entries a PPAK built from `stats` contains: every pipeline output (rebuilt
/// or up to date), minus shader sources, which reach binaries via `include_str!`.
#[cfg(feature = "build")]
pub fn pack_list(res_dir: &Path, stats: &Stats) -> Result<Vec<(String, PathBuf)>> {
    let mut seen = std::collections::HashSet::new();
    let mut to_pack: Vec<(String, PathBuf)> = Vec::new();
    // `edges` holds every produced file (primaries too); side outputs (GLB texture sidecars,
    // specular IBL) appear nowhere else.
    let side_outputs = stats.edges.iter().map(|(_, output, _)| output);
    for output in stats
        .rebuilt
        .iter()
        .chain(&stats.skipped)
        .chain(side_outputs)
    {
        let is_cook_cache = output
            .file_name()
            .is_some_and(|name| name == crate::COOK_TIMES_FILE);
        if !seen.insert(output.clone())
            || !output.exists()
            || is_shader_source(output)
            || is_cook_cache
        {
            continue;
        }
        let name = output
            .strip_prefix(res_dir)
            .with_context(|| format!("{output:?} is not under res_dir {res_dir:?}"))?
            .to_string_lossy()
            .replace('\\', "/");
        to_pack.push((name, output.clone()));
    }
    to_pack.sort();
    Ok(to_pack)
}

/// Write a PPAK v4 archive of `pack_list(res_dir, stats)` to `pak_path`, each entry encoded with
/// the codec `pak_codec::encode` picks for it. Returns the number of entries packed.
#[cfg(feature = "encode")]
pub fn pack_sources(res_dir: &Path, stats: &Stats, pak_path: &Path) -> Result<usize> {
    let to_pack = pack_list(res_dir, stats)?;

    // (name, stored bytes, raw size, codec)
    let mut entries: Vec<(String, Vec<u8>, u64, Codec)> = Vec::with_capacity(to_pack.len());
    for (name, path) in to_pack {
        let raw = std::fs::read(&path).with_context(|| format!("read {path:?}"))?;
        let (codec, stored) = pak_codec::encode(&name, &raw)?;
        entries.push((name, stored, raw.len() as u64, codec));
    }

    // Header: 4 magic + 4 version + 4 count; per entry: 4 + name + 8 offset + 8 stored + 8 raw + 1.
    let data_start: u64 = 12
        + entries
            .iter()
            .map(|(name, _, _, _)| 4 + name.len() as u64 + 25)
            .sum::<u64>();

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut offset = data_start;
    for (name, stored, raw_size, codec) in &entries {
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(stored.len() as u64).to_le_bytes());
        out.extend_from_slice(&raw_size.to_le_bytes());
        out.push(*codec as u8);
        offset += stored.len() as u64;
    }
    for (_, stored, _, _) in &entries {
        out.extend_from_slice(stored);
    }

    if let Some(parent) = pak_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {parent:?}"))?;
    }
    crate::util::write_atomic(pak_path, &out)?;
    Ok(entries.len())
}

/// For a game's build script: writes `OUT_DIR/assets.pak` from the cooked `res_dir` for release
/// and wasm builds, and an empty placeholder otherwise (debug loads from disk), so
/// `include_bytes!(concat!(env!("OUT_DIR"), "/assets.pak"))` always compiles.
#[cfg(feature = "encode")]
pub fn write_pack_for_build(res_dir: &Path, stats: &Stats) -> Result<PathBuf> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").context("OUT_DIR not set")?);
    let pak_path = out_dir.join("assets.pak");
    let release = std::env::var("PROFILE").is_ok_and(|profile| profile == "release");
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|arch| arch == "wasm32");
    if release || wasm {
        pack_sources(res_dir, stats, &pak_path)?;
    } else {
        std::fs::write(&pak_path, b"").with_context(|| format!("write {pak_path:?}"))?;
    }
    Ok(pak_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "build")]
    fn temp_res(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("pill_ppak_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("res")).unwrap();
        root
    }

    #[cfg(feature = "encode")]
    #[test]
    fn v4_round_trip_brotli_and_raw() {
        let root = temp_res("rt");
        let res = root.join("res");
        let compressible = vec![7u8; 64 * 1024];
        let incompressible: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        std::fs::write(res.join("a.dat"), &compressible).unwrap();
        std::fs::write(res.join("b.dat"), &incompressible).unwrap();
        let stats = Stats {
            rebuilt: vec![res.join("a.dat")],
            skipped: vec![res.join("b.dat")],
            ..Stats::default()
        };
        let pak = root.join("assets.pak");
        assert_eq!(pack_sources(&res, &stats, &pak).unwrap(), 2);
        let bytes = std::fs::read(&pak).unwrap();
        let ppak = Ppak::parse(&bytes).unwrap();
        let a = ppak.entries().iter().find(|e| e.name == "a.dat").unwrap();
        assert_eq!(a.codec, Codec::Brotli);
        assert!(a.stored_size < a.raw_size);
        assert_eq!(ppak.get("a.dat").unwrap().as_ref(), compressible.as_slice());
        assert_eq!(
            ppak.get("b.dat").unwrap().as_ref(),
            incompressible.as_slice()
        );
        assert!(ppak.get("missing.dat").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(feature = "build")]
    #[test]
    fn pack_list_skips_shaders_and_duplicates() {
        let root = temp_res("list");
        let res = root.join("res");
        for name in ["a.cooked_tex", "s.wgsl"] {
            std::fs::write(res.join(name), b"x").unwrap();
        }
        let stats = Stats {
            rebuilt: vec![res.join("a.cooked_tex"), res.join("s.wgsl")],
            skipped: vec![res.join("a.cooked_tex"), res.join("gone.cooked_mesh")],
            ..Stats::default()
        };
        let list = pack_list(&res, &stats).unwrap();
        assert_eq!(
            list,
            vec![("a.cooked_tex".to_string(), res.join("a.cooked_tex"))]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn other_versions_are_rejected() {
        let mut bytes = b"PPAK".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(Ppak::parse(&bytes).is_none());
        bytes[4..8].copy_from_slice(&VERSION.to_le_bytes());
        assert!(Ppak::parse(&bytes).is_some());
    }
}
