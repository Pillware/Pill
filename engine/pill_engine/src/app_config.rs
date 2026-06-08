use std::collections::HashMap;

use pill_core::Result;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileMode {
    Debug,
    Release,
    HotReload,
}

impl CompileMode {
    pub(crate) fn from_env_value(value: &str) -> Result<Self> {
        match value {
            "debug" => Ok(Self::Debug),
            "release" => Ok(Self::Release),
            "hot-reload" => Ok(Self::HotReload),
            other => Err(format!("Invalid compile mode: {other}").into()),
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            CompileMode::Debug => "debug",
            CompileMode::Release => "release",
            CompileMode::HotReload => "hot-reload",
        }
    }
}

#[derive(Clone)]
pub enum BuildTarget {
    Native,
    Web,
}

impl BuildTarget {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            BuildTarget::Web => "web",
            BuildTarget::Native => "native",
        }
    }
}

#[derive(Default, Clone)]
pub struct EngineConfig {
    values: HashMap<String, String>,
}

#[derive(Clone)]
pub struct EngineProcessInfo {
    pub(crate) mode: CompileMode,
    pub(crate) target: BuildTarget,
}

impl EngineConfig {
    pub fn from_ini(input: &str) -> Self {
        let mut values = HashMap::new();
        for line in input.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with(';')
                || line.starts_with('#')
                || line.starts_with('[')
            {
                continue;
            }
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().to_uppercase();
                let value = line[eq + 1..].trim().to_string();
                values.insert(key, value);
            }
        }
        Self { values }
    }

    pub fn set(&mut self, key: &str, value: i64) {
        self.values.insert(key.to_uppercase(), value.to_string());
    }

    pub fn get_int(&self, key: &str) -> Result<i64> {
        use pill_core::PillError;
        self.values
            .get(&key.to_uppercase())
            .ok_or_else(|| -> PillError { format!("{key} not found in config").into() })?
            .parse::<i64>()
            .map_err(|e| -> PillError {
                format!("Config key {key} is not a valid integer: {e}").into()
            })
    }

    pub fn get_bool(&self, key: &str) -> Result<bool> {
        use pill_core::PillError;
        let v = self
            .values
            .get(&key.to_uppercase())
            .ok_or_else(|| -> PillError { format!("{key} not found in config").into() })?;
        match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(format!("Config key {key} is not a valid bool: {v}").into()),
        }
    }
}

impl EngineProcessInfo {
    pub fn new(mode: &str, target: BuildTarget) -> Self {
        let translated = CompileMode::from_env_value(mode).unwrap();
        Self {
            target,
            mode: translated,
        }
    }
}
