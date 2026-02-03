//! Trouter v4 session negotiation

use anyhow::{Context, Result};
use serde::de;
use serde::Deserialize;

fn string_or_u64<'de, D: de::Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    struct Visitor;
    impl<'de> de::Visitor<'de> for Visitor {
        type Value = u64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("u64 or stringified u64")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<u64, E> {
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<u64, E> {
            v.parse().map_err(E::custom)
        }
    }
    d.deserialize_any(Visitor)
}

#[derive(Debug, Deserialize)]
pub struct ConnectParams {
    pub sr: String,
    pub issuer: String,
    pub sp: String,
    pub se: String,
    pub st: String,
    pub sig: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    pub socketio: String,
    pub surl: String,
    pub url: String,
    #[serde(deserialize_with = "string_or_u64")]
    pub ttl: u64,
    pub connectparams: ConnectParams,
    pub ccid: Option<String>,
    #[serde(default)]
    pub registrar_url: Option<String>,
}

impl SessionResponse {
    /// Build URL query string for connectparams (URL-encoded).
    fn connectparams_query(&self) -> String {
        let cp = &self.connectparams;
        let e = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
        format!(
            "sr={}&issuer={}&sp={}&se={}&st={}&sig={}",
            e(&cp.sr),
            e(&cp.issuer),
            e(&cp.sp),
            e(&cp.se),
            e(&cp.st),
            e(&cp.sig),
        )
    }

    /// Build the socket.io session URL for GET (step 1: obtain session ID).
    pub fn session_url(&self, epid: &str) -> String {
        let host = self.socketio.trim_end_matches('/');
        let cp_query = self.connectparams_query();
        let e = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();

        let tc =
            r#"{"cv":"TEAMS_TROUTER_TCCV","ua":"TeamsCDL","hr":"","v":"TEAMS_CLIENTINFO_VERSION"}"#;

        let mut url = format!(
            "{}/socket.io/1/?v=v4&{}&tc={}&con_num=0_1&auth=true&timeout=40&epid={}",
            host,
            cp_query,
            e(tc),
            e(epid),
        );

        if let Some(ref ccid) = self.ccid {
            url.push_str(&format!("&ccid={}", e(ccid)));
        }

        url
    }

    /// Build the WebSocket URL given a session ID (step 2: connect WS).
    pub fn ws_url(&self, session_id: &str, epid: &str) -> String {
        let host = self.socketio.trim_end_matches('/');
        let cp_query = self.connectparams_query();
        let e = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();

        let tc =
            r#"{"cv":"TEAMS_TROUTER_TCCV","ua":"TeamsCDL","hr":"","v":"TEAMS_CLIENTINFO_VERSION"}"#;

        let mut url =
            format!(
            "{}/socket.io/1/websocket/{}?v=v4&{}&tc={}&con_num=0_1&auth=true&timeout=40&epid={}",
            host, session_id, cp_query, e(tc), e(epid),
        );

        if let Some(ref ccid) = self.ccid {
            url.push_str(&format!("&ccid={}", e(ccid)));
        }

        url
    }
}

/// Negotiate a Trouter session, returning connection parameters.
pub async fn negotiate(
    http: &reqwest::Client,
    skype_token: &str,
) -> Result<(SessionResponse, String)> {
    let epid = uuid::Uuid::new_v4().to_string();
    let url = format!("https://go.trouter.teams.microsoft.com/v4/a?epid={}", epid);

    tracing::info!("Negotiating trouter session (epid={})", epid);

    let resp = http
        .get(&url)
        .header("X-Skypetoken", skype_token)
        .send()
        .await
        .context("Trouter session negotiation request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Trouter session negotiation failed: {} — {}", status, body);
    }

    let session: SessionResponse = resp
        .json()
        .await
        .context("Failed to parse trouter session response")?;

    tracing::info!("Trouter session negotiated: socketio={}", session.socketio);
    tracing::debug!("Trouter surl={}", session.surl);

    Ok((session, epid))
}

/// Get a socket.io session ID by sending an authenticated GET request.
pub async fn get_session_id(
    http: &reqwest::Client,
    session: &SessionResponse,
    skype_token: &str,
    epid: &str,
) -> Result<String> {
    let url = session.session_url(epid);

    tracing::info!("Getting socket.io session ID...");
    tracing::debug!("Session URL: {}", url);

    // Squads uses a no-redirect client for this request
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Failed to build HTTP client")?;

    let resp = no_redirect
        .get(&url)
        .header("X-Skypetoken", skype_token)
        .send()
        .await
        .context("Socket.io session request failed")?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        anyhow::bail!("Socket.io session request failed: {} — {}", status, text);
    }
    tracing::debug!("Session response: {}", text);

    // Format: "{session_id}:180:180:websocket,xhr-polling"
    let session_id = text
        .split(':')
        .next()
        .context("Empty session response")?
        .to_string();

    tracing::info!("Got socket.io session ID: {}", session_id);
    Ok(session_id)
}
