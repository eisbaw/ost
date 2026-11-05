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
    /// True when the tokens above live in the desktop keyring instead of
    /// this file; the file then holds no secrets.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tokens_in_keyring: bool,
}

/// Every secret in [`Config`], stored as one keyring entry.
#[derive(Default, Serialize, Deserialize)]
struct Secrets {
    access_token: Option<StoredToken>,
    refresh_token: Option<String>,
    skype_token: Option<StoredToken>,
    graph_token: Option<StoredToken>,
    ic3_token: Option<StoredToken>,
    recorder_token: Option<StoredToken>,
}

const KEYRING_SERVICE: &str = "ost";
const KEYRING_ACCOUNT: &str = "teams-tokens";

/// The keyring is skipped when OST_NO_KEYRING is set (CI, headless use).
fn keyring_entry() -> Option<keyring::Entry> {
    if std::env::var_os("OST_NO_KEYRING").is_some() {
        return None;
    }
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()
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

    /// Load configuration from disk, and the tokens from the keyring.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).context("Failed to read config file")?;
        let mut config: Self = toml::from_str(&content).context("Failed to parse config file")?;
        if config.tokens_in_keyring {
            match keyring_entry().map(|e| e.get_password()) {
                Some(Ok(json)) => config.apply_secrets(
                    serde_json::from_str(&json).context("Keyring entry is not valid")?,
                ),
                Some(Err(keyring::Error::NoEntry)) => {
                    tracing::warn!("Tokens were in the keyring but are gone; sign in again");
                }
                Some(Err(e)) => return Err(anyhow::anyhow!("Could not read the keyring: {e}")),
                None => tracing::warn!("Tokens are in the keyring, but the keyring is disabled"),
            }
        }
        Ok(config)
    }

    /// Save configuration. Tokens go to the desktop keyring when one is
    /// available, so the file on disk holds no secrets; without a keyring
    /// they are written to the file, which is created owner-only (0600).
    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir).context("Failed to create config directory")?;

        let secrets = self.secrets();
        let mut on_disk = self.clone_settings();
        match keyring_entry() {
            Some(entry) => {
                let json = serde_json::to_string(&secrets).context("Failed to encode tokens")?;
                match entry.set_password(&json) {
                    Ok(()) => on_disk.tokens_in_keyring = true,
                    Err(e) => {
                        tracing::warn!(
                            "Keyring unavailable ({e}); storing tokens in the config file"
                        );
                        on_disk.apply_secrets(secrets);
                    }
                }
            }
            None => on_disk.apply_secrets(secrets),
        }

        let path = Self::config_path()?;
        let content = toml::to_string_pretty(&on_disk).context("Failed to serialize config")?;
        write_private(&path, content.as_bytes())
    }

    fn secrets(&self) -> Secrets {
        Secrets {
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            skype_token: self.skype_token.clone(),
            graph_token: self.graph_token.clone(),
            ic3_token: self.ic3_token.clone(),
            recorder_token: self.recorder_token.clone(),
        }
    }

    fn apply_secrets(&mut self, s: Secrets) {
        self.access_token = s.access_token;
        self.refresh_token = s.refresh_token;
        self.skype_token = s.skype_token;
        self.graph_token = s.graph_token;
        self.ic3_token = s.ic3_token;
        self.recorder_token = s.recorder_token;
    }

    /// Everything except the tokens.
    fn clone_settings(&self) -> Self {
        Self {
            tenant_id: self.tenant_id.clone(),
            region_gtms: self.region_gtms.clone(),
            ..Self::default()
        }
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
        if let Some(entry) = keyring_entry() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => tracing::warn!("Could not remove tokens from the keyring: {e}"),
            }
        }
        self.tokens_in_keyring = false;
        self.access_token = None;
        self.refresh_token = None;
        self.skype_token = None;
        self.graph_token = None;
        self.ic3_token = None;
        self.recorder_token = None;
        self.region_gtms = None;
    }
}

/// Write a file readable only by its owner, without a window in which it is
/// briefly world-readable (create with 0600 rather than chmod afterwards).
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("toml.tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).context("Failed to write config file")?;
    f.write_all(bytes).context("Failed to write config file")?;
    f.sync_all().ok();
    fs::rename(&tmp, path).context("Failed to replace config file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .context("Failed to set config permissions")?;
    }
    Ok(())
}

#[cfg(test)]
mod keyring_split_tests {
    use super::*;

    #[test]
    fn settings_written_to_disk_hold_no_tokens() {
        let mut c = Config::default();
        c.set_access_token("access-secret".into(), Some(3600));
        c.set_refresh_token("refresh-secret".into());
        c.set_skype_token("skype-secret".into(), Some(3600));
        c.tenant_id = Some("09a28c80-30f0-4c4d-98ba-0f7f8d19b780".into());
        let on_disk = toml::to_string_pretty(&c.clone_settings()).unwrap();
        assert!(!on_disk.contains("secret"), "{on_disk}");
        assert!(on_disk.contains("09a28c80"));
    }

    #[test]
    fn secrets_round_trip() {
        let mut c = Config::default();
        c.set_refresh_token("r".into());
        c.set_graph_token("g".into(), None);
        let json = serde_json::to_string(&c.secrets()).unwrap();
        let mut back = Config::default();
        back.apply_secrets(serde_json::from_str(&json).unwrap());
        assert_eq!(back.get_refresh_token().as_deref(), Some("r"));
        assert_eq!(
            back.get_graph_token().map(|t| t.token).as_deref(),
            Some("g")
        );
    }
}
