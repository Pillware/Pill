//! The egui shell's long-lived session state.
//!
//! # Responsibilities
//!
//! - Own [`EguiShellState`], the shell's one world resource: the reload handle,
//!   the pass scratch, the measurements and the two requests the shell and the
//!   executable exchange.
//! - Own the per-tab and chrome state those groups are made of: the menu bar's
//!   geometry ([`MenubarSession`]), the Inspector's rows and geometry
//!   ([`InspectorSession`]), the toolbox ([`ToolboxSession`]), the command
//!   palette ([`PaletteState`]), the toasts ([`ToastSession`]), the windows
//!   modules open ([`PanelWindowSession`]) and the asset previews
//!   ([`PreviewStates`]).
//! - Own the measurements the Statistics tab and the log read
//!   ([`StatisticsSession`], [`PanelBudget`]) and the two log-once caches
//!   ([`LoggedRects`], [`LoggedTexts`]).
//!
//! # Design
//!
//! [`EguiShellState`]'s fields fall into four groups, and a new field belongs
//! to the group whose owner and boundary it shares:
//!
//! - **the reload handle** - `context`, `dock`, `mark`, `popout_contexts`,
//!   `applied_theme_generation`, `panels_remapped`: what a shell generation
//!   must find exactly where the previous one left it. The executable never
//!   writes these.
//! - **pass scratch** - the dialog, the preferences draft, the toolbox, the
//!   browser, the Inspector, the menu bar, the toasts, the palette, the panel
//!   windows and the previews: the shell owns them, and they survive a reload
//!   only so a redraw continues where it was.
//! - **measurements** - `frame_index`, `stats`, `panel_budget`: the shell
//!   writes them around each pass; the Statistics tab and the log read them.
//! - **requests** - `requests` (a pass's asks for the executable, drained onto
//!   the frame) and `pending_panel_window` (the scripted channel's ask for a
//!   panel window, cleared only when that panel draws). `console_filters` is
//!   the one shared field: the Console tab and the executable both read and
//!   write it.
//!
//! The type lives in this shared `dylib`, not in the reloadable shell
//! module, so its identity is stable across shell reloads: the reloaded shell
//! registers the same resource type and keeps the value already in the world,
//! context and dock included. A resource whose type the module defined would
//! be reset by every reload - and the context's drop code would then run from
//! an unloaded image.

// Standard library
use std::collections::{BTreeMap, BTreeSet};

// External crates
use egui::{Context, Pos2, Rect, TextureHandle, TextureOptions};
use pill_engine::Resource;

// Current crate
use crate::assets::AssetDialog;
use crate::boundary::SCENE_VIEWS_MAX;
use crate::browser::ProjectBrowser;
use crate::layout::{EditorDock, EditorTab};
use crate::notices::NoticeLevel;
use crate::requests::ShellRequests;

// =============================================================================
// Constants
// =============================================================================

/// How many panel windows one session may have open at once.
pub const PANEL_WINDOWS_MAX: usize = 8;

/// How many toasts show at once; older ones drop off the stack.
pub const TOASTS_MAX: usize = 5;

/// Frames a toast stays up - about four seconds at 60 fps.
pub const TOAST_FRAMES: u64 = 240;

/// How long one panel body may take, in microseconds, before the editor says
/// so.
pub const PANEL_BUDGET_MICROS: f32 = 4000.0;

/// How many frames in a row a panel must overshoot its budget before the
/// editor says so - a single slow frame (a reload, a texture upload) is not a
/// slow panel.
pub const PANEL_BUDGET_FRAMES: u32 = 120;

// =============================================================================
// Types
// =============================================================================

