//! The Console tab's history and the filters that narrow it.
//!
//! # Responsibilities
//!
//! - Keep the lines the executable captures: owned strings, oldest first,
//!   capped at [`CONSOLE_LINES_MAX`].
//! - Sort every line into the engine channel or the game channel, so a
//!   project's own messages can be read apart from the engine's.
//! - Define the filter state (lowest level shown, search text, channel) that
//!   the shell's tab edits and the scripted channel also sets.
//!
//! # Design
//!
//! A world resource, filled by the executable each frame from the telemetry
//! capture ring and read by the shell's tab. Every string is owned: the
//! capture ring lives in `pill_core`, but the tab outlives any module image
//! that happened to log a line, and a borrowed target would dangle across a
//! reload.

// Standard library
use std::collections::VecDeque;

// Current crate
use crate::notices::NoticeLevel;

// =============================================================================
// Constants
// =============================================================================

/// How many lines the history keeps; older ones drop off.
pub const CONSOLE_LINES_MAX: usize = 2000;

/// The prefixes the engine's own code logs under.
///
/// The engine's crates are all named `pill_*`, and its tracing roots are
/// `engine::…` and `profile::…`. A target is split at its first `::`, and the
/// first part is matched against these prefixes. So
/// `pill_master_renderer_data::scene` and `pill_engine::asset::AssetManager`
/// are the engine's, while a project's `circus_demo::game` is the game's.
///
/// The rule has one edge case: a project crate whose name starts with `pill_`
/// is filed as the engine's, exactly as a crate named `engine_tools` is filed
/// by the `engine` prefix. That is a name collision, not a classification
/// error, and keeping the prefixes in one list makes it easy to see.
pub const ENGINE_PREFIXES: [&str; 3] = ["engine", "pill_", "profile"];

/// How many rows a [`ConsoleView`] carries: the newest rows the filters leave.
pub const CONSOLE_VIEW_ROWS: usize = 200;

// =============================================================================
// Types
// =============================================================================

/// Which side of the running program a line came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConsoleChannel {
    /// The engine, the host and the editor itself.
    Engine,
    /// The project: its modules and its scripts.
    #[default]
    Game,
}

impl ConsoleChannel {
    /// The channel's name, as the tab's status row and the `[test] console`
    /// log line print it.
    pub fn name(self) -> &'static str {
        match self {
            ConsoleChannel::Engine => "engine",
            ConsoleChannel::Game => "game",
        }
    }

    /// The channel a target belongs to.
    ///
    /// The engine, the host, the editor and the renderers log under a fixed
    /// set of crate prefixes ([`ENGINE_PREFIXES`]). Every other crate is the
    /// project's, whatever its name happens to contain. The check is a prefix
    /// match on the crate name, not a substring search, so a project crate
    /// called `my_pill_game` stays in the game channel.
    pub fn of_target(target: &str) -> Self {
        let first_segment = target.split("::").next().unwrap_or(target);
        let engine_owned = ENGINE_PREFIXES
            .iter()
            .any(|prefix| first_segment.starts_with(prefix));
        if engine_owned {
            ConsoleChannel::Engine
        } else {
            ConsoleChannel::Game
        }
    }
}

/// One console row, with owned strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleLine {
    /// The capture ring's sequence number. The feed asks for the lines newer
    /// than the last one it folded in.
    pub seq: u64,
    /// How severe the line is: 1 trace through 5 error.
    pub level: u8,
    /// The tracing target the line came from.
    pub target: String,
    /// The formatted message.
    pub message: String,
    /// Which channel the target put it in.
    pub channel: ConsoleChannel,
}

impl ConsoleLine {
    /// The level's short name, as the tab and the log print it (`info`,
    /// `warn`, ...).
    pub fn level_name(&self) -> &'static str {
        level_name(self.level)
    }
}

