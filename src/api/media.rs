//! Inline media fetch (om-richmedia lane).
//!
//! Teams message HTML carries images as plain `<img src="…">` tags. Two
//! families of `src`:
//! - AMS object views (`…/v1/objects/…/views/imgo`,
//!   `…asyncgw.teams.microsoft.com…`, `…api.asm.skype.com…`): need the
//!   Skype token as `Authorization: skype_token …` (object-store scheme;
//!   the chat-service scheme 401s — om-imgfix). Chat-hosted views
//!   (`…msg.teams.microsoft.com…`) keep the chat-service scheme.
//! - public URLs (Giphy, external): plain GET, no auth — the token must
//!   never leak to third-party hosts.
//!
//! [`needs_auth`] classifies the URL; [`auth_headers`] picks the token
//! scheme per host family; [`fetch_media_data`] downloads it (15 MB cap).
//! Callers surface bytes + content type to the embedder.

use anyhow::{bail, Result};

use super::client::TeamsClient;

/// Download cap: images over this are rejected, never truncated.
pub const MAX_BYTES: usize = 15 * 1024 * 1024;

/// Fetched bytes plus the server's content type, if any.
#[derive(Debug)]
pub struct MediaBytes {
    pub data: Vec<u8>,
    pub content_type: Option<String>,
}

/// True when the URL lives on a Microsoft media host (AMS object store /
/// chat-service family) and needs skypetoken auth. Everything else fetches
/// unauthenticated. Non-https and unparseable URLs return false (the fetch
/// itself rejects them).
pub fn needs_auth(url: &str) -> bool {
    let host = match host_of(url) {
        Some(h) => h,
        None => return false,
    };
    const SUFFIXES: &[&str] = &[
        ".msg.teams.microsoft.com",
        ".msg.skype.com",
        ".asm.skype.com",
        ".asyncgw.teams.microsoft.com",
        ".skype.com",
        ".teams.microsoft.com",
        ".teams.live.com",
    ];
    SUFFIXES.iter().any(|s| host.ends_with(s))
        || matches!(
            host.as_str(),
            "msg.teams.microsoft.com"
                | "msg.skype.com"
                | "asm.skype.com"
                | "asyncgw.teams.microsoft.com"
        )
}

/// True when the URL lives on the ASM object-store family
/// (`*.asm.skype.com`, `*.asyncgw.teams.microsoft.com`). That family
/// authenticates with `Authorization: skype_token …` (object-store scheme);
/// the chat-service scheme 401s there (om-imgfix live probe).
fn is_asm_host(url: &str) -> bool {
    let host = match host_of(url) {
        Some(h) => h,
        None => return false,
    };
    host == "asm.skype.com"
        || host.ends_with(".asm.skype.com")
        || host == "asyncgw.teams.microsoft.com"
        || host.ends_with(".asyncgw.teams.microsoft.com")
}

/// Token headers for one media URL: `(name, value)` pairs to attach.
/// ASM object-store hosts take the object-store scheme, other Microsoft
/// hosts the chat-service scheme, public hosts none (the token never
/// leaks to third parties).
pub fn auth_headers(url: &str, token: &str) -> Vec<(&'static str, String)> {
    if !needs_auth(url) {
        return Vec::new();
    }
    if is_asm_host(url) {
        return vec![("Authorization", format!("skype_token {}", token))];
    }
    vec![
        ("Authentication", format!("skypetoken={}", token)),
        ("X-SkypeToken", token.to_string()),
    ]
}

/// Lowercased host of an https URL, or None.
fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("https://")?;
    let end = rest
        .find(|c: char| c == '/' || c == '?' || c == '#' || c == '@' || c == ':')
        .unwrap_or(rest.len());
    let host = rest[..end].to_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Download one media URL. Microsoft hosts attach the Skype token;