/// The egui shell's session: the reload handle, the pass scratch, the
/// measurements and the two requests the shell and the executable exchange.
///
/// The file header is the ownership map: a field that must survive a reload
/// sits under the reload handle, a field a pass writes under pass scratch, a
/// field the Statistics tab or the log reads under measurements, and a field
/// another side answers under requests.
pub struct EguiShellState {
    /// The egui context: fonts, memory and widget state.
    pub context: Context,
    /// The dock layout, persisted across restarts.
    pub dock: EditorDock,
    /// How many egui passes the shell has started; the shell's row heartbeat
    /// and its toast expiry read it.
    pub frame_index: u64,
    /// The theme generation this session last installed, as the executable
    /// publishes it on the snapshot. The shell installs the merged theme
    /// whenever this number moves, so a reloaded shell starts with the theme
    /// already in force.
    pub applied_theme_generation: u64,
    /// The open asset dialog, if there is one: the create or rename window's
    /// state, kept here so a shell reload finds it.
    pub asset_dialog: Option<AssetDialog>,
    /// The Preferences tab's unsaved field values, keyed like the file. It is
    /// `None` until the tab seeds it from the snapshot and again after a save,
    /// so the rows re-seed from the file on the next frame.
    pub preferences_draft: Option<BTreeMap<String, String>>,
    /// The floating viewport toolboxes' session state.
    pub toolbox: ToolboxSession,
    /// The Assets tab's project browser: the folder its grid lists, the
    /// tree's open folders and the tile size.
    pub browser: ProjectBrowser,
    /// The brand mark's texture, uploaded on the first frame and kept for the
    /// session: the context owns it, so a shell reload reuses it.
    pub mark: Option<TextureHandle>,
    /// The asset tiles' previews. One value holds the textures the executable
    /// sent back, the tiles it answered with no pixels, and the tiles this
    /// pass drew, because the three together answer one question: may this
    /// tile be asked for now?
    pub previews: PreviewStates,
    /// The menu bar's logged geometry and its open state.
    pub menubar: MenubarSession,
    /// The Inspector's collapsed component sections and its logged geometry.
    pub inspector: InspectorSession,
    /// The toasts the host's notices put on screen.
    pub toasts: ToastSession,
    /// The command palette's open state and query.
    pub palette: PaletteState,
    /// The Console tab's filters. The tab edits them, and the executable copies
    /// them into the Console history before the next snapshot.
    pub console_filters: crate::console::ConsoleFilterSettings,
    /// The asks this pass raised for the executable: a Problems row, an undo
    /// or redo, a dock tab. `settle_pass` moves the value onto
    /// [`crate::boundary::EguiShellFrame`], and the executable answers it at
    /// its frame boundary, so the executable never reads these from the
    /// session.
    pub requests: ShellRequests,
    /// The per-tab and whole-pass timings the Statistics tab shows.
    pub stats: StatisticsSession,
    /// Every module panel's build time, and which of them overshoots the
    /// budget.
    pub panel_budget: PanelBudget,
    /// Whether this session has already mapped the saved layout's panel tabs
    /// from the ids it was written with to the slots the registry uses today.
    /// It happens once per session, not once per shell generation, so a
    /// reload never undoes a dock the user rearranged.
    pub panels_remapped: bool,
    /// The windows the module panels asked for.
    pub panel_windows: PanelWindowSession,
    /// The window a scripted `panel window <slot> <id>` line asked a panel to
    /// open: the panel's slot and the window id it should choose. The shell
    /// clears it only in the pass where that panel draws - a request must not
    /// be dropped by a frame where its tab was not on screen.
    pub pending_panel_window: Option<(u16, String)>,
    /// The run mode the last pass drew, so the shell can tell an entering or
    /// leaving Play from a frame that is merely playing.
    pub last_run_mode: Option<pill_engine::RunMode>,
    /// The tab that was in front when Play took over, restored on Stop.
    pub tab_before_play: Option<EditorTab>,
    /// One egui context per popped-out tab: a popped-out window runs its own
    /// pass, so it needs its own context while the main one runs the dock in
    /// the same frame. The map lives in the session for the same reason the
    /// main context does: a shell reload must find each window's widget state
    /// where it left it.
    pub popout_contexts: std::collections::BTreeMap<EditorTab, Context>,
}

/// One window a panel asked the shell for.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelWindowState {
    /// The window's identity within its panel.
    pub id: String,
    /// The title its bar shows.
    pub title: String,
    /// The crate that asked for it. The window closes once that crate's panel
    /// is no longer alive.
    pub source: String,
    /// The registry slot of the panel that draws it.
    pub slot: u16,
    /// How wide it opens, in logical points.
    pub width: f32,
    /// How tall it opens, in logical points.
    pub height: f32,
    /// Whether it takes the editor's input until answered.
    pub modal: bool,
}

/// The answer a modal window took: Apply or Enter accepts, Cancel or Escape
/// dismisses.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelWindowAnswer {
    /// The id of the window that was answered.
    pub window: String,
    /// The crate that owns it.
    pub source: String,
    /// Whether the answer accepted rather than dismissed.
    pub accepted: bool,
}

/// The windows the module panels asked for.
///
/// Session-scoped like the dock, so a shell reload keeps every window open. A
/// window closes when the user closes it, when its modal is answered, or when
/// the panel that owns it is no longer alive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelWindowSession {
    /// The open windows, oldest first.
    pub windows: Vec<PanelWindowState>,
    /// The answer this frame owes the modal that asked for it; the panel reads
    /// it on its next draw and the shell clears it afterwards.
    pub answer: Option<PanelWindowAnswer>,
}

impl PanelWindowSession {
    /// Open a window, replacing one with the same id. Past
    /// [`PANEL_WINDOWS_MAX`] open windows, the oldest one closes first.
    pub fn open(&mut self, source: &str, slot: u16, request: &crate::panels::PanelWindowRequest) {
        if let Some(existing) = self
            .windows
            .iter_mut()
            .find(|window| window.id == request.id)
        {
            existing.title = request.title.clone();
            existing.width = request.width;
            existing.height = request.height;
            existing.modal = request.modal;
            existing.slot = slot;
            return;
        }
        if self.windows.len() >= PANEL_WINDOWS_MAX {
            self.windows.remove(0);
        }
        self.windows.push(PanelWindowState {
            id: request.id.clone(),
            title: request.title.clone(),
            source: source.to_string(),
            slot,
            width: request.width,
            height: request.height,
            modal: request.modal,
        });
    }

    /// Close one window of one source.
    pub fn close(&mut self, source: &str, id: &str) {
        self.windows
            .retain(|window| !(window.source == source && window.id == id));
    }

