//! Persisted desktop/TUI configuration for the Sessions feature.
//!
//! Stored at `<config_dir>/request-pilot/sessions_config.json`. Owns the
//! sessions root path and any per-store policy preferences. When `root` is
//! `None`, sessions auto-record is disabled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sessions::CapturePolicy;

/// User configuration for the Sessions feature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionsConfig {
    /// Absolute path to the sessions root directory. `None` disables
    /// auto-record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Whether to write sessions automatically after each run when a root
    /// is configured. Defaults to `true` once the user opts in by setting a
    /// root, but can be toggled off without losing the path.
    #[serde(default = "default_true")]
    pub auto_record: bool,
    /// The active capture policy. Defaults to the Snapshot preset (full
    /// bodies up to 10 MB, names-only variables, header/query redaction).
    #[serde(default)]
    pub capture_policy: CapturePolicy,
    /// Symbolic name of the active preset (`"snapshot" | "privacy_first" |
    /// "full_debug" | "custom"`). Used by the UI to decide whether to show
    /// the policy as one of the named presets or as a custom config.
    #[serde(default = "default_preset")]
    pub capture_preset: String,
    /// Global body-redaction patterns applied to every recorded session.
    /// Each entry is either a JSON path like `$.password` or a slash-delimited
    /// regex like `/secret-\d+/`. Parsing is performed by the redaction
    /// applier (shared with the `@@redact` directive); this struct only
    /// stores the raw strings.
    #[serde(default)]
    pub body_redaction_paths: Vec<String>,
    /// Optional retention policy used by `SessionStore::prune`. When `None`
    /// (or any individual cap is `None`), no caps are applied. Older configs
    /// without this field deserialize cleanly with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<RetentionPolicy>,
}

/// Caps applied by `SessionStore::prune`. Every field is optional; a `None`
/// means "no cap for this dimension".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RetentionPolicy {
    /// Drop sessions whose `started_at` is older than this many days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_days: Option<u32>,
    /// For each `(file_id, sha)`, keep only the newest N sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sessions_per_version: Option<u32>,
    /// Cap total disk usage across the entire store (in GiB). When exceeded,
    /// the oldest sessions are dropped first until the total is under cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_size_gb: Option<f64>,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            root: None,
            auto_record: true,
            capture_policy: CapturePolicy::default(),
            capture_preset: default_preset(),
            body_redaction_paths: Vec::new(),
            retention: None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_preset() -> String {
    "snapshot".to_string()
}

impl SessionsConfig {
    /// Replace `capture_policy` with the named preset. Unknown names fall
    /// back to the Snapshot preset. Use `"custom"` to keep the current
    /// policy and only update the preset label.
    pub fn apply_preset(&mut self, preset: &str) {
        self.capture_preset = preset.to_string();
        match preset {
            "snapshot" => self.capture_policy = CapturePolicy::default(),
            "privacy_first" => self.capture_policy = CapturePolicy::privacy_first(),
            "full_debug" => self.capture_policy = CapturePolicy::full_debug(),
            "custom" => {}
            _ => self.capture_policy = CapturePolicy::default(),
        }
    }
}

/// Default on-disk path for the persisted config
/// (`<config_dir>/request-pilot/sessions_config.json`).
pub fn default_config_path() -> PathBuf {
    config_dir().join("request-pilot").join("sessions_config.json")
}

fn config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Ok(p) = std::env::var("APPDATA") {
            return PathBuf::from(p);
        }
    } else if cfg!(target_os = "macos") {
        if let Ok(h) = std::env::var("HOME") {
            return PathBuf::from(h).join("Library").join("Application Support");
        }
    } else if let Ok(p) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(p);
    } else if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".config");
    }
    PathBuf::from(".")
}

