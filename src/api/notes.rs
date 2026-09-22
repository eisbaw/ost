//! Microsoft Graph API: OneNote notebooks, sections, pages.
//!
//! Read path is plain Graph GET; page content is raw HTML (`text/html`).
//! Edit path is the OneNote PATCH-with-Commands call, hand-encoded as
//! multipart/form-data so no new dependencies are needed.
//!
//! `group_id` scopes every call: `None` reads the signed-in user's own
//! OneNote (`/me/onenote/...`), `Some(g)` reads the M365 group (team)
//! notebook (`/groups/{g}/onenote/...`).

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

// -- Wire types --

#[derive(Debug, Deserialize)]
struct ValueResponse<T> {
    value: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct WireNotebook {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireSection {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WirePage {
    id: String,
    title: Option<String>,
    #[serde(rename = "lastModifiedDateTime")]
    last_modified: Option<String>,
}

// -- Public data types --

/// OneNote notebook metadata.
pub struct NotebookInfo {
    pub id: String,
    pub name: String,
}

/// OneNote section with its pages (nested to save round trips).
pub struct SectionInfo {
    // Kept: identifies the section for API consumers (CLI prints names).
    #[allow(dead_code)]
    pub id: String,
    pub name: String,
    pub pages: Vec<PageInfo>,
}

/// OneNote page metadata (content arrives via [`read_note_page_data`]).
pub struct PageInfo {
    pub id: String,
    pub title: String,
    // Kept: surfaced to API consumers (CLI prints id + title).
    #[allow(dead_code)]
    pub updated: Option<String>,
}

/// OneNote page content: title scraped from `<title>`, raw HTML body.
pub struct NotePage {
    // Kept: identifies the page for API consumers (CLI prints title + body).
    #[allow(dead_code)]
    pub id: String,
    pub title: String,
    pub html: String,
}

// -- Pure helpers (unit-tested, no network) --

/// Graph path prefix for OneNote calls. `None`/empty reads the user's own
/// OneNote; otherwise the id must be path-safe (no `/`, no whitespace).
fn notes_base(group_id: Option<&str>) -> Result<String> {
    match group_id.map(str::trim) {
        None | Some("") => Ok("/me".to_string()),
        Some(g) => {
            if g.contains('/') || g.chars().any(|c| c.is_whitespace()) {
                bail!("group_id must not contain '/' or whitespace");
            }
            Ok(format!("/groups/{}", g))
        }
    }
}

/// Require a non-blank server id before building a request path.
fn require_id<'a>(what: &str, id: &'a str) -> Result<&'a str> {
    let id = id.trim();
    if id.is_empty() {
        bail!("empty {}", what);
    }
    Ok(id)
}

/// Scrape `<title>` from OneNote page HTML (`content` endpoint ships no
/// JSON envelope, so the listing title is unavailable here).
fn page_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = lower[start..].find("</title>")? + start;
    let title = html[start..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Plain text for the CLI: strip tags, decode common entities.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Escape plain text into one HTML paragraph body.
fn escape_para(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// OneNote PATCH body: multipart/form-data with a single `Commands` part.
/// Hand-rolled so reqwest needs no `multipart` feature.
fn multipart_commands(commands_json: &str) -> (String, Vec<u8>) {
    let boundary = format!(
        "----ostnotes-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"Commands\"\r\n\
          Content-Type: application/json\r\n\r\n",
    );
    body.extend_from_slice(commands_json.as_bytes());
    body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());
    (
        format!("multipart/form-data; boundary={}", boundary),
        body,
    )
}

/// Commands payload appending one paragraph at the end of the page body.
fn append_commands(text: &str) -> String {
    serde_json::json!([{
        "target": "body",
        "action": "append",
        "position": "after",
        "content": format!("<p>{}</p>", escape_para(text)),
    }])
    .to_string()
}

// -- Data-returning API functions --

/// List OneNote notebooks: the user's own, or the M365 group (team) ones.
pub async fn list_notebooks_data(
    client: &TeamsClient,
    group_id: Option<&str>,
) -> Result<Vec<NotebookInfo>> {
    let base = notes_base(group_id)?;
    let resp = client
        .graph_get(&format!("{}/onenote/notebooks", base))
        .await?;
    let parsed: ValueResponse<WireNotebook> = resp
        .json()
        .await
        .context("Failed to parse notebooks response")?;
    Ok(parsed
        .value
        .into_iter()
        .map(|n| NotebookInfo {
            name: n.display_name.unwrap_or_else(|| n.id.clone()),
            id: n.id,
        })
        .collect())
}

/// List a notebook's sections, each with its pages (one call per section).
/// A failed pages fetch warns and yields an empty page list (teams.rs parity).
pub async fn list_notebook_sections_data(
    client: &TeamsClient,
    notebook_id: &str,
    group_id: Option<&str>,
) -> Result<Vec<SectionInfo>> {
    let notebook_id = require_id("notebook_id", notebook_id)?;
    let base = notes_base(group_id)?;
    let resp = client
        .graph_get(&format!(
            "{}/onenote/notebooks/{}/sections",
            base, notebook_id
        ))
        .await?;
    let parsed: ValueResponse<WireSection> = resp
        .json()
        .await
        .context("Failed to parse sections response")?;

    let mut out = Vec::new();
    for section in &parsed.value {
        let name = section
            .display_name
            .as_deref()
            .unwrap_or(&section.id)
            .to_string();
        let path = format!("{}/onenote/sections/{}/pages", base, section.id);
        let pages = match client.graph_get(&path).await {
            Ok(resp) => {
                let pages: ValueResponse<WirePage> = resp
                    .json()
                    .await
                    .context("Failed to parse pages response")?;
                pages
                    .value
                    .into_iter()
                    .map(|p| PageInfo {
                        title: p.title.unwrap_or_else(|| p.id.clone()),
                        updated: p.last_modified,
                        id: p.id,
                    })
                    .collect()
            }
            Err(e) => {
                tracing::warn!("Failed to fetch pages for {}: {:#}", name, e);
                Vec::new()
            }
        };
        out.push(SectionInfo {
            id: section.id.clone(),
            name,
            pages,
        });
    }
    Ok(out)
}

/// Read one page's HTML content. Title comes from the page's `<title>`.
pub async fn read_note_page_data(
    client: &TeamsClient,
    page_id: &str,
    group_id: Option<&str>,
) -> Result<NotePage> {
    let page_id = require_id("page_id", page_id)?;
    let base = notes_base(group_id)?;
    let resp = client
        .graph_get(&format!("{}/onenote/pages/{}/content", base, page_id))
        .await?;
    let html = resp.text().await.context("Failed to read page content")?;
    let title = page_title(&html).unwrap_or_default();
    Ok(NotePage {
        id: page_id.to_string(),
        title,
        html,
    })
}

/// Append one plain-text paragraph to the end of a page.
pub async fn append_note_paragraph_data(
    client: &TeamsClient,
    page_id: &str,
    text: &str,
    group_id: Option<&str>,
) -> Result<()> {
    let page_id = require_id("page_id", page_id)?;
    if text.trim().is_empty() {
        bail!("empty text");
    }
    let base = notes_base(group_id)?;
    let (content_type, body) = multipart_commands(&append_commands(text));
    client
        .graph_patch_raw(
            &format!("{}/onenote/pages/{}/content", base, page_id),
            &content_type,
            body,
        )
        .await?;
    Ok(())
}

// -- CLI entry points (print to stdout) --

/// `teams-cli notes` dispatch: `--append` needs `--page`; page beats
/// notebook beats the default notebooks list.
pub async fn notes(
    group_id: Option<&str>,
    notebook: Option<&str>,
    page: Option<&str>,
    append: Option<&str>,
) -> Result<()> {
    if append.is_some() && page.map_or(true, |p| p.trim().is_empty()) {
        bail!("--append requires --page <id>");
    }
    let client = TeamsClient::new().await?;
    if let Some(page_id) = page {
        if let Some(text) = append {
            append_note_paragraph_data(&client, page_id, text, group_id).await?;
            println!("Paragraph appended.");
        }
        return show_note_page_with_client(&client, page_id, group_id).await;
    }
    if let Some(notebook_id) = notebook {
        return show_notebook_with_client(&client, notebook_id, group_id).await;
    }
    let notebooks = list_notebooks_data(&client, group_id).await?;
    println!("\nOneNote Notebooks:");
    println!("{:-<60}", "");
    if notebooks.is_empty() {
        println!("  (no notebooks found)");
        return Ok(());
    }
    for nb in &notebooks {
        println!("  {:<30} {}", nb.name, nb.id);
    }
    Ok(())
}

async fn show_notebook_with_client(
    client: &TeamsClient,
    notebook_id: &str,
    group_id: Option<&str>,
) -> Result<()> {
    let sections = list_notebook_sections_data(client, notebook_id, group_id).await?;
    println!("\nNotebook {}:", notebook_id);
    println!("{:-<60}", "");
    if sections.is_empty() {
        println!("  (no sections found)");
        return Ok(());
    }
    for section in &sections {
        println!("Section: {} ({} pages)", section.name, section.pages.len());
        for p in &section.pages {
            println!("  {:<30} {}", p.title, p.id);
        }
        println!();
    }
    Ok(())
}

async fn show_note_page_with_client(
    client: &TeamsClient,
    page_id: &str,
    group_id: Option<&str>,
) -> Result<()> {
    let page = read_note_page_data(client, page_id, group_id).await?;
    let title = if page.title.is_empty() {
        "(untitled)"
    } else {
        page.title.as_str()
    };
    println!("\n{}:", title);
    println!("{:-<60}", "");
    println!("{}", collapse_ws(&strip_html(&page.html)));
    Ok(())
}

/// Collapse runs of whitespace (tag-stripping leaves gaps).
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_user_and_group() {
        assert_eq!(notes_base(None).unwrap(), "/me");
        assert_eq!(notes_base(Some("")).unwrap(), "/me");
        assert_eq!(notes_base(Some("  ")).unwrap(), "/me");
        assert_eq!(
            notes_base(Some("team-guid-1")).unwrap(),
            "/groups/team-guid-1"
        );
    }

    #[test]
    fn base_rejects_path_breaking_group() {
        for bad in ["a/b", "a b", "a\tb", "a\nb"] {
            assert!(notes_base(Some(bad)).is_err(), "group {:?}", bad);
        }
    }

    #[test]
    fn require_id_rejects_blank() {
        assert!(require_id("page_id", "").is_err());
        assert!(require_id("page_id", "   ").is_err());
        assert_eq!(require_id("page_id", " p1 ").unwrap(), "p1");
    }

    #[test]
    fn notebooks_fixture_parses_with_fallback_name() {
        let body: ValueResponse<WireNotebook> = serde_json::from_str(
            r#"{"value":[
                {"id":"nb1","displayName":"Work"},
                {"id":"nb2"}]}"#,
        )
        .unwrap();
        assert_eq!(body.value.len(), 2);
        assert_eq!(body.value[0].display_name.as_deref(), Some("Work"));
        assert!(body.value[1].display_name.is_none());
    }

    #[test]
    fn sections_and_pages_fixtures_parse() {
        let sections: ValueResponse<WireSection> =
            serde_json::from_str(r#"{"value":[{"id":"s1","displayName":"Notes"}]}"#).unwrap();
        assert_eq!(sections.value[0].display_name.as_deref(), Some("Notes"));
        let pages: ValueResponse<WirePage> = serde_json::from_str(
            r#"{"value":[{"id":"p1","title":"Kickoff","lastModifiedDateTime":"2026-09-22T10:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(pages.value[0].title.as_deref(), Some("Kickoff"));
        assert_eq!(
            pages.value[0].last_modified.as_deref(),
            Some("2026-09-22T10:00:00Z")
        );
    }

    #[test]
    fn page_title_scrapes_head_title() {
        let html = "<html><head><title>Kickoff Notes</title></head><body><p>hi</p></body></html>";
        assert_eq!(page_title(html).as_deref(), Some("Kickoff Notes"));
        assert!(page_title("<html><body>no title</body></html>").is_none());
        assert!(page_title("<title>   </title>").is_none());
        // Case-insensitive tags.
        assert_eq!(
            page_title("<HTML><HEAD><TITLE>Up</TITLE></HEAD></HTML>").as_deref(),
            Some("Up")
        );
    }

    #[test]
    fn append_commands_shape() {
        let v: serde_json::Value =
            serde_json::from_str(&append_commands("a<b>&c")).unwrap();
        assert_eq!(v[0]["target"], "body");
        assert_eq!(v[0]["action"], "append");
        assert_eq!(v[0]["position"], "after");
        assert_eq!(v[0]["content"], "<p>a&lt;b&gt;&amp;c</p>");
    }

    #[test]
    fn multipart_body_wraps_commands() {
        let (content_type, body) = multipart_commands(r#"[{"a":1}]"#);
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(&format!("--{}\r\n", boundary)));
        assert!(text.contains("name=\"Commands\""));
        assert!(text.contains(r#"[{"a":1}]"#));
        assert!(text.ends_with(&format!("--{}--\r\n", boundary)));
    }

    #[test]
    fn strip_and_collapse_for_cli() {
        let html = "<html><head><title>T</title></head><body><h1>Hi</h1><p>a &amp; b</p></body></html>";
        assert_eq!(collapse_ws(&strip_html(html)), "T Hi a & b");
    }
}
