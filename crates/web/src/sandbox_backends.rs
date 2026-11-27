//! Sandbox backend descriptors shown in the settings UI.

use serde_json::{Value, json};

/// Entry for the OS sandbox detected on this host (`"seatbelt"` or
/// `"bubblewrap"`), or an install hint on Linux when none is usable.
pub(crate) fn os_sandbox_backend_entry(detected: Option<&str>) -> Option<Value> {
    match detected {
        Some("seatbelt") => Some(json!({
            "id": "seatbelt",
            "label": "macOS Seatbelt (OS sandbox)",
            "kind": "local",
            "available": true,
            "description": "No container needed. System read-only, writes limited to the workspace and sandbox home, home directory and secrets hidden, network only through the egress proxy.",
        })),
        Some("bubblewrap") => Some(json!({
            "id": "bubblewrap",
            "label": "Linux bubblewrap (OS sandbox)",
            "kind": "local",
            "available": true,
            "description": "No container needed. System read-only, writes limited to the workspace and sandbox home, home directory and secrets hidden, network blocked unless set to bypass.",
        })),
        _ if cfg!(target_os = "linux") => Some(json!({
            "id": "bubblewrap",
            "label": "Linux bubblewrap (OS sandbox)",
            "kind": "local",
            "available": false,
            "installHint": "Lightweight isolation without containers. Requires unprivileged user namespaces.",
            "installCommand": "sudo apt install bubblewrap",
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_backends_are_available() {
        for name in ["seatbelt", "bubblewrap"] {
            let entry = os_sandbox_backend_entry(Some(name));
            assert_eq!(entry.as_ref().map(|e| e["id"].clone()), Some(json!(name)));
            assert_eq!(entry.map(|e| e["available"].clone()), Some(json!(true)));
        }
    }

    #[test]
    fn missing_backend_only_hinted_on_linux() {
        let entry = os_sandbox_backend_entry(None);
        if cfg!(target_os = "linux") {
            assert_eq!(entry.map(|e| e["available"].clone()), Some(json!(false)));
        } else {
            assert!(entry.is_none());
        }
    }
}
