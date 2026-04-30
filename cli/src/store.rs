//! Helpers for opening the configured `SessionStore` from the persisted
//! `SessionsConfig`. Used by the `sessions` subcommands.

#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};

use request_pilot_core::sessions::SessionStore;
use request_pilot_core::sessions_config::{
    self, default_config_path, SessionsConfig,
};

/// Load the persisted `SessionsConfig` and open a `SessionStore` rooted at
/// `config.root`. Returns a friendly error if no root has been configured
/// yet (i.e. the user has not opted in to session recording).
pub fn open_store() -> Result<(SessionsConfig, SessionStore)> {
    let path = default_config_path();
    let config = sessions_config::load_from(&path)
        .map_err(|e| anyhow!("failed to load sessions config from {}: {}", path.display(), e))?;

    let root = config.root.clone().ok_or_else(|| {
        anyhow!(
            "No sessions root configured. Set one in the desktop/TUI Sessions \
             settings (config file: {}).",
            path.display()
        )
    })?;

    let store = SessionStore::open(&root)
        .with_context(|| format!("failed to open session store at {}", root))?;

    Ok((config, store))
}
