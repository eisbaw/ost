//! Configuration and credential storage

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::auth::{StoredToken, TokenStore};

/// Application configuration
#[derive(Debug, Default, Serialize, Deserialize)]
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

    /// Save configuration to disk
    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir).context("Failed to create config directory")?;

        let path = Self::config_path()?;
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content).context("Failed to write config file")?;

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