/// The console's history: the captured lines, the filters that narrow them,
/// and the feed's read position.
#[derive(Clone, Debug)]
pub struct ConsoleState {
    /// The kept lines, oldest first, capped at [`CONSOLE_LINES_MAX`].
    pub lines: VecDeque<ConsoleLine>,
    /// The newest capture sequence already folded in.
    pub last_seq: u64,
    /// The lowest level shown: 3 hides trace and debug.
    pub minimum_level: u8,
    /// A case-insensitive substring the message or target must contain.
    pub search: String,
    /// Which channel to show; `None` shows both.
    pub channel: Option<ConsoleChannel>,
}

impl ConsoleState {
    /// Adds one captured line, dropping the oldest once the history is full.
    pub fn push(&mut self, line: ConsoleLine) {
        self.last_seq = self.last_seq.max(line.seq);
        self.lines.push_back(line);
        while self.lines.len() > CONSOLE_LINES_MAX {
            self.lines.pop_front();
        }
    }

    /// The lines the filters leave, oldest first.
    pub fn visible(&self) -> impl Iterator<Item = &ConsoleLine> {
        let search_lowercase = self.search.trim().to_lowercase();
        self.lines.iter().filter(move |line| {
            line.level >= self.minimum_level
                && self.channel.is_none_or(|channel| line.channel == channel)
                && (search_lowercase.is_empty()
                    || line.message.to_lowercase().contains(&search_lowercase)
                    || line.target.to_lowercase().contains(&search_lowercase))
        })
    }

    /// How many lines the filters leave.
    pub fn visible_count(&self) -> usize {
        self.visible().count()
    }

    /// Applies the filters the shell edited, so the next read of the history
    /// uses them.
    pub fn apply(&mut self, filter_settings: &ConsoleFilterSettings) {
        self.minimum_level = filter_settings.minimum_level;
        self.search = filter_settings.search.clone();
        self.channel = filter_settings.channel;
    }

    /// The filters in force, so the executable can compare them with the
    /// session's copy.
    pub fn settings(&self) -> ConsoleFilterSettings {
        ConsoleFilterSettings {
            minimum_level: self.minimum_level,
            search: self.search.clone(),
            channel: self.channel,
        }
    }

    /// Builds the tab's view: the filters in force and the newest rows they
    /// leave.
    pub fn view(&self) -> ConsoleView {
        let visible_lines: Vec<&ConsoleLine> = self.visible().collect();
        let first_shown_index = visible_lines.len().saturating_sub(CONSOLE_VIEW_ROWS);
        ConsoleView {
            minimum_level: self.minimum_level,
            search: self.search.clone(),
            channel: self.channel,
            total: self.lines.len(),
            visible: visible_lines.len(),
            lines: visible_lines[first_shown_index..]
                .iter()
                .map(|line| (*line).clone())
                .collect(),
        }
    }

    /// The notice level a capture level maps to: warning for 4, error for 5,
    /// and a note for anything else.
    pub fn notice_level(level: u8) -> NoticeLevel {
        match level {
            4 => NoticeLevel::Warning,
            5 => NoticeLevel::Error,
            _ => NoticeLevel::Note,
        }
    }
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            last_seq: 0,
            minimum_level: 1,
            search: String::new(),
            channel: None,
        }
    }
}

impl pill_engine::Resource for ConsoleState {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// What the Console tab draws: the filters in force and the rows they leave.
///
/// A copy taken for the frame's snapshot, like the menu bar's command rows.
/// The shell's tab reads the snapshot and never the world.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsoleView {
    /// The lowest level shown.
    pub minimum_level: u8,
    /// The search text in force.
    pub search: String,
    /// The channel in force; `None` shows both.
    pub channel: Option<ConsoleChannel>,
    /// How many lines the history holds.
    pub total: usize,
    /// How many lines the filters leave.
    pub visible: usize,
    /// The newest visible rows, oldest first.
    pub lines: Vec<ConsoleLine>,
}

/// The filters the Console tab edits.
///
/// They live in the shell's session, not in [`ConsoleState`]. The tab draws
/// from the frame's snapshot and cannot reach the world, so it edits this
/// copy. The executable applies the copy to the history on the next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleFilterSettings {
    /// The lowest level shown.
    pub minimum_level: u8,
    /// The search text.
    pub search: String,
    /// The channel, or `None` for both.
    pub channel: Option<ConsoleChannel>,
}