/// public hosts fetch without auth. Rejects non-https URLs and
/// over-cap payloads.
pub async fn fetch_media_data(client: &TeamsClient, url: &str) -> Result<MediaBytes> {
    let u = url.trim();
    if !u.starts_with("https://") {
        bail!("media: only https URLs are fetched");
    }
    if host_of(u).is_none() {
        bail!("media: unparseable URL");
    }
    client.media_get(u).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_hosts_need_token() {
        assert!(needs_auth(
            "https://amer.ng.msg.teams.microsoft.com/v1/objects/0-abc/views/imgo"
        ));
        assert!(needs_auth(
            "https://us-api.asm.skype.com/v1/objects/0-abc/views/imgo"
        ));
        assert!(needs_auth(
            "https://euno-prod.asyncgw.teams.microsoft.com/v1/objects/0-abc/views/imgpsh_fullsize"
        ));
        assert!(needs_auth("https://msg.skype.com/x"));
        // Case-insensitive host.
        assert!(needs_auth("https://US-API.ASM.SKYPE.COM/v1/objects/0/x"));
    }

    #[test]
    fn public_hosts_skip_auth() {
        assert!(!needs_auth("https://media.giphy.com/media/abc/giphy.gif"));
        assert!(!needs_auth("https://example.com/photo.png"));
        assert!(!needs_auth(
            "https://statics.teams.cdn.microsoft.com/evergreen-assets/p.png"
        ));
    }

    #[test]
    fn evil_hosts_do_not_match() {
        // Suffix games must not steal the token.
        assert!(!needs_auth("https://asm.skype.com.evil.com/x"));
        assert!(!needs_auth(
            "https://amer.ng.msg.teams.microsoft.com.evil.com/x"
        ));
        assert!(!needs_auth("https://notskype.com/x"));
        assert!(!needs_auth("http://amer.ng.msg.teams.microsoft.com/x"));
        assert!(!needs_auth("not a url"));
        assert!(!needs_auth("https://"));
        // Genuine subdomains (even odd-looking ones) are first-party.
        assert!(needs_auth("https://notams.skype.com/x"));
    }

    #[test]
    fn asm_hosts_take_object_store_scheme() {
        // om-imgfix: the chat-service scheme 401s on the ASM family
        // (live probe: us-api.asm.skype.com …/views/imgo → 401).
        for u in [
            "https://us-api.asm.skype.com/v1/objects/0-eus-d1-abc/views/imgo",
            "https://api.asm.skype.com/v1/objects/0-abc/views/imgo",
            "https://us-prod.asyncgw.teams.microsoft.com/v1/objects/0-abc/views/imgo",
            "https://euno-prod.asyncgw.teams.microsoft.com/v1/objects/0-abc/views/imgpsh_fullsize",
            "https://US-API.ASM.SKYPE.COM/v1/objects/0-abc/views/imgo",
        ] {
            assert_eq!(
                auth_headers(u, "T"),
                vec![("Authorization", "skype_token T".to_string())],
                "ASM scheme for {u}"
            );
        }
    }

    #[test]
    fn chat_hosts_keep_chat_scheme() {
        for u in [
            "https://amer.ng.msg.teams.microsoft.com/v1/objects/0-abc/views/imgo",
            "https://msg.skype.com/x",
            "https://notams.skype.com/x",
        ] {
            assert_eq!(
                auth_headers(u, "T"),
                vec![
                    ("Authentication", "skypetoken=T".to_string()),
                    ("X-SkypeToken", "T".to_string()),
                ],
                "chat scheme for {u}"
            );
        }
    }

    #[test]
    fn public_and_evil_hosts_send_no_token() {
        for u in [
            "https://media.giphy.com/media/abc/giphy.gif",
            "https://asm.skype.com.evil.com/x",
            "https://us-api.asm.skype.com.evil.com/v1/objects/0/views/imgo",
            "http://us-api.asm.skype.com/x",
            "not a url",
        ] {
            assert!(auth_headers(u, "T").is_empty(), "no token for {u}");
        }
    }

    #[test]
    fn host_parsing_strips_port_and_path() {
        assert_eq!(
            host_of("https://h.example.com:8443/a?b#c").as_deref(),
            Some("h.example.com")
        );
        assert_eq!(host_of("https://H.EXAMPLE.COM/x").as_deref(), Some("h.example.com"));
        assert!(host_of("https://").is_none());
    }
}
