//! om-imgfix: ONE read-only live probe of an AMS inline-image URL.
//!
//! Ignored by default; run explicitly against the existing signed-in
//! session (no login, no refresh — the token is loaded straight from the
//! on-disk session, so this test only ever issues GETs):
//!   cargo test -j2 --test imgfix_live_probe -- --ignored --nocapture
//!
//! Harvest (read-only): newest history pages until the first AMS `<img>`
//! URL. Probe: a SINGLE GET with the exact headers `media_get` sends,
//! redirects disabled, so the first-hop status/headers/size discriminate
//! the failure cause (401 auth vs 3xx redirect vs 200 shape). Response
//! bytes are measured then discarded; logs carry status/size/shape
//! booleans only — never URLs with object ids, never message text.

use ost::api::client::TeamsClient;
use ost::api::{list_chats_data, read_messages_data};

/// Max chats whose newest page is scanned for an `<img>` URL.
const MAX_HARVEST_CHATS: usize = 6;
/// Newest-page size per harvest read.
const HARVEST_PAGE: usize = 25;

/// First `https://` `<img src>` in raw HTML, if any. Case-insensitive
/// tag/attr scan; handles quoted and bare values.
fn first_img_src(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut base = 0usize;
    loop {
        let i = rest.find("<img")?;
        let tag_start = base + i;
        let tag_end = raw[tag_start..].find('>')? + tag_start;
        let tag_lower = &lower[tag_start..tag_end];
        if let Some(v) = attr_src(tag_lower, &raw[tag_start..tag_end]) {
            if v.starts_with("https://") {
                return Some(v);
            }
        }
        base = tag_end + 1;
        rest = &lower[base.min(lower.len())..];
    }
}

/// Value of the `src` attribute within one (lowercased, original) tag pair.
fn attr_src(tag_lower: &str, tag_orig: &str) -> Option<String> {
    let mut r = tag_lower;
    let mut off = 0usize;
    loop {
        let i = r.find("src")?;
        // Attribute boundary: preceding char must not be [a-z-].
        let ok = i == 0
            || !matches!(
                r.as_bytes()[i - 1],
                b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'_'
            );
        let abs = off + i;
        r = &r[i + 3..];
        off = abs + 3;
        if !ok {
            continue;
        }
        let after = tag_orig[abs + 3..].trim_start();
        let val = after.strip_prefix('=')?.trim_start();
        if let Some(q) = val.strip_prefix(['"', '\'']) {
            let quote = val.as_bytes()[0];
            let end = q.find(quote as char)?;
            return Some(q[..end].to_string());
        }
        let end = val
            .find(|c: char| c.is_whitespace() || c == '>' || c == '"' || c == '\'')
            .unwrap_or(val.len());
        if end == 0 {
            continue;
        }
        return Some(val[..end].to_string());
    }
}

/// `host + /…/views/<view>` — object ids never reach the log.
fn redact_url(url: &str) -> String {
    let host = url
        .strip_prefix("https://")
        .and_then(|r| r.find('/').map(|i| &r[..i]))
        .unwrap_or("?");
    let view = url.rsplit('/').next().unwrap_or("?");
    let view = view.split(['?', '#']).next().unwrap_or(view);
    format!("{host}/…/views/{view}")
}

