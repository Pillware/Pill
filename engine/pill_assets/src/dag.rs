//! Self-contained SVG report of the asset-processing DAG.
//!
//! The report reads left to right:
//!   assets (cooking pipeline, typed) → [assets.pak] → consuming module (source file)
//!   → target (native release binary · web release wasm · editor/tools).
//! A module links to every target its `#[cfg]` chain compiles it into (see
//! `usage::Scope`); an asset reaches a target only through a module that uses it,
//! or through the asset pack the module embeds.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;

use crate::usage::{normalize_path, Scope, Usage, UsageScan, GENERATED_DIR};

// Layout constants (pixels).
const COLUMN_WIDTH: f64 = 230.0;
const COLUMN_GAP: f64 = 130.0;
const ROW_HEIGHT: f64 = 46.0;
const ROW_GAP: f64 = 22.0;
const MARGIN: f64 = 24.0;
const LINE_HEIGHT: f64 = 18.0;
const CHAR_WIDTH: f64 = 7.2; // approx width of one monospace char at 12-13px
const SUBTITLE_CHAR_WIDTH: f64 = 6.0; // … at 10px (node subtitles)
const TITLE: &str = "Asset DAG";

/// One contributing crate + pipeline root (e.g. engine, game). Nodes whose path
/// lives under `root` are tagged with `label` and colored accordingly.
pub struct DagSection {
    /// The crate's `res/` directory (pipeline root).
    pub root: PathBuf,
    pub label: String,
    pub edges: Vec<(PathBuf, PathBuf, &'static str)>,
    /// Per-output cook duration, keyed by absolute output path (from the root's
    /// `.pill_assets_cache`). See `read_cook_times`.
    pub cook_times: HashMap<PathBuf, Duration>,
    /// Which source modules reference which assets, and the builds each module is
    /// compiled into. See `scan_usage`.
    pub usage: UsageScan,
    /// Assets the crate's release builds carry inside its `assets.pak` (`pack_list`);
    /// empty when the crate builds no pack.
    pub packed: Vec<PathBuf>,
}

/// Build outputs the report terminates in; `None` = not built (yet).
#[derive(Clone, Debug, Default)]
pub struct DagArtifacts {
    pub native: Option<PathBuf>,
    pub web: Option<PathBuf>,
    /// A built `assets.pak` (compressed); without one the pak's size is the raw estimate.
    pub pak: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Target {
    /// Native and web release together — used when no module or reference differs
    /// between the two, so drawing both would duplicate every edge.
    Release,
    Native,
    Web,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Asset,
    Pak,
    Module,
    Target(Target),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum EdgeKind {
    Pipeline,
    Pack,
    Use(Usage),
    Ship(Target),
}

struct Node {
    path: PathBuf,
    kind: Kind,
    origin: Option<usize>,
}

struct Edge {
    from: usize,
    to: usize,
    kind: EdgeKind,
    /// Pipeline rule name, or the narrowed build scope of a use.
    label: String,
}

// Base theme: GitHub Dark (@primer/primitives functional/themes/dark.css; token in comments).
const SURFACE: &str = "#0d1117"; // --bgColor-default
const NODE_FILL: &str = "#151b23"; // --bgColor-muted
const BORDER: &str = "#3d444d"; // --borderColor-default
const TEXT_PRIMARY: &str = "#f0f6fc"; // --fgColor-default
const TEXT_MUTED: &str = "#9198a1"; // --fgColor-muted

// Pill Engine brand (pillengine.org CSS tokens) for the header, modules and targets.
const BRAND: &str = "#ff4444"; // --brand (brand-500)
const BRAND_100: &str = "#ffe0e0"; // --brand-100
const BRAND_300: &str = "#ff9494"; // --brand-300
const BRAND_400: &str = "#ff6363"; // --brand-400
const BRAND_700: &str = "#c41c1c"; // --brand-700
const BRAND_900: &str = "#871919"; // --brand-900

// Per-origin asset outline. Index = section order; wraps for extras.
const PALETTE: &[&str] = &[
    "#4493f8", // section 0 (engine) — --fgColor-accent
    "#db6d28", // section 1 (game)   — --fgColor-severe
    "#ab7df8", // extra — --fgColor-done
    "#3fb950", // extra — --fgColor-success
];
const FONT_MONO: &str = "'Cascadia Mono', Consolas, ui-monospace, monospace"; // --font-code
const PACK_COLOR: &str = "#d29922"; // --fgColor-attention
const MISSING_COLOR: &str = "#f85149"; // --fgColor-danger

/// Link color for how a module consumes an asset.
fn usage_color(mode: Usage) -> &'static str {
    match mode {
        Usage::Embedded => "#3fb950", // --fgColor-success
        Usage::Loaded => "#4493f8",   // --fgColor-accent
        Usage::Fetched => "#ab7df8",  // --fgColor-done
    }
}

fn target_color(target: Target) -> &'static str {
    match target {
        Target::Release | Target::Native => BRAND,
        Target::Web => BRAND_700,
        Target::Editor => BRAND_300,
    }
}

/// Short asset type from the file extension.
pub fn asset_type(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("cooked_mesh") => "mesh",
        Some("cooked_tex" | "rtex" | "png" | "jpg" | "jpeg") => "texture",
        Some("hdr" | "exr") => "env map",
        Some("obj" | "glb" | "gltf" | "fbx" | "blend") => "model source",
        Some("mtl") => "material lib",
        Some("wgsl") => "shader",
        Some("hlsl") => "shader source",
        Some("ini") => "config",
        Some("ron") => "data",
        Some("wav" | "mp3" | "ogg" | "flac") => "audio",
        Some("procedural_equirect" | "brdf_lut") => "bake recipe",
        Some("pak") => "asset pack",
        Some("f32bin") => "baked data",
        _ => "file",
    }
}

/// Render the merged asset DAG across `sections` to an SVG file at `path`.
pub fn write_dag_svg(sections: &[DagSection], artifacts: &DagArtifacts, path: &Path) -> Result<()> {
    let svg = render(sections, artifacts);
    crate::util::write_atomic(path, svg.as_bytes())
}

#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    index: HashMap<PathBuf, usize>,
    edges: Vec<Edge>,
}

impl Graph {
    fn node(&mut self, path: &Path, kind: Kind, origin: Option<usize>) -> usize {
        let key = normalize_path(path);
        if let Some(&id) = self.index.get(&key) {
            // A pak is also referenced as a generated asset (include_bytes!(OUT_DIR…)).
            if kind == Kind::Pak {
                self.nodes[id].kind = Kind::Pak;
            }
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            path: key.clone(),
            kind,
            origin,
        });
        self.index.insert(key, id);
        id
    }

    fn edge(&mut self, from: usize, to: usize, kind: EdgeKind, label: String) {
        let exists = self
            .edges
            .iter()
            .any(|edge| edge.from == from && edge.to == to && edge.kind == kind);
        if !exists {
            self.edges.push(Edge {
                from,
                to,
                kind,
                label,
            });
        }
    }
}

fn origin_of(sections: &[DagSection], path: &Path) -> Option<usize> {
    sections.iter().position(|section| {
        path.starts_with(&section.root)
            || section
                .root
                .parent()
                .is_some_and(|crate_dir| path.starts_with(crate_dir.join(GENERATED_DIR)))
    })
}

