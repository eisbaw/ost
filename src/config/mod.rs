//! Configuration and credential storage

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::auth::{StoredToken, TokenStore};

/// Application configuration
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Stored AAD access token (audience: api.spaces.skype.com)
    pub access_token: Option<StoredToken>,
    /// Stored AAD refresh token
    pub refresh_token: Option<String>,
    /// User's tenant ID (from last login)
    pub tenant_id: Option<String>,
    /// Stored Skype token (from authsvc exchange)
    pub skype_token: Option<StoredToken>,
    /// Stored Graph API access token (audience: graph.microsoft.com)
    pub graph_token: Option<StoredToken>,
    /// Stored IC3 token (audience: ic3.teams.office.com)
    pub ic3_token: Option<StoredToken>,
    /// Stored recorder service token (audience: 4580fd1d-e5a3-4f56-9ad1-aab0e3bf8f76)
    pub recorder_token: Option<StoredToken>,
    /// Regional endpoint URLs from authsvc response (JSON stored as string for TOML compat)
    pub region_gtms: Option<String>,
}

impl Config {
    /// Get config directory path
    fn config_dir() -> Result<PathBuf> {
        let proj_dirs = ProjectDirs::from("com", "teams-cli", "teams-cli")
            .context("Could not determine config directory")?;
        Ok(proj_dirs.config_dir().to_path_buf())
    }

    /// Get config file path
    fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    /// Load configuration from disk
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).context("Failed to read config file")?;
        toml::from_str(&content).context("Failed to parse config file")
    }

    /// Load configuration, reusing an in-memory copy when the file is
    /// unchanged (same size + mtime; missing file caches as default).
    /// Same result as [`Self::load`]; skips disk read + TOML parse on hits.
    /// [`Self::save`] writes through the cache, so in-process updates are
    /// always coherent; external writers are picked up on mtime change.
    pub fn load_cached() -> Result<Self> {
        let path = Self::config_path()?;
        let fp = fingerprint_of(&path);
        if let Some(hit) = cache_get(&fp) {
            return Ok(hit);
        }
        let cfg = Self::load()?;
        cache_put(&fp, cfg.clone());
        Ok(cfg)
    }

    /// Drop the cached config (tests; the next [`Self::load_cached`]
    /// re-reads from disk).
    pub fn invalidate_cache() {
        *config_cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Save configuration to disk
    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir).context("Failed to create config directory")?;

        let path = Self::config_path()?;
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content).context("Failed to write config file")?;
        // Write through the cache so later load_cached() stays coherent.
        let fp = fingerprint_of(&path);
        cache_put(&fp, self.clone());

        // Set restrictive permissions on config file (contains tokens)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, perms).context("Failed to set config permissions")?;
        }

        Ok(())
    }

    pub fn get_skype_token(&self) -> Option<StoredToken> {
        self.skype_token.clone()
    }

    pub fn set_skype_token(&mut self, token: String, expires_in: Option<u64>) {
        self.skype_token = Some(StoredToken::new(token, expires_in));
    }

    pub fn set_region_gtms(&mut self, gtms: serde_json::Value) {
        self.region_gtms = Some(gtms.to_string());
    }

    pub fn get_region_gtms(&self) -> Option<serde_json::Value> {
        self.region_gtms
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
    }

    pub fn get_graph_token(&self) -> Option<StoredToken> {
        self.graph_token.clone()
    }

    pub fn set_graph_token(&mut self, token: String, expires_in: Option<u64>) {
        self.graph_token = Some(StoredToken::new(token, expires_in));
    }

    pub fn get_ic3_token(&self) -> Option<StoredToken> {
        self.ic3_token.clone()
    }

    pub fn set_ic3_token(&mut self, token: String, expires_in: Option<u64>) {
        self.ic3_token = Some(StoredToken::new(token, expires_in));
    }

    pub fn get_recorder_token(&self) -> Option<StoredToken> {
        self.recorder_token.clone()
    }

    pub fn set_recorder_token(&mut self, token: String, expires_in: Option<u64>) {
        self.recorder_token = Some(StoredToken::new(token, expires_in));
    }
}

/// (file size, mtime) when the config file exists; `None` when absent
/// (caches as default until the file appears).
type Fingerprint = Option<(u64, SystemTime)>;

fn config_cache() -> &'static Mutex<Option<(Fingerprint, Config)>> {
    static C: OnceLock<Mutex<Option<(Fingerprint, Config)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn fingerprint_of(path: &PathBuf) -> Fingerprint {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok().map(|t| (m.len(), t)))
}

fn cache_get(fp: &Fingerprint) -> Option<Config> {
    let guard = config_cache().lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some((cfp, cfg)) if cfp == fp => Some(cfg.clone()),
        _ => None,
    }
}

fn cache_put(fp: &Fingerprint, cfg: Config) {
    *config_cache().lock().unwrap_or_else(|e| e.into_inner()) = Some((fp.clone(), cfg));
}

impl TokenStore for Config {
    fn get_access_token(&self) -> Option<StoredToken> {
        self.access_token.clone()
    }

    fn set_access_token(&mut self, token: String, expires_in: Option<u64>) {
        self.access_token = Some(StoredToken::new(token, expires_in));
    }

    fn get_refresh_token(&self) -> Option<String> {
        self.refresh_token.clone()
    }

    fn set_refresh_token(&mut self, token: String) {
        self.refresh_token = Some(token);
    }

    fn clear_tokens(&mut self) {
        self.access_token = None;
        self.refresh_token = None;
        self.skype_token = None;
        self.graph_token = None;
        self.ic3_token = None;
        self.recorder_token = None;
        self.region_gtms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_cached_matches_load_across_invalidate() {
        // Read-only: no interference with other tests.
        Config::invalidate_cache();
        let disk = Config::load().expect("load");
        let hit = Config::load_cached().expect("cached");
        let ser = |c: &Config| toml::to_string(c).expect("serialize");
        assert_eq!(ser(&disk), ser(&hit));
        Config::invalidate_cache();
        let reloaded = Config::load_cached().expect("reload");
        assert_eq!(ser(&disk), ser(&reloaded));
    }
}
