//! Trouter registrar — registers our endpoint with the Teams notification service

use anyhow::{Context, Result};

/// Registration entry: appId, templateKey, path suffix, context
struct RegEntry {
    app_id: &'static str,
    template_key: &'static str,
    path_suffix: &'static str,
    context: &'static str,
}

const REGISTRATIONS: &[RegEntry] = &[
    RegEntry {
        // Business-tenant CDL registration (template 2.1, empty
        // transport context). Was 2.6 + transport context "TFL" (the
        // consumer "Teams for Life" product context, and in the wrong
        // field): registrations succeeded but no chat-service messaging
        // push ever reached the socket.
        app_id: "TeamsCDLWebWorker",
        template_key: "TeamsCDLWebWorker_2.1",
        path_suffix: "",
        context: "",
    },
    RegEntry {
        app_id: "SkypeSpacesWeb",
        template_key: "SkypeSpacesWeb_2.4",
        path_suffix: "SkypeSpacesWeb",
        context: "",
    },
    RegEntry {
        app_id: "NextGenCalling",
        template_key: "DesktopNgc_2.5:SkypeNgc",
        path_suffix: "NGCallManagerWin",
        context: "",
    },
];

/// Register our trouter endpoint with the Teams registrar service.
///
/// Performs three separate registrations (TeamsCDLWebWorker, SkypeSpacesWeb,
/// NextGenCalling) as the real Teams client does.
pub async fn register(
    http: &reqwest::Client,
    skype_token: &str,
    registrar_url: &str,
    trouter_surl: &str,
) -> Result<()> {
    register_with_endpoint(http, skype_token, registrar_url, trouter_surl, None).await
}

/// Like [`register`], but the CDL (chat) registration's
/// `registrationId` is the socket's endpoint id (`epid`), as the Teams
/// clients do; other apps keep a random id.
pub async fn register_with_endpoint(
    http: &reqwest::Client,
    skype_token: &str,
    registrar_url: &str,
    trouter_surl: &str,
    epid: Option<&str>,
) -> Result<()> {
    let url = registrar_url.trim_end_matches('/').to_string();

    for entry in REGISTRATIONS {
        let reg_id = match epid {
            Some(e) if entry.app_id == "TeamsCDLWebWorker" && !e.trim().is_empty() => e.to_string(),
            _ => uuid::Uuid::new_v4().to_string(),
        };
        let path = format!("{}{}", trouter_surl, entry.path_suffix);

        let payload = serde_json::json!({
            "clientDescription": {
                "appId": entry.app_id,
                "aesKey": "",
                "languageId": "en-US",
                "platform": "edge",
                "templateKey": entry.template_key,
                "platformUIVersion": "49/1.0.0"
            },
            "registrationId": reg_id,
            "nodeId": "",
            "transports": {
                "TROUTER": [{
                    "context": entry.context,
                    "path": path,
                    "ttl": 86400
                }]
            }
        });

        tracing::info!(
            "Registering {} at {} (appId={}, templateKey={})",
            entry
                .path_suffix
                .is_empty()
                .then_some("base")
                .unwrap_or(entry.path_suffix),
            url,
            entry.app_id,
            entry.template_key,
        );

        let resp = http
            .post(&url)
            .header("X-Skypetoken", skype_token)
            .json(&payload)
            .send()
            .await
            .context("Registrar POST failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Registrar {} returned {}: {}", entry.app_id, status, body);
        }
        tracing::info!("Registrar {} registration succeeded", entry.app_id);
    }

    Ok(())
}

/// Re-register only the CDL (chat) entry under `template_key`
/// with `registrationId` = `epid` (same id → replaces the first one). The
/// Teams clients do this with `TeamsCDLWebWorker_1.9` when the server
/// reports `trouter.message_loss` right after connect.
pub async fn register_cdl(
    http: &reqwest::Client,
    skype_token: &str,
    registrar_url: &str,
    trouter_surl: &str,
    epid: &str,
    template_key: &str,
) -> Result<()> {
    let payload = serde_json::json!({
        "clientDescription": {
            "appId": "TeamsCDLWebWorker",
            "aesKey": "",
            "languageId": "en-US",
            "platform": "edge",
            "templateKey": template_key,
            "platformUIVersion": "49/1.0.0"
        },
        "registrationId": epid,
        "nodeId": "",
        "transports": {"TROUTER": [{"context": "", "path": trouter_surl, "ttl": 86400}]}
    });
    let resp = http
        .post(registrar_url.trim_end_matches('/'))
        .header("X-Skypetoken", skype_token)
        .json(&payload)
        .send()
        .await
        .context("Registrar POST failed")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("Registrar CDL {} returned {}", template_key, status);
    }
    tracing::info!("Registrar CDL {} registration succeeded", template_key);
    Ok(())
}