fn pak_path(section: &DagSection) -> PathBuf {
    section
        .root
        .parent()
        .unwrap_or(&section.root)
        .join(GENERATED_DIR)
        .join("assets.pak")
}

/// A reference's builds; a crate's asset pack only exists in release/wasm builds (its build
/// script writes an empty placeholder otherwise), whatever cfg the embedding line carries.
fn effective_scope(section: &DagSection, asset_ref: &crate::usage::AssetRef) -> Scope {
    let release = Scope {
        native: true,
        web: true,
        editor: false,
    };
    if !section.packed.is_empty() && normalize_path(&asset_ref.asset) == pak_path(section) {
        asset_ref.scope.and(release)
    } else {
        asset_ref.scope
    }
}

fn render(sections: &[DagSection], artifacts: &DagArtifacts) -> String {
    // 1. Build the graph: pipeline → pack → use → ship.
    let mut graph = Graph::default();
    let mut targets: HashMap<Target, usize> = HashMap::new();
    // Split mac/web only when some module or reference is compiled into one but not
    // the other (e.g. a `cfg(not(target_arch = "wasm32"))` module that uses assets).
    let split_release = sections.iter().any(|section| {
        section
            .usage
            .modules
            .values()
            .any(|scope| scope.native != scope.web)
            || section
                .usage
                .refs
                .iter()
                .any(|asset_ref| asset_ref.scope.native != asset_ref.scope.web)
    });
    let target_path = |target: Target| -> PathBuf {
        match target {
            Target::Release => PathBuf::from("<release>"),
            Target::Native => artifacts
                .native
                .clone()
                .unwrap_or_else(|| PathBuf::from("<native release>")),
            Target::Web => artifacts
                .web
                .clone()
                .unwrap_or_else(|| PathBuf::from("<web release>")),
            Target::Editor => PathBuf::from("<editor>"),
        }
    };
    let mut pak_entries: HashMap<usize, Vec<usize>> = HashMap::new();

    for (section_index, section) in sections.iter().enumerate() {
        for (from, to, rule) in &section.edges {
            let from_id = graph.node(from, Kind::Asset, origin_of(sections, from));
            let to_id = graph.node(to, Kind::Asset, origin_of(sections, to));
            graph.edge(from_id, to_id, EdgeKind::Pipeline, rule.to_string());
        }
        if !section.packed.is_empty() {
            let pak_id = graph.node(&pak_path(section), Kind::Pak, Some(section_index));
            for asset in &section.packed {
                let asset_id = graph.node(asset, Kind::Asset, origin_of(sections, asset));
                graph.edge(asset_id, pak_id, EdgeKind::Pack, String::new());
                pak_entries.entry(pak_id).or_default().push(asset_id);
            }
        }
        let mut modules: Vec<(&PathBuf, &Scope)> = section.usage.modules.iter().collect();
        modules.sort_by(|first, second| first.0.cmp(second.0));
        for (module, module_scope) in modules {
            let module_id = graph.node(module, Kind::Module, Some(section_index));
            let ships = if split_release {
                vec![
                    (Target::Native, module_scope.native),
                    (Target::Web, module_scope.web),
                    (Target::Editor, module_scope.editor),
                ]
            } else {
                vec![
                    (Target::Release, module_scope.native || module_scope.web),
                    (Target::Editor, module_scope.editor),
                ]
            };
            for (target, enabled) in ships {
                if !enabled {
                    continue;
                }
                let target_id = *targets.entry(target).or_insert_with(|| {
                    graph.node(&target_path(target), Kind::Target(target), None)
                });
                graph.edge(module_id, target_id, EdgeKind::Ship(target), String::new());
            }
        }
        for asset_ref in &section.usage.refs {
            let module_scope = section
                .usage
                .modules
                .get(&asset_ref.module)
                .copied()
                .unwrap_or(Scope::ALL);
            let module_id = graph.node(&asset_ref.module, Kind::Module, Some(section_index));
            let asset_id = graph.node(
                &asset_ref.asset,
                Kind::Asset,
                origin_of(sections, &asset_ref.asset),
            );
            let scope = effective_scope(section, asset_ref);
            let label = if scope != module_scope {
                format!("{} only", scope.label())
            } else {
                String::new()
            };
            graph.edge(asset_id, module_id, EdgeKind::Use(asset_ref.mode), label);
        }
    }

    let node_count = graph.nodes.len();
    if node_count == 0 {
        return empty_svg();
    }
    let nodes = &graph.nodes;
    let edges = &graph.edges;

    // Per-node relations.
    let mut uses_of: Vec<Vec<usize>> = vec![Vec::new(); node_count]; // edge ids asset→module
    let mut pipeline_in = vec![0usize; node_count];
    let mut pipeline_children: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    let mut producer: Vec<Option<(usize, &str)>> = vec![None; node_count];
    let mut first_child: Vec<Option<(usize, &str)>> = vec![None; node_count];
    let mut packed_into: Vec<Option<usize>> = vec![None; node_count];
    for (edge_id, edge) in edges.iter().enumerate() {
        match edge.kind {
            EdgeKind::Pipeline => {
                pipeline_in[edge.to] += 1;
                pipeline_children[edge.from].push(edge.to);
                producer[edge.to].get_or_insert((edge.from, edge.label.as_str()));
                first_child[edge.from].get_or_insert((edge.to, edge.label.as_str()));
            }
            EdgeKind::Pack => packed_into[edge.from] = Some(edge.to),
            EdgeKind::Use(_) => uses_of[edge.from].push(edge_id),
            EdgeKind::Ship(_) => {}
        }
    }
    let ref_scope = |edge_id: usize| -> Scope {
        let edge = &edges[edge_id];
        let module = &nodes[edge.to];
        let origin = module.origin.unwrap_or(0);
        sections
            .get(origin)
            .and_then(|section| {
                section
                    .usage
                    .refs
                    .iter()
                    .find(|asset_ref| {
                        normalize_path(&asset_ref.module) == module.path
                            && normalize_path(&asset_ref.asset) == nodes[edge.from].path
                    })
                    .map(|asset_ref| effective_scope(section, asset_ref))
            })
            .unwrap_or(Scope::ALL)
    };
    let union = |first: Scope, second: Scope| Scope {
        native: first.native || second.native,
        web: first.web || second.web,
        editor: first.editor || second.editor,
    };
    // Builds each asset/pak reaches through its uses.
    let direct_reach: Vec<Scope> = (0..node_count)
        .map(|node_index| {
            uses_of[node_index]
                .iter()
                .fold(Scope::default(), |scope, &edge_id| {
                    union(scope, ref_scope(edge_id))
                })
        })
        .collect();
    // Own uses + the pack it rides in + (transitively) whatever its cooked outputs reach.
    let mut reach: Vec<Scope> = (0..node_count)
        .map(|node_index| match packed_into[node_index] {
            Some(pak) => union(direct_reach[node_index], direct_reach[pak]),
            None => direct_reach[node_index],
        })
        .collect();
    loop {
        let mut changed = false;
        for edge in edges.iter().filter(|edge| edge.kind == EdgeKind::Pipeline) {
            let widened = union(reach[edge.from], reach[edge.to]);
            if widened != reach[edge.from] {
                reach[edge.from] = widened;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Loaded at runtime by shipped code but not carried in any release pack.
    let missing_in_release: Vec<bool> = (0..node_count)
        .map(|node_index| {
            nodes[node_index].kind == Kind::Asset
                && packed_into[node_index].is_none()
                && uses_of[node_index].iter().any(|&edge_id| {
                    let scope = ref_scope(edge_id);
                    edges[edge_id].kind == EdgeKind::Use(Usage::Loaded)
                        && (scope.native || scope.web)
                })
        })
        .collect();
    let referenced: Vec<bool> = (0..node_count)
        .map(|node_index| !uses_of[node_index].is_empty())
        .collect();
    // A produced output nothing uses or packs is dead weight.
    let dead: Vec<bool> = (0..node_count)
        .map(|node_index| {
            nodes[node_index].kind == Kind::Asset
                && pipeline_in[node_index] > 0
                && pipeline_children[node_index].is_empty()
                && !referenced[node_index]
                && packed_into[node_index].is_none()
        })
        .collect();

    // Per-node facts from disk: size + last re-cook (the file's mtime).
    let metadata: Vec<Option<std::fs::Metadata>> = nodes
        .iter()
        .map(|node| match node.kind {
            Kind::Asset | Kind::Target(Target::Native) | Kind::Target(Target::Web) => {
                std::fs::metadata(&node.path).ok()
            }
            _ => None,
        })
        .collect();
    let mut sizes: Vec<Option<u64>> = metadata
        .iter()
        .map(|entry| entry.as_ref().map(|meta| meta.len()))
        .collect();
    // Pak size: the built file when there is one; else the uncompressed PPAK v4 layout,
    // 12-byte header + per entry (4 + name + 25) + data.
    let built_pak_size = artifacts
        .pak
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len());
    for (&pak_id, entries) in &pak_entries {
        if let Some(bytes) = built_pak_size {
            sizes[pak_id] = Some(bytes);
            continue;
        }
        let root = nodes[pak_id]
            .origin
            .map(|origin| sections[origin].root.clone())
            .unwrap_or_default();
        let bytes: u64 = 12
            + entries
                .iter()
                .map(|&asset_id| {
                    let name_len = nodes[asset_id]
                        .path
                        .strip_prefix(&root)
                        .map(|relative| relative.to_string_lossy().len())
                        .unwrap_or(0) as u64;
                    29 + name_len + sizes[asset_id].unwrap_or(0)
                })
                .sum::<u64>();
        sizes[pak_id] = Some(bytes);
    }
    let recooked: Vec<Option<SystemTime>> = metadata
        .iter()
        .map(|entry| entry.as_ref().and_then(|meta| meta.modified().ok()))
        .collect();
    let asset_total: u64 = (0..node_count)
        .filter(|&node_index| nodes[node_index].kind == Kind::Asset)
        .filter_map(|node_index| sizes[node_index])
        .sum();
    let cook_times: Vec<Option<Duration>> = (0..node_count)
        .map(|node_index| {
            nodes[node_index].origin.and_then(|section_index| {
                sections[section_index]
                    .cook_times
                    .get(&nodes[node_index].path)
                    .copied()
            })
        })
        .collect();
    let total_cook: Duration = cook_times.iter().flatten().copied().sum();
    let last_recook: Option<SystemTime> = (0..node_count)
        .filter(|&node_index| nodes[node_index].kind == Kind::Asset)
        .filter_map(|node_index| recooked[node_index])
        .max();
    let now = SystemTime::now();

    // 2. Columns: assets by longest pipeline path; then pak, modules, targets.
    let is_asset = |node_index: usize| nodes[node_index].kind == Kind::Asset;
    let mut layers = vec![0usize; node_count];
    {
        let mut remaining = pipeline_in.clone();
        let mut queue: Vec<usize> = (0..node_count)
            .filter(|&node_index| is_asset(node_index) && remaining[node_index] == 0)
            .collect();
        let mut head = 0;
        while head < queue.len() {
            let current = queue[head];
            head += 1;
            for &child in &pipeline_children[current] {
                layers[child] = layers[child].max(layers[current] + 1);
                remaining[child] -= 1;
                if remaining[child] == 0 {
                    queue.push(child);
                }
            }
        }
    }
    let max_asset_layer = (0..node_count)
        .filter(|&node_index| is_asset(node_index))
        .map(|node_index| layers[node_index])
        .max()
        .unwrap_or(0);
    let has_pak = nodes.iter().any(|node| node.kind == Kind::Pak);
    let pak_layer = max_asset_layer + 1;
    let module_layer = if has_pak { pak_layer + 1 } else { pak_layer };
    let target_layer = module_layer + 1;
    for (node_index, node) in nodes.iter().enumerate() {
        match node.kind {
            Kind::Asset => {}
            Kind::Pak => layers[node_index] = pak_layer,
            Kind::Module => layers[node_index] = module_layer,
            Kind::Target(_) => layers[node_index] = target_layer,
        }
    }

    // 3. Rows. Assets: tidy walk so pipeline chains stay on one line, sources
    //    ordered by (origin, name). Pak/modules/targets: centered on their inputs,
    //    then de-overlapped per column.
    let names: Vec<String> = nodes.iter().map(|node| node_name(node, sections)).collect();
    for child_list in &mut pipeline_children {
        child_list.sort_by(|&first, &second| names[first].cmp(&names[second]));
        child_list.dedup();
    }
    let mut sources: Vec<usize> = (0..node_count)
        .filter(|&node_index| is_asset(node_index) && pipeline_in[node_index] == 0)
        .collect();
    sources.sort_by(|&first, &second| {
        let origin_first = nodes[first].origin.unwrap_or(usize::MAX);
        let origin_second = nodes[second].origin.unwrap_or(usize::MAX);
        origin_first
            .cmp(&origin_second)
            .then_with(|| names[first].cmp(&names[second]))
    });
    fn assign(
        node: usize,
        children: &[Vec<usize>],
        placed: &mut [bool],
        rows: &mut [f64],
        next_row: &mut f64,
    ) {
        if placed[node] {
            return;
        }
        placed[node] = true;
        let child_nodes = &children[node];
        if child_nodes.is_empty() {
            rows[node] = *next_row;
            *next_row += 1.0;
            return;
        }
        for &child in child_nodes {
            assign(child, children, placed, rows, next_row);
        }
        rows[node] =
            child_nodes.iter().map(|&child| rows[child]).sum::<f64>() / child_nodes.len() as f64;
    }
    let mut rows = vec![0f64; node_count];
    let mut placed = vec![false; node_count];
    let mut next_row = 0f64;
    for &source in &sources {
        assign(
            source,
            &pipeline_children,
            &mut placed,
            &mut rows,
            &mut next_row,
        );
    }
    for node_index in 0..node_count {
        if is_asset(node_index) && !placed[node_index] {
            rows[node_index] = next_row;
            next_row += 1.0;
        }
    }
    let center_on_inputs = |rows: &mut Vec<f64>, kind_matches: &dyn Fn(Kind) -> bool| {
        let mut column: Vec<usize> = (0..node_count)
            .filter(|&node_index| kind_matches(nodes[node_index].kind))
            .collect();
        for &node_index in &column {
            let inputs: Vec<f64> = edges
                .iter()
                .filter(|edge| edge.to == node_index)
                .map(|edge| rows[edge.from])
                .collect();
            if !inputs.is_empty() {
                rows[node_index] = inputs.iter().sum::<f64>() / inputs.len() as f64;
            }
        }
        column.sort_by(|&first, &second| rows[first].total_cmp(&rows[second]));
        for index in 1..column.len() {
            let previous = rows[column[index - 1]];
            if rows[column[index]] < previous + 1.0 {
                rows[column[index]] = previous + 1.0;
            }
        }
    };
    center_on_inputs(&mut rows, &|kind| kind == Kind::Pak);
    center_on_inputs(&mut rows, &|kind| kind == Kind::Module);
    center_on_inputs(&mut rows, &|kind| matches!(kind, Kind::Target(_)));

    // --- Header lines, computed up-front (they set the header height). ---
    let asset_ids: Vec<usize> = (0..node_count)
        .filter(|&node_index| is_asset(node_index))
        .collect();
    let module_count = nodes
        .iter()
        .filter(|node| node.kind == Kind::Module)
        .count();
    let inventory = format!(
        "{} sources · {} assets · {} modules · {}",
        sections.len(),
        asset_ids.len(),
        module_count,
        human_size(asset_total)
    );
    let types = {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for &node_index in &asset_ids {
            let kind = asset_type(&nodes[node_index].path);
            match counts.iter_mut().find(|(name, _)| *name == kind) {
                Some((_, count)) => *count += 1,
                None => counts.push((kind, 1)),
            }
        }
        counts.sort_by(|first, second| second.1.cmp(&first.1).then(first.0.cmp(second.0)));
        counts
            .iter()
            .map(|(name, count)| format!("{count} {name}"))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let count_assets = |predicate: &dyn Fn(usize) -> bool| {
        asset_ids
            .iter()
            .filter(|&&node_index| predicate(node_index))
            .count()
    };
    let has_use = |node_index: usize, mode: Usage| {
        uses_of[node_index]
            .iter()
            .any(|&edge_id| edges[edge_id].kind == EdgeKind::Use(mode))
    };
    let use_summary = format!(
        "{} embedded · {} loaded · {} packed · {} packed-unreferenced · {} dead · {} missing-in-release",
        count_assets(&|node_index| has_use(node_index, Usage::Embedded)),
        count_assets(&|node_index| has_use(node_index, Usage::Loaded)),
        count_assets(&|node_index| packed_into[node_index].is_some()),
        count_assets(&|node_index| packed_into[node_index].is_some() && !referenced[node_index]),
        count_assets(&|node_index| dead[node_index]),
        count_assets(&|node_index| missing_in_release[node_index]),
    );
    let target_summary = {
        let mut parts = Vec::new();
        let release_os = if std::env::consts::OS == "macos" {
            "mac"
        } else {
            "native"
        };
        for (role, path) in [
            (format!("{release_os} release"), &artifacts.native),
            ("web release".to_string(), &artifacts.web),
        ] {
            if let Some(path) = path {
                parts.push(format!(
                    "{role} {} {}",
                    file_label(path),
                    artifact_size(path)
                ));
            }
        }
        for (node_index, node) in nodes.iter().enumerate() {
            if node.kind == Kind::Pak {
                parts.push(format!(
                    "{} {}",
                    names[node_index],
                    sizes[node_index].map(human_size).unwrap_or_default()
                ));
            }
        }
        parts.join(" · ")
    };
    let timing = {
        let mut parts = Vec::new();
        if total_cook.as_millis() > 0 {
            parts.push(format!("{} total", human_dur(total_cook)));
        }
        if let Some(time) = last_recook {
            parts.push(format!("last re-cook {}", rel_time(now, time)));
        }
        parts.join(" · ")
    };
    let mut groups: Vec<(&str, &str)> = vec![("Inventory", inventory.as_str())];
    if !types.is_empty() {
        groups.push(("Types", types.as_str()));
    }
    groups.push(("Use", use_summary.as_str()));
    if !target_summary.is_empty() {
        groups.push(("Artifacts", target_summary.as_str()));
    }
    if !timing.is_empty() {
        groups.push(("Cook", timing.as_str()));
    }
    let legend_note =
        "thick=source · dashed=dead/unreferenced · red=missing in release · edge label=narrower cfg";
    let header_height = 46.0 + groups.len() as f64 * LINE_HEIGHT + 2.0 * LINE_HEIGHT;

    let max_layer = layers.iter().copied().max().unwrap_or(0);
    let positions: Vec<(f64, f64)> = (0..node_count)
        .map(|node_index| {
            (
                MARGIN + layers[node_index] as f64 * (COLUMN_WIDTH + COLUMN_GAP),
                header_height + MARGIN + rows[node_index] * (ROW_HEIGHT + ROW_GAP),
            )
        })
        .collect();
    let right = MARGIN + max_layer as f64 * (COLUMN_WIDTH + COLUMN_GAP) + COLUMN_WIDTH;
    let bottom = positions.iter().map(|&(_, y)| y).fold(0.0_f64, f64::max) + ROW_HEIGHT;
    let group_width = groups
        .iter()
        .map(|(label, value)| {
            MARGIN + (label.len() + 2 + value.chars().count()) as f64 * CHAR_WIDTH + MARGIN
        })
        .fold(0.0_f64, f64::max);
    let legend_width = MARGIN + 900.0 + legend_note.chars().count() as f64 * 6.6;
    let width = (right + MARGIN)
        .max(group_width)
        .max(legend_width)
        .max(560.0);
    let height = bottom + MARGIN;

    // 4. Emit SVG.
    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" font-family="{FONT_MONO}">
<defs>
<marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
<path d="M0,0 L10,5 L0,10 z" fill="{TEXT_MUTED}"/>
</marker>
</defs>
<rect width="100%" height="100%" fill="{SURFACE}"/>
<text x="{MARGIN:.0}" y="26" font-size="18" font-weight="bold" font-family="Chillax, Outfit, Inter, sans-serif" fill="{BRAND}">{TITLE}</text>
"##
    );
    let mut line_y = 46.0;
    for (label, value) in &groups {
        let _ = writeln!(
            svg,
            r##"<text x="{MARGIN:.0}" y="{line_y:.0}" font-size="12" fill="{TEXT_PRIMARY}"><tspan font-weight="bold" fill="{BRAND_300}">{label}</tspan>  {value}</text>"##,
            label = esc(label),
            value = esc(value),
        );
        line_y += LINE_HEIGHT;
    }

    // Legend: origins, link kinds, targets, note.
    let legend_y = line_y + 4.0;
    let mut legend_x = MARGIN;
    for (section_index, section) in sections.iter().enumerate() {
        let (fill, stroke) = palette(Some(section_index));
        let _ = writeln!(
            svg,
            r##"<rect x="{legend_x:.0}" y="{rect_y:.0}" width="14" height="14" rx="3" fill="{fill}" stroke="{stroke}" stroke-width="1.5"/><text x="{text_x:.0}" y="{legend_y:.0}" font-size="12" fill="{TEXT_PRIMARY}">{label}</text>"##,
            rect_y = legend_y - 11.0,
            text_x = legend_x + 20.0,
            label = esc(&section.label),
        );
        legend_x += 30.0 + section.label.chars().count() as f64 * CHAR_WIDTH;
    }
    let mut link_kinds: Vec<(&str, &str)> = vec![
        (usage_color(Usage::Embedded), "embedded"),
        (usage_color(Usage::Loaded), "loaded"),
        (usage_color(Usage::Fetched), "fetched"),
        (PACK_COLOR, "packed"),
    ];
    if split_release {
        link_kinds.push((target_color(Target::Native), "→native"));
        link_kinds.push((target_color(Target::Web), "→web"));
    } else {
        link_kinds.push((target_color(Target::Release), "→release"));
    }
    link_kinds.push((target_color(Target::Editor), "→editor"));
    for (color, label) in link_kinds {
        let _ = writeln!(
            svg,
            r##"<line x1="{legend_x:.0}" y1="{line_y:.0}" x2="{end_x:.0}" y2="{line_y:.0}" stroke="{color}" stroke-width="3"/><text x="{text_x:.0}" y="{legend_y:.0}" font-size="12" fill="{TEXT_PRIMARY}">{label}</text>"##,
            line_y = legend_y - 4.0,
            end_x = legend_x + 18.0,
            text_x = legend_x + 22.0,
            label = esc(label),
        );
        legend_x += 30.0 + label.chars().count() as f64 * CHAR_WIDTH;
    }
    let _ = writeln!(
        svg,
        r##"<text x="{MARGIN:.0}" y="{note_y:.0}" font-size="11" fill="{TEXT_MUTED}">{note}</text>"##,
        note_y = legend_y + LINE_HEIGHT,
        note = esc(legend_note),
    );

    // Edges under nodes; each wrapped in a <g class="edge"> for hover highlighting.
    for (edge_id, edge) in edges.iter().enumerate() {
        let _ = write!(svg, r##"<g class="edge" data-e="{edge_id}">"##);
        let (from_x, from_y) = positions[edge.from];
        let (to_x, to_y) = positions[edge.to];
        let start_x = from_x + COLUMN_WIDTH;
        let start_y = from_y + ROW_HEIGHT / 2.0;
        let end_y = to_y + ROW_HEIGHT / 2.0;
        let (color, width, opacity) = match edge.kind {
            EdgeKind::Pipeline => (TEXT_MUTED, 1.3, 0.9),
            EdgeKind::Pack => (PACK_COLOR, 1.4, 0.6),
            EdgeKind::Use(mode) => (usage_color(mode), 1.4, 0.7),
            EdgeKind::Ship(target) => (target_color(target), 2.0, 0.8),
        };
        let dash = if edge.label.is_empty() || edge.kind == EdgeKind::Pipeline {
            ""
        } else {
            r##" stroke-dasharray="5 3""##
        };
        bezier_path(
            &mut svg,
            start_x,
            start_y,
            to_x,
            end_y,
            &format!(
                r##"stroke="{color}" stroke-width="{width}" opacity="{opacity}"{dash} marker-end="url(#arrow)""##
            ),
        );
        if !edge.label.is_empty() {
            let _ = writeln!(
                svg,
                r##"<text x="{mid_x:.1}" y="{label_y:.1}" font-size="10" fill="{color}" text-anchor="middle">{label}</text>"##,
                mid_x = (start_x + to_x) / 2.0,
                label_y = (start_y + end_y) / 2.0 - 4.0,
                label = esc(&edge.label),
            );
        }
        svg.push_str("</g>\n");
    }

    // Nodes.
    for node_index in 0..node_count {
        let node = &nodes[node_index];
        let (x, y) = positions[node_index];
        let (mut fill, mut stroke) = palette(node.origin);
        let mut border = 1.3;
        let mut dash = "";
        let mut subtitle_color = TEXT_MUTED;
        let mut rx = 6.0;
        let subtitle = match node.kind {
            Kind::Asset => {
                if pipeline_in[node_index] == 0 {
                    border = 2.6;
                }
                if dead[node_index]
                    || (packed_into[node_index].is_some() && !referenced[node_index])
                {
                    dash = r##" stroke-dasharray="4 3""##;
                }
                if dead[node_index] {
                    fill = SURFACE;
                    stroke = BORDER;
                }
                if missing_in_release[node_index] {
                    stroke = MISSING_COLOR;
                    border = 2.6;
                }
                let mut subtitle = format!(
                    "{} · {}",
                    asset_type(&node.path),
                    sizes[node_index]
                        .map(human_size)
                        .unwrap_or_else(|| "—".into())
                );
                if let Some(duration) = cook_times[node_index] {
                    subtitle.push_str(" · ");
                    subtitle.push_str(&human_dur(duration));
                }
                subtitle
            }
            Kind::Pak => {
                fill = NODE_FILL;
                stroke = PACK_COLOR;
                border = 2.4;
                format!(
                    "{} entries · {}{}",
                    pak_entries.get(&node_index).map_or(0, Vec::len),
                    sizes[node_index].map(human_size).unwrap_or_default(),
                    if built_pak_size.is_some() {
                        " compressed"
                    } else {
                        " raw"
                    }
                )
            }
            Kind::Module => {
                // Brand-colored like the targets, so the crate goes in the subtitle, not the color.
                rx = 16.0;
                fill = BRAND_900;
                stroke = BRAND_400;
                border = 1.8;
                subtitle_color = BRAND_100;
                let scope = scope_of_module(node, sections);
                let crate_label = origin_label(node, sections).unwrap_or("?");
                let directory = module_relative_path(node, sections)
                    .rsplit_once('/')
                    .map(|(directory, _)| format!("{directory}/ · "))
                    .unwrap_or_default();
                let builds = if scope == Scope::ALL {
                    "all builds".to_string()
                } else {
                    scope.label()
                };
                format!("{crate_label} · {directory}{builds}")
            }
            Kind::Target(target) => {
                border = 2.4;
                stroke = target_color(target);
                subtitle_color = BRAND_100;
                fill = if target == Target::Editor {
                    BRAND_900
                } else {
                    target_color(target)
                };
                match target {
                    Target::Editor => "hot-reload · reads res/".into(),
                    Target::Release => [&artifacts.native, &artifacts.web]
                        .iter()
                        .filter_map(|path| path.as_ref())
                        .map(|path| format!("{} {}", artifact_kind(path), artifact_size(path)))
                        .collect::<Vec<_>>()
                        .join(" · "),
                    _ => format!(
                        "{} · {}",
                        file_label(&node.path),
                        sizes[node_index]
                            .map(human_size)
                            .unwrap_or_else(|| "not built".into())
                    ),
                }
            }
        };
        let tooltip_text = tooltip(
            node_index,
            nodes,
            edges,
            sections,
            &names,
            &sizes,
            asset_total,
            &cook_times,
            &recooked,
            now,
            &reach,
            &packed_into,
            &missing_in_release,
            &dead,
            &producer,
            &first_child,
            artifacts,
        );
        let text_center_x = x + COLUMN_WIDTH / 2.0;
        let _ = writeln!(
            svg,
            r##"<g class="node" data-n="{node_index}"><title>{tooltip_text}</title><rect x="{x:.1}" y="{y:.1}" width="{COLUMN_WIDTH:.0}" height="{ROW_HEIGHT:.0}" rx="{rx}" fill="{fill}" stroke="{stroke}" stroke-width="{border}"{dash}/><text x="{text_center_x:.1}" y="{name_y:.1}" font-size="12" fill="{TEXT_PRIMARY}" text-anchor="middle">{name}</text><text x="{text_center_x:.1}" y="{subtitle_y:.1}" font-size="10" fill="{subtitle_color}" text-anchor="middle">{subtitle}</text></g>"##,
            name_y = y + 19.0,
            subtitle_y = y + 35.0,
            name = esc(&truncate(
                &names[node_index],
                COLUMN_WIDTH - 16.0,
                CHAR_WIDTH
            )),
            subtitle = esc(&truncate(
                &subtitle,
                COLUMN_WIDTH - 10.0,
                SUBTITLE_CHAR_WIDTH
            )),
        );
    }

    svg.push_str(&hover_script(edges));
    svg.push_str("</svg>\n");
    svg
}

/// Hover a node → highlight its whole lineage: every node/edge upstream (back to the
/// source assets) and downstream (through pak and modules to the targets); dim the rest.
/// Inline script, so it works when the SVG is opened directly (not via <img>).
fn hover_script(edges: &[Edge]) -> String {
    let edge_list = edges
        .iter()
        .map(|edge| format!("[{},{}]", edge.from, edge.to))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r##"<style>
.dim .node:not(.hl) {{ opacity: 0.5; }}
.dim .edge:not(.hl) {{ opacity: 0.5; }}
.edge.hl path {{ stroke-width: 3; opacity: 1; }}
.node.hl rect {{ stroke-width: 3; }}
</style>
<script><![CDATA[
(function () {{
  const root = document.documentElement;
  const edges = [{edge_list}];
  const down = {{}}, up = {{}};
  edges.forEach(([from, to], id) => {{
    (down[from] = down[from] || []).push([to, id]);
    (up[to] = up[to] || []).push([from, id]);
  }});
  const walk = (start, adjacency, nodes, edgeIds) => {{
    const stack = [start];
    while (stack.length) {{
      const node = stack.pop();
      (adjacency[node] || []).forEach(([next, id]) => {{
        edgeIds.add(id);
        if (!nodes.has(next)) {{ nodes.add(next); stack.push(next); }}
      }});
    }}
  }};
  const nodeEls = root.querySelectorAll('.node');
  const edgeEls = root.querySelectorAll('.edge');
  nodeEls.forEach((group) => {{
    group.addEventListener('mouseenter', () => {{
      const start = +group.getAttribute('data-n');
      const nodes = new Set([start]), edgeIds = new Set();
      walk(start, down, nodes, edgeIds);
      walk(start, up, nodes, edgeIds);
      root.classList.add('dim');
      nodeEls.forEach((el) => el.classList.toggle('hl', nodes.has(+el.getAttribute('data-n'))));
      edgeEls.forEach((el) => el.classList.toggle('hl', edgeIds.has(+el.getAttribute('data-e'))));
    }});
    group.addEventListener('mouseleave', () => root.classList.remove('dim'));
  }});
}})();
]]></script>
"##
    )
}

fn artifact_size(path: &Path) -> String {
    std::fs::metadata(path)
        .map(|meta| human_size(meta.len()))
        .unwrap_or_else(|_| "not built".into())
}

/// "wasm" for a .wasm artifact, "exe" otherwise.
fn artifact_kind(path: &Path) -> &'static str {
    if path
        .extension()
        .is_some_and(|extension| extension == "wasm")
    {
        "wasm"
    } else {
        "exe"
    }
}

/// Label of the section (crate) a node belongs to.
fn origin_label<'a>(node: &Node, sections: &'a [DagSection]) -> Option<&'a str> {
    node.origin
        .and_then(|origin| sections.get(origin))
        .map(|section| section.label.as_str())
}

