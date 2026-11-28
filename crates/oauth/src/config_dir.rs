use std::path::PathBuf;

/// Returns the configured Antre config directory.
///
/// Resolution order comes from `antre_config::config_dir()`:
/// 1. programmatic override (`set_config_dir`)
/// 2. `ANTRE_CONFIG_DIR`
/// 3. `~/.config/antre`
pub fn antre_config_dir() -> PathBuf {
    antre_config::config_dir().unwrap_or_else(|| PathBuf::from(".config/antre"))
}
