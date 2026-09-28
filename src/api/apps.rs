//! Teams app catalog (apps-platform middle tier): installed/pinned
//! entitlements and app definitions (manifests), so a host can open a
//! personal app's static tab without the Teams web shell.
//!
//! Endpoints (from the public Teams web shell bundles; response shapes
//! are INFERRED and parsed leniently, not verified against a tenant):
//! - `GET  {mt}/beta/users/apps/entitlements`
//! - `POST {mt}/beta/users/apps/aggregatedEntitlements?appbarview=userpinned`
//! - `POST {mt}/beta/users/apps/batchedDefinitions`
//!
//! `{mt}` is `region_gtms.middleTier`. Auth is the Teams AAD token
//! (Bearer) plus `X-Skypetoken` (see `TeamsClient::mt_get`).

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};

use super::client::TeamsClient;

/// One static (personal) tab of an app manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaticTab {
    pub entity_id: String,
    pub name: String,
    pub content_url: Option<String>,
    pub website_url: Option<String>,
    pub scopes: Vec<String>,
}

/// One configurable (channel/group chat) tab of an app manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigurableTab {
    pub configuration_url: String,
    pub can_update_configuration: bool,
    pub scopes: Vec<String>,
}

/// `webApplicationInfo`: the AAD app the host mints SSO tokens for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebApplicationInfo {
    pub id: String,
    pub resource: Option<String>,
}

/// The hostable part of one app definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppManifest {
    pub id: String,
    pub name: String,
    pub short_description: Option<String>,
    pub developer: Option<String>,
    pub version: Option<String>,
    pub color_icon: Option<String>,
    pub outline_icon: Option<String>,
    pub accent_color: Option<String>,
    pub static_tabs: Vec<StaticTab>,
    pub configurable_tabs: Vec<ConfigurableTab>,
    pub web_application_info: Option<WebApplicationInfo>,
    pub valid_domains: Vec<String>,
    /// Long description (store detail page).
    pub full_description: Option<String>,
    /// Manifest `bots` / `composeExtensions` present (detail capabilities).
    pub has_bot: bool,
    pub has_messaging_extension: bool,
    /// Manifest `permissions` plus resource-specific consent names.
    pub permissions: Vec<String>,
    /// Store categories (`categories` / `category`).
    pub categories: Vec<String>,
    pub website_url: Option<String>,
    pub privacy_url: Option<String>,
    pub terms_of_use_url: Option<String>,
}

/// One store section (a titled shelf of app ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreSection {
    pub title: String,
    pub app_ids: Vec<String>,
}

/// Store / app library browse data.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AppStore {
    pub sections: Vec<StoreSection>,
    pub apps: Vec<AppManifest>,
}

/// One installed app for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppEntitlement {
    pub app_id: String,
    pub state: Option<String>,
    pub pinned: bool,
}

/// Installed + pinned apps and their manifests.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AppCatalog {
    pub entitlements: Vec<AppEntitlement>,
    /// App bar order (userpinned view), app ids.
    pub pinned: Vec<String>,
    pub apps: Vec<AppManifest>,
}

// MARK: - URLs and bodies (pure)

pub fn entitlements_url(mt: &str) -> String {
    format!("{}/beta/users/apps/entitlements", mt.trim_end_matches('/'))
}

pub fn pinned_url(mt: &str) -> String {
    format!(
        "{}/beta/users/apps/aggregatedEntitlements?appbarview=userpinned",
        mt.trim_end_matches('/')
    )
}

pub fn definitions_url(mt: &str) -> String {
    format!(
        "{}/beta/users/apps/batchedDefinitions?includeCopilotPlugins=false&includeblockedapps=false",
        mt.trim_end_matches('/')
    )
}

/// aggregatedEntitlements body: empty hashes = "send everything".
pub fn pinned_body() -> Value {
    json!([{ "userEntitlementsHash": "", "teamEntitlementsHash": "" }])
}

/// batchedDefinitions body (INFERRED shape): the app ids to resolve.
pub fn definitions_body(ids: &[String]) -> Value {
    json!({ "appIds": ids })
}