impl Default for ConsoleFilterSettings {
    fn default() -> Self {
        Self {
            minimum_level: 1,
            search: String::new(),
            channel: None,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The short name of a capture level: `trace`, `debug`, `info`, `warn` or
/// `error`.
pub fn level_name(level: u8) -> &'static str {
    match level {
        0..=1 => "trace",
        2 => "debug",
        3 => "info",
        4 => "warn",
        _ => "error",
    }
}

/// The capture level a name stands for, or `None` when the name is not one of
/// them. The scripted `console filter` verb goes through this.
pub fn level_from_name(requested_name: &str) -> Option<u8> {
    match requested_name {
        "trace" => Some(1),
        "debug" => Some(2),
        "info" => Some(3),
        "warn" => Some(4),
        "error" => Some(5),
        _ => None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One console row at `level` from `target`.
    fn console_line(sequence: u64, level: u8, target: &str, message: &str) -> ConsoleLine {
        ConsoleLine {
            seq: sequence,
            level,
            target: target.to_string(),
            message: message.to_string(),
            channel: ConsoleChannel::of_target(target),
        }
    }

    #[test]
    fn engine_targets_and_project_targets_are_filed_in_their_channels() {
        // These targets all start with one of the prefixes: they name the
        // engine, the host, the editor, a renderer or the profiler.
        for target in [
            "engine::ecs",
            "engine::rendering",
            "pill_core::platform",
            "pill_engine::asset::AssetManager",
            "pill_development_host::loader",
            "pill_editor_core::menu",
            "pill_editor::runner",
            "pill_master_renderer_data::scene",
            "pill_renderer_api::limits",
            "profile::coarse",
        ] {
            assert_eq!(
                ConsoleChannel::of_target(target),
                ConsoleChannel::Engine,
                "{target} is one of the engine's own crates"
            );
        }
        // ...and any other crate is the game's, whatever its name contains.
        // None of these starts with a prefix, so each belongs to the game,
        // even where the name mentions an engine-side word
        // (`master_renderer_test`, `acme_editor`).
        for target in [
            "italian_brainrot::game",
            "project_rs::systems",
            "master_renderer_test::controls",
            "render_demo::controls",
            "acme_editor::panel",
            "head_circus::tick",
        ] {
            assert_eq!(
                ConsoleChannel::of_target(target),
                ConsoleChannel::Game,
                "{target} is a project's crate"
            );
        }
    }

    #[test]
    fn the_history_stays_bounded() {
        let mut console_state = ConsoleState::default();
        for sequence in 1..=(CONSOLE_LINES_MAX as u64 + 10) {
            console_state.push(console_line(sequence, 3, "engine::ecs", "tick"));
        }
        assert_eq!(console_state.lines.len(), CONSOLE_LINES_MAX);
        assert_eq!(console_state.last_seq, CONSOLE_LINES_MAX as u64 + 10);
        // Ten lines went past the cap, so the first row kept is sequence 11.
        assert_eq!(console_state.lines.front().map(|row| row.seq), Some(11));
    }

    #[test]
    fn the_filters_narrow_by_level_search_and_channel() {
        let mut console_state = ConsoleState::default();
        console_state.push(console_line(1, 3, "engine::ecs", "reload finished"));
        console_state.push(console_line(2, 4, "engine::ecs", "shader warning"));
        console_state.push(console_line(3, 3, "project_rs::game", "spawned a cube"));
        assert_eq!(console_state.visible_count(), 3);

        console_state.minimum_level = 4;
        assert_eq!(console_state.visible_count(), 1);

        console_state.minimum_level = 1;
        console_state.search = "CUBE".to_string();
        assert_eq!(console_state.visible_count(), 1);

        console_state.search.clear();
        console_state.channel = Some(ConsoleChannel::Engine);
        assert_eq!(console_state.visible_count(), 2);
    }
}
