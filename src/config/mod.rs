//! Configuration and credential storage

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::auth::{StoredToken, TokenStore};

/// Legacy single-account profile: maps to `config.toml` (all existing
/// installs + the CLI default). Every other profile maps to
/// `config-<sanitized>.toml` beside it.
pub const DEFAULT_PROFILE: &str = "default";

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

    /// Config file path for one profile. `""`/`"default"` (any case,
    /// surrounding whitespace ignored) is the legacy `config.toml`;
    /// every other id is sanitized (alphanumeric + `._-` kept, the
    /// rest `_`, capped at 64 chars) into `config-<id>.toml`.
    pub fn config_path_for(profile: &str) -> Result<PathBuf> {
        let name = normalize_profile(profile);
        let file = if name.eq_ignore_ascii_case(DEFAULT_PROFILE) {
            "config.toml".to_string()
        } else {
            format!("config-{}.toml", sanitize_profile(&name))
        };
        Ok(Self::config_dir()?.join(file))
    }

    /// Load configuration from disk
    pub fn load() -> Result<Self> {
        Self::load_for(&active_profile())
    }

    /// Load one profile's configuration from disk (no cross-read:
    /// each profile sees only its own file).
    pub fn load_for(profile: &str) -> Result<Self> {
        let path = Self::config_path_for(profile)?;

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
        Self::load_cached_for(&active_profile())
    }

    /// Cached load of one profile (per-profile cache slot; same
    /// fingerprint semantics as [`Self::load_cached`]).
    pub fn load_cached_for(profile: &str) -> Result<Self> {
        let name = normalize_profile(profile);
        let path = Self::config_path_for(&name)?;
        let fp = fingerprint_of(&path);
        if let Some(hit) = cache_get(&name, &fp) {
            return Ok(hit);
        }
        let cfg = Self::load_for(&name)?;
        cache_put(&name, &fp, cfg.clone());
        Ok(cfg)
    }

    /// Drop the cached config (tests; the next [`Self::load_cached`]
    /// re-reads from disk). Clears every profile slot.
    pub fn invalidate_cache() {
        config_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Drop one profile's cache slot; the next
    /// [`Self::load_cached_for`] re-reads from disk.
    pub fn invalidate_cache_for(profile: &str) {
        config_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&normalize_profile(profile));
    }

    /// Save configuration to disk
    pub fn save(&self) -> Result<()> {
        self.save_to(&active_profile())
    }

    /// Delete one profile's file (remove-account). Returns true when a
    /// file was removed. The cache slot is dropped either way.
    pub fn delete_for(profile: &str) -> Result<bool> {
        let name = normalize_profile(profile);
        Self::invalidate_cache_for(&name);
        let path = Self::config_path_for(&name)?;
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path).context("Failed to delete profile config file")?;
        Ok(true)
    }

    /// Save to one profile's file (0600, cache write-through).
    pub fn save_to(&self, profile: &str) -> Result<()> {
        let name = normalize_profile(profile);
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir).context("Failed to create config directory")?;

        let path = Self::config_path_for(&name)?;
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content).context("Failed to write config file")?;
        // Write through the cache so later load_cached() stays coherent.
        let fp = fingerprint_of(&path);
        cache_put(&name, &fp, self.clone());

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

/// Process-wide active profile (multi-account: the account the app
/// currently shows). Defaults to [`DEFAULT_PROFILE`], so the CLI and
/// all legacy callers keep the `config.toml` behavior.
fn active_slot() -> &'static Mutex<String> {
    static A: OnceLock<Mutex<String>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(DEFAULT_PROFILE.to_string()))
}

/// Current active profile id (normalized).
pub fn active_profile() -> String {
    active_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Switch the active profile (normalized; empty → default). All
/// profile-agnostic `load`/`save`/client/trouter paths follow it.
pub fn set_active_profile(profile: &str) {
    *active_slot().lock().unwrap_or_else(|e| e.into_inner()) =
        normalize_profile(profile);
}

/// Trimmed id, or [`DEFAULT_PROFILE`] when blank.
pub fn normalize_profile(profile: &str) -> String {
    let t = profile.trim();
    if t.is_empty() {
        DEFAULT_PROFILE.to_string()
    } else {
        t.to_string()
    }
}

/// Filename-safe profile id: alphanumerics + `._-` kept, the rest
/// `_`, capped at 64 chars (never empty).
fn sanitize_profile(profile: &str) -> String {
    let clean: String = profile
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if clean.is_empty() {
        "_".to_string()
    } else {
        clean
    }
}

fn config_cache() -> &'static Mutex<HashMap<String, (Fingerprint, Config)>> {
    static C: OnceLock<Mutex<HashMap<String, (Fingerprint, Config)>>> =
        OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint_of(path: &PathBuf) -> Fingerprint {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok().map(|t| (m.len(), t)))
}

fn cache_get(profile: &str, fp: &Fingerprint) -> Option<Config> {
    let guard = config_cache().lock().unwrap_or_else(|e| e.into_inner());
    match guard.get(profile) {
        Some((cfp, cfg)) if cfp == fp => Some(cfg.clone()),
        _ => None,
    }
}

fn cache_put(profile: &str, fp: &Fingerprint, cfg: Config) {
    config_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(profile.to_string(), (fp.clone(), cfg));
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

    #[test]
    fn profile_paths_separate_default_from_accounts() {
        let file = |p: &str| {
            Config::config_path_for(p)
                .expect("path")
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        };
        assert_eq!(file("default"), "config.toml");
        assert_eq!(file(""), "config.toml");
        assert_eq!(file("  DEFAULT  "), "config.toml");
        let a = file("user-a-id");
        let b = file("user-b-id");
        assert_eq!(a, "config-user-a-id.toml");
        assert_eq!(b, "config-user-b-id.toml");
        assert_ne!(a, b);
        // Hostile ids stay one flat filename (no separators survive).
        let evil = file("../../etc/x");
        assert_eq!(evil, "config-.._.._etc_x.toml");
        assert!(!evil.contains('/'));
    }

    #[test]
    fn active_profile_round_trips_and_defaults() {
        let prev = active_profile();
        set_active_profile("user-a-id");
        assert_eq!(active_profile(), "user-a-id");
        set_active_profile("   ");
        assert_eq!(active_profile(), DEFAULT_PROFILE);
        set_active_profile(&prev);
        Config::invalidate_cache();
    }

    #[test]
    fn delete_missing_profile_is_false_without_write() {
        Config::invalidate_cache();
        let gone = Config::delete_for("d1-accounts-test-no-such-profile")
            .expect("delete");
        assert!(!gone);
    }
}
