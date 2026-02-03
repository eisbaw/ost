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
        app_id: "TeamsCDLWebWorker",
        template_key: "TeamsCDLWebWorker_2.6",
        path_suffix: "",
        context: "TFL",
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
    let url = registrar_url.trim_end_matches('/').to_string();

    for entry in REGISTRATIONS {
        let reg_id = uuid::Uuid::new_v4().to_string();
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