/// Store home (INFERRED shape: shelves of apps, lenient parser).
pub fn store_url(mt: &str) -> String {
    format!("{}/beta/users/apps/store", mt.trim_end_matches('/'))
}

/// Store search (INFERRED query parameter name).
pub fn search_url(mt: &str, query: &str) -> String {
    let q: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
    format!("{}/beta/users/apps/search?query={}", mt.trim_end_matches('/'), q)
}

/// Graph personal-scope install (documented: `POST /me/teamwork/installedApps`).
pub const INSTALL_PATH: &str = "/me/teamwork/installedApps";

/// Graph install body for a catalog app id.
pub fn install_body(app_id: &str) -> Value {
    json!({
        "teamsApp@odata.bind": format!("https://graph.microsoft.com/v1.0/appCatalogs/teamsApps/{}", app_id.trim())
    })
}

/// Definitions per batchedDefinitions call.
pub const DEFINITIONS_BATCH: usize = 50;

// MARK: - Parsing (pure, lenient)

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    })
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Collects every object (depth-first) that carries one of `id_keys`
/// and satisfies `accept`: the MT responses wrap their lists
/// differently per endpoint (`value`, `appEntitlements`, `definitions`,
/// per-scope maps), so the parsers do not depend on the wrapper.
fn collect<'a>(v: &'a Value, accept: &dyn Fn(&Value) -> bool, out: &mut Vec<&'a Value>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect(x, accept, out)),
        Value::Object(m) => {
            if accept(v) {
                out.push(v);
                return;
            }
            m.values().for_each(|x| collect(x, accept, out));
        }
        _ => {}
    }
}

/// Entitlements (installed apps) from any MT entitlements response.
/// Deduplicated by app id, first occurrence wins; a `pinned`/`isPinned`
/// flag on any duplicate marks the app pinned.
pub fn parse_entitlements(v: &Value) -> Vec<AppEntitlement> {
    let mut found = Vec::new();
    collect(v, &|o| str_field(o, &["appId"]).is_some(), &mut found);
    let mut out: Vec<AppEntitlement> = Vec::new();
    for o in found {
        let Some(app_id) = str_field(o, &["appId"]) else { continue };
        let pinned = ["pinned", "isPinned", "isAppBarPinned"]
            .iter()
            .any(|k| o.get(*k).and_then(Value::as_bool).unwrap_or(false));
        if let Some(e) = out.iter_mut().find(|e| e.app_id.eq_ignore_ascii_case(&app_id)) {
            e.pinned |= pinned;
            continue;
        }
        out.push(AppEntitlement {
            app_id,
            state: str_field(o, &["state", "installationState"]),
            pinned,
        });
    }
    out
}

/// Pinned app ids in app bar order from the userpinned view.
pub fn parse_pinned(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in parse_entitlements(v) {
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&e.app_id)) {
            out.push(e.app_id);
        }
    }
    out
}

fn name_of(o: &Value) -> Option<String> {
    match o.get("name") {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(n @ Value::Object(_)) => str_field(n, &["short", "full"]),
        _ => str_field(o, &["shortName", "displayName", "title"]),
    }
}

fn static_tab(t: &Value) -> Option<StaticTab> {
    let entity_id = str_field(t, &["entityId"])?;
    Some(StaticTab {
        name: str_field(t, &["name"]).unwrap_or_default(),
        content_url: str_field(t, &["contentUrl"]),
        website_url: str_field(t, &["websiteUrl"]),
        scopes: str_list(t, "scopes"),
        entity_id,
    })
}

fn configurable_tab(t: &Value) -> Option<ConfigurableTab> {
    Some(ConfigurableTab {
        configuration_url: str_field(t, &["configurationUrl"])?,
        can_update_configuration: t
            .get("canUpdateConfiguration")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        scopes: str_list(t, "scopes"),
    })
}

fn developer_url(o: &Value, key: &str) -> Option<String> {
    o.get("developer").and_then(|d| str_field(d, &[key])).or_else(|| str_field(o, &[key]))
}