fn scope_of_module(node: &Node, sections: &[DagSection]) -> Scope {
    node.origin
        .and_then(|origin| sections.get(origin))
        .and_then(|section| {
            section
                .usage
                .modules
                .iter()
                .find(|(module, _)| normalize_path(module) == node.path)
                .map(|(_, scope)| *scope)
        })
        .unwrap_or(Scope::ALL)
}

/// A module's source path relative to its crate's `src/` (e.g. `graphics/pass_fxaa.rs`).
fn module_relative_path(node: &Node, sections: &[DagSection]) -> String {
    node.origin
        .and_then(|origin| sections.get(origin))
        .and_then(|section| section.root.parent())
        .and_then(|crate_dir| node.path.strip_prefix(crate_dir).ok())
        .map(|relative| {
            relative
                .to_string_lossy()
                .trim_start_matches("src/")
                .to_string()
        })
        .unwrap_or_else(|| file_label(&node.path))
}

/// Display name: assets and modules by file name (crate shown by color, directory in
/// the subtitle), targets by role (artifact file in the subtitle).
fn node_name(node: &Node, _sections: &[DagSection]) -> String {
    match node.kind {
        Kind::Asset | Kind::Pak | Kind::Module => file_label(&node.path),
        Kind::Target(Target::Native) => {
            if std::env::consts::OS == "macos" {
                "mac release".into()
            } else {
                "native release".into()
            }
        }
        Kind::Target(Target::Web) => "web release".into(),
        Kind::Target(Target::Release) => {
            if std::env::consts::OS == "macos" {
                "release: mac + web".into()
            } else {
                "release: native + web".into()
            }
        }
        Kind::Target(Target::Editor) => "editor / tools".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn tooltip(
    node_index: usize,
    nodes: &[Node],
    edges: &[Edge],
    sections: &[DagSection],
    names: &[String],
    sizes: &[Option<u64>],
    total: u64,
    cook_times: &[Option<Duration>],
    recooked: &[Option<SystemTime>],
    now: SystemTime,
    reach: &[Scope],
    packed_into: &[Option<usize>],
    missing_in_release: &[bool],
    dead: &[bool],
    producer: &[Option<(usize, &str)>],
    first_child: &[Option<(usize, &str)>],
    artifacts: &DagArtifacts,
) -> String {
    let node = &nodes[node_index];
    let origin_name = origin_label(node, sections).unwrap_or("external");
    let size_line = match sizes[node_index] {
        Some(bytes) if total > 0 && node.kind == Kind::Asset => format!(
            "{} ({:.1}% of {})",
            human_size(bytes),
            pct(bytes, total),
            human_size(total)
        ),
        Some(bytes) => human_size(bytes),
        None => "not built".into(),
    };
    let mut text = format!("{}\norigin: {}", node.path.to_string_lossy(), origin_name);
    match node.kind {
        Kind::Asset | Kind::Pak => {
            let _ = write!(
                text,
                "\ntype: {}\nsize: {}",
                asset_type(&node.path),
                size_line
            );
            if let Some(duration) = cook_times[node_index] {
                let _ = write!(text, "\ncook: {}", human_dur(duration));
            }
            if let Some(time) = recooked[node_index] {
                let _ = write!(text, "\nlast re-cook: {}", rel_time(now, time));
            }
            let uses: Vec<String> = edges
                .iter()
                .filter(|edge| edge.from == node_index && matches!(edge.kind, EdgeKind::Use(_)))
                .map(|edge| {
                    let EdgeKind::Use(mode) = edge.kind else {
                        unreachable!()
                    };
                    let narrowed = if edge.label.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", edge.label)
                    };
                    let module = &nodes[edge.to];
                    let crate_label = origin_label(module, sections).unwrap_or("?");
                    format!(
                        "{crate_label}:{} ({}{})",
                        module_relative_path(module, sections),
                        mode.label(),
                        narrowed
                    )
                })
                .collect();
            if uses.is_empty() {
                text.push_str("\nused by: —");
            } else {
                let _ = write!(text, "\nused by: {}", uses.join("; "));
            }
            if let Some(pak) = packed_into[node_index] {
                let _ = write!(text, "\npacked into: {}", names[pak]);
            }
            let reached = reach[node_index];
            let _ = write!(
                text,
                "\nreaches: {}",
                if reached.is_empty() {
                    "nothing".into()
                } else {
                    reached.label()
                }
            );
            if missing_in_release[node_index] {
                text.push_str(
                    "\nMISSING IN RELEASE: loaded at runtime by shipped code but not packed",
                );
            }
            if dead[node_index] {
                text.push_str("\nDEAD: produced but neither used nor packed");
            }
            if node.kind == Kind::Asset {
                let _ = write!(
                    text,
                    "\npipeline: {}",
                    lane(node_index, producer, first_child, names)
                );
            }
        }
        Kind::Module => {
            let _ = write!(text, "\nmodule: {}", module_relative_path(node, sections));
            let used: Vec<&str> = edges
                .iter()
                .filter(|edge| edge.to == node_index)
                .map(|edge| names[edge.from].as_str())
                .collect();
            let _ = write!(
                text,
                "\ncompiled into: {}\nuses: {}",
                scope_of_module(node, sections).label(),
                used.join(", ")
            );
        }
        Kind::Target(Target::Release) => {
            text = "native + web release (every module and reference compiles into both)".into();
            for path in [&artifacts.native, &artifacts.web].into_iter().flatten() {
                let _ = write!(
                    text,
                    "\n{}: {}",
                    path.to_string_lossy(),
                    artifact_size(path)
                );
            }
            let modules = edges.iter().filter(|edge| edge.to == node_index).count();
            let _ = write!(text, "\nmodules linked in: {modules}");
        }
        Kind::Target(_) => {
            let _ = write!(text, "\nsize: {size_line}");
            let modules = edges.iter().filter(|edge| edge.to == node_index).count();
            let _ = write!(text, "\nmodules linked in: {modules}");
        }
    }
    esc(&text)
}

/// Full pipeline lane through node `node_index`: walk back to a source, forward to a
/// leaf, joining file names with the rule applied at each step; mark it with «».
fn lane(
    node_index: usize,
    producer: &[Option<(usize, &str)>],
    first_child: &[Option<(usize, &str)>],
    names: &[String],
) -> String {
    let mut backward: Vec<usize> = vec![node_index];
    let mut current = node_index;
    while let Some((parent, _)) = producer[current] {
        backward.push(parent);
        current = parent;
        if backward.len() > 256 {
            break; // cycle guard
        }
    }
    backward.reverse();

    let mut forward: Vec<(usize, &str)> = Vec::new();
    let mut current = node_index;
    while let Some((child, rule)) = first_child[current] {
        forward.push((child, rule));
        current = child;
        if forward.len() > 256 {
            break;
        }
    }

    let mut lane = String::new();
    for &node in &backward {
        if let Some((_, rule)) = producer[node] {
            let _ = write!(lane, " →({}) ", rule);
        }
        if node == node_index {
            let _ = write!(lane, "«{}»", names[node]);
        } else {
            lane.push_str(&names[node]);
        }
    }
    for (node, rule) in &forward {
        let _ = write!(lane, " →({}) {}", rule, names[*node]);
    }
    lane
}

/// (fill, stroke) of an asset node from its origin section.
fn palette(origin: Option<usize>) -> (&'static str, &'static str) {
    let stroke = match origin {
        Some(section_index) => PALETTE[section_index % PALETTE.len()],
        None => TEXT_MUTED,
    };
    (NODE_FILL, stroke)
}

/// Emit a left-to-right cubic curve from (start_x,start_y) to (end_x,end_y) with the
/// given trailing stroke attributes (`stroke=… stroke-width=… …`).
fn bezier_path(
    svg: &mut String,
    start_x: f64,
    start_y: f64,
    end_x: f64,
    end_y: f64,
    attributes: &str,
) {
    let mid_x = (start_x + end_x) / 2.0;
    let _ = writeln!(
        svg,
        r##"<path d="M{start_x:.1},{start_y:.1} C{mid_x:.1},{start_y:.1} {mid_x:.1},{end_y:.1} {end_x:.1},{end_y:.1}" fill="none" {attributes}/>"##,
    );
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let size = bytes as f64;
    if size < KB {
        format!("{bytes} B")
    } else if size < MB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{:.1} MB", size / MB)
    }
}

