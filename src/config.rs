use std::path::PathBuf;

use serde::Deserialize;

/// User configuration. Every field has a default so the file is optional.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Directory names to skip at any depth.
    pub exclude: Vec<String>,
    /// Absolute paths never treated as project roots. Reserved for later use.
    #[allow(dead_code)]
    pub exclude_roots: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude: [".git", "node_modules", "dist", "build", "target"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            exclude_roots: Vec::new(),
        }
    }
}

impl Config {
    /// `%APPDATA%\navkit\config.toml` on Windows, `~/.config/navkit/config.toml` elsewhere.
    pub fn path() -> Option<PathBuf> {
        let base = directories::BaseDirs::new()?;
        Some(base.config_dir().join("navkit").join("config.toml"))
    }

    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("{}: {err}", path.display()).into()),
        };
        toml::from_str(&text).map_err(|err| format!("{}: {err}", path.display()).into())
    }
}