pub fn load_from(path: &Path) -> Result<SessionsConfig, String> {
    if !path.exists() {
        return Ok(SessionsConfig::default());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    if content.trim().is_empty() {
        return Ok(SessionsConfig::default());
    }
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

pub fn save_to(path: &Path, cfg: &SessionsConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("Failed to serialize sessions config: {}", e))?;
    std::fs::write(path, json)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rp-sessions-config-{}-{}.json",
            label,
            uuid::Uuid::new_v4()
        ));
        p
    }

    #[test]
    fn round_trip_default() {
        let p = tmp_path("rt");
        let cfg = SessionsConfig {
            root: Some("C:/tmp/sessions".to_string()),
            auto_record: true,
            capture_policy: CapturePolicy::default(),
            capture_preset: "snapshot".to_string(),
            body_redaction_paths: Vec::new(),
            retention: None,
        };
        save_to(&p, &cfg).unwrap();
        let loaded = load_from(&p).unwrap();
        assert_eq!(loaded, cfg);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_yields_default() {
        let p = tmp_path("missing");
        let cfg = load_from(&p).unwrap();
        assert!(cfg.root.is_none());
        assert!(cfg.auto_record); // default_true
    }

    #[test]
    fn empty_file_yields_default() {
        let p = tmp_path("empty");
        std::fs::write(&p, "").unwrap();
        let cfg = load_from(&p).unwrap();
        assert!(cfg.root.is_none());
        assert_eq!(cfg.capture_preset, "snapshot");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn apply_preset_switches_policy() {
        let mut cfg = SessionsConfig::default();
        assert_eq!(cfg.capture_preset, "snapshot");

        cfg.apply_preset("privacy_first");
        assert_eq!(cfg.capture_preset, "privacy_first");
        // Privacy-first drops request bodies
        match cfg.capture_policy.request_bodies {
            crate::sessions::BodyCapture::Off => {}
            other => panic!("expected Off, got {:?}", other),
        }

        cfg.apply_preset("full_debug");
        assert_eq!(cfg.capture_preset, "full_debug");
        match cfg.capture_policy.request_bodies {
            crate::sessions::BodyCapture::Full => {}
            other => panic!("expected Full, got {:?}", other),
        }
        assert!(cfg.capture_policy.headers_denylist.is_empty());

        cfg.apply_preset("snapshot");
        assert_eq!(cfg.capture_preset, "snapshot");
    }

    #[test]
    fn default_config_has_empty_body_redaction_paths() {
        let cfg = SessionsConfig::default();
        assert!(cfg.body_redaction_paths.is_empty());
    }

    #[test]
    fn legacy_config_without_field_deserializes_to_empty() {
        let p = tmp_path("legacy-redact");
        std::fs::write(&p, r#"{"root":"C:/x","auto_record":true}"#).unwrap();
        let cfg = load_from(&p).unwrap();
        assert!(cfg.body_redaction_paths.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn roundtrip_with_redaction_paths() {
        let p = tmp_path("rt-redact");
        let cfg = SessionsConfig {
            root: Some("C:/tmp/sessions".to_string()),
            auto_record: true,
            capture_policy: CapturePolicy::default(),
            capture_preset: "snapshot".to_string(),
            body_redaction_paths: vec![
                "$.password".to_string(),
                "$.user.token".to_string(),
                r"/secret-\d+/".to_string(),
            ],
            retention: None,
        };
        save_to(&p, &cfg).unwrap();
        let loaded = load_from(&p).unwrap();
        assert_eq!(loaded, cfg);
        assert_eq!(loaded.body_redaction_paths.len(), 3);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn legacy_config_without_capture_policy_loads() {
        let p = tmp_path("legacy");
        // Pre-1.5 format: only `root` + `auto_record`.
        std::fs::write(&p, r#"{"root":"C:/x","auto_record":true}"#).unwrap();
        let cfg = load_from(&p).unwrap();
        assert_eq!(cfg.root.as_deref(), Some("C:/x"));
        assert_eq!(cfg.capture_preset, "snapshot");
        let _ = std::fs::remove_file(&p);
    }
}
