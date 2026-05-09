use log::LevelFilter;

use crate::PillStyle;

#[cfg(not(target_arch = "wasm32"))]
use {
    std::collections::HashMap,
    std::io::Write,
    strum_macros::{AsRefStr, EnumString},
};

/// Contexts for logging
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Eq, PartialEq, Hash, AsRefStr, EnumString, Clone)]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum LogContext {
    HotReload,
    Engine,
    Input,
    ECS,
    Rendering,
    Resources,
    Frame,
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub enum LogContext {
    HotReload,
    Engine,
    Input,
    ECS,
    Rendering,
    Resources,
    Frame,
}

#[cfg(target_arch = "wasm32")]
impl AsRef<str> for LogContext {
    fn as_ref(&self) -> &str {
        match self {
            LogContext::HotReload => "hot_reload",
            LogContext::Engine => "engine",
            LogContext::Input => "input",
            LogContext::ECS => "ecs",
            LogContext::Rendering => "rendering",
            LogContext::Resources => "resources",
            LogContext::Frame => "frame",
        }
    }
}

pub fn get_default_log_levels() -> String {
    [
        format!("{}: info", LogContext::HotReload.as_ref()),
        format!("{}: info", LogContext::Engine.as_ref()),
        format!("{}: info", LogContext::Input.as_ref()),
        format!("{}: info", LogContext::ECS.as_ref()),
        format!("{}: info", LogContext::Rendering.as_ref()),
        format!("{}: info", LogContext::Resources.as_ref()),
        format!("{}: info", LogContext::Frame.as_ref()),
    ]
    .join(", ")
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_log_level(level: &str) -> LevelFilter {
    match level.to_lowercase().as_str() {
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "warning" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "off" => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_config_log_settings(log_levels_config_setting: &str) -> HashMap<String, LevelFilter> {
    let mut map = HashMap::new();
    for context_log_level in log_levels_config_setting.split(',') {
        let context_log_level = context_log_level.trim();
        if context_log_level.is_empty() {
            continue;
        }
        let (context, level) = context_log_level
            .split_once('=')
            .or_else(|| context_log_level.split_once(':'))
            .unwrap_or((context_log_level, "info"));
        map.insert(context.trim().to_string(), parse_log_level(level.trim()));
    }
    map
}

#[cfg(not(target_arch = "wasm32"))]
pub fn set_log_levels(log_levels_config_setting: &str, show_date: bool) {
    let context_levels = parse_config_log_settings(log_levels_config_setting);

    fn styled_level(level: log::Level) -> String {
        use colored::Colorize;
        match level {
            log::Level::Info => "INFO".white().to_string(),
            log::Level::Debug => "DEBUG".blue().bold().to_string(),
            log::Level::Warn => "WARN".yellow().bold().to_string(),
            log::Level::Error => "ERROR".red().bold().to_string(),
            _ => level.to_string(),
        }
    }

    let mut builder = env_logger::Builder::new();

    builder.format(move |buf, record| {
        writeln!(
            buf,
            "[{}][{}] {} {}:{}: {}",
            styled_level(record.level()),
            record.target(),
            chrono::Local::now().format(if show_date {
                "%Y-%m-%dT%H:%M:%S"
            } else {
                "%H:%M:%S"
            }),
            record.file().unwrap_or("unknown"),
            record.line().unwrap_or(0),
            record.args()
        )
    });

    builder.filter_module("default", LevelFilter::Info);

    for (context, level) in &context_levels {
        builder.filter_module(context.as_ref(), *level);
    }

    builder.init();
}

#[cfg(target_arch = "wasm32")]
pub fn set_log_levels(_log_levels_config_setting: &str, _show_date: bool) {}

/// Log at a specific level with an explicit module target.
#[macro_export]
macro_rules! log_context {
    ($level:ident, $ctx:expr, $($arg:tt)+) => {
        log::$level!(target: $ctx.as_ref(), $($arg)+)
    };
}

#[macro_export]
macro_rules! info {
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(info, $ctx, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log_context!(debug, "default", $($arg)+)
    };
}

#[macro_export]
macro_rules! debug {
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(debug, $ctx, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log_context!(debug, "default", $($arg)+)
    };
}

#[macro_export]
macro_rules! warn {
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(warn, $ctx, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log_context!(warn, "default", $($arg)+)
    };
}

#[macro_export]
macro_rules! error {
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(error, $ctx, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log_context!(error, "default", $($arg)+)
    };
}
