//! Paths used by every SonicForge component.
//!
//! Configuration lives under `%APPDATA%\SonicForge`, logs and cached presets
//! under `%LOCALAPPDATA%\SonicForge`. Nothing is written to the installation
//! directory, which keeps the application compatible with Program Files ACLs.

use std::path::PathBuf;

/// Root of the per-user configuration tree.
pub fn config_dir() -> PathBuf {
    base_dir("APPDATA", "SonicForge").join("config")
}

/// Root of the per-user data tree (logs, cache).
pub fn data_dir() -> PathBuf {
    base_dir("LOCALAPPDATA", "SonicForge")
}

/// Directory holding log files.
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// The main configuration file processed by the APO.
pub fn config_file() -> PathBuf {
    config_dir().join("config.txt")
}

/// Directory holding the presets shipped with the application.
pub fn presets_dir() -> PathBuf {
    config_dir().join("presets")
}

/// Optional per-endpoint configuration override.
///
/// The APO looks for `<config dir>\devices\<guid>.txt` before falling back to
/// `config.txt`, which gives every device an independent configuration.
pub fn device_config_file(guid: &str) -> PathBuf {
    let safe: String = guid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    config_dir().join("devices").join(format!("{safe}.txt"))
}

fn base_dir(env: &str, app: &str) -> PathBuf {
    let root = std::env::var_os(env)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|p| {
                PathBuf::from(p).join("AppData").join(if env == "APPDATA" {
                    "Roaming"
                } else {
                    "Local"
                })
            })
        })
        .unwrap_or_else(|| PathBuf::from("."));
    root.join(app)
}

/// Creates the configuration tree if it does not exist yet and returns the
/// main configuration path. Used on first start.
pub fn ensure_config_layout() -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(config_dir().join("devices"))?;
    std::fs::create_dir_all(presets_dir())?;
    std::fs::create_dir_all(log_dir())?;
    let config = config_file();
    if !config.exists() {
        std::fs::write(
            &config,
            "# SonicForge configuration\n# See docs/CONFIGURATION.md for the command reference.\nPreamp: 0 dB\n",
        )?;
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_config_names_are_filesystem_safe() {
        let path = device_config_file("{0.0.0.00000000}.{A1/B2}");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
        assert!(name.ends_with(".txt"));
    }

    #[test]
    fn config_file_lives_in_the_config_directory() {
        assert_eq!(config_file().parent().unwrap(), config_dir());
    }

    #[test]
    fn log_directory_is_inside_the_data_directory() {
        assert!(log_dir().starts_with(data_dir()));
    }
}
