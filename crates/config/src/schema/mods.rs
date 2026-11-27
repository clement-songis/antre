use serde::{Deserialize, Serialize};

/// `[mods]`: the mod runtime (WASM components with consent-gated
/// capabilities). Installed mods and grants live in `data_dir()/mods/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModsConfig {
    /// Load installed mods and expose the `mods.*` API.
    pub enabled: bool,
    /// Fuel (roughly, WASM instructions) per call into a mod.
    pub fuel: u64,
    /// Memory limit per call, in MiB.
    pub memory_mb: u32,
    /// Wall-clock limit per call, in milliseconds.
    pub timeout_ms: u64,
    /// Epoch tick used to enforce `timeout_ms`, in milliseconds.
    pub epoch_interval_ms: u64,
    /// Largest string a mod may return from one call, in KiB.
    pub max_output_kb: u32,
    /// Pin filesystem shell hooks by content: hooks that change or appear
    /// after the first run stay disabled until approved.
    pub pin_shell_hooks: bool,
}

impl Default for ModsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fuel: 50_000_000,
            memory_mb: 32,
            timeout_ms: 10_000,
            epoch_interval_ms: 50,
            max_output_kb: 1024,
            pin_shell_hooks: true,
        }
    }
}
