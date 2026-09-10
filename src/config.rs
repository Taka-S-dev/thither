use std::path::PathBuf;

use serde::Deserialize;

/// User configuration. Every field has a default so the file is optional.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Show Nerd Font icons. The terminal must use a compatible font.
    pub icons: bool,
    /// Capture mouse input for list selection and scrolling.
    pub mouse: bool,
    /// Maximum temporary copy size in MiB; zero disables temporary copies.
    pub temp_copy_max_mib: u64,
    /// Directory names to skip at any depth.
    pub exclude: Vec<String>,
    /// Absolute paths never treated as project roots. Reserved for later use.
    #[allow(dead_code)]
    pub exclude_roots: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            icons: false,
            mouse: true,
            temp_copy_max_mib: 100,
            exclude: [".git", "node_modules", "dist", "build", "target"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            exclude_roots: Vec::new(),
        }
    }
}

impl Config {
    /// Explicit override, portable `config/` beside the executable, then user config.
    pub fn path() -> Option<PathBuf> {
        // An explicit override also isolates subprocess tests on Windows, where
        // Known Folder APIs do not follow a replaced APPDATA environment variable.
        if let Some(dir) = std::env::var_os("TADORU_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
            return Some(std::path::absolute(dir).ok()?.join("config.toml"));
        }
        if let Some(path) = std::env::current_exe().ok().and_then(portable_config) {
            return Some(path);
        }
        let base = directories::BaseDirs::new()?;
        Some(base.config_dir().join("tadoru").join("config.toml"))
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

fn portable_config(exe: PathBuf) -> Option<PathBuf> {
    let dir = exe.parent()?.join("config");
    dir.is_dir().then(|| dir.join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_directory_enables_configuration_even_before_files_exist() {
        let root = std::env::temp_dir().join(format!("tadoru-portable-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("tadoru.exe");
        assert_eq!(portable_config(exe.clone()), None);
        std::fs::create_dir(root.join("config")).unwrap();
        assert_eq!(portable_config(exe), Some(root.join("config/config.toml")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_configs_keep_icons_disabled() {
        let config: Config = toml::from_str("exclude = ['.git']").unwrap();
        assert!(!config.icons);
        assert_eq!(config.temp_copy_max_mib, 100);
        let config: Config = toml::from_str("icons = true").unwrap();
        assert!(config.icons);
        assert_eq!(config.exclude, Config::default().exclude);
    }
}