fn manifest(o: &Value) -> Option<AppManifest> {
    let id = str_field(o, &["id", "appId"])?;
    let list = |key: &str| o.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let icons = o.get("icons");
    let developer = o
        .get("developer")
        .and_then(|d| str_field(d, &["name"]))
        .or_else(|| str_field(o, &["developerName"]));
    let description = match o.get("description") {
        Some(d @ Value::Object(_)) => str_field(d, &["short", "full"]),
        _ => str_field(o, &["shortDescription"]),
    };
    let wai = o.get("webApplicationInfo").and_then(|w| {
        Some(WebApplicationInfo {
            id: str_field(w, &["id"])?,
            resource: str_field(w, &["resource"]),
        })
    });
    Some(AppManifest {
        name: name_of(o).unwrap_or_else(|| id.clone()),
        short_description: description,
        developer,
        version: str_field(o, &["version"]),
        color_icon: icons
            .and_then(|i| str_field(i, &["color"]))
            .or_else(|| str_field(o, &["largeImageUrl", "colorIcon"])),
        outline_icon: icons
            .and_then(|i| str_field(i, &["outline"]))
            .or_else(|| str_field(o, &["smallImageUrl", "outlineIcon"])),
        accent_color: str_field(o, &["accentColor"]),
        static_tabs: list("staticTabs").iter().filter_map(static_tab).collect(),
        configurable_tabs: list("configurableTabs")
            .iter()
            .chain(list("galleryTabs").iter())
            .filter_map(configurable_tab)
            .collect(),
        web_application_info: wai,
        valid_domains: str_list(o, "validDomains"),
        full_description: match o.get("description") {
            Some(d @ Value::Object(_)) => str_field(d, &["full"]),
            _ => str_field(o, &["longDescription", "fullDescription"]),
        },
        has_bot: o.get("bots").and_then(Value::as_array).is_some_and(|a| !a.is_empty()),
        has_messaging_extension: ["composeExtensions", "inputExtensions"]
            .iter()
            .any(|k| o.get(*k).and_then(Value::as_array).is_some_and(|a| !a.is_empty())),
        permissions: {
            let mut p = str_list(o, "permissions");
            let rsc = o
                .pointer("/authorization/permissions/resourceSpecific")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            p.extend(rsc.iter().filter_map(|r| str_field(r, &["name"])));
            p.dedup();
            p
        },
        categories: {
            let mut c = str_list(o, "categories");
            if let Some(one) = str_field(o, &["category"]) {
                c.push(one);
            }
            c
        },
        website_url: developer_url(o, "websiteUrl"),
        privacy_url: developer_url(o, "privacyUrl"),
        terms_of_use_url: developer_url(o, "termsOfUseUrl"),
        id,
    })
}

/// Manifests from a batchedDefinitions/definitions response: every
/// object that has an id plus a manifest marker (`staticTabs`,
/// `configurableTabs`, `galleryTabs`, `bots`, `webApplicationInfo`,
/// `manifestVersion`, `validDomains`). Deduplicated by id.
pub fn parse_definitions(v: &Value) -> Vec<AppManifest> {
    const MARKERS: [&str; 7] = [
        "staticTabs",
        "configurableTabs",
        "galleryTabs",
        "bots",
        "webApplicationInfo",
        "manifestVersion",
        "validDomains",
    ];
    let mut found = Vec::new();
    collect(
        v,
        &|o| str_field(o, &["id", "appId"]).is_some() && MARKERS.iter().any(|k| o.get(*k).is_some()),
        &mut found,
    );
    let mut out: Vec<AppManifest> = Vec::new();
    for m in found.into_iter().filter_map(manifest) {
        if !out.iter().any(|x| x.id.eq_ignore_ascii_case(&m.id)) {
            out.push(m);
        }
    }
    out
}

