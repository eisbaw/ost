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
    /// The signed-in tenant: the recorded one, or failing that the `tid` of
    /// the stored access token (configs written before it was recorded).
    pub fn tenant_id(&self) -> Option<String> {
        self.tenant_id.clone().or_else(|| {
            self.access_token
                .as_ref()
                .and_then(|t| tenant_id_from_jwt(&t.token))
        })
    }

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
        // Calling needs the tenant id, and nothing else records it: take it
        // from the token's `tid` claim each time the token is stored, so both
        // a fresh login and a refresh keep it current.
        if let Some(tid) = tenant_id_from_jwt(&token) {
            self.tenant_id = Some(tid);
        }
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

/// Read the `tid` (tenant id) claim from a JWT without verifying it.
///
/// The token was just issued to us by Azure AD over TLS; this only reads a
/// routing value from it, and never trusts it for authorization.
fn tenant_id_from_jwt(token: &str) -> Option<String> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let tid = json.get("tid")?.as_str()?;
    // A tenant id is a GUID; refuse anything else rather than store junk.
    let is_guid = tid.len() == 36
        && tid.chars().enumerate().all(|(i, c)| {
            matches!(i, 8 | 13 | 18 | 23) == (c == '-') && (c == '-' || c.is_ascii_hexdigit())
        });
    is_guid.then(|| tid.to_string())
}

#[cfg(test)]
mod tenant_id_tests {
    use super::*;
    use base64::Engine;

    fn jwt_with(payload: &str) -> String {
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!(
            "{}.{}.sig",
            enc.encode(b"{\"alg\":\"none\"}"),
            enc.encode(payload)
        )
    }

    #[test]
    fn reads_tid_claim() {
        let t = jwt_with(r#"{"tid":"2716d134-5614-4856-a48d-2002edd9a570","oid":"x"}"#);
        assert_eq!(
            tenant_id_from_jwt(&t).as_deref(),
            Some("2716d134-5614-4856-a48d-2002edd9a570")
        );
    }

    #[test]
    fn rejects_missing_or_malformed_tid() {
        assert_eq!(tenant_id_from_jwt(&jwt_with(r#"{"oid":"x"}"#)), None);
        assert_eq!(
            tenant_id_from_jwt(&jwt_with(r#"{"tid":"../../etc"}"#)),
            None
        );
        assert_eq!(tenant_id_from_jwt("not-a-jwt"), None);
    }

    #[test]
    fn set_access_token_records_tenant() {
        let mut c = Config::default();
        c.set_access_token(
            jwt_with(r#"{"tid":"09a28c80-30f0-4c4d-98ba-0f7f8d19b780"}"#),
            Some(3600),
        );
        assert_eq!(
            c.tenant_id.as_deref(),
            Some("09a28c80-30f0-4c4d-98ba-0f7f8d19b780")
        );
    }
}