    /// Close every window a source opened; returns how many went.
    pub fn close_source(&mut self, source: &str) -> usize {
        let before = self.windows.len();
        self.windows.retain(|window| window.source != source);
        before - self.windows.len()
    }

    /// The modal window that is on screen, if one is: the editor's input
    /// belongs to it.
    pub fn modal(&self) -> Option<&PanelWindowState> {
        self.windows.iter().find(|window| window.modal)
    }
}

/// One tab's measured cost, smoothed so a single slow frame does not dominate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabTiming {
    /// Which tab.
    pub tab: EditorTab,
    /// The exponentially weighted mean of one body pass, in microseconds.
    pub micros: f64,
    /// How many passes were measured.
    pub passes: u64,
}

/// The timings the Statistics tab reads.
///
/// Written by the shell's own pass, read by the tab: the measurement is the
/// shell's, so the tab cannot inflate what it reports by drawing itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatisticsSession {
    /// One entry per tab drawn since the session began, in first-drawn order.
    pub tabs: Vec<TabTiming>,
    /// The whole shell pass's smoothed time, in microseconds.
    pub shell_micros: f64,
    /// Completed shell passes.
    pub shell_passes: u64,
    /// The editor's smoothed frame rate, as the shell measured it.
    pub frames_per_second: f32,
}

impl StatisticsSession {
    /// How strongly one pass moves the mean: one twentieth, so the mean
    /// catches up with a real change in about twenty passes.
    const WEIGHT: f64 = 0.05;

    /// Fold one tab body's time into its entry.
    pub fn record(&mut self, tab: EditorTab, micros: f64) {
        match self.tabs.iter_mut().find(|entry| entry.tab == tab) {
            Some(entry) => {
                entry.micros = smooth_sample(entry.micros, micros, Self::WEIGHT);
                entry.passes += 1;
            }
            None => self.tabs.push(TabTiming {
                tab,
                micros,
                passes: 1,
            }),
        }
    }

    /// Fold one whole shell pass's time in.
    pub fn record_shell(&mut self, micros: u64) {
        self.shell_micros = smooth_sample(self.shell_micros, micros as f64, Self::WEIGHT);
        self.shell_passes += 1;
    }

    /// One tab's smoothed time, or zero when it was never drawn.
    pub fn tab_micros(&self, tab: EditorTab) -> f64 {
        self.tabs
            .iter()
            .find(|entry| entry.tab == tab)
            .map_or(0.0, |entry| entry.micros)
    }
}

/// The command palette's session state.
///
/// Owned strings, not callbacks: the palette outlives a shell reload, and a
/// reload must not leave a closure from the old image behind.
#[derive(Clone, Debug, Default)]
pub struct PaletteState {
    /// Whether the palette is on screen.
    pub open: bool,
    /// What the query field holds; the filter is a substring match on titles.
    pub query: String,
    /// The selected row's index in the filtered list.
    pub selected: usize,
    /// A run the palette asked for: its own Enter or click sets it in the pass
    /// that takes it, and the scripted channel sets it between passes so the
    /// next pass takes it - either way both paths run the same action.
    pub run_requested: bool,
    /// The query the log already reported, so one query logs one line.
    pub logged_query: Option<String>,
}

/// The floating viewport toolbox's session state. A drag moves one scene
/// view's toolbox, and the log reports each new placement once.
///
/// Session-scoped, like the dock's floating tabs' positions: a restart starts
/// every toolbox at its default corner.
pub struct ToolboxSession {
    /// One panel origin per scene view, in logical points. A view's entry
    /// stays `None` until the user drags its toolbox, so the default anchor
    /// keeps following the viewport until then; the list has a slot for every
    /// view index, opened or not.
    pub origins: Vec<Option<Pos2>>,
    /// Per-scene-view panel rectangles whose rows the log already reported,
    /// so the rect lines appear once per placement and once per drag.
    pub logged: LoggedRects<usize>,
    /// Per-scene-view row rectangles, in [`crate::toolbox::rows`] order: the
    /// face a row's button drew this pass, which is what a click aims at. A
    /// row's face is narrower than the row it sits in, so the logged row
    /// rectangle alone would not say where the button is.
    pub faces: Vec<Vec<Rect>>,
}

impl Default for ToolboxSession {
    fn default() -> Self {
        Self {
            origins: vec![None; SCENE_VIEWS_MAX],
            logged: LoggedRects::default(),
            faces: vec![Vec::new(); SCENE_VIEWS_MAX],
        }
    }
}

impl ToolboxSession {
    /// The placed origin of `view`'s toolbox.
    pub fn origin(&self, view: usize) -> Option<Pos2> {
        self.origins.get(view).copied().flatten()
    }

    /// Move `view`'s toolbox to `origin`.
    pub fn set_origin(&mut self, view: usize, origin: Pos2) {
        if self.origins.len() <= view {
            self.origins.resize(view + 1, None);
        }
        self.origins[view] = Some(origin);
    }