fn human_dur(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        format!("{millis} ms")
    } else if duration.as_secs() < 60 {
        format!("{:.1} s", duration.as_secs_f64())
    } else {
        format!("{}m {}s", duration.as_secs() / 60, duration.as_secs() % 60)
    }
}

fn rel_time(now: SystemTime, then: SystemTime) -> String {
    let seconds = now
        .duration_since(then)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64 * 100.0
    }
}

fn truncate(label: &str, max_pixels: f64, char_width: f64) -> String {
    let max_characters = (max_pixels / char_width).floor().max(4.0) as usize;
    if label.chars().count() <= max_characters {
        return label.to_string();
    }
    let kept = max_characters.saturating_sub(1);
    let mut truncated: String = label.chars().take(kept).collect();
    truncated.push('…');
    truncated
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn empty_svg() -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="360" height="80" viewBox="0 0 360 80" font-family="{FONT_MONO}">
<rect width="100%" height="100%" fill="{SURFACE}"/>
<text x="180" y="44" font-size="14" fill="{TEXT_MUTED}" text-anchor="middle">no assets discovered</text>
</svg>
"##,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::AssetRef;

    fn sec(label: &str, root: &str, edges: &[(&str, &str, &'static str)]) -> DagSection {
        DagSection {
            root: PathBuf::from(root),
            label: label.into(),
            edges: edges
                .iter()
                .map(|(from, to, rule)| (PathBuf::from(from), PathBuf::from(to), *rule))
                .collect(),
            cook_times: HashMap::new(),
            usage: UsageScan::default(),
            packed: Vec::new(),
        }
    }

    fn uses(
        section: &mut DagSection,
        asset: &str,
        module: &str,
        mode: Usage,
        scope: Scope,
        module_scope: Scope,
    ) {
        section.usage.refs.push(AssetRef {
            asset: PathBuf::from(asset),
            module: PathBuf::from(module),
            mode,
            scope,
        });
        section
            .usage
            .modules
            .insert(PathBuf::from(module), module_scope);
    }

    fn none() -> DagArtifacts {
        DagArtifacts::default()
    }

    #[test]
    fn pipeline_edges_labelled_and_typed() {
        let svg = render(
            &[sec(
                "engine",
                "/e/res",
                &[("/e/res/a.hlsl", "/e/res/a.wgsl", "hlsl_to_wgsl")],
            )],
            &none(),
        );
        assert!(svg.contains("a.hlsl") && svg.contains("a.wgsl"));
        assert!(svg.contains("hlsl_to_wgsl"));
        assert!(svg.contains("shader source") && svg.contains("shader ·"));
    }

    #[test]
    fn engine_shader_reaches_artifacts_through_its_module() {
        let mut engine = sec(
            "engine",
            "/e/res",
            &[("/e/res/f.hlsl", "/e/res/f.wgsl", "hlsl_to_wgsl")],
        );
        uses(
            &mut engine,
            "/e/res/f.wgsl",
            "/e/src/graphics/pass_fxaa.rs",
            Usage::Embedded,
            Scope::ALL,
            Scope::ALL,
        );
        let svg = render(&[engine], &none());
        assert!(svg.contains(">pass_fxaa.rs<") && svg.contains("engine · graphics/ · all builds"));
        // Every module compiles into both releases → one merged release target.
        assert!(svg.contains("release: mac + web") || svg.contains("release: native + web"));
        assert!(!svg.contains(">web release<"));
        assert!(svg.contains("editor / tools"));
        assert!(svg.contains("reaches: native+web+editor"));
        assert!(svg.contains("1 embedded"));
    }

    #[test]
    fn tool_only_module_links_only_to_editor() {
        let mut game = sec("game", "/g/res", &[]);
        uses(
            &mut game,
            "/g/res/shots.ron",
            "/g/src/tool.rs",
            Usage::Loaded,
            Scope::EDITOR,
            Scope::EDITOR,
        );
        let svg = render(&[game], &none());
        assert!(svg.contains("editor / tools"));
        assert!(!svg.contains("web release"));
        assert!(svg.contains("reaches: editor"));
        assert!(svg.contains("0 missing-in-release"));
    }

    #[test]
    fn packed_asset_reaches_release_through_pak_and_embedding_module() {
        let mut game = sec(
            "game",
            "/g/res",
            &[("/g/res/m.glb", "/g/res/m.cooked_mesh", "glb_to_cooked_mesh")],
        );
        game.packed = vec![
            PathBuf::from("/g/res/m.cooked_mesh"),
            PathBuf::from("/g/res/unused.cooked_mesh"),
        ];
        // Embedded under no cfg: a pack still only exists in release builds.
        uses(
            &mut game,
            "/g/(generated)/assets.pak",
            "/g/src/demo.rs",
            Usage::Embedded,
            Scope::ALL,
            Scope::ALL,
        );
        uses(
            &mut game,
            "/g/res/m.cooked_mesh",
            "/g/src/demo.rs",
            Usage::Loaded,
            Scope::ALL,
            Scope::ALL,
        );
        let svg = render(&[game], &none());
        assert!(svg.contains("assets.pak"));
        assert!(svg.contains("2 entries"));
        assert!(svg.contains("packed into: assets.pak"));
        assert!(svg.contains("native+web only")); // narrowed use edge label
        assert!(svg.contains("2 packed · 1 packed-unreferenced"));
        // The source reaches release through its cooked, packed output.
        assert!(svg.contains("m.glb\norigin") || svg.contains("m.glb"));
        let glb_tip = svg
            .split("<title>")
            .find(|tip| tip.starts_with("/g/res/m.glb"))
            .unwrap();
        assert!(glb_tip.contains("reaches: native+web+editor"));
        assert!(svg.contains("0 missing-in-release"));
    }

    #[test]
    fn loaded_but_unpacked_shipped_asset_is_missing_in_release() {
        let mut game = sec("game", "/g/res", &[]);
        uses(
            &mut game,
            "/g/res/x.wav",
            "/g/src/audio.rs",
            Usage::Loaded,
            Scope::ALL,
            Scope::ALL,
        );
        let svg = render(&[game], &none());
        assert!(svg.contains("1 missing-in-release"));
        assert!(svg.contains("MISSING IN RELEASE"));
        assert!(svg.contains(MISSING_COLOR));
    }

    #[test]
    fn unused_pipeline_output_is_dead() {
        let svg = render(
            &[sec(
                "game",
                "/g/res",
                &[("/g/res/a.glb", "/g/res/a.cooked_mesh", "glb_to_cooked_mesh")],
            )],
            &none(),
        );
        assert!(svg.contains("1 dead"));
        assert!(svg.contains("DEAD: produced but neither used nor packed"));
        assert!(svg.contains("stroke-dasharray"));
    }

    #[test]
    fn cook_time_shown_in_header_and_node() {
        let mut section = sec(
            "engine",
            "eng",
            &[("eng/a.hlsl", "eng/a.wgsl", "hlsl_to_wgsl")],
        );
        section
            .cook_times
            .insert(PathBuf::from("eng/a.wgsl"), Duration::from_millis(42));
        let svg = render(&[section], &none());
        assert!(svg.contains("42 ms total"));
        assert!(svg.contains("cook: 42 ms"));
    }

    #[test]
    fn linear_chain_shares_one_row() {
        let section = sec(
            "engine",
            "eng",
            &[("eng/a", "eng/b", "r1"), ("eng/b", "eng/c", "r2")],
        );
        let svg = render(&[section], &none());
        let node_ys: Vec<String> = svg
            .split("<rect ")
            .filter(|fragment| fragment.contains("rx=\"6\""))
            .filter_map(|fragment| {
                Some(
                    fragment
                        .split("y=\"")
                        .nth(1)?
                        .split('"')
                        .next()?
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(node_ys.len(), 3);
        assert!(node_ys.iter().all(|y| *y == node_ys[0]));
    }

    #[test]
    fn empty_sections_yield_placeholder() {
        assert!(render(&[], &none()).contains("no assets discovered"));
    }

    #[test]
    fn escapes_xml_in_labels() {
        let svg = render(&[sec("g", "r", &[("r/a<b>.x", "r/o.y", "R&D")])], &none());
        assert!(svg.contains("R&amp;D"));
        assert!(!svg.contains(">R&D<"));
    }

    #[test]
    fn release_splits_when_a_module_is_native_only() {
        let mut game = sec("game", "/g/res", &[]);
        let native_only = Scope {
            native: true,
            web: false,
            editor: true,
        };
        uses(
            &mut game,
            "/g/res/a.wav",
            "/g/src/export.rs",
            Usage::Embedded,
            native_only,
            native_only,
        );
        uses(
            &mut game,
            "/g/res/b.ron",
            "/g/src/demo.rs",
            Usage::Embedded,
            Scope::ALL,
            Scope::ALL,
        );
        let svg = render(&[game], &none());
        assert!(svg.contains(">mac release<") || svg.contains(">native release<"));
        assert!(svg.contains(">web release<"));
        assert!(!svg.contains("release: mac + web"));
    }

    #[test]
    fn hover_script_carries_edge_list_and_hooks() {
        let svg = render(
            &[sec(
                "g",
                "/g/res",
                &[("/g/res/a.obj", "/g/res/a.cooked_mesh", "obj")],
            )],
            &none(),
        );
        assert!(svg.contains(r#"<g class="node" data-n="0">"#));
        assert!(svg.contains(r#"<g class="edge" data-e="0">"#));
        assert!(svg.contains("const edges = [[0,1]];"));
        assert!(svg.contains("<![CDATA["));
    }
}