/// Store apps: any object with an id and a name plus a listing marker
/// (description, icons, categories or a manifest marker). Store
/// listings carry less than a definition; missing fields stay empty.
pub fn parse_store_apps(v: &Value) -> Vec<AppManifest> {
    const MARKERS: [&str; 12] = [
        "shortDescription",
        "description",
        "icons",
        "largeImageUrl",
        "categories",
        "category",
        "staticTabs",
        "configurableTabs",
        "bots",
        "composeExtensions",
        "manifestVersion",
        "validDomains",
    ];
    let mut found = Vec::new();
    collect(
        v,
        &|o| {
            str_field(o, &["id", "appId"]).is_some()
                && name_of(o).is_some()
                && MARKERS.iter().any(|k| o.get(*k).is_some())
        },
        &mut found,
    );
    let mut out: Vec<AppManifest> = Vec::new();
    for m in found.into_iter().filter_map(manifest) {
        if !out.iter().any(|x| x.id.eq_ignore_ascii_case(&m.id)) {
            out.push(m);
        }
    }
    out
}

/// Store shelves: objects with a title and an array of apps (objects
/// with an id, or bare id strings) under `apps`/`items`/`appIds`.
pub fn parse_store_sections(v: &Value) -> Vec<StoreSection> {
    let mut found = Vec::new();
    collect(
        v,
        &|o| {
            str_field(o, &["title", "displayName", "name"]).is_some()
                && ["apps", "items", "appIds"].iter().any(|k| o.get(*k).and_then(Value::as_array).is_some())
        },
        &mut found,
    );
    found
        .into_iter()
        .filter_map(|o| {
            let title = str_field(o, &["title", "displayName", "name"])?;
            let list = ["apps", "items", "appIds"].iter().find_map(|k| o.get(*k).and_then(Value::as_array))?;
            let app_ids: Vec<String> = list
                .iter()
                .filter_map(|x| match x {
                    Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                    _ => str_field(x, &["id", "appId"]),
                })
                .collect();
            (!app_ids.is_empty()).then_some(StoreSection { title, app_ids })
        })
        .collect()
}

// MARK: - Network

/// Installed apps, pinned order, and their manifests (read-only).
/// The pinned view is best effort: a failure there leaves `pinned` to
/// the entitlement flags.
pub async fn app_catalog_data(client: &TeamsClient) -> Result<AppCatalog> {
    let mt = client.middle_tier_url();
    let ent: Value = client.mt_get(&entitlements_url(&mt)).await?.json().await?;
    let entitlements = parse_entitlements(&ent);
    let pinned = match client.mt_post(&pinned_url(&mt), &pinned_body()).await {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => parse_pinned(&v),
            Err(_) => Vec::new(),
        },
        Err(e) => {
            tracing::warn!("apps: pinned view failed: {:#}", e);
            Vec::new()
        }
    };
    let pinned = if pinned.is_empty() {
        entitlements.iter().filter(|e| e.pinned).map(|e| e.app_id.clone()).collect()
    } else {
        pinned
    };
    let mut ids: Vec<String> = entitlements.iter().map(|e| e.app_id.clone()).collect();
    for p in &pinned {
        if !ids.iter().any(|i| i.eq_ignore_ascii_case(p)) {
            ids.push(p.clone());
        }
    }
    let mut apps = Vec::new();
    for chunk in ids.chunks(DEFINITIONS_BATCH) {
        let v: Value = client
            .mt_post(&definitions_url(&mt), &definitions_body(chunk))
            .await?
            .json()
            .await?;
        apps.extend(parse_definitions(&v));
    }
    Ok(AppCatalog { entitlements, pinned, apps })
}

/// Store home: shelves + listed apps (read-only).
pub async fn app_store_data(client: &TeamsClient) -> Result<AppStore> {
    let v: Value = client.mt_get(&store_url(&client.middle_tier_url())).await?.json().await?;
    Ok(AppStore { sections: parse_store_sections(&v), apps: parse_store_apps(&v) })
}

/// Store search (read-only).
pub async fn app_search_data(client: &TeamsClient, query: &str) -> Result<Vec<AppManifest>> {
    if query.trim().is_empty() {
        anyhow::bail!("empty query");
    }
    let v: Value = client.mt_get(&search_url(&client.middle_tier_url(), query)).await?.json().await?;
    Ok(parse_store_apps(&v))
}