    /// Record `view`'s toolbox rectangle as logged, answering whether this is
    /// a placement the log has not reported yet.
    ///
    /// Pixel-exact: the panel is dragged as a whole, so a fraction of a point
    /// is a move the user made.
    pub fn remember_logged(&mut self, view: usize, rect: Rect) -> bool {
        self.logged.remember_exact(view, rect)
    }

    /// The face `view`'s row `index` drew this pass, when it drew one.
    pub fn face(&self, view: usize, index: usize) -> Option<Rect> {
        self.faces.get(view)?.get(index).copied()
    }

    /// Record the faces `view`'s rows drew this pass, in row order.
    pub fn set_faces(&mut self, view: usize, faces: Vec<Rect>) {
        if self.faces.len() <= view {
            self.faces.resize(view + 1, Vec::new());
        }
        self.faces[view] = faces;
    }
}

/// The slot a menu-bar rectangle is remembered under: the strip itself, or one
/// title by its [`crate::menubar::Menu::ALL`] index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MenubarSlot {
    /// The strip behind the titles.
    Bar,
    /// One menu title, by its index in [`crate::menubar::Menu::ALL`].
    Title(usize),
    /// The play controls, centred over the strip.
    Toolbar,
}

/// The menu bar's session state: the logged geometry of the strip, its titles
/// and each open menu's entries, which menu is open, and whether the About
/// window is open.
///
/// The log reports each placement once, as the toolbox does, so a suite can
/// click the titles. The requests the menu items raise do not live here: a
/// pass's asks for the executable are [`crate::requests::ShellRequests`], and
/// a dock edit the pass cannot make while the dock draws is the shell's
/// `FrameRequests`.
#[derive(Default)]
pub struct MenubarSession {
    /// The strip's and each title's last logged rectangle, keyed by slot: the
    /// bar reports a placement once, and a title that moves is reported again.
    pub geometry: LoggedRects<MenubarSlot>,
    /// Each open menu entry's last logged rectangle, keyed `menu/label`; a
    /// menu's entries log while it is open, so their presence is the proof it
    /// opened.
    pub entries: LoggedRects<String>,
    /// Whether the About window is open.
    pub about_open: bool,
    /// The menu whose dropdown is open, while one is; the log reports each
    /// opening once so a suite can tell an open menu from a closed one.
    pub open: Option<crate::menubar::Menu>,
}

/// One toast on screen: a short-lived line about something the host did.
#[derive(Clone, Debug)]
pub struct Toast {
    /// The notice event's id, so a frame never queues the same line twice.
    pub seq: u64,
    /// Error, warning or note, picking the stripe's colour.
    pub level: NoticeLevel,
    /// The line to show.
    pub text: String,
    /// The frame index at which it disappears.
    pub expires_at_frame: u64,
}

/// The toast stack's session state.
#[derive(Default)]
pub struct ToastSession {
    /// The live toasts, oldest first.
    pub toasts: std::collections::VecDeque<Toast>,
    /// The newest notice id already queued; `None` until the first event.
    pub last_seq: Option<u64>,
    /// The ids whose log line already went out, so a redrawn toast does not
    /// flood the log.
    pub logged: Vec<u64>,
    /// How many toasts the shell queued itself, counted up from
    /// [`ToastSession::LOCAL_SEQ_BASE`] so a local toast can never collide
    /// with a host notice's id.
    local_toast_count: u64,
}

impl ToastSession {
    /// The first id the shell's own toasts take, far above the host's notice
    /// ids so the two can never collide.
    pub const LOCAL_SEQ_BASE: u64 = u64::MAX / 2;

    /// Queue one toast the shell itself produced - a panel over its budget,
    /// rather than something the host reported.
    pub fn push_local(&mut self, level: NoticeLevel, text: String, frame_index: u64) -> u64 {
        if self.toasts.len() >= TOASTS_MAX {
            self.toasts.pop_front();
        }
        let seq = Self::LOCAL_SEQ_BASE + self.local_toast_count;
        self.local_toast_count += 1;
        self.toasts.push_back(Toast {
            seq,
            level,
            text,
            expires_at_frame: frame_index + TOAST_FRAMES,
        });
        seq
    }
}

/// One panel's build time and how long it has been over budget.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelTiming {
    /// The panel's registry slot.
    pub slot: u16,
    /// The panel body's smoothed build time, in microseconds.
    pub micros: f32,
    /// Passes the panel body was timed.
    pub passes: u64,
    /// Consecutive passes over [`PANEL_BUDGET_MICROS`].
    pub slow_frames: u32,
    /// Whether the overshoot was already reported, so one panel warns once.
    pub warned: bool,
    /// Whether the warning toast was queued, so a redrawn frame does not
    /// queue it again.
    pub toasted: bool,
}

/// Every module panel's build time.
///
/// Written by the shell's own pass; the dock's tab title and the executable's
/// panels dump read it. The measurement belongs to whoever timed the call, so
/// a panel cannot inflate its own row by drawing it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelBudget {
    /// One entry per panel drawn since the session began, in first-drawn order.
    pub entries: Vec<PanelTiming>,
}

impl PanelBudget {
    /// How strongly one pass moves the mean, the same weight the statistics
    /// use.
    pub const WEIGHT: f32 = 0.05;

