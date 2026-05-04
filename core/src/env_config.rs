//! Persisted list of loaded `.env` files and the currently-active index.
//!
//! Users can load multiple `.env` files via the desktop/TUI UIs; exactly one
//! is active at a time and contributes variables to the runtime. This module
//! owns the on-disk representation (a small JSON file in the platform config
//! dir) so the list and active choice survive app restarts.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A single loaded `.env` entry. `name` is either the `# @@name` directive
/// embedded in the file or the filename stem if absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvEntry {
    pub path: String,
    pub name: String,
}

/// Persisted env-file list plus the active index (or `None` for "no env").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvConfig {
    #[serde(default)]
    pub entries: Vec<EnvEntry>,
    #[serde(default)]
    pub active_index: Option<usize>,
}

impl EnvConfig {
    /// Add `entry` to the list, deduping by canonicalized path. Returns the
    /// index of the entry in the list afterwards. If it's the first entry,
    /// it is automatically activated.
    pub fn add(&mut self, entry: EnvEntry) -> usize {
        let norm = canonical_key(&entry.path);
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| canonical_key(&e.path) == norm)
        {
            // Refresh the display name in case it changed (e.g. user edited the
            // `# @@name` directive). Keep the active_index unchanged.
            self.entries[i].name = entry.name;
            return i;
        }
        self.entries.push(entry);
        let i = self.entries.len() - 1;
        if self.active_index.is_none() {
            self.active_index = Some(i);
        }
        i
    }

    /// Remove the entry at `idx`. Adjusts `active_index` so the same entry
    /// stays active when possible; clears it if we removed the active one.
    pub fn remove(&mut self, idx: usize) {
        if idx >= self.entries.len() {
            return;
        }
        self.entries.remove(idx);
        self.active_index = match self.active_index {
            Some(a) if a == idx => {
                if self.entries.is_empty() {
                    None
                } else {
                    Some(a.min(self.entries.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
    }

    /// Set the active entry. `None` deactivates; out-of-range is a no-op.
    pub fn set_active(&mut self, idx: Option<usize>) {
        match idx {
            None => self.active_index = None,
            Some(i) if i < self.entries.len() => self.active_index = Some(i),
            Some(_) => { /* ignore */ }
        }
    }

    pub fn active_entry(&self) -> Option<&EnvEntry> {
        self.active_index.and_then(|i| self.entries.get(i))
    }
}

fn canonical_key(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

/// Return the on-disk path for the persisted env config. Honors the
/// `RP_ENV_CONFIG_PATH` env var (used by tests to sandbox to a temp dir),
/// otherwise defaults to `<config_dir>/request-pilot/env_config.json`.
pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("RP_ENV_CONFIG_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    config_dir().join("request-pilot").join("env_config.json")
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

/// Load the persisted config from `path`. Returns an empty config if the file
/// does not exist. Surfaces JSON parse errors to the caller so the UI can
/// decide whether to show a warning vs. silently reset.
pub fn load_from(path: &Path) -> Result<EnvConfig, String> {
    if !path.exists() {
        return Ok(EnvConfig::default());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    if content.trim().is_empty() {
        return Ok(EnvConfig::default());
    }
    let mut cfg: EnvConfig = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    // Guard: clamp stale active_index (e.g. user hand-edited the file).
    if let Some(i) = cfg.active_index {
        if i >= cfg.entries.len() {
            cfg.active_index = if cfg.entries.is_empty() { None } else { Some(0) };
        }
    }
    Ok(cfg)
}

/// Persist `cfg` to `path`, creating parent directories as needed.
pub fn save_to(path: &Path, cfg: &EnvConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let content =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(())
}

/// Convenience: load from the default path.
pub fn load() -> Result<EnvConfig, String> {
    load_from(&default_config_path())
}

/// Convenience: save to the default path.
pub fn save(cfg: &EnvConfig) -> Result<(), String> {
    save_to(&default_config_path(), cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str) -> EnvEntry {
        EnvEntry {
            name: name.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn first_entry_auto_activates() {
        let mut c = EnvConfig::default();
        let i = c.add(entry("dev", "unlikely-path-dev.env"));
        assert_eq!(i, 0);
        assert_eq!(c.active_index, Some(0));
    }

    #[test]
    fn second_entry_does_not_change_active() {
        let mut c = EnvConfig::default();
        c.add(entry("dev", "unlikely-path-dev.env"));
        c.add(entry("prod", "unlikely-path-prod.env"));
        assert_eq!(c.active_index, Some(0));
    }

    #[test]
    fn add_dedupes_by_path_and_refreshes_name() {
        let mut c = EnvConfig::default();
        c.add(entry("dev", "unlikely-path-dev.env"));
        let i = c.add(entry("renamed", "unlikely-path-dev.env"));
        assert_eq!(i, 0);
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].name, "renamed");
    }

    #[test]
    fn remove_active_shifts_to_neighbor_or_clears() {
        let mut c = EnvConfig::default();
        c.add(entry("a", "p-a.env"));
        c.add(entry("b", "p-b.env"));
        c.add(entry("c", "p-c.env"));
        c.set_active(Some(1));
        c.remove(1); // remove b
        // active was 1, now should stay at index 1 (formerly c)
        assert_eq!(c.active_index, Some(1));
        assert_eq!(c.entries[1].name, "c");

        c.remove(1); // remove c
        assert_eq!(c.active_index, Some(0));
        c.remove(0); // remove a
        assert_eq!(c.active_index, None);
        assert!(c.entries.is_empty());
    }

    #[test]
    fn remove_before_active_decrements_active_index() {
        let mut c = EnvConfig::default();
        c.add(entry("a", "p-a.env"));
        c.add(entry("b", "p-b.env"));
        c.add(entry("c", "p-c.env"));
        c.set_active(Some(2)); // c
        c.remove(0); // remove a
        assert_eq!(c.active_index, Some(1));
        assert_eq!(c.entries[c.active_index.unwrap()].name, "c");
    }

    #[test]
    fn set_active_out_of_range_is_noop() {
        let mut c = EnvConfig::default();
        c.add(entry("a", "p-a.env"));
        c.set_active(Some(42));
        assert_eq!(c.active_index, Some(0));
        c.set_active(None);
        assert_eq!(c.active_index, None);
    }

    #[test]
    fn load_missing_returns_default() {
        let p = std::env::temp_dir().join("rp-env-config-nonexistent-xyz.json");
        let _ = std::fs::remove_file(&p);
        let cfg = load_from(&p).unwrap();
        assert!(cfg.entries.is_empty());
        assert_eq!(cfg.active_index, None);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let p = std::env::temp_dir().join("rp-env-config-roundtrip.json");
        let _ = std::fs::remove_file(&p);
        let mut cfg = EnvConfig::default();
        cfg.add(entry("dev", "some-dev.env"));
        cfg.add(entry("prod", "some-prod.env"));
        cfg.set_active(Some(1));
        save_to(&p, &cfg).unwrap();
        let loaded = load_from(&p).unwrap();
        assert_eq!(loaded, cfg);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_clamps_stale_active_index() {
        let p = std::env::temp_dir().join("rp-env-config-clamp.json");
        std::fs::write(&p, r#"{"entries":[{"name":"a","path":"a.env"}],"active_index":99}"#)
            .unwrap();
        let cfg = load_from(&p).unwrap();
        assert_eq!(cfg.active_index, Some(0));
        let _ = std::fs::remove_file(&p);
    }
}
