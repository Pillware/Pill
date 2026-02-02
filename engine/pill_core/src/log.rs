use colored::Colorize;
use config::Config;
use log::LevelFilter;
use std::io::Write;
use std::str::FromStr;
use std::{collections::HashMap, fmt::Debug};
use strum_macros::{AsRefStr, EnumString};

use crate::PillStyle;

/// Contexts for logging
//#[derive(Debug, Eq, PartialEq, Hash)]
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

/// Convert string to LevelFilter
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

/// Parse a string like "ecs: debug, renderer: info" or "ecs=debug, renderer=info"
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

/// Initialize logger with per-context levels from Config
pub fn set_log_levels(log_levels_config_setting: &str, show_date: bool) {
    let context_levels = parse_config_log_settings(log_levels_config_setting);

    fn styled_level(level: log::Level) -> colored::ColoredString {
        match level {
            log::Level::Info => "INFO".white(),
            log::Level::Debug => "DEBUG".debug_style(),
            log::Level::Warn => "WARN".warn_style(),
            log::Level::Error => "ERROR".error_style(),
            _ => level.to_string().white(),
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

    // Allow non-contextual logging
    //builder.filter_level(LevelFilter::Info);
    //builder.filter_module("wgpu_hal::vulkan::instance", LevelFilter::Info);

    // Allow default context logging
    // TODO: Does it work??
    builder.filter_module("default", LevelFilter::Info);

    // Apply per-module filters
    for (context, level) in &context_levels {
        builder.filter_module(context.as_ref(), *level);
    }

    builder.init();
}

/// Context-aware logging macros
/// Log an error message with context. The first argument must be a LogContext, and
/// the format string must be a string literal, like `"error: {}"` not a variable.
#[macro_export]
macro_rules! log_context {
    ($level:ident, $ctx:expr, $($arg:tt)+) => {
        log::$level!(target: $ctx.as_ref(), $($arg)+)
    };
}

#[macro_export]
macro_rules! info {
    // Contextual logging: require `ctx =>` form
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(info, $ctx, $($arg)+)
    };

    // Fallback to standard logging
    ($($arg:tt)+) => {
        $crate::log_context!(debug, "default", $($arg)+)
    };
}

#[macro_export]
macro_rules! debug {
    // Contextual logging: require `ctx =>` form
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(debug, $ctx, $($arg)+)
    };

    // Fallback to standard logging
    ($($arg:tt)+) => {
        $crate::log_context!(debug, "default", $($arg)+)
    };
}

#[macro_export]
macro_rules! warn {
    // Contextual logging: require `ctx =>` form
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(warn, $ctx, $($arg)+)
    };

    // Fallback to standard logging
    ($($arg:tt)+) => {
        $crate::log_context!(warn, "default", $($arg)+)
    };
}

#[macro_export]
macro_rules! error {
    // Contextual logging: require `ctx =>` form
    ($ctx:expr => $($arg:tt)+) => {
        $crate::log_context!(error, $ctx, $($arg)+)
    };

    // Fallback to standard logging
    ($($arg:tt)+) => {
        $crate::log_context!(error, "default", $($arg)+)
    };
}