/// Installs a catalog app for the signed-in user (REMOTE WRITE: the
/// tenant sees a new personal install). Callers must confirm first.
pub async fn install_app_for_user(client: &TeamsClient, app_id: &str) -> Result<()> {
    if app_id.trim().is_empty() {
        anyhow::bail!("empty app id");
    }
    client.graph_post(INSTALL_PATH, &install_body(app_id)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped per the APPHOST spike (field names from the shell's
    /// resolver chunk); values are public sample data.
    fn definitions_fixture() -> Value {
        json!({
            "definitions": [
                {
                    "id": "0d820ecd-def2-4297-adad-78056cde7c78",
                    "manifestVersion": "1.16",
                    "version": "1.2.0",
                    "name": { "short": "Sample Tasks", "full": "Sample Tasks for Teams" },
                    "description": { "short": "Track tasks", "full": "Track your tasks." },
                    "developer": { "name": "Contoso" },
                    "icons": { "color": "https://cdn.example.com/c.png", "outline": "https://cdn.example.com/o.png" },
                    "accentColor": "#6264A7",
                    "staticTabs": [
                        {
                            "entityId": "home",
                            "name": "Home",
                            "contentUrl": "https://tasks.example.com/tab?tid={tid}&locale={locale}",
                            "websiteUrl": "https://tasks.example.com/",
                            "scopes": ["personal"]
                        },
                        { "entityId": "about", "scopes": ["personal"] },
                        { "name": "no entity id is skipped" }
                    ],
                    "configurableTabs": [
                        { "configurationUrl": "https://tasks.example.com/config", "canUpdateConfiguration": true, "scopes": ["team", "groupchat"] }
                    ],
                    "webApplicationInfo": { "id": "11111111-2222-3333-4444-555555555555", "resource": "api://tasks.example.com/11111111-2222-3333-4444-555555555555" },
                    "validDomains": ["tasks.example.com", "*.example.com", ""]
                },
                {
                    "appId": "legacy-app",
                    "shortName": "Legacy",
                    "largeImageUrl": "https://cdn.example.com/l.png",
                    "galleryTabs": [{ "configurationUrl": "https://legacy.example.com/c" }]
                },
                { "id": "0d820ecd-def2-4297-adad-78056cde7c78", "staticTabs": [] }
            ]
        })
    }

    #[test]
    fn definitions_parse_manifest_fields() {
        let apps = parse_definitions(&definitions_fixture());
        assert_eq!(apps.len(), 2, "dedup by id + legacy shape");
        let a = &apps[0];
        assert_eq!(a.name, "Sample Tasks");
        assert_eq!(a.short_description.as_deref(), Some("Track tasks"));
        assert_eq!(a.developer.as_deref(), Some("Contoso"));
        assert_eq!(a.color_icon.as_deref(), Some("https://cdn.example.com/c.png"));
        assert_eq!(a.static_tabs.len(), 2);
        assert_eq!(a.static_tabs[0].entity_id, "home");
        assert_eq!(
            a.static_tabs[0].content_url.as_deref(),
            Some("https://tasks.example.com/tab?tid={tid}&locale={locale}")
        );
        assert_eq!(a.static_tabs[0].scopes, vec!["personal"]);
        assert_eq!(a.static_tabs[1].content_url, None);
        assert_eq!(a.configurable_tabs.len(), 1);
        assert!(a.configurable_tabs[0].can_update_configuration);
        let w = a.web_application_info.as_ref().expect("wai");
        assert_eq!(w.id, "11111111-2222-3333-4444-555555555555");
        assert!(w.resource.as_deref().unwrap().starts_with("api://tasks.example.com/"));
        assert_eq!(a.valid_domains, vec!["tasks.example.com", "*.example.com"]);
        let l = &apps[1];
        assert_eq!((l.id.as_str(), l.name.as_str()), ("legacy-app", "Legacy"));
        assert_eq!(l.color_icon.as_deref(), Some("https://cdn.example.com/l.png"));
        assert_eq!(l.configurable_tabs.len(), 1);
    }

    #[test]
    fn entitlements_and_pinned_parse_any_wrapper() {
        let ent = json!({
            "appEntitlements": [
                { "appId": "A", "state": "Installed" },
                { "appId": "b", "state": "Installed", "isPinned": true },
                { "appId": "B", "pinned": false },
                { "notAnApp": true }
            ]
        });
        let e = parse_entitlements(&ent);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].state.as_deref(), Some("Installed"));
        assert!(!e[0].pinned && e[1].pinned);

        let pinned = json!([{ "users": { "appEntitlements": [
            { "appId": "chat" }, { "appId": "Z" }, { "appId": "A" }, { "appId": "z" }
        ] } }]);
        assert_eq!(parse_pinned(&pinned), vec!["chat", "Z", "A"]);
        assert!(parse_entitlements(&json!({"value": []})).is_empty());
    }

    #[test]
    fn urls_and_bodies() {
        let mt = "https://teams.microsoft.com/api/mt/emea/";
        assert_eq!(entitlements_url(mt), "https://teams.microsoft.com/api/mt/emea/beta/users/apps/entitlements");
        assert!(pinned_url(mt).ends_with("/beta/users/apps/aggregatedEntitlements?appbarview=userpinned"));
        assert!(definitions_url(mt).contains("/beta/users/apps/batchedDefinitions?"));
        assert_eq!(definitions_body(&["x".into()]), json!({"appIds": ["x"]}));
        assert!(pinned_body().is_array());
    }

    #[test]
    fn store_parse_sections_detail_fields_and_install_body() {
        let v = json!({
            "sections": [
                { "title": "Popular", "apps": [{ "id": "app-a" }, "app-b"] },
                { "title": "Empty", "items": [] }
            ],
            "apps": [
                {
                    "id": "app-a",
                    "name": { "short": "Board", "full": "Sprint Board" },
                    "description": { "short": "Plan sprints", "full": "Plan sprints with your team." },
                    "developer": { "name": "Northwind Labs", "websiteUrl": "https://northwind.example",
                                   "privacyUrl": "https://northwind.example/privacy" },
                    "categories": ["Productivity"],
                    "bots": [{ "botId": "b" }],
                    "composeExtensions": [],
                    "permissions": ["identity"],
                    "authorization": { "permissions": { "resourceSpecific": [{ "name": "ChannelMessage.Read.Group" }] } },
                    "validDomains": ["northwind.example"]
                },
                { "appId": "app-b", "displayName": "Polls", "shortDescription": "Quick polls", "category": "Utilities" }
            ]
        });
        let sections = parse_store_sections(&v);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Popular");
        assert_eq!(sections[0].app_ids, vec!["app-a", "app-b"]);
        let apps = parse_store_apps(&v);
        assert_eq!(apps.len(), 2);
        let a = &apps[0];
        assert_eq!(a.name, "Board");
        assert_eq!(a.full_description.as_deref(), Some("Plan sprints with your team."));
        assert!(a.has_bot);
        assert!(!a.has_messaging_extension);
        assert_eq!(a.permissions, vec!["identity", "ChannelMessage.Read.Group"]);
        assert_eq!(a.categories, vec!["Productivity"]);
        assert_eq!(a.privacy_url.as_deref(), Some("https://northwind.example/privacy"));
        assert_eq!(apps[1].categories, vec!["Utilities"]);
        assert_eq!(apps[1].short_description.as_deref(), Some("Quick polls"));
        assert_eq!(
            search_url("https://mt.example/api/mt/amer/", "sprint board"),
            "https://mt.example/api/mt/amer/beta/users/apps/search?query=sprint+board"
        );
        assert_eq!(
            install_body("app-a")["teamsApp@odata.bind"],
            "https://graph.microsoft.com/v1.0/appCatalogs/teamsApps/app-a"
        );
    }
}