    /// Record one panel body's build time; returns whether this pass is the
    /// one that reports an overshoot.
    ///
    /// The mean is smoothed, but the overshoot count is not: a panel that
    /// sits just over budget every frame is slow, and a panel that is fast
    /// again resets the count, so only a sustained overshoot warns.
    pub fn record(&mut self, slot: u16, micros: f32) -> bool {
        // Step 1: find the panel's entry, adding one on its first timed pass.
        let index = match self.entries.iter().position(|entry| entry.slot == slot) {
            Some(index) => index,
            None => {
                self.entries.push(PanelTiming {
                    slot,
                    ..PanelTiming::default()
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[index];
        // Step 2: fold this pass into the smoothed mean.
        entry.micros = if entry.passes == 0 {
            micros
        } else {
            entry.micros * (1.0 - Self::WEIGHT) + micros * Self::WEIGHT
        };
        entry.passes += 1;
        // Step 3: count a run of slow passes and report it once.
        if micros > PANEL_BUDGET_MICROS {
            entry.slow_frames = entry.slow_frames.saturating_add(1);
        } else {
            entry.slow_frames = 0;
        }
        if entry.slow_frames >= PANEL_BUDGET_FRAMES && !entry.warned {
            entry.warned = true;
            return true;
        }
        false
    }

    /// The timing of one slot, when it has been timed.
    pub fn get(&self, slot: u16) -> Option<&PanelTiming> {
        self.entries.iter().find(|entry| entry.slot == slot)
    }

    /// Whether the slot's panel has been reported as over budget. It stays
    /// true for the session, and the dock's tab title adds `(slow)` from it.
    pub fn is_slow(&self, slot: u16) -> bool {
        self.get(slot).is_some_and(|entry| entry.warned)
    }
}

/// The rectangles a pass last reported, so a placement reaches the log once
/// rather than once a frame.
///
/// One type serves every "log this rectangle once" cache: [`Self::remember`]
/// decides whether a rectangle differs from the last one, [`Self::forget_missing`]
/// drops keys that stopped drawing, and [`Self::clear`] drops everything, so
/// the caches cannot drift into separate rules.
///
/// A placement is compared **rounded to whole points** by default, because
/// egui lays a panel out in fractional points: a fraction of a point of jitter
/// is the same place, and a line per frame of it is noise. A caller that needs
/// a pixel-exact comparison says so with [`Self::remember_exact`].
#[derive(Clone, Debug)]
pub struct LoggedRects<K: Ord + Clone> {
    /// The rectangle last remembered per key.
    rects_by_key: BTreeMap<K, Rect>,
}

impl<K: Ord + Clone> Default for LoggedRects<K> {
    fn default() -> Self {
        Self {
            rects_by_key: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Clone> LoggedRects<K> {
    /// Record `rect` for `key`, answering whether it is a placement to log.
    pub fn remember(&mut self, key: K, rect: Rect) -> bool {
        let quantized = Self::quantize(rect);
        match self.rects_by_key.get(&key) {
            Some(logged) if Self::quantize(*logged) == quantized => false,
            _ => {
                self.rects_by_key.insert(key, quantized);
                true
            }
        }
    }

    /// [`Self::remember`] without the quantization: any pixel difference is a
    /// new placement.
    pub fn remember_exact(&mut self, key: K, rect: Rect) -> bool {
        match self.rects_by_key.get(&key) {
            Some(logged) if *logged == rect => false,
            _ => {
                self.rects_by_key.insert(key, rect);
                true
            }
        }
    }

    /// The rectangle last remembered for `key`.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<Rect> {
        self.rects_by_key.get(key).copied()
    }

    /// Every remembered key with its rectangle, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, Rect)> {
        self.rects_by_key.iter().map(|(key, rect)| (key, *rect))
    }

    /// Whether `key` has a logged rectangle.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.rects_by_key.contains_key(key)
    }

    /// Drop every key that `seen` does not name.
    ///
    /// A row that stopped drawing is dropped instead of keeping its last
    /// placement forever: the geometry of everything a session has ever drawn
    /// is not worth remembering, and a cache that never shrinks is a slow leak.
    pub fn forget_missing(&mut self, seen: &BTreeSet<K>) {
        self.rects_by_key.retain(|key, _| seen.contains(key));
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.rects_by_key.clear();
    }

    /// How many rectangles are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rects_by_key.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rects_by_key.is_empty()
    }

    /// Whether two rectangles are the same placement under [`Self::remember`]'s
    /// rule, for the callers that keep a single rectangle rather than a cache.
    #[must_use]
    pub fn same_placement(left: Rect, right: Rect) -> bool {
        Self::quantize(left) == Self::quantize(right)
    }

    /// `rect` with both corners rounded to whole points.
    fn quantize(rect: Rect) -> Rect {
        Rect::from_min_max(
            Pos2::new(rect.min.x.round(), rect.min.y.round()),
            Pos2::new(rect.max.x.round(), rect.max.y.round()),
        )
    }
}

/// The texts a pass last reported per key, so a row logs a changed value once
/// rather than once a frame.
#[derive(Clone, Debug)]
pub struct LoggedTexts<K: Ord + Clone> {
    /// The text last remembered per key.
    texts_by_key: BTreeMap<K, String>,
}

impl<K: Ord + Clone> Default for LoggedTexts<K> {
    fn default() -> Self {
        Self {
            texts_by_key: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Clone> LoggedTexts<K> {
    /// Record `text` for `key`, answering whether it changed since the last
    /// draw that reported it.
    pub fn remember(&mut self, key: K, text: &str) -> bool {
        match self.texts_by_key.get(&key) {
            Some(logged) if logged == text => false,
            _ => {
                self.texts_by_key.insert(key, text.to_owned());
                true
            }
        }
    }

    /// The text last logged for `key`.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<&str> {
        self.texts_by_key.get(key).map(String::as_str)
    }

    /// Every remembered key with its text, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &str)> {
        self.texts_by_key
            .iter()
            .map(|(key, text)| (key, text.as_str()))
    }

    /// Drop every key `seen` does not name; see [`LoggedRects::forget_missing`].
    pub fn forget_missing(&mut self, seen: &BTreeSet<K>) {
        self.texts_by_key.retain(|key, _| seen.contains(key));
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.texts_by_key.clear();
    }

    /// How many texts are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.texts_by_key.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.texts_by_key.is_empty()
    }
}

/// The Inspector's session state: which component sections are collapsed, and
/// the geometry its log already reported.
///
/// Session-scoped like the toolbox: a shell reload keeps it, a restart starts
/// every section expanded. The collapse state is keyed by the registered type
/// name, so selecting another entity keeps a type's foldout where the user
/// left it (Unity's behaviour).
#[derive(Default)]
pub struct InspectorSession {
    /// The component type names whose sections are collapsed.
    pub collapsed: std::collections::BTreeSet<String>,
    /// Each section header's last logged rectangle, keyed by type name.
    pub section_rects: LoggedRects<String>,
    /// The Add Component button's last logged rectangle.
    pub add_rect: Option<Rect>,
    /// Whether the Add Component popup is open.
    pub add_open: bool,
    /// The addable entries' last logged rectangles, keyed by type name.
    pub add_entries: LoggedRects<String>,
    /// The row model each drawn component's section shows, keyed by its
    /// registered type name: the rows, their once-per-subject log memory and
    /// the selection's write targets.
    pub row_models: std::collections::BTreeMap<String, crate::inspector::CachedRows>,
    /// The one-shot notices the Inspector already logged, keyed by what they
    /// describe (the selected asset's key): the tab redraws every frame.
    pub notices_logged: std::collections::BTreeSet<String>,
    /// The entity whose rows were last reported. A different subject clears
    /// the row memories below, because the rows on screen are that entity's.
    pub subject: Option<pill_engine::Entity>,
    /// Each asset reference row's last logged rectangle, keyed by
    /// `entity.component.field`: what a suite drops an asset onto.
    /// Only a row on screen keeps an entry; see the shell's
    /// `remember_asset_row_rect`.
    pub asset_row_rects: LoggedRects<String>,
    /// Whether the row and section rectangles reach the log.
    ///
    /// The shell sets it from the frame's `scripted` flag
    /// ([`crate::boundary::EguiShellInput`]): a scripted run reads these
    /// rectangles to aim a click or a drop, and a person's editor does not
    /// need a line per placement.
    pub log_rects: bool,
    /// Each asset reference row's last logged text, keyed the same way: the
    /// row logs again whenever it draws a different asset.
    pub asset_row_texts: LoggedTexts<String>,
    /// The `component.field` whose asset picker is open, if one is.
    pub asset_picker: Option<String>,
    /// The open asset picker's search text.
    pub asset_search: String,
    /// Each entity reference row's last logged rectangle, keyed by
    /// `entity.component.field`: what a suite drags an
    /// entity onto.
    pub entity_row_rects: LoggedRects<String>,
    /// Each entity reference row's last logged text, keyed the same way: the
    /// row logs again whenever it draws a different entity.
    pub entity_row_texts: LoggedTexts<String>,
    /// The `component.field` whose entity picker is open, if one is.
    pub entity_picker: Option<String>,
    /// The open entity picker's search text.
    pub entity_search: String,
    /// The settings-document key (`textures.albedo`) whose picker is open, if
    /// one is.
    pub settings_picker: Option<String>,
    /// The open settings picker's search text.
    pub settings_search: String,
    /// Each reference-list row's last logged rectangle, keyed by
    /// `asset:family[index]`: what a suite drags to reorder a list.
    pub settings_row_rects: LoggedRects<String>,
    /// Each Hierarchy row's last logged rectangle, keyed by the entity's
    /// display: what a suite drags onto an entity reference
    /// row. The Entities tab shares this session, since the cache is the same
    /// "geometry the log already reported" concern.
    pub hierarchy_row_rects: LoggedRects<String>,
    /// Each scene badge's last logged rectangle, keyed by `view:entity`:
    /// what a suite clicks to select through a badge.
    pub badge_rects: LoggedRects<String>,
    /// The Hierarchy rows this pass drew, so [`Self::begin_geometry_pass`] can
    /// drop the ones that left: the tab redraws what it shows, and the
    /// Entities list can be tens of thousands of rows long.
    pub hierarchy_seen: BTreeSet<String>,
    /// The scene badges this pass drew, keyed like [`Self::badge_rects`].
    pub badge_seen: BTreeSet<String>,
}

impl InspectorSession {
    /// Forget every rectangle and text the log already reported.
    ///
    /// Called when the subject changes: the rows on screen are another
    /// entity's, so their geometry has to be reported again for the new one.
    pub fn clear_geometry(&mut self) {
        self.section_rects.clear();
        self.add_rect = None;
        self.add_entries.clear();
        self.asset_row_rects.clear();
        self.asset_row_texts.clear();
        self.entity_row_rects.clear();
        self.entity_row_texts.clear();
        self.settings_row_rects.clear();
        self.hierarchy_row_rects.clear();
        self.badge_rects.clear();
    }

    /// Start a frame of geometry reporting: drop the rows and badges the
    /// previous frame did not draw, then collect this frame's.
    ///
    /// The seen sets collect every pass of a frame, the main dock pass and the
    /// popped-out tabs' passes, so a tab drawn in its own window still counts
    /// as drawn. Without the prune the two caches would grow with every entity
    /// the session ever listed and never shrink; a hidden row's rectangle is
    /// not one a suite can click, so forgetting it costs nothing - it logs
    /// again when the row comes back.
    pub fn begin_geometry_pass(&mut self) {
        let hierarchy_seen = std::mem::take(&mut self.hierarchy_seen);
        self.hierarchy_row_rects.forget_missing(&hierarchy_seen);
        let badge_seen = std::mem::take(&mut self.badge_seen);
        self.badge_rects.forget_missing(&badge_seen);
    }
}

/// The preview state of every asset tile, keyed by the tile's row key.
///
/// One value holds the three things the shell and the executable have to agree
/// on: which tiles have a texture, which were answered with no pixels, and
/// which this pass drew. The rules that keep them consistent live here: a key
/// is never both ready and refused ([`PreviewStates::mark_ready`] clears the
/// refusal, [`PreviewStates::mark_refused`] drops the texture), and only a tile
/// this pass drew is asked for ([`PreviewStates::wants`]).
#[derive(Default)]
pub struct PreviewStates {
    /// The textures ready to draw, by key.
    ready_textures: BTreeMap<String, TextureHandle>,
    /// The keys the executable answered without pixels.
    refused_keys: BTreeSet<String>,
    /// The keys the grid drew this pass, in draw order.
    visible_keys: Vec<String>,
}

impl PreviewStates {
    /// Start a pass: the visible list is whatever this pass draws.
    pub fn begin_pass(&mut self) {
        self.visible_keys.clear();
    }

    /// Record one tile the grid drew, in draw order.
    pub fn push_visible(&mut self, key: &str) {
        self.visible_keys.push(key.to_string());
    }

    /// The keys this pass drew, in draw order.
    #[must_use]
    pub fn visible(&self) -> &[String] {
        &self.visible_keys
    }

    /// The rendered texture for `key`, when there is one.
    #[must_use]
    pub fn ready(&self, key: &str) -> Option<&TextureHandle> {
        self.ready_textures.get(key)
    }

    /// Whether `key` may be requested now: drawn this pass, not ready, not
    /// refused.
    #[must_use]
    pub fn wants(&self, key: &str) -> bool {
        self.visible_keys.iter().any(|visible| visible == key)
            && !self.ready_textures.contains_key(key)
            && !self.refused_keys.contains(key)
    }

    /// Record a rendered texture for `key`.
    pub fn mark_ready(&mut self, key: &str, texture: TextureHandle) {
        self.refused_keys.remove(key);
        self.ready_textures.insert(key.to_string(), texture);
    }

    /// Record that `key` was answered without pixels: an undecodable file or a
    /// frame that drew nothing. The tile keeps its type icon, and the grid asks
    /// for the next one instead of waiting on this one forever.
    pub fn mark_refused(&mut self, key: &str) {
        self.ready_textures.remove(key);
        self.refused_keys.insert(key.to_string());
    }

    /// Keep only the previews whose key is in `kept`; the listing passes the
    /// rows that did not change, so only their textures survive a refresh.
    pub fn retain_keys(&mut self, kept: &BTreeSet<String>) {
        self.ready_textures.retain(|key, _| kept.contains(key));
        self.refused_keys.retain(|key| kept.contains(key));
    }

    /// Forget the refusals, so the next pass asks for those tiles again.
    pub fn clear_refused(&mut self) {
        self.refused_keys.clear();
    }

    /// Forget every preview.
    pub fn clear(&mut self) {
        self.ready_textures.clear();
        self.refused_keys.clear();
    }
}

/// What one generation's claim on the session's egui contexts cleared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextClaim {
    /// The entries the main context's `Memory::data` held.
    pub main_entries: usize,
    /// How many popped-out windows' contexts held anything.
    pub popout_windows: usize,
}

impl EguiShellState {
    /// Clear every egui context this session owns and report what was cleared.
    ///
    /// The session owns the main context and one context per popped-out tab,
    /// so a third kind of context joins this method instead of gaining a flag
    /// of its own in the shell. A shell generation calls it on its first
    /// frame, before its pass: a value the previous generation left in
    /// `Memory::data` carries that image's drop code, and touching it after
    /// the reload is a use-after-unmap.
    pub fn claim_generation(&mut self) -> ContextClaim {
        // Step 1: the main context.
        let main_entries = self.context.memory_mut(|memory| {
            let entries = memory.data.len();
            memory.data.clear();
            entries
        });
        // Step 2: every popped-out window's context.
        let mut popout_windows = 0;
        for context in self.popout_contexts.values() {
            let entries = context.memory_mut(|memory| {
                let entries = memory.data.len();
                memory.data.clear();
                entries
            });
            if entries > 0 {
                popout_windows += 1;
            }
        }
        ContextClaim {
            main_entries,
            popout_windows,
        }
    }

    /// Session state with the editor's theme and the saved dock.
    ///
    /// A document this build cannot use falls back to the default layout and
    /// logs why (`[editor] layout reset`); a missing one is the first run.
    pub fn new() -> Self {
        let context = Context::default();
        crate::style::apply(&context);
        Self {
            context,
            dock: crate::layout::load_or_default(),
            frame_index: 0,
            // The record already holds whatever the executable resolved, and
            // the install above put it in this context: the shell is current
            // until the next generation arrives.
            applied_theme_generation: crate::style::generation(),
            asset_dialog: None,
            preferences_draft: None,
            toolbox: ToolboxSession::default(),
            browser: ProjectBrowser::default(),
            mark: None,
            previews: PreviewStates::default(),
            menubar: MenubarSession::default(),
            inspector: InspectorSession::default(),
            toasts: ToastSession::default(),
            palette: PaletteState::default(),
            console_filters: crate::console::ConsoleFilterSettings::default(),
            requests: ShellRequests::default(),
            stats: StatisticsSession::default(),
            panel_budget: PanelBudget::default(),
            panels_remapped: false,
            panel_windows: PanelWindowSession::default(),
            pending_panel_window: None,
            last_run_mode: None,
            tab_before_play: None,
            popout_contexts: std::collections::BTreeMap::new(),
        }
    }

    /// The brand mark's texture, uploaded on first use and cloned from then on.
    pub fn mark_texture(&mut self) -> TextureHandle {
        if let Some(mark) = &self.mark {
            return mark.clone();
        }
        let texture = self.context.load_texture(
            "pill_editor.mark",
            crate::brand::mark_color_image(),
            TextureOptions::LINEAR,
        );
        self.mark = Some(texture.clone());
        texture
    }
}

impl Resource for EguiShellState {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

impl Default for EguiShellState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// One step of an exponential moving average, seeded by the first sample.
fn smooth_sample(current_mean: f64, sample_value: f64, weight: f64) -> f64 {
    if current_mean <= 0.0 {
        sample_value
    } else {
        current_mean * (1.0 - weight) + sample_value * weight
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A rectangle that repeats logs once, a rectangle that moves logs again,
    /// and a key that stops drawing is dropped, so the cache does not grow with
    /// everything the session ever drew.
    #[test]
    fn a_logged_rect_answers_once_per_placement() {
        let mut logged = LoggedRects::default();
        let rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(110.0, 40.0));
        assert!(
            logged.remember("row".to_owned(), rect),
            "the first draw logs"
        );
        assert!(
            !logged.remember("row".to_owned(), rect),
            "the same placement does not log again"
        );
        let jittered = rect.translate(egui::vec2(0.2, -0.3));
        assert!(
            !logged.remember("row".to_owned(), jittered),
            "a fraction of a point is the same placement"
        );
        let moved = rect.translate(egui::vec2(0.0, 12.0));
        assert!(logged.remember("row".to_owned(), moved), "a move logs");

        let seen = BTreeSet::from(["other".to_owned()]);
        logged.forget_missing(&seen);
        assert!(logged.is_empty(), "a key that stopped drawing is dropped");
    }

    /// The exact comparison is the caller's choice: half a point is a new
    /// placement, and only [`LoggedRects::remember_exact`] treats it as one.
    #[test]
    fn the_exact_rule_takes_every_pixel() {
        let mut logged = LoggedRects::default();
        let rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(110.0, 40.0));
        assert!(logged.remember_exact("header".to_owned(), rect));
        assert!(!logged.remember_exact("header".to_owned(), rect));
        assert!(logged.remember_exact("header".to_owned(), rect.translate(egui::vec2(0.5, 0.0))));
    }

    /// A row logs the text it draws once per change, not once per frame.
    #[test]
    fn a_logged_text_answers_only_on_change() {
        let mut logged = LoggedTexts::default();
        assert!(logged.remember("cfg:speed".to_owned(), "1.5"));
        assert!(!logged.remember("cfg:speed".to_owned(), "1.5"));
        assert!(logged.remember("cfg:speed".to_owned(), "2.0"));
        assert_eq!(logged.get(&"cfg:speed".to_owned()), Some("2.0"));
        let seen = BTreeSet::new();
        logged.forget_missing(&seen);
        assert!(logged.is_empty());
    }
}