#[tokio::test]
#[ignore]
async fn live_probe_one_ams_get() {
    // Existing session, loaded straight from disk: no refresh POST, the
    // harvest below is GET-only.
    let cfg = ost::config::Config::load().expect("load session config");
    let skype = cfg
        .get_skype_token()
        .expect("signed-in session required (no skype token)");
    assert!(!skype.is_expired(), "skype token expired; re-login first");
    assert!(!skype.token.is_empty(), "empty skype token");
    eprintln!("session: skype token present, len={}", skype.token.len());

    let client = TeamsClient::new().await.expect("TeamsClient::new");

    // Harvest: newest page per chat until the first AMS image URL.
    let chats = list_chats_data(&client, 20).await.expect("list chats");
    assert!(!chats.is_empty(), "no chats in session");
    let mut found: Option<String> = None;
    let mut reads = 0usize;
    for c in chats.iter().take(MAX_HARVEST_CHATS) {
        let msgs = read_messages_data(&client, &c.id, HARVEST_PAGE)
            .await
            .expect("read newest page");
        reads += 1;
        for m in msgs.iter().rev() {
            // Newest first (`read_messages_data` returns oldest-first).
            if let Some(u) = first_img_src(&m.raw) {
                if ost::api::media::needs_auth(&u) {
                    found = Some(u);
                    break;
                }
            }
        }
        if found.is_some() {
            break;
        }
    }
    let url = found.expect("no AMS <img> URL in harvested pages");
    eprintln!(
        "harvest: {} chats listed, {} pages read, probe url={}",
        chats.len(),
        reads,
        redact_url(&url)
    );

    // THE probe: one GET, redirects disabled, exact production headers
    // (`media::auth_headers`, the same mapping `media_get` sends).
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("probe client");
    let mut req = http.get(&url);
    for (name, value) in ost::api::media::auth_headers(&url, &skype.token) {
        req = req.header(name, value);
    }
    let resp = req.send().await.expect("probe GET sends");
    let status = resp.status();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("?")
        .to_string();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(redact_url)
        .unwrap_or_else(|| "-".to_string());
    let body = resp.bytes().await.expect("probe body reads");
    let len = body.len();
    let is_image_ct = ctype.starts_with("image/");
    let magic_image = body.starts_with(b"\x89PNG")
        || body.starts_with(b"\xff\xd8\xff")
        || body.starts_with(b"GIF87a")
        || body.starts_with(b"GIF89a")
        || (body.starts_with(b"RIFF") && body.len() > 12 && &body[8..12] == b"WEBP");
    // Bytes measured, now dropped: only booleans reach the log.
    drop(body);
    eprintln!(
        "probe: status={} content_type={} len={} location={} image_ct={} magic_image={}",
        status.as_u16(),
        ctype,
        len,
        location,
        is_image_ct,
        magic_image
    );

    assert!(
        status.is_success(),
        "live AMS fetch fails at first hop: status={} location={}",
        status.as_u16(),
        location
    );
    assert!(len > 0, "live AMS fetch returned empty body");
    assert!(
        len <= ost::api::MAX_BYTES,
        "live AMS fetch exceeds {}-byte cap: {}",
        ost::api::MAX_BYTES,
        len
    );
    assert!(
        is_image_ct || magic_image,
        "live AMS fetch shape is not an image: content_type={}",
        ctype
    );
}

/// Read-only attribution helper (ignored, live): reports the redacted
/// host+view of the first `<img>` in the chat whose name contains
/// `CHAT_MATCH`, to attribute a screenshot back to the ASM/chat family.
/// Two GETs max (list + one newest page). Prints no names, no ids.
#[tokio::test]
#[ignore]
async fn live_attribute_img_host() {
    let needle =
        std::env::var("IMG_MATCH").unwrap_or_else(|_| "MyChart".to_string());
    let client = TeamsClient::new().await.expect("TeamsClient::new");
    let chats = list_chats_data(&client, 20).await.expect("list chats");
    let chat = chats
        .iter()
        .find(|c| c.name.contains(needle.as_str()))
        .expect("matching chat in list");
    let msgs = read_messages_data(&client, &chat.id, 25)
        .await
        .expect("read newest page");
    let mut found = None;
    for m in msgs.iter().rev() {
        if let Some(u) = first_img_src(&m.raw) {
            found = Some((redact_url(&u), ost::api::media::needs_auth(&u)));
            break;
        }
    }
    match found {
        Some((r, auth)) => eprintln!("attribute: match img host={} needs_auth={}", r, auth),
        None => eprintln!("attribute: no <img> in newest page"),
    }
}

#[test]
fn img_src_mining_shapes() {
    assert_eq!(
        first_img_src(r#"<p><img src="https://h/a.png"></p>"#).as_deref(),
        Some("https://h/a.png")
    );
    assert_eq!(
        first_img_src("<P><IMG SRC='https://h/b.png'></P>").as_deref(),
        Some("https://h/b.png")
    );
    assert_eq!(
        first_img_src("<p><img src=https://h/c.png></p>").as_deref(),
        Some("https://h/c.png")
    );
    // Non-images / missing src are skipped, first https img wins.
    assert_eq!(
        first_img_src(r#"<img alt="x"><img src="https://h/d.png">"#).as_deref(),
        Some("https://h/d.png")
    );
    assert_eq!(first_img_src("<p>no image</p>"), None);
    assert_eq!(first_img_src(r#"<img src="http://h/e.png">"#), None);
    // data-src must not match the src attribute scan.
    assert_eq!(
        first_img_src(r#"<img data-src="https://h/no.png">"#),
        None
    );
    assert_eq!(
        redact_url("https://amer.ng.msg.teams.microsoft.com/v1/objects/0-secret/views/imgo"),
        "amer.ng.msg.teams.microsoft.com/…/views/imgo"
    );
}
